//! Workspace automation. Run via `cargo run -p xtask -- <command>` or the
//! justfile front door.

use std::process::ExitCode;

const USAGE: &str = "xtask — Knowledge Plane workspace automation

Usage: cargo run -p xtask -- <command>

Commands: (none yet)";

fn main() -> ExitCode {
    println!("{USAGE}");
    ExitCode::SUCCESS
}
