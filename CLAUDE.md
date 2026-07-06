# Knowledge Plane

The substrate under a personal knowledge base: any plain-markdown vault +
one embedded index + MCP for agents + a deterministic librarian. Notes
stay plain files; agents propose, the human decides. Rust workspace,
pre-release.

## Repo map

| path | contents |
|---|---|
| `contracts/` | **the API** — the four published contracts (kp-note/v1, kp-config/v1, proposals/v1, mcp/v1), each with spec + CHANGELOG (+ JSON Schemas); `vendor/curio/` holds sha-pinned producer schemas |
| `crates/curator-core` | vault model, contract data types, identity, config loading, proposals validator |
| `crates/curator-index` | chunker, `Embedder` trait (`builtin` ONNX / `hash` test), one `index.db` (sqlite-vec + FTS5 + edges), blue/green epochs |
| `crates/curator-ingest` | producer adapters (Curio notes + events tail), ingest orchestration |
| `crates/curator-zotero` | read-only two-channel Zotero producer |
| `crates/curator-mcp` | the one MCP entrypoint — MCP surface v1 |
| `crates/curator-librarian` | deterministic zero-LLM digest baseline; LLM harness = optional prose enhancer |
| `crates/curator-cli` | the `curator` binary |
| `xtask/` | workspace automation — the grep litmus |
| `docs/design/` | architecture + decisions (the verdict-driven design record) |

Dependency direction is strictly downward:
`curator-cli → {curator-ingest, curator-zotero, curator-mcp, curator-librarian} → curator-index → curator-core`,
plus one same-tier edge: `curator-librarian → curator-ingest` (the Curio
ownership oracle + managed-region parser).

## Binding rules

- **Contract-first.** The four documents under `contracts/` are the API.
  Code conforms to `contracts/`, never vice versa — a mismatch is a code
  bug. Changing a contract follows `.claude/skills/contract-change/`
  (additive = minor, breaking = major + new spec file; v1 semantics are
  never edited after publication).
- **Hermetic tests.** No network, no model downloads, no external
  services — the deterministic `hash` embedder backs ALL embedding tests.
  CI runs on a clean runner with nothing but Rust; a test that needs a
  service is a bug.
- **Litmus doctrine.** This is a public product repo: zero references to
  any private reference-deployment — no LAN prefixes, no internal service
  names, no host topology. `just litmus` (also in CI and self-tested in
  `xtask/src/litmus.rs`, where the pattern set lives) fails the build on
  any hit. Where deployment choices exist, they are seams (traits,
  config), never named instances.
- **Justfile front door.** `just` lists everything: `setup`, `build`,
  `test`, `fmt`/`fmt-check`, `clippy`, `litmus`, `lint`, `doc`, `cov`,
  `ci`. Run `just ci` before pushing — it is exactly what CI runs.
- **Coverage gate.** `just cov` (in `ci`) enforces region coverage >= 80%
  on curator-core, curator-index, curator-librarian (report-only elsewhere) via
  `xtask coverage-gate`. Under the floor = write the missing tests;
  exclusion games are forbidden. curator-core carries `missing_docs`.
- **Conventional Commits** on `main` (commit-msg hook enforces).
  Workspace lints: `unsafe_code = "forbid"` (relax only if sqlite-vec FFI
  registration truly requires it, with a comment at the site).
- **License is deliberately TBD** — do not add a LICENSE file or a
  `license` field; that decision is the maintainer's alone.
