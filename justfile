# Knowledge Plane — the command front door. `just` lists recipes.

# List available recipes
default:
    @just --list

# One-time dev setup: git hooks + toolchain components
setup:
    lefthook install
    rustup component add rustfmt clippy

# Build every workspace crate
build:
    cargo build --workspace

# Run the full hermetic test suite (no network, no models, no services)
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

# fmt-check + clippy + litmus
lint: fmt-check clippy litmus

# Coverage (requires cargo-llvm-cov: `cargo install cargo-llvm-cov`)
cov:
    cargo llvm-cov --workspace --summary-only

# Everything CI runs, in CI's order
ci: fmt-check clippy test litmus
