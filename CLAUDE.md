# Curator

Curator — the Knowledge Plane: the substrate under a personal knowledge base: any plain-markdown vault +
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
| `xtask/` | workspace automation — the grep litmus, the coverage gate, the docs-site generator |
| `examples/` | the integration story, runnable: `sample-vault/` (12 notes, used by `just demo` + `just e2e-real`), `rss-to-notes.sh` (a complete shell producer), `compose/` (container config + env template) |
| `docs/design/` | architecture + decisions (the verdict-driven design record) |
| `docs/site/` | the public docs-site sources (markdown pages + `nav.json` + vendored assets); `just site` renders them deterministically into `target/site/`, `.github/workflows/pages.yml` deploys to GitHub Pages on push to main |
| `Dockerfile`, `compose.yaml` | the container deployment: multi-stage build → slim non-root runtime; compose profiles `core` \| `zotero` \| `librarian` (config+vault bind mounts, state volume, secrets env-only) |
| `deny.toml` | the license audit config (`just deny`): permissive-only allow-list, one scoped MPL-2.0 exception (`option-ext`) |

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
  service is a bug. The ONE deliberate exception lives outside the
  suite: `.github/workflows/e2e-real.yml` runs `just e2e-real` (real
  ONNX model, cached) weekly and as the release blocker — never
  per-commit.
- **Litmus doctrine.** This is a public product repo: zero references to
  any private reference-deployment — no LAN prefixes, no internal service
  names, no host topology. `just litmus` (also in CI and self-tested in
  `xtask/src/litmus.rs`, where the pattern set lives) fails the build on
  any hit. Where deployment choices exist, they are seams (traits,
  config), never named instances.
- **Justfile front door.** `just` lists everything: `setup`, `build`,
  `test`, `fmt`/`fmt-check`, `clippy`, `litmus`, `lint`, `deny`, `doc`,
  `site`, `cov`, `ci`, plus `demo` (offline sample-vault walk-through)
  and `e2e-real` (real-model loop; weekly + release-gating in CI). Run
  `just ci` before pushing — it is exactly what CI's gate jobs run
  (only the gitleaks secret scan is CI-side-only). The docs site is a
  gate too: `just site` (in `ci`) fails on dangling links, missing
  anchors, or a page without an H1.
- **Coverage gate.** `just cov` (in `ci`) enforces region coverage >= 80%
  on curator-core, curator-index, curator-librarian (report-only elsewhere) via
  `xtask coverage-gate`. Under the floor = write the missing tests;
  exclusion games are forbidden. curator-core carries `missing_docs`.
- **Conventional Commits** on `main` (commit-msg hook enforces).
  Workspace lints: `unsafe_code = "forbid"` (relax only if sqlite-vec FFI
  registration truly requires it, with a comment at the site).
- **License is [MIT](LICENSE)** (maintainer's decision, 2026) — the
  top-level `LICENSE` + `license = "MIT"` workspace metadata cover the tree;
  no per-file headers.
