# MCP surface contract changelog

## v1 — 2026-07-03

- Initial publication: six tools — `kp_search`, `kp_get_note`,
  `kp_related`, `kp_recent`, `kp_propose`, `kp_digest_latest` — on one
  entrypoint (stdio default, streamable HTTP + bearer optional).
- `kp_propose` is the only write verb; all writes ride `proposals/v1`.
