---
name: release
description: Cut a Knowledge Plane release — gate, version, changelog, tag. Use when asked to release, tag, or publish a version.
---

# Release procedure

Pre-release phase: no crates.io publishing (`publish = false` everywhere),
no binary distribution yet, **no LICENSE decision made** — a release is a
tagged, gated snapshot.

## Steps

1. **Gate.** `just ci` must be green locally (fmt, clippy -D warnings,
   hermetic tests, litmus). Never tag around a red gate.
2. **Contract audit.** For each of the four contracts, confirm the code
   still matches the spec and every change since the last tag has a
   `CHANGELOG.md` entry in its contract dir. A contract change without a
   changelog entry blocks the release.
3. **Version.** Bump `[workspace.package] version` in the root
   `Cargo.toml` (one version for the whole workspace). SemVer against the
   CONTRACTS, not the code: breaking contract change → major (0.x: minor),
   additive → minor, internal-only → patch.
4. **Changelog.** Update the root `CHANGELOG.md` (create on first
   release) from Conventional Commit subjects since the last tag, grouped
   feat/fix/docs/chore.
5. **Commit + tag.** `chore(release): vX.Y.Z` commit, then an annotated
   tag `vX.Y.Z` whose message lists contract versions
   (e.g. `kp-note/v1, kp-config/v1, proposals/v1, mcp/v1`).
6. **Do NOT push** unless the maintainer says so — no remote may even
   exist. Do NOT add a LICENSE file or license metadata as part of a
   release; that decision is the maintainer's alone.

## Not yet in scope (post-launch)

crates.io publication order (curator-core → curator-index → … → curator-cli), binary
artifacts, install scripts. When these land, this skill gets amended —
until then a release is a tag.
