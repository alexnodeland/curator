//! Workspace automation. Run via `cargo run -p xtask -- <command>` or the
//! justfile front door (`just litmus`, `just site`).

mod coverage;
mod docs;
mod litmus;

use std::process::ExitCode;

const USAGE: &str = "xtask — Curator workspace automation

Usage: cargo run -p xtask -- <command>

Commands:
  litmus [root]          scan the repo for banned private-infrastructure strings
                         (defaults to the workspace root); nonzero exit on any hit
  coverage-gate <json>   enforce the region-coverage floor from a
                         `cargo llvm-cov report --json --summary-only` export
                         (gated: curator-core, curator-index, curator-librarian >= 80%)
  docs [root]            render docs/site/ into the deterministic static site at
                         target/site/ (defaults to the workspace root); nonzero
                         exit on a missing page, missing H1, or dangling link";

fn main() -> ExitCode {
    let mut args = std::env::args().skip(1);
    match args.next().as_deref() {
        Some("litmus") => litmus::run(args.next().as_deref()),
        Some("docs") => docs::run(args.next().as_deref()),
        Some("coverage-gate") => coverage::run(args.next().as_deref()),
        None | Some("--help" | "-h" | "help") => {
            println!("{USAGE}");
            ExitCode::SUCCESS
        }
        Some(other) => {
            eprintln!("xtask: unknown command {other:?}\n\n{USAGE}");
            ExitCode::from(2)
        }
    }
}
