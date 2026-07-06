# Knowledge Plane contracts

The Knowledge Plane publishes **exactly four contracts**. Everything that
crosses the system boundary is one of these; everything else — the
`index.db` schema, embedder internals, cursor formats, cache layouts — is
internal and may change freely.

| contract | governs | spec |
|---|---|---|
| `kp-note/v1` | note identity + enrichment frontmatter | [kp-note/v1.md](kp-note/v1.md) · [JSON Schema](kp-note/v1.schema.json) |
| `kp-config/v1` | `curator.toml` configuration (legacy name `kp.toml`) | [kp-config/v1.md](kp-config/v1.md) · [JSON Schema](kp-config/v1.schema.json) |
| `proposals/v1` | the only agent write path | [proposals/v1.md](proposals/v1.md) · [JSON Schema](proposals/v1.schema.json) |
| MCP surface v1 | the agent tool surface | [mcp/v1.md](mcp/v1.md) |

## Versioning discipline

- **Additive changes are minor versions; breaking changes are major.**
  A major change mints a new spec file (`v2.md`) — v1 semantics are never
  edited after publication.
- Each contract directory carries its own `CHANGELOG.md`.
- Consumers pin versions (`kp_schema: kp-note/v1`, `schema = "kp-config/v1"`,
  `"schema": "proposals/v1"`).
- **The contracts are the API.** Code conforms to these documents, never
  the other way around. A code change that contradicts a contract is a bug
  in the code.

## Vendored producer schemas

[vendor/curio/](vendor/curio/) holds sha-pinned copies of Curio's
published JSON Schemas (`curio.frontmatter.v1`, `curio.events.v1`), which
the Curio adapter in `kp-ingest` validates against at the boundary.
