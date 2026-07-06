# Knowledge Plane — design decisions

Distilled from the adversarial design review: three parallel designs (distribution,
integration fabric, MCP/agent architecture) were each subjected to independent verdict
passes hunting for contradictions, silent failure modes, and scope lies. The raw verdicts
are archived in [research/verdicts.json](research/verdicts.json). What follows are the
rulings that shaped v1 — each one killed a design that was otherwise on the table.

## 1. No graph database

**Killed:** an embedded graph store (Kuzu) as the default graph driver; also an embedded
vector-file store (LanceDB).

**Why.** Two independent kills. First, embedded Kuzu does not support safe multi-process
concurrent access — and this architecture is multi-process by construction (MCP clients
spawn stdio server processes per session while the indexer daemon writes the same store).
Kuzu either breaks or forces a serving daemon, destroying the zero-infra story. Second,
the payoff at personal scale (≤100k nodes, one-hop expansion, supports/refutes queries) is
zero: recursive CTEs over plain relational edge tables cover every query the retrieval
plane makes. A graph database is a dependency without a payoff.

**Consequence.** The claim/link graph is relational edge tables inside the *same*
`index.db` as vectors (sqlite-vec) and full-text (FTS5). One file, WAL, multi-process-safe.
Dropped permanently, not made optional — an optional driver axis would fork index
semantics and double the test matrix.

## 2. Checksum is never identity

**Killed:** identity precedence ending in body-checksum (`curio_id | zotero_key |
checksum`), and the `path+checksum` identity variant.

**Why.** Checksum-as-identity silently merges distinct notes: two notes with identical
normalized bodies (template-created stubs, empty capture notes, duplicated boilerplate)
collapse into one index identity — one file becomes permanently invisible to search with
zero error. Path-based identity breaks on every rename. Both are corrupt identity schemes.

**Consequence.** Identity is *minted*, never derived from content or location:
producer-namespaced `kp_id` (`curio:<uuidv7>` | `zotero:<itemKey>` | `kp:<uuidv7>`, with
`path:<relpath>` as a documented rename-fragile fallback for plain vault notes). The
`checksum` field is exclusively a change token; the proposals validator rejects any new
note whose identity is absent or duplicates an existing identity.

## 3. The validator is local-first — no forge required

**Killed:** a safety model that silently required a git forge (proposal branches + branch
protection + a separately-credentialed CI validator with exclusive merge rights).

**Why.** The stranger's default deployment is a laptop with *no git remote at all*. In
that mode, branch-push write verbs have no defined behavior, the CI validator has nowhere
to run, and the entire "agent proposes, a different process disposes" security story
evaporates — or worse, the docs imply you should push your private personal notes to a
forge to get safety, contradicting the local-first privacy posture.

**Consequence.** `proposals/v1` is a local mechanism: `curator propose` writes a changeset
under `<vault>/.kp/proposals/<ULID>/`, and `curator review` / `curator apply` run one importable,
deterministic validator (schema, path allowlist, ownership-oracle refusal, clean patch
application) in-process. Forge CI + branch protection is an *optional hardening tier* that
runs the exact same validator module. The safety model works with zero infrastructure.

## 4. In-process pinned CPU ONNX embedder by default; deterministic hash embedder for tests

**Killed:** "embeddings arrive over OpenAI-compatible HTTP, no in-process ML" as the core
posture — which made the quickstart promise a lie.

**Why.** If the default requires a local model server installed and a model pulled before
minute zero, the "clone → ingest → search in minutes" claim is false for exactly the
audience it targets, and the CI job "enforcing" it must either run a real model server
(slow, flaky, permanently red) or mock the endpoint — at which point it proves nothing
about the stranger's path.

**Consequence.** The `Embedder` trait ships two v1 backends: `builtin` — a pinned,
hash-verified small CPU ONNX model running in-process (model id + dims recorded in the
index epoch; still CPU-only, honoring modest hardware) — and `hash` — a deterministic
embedder backing **all** embedding tests, so the test suite is hermetic: no network, no
model downloads, no external services. Remote OpenAI-compatible endpoints remain the
documented post-v1 upgrade seam. The quickstart states one measured wall-clock number,
prerequisites included.

