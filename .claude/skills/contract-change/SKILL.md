---
name: contract-change
description: Change one of the four published Knowledge Plane contracts (kp-note, kp-config, proposals, mcp) — classify the change, version it, update spec + schema + changelog + code in the right order. Use whenever a task touches anything under contracts/ or alters a published shape.
---

# Contract change procedure

The four contracts are the API. Code conforms to `contracts/`, never vice
versa. Every contract change follows this order — spec first, code last.

## 1. Classify

- **Additive** (new optional field, new MCP tool, new config key with a
  default): **minor** version. The existing spec file is amended, with the
  addition clearly marked and changelogged.
- **Breaking** (rename, removal, semantic change, required-ness change,
  MCP tool shape change): **major** version. Mint a NEW spec file
  (`v2.md`, `v2.schema.json`) — v1 semantics are never edited after
  publication. v1 stays in the tree until formally retired.

## 2. Update the contract artifacts (one commit)

1. Edit/create the spec under `contracts/<name>/`.
2. Update the JSON Schema if the contract has one (`kp-note`,
   `proposals`) — keep `$id` version-suffixed.
3. Add a dated entry to that contract's `CHANGELOG.md` saying what
   changed and why.
4. If the change affects `kp-config`, update `kp.example.toml` — the
   kp-core test pins the example to the model, so they must move together.

## 3. Conform the code (follow-up commits)

Update `kp-core` types and any downstream crate. Tests that encode the
contract examples (kp-core has one per contract) must be updated to the
new spec text — copy from the spec, do not invent.

## 4. Gate

`just ci` green. If consumers exist outside this repo (Curio adapter
schemas are the inverse case), note the migration in the changelog entry.

## Vendored producer schemas are the mirror image

`contracts/vendor/curio/` changes only by re-sync from upstream: copy
byte-identical files, regenerate the `PIN` (upstream commit + sha256 per
file), one commit. Never hand-edit a vendored schema.
