# The optional enhancer — an agent as prose polish, never a dependency

**Status:** design only. There is deliberately **no LLM code in this
repository** (decisions.md §5): the baseline digest is pure code, and the
plane is fully functional — ingest, index, search, digest, proposals —
with no agent installed, no API key configured, no model downloaded.

## What an enhancer is

An enhancer is any external agent process (a scheduled Claude Code
session, a cron-driven script around some other harness, a human with
strong opinions) that rewrites the *prose* of a deterministic digest.
It rides the same two published surfaces every other consumer uses:

- **read** — the MCP tools (`kp_digest_latest`, `kp_get_note`,
  `kp_search`, `kp_related`) or plain files in the vault;
- **write** — `proposals/v1`, the only write path. No enhancer ever
  writes vault files directly.

Harness neutrality is a corollary of the contract: *any* agent that
produces conforming proposals is a valid librarian enhancer. There is no
SDK to link against, no plugin API to implement, no vendor named anywhere
in the loop.

## The loop, step by step

1. **The deterministic digest runs first** (scheduled `curator digest run
   --auto`). It writes `digests/<date>.md` through an auto-applied
   proposal: wikilinks, extractive one-line summaries, why-surfaced
   notes, quiet tail. This artifact is complete and useful as-is —
   everything after this point is optional.
2. **The enhancer wakes on its own schedule** and reads the latest digest
   (`kp_digest_latest`, or the proposal record under
   `.kp/proposals/<ULID>/`). The proposal's `rationale` and the digest's
   why-notes give it the full deterministic reasoning: which notes
   surfaced and why.
3. **It may read around** — `kp_get_note` for full bodies, `kp_related`
   for context — to write better prose: real summaries instead of
   extractive first paragraphs, connective tissue between clustered
   items, a headline.
4. **It submits a superseding proposal** (`kp_propose` /
   `curator propose`): the same digest note path, rewritten body. Two hard
   shape rules, enforced by the validator and by convention:
   - the **frontmatter identity is preserved** — same `kp_id`, same
     `kp_schema`, same `tags: [digest]`; the enhancer changes prose,
     never identity (a changed machine key on a Curio note would be
     hard-rejected; digest notes deserve the same discipline);
   - the **structure is preserved** — every surfaced note keeps its
     wikilink; the enhancer may reorder within clusters, rewrite
     sentences, and retitle sections, but a digest that *drops* items is
     not an enhancement, it is a different digest.
5. **A human (or a standing rule) applies it** — `curator apply <id>`. This
   proposal *modifies* an existing file outside nothing-but-additions
   territory, so **it never auto-applies**: the auto-apply gate admits
   only pure additions under the digest dir. Enhanced prose always has a
   review step, which is exactly the trust boundary an LLM in the write
   path should sit behind.

## Failure semantics

The system never waits for the enhancer:

| failure | effect |
|---|---|
| enhancer never runs | the deterministic digest stands — complete, linked, scored |
| enhancer produces garbage | it is a proposal; review rejects it, the digest stands |
| enhancer edits the managed surface of a Curio note | validator hard-reject, stamped `rejected` |
| enhancer patch races a vault change | non-clean application, hard-reject; re-propose against the new tree |
| enhancer vendor/API goes away | nothing in the plane references it; unschedule the job |

This is the whole point of deterministic-first (decisions.md §5): the
enhancer changes prose *quality*; it can never change artifact *shape*,
*safety*, or *availability*.

## Non-goals

- No enhancer harness ships in this repo — not even an adapter trait.
  The seam is the proposals contract, not a Rust API.
- No "enhancer mode" configuration: the plane cannot tell whether its
  proposals come from an agent, a script, or a person, and must not care.
- No LLM-scored ranking. Scoring stays deterministic (anchor similarity ×
  recency × behavior); an agent that disagrees with the ranking can say
  so in prose, not reorder the score.
