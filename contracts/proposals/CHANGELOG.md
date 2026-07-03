# `proposals` contract changelog

## v1 — 2026-07-03

- Initial publication: `<vault>/.kp/proposals/<ULID>/` layout with
  `proposal.json` + `changes.patch`; `open | applied | rejected`
  lifecycle; deterministic local-first validator (`.curio/**` refusal,
  managed-region refusal, vault-boundary refusal, clean-apply refusal,
  identity uniqueness); auto-applicable create-only digest rule.
