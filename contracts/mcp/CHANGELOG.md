# MCP surface contract changelog

## v1 (clarification) — 2026-07-03

- Documented the exact JSON output shape of every tool ("Output shapes",
  normative), matching the reference implementation's advertised output
  schemas. No name or shape changed: results/notes arrays, field names,
  and nullability were implicit in the tool table and are now written
  down — plus two serving behaviors: `kp_recent` returns at most 50 rows,
  and tool failures surface as MCP tool errors (`isError: true`), never
  protocol failures.

## v1 — 2026-07-03

- Initial publication: six tools — `kp_search`, `kp_get_note`,
  `kp_related`, `kp_recent`, `kp_propose`, `kp_digest_latest` — on one
  entrypoint (stdio default, streamable HTTP + bearer optional).
- `kp_propose` is the only write verb; all writes ride `proposals/v1`.
