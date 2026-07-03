//! `kp` — the Knowledge Plane CLI.

use std::path::PathBuf;
use std::process::ExitCode;

use kp_core::KpConfig;
use kp_index::{Embedder, HashEmbedder};

/// Subcommands the v1 CLI grows into (per `docs/design/architecture.md`).
const COMMANDS: [&str; 11] = [
    "init", "ingest", "index", "reindex", "search", "mcp", "propose", "review", "apply", "digest",
    "status",
];

const USAGE: &str = "kp — the Knowledge Plane

Usage: kp <command> [args]

Commands:
  init            create kp.toml and the vault scaffolding
  ingest          run producer adapters (Curio, web clips) into the vault/index
  index rebuild   rebuild index.db (blue/green epoch swap)
  reindex         alias for `index rebuild`
  search          hybrid retrieval from the terminal
  mcp             serve the MCP surface (stdio default)
  propose         create a proposals/v1 changeset
  review          render a proposal for human review
  apply           validate and apply a proposal
  digest          run the deterministic librarian digest
  status          vault + index + proposals overview

Options (ingest / index rebuild):
  --config <path>  kp.toml location (default: $KP_CONFIG, then ./kp.toml)
  --json           machine-readable summary on stdout

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
    let config_path = config_path
        .or_else(|| std::env::var_os("KP_CONFIG").map(PathBuf::from))
        .unwrap_or_else(|| PathBuf::from("kp.toml"));
    let config = KpConfig::load(&config_path).map_err(|e| e.to_string())?;
    Ok(BatchArgs { config, json })
}

/// The embedder named by `[index].embedder`.
fn embedder_for(config: &KpConfig) -> Result<Box<dyn Embedder>, String> {
    match config.index.embedder.as_str() {
        "hash" => Ok(Box::new(HashEmbedder::default())),
        "builtin" => Ok(Box::new(kp_index::FastEmbedder::from_config(config))),
        other => Err(format!(
            "unknown [index].embedder {other:?} (expected \"builtin\" or \"hash\")"
        )),
    }
}

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
