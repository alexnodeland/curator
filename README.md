# Knowledge Plane

> **Status: pre-release.** Working name, contracts v1, APIs settling.
> Not yet packaged or published. **License: TBD — private until launch.**

The Knowledge Plane is the substrate under your personal knowledge
system: **any plain-markdown vault** + **one index database** + **MCP for
agents** + **a deterministic librarian**.

- **Your notes stay plain files.** A markdown+YAML directory under git is
  the whole canonical store; the plane's index is derived and disposable
  (rebuilt, never migrated). No plugin, no proprietary format, no live
  editor required — bring your own viewer.
- **Agents are first-class readers** — one MCP entrypoint gives any agent
  search, retrieval, and relatedness over your whole corpus, citations
  included.
- **Agents propose, you decide.** The only write path is a validated,
  human-applied proposal. No tool anywhere in the surface writes your
  notes directly.
- **The librarian is deterministic.** Ranked, grouped digests of what's
  new against your current interests — zero LLM required; an agent
  harness is an optional prose enhancer.

## Quickstart

No model server, no database service, no daemon — one binary, one
SQLite file:

```sh
git clone <this-repo> && cd knowledge-plane
cargo build --workspace
cargo run -p kp-cli -- --help     # the `kp` binary
```

**Two downloads to know about up front.** The default build compiles the
in-process ONNX embedder: `cargo build` fetches ONNX Runtime binaries
(via the `ort` download feature) at build time, and the first command
that embeds (e.g. default-config `kp init` / `kp ingest`) fetches the
pinned ~130 MB embedding model from Hugging Face into `.kp/models/`
(one-time, announced with a progress bar). For a fully-offline or lean
setup: set `embedder = "hash"` in `kp.toml` (deterministic, no ML), or
build with `cargo build -p kp-cli --no-default-features` — that binary
has no ONNX stack and performs zero downloads at build or run time.

Copy [`kp.example.toml`](kp.example.toml) to `kp.toml` and point
`[vault].path` at your markdown directory — or let `kp init` scaffold
one. `kp --help` lists the surface: ingest, index rebuild, Zotero sync,
search/get/related/recent, `mcp serve`, proposals, the librarian digest,
doctor. (Pre-release: APIs are still settling — `docs/design/` is the
design record.)

For development: `just` lists the front door (`just ci` = exactly what CI
runs).

## The four contracts

Everything that crosses the system boundary is one of four published
contracts; everything else is internal and changes freely.

| contract | governs | spec |
|---|---|---|
| `kp-note/v1` | note identity + enrichment frontmatter | [contracts/kp-note/v1.md](contracts/kp-note/v1.md) |
| `kp-config/v1` | `kp.toml` configuration | [contracts/kp-config/v1.md](contracts/kp-config/v1.md) |
| `proposals/v1` | the only agent write path | [contracts/proposals/v1.md](contracts/proposals/v1.md) |
| MCP surface v1 | the agent tool surface | [contracts/mcp/v1.md](contracts/mcp/v1.md) |

Any producer that writes conforming markdown+frontmatter into the vault
is a valid producer — that sentence is the whole integration story. The
sibling Curio reader and Zotero integrate this way (see
[docs/design/architecture.md](docs/design/architecture.md)).

## Contributing

See [CONTRIBUTING.md](CONTRIBUTING.md), [GOVERNANCE.md](GOVERNANCE.md),
[SECURITY.md](SECURITY.md), and the
[Code of Conduct](CODE_OF_CONDUCT.md).

## License

**TBD — private until launch.** No license has been chosen yet; until
one is, no rights are granted beyond viewing.
