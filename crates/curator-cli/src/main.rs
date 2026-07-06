//! `curator` — the Knowledge Plane CLI.

use std::path::{Path, PathBuf};
use std::process::ExitCode;
use std::sync::Arc;

use curator_core::KpConfig;
use curator_index::{Embedder, embedder_from_config};
use curator_mcp::KpEngine;
use curator_mcp::types::{NoteKind, SearchMode};

/// Subcommands the v1 CLI grows into (per `docs/design/architecture.md`).
const COMMANDS: [&str; 17] = [
    "init",
    "ingest",
    "index",
    "reindex",
    "search",
    "get",
    "related",
    "recent",
    "mcp",
    "propose",
    "review",
    "apply",
    "proposals",
    "digest",
    "doctor",
    "status",
    "zotero",
];

const USAGE: &str = "curator — the Knowledge Plane

Usage: curator <command> [args]

Commands:
  init [dir]      scaffold a vault: kp.toml (from the example), .kp/, first index
  ingest          run producer adapters (Curio, web clips) into the vault/index
  index rebuild   rebuild index.db (blue/green epoch swap)
  reindex         alias for `index rebuild`
  zotero sync     two-channel Zotero sync into the vault (delta + fulltext)
  search          hybrid retrieval from the terminal
  get             one note by id (any namespace) — content + metadata
  related         embedding-nearest notes to a note
  recent          recently ingested/changed notes
  mcp serve       serve the MCP surface (stdio default; --http + bearer)
  propose         create a proposals/v1 changeset from a directory of files
  review <id>     render a proposal for human review
  apply <id>      validate and apply a proposal (stamps applied/rejected)
  proposals list  list stored proposals and their status
  digest run      run the deterministic librarian digest (--auto to apply)
  doctor          config / vault / index / cursors health
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

Options (digest run):
  --auto           auto-apply the digest proposal when the gate admits it
                   (pure additions under [librarian].digest_dir, kp:<uuidv7>)
  --now <rfc3339>  inject the clock (testing/reproducibility; default: now)

Options (propose):
  --title <t>      proposal title (required)
  --rationale <r>  why this change (default: empty)
  --author <a>     proposal author (default: curator-cli)
  --from <dir>     directory of generated files; every file maps to the
                   same vault-relative path (required)

Options (init):
  --embedder <e>   builtin | hash — stamped into the scaffolded kp.toml.
                   builtin fetches its pinned ~130 MB ONNX model on first
                   use (one-time, announced); hash is offline and
                   deterministic (no ML)

Options:
  -h, --help       show this help
  -V, --version    show version";

/// Render library `tracing` events to STDERR (stdout belongs to command
/// output — and to the MCP protocol under `curator mcp serve`). Without a
/// subscriber the contract-promised warnings (kp-config/v1: "unknown
/// keys warn, never fail"; skipped notes; malformed event lines) would
/// be silently dropped. Default level `warn`; `RUST_LOG` overrides.
fn init_tracing() {
    use tracing_subscriber::EnvFilter;
    let filter = EnvFilter::try_from_default_env().unwrap_or_else(|_| EnvFilter::new("warn"));
    tracing_subscriber::fmt()
        .with_env_filter(filter)
        .with_writer(std::io::stderr)
        .with_target(false)
        .init();
}

