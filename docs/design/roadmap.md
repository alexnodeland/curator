# Curator — roadmap

> **Status:** the implementation has **landed through Phase 4 — v1 is complete**,
> and Phase 5 (Curio native integration) is in as of Curio's own v0.1.0. Phase 6
> (OSS hardening + public launch) is underway with this release. The phase plan
> below is the design record the build was measured against, not a to-do list;
> the per-phase text is preserved as that record. Where an early draft
> ([research/roadmap.json](research/roadmap.json)) predates later rulings —
> notably the language re-decision to **Rust** and the final contract shapes —
> this document and the published contracts win. Instance-only items (the
> maintainer's private reference deployment) are marked *(instance-side)*; they
> are configuration of that deployment, never product code.

## Phase overview

| phase | name | gate to enter | status |
|---|---|---|---|
| 0 | Decisions, contracts, scaffold | — | ✅ landed |
| 1 | Spine: vault, MCP reads, proposals | — | ✅ landed |
| 2 | Ingestion: Zotero two-channel + web clips | Phase 1 | ✅ landed |
| 3 | Index + retrieval | measured resource entry gate | ✅ landed |
| 4 | Librarian + deterministic digest (v1 complete) | Phase 3 | ✅ **landed — v1 complete** |
| 5 | Curio native integration | Curio ships released contracts | ✅ landed (adapter + events) |
| 6 | OSS hardening + public launch | ≥8 weeks instance survival on tagged code; name + license decided | 🚧 in progress (this release) |
| 7 | Post-launch menu | per-item gates | menu |

## Phase 0 — Decisions, contracts, scaffold *(landed)*

**Goal:** lock the irreversible choices, file the time-critical Curio schema review
upstream, and stand up a public-safe repo whose history is publishable by construction
from commit one.

- Curio schema review filed against the Curio repo (negation events, ULID `event_id`,
  tags-in-payload, `ts`/cursor/rotation spec, stable schema `$id`s, private-network
  allowlist semantics, managed-region marker, manifest write-ordering) — the only
  genuinely calendar-critical deliverable, due before Curio freezes its contracts.
- The four contract drafts: `kp-note/v1`, `kp-config/v1`, `proposals/v1`, MCP surface v1.
- Fresh product repo scaffold: **Rust cargo workspace** — six library crates
  (`curator-core`, `curator-ingest`, `curator-index`, `curator-zotero`, `curator-mcp`, `curator-librarian`) plus the
  `curator` binary; justfile, hooks, `curator.example.toml`; the CI grep litmus (no
  private-instance references in product code or contracts) enforced from commit one.
- Curio draft schemas vendored as sha-pinned fixtures, labeled draft.
- Zotero official `/fulltext` API verified against a real item **before** any WebDAV
  shim code exists — the verification decides the shim's (demoted, fallback-only) role.
- Reference-deployment prep *(instance-side)*: attachment-store host, vault remote.

**Exit:** schema-review issues live upstream; grep litmus green; `/fulltext` go/no-go
recorded; contracts drafted; repo scaffold committed.
**Size:** 1–2 weeks calendar. **OSS state:** nothing runnable; a stranger could read
the contracts and architecture docs.

## Phase 1 — Spine: vault, MCP reads, proposals *(landed)*

**Goal:** any markdown vault becomes agent-addressable — reads via MCP, writes only via
proposals — with the full safety model working **forge-free** (the OSS default) and
forge-hardened (optional tier).

- `curator-core`: `kp.toml` load/validate per `kp-config/v1` (env overrides, unknown keys
  warn), `kp-note/v1` frontmatter parse, atomic tmp+rename writes, dual hashing
  (contract body-checksum as opaque change token + internal full-file hash for
  reindex), `kp:<uuidv7>` identity minting, `.curio/manifest.json` ownership reader.
- MCP surface v1 vault reads (`kp_get_note`, `kp_recent`) and `kp_propose` from the one
  `curator-mcp` entrypoint — stdio default, streamable HTTP + bearer token optional.
- Local disposal flow as the primary safety model: `curator propose` / `curator review` /
  `curator apply` running **one importable deterministic validator** (schema, path
  allowlist, ownership-oracle refusal of `.curio/**` and managed regions,
  duplicate/absent-identity rejection, clean patch application). The same module runs
  as an optional forge CI hardening tier *(instance-side deployment of that tier)*.
- Vault mirror + branch-protection ritual *(instance-side)*.

**Exit:** an MCP client lists/reads/searches the vault; a proposal round-trips
propose → validate → human apply on a laptop with **no git remote**; the validator
fixture suite (ownership violations, duplicate identity, conflict markers) passes.
**Size:** 2–3 weeks focused. **OSS state:** point `curator` at any markdown directory and
get the vault MCP surface plus local propose/review/apply — zero forge, zero LLM,
zero Zotero.

## Phase 2 — Ingestion: Zotero two-channel + web clips

**Goal:** a Zotero library flows into the vault as literature stubs with correct
identity, deletion, and race handling.

- `curator-zotero` metadata channel: Web API delta polling (`Last-Modified-Version`
  etiquette, backoff) into a disposable local cache; `/deleted?since` consumed each
  cycle — tombstones drop index rows and raise an orphaned-note **proposal**, never an
  auto-delete. Metadata-backend seam defined in config (Web API now; local-database
  backend post-v1).
- Fulltext: official `/fulltext` endpoint primary; ~30-line CRC-verified `.prop`/`.zip`
  WebDAV fallback for self-hosted attachment stores. Attachment-backend seam.
- Literature stubs via proposals keyed strictly on `zotero:<itemKey>`; citekey is a
  filename/wikilink handle only — a citekey change emits a **rename proposal**, never a
  duplicate stub; metadata-before-attachment races handled (pending-retry,
  fulltext-present flag, index-invalidate on arrival).
- Web-clip producer (`kp:<uuidv7>` identity); PDF/OCR extraction via
  subprocess-isolated external tools — never in the dependency tree.
- Zotero annotations mirrored into a KP-owned *companion* note (regenerable,
  proposal-landed) — no writes into human-grown note bodies.
- Zotero access is **read-only at v1**: the API key carries no write scope.

**Exit:** full-library poll yields a valid stub proposal per item; deletion produces
tombstone + orphan proposal within one cycle; citekey re-pin yields a rename with zero
duplicates. All tests fixture-driven — no network.
**Size:** 2–3 weeks focused.

## Phase 3 — Index + retrieval

**Goal:** hybrid retrieval over the one embedded store, operable by
rebuild-not-migrate, entered only through a measured resource gate.

- **Entry gate (blocking):** measured throughput numbers (extraction docs/hour,
  embedding chunks/sec on the pinned model) on the reference host, with go/no-go
  thresholds; the named fallback is reduced cadence/scope, never new hardware
  *(instance-side measurement; the thresholds document the product's honest floor)*.
- `curator-index`: one `index.db` — sqlite-vec vectors + FTS5 BM25 + relational edge tables
  (recursive CTEs for one-hop expansion, supports/refutes); WAL, multi-process-safe.
  Chunk-level content hashes so unchanged chunks skip re-embedding across rebuilds.
- `Embedder` trait: **`builtin`** in-process pinned CPU ONNX model as default,
  **`hash`** deterministic embedder backing all tests; OpenAI-compatible remote
  endpoints stay a documented post-v1 seam.
- Incremental reindex driven by full-file-hash comparison — frontmatter-only edits
  update metadata rows without re-embedding; conflict-markered files are
  skipped-and-alerted.
- **Blue/green epochs:** rebuild into `index.db.next`, completeness check, atomic
  rename; epoch keyed *only* on (embedding model + dims, chunker version,
  normalization version) — software version bumps never invalidate; a mid-rebuild
  crash leaves the serving epoch intact.
- Retrieval tools land on the MCP surface: `kp_search` (hybrid RRF fusion over vector +
  FTS legs, one-hop edge expansion, per-leg score breakdown), `kp_related`; citations
  are path + heading + checksum. In-process retrieval — no internal network API.
- MCP DX: client install helpers, token mint/list/revoke, secure-by-default localhost
  binds.

**Exit:** the full single-box path (ingest → index → `kp_search`) runs on a clean
machine with **no external services** — in-process embedder only; embedding-model swap
completes blue/green with zero retrieval downtime; `kill -9` mid-rebuild is harmless.
**Size:** 3–4 weeks focused. Phases 1–3 are the personal-instance spine.

## Phase 4 — Librarian + deterministic digest *(completes v1)*

**Goal:** the librarian drains the inbox and emits a useful digest with **zero LLM
required**; an agent harness rides on top as an optional enhancer.

- Deterministic maintenance jobs (ingest routing → extraction → embed →
  dedupe-by-threshold → stub/companion proposals); the librarian consults open
  proposals + the index before proposing — no duplicate stubs, idempotent re-runs.
- Baseline digest v0 (pure code): new-since-last-digest candidates ranked by
  `cosine(note, now.md anchor) × exp(−age/half-life)`, grouped, rendered with links +
  extractive one-line summaries into the digest directory via a proposal; digests are
  **create-only** in the validator and idempotent by date; `kp_digest_latest` serves
  the newest one.
- Harness seam: any agent producing conforming proposals is a valid librarian;
  `harness = none` is a first-class supported mode *(the reference instance runs a
  Claude Code harness under standing orders — instance-side)*.
- The four contracts finalized and frozen with per-contract changelogs.

**Exit:** a scheduled run drains a seeded inbox into validator-green proposals with
zero duplicates on immediate re-run; the harness-off digest is structurally identical
to the harness-on digest (prose differs, artifact shape doesn't). The reference
instance enters continuous operation — **the ≥8-week survival clock for Phase 6
starts here.**
**Size:** 2 weeks focused.

## Phase 5 — Curio native integration *(calendar-gated)*

**Goal:** vanilla `curio.frontmatter.v1` / `curio.events.v1` consumption flips from
draft fixtures to released artifacts; the reading surface plugs in as pure
configuration on both sides. **Blocks on Curio shipping its contracts**; feeds Curio
before that — the Phase 0 schema review and Phases 1–2 fixtures are what Curio's
generic-consumer acceptance test demos against.

- Curio adapter GA in `curator-ingest`: validates vanilla frontmatter against pinned
  released schemas (additive minors tolerated; major mismatch → index-side quarantine
  flag + alert, never a file move); maps `curio_id` → `kp_id: curio:<uuidv7>`;
  lifecycle stays index-side.
- Events consumption: rotation-aware `(file, line)` cursors in the local state dir,
  `event_id` dedupe, negation-event folding; `.curio/events/` gitignored everywhere —
  behavioral history never enters git; an events-replay CLI ships with it.
- Vault autocommit helper with quiesce logic (manifest mtime stable, every manifest
  entry resolves, no temp files) — the torn note+manifest pair is designed out.
- Ownership enforcement live end-to-end: validator reads the manifest at merge-base,
  checksum verification discriminates by identity namespace (`curio:*` opaque,
  `kp:*`/`zotero:*` verified).
- Companion-note enrichment is the shipped mechanism; in-region enrichment remains a
  paper reservation.
- Cross-repo contract CI flips from allow-fail (drafts) to hard-fail (released
  schemas); Curio's wipe-and-reinstall reconcile runs jointly against a real KP vault.

**Exit:** save-in-Curio → vault → indexed, with measured latency documented as the
honest contract; Curio re-promotion of a KP-enriched note clobbers nothing; cursors
survive rotation; replayed events dedupe.
**Size:** 2–3 weeks focused; start date gated on Curio.

## Phase 6 — OSS hardening + public launch *(gated)*

**Goal:** the repo goes public and the quickstart promise is machine-true.
**Entry gates:** ≥8 consecutive weeks of reference-instance operation on tagged code;
name/trademark and license decisions executed; secret-scan + license-audit green over
the full history.

- Docs: quickstart with **one measured wall-clock number** from a bare runner
  (prerequisites box included); concepts (canonical/derived); integrations (Curio,
  Zotero, Obsidian-as-viewer, generic producer); operations (backup, rebuild,
  upgrade-by-rebuild); reference (config, CLI, MCP tools); a tested-MCP-clients matrix
  replacing "any client works".
- `examples/sample-vault` + a demo recipe; `curator init` vault starter; an example
  RSS-to-`kp-note` script (no fetcher crate — "any conforming producer" *is* the
  integration answer).
- CI: fast per-commit gate on the `hash` embedder; a weekly scheduled real end-to-end
  (pinned ONNX model cached) as a release blocker; secret-scan; license-audit proving
  copyleft extraction extras stay out of the dependency tree.
- Packaging: single static binary — installs via git tags / `cargo install`; one
  container compose profile set (core | zotero | librarian, CPU-only) as the co-equal
  documented mode. No registry publication yet.
- Launch mechanics: LICENSE (per the maintainer's decision — TBD until then),
  GOVERNANCE.md, SECURITY.md; repo flipped public; the reference instance pins the
  launch tag with a zero-forked-code representativeness check *(instance-side)*.

**Exit:** a clean-machine quickstart run by someone-not-the-maintainer completes
ingest → index → search under the published number; the weekly real end-to-end job has
been green four consecutive weeks.
**Size:** 3–4 weeks focused; the survival gate makes the calendar floor ~2 months
after Phase 4.

## Phase 7 — Post-launch menu *(each item independently gated)*

Nothing here blocks or is assumed by Phases 0–6; each item merges only with its gate
evidence recorded.

| item | gate |
|---|---|
| Interest model v1 (event-boosted re-rank inside `now.md` thread scope, inspectable artifact) | months of corpus + events flowing; negation-events review outcome known |
| Digest Atom feed + Curio subscription (closing the digest → read/star/promote loop) | Curio ships the private-network allowlist; feed stays a static file, the digest note stays primary |
| Webhook events receiver (bearer-auth, schema-validated, replay-idempotent) | file-tail latency proven insufficient in practice |
| Network-database store profile (Postgres/pgvector, identical edge DDL) | a real user needs cross-host or scale |
| Managed-region in-note enrichment + annotation regions | Curio ships region-scoped re-export honoring the reserved marker |
| Release train (registry publication, multi-arch images) | external users exist |
| Zotero write tools (staging-collection-enforced) | a review-then-move ritual is designed |
| CPU reranker profile | measured retrieval-quality need |
