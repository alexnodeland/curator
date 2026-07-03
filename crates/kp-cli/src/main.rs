//! `kp` — the Knowledge Plane CLI.

use std::path::PathBuf;
use std::process::ExitCode;
use std::sync::Arc;

use kp_core::KpConfig;
use kp_index::{Embedder, embedder_from_config};
use kp_mcp::KpEngine;
use kp_mcp::types::{NoteKind, SearchMode};

/// Subcommands the v1 CLI grows into (per `docs/design/architecture.md`).
const COMMANDS: [&str; 15] = [
    "init", "ingest", "index", "reindex", "search", "get", "related", "recent", "mcp", "propose",
    "review", "apply", "digest", "status", "zotero",
];

const USAGE: &str = "kp — the Knowledge Plane

Usage: kp <command> [args]

Commands:
  init            create kp.toml and the vault scaffolding
  ingest          run producer adapters (Curio, web clips) into the vault/index
  index rebuild   rebuild index.db (blue/green epoch swap)
  reindex         alias for `index rebuild`
  zotero sync     two-channel Zotero sync into the vault (delta + fulltext)
  search          hybrid retrieval from the terminal
  get             one note by id (any namespace) — content + metadata
  related         embedding-nearest notes to a note
  recent          recently ingested/changed notes
  mcp serve       serve the MCP surface (stdio default; --http + bearer)
  propose         create a proposals/v1 changeset
  review          render a proposal for human review
  apply           validate and apply a proposal
  digest          run the deterministic librarian digest
  status          vault + index + proposals overview

Options (ingest / index rebuild / zotero sync):
  --config <path>  kp.toml location (default: $KP_CONFIG, then ./kp.toml)
  --json           machine-readable summary on stdout

Options (zotero sync):
  --dir <path>          vault-relative notes dir (default: zotero)
  --no-fulltext         skip the fulltext pass
  --fulltext-cap <n>    fulltext truncation cap, characters (default: 20000)

Options (search / get / related / recent):
  --config <path>  kp.toml location (default: $KP_CONFIG, then ./kp.toml)
  --json           print the MCP-shaped JSON output
  --k <n>          result count (search, related; default 10)
  --mode <m>       search mode: hybrid | vector | fts (default hybrid)
  --days <n>       look-back window in days (recent; default 7)
  --kind <ns>      identity-namespace filter: curio | zotero | kp | path

Options (mcp serve):
  --config <path>  kp.toml location (default: $KP_CONFIG, then ./kp.toml)
  --http           streamable HTTP on [mcp].http_bind — REQUIRES the bearer
                   token env named by [mcp].bearer_token_env

Options:
  -h, --help       show this help
  -V, --version    show version";

fn main() -> ExitCode {
    let args: Vec<String> = std::env::args().skip(1).collect();
    match args.first().map(String::as_str) {
        None | Some("--help" | "-h" | "help") => {
            println!("{USAGE}");
            ExitCode::SUCCESS
        }
        Some("--version" | "-V") => {
            println!("kp {}", env!("CARGO_PKG_VERSION"));
            ExitCode::SUCCESS
        }
        Some("ingest") => run_or_fail(cmd_ingest(&args[1..])),
        Some("index") => match args.get(1).map(String::as_str) {
            Some("rebuild") => run_or_fail(cmd_rebuild(&args[2..])),
            other => {
                eprintln!("kp index: unknown subcommand {other:?} — try `kp index rebuild`");
                ExitCode::from(2)
            }
        },
        Some("reindex") => run_or_fail(cmd_rebuild(&args[1..])),
        Some("zotero") => match args.get(1).map(String::as_str) {
            Some("sync") => run_or_fail(cmd_zotero_sync(&args[2..])),
            other => {
                eprintln!("kp zotero: unknown subcommand {other:?} — try `kp zotero sync`");
                ExitCode::from(2)
            }
        },
        Some("search") => run_or_fail(cmd_search(&args[1..])),
        Some("get") => run_or_fail(cmd_get(&args[1..])),
        Some("related") => run_or_fail(cmd_related(&args[1..])),
        Some("recent") => run_or_fail(cmd_recent(&args[1..])),
        Some("mcp") => match args.get(1).map(String::as_str) {
            Some("serve") => run_or_fail(cmd_mcp_serve(&args[2..])),
            other => {
                eprintln!("kp mcp: unknown subcommand {other:?} — try `kp mcp serve`");
                ExitCode::from(2)
            }
        },
        Some(cmd) if COMMANDS.contains(&cmd) => {
            eprintln!("kp {cmd}: not implemented yet (pre-release scaffold)");
            ExitCode::from(2)
        }
        Some(other) => {
            eprintln!("kp: unknown command {other:?} — run `kp --help`");
            ExitCode::from(2)
        }
    }
}

