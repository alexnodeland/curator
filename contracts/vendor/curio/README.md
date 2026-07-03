# Vendored Curio schemas

This directory holds **sha-pinned copies** of Curio's published JSON
Schemas. The Curio adapter in `kp-ingest` validates every note and event
at the boundary against these vendored copies — never against a live
checkout — so the adapter's behavior is a function of a recorded upstream
commit, not of whatever the sibling repo looks like today.

## Expected contents

| file | upstream source | contract |
|---|---|---|
| `frontmatter.v1.json` | `schemas/frontmatter.v1.json` in the Curio repo | `curio.frontmatter.v1` — exported markdown notes |
| `events.v1.json` | `schemas/events.v1.json` in the Curio repo | `curio.events.v1` — append-only behavioral event log (JSONL) |
| `PIN` | generated at sync time | provenance record |

## The pin mechanism

Every sync writes a `PIN` file recording:

```text
source_repo: <upstream repo identifier>
commit: <git rev-parse HEAD of the upstream checkout at copy time>
synced: <RFC 3339 UTC timestamp>
sha256 frontmatter.v1.json: <sha256 of the vendored file>
sha256 events.v1.json: <sha256 of the vendored file>
```

Rules:

1. Vendored files are **byte-identical** copies — never edited here. A
   change upstream means a re-sync with a new `PIN`, reviewed as its own
   commit.
2. Curio schemas are versioned-immutable upstream (a breaking change
   mints `*.v2.json`, never edits v1), so a pin bump is always additive
   review, never a silent semantic shift.
3. Code reads only the vendored copies; nothing in the workspace may
   reach outside the repo for a schema.

## Current status

Synced. Both schema files are vendored byte-identical from the upstream
Curio repo; [`PIN`](PIN) records the upstream commit and the sha256 of
each vendored file. The Curio adapter in `kp-ingest` embeds these copies
at compile time (`include_str!`) — the vendored bytes ARE the boundary.

The pin is enforced, not just recorded: `kp-ingest`'s `vendor_pin` test
recomputes the sha256 of the embedded bytes against `PIN` on every test
run (so every commit and CI pass) — a silent local edit to a vendored
schema fails the suite until the PIN is legitimately re-synced.
