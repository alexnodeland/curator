# Curator

![A curator terminal session: init, hybrid search, librarian digest, status](assets/demo.svg)

Curator is the knowledge plane under your personal knowledge system:
**any plain-markdown vault** + **one index database** + **MCP for
agents** + **a deterministic librarian**.

- **Your notes stay plain files.** A markdown+YAML directory under git
  is the whole canonical store; the plane's index is derived and
  disposable — rebuilt, never migrated. No plugin, no proprietary
  format, no live editor required. Bring your own viewer.
- **Agents are first-class readers.** One MCP entrypoint gives any
  agent search, retrieval, and relatedness over your whole corpus,
  citations included.
- **Agents propose, you decide.** The only write path is a validated,
  human-applied proposal. No tool anywhere in the surface writes your
  notes directly.
- **The librarian is deterministic.** Ranked, grouped digests of what's
  new against your current interests — zero LLM required; an agent
  harness is an optional prose enhancer.

> **Status: pre-release.** Contracts v1 are published and stable; APIs
> around them are still settling. Not yet packaged — install is
> build-from-source ([Quickstart](quickstart.md)).

## Where to start

| you want to | read |
|---|---|
| try it in under a minute | [Quickstart](quickstart.md) |
| understand the design commitments | [Concepts](concepts.md) |
| plug in a reader, Zotero, or your own tool | [Integrations](integrations/curio.md) |
| run it on a server, back it up, upgrade it | [Operations](operations.md) |
| look something up | [Configuration](reference/config.md) · [CLI](reference/cli.md) · [MCP tools](reference/mcp.md) |

## The four contracts

Everything that crosses the system boundary is one of four published
contracts; everything else is internal and changes freely. Consumers
pin versions; additive changes are minors, breaking changes are majors,
each contract keeps its own changelog.

| contract | governs | spec |
|---|---|---|
| `kp-note/v1` | note identity + enrichment frontmatter | [contracts/kp-note/v1.md](https://github.com/alexnodeland/curator/blob/main/contracts/kp-note/v1.md) |
| `kp-config/v1` | `curator.toml` configuration (legacy name `kp.toml`) | [contracts/kp-config/v1.md](https://github.com/alexnodeland/curator/blob/main/contracts/kp-config/v1.md) |
| `proposals/v1` | the only agent write path | [contracts/proposals/v1.md](https://github.com/alexnodeland/curator/blob/main/contracts/proposals/v1.md) |
| MCP surface v1 | the agent tool surface | [contracts/mcp/v1.md](https://github.com/alexnodeland/curator/blob/main/contracts/mcp/v1.md) |

Any producer that writes conforming markdown+frontmatter into the vault
is a valid producer — that sentence is the whole integration story.
[Write a producer](integrations/producers.md) spells it out; the sibling
[Curio](integrations/curio.md) reader and [Zotero](integrations/zotero.md)
integrate exactly this way.

## Source

Curator lives at
[github.com/alexnodeland/curator](https://github.com/alexnodeland/curator).
This site is generated from the repo's `docs/site/` sources — same
sources, same bytes, on every build.