fn run_or_fail(result: Result<(), String>) -> ExitCode {
    match result {
        Ok(()) => ExitCode::SUCCESS,
        Err(message) => {
            eprintln!("kp: {message}");
            ExitCode::FAILURE
        }
    }
}

fn load_config(config_path: Option<PathBuf>) -> Result<KpConfig, String> {
    let config_path = config_path
        .or_else(|| std::env::var_os("KP_CONFIG").map(PathBuf::from))
        .unwrap_or_else(|| PathBuf::from("kp.toml"));
    KpConfig::load(&config_path).map_err(|e| e.to_string())
}

/// Shared flags of the batch commands.
struct BatchArgs {
    config: KpConfig,
    json: bool,
}

fn parse_batch_args(args: &[String]) -> Result<BatchArgs, String> {
    let mut config_path: Option<PathBuf> = None;
    let mut json = false;
    let mut it = args.iter();
    while let Some(arg) = it.next() {
        match arg.as_str() {
            "--config" => {
                let value = it.next().ok_or("--config needs a path")?;
                config_path = Some(PathBuf::from(value));
            }
            "--json" => json = true,
            other => return Err(format!("unknown argument {other:?}")),
        }
    }
    let config = load_config(config_path)?;
    Ok(BatchArgs { config, json })
}

/// The embedder named by `[index].embedder`.
fn embedder_for(config: &KpConfig) -> Result<Box<dyn Embedder>, String> {
    embedder_from_config(config).map_err(|e| e.to_string())
}

// ---------------------------------------------------------------------------
// Query commands — thin shells over kp-mcp's KpEngine, the SAME layer the
// MCP tools ride, so `kp search` and `kp_search` cannot drift apart.
// ---------------------------------------------------------------------------

/// Flags shared by search / get / related / recent, plus positionals.
struct QueryArgs {
    config_path: Option<PathBuf>,
    json: bool,
    k: Option<u32>,
    mode: Option<SearchMode>,
    days: Option<u32>,
    kind: Option<NoteKind>,
    positional: Vec<String>,
}

fn parse_query_args(args: &[String]) -> Result<QueryArgs, String> {
    let mut out = QueryArgs {
        config_path: None,
        json: false,
        k: None,
        mode: None,
        days: None,
        kind: None,
        positional: Vec::new(),
    };
    let mut it = args.iter();
    while let Some(arg) = it.next() {
        match arg.as_str() {
            "--config" => {
                let value = it.next().ok_or("--config needs a path")?;
                out.config_path = Some(PathBuf::from(value));
            }
            "--json" => out.json = true,
            "--k" => {
                let value = it.next().ok_or("--k needs a number")?;
                out.k = Some(value.parse().map_err(|e| format!("--k {value:?}: {e}"))?);
            }
            "--mode" => {
                let value = it.next().ok_or("--mode needs hybrid|vector|fts")?;
                out.mode = Some(match value.as_str() {
                    "hybrid" => SearchMode::Hybrid,
                    "vector" => SearchMode::Vector,
                    "fts" => SearchMode::Fts,
                    other => return Err(format!("unknown --mode {other:?} (hybrid|vector|fts)")),
                });
            }
            "--days" => {
                let value = it.next().ok_or("--days needs a number")?;
                out.days = Some(
                    value
                        .parse()
                        .map_err(|e| format!("--days {value:?}: {e}"))?,
                );
            }
            "--kind" => {
                let value = it.next().ok_or("--kind needs curio|zotero|kp|path")?;
                out.kind = Some(match value.as_str() {
                    "curio" => NoteKind::Curio,
                    "zotero" => NoteKind::Zotero,
                    "kp" => NoteKind::Kp,
                    "path" => NoteKind::Path,
                    other => {
                        return Err(format!("unknown --kind {other:?} (curio|zotero|kp|path)"));
                    }
                });
            }
            flag if flag.starts_with("--") => return Err(format!("unknown argument {flag:?}")),
            positional => out.positional.push(positional.to_owned()),
        }
    }
    Ok(out)
}

fn engine_for(config_path: Option<PathBuf>) -> Result<KpEngine, String> {
    let config = load_config(config_path)?;
    KpEngine::from_config(config).map_err(|e| e.to_string())
}

fn print_json<T: serde::Serialize>(value: &T) -> Result<(), String> {
    println!(
        "{}",
        serde_json::to_string_pretty(value).map_err(|e| e.to_string())?
    );
    Ok(())
}

