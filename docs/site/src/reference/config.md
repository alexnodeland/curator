# Configuration

One TOML file governs everything, versioned by the
[`kp-config/v1` contract](https://github.com/alexnodeland/curator/blob/main/contracts/kp-config/v1.md).
A complete, working example ships at the repo root as
[`curator.example.toml`](https://github.com/alexnodeland/curator/blob/main/curator.example.toml);
`curator init` scaffolds from it.

Three binding rules:

1. **Versioned** via the top-level `schema` key.
2. **Unknown keys warn, never fail** — a config written for a newer
   minor version loads on an older binary.
3. **Secrets only via env indirection** — `*_env` keys name an
   environment variable; secret *values* never appear in the file.

## Discovery

Every command takes `--config <path>`. Without it, Curator looks in
order:

1. `$CURATOR_CONFIG` (preferred) or `$KP_CONFIG` — an explicit path;
2. `./curator.toml`;
3. `./kp.toml` — the legacy name, still accepted; loading it warns and
   suggests renaming. It is deprecated, not removed: existing setups
   keep working.

## The full file

```toml
schema = "kp-config/v1"

[vault]
path = "~/vault"                 # the markdown corpus root
proposals_dir = ".kp/proposals"  # relative to vault

[index]
path = "~/.local/share/kp/index.db"   # ONE embedded SQLite db: vec + FTS5 + edges
embedder = "builtin"                  # builtin = in-process pinned CPU ONNX; or "hash" (test)
chunk_tokens = 512
chunk_overlap = 64

[curio]
enabled = false
events_dir = "~/.local/share/curio/events"   # tail target (rotation-aware cursors)
notes_dirs = ["curio"]                       # vault-relative dirs Curio exports into

[zotero]
enabled = false
api_base = "https://api.zotero.org"
user_id = ""
api_key_env = "KP_ZOTERO_KEY"    # never inline; CURATOR_ZOTERO_KEY preferred alias
webdav_fallback = false          # ~30-line CRC-verified .prop/.zip shim
webdav_url = ""

[librarian]
now_path = "now.md"              # interest anchor note (vault-relative)
digest_dir = "digests"           # vault-relative output dir (kp: namespace notes)
half_life_days = 14              # recency decay
top_k = 12

[mcp]
transport = "stdio"              # stdio (default) | http
http_bind = "127.0.0.1:8377"
bearer_token_env = "KP_MCP_TOKEN" # required when http; CURATOR_MCP_TOKEN preferred alias
```

## Key by key

### `[vault]`

| key | default | meaning |
|---|---|---|
| `path` | `~/vault` | the markdown corpus root — the canonical store |
| `proposals_dir` | `.kp/proposals` | where proposals stage, vault-relative |

### `[index]`

| key | default | meaning |
|---|---|---|
| `path` | `~/.local/share/kp/index.db` | the one derived SQLite file (vectors + FTS5 + edges). `curator init` scaffolds it inside the vault's `.kp/` instead |
| `embedder` | `builtin` | `builtin` = pinned in-process CPU ONNX model (~130 MB one-time download); `hash` = deterministic, offline, no ML |
| `chunk_tokens` | `512` | chunk size for embedding |
| `chunk_overlap` | `64` | overlap between adjacent chunks |

Changing `embedder` (or chunking) changes the index epoch — run
`curator index rebuild`
([epochs, not migrations](../concepts.md#epochs-not-migrations)).

### `[curio]`

| key | default | meaning |
|---|---|---|
| `enabled` | `false` | turn the [Curio adapter](../integrations/curio.md) on |
| `events_dir` | `~/.local/share/curio/events` | `curio.events.v1` JSONL tail target (rotation-aware cursors) |
| `notes_dirs` | `["curio"]` | vault-relative directories Curio exports notes into |

### `[zotero]`

| key | default | meaning |
|---|---|---|
| `enabled` | `false` | turn the [Zotero producer](../integrations/zotero.md) on |
| `api_base` | `https://api.zotero.org` | Zotero Web API base |
| `user_id` | `""` | your Zotero user id |
| `api_key_env` | `KP_ZOTERO_KEY` | env var **name** holding the API key |
| `webdav_fallback` | `false` | enable the CRC-verified WebDAV fulltext fallback |
| `webdav_url` | `""` | WebDAV base URL when the fallback is enabled |

### `[librarian]`

| key | default | meaning |
|---|---|---|
| `now_path` | `now.md` | the interest anchor note, vault-relative |
| `digest_dir` | `digests` | where applied digests land, vault-relative |
| `half_life_days` | `14` | recency decay half-life for digest scoring |
| `top_k` | `12` | how many notes a digest surfaces |

### `[mcp]`

| key | default | meaning |
|---|---|---|
| `transport` | `stdio` | `stdio` (default) or `http` (streamable HTTP) |
| `http_bind` | `127.0.0.1:8377` | bind address for the HTTP transport |
| `bearer_token_env` | `KP_MCP_TOKEN` | env var **name** holding the bearer token; required when `transport = "http"` — [no unauthenticated network mode exists](mcp.md#binding-rules) |

## Environment variables

Curator's original env names carry the `KP_` prefix; every one has a
preferred `CURATOR_` alias. **`CURATOR_<X>` wins when both are set;
`KP_<X>` keeps working** — existing deployments never break.

| purpose | preferred | legacy (still honored) |
|---|---|---|
| explicit config path | `CURATOR_CONFIG` | `KP_CONFIG` |
| Zotero API key (default `api_key_env`) | `CURATOR_ZOTERO_KEY` | `KP_ZOTERO_KEY` |
| MCP bearer token (default `bearer_token_env`) | `CURATOR_MCP_TOKEN` | `KP_MCP_TOKEN` |

The `*_env` config keys name a variable of your choosing; the alias
pairing applies to whatever name you configure, in both directions — a
`KP_<X>`-named variable is also looked up as `CURATOR_<X>` (preferred)
and vice versa. `RUST_LOG` additionally controls log verbosity
(default `warn`, stderr).
