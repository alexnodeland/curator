# `kp-note` contract changelog

## v1 — 2026-07-03

- Initial publication: producer-namespaced `kp_id` (`curio:` | `zotero:` |
  `kp:` | `path:` fallback), `kp_schema`, `checksum` as change-token-only,
  `title`, `created`, `updated`, `tags`, `source`.
- No `status` field, by design — lifecycle is index-side.
- Curio boundary rules: `.curio/**` ownership, managed-region enrichment
  placement, adapter-not-template ingestion.