fn main() -> ExitCode {
    init_tracing();
    let args: Vec<String> = std::env::args().skip(1).collect();
    match args.first().map(String::as_str) {
        None | Some("--help" | "-h" | "help") => {
            println!("{USAGE}");
            ExitCode::SUCCESS
        }
        Some("--version" | "-V") => {
            println!("curator {}", env!("CARGO_PKG_VERSION"));
            ExitCode::SUCCESS
        }
        Some("ingest") => run_or_fail(cmd_ingest(&args[1..])),
        Some("index") => match args.get(1).map(String::as_str) {
            Some("rebuild") => run_or_fail(cmd_rebuild(&args[2..])),
            other => {
                eprintln!(
                    "curator index: unknown subcommand {other:?} — try `curator index rebuild`"
                );
                ExitCode::from(2)
            }
        },
        Some("reindex") => run_or_fail(cmd_rebuild(&args[1..])),
        Some("zotero") => match args.get(1).map(String::as_str) {
            Some("sync") => run_or_fail(cmd_zotero_sync(&args[2..])),
            other => {
                eprintln!(
                    "curator zotero: unknown subcommand {other:?} — try `curator zotero sync`"
                );
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
                eprintln!("curator mcp: unknown subcommand {other:?} — try `curator mcp serve`");
                ExitCode::from(2)
            }
        },
        Some("digest") => match args.get(1).map(String::as_str) {
            Some("run") => run_or_fail(cmd_digest_run(&args[2..])),
            other => {
                eprintln!(
                    "curator digest: unknown subcommand {other:?} — try `curator digest run`"
                );
                ExitCode::from(2)
            }
        },
        Some("propose") => run_or_fail(cmd_propose(&args[1..])),
        Some("review") => run_or_fail(cmd_review(&args[1..])),
        Some("apply") => run_or_fail(cmd_apply(&args[1..])),
        Some("proposals") => match args.get(1).map(String::as_str) {
            Some("list") => run_or_fail(cmd_proposals_list(&args[2..])),
            other => {
                eprintln!(
                    "curator proposals: unknown subcommand {other:?} — try `curator proposals list`"
                );
                ExitCode::from(2)
            }
        },
        Some("doctor") => run_or_fail(cmd_doctor(&args[1..])),
        Some("init") => run_or_fail(cmd_init(&args[1..])),
        Some(cmd) if COMMANDS.contains(&cmd) => {
            eprintln!("curator {cmd}: not implemented yet (pre-release scaffold)");
            ExitCode::from(2)
        }
        Some(other) => {
            eprintln!("curator: unknown command {other:?} — run `curator --help`");
            ExitCode::from(2)
        }
    }
}

