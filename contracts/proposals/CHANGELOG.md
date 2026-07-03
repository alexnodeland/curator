# `proposals` contract changelog

## v1 clarifications — 2026-07-03 (same day, pre-publication review)

- Hard-reject 5 reworded: identity is never absent under kp-note/v1
  (plain notes carry the implicit `path:<relpath>` identity), so the rule
  is duplicate-identity only — now explicitly across BOTH explicit
  `kp_id` and implicit `path:` identities. The validator was tightened to
  match (implicit collisions were previously admitted).
- Hard-reject 1 now names the full dot-component rule the validator
  enforces (any dot-named component at any depth, not only `.curio/**`).
- CLI table: `kp propose` documented as generated-content only
  (`--from <dir>`); the staged-changes mode is explicitly future work.

## v1 — 2026-07-03

- Initial publication: `<vault>/.kp/proposals/<ULID>/` layout with
  `proposal.json` + `changes.patch`; `open | applied | rejected`
  lifecycle; deterministic local-first validator (`.curio/**` refusal,
  managed-region refusal, vault-boundary refusal, clean-apply refusal,
  identity uniqueness); auto-applicable create-only digest rule.
