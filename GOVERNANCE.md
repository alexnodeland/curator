# Governance

## Model: single maintainer (BDFL)

The Knowledge Plane is designed, built, and maintained by one person. The
maintainer has final say on scope, design, contracts, releases — and the
still-open license decision.

## What is governed strictly

- **The four published contracts** (`contracts/`): changes follow the
  written versioning discipline — additive = minor, breaking = major with
  a new spec file; v1 semantics are never edited after publication; every
  change is changelogged. This discipline binds the maintainer too.
- **The public-safety boundary:** the repo describes a product, never any
  private deployment. The grep litmus enforces this in CI and is not
  bypassable by policy.
- **Human authority over agent writes:** no change may add a write path
  that bypasses `proposals/v1`. This is an architectural invariant, not a
  preference.

## What is informal

Everything else — internals, roadmap ordering, tooling — is at the
maintainer's discretion, guided by the design record in
[docs/design/decisions.md](docs/design/decisions.md).

## Evolution

If the project attracts sustained outside contribution after launch,
this document graduates: named co-maintainers, a lightweight RFC process
for contract changes, and a documented release rotation. Until then,
simplicity is the governance feature.