fn run_or_fail(result: Result<(), String>) -> ExitCode {
    match result {
        Ok(()) => ExitCode::SUCCESS,
        Err(message) => {
            eprintln!("curator: {message}");
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
// Query commands — thin shells over curator-mcp's KpEngine, the SAME layer the
// MCP tools ride, so `curator search` and `kp_search` cannot drift apart.
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
        return Err("search needs a query — `curator search <query>`".to_owned());
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
        return Err("get needs exactly one id — `curator get <id>`".to_owned());
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
        return Err("related needs exactly one id — `curator related <id>`".to_owned());
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
                .block_on(curator_mcp::serve_stdio(engine))
                .map_err(|e| e.to_string())
        }
        "http" => {
            // Contract binding rule 4: no unauthenticated network mode —
            // resolve the bearer token BEFORE binding anything.
            let token =
                curator_mcp::resolve_bearer_token(&config.mcp).map_err(|e| e.to_string())?;
            let bind = config.mcp.http_bind.clone();
            let engine = Arc::new(KpEngine::from_config(config).map_err(|e| e.to_string())?);
            runtime
                .block_on(curator_mcp::serve_http(engine, &bind, &token))
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
    let report =
        curator_ingest::ingest(&batch.config, embedder.as_ref()).map_err(|e| e.to_string())?;
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
    let mut options = curator_zotero::SyncOptions::default();
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

    // The version cursor lives in curator-index — open the db, creating epoch 1
    // when this is the very first plane operation.
    let index_path = config.index_path();
    let mut index = if index_path.exists() {
        curator_index::Index::open(&index_path, embedder.as_ref()).map_err(|e| e.to_string())?
    } else {
        if let Some(parent) = index_path.parent() {
            std::fs::create_dir_all(parent).map_err(|e| e.to_string())?;
        }
        curator_index::Index::create(&index_path, embedder.as_ref(), 1)
            .map_err(|e| e.to_string())?
    };

    let report = curator_zotero::sync(&config, &mut index, &options).map_err(|e| e.to_string())?;
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

// ---------------------------------------------------------------------------
// Librarian: digest + proposals lifecycle
// ---------------------------------------------------------------------------

fn cmd_digest_run(args: &[String]) -> Result<(), String> {
    let mut config_path: Option<PathBuf> = None;
    let mut json = false;
    let mut auto = false;
    let mut now = curator_core::time::unix_now();
    let mut it = args.iter();
    while let Some(arg) = it.next() {
        match arg.as_str() {
            "--config" => {
                let value = it.next().ok_or("--config needs a path")?;
                config_path = Some(PathBuf::from(value));
            }
            "--json" => json = true,
            "--auto" => auto = true,
            "--now" => {
                let value = it.next().ok_or("--now needs an RFC 3339 UTC timestamp")?;
                now = curator_core::time::parse_rfc3339_utc(value)
                    .ok_or_else(|| format!("--now {value:?} is not RFC 3339 UTC (…Z)"))?;
            }
            other => return Err(format!("unknown argument {other:?}")),
        }
    }
    let config = load_config(config_path)?;
    let embedder = embedder_for(&config)?;
    let report = curator_librarian::run_digest(&config, embedder.as_ref(), now, auto)
        .map_err(|e| e.to_string())?;
    if json {
        return print_json(&report);
    }
    for warning in &report.warnings {
        eprintln!("warning: {warning}");
    }
    match &report.skipped {
        Some(reason) => println!("skipped: {reason}"),
        None => {
            let applied = if report.applied {
                ", auto-applied"
            } else {
                " (open — `curator apply` after review)"
            };
            println!(
                "digest {} → {}: {} surfaced, {} quiet of {} candidate(s); proposal {}{}",
                report.date,
                report.note_path,
                report.items,
                report.quiet,
                report.candidates,
                report.proposal_id.as_deref().unwrap_or("-"),
                applied,
            );
        }
    }
    Ok(())
}

fn cmd_propose(args: &[String]) -> Result<(), String> {
    let mut config_path: Option<PathBuf> = None;
    let mut json = false;
    let mut title: Option<String> = None;
    let mut rationale = String::new();
    let mut author = "curator-cli".to_owned();
    let mut from: Option<PathBuf> = None;
    let mut it = args.iter();
    while let Some(arg) = it.next() {
        match arg.as_str() {
            "--config" => {
                let value = it.next().ok_or("--config needs a path")?;
                config_path = Some(PathBuf::from(value));
            }
            "--json" => json = true,
            "--title" => title = Some(it.next().ok_or("--title needs a value")?.clone()),
            "--rationale" => rationale = it.next().ok_or("--rationale needs a value")?.clone(),
            "--author" => author = it.next().ok_or("--author needs a value")?.clone(),
            "--from" => from = Some(PathBuf::from(it.next().ok_or("--from needs a directory")?)),
            other => return Err(format!("unknown argument {other:?}")),
        }
    }
    let title = title.ok_or("propose needs --title")?;
    let from = from.ok_or("propose needs --from <dir> (files map to vault-relative paths)")?;
    let config = load_config(config_path)?;
    let vault = curator_core::Vault::open(config.vault_path()).map_err(|e| e.to_string())?;

    let files = collect_proposal_files(&from)?;
    if files.is_empty() {
        return Err(format!("{} contains no files", from.display()));
    }
    let proposal = curator_core::create_proposal(
        &vault,
        &config.vault.proposals_dir,
        &author,
        &title,
        &rationale,
        &files,
    )
    .map_err(|e| e.to_string())?;
    if json {
        return print_json(&proposal);
    }
    println!(
        "proposal {} created ({} file(s)) — `curator review {}`, then `curator apply {}`",
        proposal.id,
        proposal.files.len(),
        proposal.id,
        proposal.id
    );
    Ok(())
}

/// Every non-hidden file under `dir`, as `(vault-relative path, content)`,
/// path-sorted. The directory layout IS the proposed vault layout.
fn collect_proposal_files(dir: &Path) -> Result<Vec<curator_core::ProposalFile>, String> {
    let mut out = Vec::new();
    let mut stack = vec![dir.to_owned()];
    while let Some(current) = stack.pop() {
        let entries = std::fs::read_dir(&current)
            .map_err(|e| format!("cannot read {}: {e}", current.display()))?;
        for entry in entries {
            let entry = entry.map_err(|e| e.to_string())?;
            if entry.file_name().to_string_lossy().starts_with('.') {
                continue;
            }
            let path = entry.path();
            if path.is_dir() {
                stack.push(path);
            } else if path.is_file() {
                let rel = path
                    .strip_prefix(dir)
                    .expect("walk stays under dir")
                    .components()
                    .map(|c| c.as_os_str().to_string_lossy())
                    .collect::<Vec<_>>()
                    .join("/");
                let content = std::fs::read_to_string(&path)
                    .map_err(|e| format!("cannot read {} as UTF-8: {e}", path.display()))?;
                out.push(curator_core::ProposalFile { path: rel, content });
            }
        }
    }
    out.sort_by(|a, b| a.path.cmp(&b.path));
    Ok(out)
}

fn cmd_review(args: &[String]) -> Result<(), String> {
    let q = parse_query_args(args)?;
    let [id] = q.positional.as_slice() else {
        return Err("review needs exactly one id — `curator review <id>`".to_owned());
    };
    let config = load_config(q.config_path)?;
    let vault = curator_core::Vault::open(config.vault_path()).map_err(|e| e.to_string())?;
    let (proposal, patch) = curator_core::load_proposal(&vault, &config.vault.proposals_dir, id)
        .map_err(|e| e.to_string())?;
    print!("{}", curator_librarian::render_review(&proposal, &patch));
    Ok(())
}

fn cmd_apply(args: &[String]) -> Result<(), String> {
    let q = parse_query_args(args)?;
    let [id] = q.positional.as_slice() else {
        return Err("apply needs exactly one id — `curator apply <id>`".to_owned());
    };
    let config = load_config(q.config_path)?;
    let vault = curator_core::Vault::open(config.vault_path()).map_err(|e| e.to_string())?;
    let report = curator_librarian::apply_proposal(&vault, &config.vault.proposals_dir, id)
        .map_err(|e| e.to_string())?;
    if q.json {
        return print_json(&report);
    }
    println!(
        "applied {} ({}): {} file(s) written",
        report.id,
        report.title,
        report.files_written.len()
    );
    for file in &report.files_written {
        println!("  {file}");
    }
    Ok(())
}

fn cmd_proposals_list(args: &[String]) -> Result<(), String> {
    let q = parse_query_args(args)?;
    if !q.positional.is_empty() {
        return Err(format!("unexpected argument {:?}", q.positional[0]));
    }
    let config = load_config(q.config_path)?;
    let vault = curator_core::Vault::open(config.vault_path()).map_err(|e| e.to_string())?;
    let proposals = curator_core::list_proposals(&vault, &config.vault.proposals_dir)
        .map_err(|e| e.to_string())?;
    if q.json {
        return print_json(&proposals);
    }
    if proposals.is_empty() {
        println!("no proposals");
        return Ok(());
    }
    for p in &proposals {
        let status = serde_json::to_value(p.status)
            .ok()
            .and_then(|v| v.as_str().map(str::to_owned))
            .unwrap_or_default();
        println!(
            "{}  {:8}  {}  ({} file(s), by {})",
            p.id,
            status,
            p.title,
            p.files.len(),
            p.author
        );
    }
    Ok(())
}

// ---------------------------------------------------------------------------
// doctor + init
// ---------------------------------------------------------------------------

#[derive(serde::Serialize)]
struct DoctorCheck {
    check: &'static str,
    level: &'static str, // ok | warn | error
    message: String,
}

fn check(checks: &mut Vec<DoctorCheck>, check: &'static str, level: &'static str, message: String) {
    checks.push(DoctorCheck {
        check,
        level,
        message,
    });
}

fn cmd_doctor(args: &[String]) -> Result<(), String> {
    let batch = parse_batch_args(args)?;
    let config = batch.config;
    let mut checks: Vec<DoctorCheck> = Vec::new();
    check(
        &mut checks,
        "config",
        "ok",
        format!("schema {}", config.schema),
    );

    // Vault.
    let vault = match curator_core::Vault::open(config.vault_path()) {
        Ok(vault) => {
            let notes = vault.note_paths().map(|p| p.len()).unwrap_or(0);
            check(
                &mut checks,
                "vault",
                "ok",
                format!("{} — {notes} note(s)", vault.root().display()),
            );
            Some(vault)
        }
        Err(e) => {
            check(&mut checks, "vault", "error", e.to_string());
            None
        }
    };

    // Proposals queue.
    if let Some(vault) = &vault {
        match curator_core::list_proposals(vault, &config.vault.proposals_dir) {
            Ok(proposals) => {
                let open = proposals
                    .iter()
                    .filter(|p| p.status == curator_core::ProposalStatus::Open)
                    .count();
                check(
                    &mut checks,
                    "proposals",
                    if open > 0 { "warn" } else { "ok" },
                    format!("{} total, {open} open", proposals.len()),
                );
            }
            Err(e) => check(&mut checks, "proposals", "error", e.to_string()),
        }
        // The librarian's anchor.
        let now_path = &config.librarian.now_path;
        match vault.resolve(now_path) {
            Ok(p) if p.exists() => check(&mut checks, "now.md", "ok", now_path.clone()),
            _ => check(
                &mut checks,
                "now.md",
                "warn",
                format!("{now_path} missing — digests fall back to recency-only scoring"),
            ),
        }
    }

    // Index + embedder identity + cursors + digest log.
    let index_path = config.index_path();
    if !index_path.exists() {
        check(
            &mut checks,
            "index",
            "warn",
            format!("{} missing — run `curator ingest`", index_path.display()),
        );
    } else {
        match curator_index::IndexReader::open(&index_path) {
            Ok(reader) => {
                let meta = reader.meta().clone();
                let notes = reader.note_count().unwrap_or(0);
                check(
                    &mut checks,
                    "index",
                    "ok",
                    format!(
                        "epoch {} · schema v{} · {} ({} dims) · {notes} note(s)",
                        meta.epoch, meta.schema_version, meta.embedder_id, meta.dims
                    ),
                );
                match embedder_for(&config) {
                    Ok(embedder)
                        if embedder.id() != meta.embedder_id || embedder.dims() != meta.dims =>
                    {
                        check(
                            &mut checks,
                            "embedder",
                            "error",
                            format!(
                                "config says {} ({} dims) but the index was built by {} ({} dims) \
                                 — run `curator index rebuild`",
                                embedder.id(),
                                embedder.dims(),
                                meta.embedder_id,
                                meta.dims
                            ),
                        );
                    }
                    Ok(embedder) => check(
                        &mut checks,
                        "embedder",
                        "ok",
                        format!("{} ({} dims)", embedder.id(), embedder.dims()),
                    ),
                    Err(e) => check(&mut checks, "embedder", "error", e),
                }
                match reader.last_digest_entry() {
                    Ok(Some(entry)) => check(
                        &mut checks,
                        "digest",
                        "ok",
                        format!("latest {} ({})", entry.digest_date, entry.kp_id),
                    ),
                    Ok(None) => check(
                        &mut checks,
                        "digest",
                        "ok",
                        "none yet — `curator digest run`".to_owned(),
                    ),
                    Err(e) => check(&mut checks, "digest", "error", e.to_string()),
                }
                if config.curio.enabled {
                    let cursors = reader
                        .cursors_for(curator_ingest::EVENTS_CONSUMER)
                        .map(|c| c.len())
                        .unwrap_or(0);
                    let events_dir = config.curio_events_dir();
                    if events_dir.is_dir() {
                        check(
                            &mut checks,
                            "curio",
                            "ok",
                            format!("{} — {cursors} cursor(s)", events_dir.display()),
                        );
                    } else {
                        check(
                            &mut checks,
                            "curio",
                            "warn",
                            format!("[curio].events_dir {} missing", events_dir.display()),
                        );
                    }
                }
            }
            Err(e) => check(&mut checks, "index", "error", e.to_string()),
        }
    }
    if !config.curio.enabled {
        check(&mut checks, "curio", "ok", "disabled".to_owned());
    }

    // MCP transport sanity.
    match config.mcp.transport.as_str() {
        "stdio" => check(&mut checks, "mcp", "ok", "stdio".to_owned()),
        "http" => match config.mcp.bearer_token() {
            Some(_) => check(
                &mut checks,
                "mcp",
                "ok",
                format!("http on {} (bearer set)", config.mcp.http_bind),
            ),
            None => check(
                &mut checks,
                "mcp",
                "error",
                format!(
                    "transport http but ${} is unset — the server will refuse to start",
                    config.mcp.bearer_token_env
                ),
            ),
        },
        other => check(
            &mut checks,
            "mcp",
            "error",
            format!("unknown transport {other:?}"),
        ),
    }

    if batch.json {
        print_json(&checks)?;
    } else {
        for c in &checks {
            println!("{:5} {:9} {}", c.level, c.check, c.message);
        }
    }
    let errors = checks.iter().filter(|c| c.level == "error").count();
    if errors > 0 {
        return Err(format!("{errors} check(s) failed"));
    }
    Ok(())
}

fn cmd_init(args: &[String]) -> Result<(), String> {
    let mut dir = PathBuf::from(".");
    let mut embedder_name = "builtin".to_owned();
    let mut it = args.iter();
    while let Some(arg) = it.next() {
        match arg.as_str() {
            "--embedder" => {
                embedder_name = it.next().ok_or("--embedder needs builtin|hash")?.clone();
            }
            flag if flag.starts_with("--") => return Err(format!("unknown argument {flag:?}")),
            positional => dir = PathBuf::from(positional),
        }
    }
    std::fs::create_dir_all(&dir).map_err(|e| format!("cannot create {}: {e}", dir.display()))?;
    let dir = std::fs::canonicalize(&dir).map_err(|e| e.to_string())?;

    // kp.toml, scaffolded from the shipped example (comments included),
    // pointed at this vault. Never overwrites an existing config.
    let config_path = dir.join("kp.toml");
    if config_path.exists() {
        println!("kp.toml exists — leaving it untouched");
    } else {
        let content = include_str!("../../../curator.example.toml")
            .replace(
                "path = \"~/vault\"",
                &format!("path = \"{}\"", dir.display()),
            )
            .replace(
                "path = \"~/.local/share/kp/index.db\"",
                &format!("path = \"{}\"", dir.join(".kp/index.db").display()),
            )
            .replace(
                "embedder = \"builtin\"",
                &format!("embedder = \"{embedder_name}\""),
            );
        // Refuse to write a config this binary cannot load back.
        KpConfig::from_toml_str(&content).map_err(|e| e.to_string())?;
        std::fs::write(&config_path, content).map_err(|e| e.to_string())?;
        println!("created {}", config_path.display());
    }
    let config = KpConfig::load(&config_path).map_err(|e| e.to_string())?;

    // .kp/ scaffolding + a starter interest anchor.
    let proposals_dir = config.vault_path().join(&config.vault.proposals_dir);
    std::fs::create_dir_all(&proposals_dir).map_err(|e| e.to_string())?;
    println!("created {}", proposals_dir.display());
    let now_path = config.vault_path().join(&config.librarian.now_path);
    if !now_path.exists() {
        std::fs::write(
            &now_path,
            "# Now\n\nWhat you are focused on right now. The librarian scores new notes \
             against this note — keep it current.\n",
        )
        .map_err(|e| e.to_string())?;
        println!("created {}", now_path.display());
    }

    // First index: a full ingest (creates index.db, epoch 1).
    if config.index_path().exists() {
        println!("index exists — skipping first ingest (`curator ingest` to refresh)");
        return Ok(());
    }
    let embedder = embedder_for(&config)?;
    let report = curator_ingest::ingest(&config, embedder.as_ref()).map_err(|e| e.to_string())?;
    println!(
        "first index built: {} note(s) ingested ({} skipped, {} ignored)",
        report.ingested, report.skipped, report.ignored
    );
    Ok(())
}

fn cmd_rebuild(args: &[String]) -> Result<(), String> {
    let batch = parse_batch_args(args)?;
    let embedder = embedder_for(&batch.config)?;
    // curator-ingest's rebuild: the SAME corpus, identities, and chunks that
    // incremental ingest produces, blue/green-swapped as a new epoch.
    let report =
        curator_ingest::rebuild(&batch.config, embedder.as_ref()).map_err(|e| e.to_string())?;
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
