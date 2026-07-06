# Contributing

Pre-release: the project is not yet accepting external contributions —
this document exists so the workflow is public from day one.

## Ground rules

- **Contract-first.** The four documents under [`contracts/`](contracts/)
  are the API. Code conforms to contracts, never vice versa; contract
  changes follow the versioning discipline in
  [contracts/README.md](contracts/README.md) and land spec-first.
- **Hermetic tests.** `cargo test --workspace` must pass with no network,
  no model downloads, no external services. Embedding-shaped tests use
  the deterministic `hash` embedder; producer tests use checked-in
  fixtures.
- **Gates are the review.** `just ci` (fmt, clippy with warnings denied,
  tests, rustdoc with warnings denied, the grep litmus, the coverage
  gate — region coverage >= 80% on curator-core/curator-index/curator-librarian — and
  the cargo-deny license audit) must be green before any commit is
  proposed. Pre-commit hooks enforce the fast half locally: `just setup`
  once (it also installs `cargo-llvm-cov` and `cargo-deny`). A failing
  coverage gate means missing tests — never exclusions; a failing
  license audit means a copyleft dependency — never a routine allow.
- **Conventional Commits** — `type(scope)?: summary`, types:
  feat fix docs refactor chore ci test perf build revert.
- **Public-safety litmus.** This repo describes a product, never a
  deployment: no private hostnames, LAN addresses, or internal service
  names anywhere. `just litmus` fails the build on a hit.
- **Rust.** Workspace lints apply: `unsafe_code = "forbid"` everywhere
  except curator-index, which scopes it to `deny` for exactly one documented
  `#[allow]` site (sqlite-vec's `sqlite3_auto_extension` FFI
  registration — see `crates/curator-index/src/db.rs`);
  `cargo fmt` formatting is canonical.

## Workflow

1. Branch from `main`.
2. Make the change (spec-first if it touches a contract).
3. `just ci` green.
4. Open a PR with a clear why/what description.

## License note

The license is deliberately undecided (see README). By the nature of the
pre-release phase, do not add license headers, a LICENSE file, or
`license` metadata.
