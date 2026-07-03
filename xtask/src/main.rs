//! Workspace automation. Run via `cargo run -p xtask -- <command>` or the
//! justfile front door (`just litmus`).

mod litmus;

use std::process::ExitCode;

const USAGE: &str = "xtask — Knowledge Plane workspace automation

Usage: cargo run -p xtask -- <command>

Commands:
  litmus [root]   scan the repo for banned private-infrastructure strings
                  (defaults to the workspace root); nonzero exit on any hit";

fn main() -> ExitCode {
    let mut args = std::env::args().skip(1);
    match args.next().as_deref() {
        Some("litmus") => litmus::run(args.next().as_deref()),
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
