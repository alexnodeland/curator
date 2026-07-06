# Curator — the command front door. `just` lists recipes.

# List available recipes
default:
    @just --list

# One-time dev setup: git hooks + toolchain components + coverage tooling
setup:
    lefthook install
    rustup component add rustfmt clippy llvm-tools-preview
    cargo llvm-cov --version >/dev/null 2>&1 || cargo install cargo-llvm-cov --locked

# Build every workspace crate
build:
    cargo build --workspace

# Run the full hermetic test suite: no external network (curator-zotero's
# wiremock tests bind loopback sockets only), no model downloads, no
# services
test:
    cargo test --workspace

# Format the whole workspace
fmt:
    cargo fmt --all

# Check formatting without writing
fmt-check:
    cargo fmt --all -- --check

# Clippy, warnings are errors
clippy:
    cargo clippy --workspace --all-targets -- -D warnings

# Scan the repo for banned private-infrastructure strings
litmus:
    cargo run -p xtask -- litmus

# The lean build stays buildable: `curator` without the ONNX embedder stack
# (no build-time ort download, no run-time model fetch)
lean-check:
    cargo check -p curator-cli --no-default-features

# fmt-check + clippy + litmus
lint: fmt-check clippy litmus

# API docs, warnings are errors (curator-core also gates missing_docs)
doc:
    RUSTDOCFLAGS="-D warnings" cargo doc --workspace --no-deps

# The docs site: render docs/site/ into the deterministic static site at
# target/site/ (also a gate — dangling links and missing anchors fail)
site:
    cargo run -p xtask -- docs

# Build the docs site and open it locally
site-open: site
    open target/site/index.html

# Coverage: one instrumented hermetic test run, then the region gate —
# fail-under 80% on curator-core/curator-index/curator-librarian, report-only elsewhere.
# Requires cargo-llvm-cov (`just setup` installs it).
cov:
    cargo llvm-cov --workspace --no-report
    cargo llvm-cov report --summary-only
    cargo llvm-cov report --json --summary-only --output-path target/coverage.json
    cargo run -p xtask -- coverage-gate target/coverage.json

# The full walk-through, offline: scratch-copy examples/sample-vault,
# init with the deterministic hash embedder (no ML, no downloads),
# ingest, search, digest. Non-interactive; target/demo is disposable.
demo:
    #!/usr/bin/env bash
    set -euo pipefail
    scratch=target/demo
    rm -rf "$scratch"
    mkdir -p "$scratch"
    cp -R examples/sample-vault "$scratch/vault"
    run() { cargo run --quiet -p curator-cli --no-default-features -- "$@"; }
    cfg="$scratch/vault/curator.toml"
    echo "== curator init (hash embedder: offline, deterministic) =="
    run init "$scratch/vault" --embedder hash
    echo
    echo "== curator search \"hybrid retrieval\" =="
    run search "hybrid retrieval" --config "$cfg" --k 3
    echo
    echo "== curator search \"dialing in espresso\" --mode fts =="
    run search "dialing in espresso" --config "$cfg" --mode fts --k 3
    echo
    echo "== curator digest run (the librarian, zero LLM) =="
    run digest run --config "$cfg"
    echo
    echo "== curator proposals list (digests are proposals — humans apply) =="
    run proposals list --config "$cfg"
    echo
    echo "== curator doctor =="
    run doctor --config "$cfg"
    echo
    echo "demo vault: $scratch/vault (config inside; rerun with 'just demo')"

# Everything CI runs, in CI's order
ci: fmt-check clippy test doc litmus lean-check site cov
