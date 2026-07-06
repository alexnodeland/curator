# Obsidian as viewer

Curator has no editor and wants none. Any tool that reads and writes
plain markdown works on a Curator vault; Obsidian is a recommended
viewer **and nothing more** — no plugin, no sync service, no
Curator-specific configuration inside it.

## Point both at the same directory

An Obsidian vault *is* a markdown+YAML directory, which is exactly what
Curator indexes. Run `curator init .` in an existing Obsidian vault (or
open a Curator-scaffolded directory as an Obsidian vault) and both
tools see the same files:

- **Frontmatter coexists.** Obsidian shows `kp-note/v1` keys
  (`kp_id`, `tags`, `source`, …) in its Properties panel; unknown keys
  are preserved by both sides — that's a
  [binding rule of the note contract](../concepts.md#identity-minted-never-derived).
- **Wikilinks are indexed.** Ingest extracts both `[[wikilinks]]`
  (including `[[target|display]]` and `[[target#heading]]`) and
  standard markdown links into the index's edge tables, so the link
  structure you build in Obsidian powers relatedness and one-hop
  expansion in search.
- **Digests are just notes.** Applied librarian digests land under
  your digest directory (default `digests/`) and render like any other
  note, links included.
- **`now.md` is just a note.** Edit the librarian's interest anchor
  wherever you like editing markdown.

## Obsidian's Web Viewer is also a producer

Obsidian's core **Web Viewer** plugin doesn't just browse the web in a tab —
its **Save to vault** turns the page you're reading into a markdown note with
YAML frontmatter (a `source` URL, `title`, `author`, date, `tags`). Curator's
**web-clip adapter** recognizes those notes and maps their frontmatter onto
`kp-note/v1` during ingest, so **ad-hoc web reading becomes a first-class
source** — searchable, citable by its source URL, tag-filterable, and
recency-ranked — right alongside your feeds and papers. Two capture arms, one
plane: subscriptions come in through Curio, ad-hoc pages through the Web Viewer,
and both land as markdown that Curator indexes uniformly.

- **Tolerant, not a pinned contract.** Clippers let you edit the property
  template, so the adapter *aliases* the common keys (`source` **or** `url`;
  `published`/`created`/`date`; a `tags` list or comma string) and never fails
  — a note that isn't a recognizable web clip (no `http(s)` source) is just an
  ordinary note. The defining signal is the source URL the clip came from.
- **The file is never rewritten.** Like every producer, the Web Viewer keeps
  byte-ownership of its own frontmatter; the adapter only synthesizes the
  in-memory indexed view. Identity stays `path:<relpath>` — no `kp-note/v1`
  contract change.
- **Any clipper works.** The mapping keys on shape, not on Obsidian: a browser
  extension, a shell script, or a different app that writes `source`-bearing
  markdown into the vault is recognized the same way.

`curator ingest` reports how many it mapped (`web clips: N …`, and `web_clips`
in `--json`).

## Two hygiene rules

1. **Keep derived state out of git.** `.kp/` (proposals staging,
   cursors, model cache) and `index.db` are the plane's working state.
   The scaffolded setup keeps the index inside `.kp/`; make sure your
   vault's `.gitignore` covers `.kp/` and `*.db`.
2. **Don't fight the managed regions.** Producer-owned notes (Zotero
   literature notes, Curio exports) carry marked machine regions;
   edit below the markers and your changes survive re-syncs
   ([how that works](zotero.md#notes-land-as-managed-regions)).

## What Curator does not do for Obsidian

Honesty section: Curator indexes markdown content, YAML frontmatter,
wikilinks, and standard markdown links. It is not an Obsidian plugin,
does not read `.obsidian/` configuration, and has no special handling
for other Obsidian-only syntax — vault features like canvases or
dataview queries are outside the contracts. If a future producer wants
to emit them, the [producer path](producers.md) is open.
