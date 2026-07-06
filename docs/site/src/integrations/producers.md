# Write a producer

Any tool that writes conforming markdown+frontmatter into the vault is
a valid producer. **That sentence is the whole integration story** —
the contracts are published so the adapter list never needs to be.
Curio and Zotero integrate exactly this way; so can your exporter,
clipper, or pipeline.

## Emit `kp-note/v1` files

One markdown file per note, YAML frontmatter first:

```yaml
---
kp_id: "kp:0197b2c4-8f3e-7cc1-a5d2-3e9f10aa4b6d"
kp_schema: kp-note/v1
checksum: "sha256:9f86d08..."     # change token ONLY, never identity
title: "Article title"
created: 2026-07-03T09:15:00Z
updated: 2026-07-03T10:00:00Z
tags: [rust, databases]
source: "https://example.com/article"   # null for born-in-vault notes
---
```

The normative spec (with JSON Schema) is
[`contracts/kp-note/v1.md`](https://github.com/alexnodeland/curator/blob/main/contracts/kp-note/v1.md).
The rules that matter most when producing:

1. **Mint `kp_id`, never derive it.** Identity comes from the producer
   at creation time and survives renames, edits, and re-exports. Notes
   without `kp_id` get the implicit `path:<relpath>` identity — it
   works, but it's rename-fragile; real producers mint.
2. **`checksum` is a change token only.** Consumers must never key
   anything on it; two notes with identical bodies are still two notes.
3. **No `status` field.** Lifecycle lives index-side. Producer
   re-exports re-render whole files — injected lifecycle fields would
   be silently clobbered, so the contract forbids them outright.
4. **Preserve unknown frontmatter keys.** The contract block is what
   the plane reads and writes; everything else belongs to the user and
   other tools. If your producer re-renders files, round-trip what you
   don't own.

## If you re-export: own a marked region

A producer that *re-renders* its notes (rather than writing once)
should follow the managed-region pattern Curio and Zotero use: keep
machine content inside a clearly marked comment region and replace only
that region on re-export. Users write below the markers; enrichment
and manual notes survive every sync. See
[how Zotero notes do it](zotero.md#notes-land-as-managed-regions).

## Then Curator does the rest

```sh
curator ingest        # picks up new and changed files incrementally
```

Ingest hashes files, so unchanged notes cost nothing; changed notes are
re-chunked and re-embedded; wiki and markdown links become graph edges.
Your producer's notes are immediately searchable, relatable, and
citable through [the MCP surface](../reference/mcp.md) — no
registration, no plugin API, no adapter to submit.

## Choosing an identity namespace

| you are | use |
|---|---|
| a producer app with stable internal ids | your own namespace conventions inside `kp_id` (Curio uses `curio:<uuidv7>`) |
| a one-shot importer | freshly minted `kp:<uuidv7>` ids |
| hand-written notes / no producer | omit `kp_id`; the `path:` fallback covers you |

If you want boundary validation like the built-in producers have,
publish a JSON Schema for your format — the Curio adapter's
[sha-pinned vendoring](curio.md#sha-pinned-schemas) is the pattern to
copy.
