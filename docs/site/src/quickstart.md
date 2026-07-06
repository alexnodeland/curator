# Quickstart

From a fresh checkout to your first search — measured, not estimated:
**9.7 seconds** end-to-end on the reference machine below, including the
one-time embedding-model download. With the model cached (every run
after the first) the same loop is sub-second.

> **Prerequisites**
>
> - **Rust ≥ 1.89** (`rustup` recommended) — Curator is pre-release and
>   installs from source.
> - **git** — your vault should live under version control; it is the
>   canonical store.
> - **~150 MB disk + network on first use** for the default embedder's
>   one-time model download — *or nothing at all*: the `hash` embedder
>   is fully offline and deterministic (see
>   [the offline path](#the-fully-offline-path) below).
> - No database server, no model server, no daemon. One binary, one
>   SQLite file.

## Install the binary

The primary install is Cargo, straight from the source tree:

```sh
git clone https://github.com/alexnodeland/curator && cd curator
cargo install --locked --path crates/curator-cli
curator --version
```

(equivalently, without a checkout:
`cargo install --locked --git https://github.com/alexnodeland/curator curator-cli`;
crates.io publication is a staged follow-up while pre-release. Tagged
releases also ship prebuilt linux x86_64 + macOS arm64 binaries with
checksums, and a container image builds from the repo's `Dockerfile` —
see [Operations](operations.md#containers).)

The default build compiles the in-process ONNX embedder; the build
fetches ONNX Runtime binaries at build time (via the `ort` download
feature). For development, `cargo build --release -p curator-cli`
produces the same binary at `target/release/curator`.

**Want to see the loop before pointing it at your own notes?** The
repo bundles a 12-note sample vault and a non-interactive walk-through:

```sh
just demo    # scratch-dir init → ingest → search → digest, fully offline
```

## Init, ingest, search

Point Curator at a markdown directory — or let `curator init` scaffold
everything in place:

```sh
cd ~/my-vault          # any directory of markdown notes
curator init .         # writes curator.toml, .kp/, now.md, first index
curator ingest         # incremental re-scan (init already built the index)
curator search "hybrid retrieval over an embedded index"
```

`curator init` scaffolds `curator.toml` (from the shipped example,
pointed at this directory), creates `.kp/proposals/`, seeds a `now.md`
interest anchor for the librarian, and builds the first index. On first
use the default `builtin` embedder announces and fetches its pinned
~130 MB model into `.kp/models/` — one time, with a progress bar.

### The measured number

Measured 2026-07-06 on an **Apple M3 Max (16 cores, 128 GB RAM), macOS
26.5**, release build, gigabit-class connection, against a fresh 12-note
sample vault:

```sh
curator init . && curator ingest && \
  curator search "hybrid retrieval over an embedded index"
```

| step | wall clock |
|---|---|
| `curator init .` (scaffold + **one-time ~130 MB model download** + first index) | 9.56 s |
| `curator ingest` (incremental re-scan, nothing changed) | 0.02 s |
| first `curator search` | 0.12 s |
| **total** | **9.70 s** |

The download dominates: everything that is actually Curator finishes in
fractions of a second at this corpus size. Your first-run number scales
with your connection and your note count.

### The fully-offline path

The same loop with the deterministic `hash` embedder — no ML, no
downloads at build or run time — measured on the same machine and
sample vault: **0.06 s total**.

```sh
curator init . --embedder hash
```

The `hash` embedder is built for tests and offline use; retrieval
quality is what you'd expect from a non-semantic embedding, but FTS
(keyword) search is unaffected. You can switch embedders later — edit
`[index].embedder` in `curator.toml` and run `curator index rebuild`
([epochs, not migrations](concepts.md#epochs-not-migrations)).

## Wire up an agent

Serve the whole corpus to any MCP client over stdio — one config entry,
no network, no token:

```sh
curator mcp serve
```

See [MCP tools](reference/mcp.md) for the surface and
[Tested MCP clients](reference/clients.md) for client-by-client status
and config snippets. For a network deployment (streamable HTTP + bearer
token) see [Operations](operations.md#serving-mcp-over-http).

## Where to go next

- [Concepts](concepts.md) — the canonical/derived split and the
  proposals-only write path, in five minutes.
- [Configuration](reference/config.md) — every key in `curator.toml`.
- [Integrations](integrations/curio.md) — Curio, Zotero, or your own
  producer.