**Build reality check (2026-07-03).** The rule for the cargo wiring was: try
`embed-onnx` (fastembed + ort, pinned `BGE-small-en-v1.5`, 384 dims) as a DEFAULT
feature of `curator-index`, and demote it to opt-in only if it failed to build cleanly on
Apple Silicon — the quickstart must never lie about what a default build delivers.
It built and linked cleanly (ort 2.0.0-rc.12, downloaded CPU binaries), so **the
default stands**: a stock `cargo build` includes the `builtin` embedder, matching the
`embedder = "builtin"` default in kp-config/v1. Model *fetch* stays lazy — first
`embed()` call, never construction — so the default `cargo test --workspace` remains
fully hermetic; the one end-to-end inference test is `#[ignore]`d and run manually.
`--no-default-features` yields a hash-embedder-only build for constrained
environments.

## 5. The librarian is deterministic-first; the LLM is an optional enhancer

**Killed:** an agent-harness-dependent discovery loop — where the headline feature (the
digest) de facto required a subscription to one AI vendor.

**Why.** "Reference harness now, provider-neutral harness later" is Anthropic-dependence
deferred, not solved; and a provider-neutral agent loop doing discovery-grade judgment on
small local models is a research project, not a v1 deliverable. Meanwhile the actual
baseline digest is just code.

**Consequence.** Maintenance (ingest routing, embedding, dedupe-by-threshold, stub
generation) and the baseline digest (rank new-since-last-digest notes by embedding
similarity to the `now.md` anchor × recency decay; group; render with extractive
summaries) require **zero LLM**. An agent harness is an optional prose enhancer riding the
proposals path; enabling it changes prose quality, never artifact shape. Harness
neutrality is guaranteed by the `proposals/v1` contract — any harness producing conforming
proposals is a valid librarian.

## 6. Rust

One toolchain end to end (single static binary, no runtime for strangers to install) —
re-decided upstream after the review round, superseding the earlier Python lean; the
sibling reader is Rust and every integration seam is a file contract, so nothing is lost
at the boundary.

## Appendix: other verdict-driven rulings

| ruling | verdict it came from |
|---|---|
| **Exactly four published contracts** (kp-note/v1, kp-config/v1, proposals/v1, MCP surface v1); everything else internal | ~10 published schemas for a system with one consumer is a forever-promise a solo maintainer defaults on |
| **Embedded store only at v1**; network database driver is a post-v1 profile | two store backends double the index plane, and the reference instance must exercise the exact OSS default code path |
| **Blue/green epoch rebuilds**, epoch keyed only on model+dims / chunker / normalization | refuse-to-serve on mismatch = multi-day retrieval outage on CPU hardware; version bumps must never force rebuilds |
| **In-process retrieval** — no versioned internal network API | the API's only consumer was one crate; a versioned contract nobody else calls is speculative infrastructure |
| **No `status` in frontmatter; lifecycle index-side for all notes** | producer re-exports re-render whole files, silently clobbering injected fields |
| **Full-file hash for reindex** (body checksum stays a contract-level change token) | body-only checksums make frontmatter edits (tags, titles) invisible to the index forever |
| **Digests are create-only, one namespace, idempotent by date** | a re-run proposing edits to an already-applied digest becomes an unreviewed overwrite of canonical content |
| **Zotero access is read-only at v1**; `/deleted` tombstones consumed; deletions raise proposals, never auto-delete | agent write access to a canonical store is a pollution loop with no gate |
| **Behavioral events are never committed to git** | unbounded reading-history JSONL in a synced repo is a privacy incident waiting for a stranger |
| **One combined MCP entrypoint** | three servers = three config entries and three tokens; "any MCP client works" was a claim, not a test |
