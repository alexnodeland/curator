---
name: add-producer
description: Integrate a new content producer (reader app, clipper, reference manager) into the Knowledge Plane via the adapter pattern. Use when asked to ingest a new source of notes or events.
---

# Adding a producer — the adapter pattern

Producers integrate **by adapter, never by template**. The Knowledge
Plane consumes a producer's OWN published formats and maps them onto
`kp-note/v1` identities; it never asks a producer to emit KP frontmatter.
Anything that writes conforming markdown+frontmatter into the vault is
already a valid producer with zero code — this skill is for producers
with their own formats worth adapting (richer identity, events, managed
regions).

## Checklist

1. **Pin the producer's contract.** Vendor its machine-readable schemas
   under `contracts/vendor/<producer>/` with a `PIN` file (upstream repo,
   commit, sha256 per file — byte-identical copies, never edited here).
   If the producer has no published schema, stop: get one published
   upstream first. The adapter boundary is a schema, not a guess.
2. **Mint the identity mapping.** Reserve a `kp_id` namespace
   (`<producer>:<their-stable-id>`) and document it in
   `contracts/kp-note/v1.md` — that IS a contract change, so follow
   `.claude/skills/contract-change/` (additive → minor).
3. **Write the adapter in `kp-ingest`** (`src/<producer>.rs`):
   - validate every input against the vendored schema at the boundary;
   - map the producer's id → `kp_id`; producer checksum stays a change
     token only;
   - respect ownership: if the producer marks managed regions or owns a
     state dir, enrichment goes OUTSIDE those (see the Curio rules in
     `contracts/kp-note/v1.md`), and the proposals validator learns the
     refusal;
   - events (if any): rotation-aware `(file, line)` cursors, dedupe by
     event id, tolerate vanished files; behavioral data never goes to git.
4. **Config seam.** Add a `[<producer>]` table (with `enabled = false`
   default) via a contract change to `kp-config/v1` + `kp.example.toml`.
5. **Hermetic tests only.** Fixture files checked into the crate's
   `tests/fixtures/`; the `hash` embedder for anything embedding-shaped;
   no network, no producer binary required.
6. **Gate.** `just ci` green; the litmus stays clean (no private instance
   names — the producer's public name and formats only).

The Curio adapter is the reference implementation of every rule above.
