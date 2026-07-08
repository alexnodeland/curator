# CLI

The `curator` binary is the whole operational surface. Query commands
are thin shells over the same engine the MCP tools ride, so
`curator search` and `kp_search` cannot drift apart.

`curator --help` prints the canonical usage text; this page is that
text, expanded. Every command exits `0` on success, `1` on failure
(message on stderr), `2` on usage errors.

## Commands

| command | does |
|---|---|
| [`init [dir]`](#curator-init) | scaffold a vault: `curator.toml`, `.kp/`, `now.md`, first index |
| [`ingest`](#curator-ingest) | run producer adapters (Curio, vault notes) into the vault/index, incrementally |
| [`index rebuild`](#curator-index-rebuild) | rebuild `index.db` (blue/green epoch swap); `reindex` is an alias |
| [`zotero sync`](#curator-zotero-sync) | two-channel Zotero sync into the vault (delta metadata + fulltext) |
| [`search`](#curator-search) | hybrid retrieval from the terminal |
| [`get`](#curator-get) | one note by id (any namespace) — content + metadata |
| [`related`](#curator-related) | embedding-nearest notes to a note |
| [`recent`](#curator-recent) | recently ingested/changed notes |
| [`mcp serve`](#curator-mcp-serve) | serve the MCP surface (stdio default; `--http` + bearer) |
| [`propose`](#curator-propose) | create a `proposals/v1` changeset from a directory of files |
| [`review <id>`](#curator-review-apply-proposals-list) | render a proposal for human review |
| [`apply <id>`](#curator-review-apply-proposals-list) | validate and apply a proposal (stamps applied/rejected) |
| [`proposals list`](#curator-review-apply-proposals-list) | list stored proposals and their status |
| [`digest run`](#curator-digest-run) | run the deterministic librarian digest (`--auto` to apply) |
| [`doctor`](#curator-doctor) | config / vault / index / cursors health |
| [`status`](#curator-status) | vault + index + proposals snapshot (always succeeds; `--json`) |
| `-h`, `--help` / `-V`, `--version` | usage / version |

Shared flag on every command that loads config:
`--config <path>` — config location (default: `$CURATOR_CONFIG` or
`$KP_CONFIG`, then `./curator.toml`, `./kp.toml` —
[discovery details](config.md#discovery)). Batch and query commands
also take `--json` for machine-readable output on stdout.

## `curator init`

```sh
curator init [dir] [--embedder builtin|hash]
```

Scaffolds `dir` (default `.`) as a Curator vault: writes
`curator.toml` from the shipped example pointed at the directory
(never overwriting an existing `curator.toml` or legacy `kp.toml`),
creates the proposals directory and a starter `now.md`, and builds the
first index by running a full ingest.

| flag | meaning |
|---|---|
| `--embedder <e>` | `builtin` \| `hash`, stamped into the scaffolded config. `builtin` fetches its pinned ~130 MB ONNX model on first use (one-time, announced); `hash` is offline and deterministic (no ML) |

## `curator ingest`

```sh
curator ingest [--config <path>] [--json]
```

Incremental: full-file hashes mean unchanged notes cost nothing;
changed notes are re-chunked and re-embedded; frontmatter-only edits
update metadata without re-embedding. Reports counts for
ingested/unchanged/skipped/ignored/removed notes and links; with
`[curio].enabled`, also folded/duplicate/malformed event counts.

## `curator index rebuild`

```sh
curator index rebuild [--config <path>] [--json]    # alias: curator reindex
```

Rebuilds the same corpus, identities, and chunks that incremental
ingest produces, blue/green-swapped in as a new epoch — see
[Operations](../operations.md#rebuild) for when and why.

## `curator zotero sync`

```sh
curator zotero sync [--config <path>] [--json]
                    [--dir <path>] [--no-fulltext] [--fulltext-cap <n>]
```

| flag | meaning |
|---|---|
| `--dir <path>` | vault-relative notes dir (default: `zotero`) |
| `--no-fulltext` | skip the fulltext pass |
| `--fulltext-cap <n>` | fulltext truncation cap, characters (default: 20000) |

Two-channel sync — [how it works](../integrations/zotero.md).

## `curator search`

```sh
curator search <query> [--k <n>] [--mode hybrid|vector|fts]
               [--config <path>] [--json]
```

| flag | meaning |
|---|---|
| `--k <n>` | result count (default 10) |
| `--mode <m>` | `hybrid` (default) \| `vector` \| `fts` |
| `--json` | print the MCP-shaped JSON output (`kp_search`'s shape) |

## `curator get`

```sh
curator get <id> [--config <path>] [--json]
```

`<id>` accepts every identity namespace: `curio:…`, `zotero:…`,
`kp:…`, `path:<vault-relative-path>`.

## `curator related`

```sh
curator related <id> [--k <n>] [--config <path>] [--json]
```

Embedding-nearest notes to the anchor note (anchor excluded).

## `curator recent`

```sh
curator recent [--days <n>] [--kind curio|zotero|kp|path]
               [--config <path>] [--json]
```

| flag | meaning |
|---|---|
| `--days <n>` | look-back window in days (default 7) |
| `--kind <ns>` | identity-namespace filter |

## `curator mcp serve`

```sh
curator mcp serve [--config <path>] [--http]
```

Serves [MCP surface v1](mcp.md). stdio by default; `--http` (or
`[mcp].transport = "http"`) serves streamable HTTP on
`[mcp].http_bind` and **requires** the bearer token env named by
`[mcp].bearer_token_env` — the server refuses to start without it.

## `curator propose`

```sh
curator propose --title <t> --from <dir>
                [--rationale <r>] [--author <a>]
                [--config <path>] [--json]
```

| flag | meaning |
|---|---|
| `--title <t>` | proposal title (required) |
| `--from <dir>` | directory of generated files; every file maps to the same vault-relative path (required) |
| `--rationale <r>` | why this change (default: empty) |
| `--author <a>` | proposal author (default: `curator-cli`) |

Generated-content is the only creation mode in v1; a staged-changes
mode (diffing working-tree edits) is planned, not yet implemented.

## `curator review` / `apply` / `proposals list`

```sh
curator review <id>       # human-readable render of the changeset
curator apply <id>        # validate -> apply -> stamp status
curator proposals list    # ids, status, titles, file counts
```

`apply` runs the deterministic validator
([hard-reject list](../concepts.md#proposals-the-only-write-path))
and stamps `applied`/`rejected`.

## `curator digest run`

```sh
curator digest run [--auto] [--now <rfc3339>] [--config <path>] [--json]
```

| flag | meaning |
|---|---|
| `--auto` | auto-apply the digest proposal when the gate admits it (pure additions under `[librarian].digest_dir`, `kp:<uuidv7>` identities) |
| `--now <rfc3339>` | inject the clock (testing/reproducibility; default: now) |

## `curator doctor`

```sh
curator doctor [--config <path>] [--json]
```

Checks config, vault, proposals queue, `now.md`, index meta +
embedder identity match, digest log, Curio cursors, and MCP transport
sanity. Levels `ok`/`warn`/`error`; exits nonzero when anything
errors — healthcheck-ready
([operations guidance](../operations.md#watching-it)).

## `curator status`

```sh
curator status [--config <path>] [--json]
```

A state *snapshot* — the counterpart to `doctor`'s health *checks*. Reports
the vault note count, the serving index epoch/embedder/note count, the latest
librarian digest, the proposal queue (total + open), and the MCP transport.
Unlike `doctor` it never fails: a not-yet-built index is reported as `not
built`, not an error, so `status --json` is safe to pipe into scripts.
