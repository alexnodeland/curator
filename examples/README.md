# Examples

Small, runnable demonstrations of the integration story: **any producer
that writes conforming markdown+frontmatter into the vault is a valid
producer** ([contracts/kp-note/v1.md](../contracts/kp-note/v1.md)).

## `sample-vault/`

A 12-note markdown vault — retrieval notes, a reading list, some
kitchen-and-garden life — used by `just demo` (the offline
walk-through) and `just e2e-real` (the real-embedder gate). It shows
the full frontmatter spectrum on purpose:

- notes **with** kp-note/v1 frontmatter (`title`, `created`, `tags`,
  `source`) — what producers emit;
- notes **without** any frontmatter (`zettelkasten-workflow.md`,
  `sourdough-hydration.md`) — indexed anyway under the implicit
  `path:` identity;
- `[[wikilinks]]` between notes — ingest records them as edges;
- `now.md` — the librarian's interest anchor (`curator digest run`
  scores new notes against it).

Try it without touching your own notes:

```sh
just demo          # scratch copy, offline hash embedder, zero downloads
```

or by hand:

```sh
cp -R examples/sample-vault /tmp/my-vault
cargo run -p curator-cli -- init /tmp/my-vault
cargo run -p curator-cli -- search "hybrid retrieval" --config /tmp/my-vault/curator.toml
```

## `rss-to-notes.sh`

A complete producer in ~150 lines of portable shell: fetches an RSS 2.0
or Atom feed and writes one markdown note per item into a vault
directory — kp-note/v1 frontmatter (`title`, `created`, `source`,
`tags`), no `kp_id` (the plane assigns `path:` identity), deterministic
filenames derived from each item's link so re-runs are idempotent and
never clobber notes the plane has since enriched.

```sh
examples/rss-to-notes.sh https://example.com/feed.xml ~/vault/clips
curator ingest     # pick the new notes up
```

Dependencies: `curl`, `awk`, `sha256sum`/`shasum` — nothing else. It is
deliberately a sketch (crude HTML stripping, minimal entity handling):
the point is the *shape* of a producer, not feed-parser completeness.

## `compose/`

The container deployment's config half: `curator.toml` tuned for the
paths and bind address the [compose file](../compose.yaml) mounts. See
the operations docs for the full container story.
