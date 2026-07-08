# Curator — the command front door. `just` lists recipes.

# List available recipes
default:
    @just --list

# One-time dev setup: git hooks + toolchain components + coverage tooling
setup:
    lefthook install
    rustup component add rustfmt clippy llvm-tools-preview
    cargo llvm-cov --version >/dev/null 2>&1 || cargo install cargo-llvm-cov --locked
    cargo deny --version >/dev/null 2>&1 || cargo install cargo-deny --locked

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

# License audit: the shipped dependency tree stays permissive-only
# (allow-list + the one scoped exception live in deny.toml)
deny:
    cargo deny check licenses

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
    echo "== the digest above is an OPEN proposal — now review it interactively: =="
    echo "   cargo run -p curator-cli --no-default-features -- review --config $cfg"
    echo "   (or, once installed: curator review --config $cfg)"
    echo
    echo "demo vault: $scratch/vault (config inside; rerun with 'just demo')"

# The REAL end-to-end loop — the release blocker: builtin ONNX
# embedder, real model, ingest -> search must hit. First run fetches
# the pinned ~130 MB model into target/e2e-real/state/models (kept
# across runs; CI caches it), the index is rebuilt fresh every run.
e2e-real:
    #!/usr/bin/env bash
    set -euo pipefail
    state="$PWD/target/e2e-real/state"
    scratch="$PWD/target/e2e-real/run"
    rm -rf "$scratch"
    rm -f "$state"/index.db*
    mkdir -p "$scratch" "$state"
    cp -R examples/sample-vault "$scratch/vault"
    cargo build --release -p curator-cli
    bin=target/release/curator
    cfg="$scratch/curator.toml"
    printf '%s\n' \
        'schema = "kp-config/v1"' \
        '[vault]' "path = \"$scratch/vault\"" \
        '[index]' "path = \"$state/index.db\"" 'embedder = "builtin"' \
        > "$cfg"
    echo "== ingest (builtin ONNX embedder) =="
    "$bin" ingest --config "$cfg" --json
    echo "== search must return semantically-ranked hits =="
    "$bin" search "getting the espresso grinder setting right" --config "$cfg" --json \
        > "$scratch/search.json"
    python3 - "$scratch/search.json" <<'EOF'
    import json, sys
    out = json.load(open(sys.argv[1]))
    results = out["results"]
    assert results, "real-embedder search returned no hits"
    top = results[0]
    print(f"top hit: {top['id']}  (score {top['score']:.4f})")
    assert "espresso" in top["path"], f"expected the espresso note on top, got {top['path']}"
    EOF
    echo "== digest + doctor stay healthy on a real index =="
    "$bin" digest run --config "$cfg"
    "$bin" doctor --config "$cfg"
    echo "e2e-real: PASS"

# Everything CI's gate jobs run: the ci job's steps in its order, plus
# the license audit (its own job in ci.yml). The secret scan
# (gitleaks) and the weekly real-model e2e run CI-side only.
ci: fmt-check clippy test doc litmus lean-check site cov deny
