# Security Policy

## Reporting

Pre-release: report privately to the maintainer (see repository owner
profile). Do not open public issues for vulnerabilities. You should
receive an acknowledgment within 7 days.

## Scope and model

Curator's security posture is structural, not bolted on:

- **Local-first.** The default deployment touches no network at all:
  vault, index, and MCP-over-stdio are all local. The only optional
  network surfaces are the Zotero Web API client (outbound, read-only)
  and the opt-in MCP HTTP transport.
- **Agents cannot write canonical content.** The only write path is
  `proposals/v1` — a deterministic validator plus explicit human
  application. Validator bypasses are the highest-severity bug class.
- **The MCP HTTP transport requires a bearer token** (env-injected);
  there is no unauthenticated network mode. Report any path that serves
  without the token.
- **Secrets never live in config files** — `kp-config/v1` only names
  environment variables. A code path that logs or persists a secret
  value is a vulnerability.
- **Hermetic supply chain posture:** vendored producer schemas are
  sha-pinned; the builtin embedding model is hash-verified at fetch.

## Supported versions

Pre-release: only the tip of `main` receives fixes.