fn cmd_search(args: &[String]) -> Result<(), String> {
    let q = parse_query_args(args)?;
    if q.positional.is_empty() {
        return Err("search needs a query — `kp search <query>`".to_owned());
    }
    let query = q.positional.join(" ");
    let engine = engine_for(q.config_path)?;
    let out = engine
        .search(&query, q.k, q.mode)
        .map_err(|e| e.to_string())?;
    if q.json {
        return print_json(&out);
    }
    if out.results.is_empty() {
        println!("no hits ({} mode)", out.mode);
        return Ok(());
    }
    for hit in &out.results {
        println!(
            "{:>8.4}  {}  {} — {}",
            hit.score, hit.id, hit.title, hit.path
        );
    }
    Ok(())
}

fn cmd_get(args: &[String]) -> Result<(), String> {
    let q = parse_query_args(args)?;
    let [id] = q.positional.as_slice() else {
        return Err("get needs exactly one id — `kp get <id>`".to_owned());
    };
    let engine = engine_for(q.config_path)?;
    let out = engine.get_note(id).map_err(|e| e.to_string())?;
    if q.json {
        return print_json(&out);
    }
    println!("# {} ({})", out.title, out.id);
    println!("path: {}", out.path);
    if !out.frontmatter.tags.is_empty() {
        println!("tags: {}", out.frontmatter.tags.join(", "));
    }
    if let Some(source) = &out.frontmatter.source {
        println!("source: {source}");
    }
    println!("ingested: {}", out.index.ingested_at);
    println!();
    println!("{}", out.content);
    Ok(())
}

fn cmd_related(args: &[String]) -> Result<(), String> {
    let q = parse_query_args(args)?;
    let [id] = q.positional.as_slice() else {
        return Err("related needs exactly one id — `kp related <id>`".to_owned());
    };
    let engine = engine_for(q.config_path)?;
    let out = engine.related(id, q.k).map_err(|e| e.to_string())?;
    if q.json {
        return print_json(&out);
    }
    if out.results.is_empty() {
        println!("no related notes");
        return Ok(());
    }
    for hit in &out.results {
        println!(
            "{:>8.4}  {}  {} — {}",
            hit.score, hit.id, hit.title, hit.path
        );
    }
    Ok(())
}

fn cmd_recent(args: &[String]) -> Result<(), String> {
    let q = parse_query_args(args)?;
    if !q.positional.is_empty() {
        return Err(format!("unexpected argument {:?}", q.positional[0]));
    }
    let engine = engine_for(q.config_path)?;
    let out = engine.recent(q.days, q.kind).map_err(|e| e.to_string())?;
    if q.json {
        return print_json(&out);
    }
    if out.notes.is_empty() {
        println!("nothing ingested in the last {} day(s)", out.days);
        return Ok(());
    }
    for note in &out.notes {
        println!(
            "{}  {}  {} — {}",
            note.ingested_at, note.id, note.title, note.path
        );
    }
    Ok(())
}

// ---------------------------------------------------------------------------
// MCP serving
// ---------------------------------------------------------------------------

fn cmd_mcp_serve(args: &[String]) -> Result<(), String> {
    let mut config_path: Option<PathBuf> = None;
    let mut http = false;
    let mut it = args.iter();
    while let Some(arg) = it.next() {
        match arg.as_str() {
            "--config" => {
                let value = it.next().ok_or("--config needs a path")?;
                config_path = Some(PathBuf::from(value));
            }
            "--http" => http = true,
            other => return Err(format!("unknown argument {other:?}")),
        }
    }
    let config = load_config(config_path)?;
    let transport = if http {
        "http"
    } else {
        config.mcp.transport.as_str()
    };
    let runtime = tokio::runtime::Runtime::new().map_err(|e| e.to_string())?;
    match transport {
        "stdio" => {
            let engine = Arc::new(KpEngine::from_config(config).map_err(|e| e.to_string())?);
            runtime
                .block_on(kp_mcp::serve_stdio(engine))
                .map_err(|e| e.to_string())
        }
        "http" => {
            // Contract binding rule 4: no unauthenticated network mode —
            // resolve the bearer token BEFORE binding anything.
            let token = kp_mcp::resolve_bearer_token(&config.mcp).map_err(|e| e.to_string())?;
            let bind = config.mcp.http_bind.clone();
            let engine = Arc::new(KpEngine::from_config(config).map_err(|e| e.to_string())?);
            runtime
                .block_on(kp_mcp::serve_http(engine, &bind, &token))
                .map_err(|e| e.to_string())
        }
        other => Err(format!(
            "unknown [mcp].transport {other:?} (expected \"stdio\" or \"http\")"
        )),
    }
}

// ---------------------------------------------------------------------------
// Batch commands (ingest / rebuild / zotero)
// ---------------------------------------------------------------------------

