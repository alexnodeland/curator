//! `kp` — the Knowledge Plane CLI.

use std::process::ExitCode;

/// Subcommands the v1 CLI will grow into (per `docs/design/architecture.md`).
const COMMANDS: [&str; 10] = [
    "init", "ingest", "reindex", "search", "mcp", "propose", "review", "apply", "digest", "status",
];

const USAGE: &str = "kp — the Knowledge Plane

Usage: kp <command> [args]

Commands:
  init      create kp.toml and the vault scaffolding
  ingest    run producer adapters (Curio, web clips) into the vault/index
  reindex   rebuild index.db (blue/green epoch)
  search    hybrid retrieval from the terminal
  mcp       serve the MCP surface (stdio default)
  propose   create a proposals/v1 changeset
  review    render a proposal for human review
  apply     validate and apply a proposal
  digest    run the deterministic librarian digest
  status    vault + index + proposals overview

Options:
  -h, --help       show this help
  -V, --version    show version";

fn main() -> ExitCode {
    let mut args = std::env::args().skip(1);
    match args.next().as_deref() {
        None | Some("--help" | "-h" | "help") => {
            println!("{USAGE}");
            ExitCode::SUCCESS
        }
        Some("--version" | "-V") => {
            println!("kp {}", env!("CARGO_PKG_VERSION"));
            ExitCode::SUCCESS
        }
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
