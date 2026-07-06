# MCP tools

One MCP entrypoint serves the whole corpus. The surface is the
published
[MCP surface v1 contract](https://github.com/alexnodeland/curator/blob/main/contracts/mcp/v1.md):
**tool names and output shapes ARE the contract** — adding a tool is a
minor version, changing any existing name or shape is a major.

## Transports

| transport | how | auth |
|---|---|---|
| **stdio** (default) | `curator mcp serve` — any MCP client gets the whole corpus with one config entry | process-local; none needed |
| **streamable HTTP** | `curator mcp serve --http` (or `[mcp].transport = "http"`), bound to `[mcp].http_bind` | **bearer token, required** — env-injected via `[mcp].bearer_token_env`; the server refuses to start without it |

Client-by-client status and config snippets:
[Tested MCP clients](clients.md).

## The six tools

| tool | args | returns |
|---|---|---|
| `kp_search` | `{query, k?=10, mode?="hybrid"\|"vector"\|"fts"}` | ranked notes: id, title, path, snippet, score |
| `kp_get_note` | `{id}` (any namespace) | full note content + frontmatter + index metadata |
| `kp_related` | `{id, k?=10}` | embedding-nearest notes |
| `kp_recent` | `{days?=7, kind?}` | recently ingested/changed notes |
| `kp_propose` | `{title, rationale, files:[{path, content}]}` | proposal id (writes via `proposals/v1` ONLY) |
| `kp_digest_latest` | `{}` | latest librarian digest note |

## Output shapes (normative)

Every tool returns its result as MCP structured content with exactly
these JSON shapes (`?` marks nullable fields; timestamps are RFC 3339
UTC strings). The server also advertises them as tool output schemas.

### `kp_search`

`mode` echoes the mode that served the query; hits are best-first and
`score` is comparable only within one response:

```json
{"mode": "hybrid",
 "results": [{"id": "…", "title": "…", "path": "…", "snippet": "…", "score": 0.9}]}
```

### `kp_get_note`

```json
{"id": "…", "title": "…", "path": "…", "content": "…",
 "frontmatter": {"tags": [], "source": "…?", "created": "…?",
                 "updated": "…?", "checksum": "…?"},
 "index": {"ingested_at": "…", "links": [{"to": "…", "kind": "…"}]}}
```

### `kp_related`

Same hit shape as `kp_search`, anchor excluded:

```json
{"id": "…", "results": [{"id": "…", "title": "…", "path": "…", "snippet": "…", "score": 0.9}]}
```

### `kp_recent`

`kind` is an identity namespace (`curio` | `zotero` | `kp` | `path`);
newest first by index write time, at most 50 rows:

```json
{"days": 7, "kind": null,
 "notes": [{"id": "…", "title": "…", "path": "…", "tags": [],
            "source": "…?", "updated": "…?", "ingested_at": "…"}]}
```

### `kp_propose`

`dir` is the vault-relative proposal directory:

```json
{"id": "<ULID>", "status": "open", "dir": ".kp/proposals/<ULID>", "files": ["…"]}
```

### `kp_digest_latest`

`digest` is `null` when no digest exists yet:

```json
{"digest": {"id": "…", "title": "…", "path": "…", "content": "…",
            "created": "…?", "ingested_at": "…"}}
```

## Errors

Tool-level failures (unknown id, invalid proposal, missing index, …)
surface as MCP tool errors (`isError: true`) with a human-readable
message — never as protocol failures.

## Binding rules

1. **Tool names and shapes ARE the contract.** Adding a tool is a
   minor version; changing any existing name or shape is a major
   version.
2. **`kp_propose` is the only write verb.** There is no tool that
   writes canonical content directly — every write rides
   [`proposals/v1`](../concepts.md#proposals-the-only-write-path) and
   waits for human application. No exception exists anywhere in the
   surface.
3. `id` arguments accept every `kp-note/v1` identity namespace
   (`curio:` | `zotero:` | `kp:` | `path:`).
4. When `transport = "http"`, the bearer token (env-injected, see
   [Configuration](config.md#mcp)) is required — **there is no
   unauthenticated network mode.**