fn cmd_ingest(args: &[String]) -> Result<(), String> {
    let batch = parse_batch_args(args)?;
    let embedder = embedder_for(&batch.config)?;
    let report = kp_ingest::ingest(&batch.config, embedder.as_ref()).map_err(|e| e.to_string())?;
    if batch.json {
        println!(
            "{}",
            serde_json::to_string_pretty(&report).map_err(|e| e.to_string())?
        );
    } else {
        println!(
            "ingested {} note(s) ({} unchanged, {} skipped, {} ignored, {} removed), {} link(s)",
            report.ingested,
            report.unchanged,
            report.skipped,
            report.ignored,
            report.removed,
            report.links,
        );
        if let Some(events) = &report.events {
            println!(
                "events: {} folded, {} duplicate(s), {} malformed across {} file(s)",
                events.folded, events.duplicates, events.malformed, events.files
            );
        }
    }
    Ok(())
}

fn cmd_zotero_sync(args: &[String]) -> Result<(), String> {
    // zotero sync takes the batch flags plus its own knobs, so it parses
    // its argument list itself.
    let mut config_path: Option<PathBuf> = None;
    let mut json = false;
    let mut options = kp_zotero::SyncOptions::default();
    let mut it = args.iter();
    while let Some(arg) = it.next() {
        match arg.as_str() {
            "--config" => {
                let value = it.next().ok_or("--config needs a path")?;
                config_path = Some(PathBuf::from(value));
            }
            "--json" => json = true,
            "--dir" => {
                let value = it.next().ok_or("--dir needs a vault-relative path")?;
                options.notes_dir = value.clone();
            }
            "--no-fulltext" => options.fulltext = false,
            "--fulltext-cap" => {
                let value = it.next().ok_or("--fulltext-cap needs a number")?;
                options.fulltext_max_chars = value
                    .parse()
                    .map_err(|e| format!("--fulltext-cap {value:?}: {e}"))?;
            }
            other => return Err(format!("unknown argument {other:?}")),
        }
    }
    let config = load_config(config_path)?;
    let embedder = embedder_for(&config)?;

    // The version cursor lives in kp-index — open the db, creating epoch 1
    // when this is the very first plane operation.
    let index_path = config.index_path();
    let mut index = if index_path.exists() {
        kp_index::Index::open(&index_path, embedder.as_ref()).map_err(|e| e.to_string())?
    } else {
        if let Some(parent) = index_path.parent() {
            std::fs::create_dir_all(parent).map_err(|e| e.to_string())?;
        }
        kp_index::Index::create(&index_path, embedder.as_ref(), 1).map_err(|e| e.to_string())?
    };

    let report = kp_zotero::sync(&config, &mut index, &options).map_err(|e| e.to_string())?;
    index.close().map_err(|e| e.to_string())?;

    if json {
        println!(
            "{}",
            serde_json::to_string_pretty(&report).map_err(|e| e.to_string())?
        );
        return Ok(());
    }
    if !report.enabled {
        println!(
            "zotero sync disabled: {}",
            report.disabled_reason.as_deref().unwrap_or("(no reason)")
        );
        return Ok(());
    }
    if report.not_modified {
        println!(
            "library unchanged at version {}",
            report.version_after.unwrap_or_default()
        );
        return Ok(());
    }
    println!(
        "synced to library version {} ({} fetched): {} upserted, {} unchanged, {} skipped; \
         fulltext {} added / {} missing; tombstones {} ({} deleted, {} trashed)",
        report.version_after.unwrap_or_default(),
        report.fetched,
        report.upserted,
        report.unchanged,
        report.skipped,
        report.fulltext_added,
        report.fulltext_missing,
        report.tombstones,
        report.deleted_files,
        report.trashed_files,
    );
    for warning in &report.warnings {
        eprintln!("warning: {warning}");
    }
    Ok(())
}

fn cmd_rebuild(args: &[String]) -> Result<(), String> {
    let batch = parse_batch_args(args)?;
    let embedder = embedder_for(&batch.config)?;
    // kp-ingest's rebuild: the SAME corpus, identities, and chunks that
    // incremental ingest produces, blue/green-swapped as a new epoch.
    let report = kp_ingest::rebuild(&batch.config, embedder.as_ref()).map_err(|e| e.to_string())?;
    if batch.json {
        println!(
            "{}",
            serde_json::to_string_pretty(&report).map_err(|e| e.to_string())?
        );
    } else {
        println!(
            "epoch {} built: {} note(s) indexed, {} skipped, {} ignored, {} link(s)",
            report.epoch, report.notes_indexed, report.notes_skipped, report.ignored, report.links
        );
        if let Some(events) = &report.events {
            println!(
                "events: {} folded, {} duplicate(s), {} malformed across {} file(s)",
                events.folded, events.duplicates, events.malformed, events.files
            );
        }
    }
    Ok(())
}
