# KP consumer review of the Curio contracts

The Knowledge Plane is the primary machine consumer of `curio.frontmatter.v1`
and `curio.events.v1`. Before the v1 freeze, the KP side filed a consumer
review upstream; every ask below was **adopted into the v1 contract draft**
that the vendored schemas in this directory pin, and is implemented by the
adapter in `kp-ingest`. Upstream ratification (or amendment) closes each
issue; a semantic amendment after ratification means a schema re-sync and a
new [`PIN`](PIN).

Upstream tracker: `github.com/alexnodeland/curio-rss`.

## Contract asks (blocking for v1)

| issue | ask | why KP needs it |
|---|---|---|
| [#5](https://github.com/alexnodeland/curio-rss/issues/5) | Negation events (`article.unstarred`, `read_later.removed`, `unarchived`, `untagged`, `feed.removed`) | State reconstruction folds the log; without negations, behavior sets are monotone-only and permanently wrong after any un-action. |
| [#6](https://github.com/alexnodeland/curio-rss/issues/6) | ULID `event_id` on every event | Replay idempotency: cursors can restart from older files safely only if consumers can dedupe by id. |
| [#7](https://github.com/alexnodeland/curio-rss/issues/7) | State-carrying payloads include `tags` | The librarian scores behavior without a join against Curio's private DB. |
| [#8](https://github.com/alexnodeland/curio-rss/issues/8) | `ts` format/ordering, cursor opacity, rotation + retention spelled out | The events tail persists `(file, line)` cursors across rotation at UTC midnight / 50 MB and tolerates ≥90-day pruning. |
| [#9](https://github.com/alexnodeland/curio-rss/issues/9) | Stable schema `$id`s, per-schema CHANGELOG, fetchable registry location | This vendor directory pins by sha; stable `$id`s make the pin meaningful. |
| [#11](https://github.com/alexnodeland/curio-rss/issues/11) | Managed-region markers + checksum scoped to region bytes (W2) | KP enrichment lives outside the region; the proposals validator enforces region byte-exactness, and `checksum` is a change token, never identity. |
| [#12](https://github.com/alexnodeland/curio-rss/issues/12) | Manifest write-ordering + git-mergeable format | The manifest is KP's write-ownership oracle; note-first/manifest-second ordering means a dangling manifest entry can't exist. |

## Producer-side asks (adopted, not adapter-visible)

| issue | ask |
|---|---|
| [#10](https://github.com/alexnodeland/curio-rss/issues/10) | Per-feed private-network allowlist (W1) so localhost digest feeds stay subscribable under the default-deny SSRF guard. |
| [#13](https://github.com/alexnodeland/curio-rss/issues/13) | Digest-feed convention (W6): a marker excluding agent-published digests from re-ingest loops. |
| [#14](https://github.com/alexnodeland/curio-rss/issues/14) | `curio later <url>` inbound capture (W5, post-v1). |
| [#15](https://github.com/alexnodeland/curio-rss/issues/15) | `article.updated` re-promotion semantics drift note. |

## Status

Filed and adopted into the v1 draft 2026-07-03; the pinned schemas here
already reflect every blocking ask. Issues stay open upstream until the
producer ratifies the freeze ("ratify or amend before Phase 3 freeze").
