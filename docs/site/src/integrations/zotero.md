# Zotero

Curator treats a Zotero library as a second canonical store and syncs
it into the vault as literature notes — **read-only against Zotero**,
one file per item, keyed strictly on the item key.

## Two channels

| channel | source | carries |
|---|---|---|
| metadata | Zotero Web API, delta polling | items, creators, tags, collections — resumed from the library's `Last-Modified-Version`, with `/deleted` tombstones |
| fulltext | official `/fulltext` endpoint **(primary)**, WebDAV fallback | extracted attachment text for search |

Metadata sync is incremental by construction: Curator stores the
library version cursor in the index and asks only for what changed
since. An unchanged library is a no-op round trip.

Fulltext comes from Zotero's own `/fulltext` endpoint first — the text
Zotero has already extracted. For self-hosted attachment stores there
is a deliberately small **CRC-verified WebDAV `.prop`/`.zip` fallback**
(`webdav_fallback = true`): it fetches the attachment archive, verifies
the CRC recorded in the `.prop` sidecar, and extracts text locally.
Long documents are truncated at a configurable cap
(`--fulltext-cap`, default 20 000 characters).

## Notes land as managed regions

Each item becomes a `kp-note/v1` file in a configured vault directory
(default `zotero/`), with identity `zotero:<itemKey>`:

- The machine content lives inside a `kp-zotero:managed`
  comment-marker region — a producer writing its own marked region,
  [exactly like Curio](curio.md#ownership-the-manifest-oracle).
- Re-syncs replace **only the managed region**; anything you write
  below it, and any extra frontmatter keys, ride along untouched.
- Because identity is the item key, a citekey or title rename updates
  the same note — never a duplicate.
- Library tombstones delete only *pristine* (fully machine-owned)
  files; anything you edited moves to `.kp/trash/`, never deleted.

## Setup

```toml
[zotero]
enabled = true
api_base = "https://api.zotero.org"
user_id = "1234567"                 # your Zotero user id
api_key_env = "KP_ZOTERO_KEY"       # env var NAME — never the key itself
webdav_fallback = false
webdav_url = ""
```

The API key is read from the environment variable named by
`api_key_env` — secrets never live in the config file.
`CURATOR_ZOTERO_KEY` is the preferred alias and wins when both are set;
`KP_ZOTERO_KEY` keeps working
([details](../reference/config.md#environment-variables)).

```sh
export CURATOR_ZOTERO_KEY="<key from zotero.org/settings/keys>"
curator zotero sync                  # delta metadata + fulltext
curator zotero sync --no-fulltext    # metadata only
curator zotero sync --json           # machine-readable report
```

The sync report counts fetched/upserted/unchanged items, fulltext
added/missing, and tombstones processed. See the
[CLI reference](../reference/cli.md#curator-zotero-sync) for all flags.
