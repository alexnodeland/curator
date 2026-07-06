# The docs site

Sources for the public documentation site, rendered by the in-repo
generator — no external site tooling.

## Machinery

| piece | role |
|---|---|
| `src/**/*.md` | the pages (plain markdown; mermaid fences render) |
| `nav.json` | page order + sidebar labels — the single navigation source |
| `assets/` | stylesheet, mermaid init, and the **pinned, vendored** `mermaid.min.js` ([provenance](assets/README.md)) |
| `xtask/src/docs.rs` | the generator: `cargo run -p xtask -- docs` (front door: `just site`) |
| `target/site/` | the build output — gitignored, litmus-skipped, never committed |
| `.github/workflows/pages.yml` | builds on push to `main`, proves determinism (build twice, `diff -r`), deploys via `actions/deploy-pages` |

## Properties, enforced

- **Deterministic:** same sources → byte-identical site; pages.yml
  diffs two builds on every deploy.
- **Gated:** the build fails on a nav entry without a file, a page
  without an `# H1`, a relative `*.md` link that isn't a nav page, or
  a `#fragment` that matches no heading anchor on its target — `just
  site` runs in `just ci`, so a broken cross-reference can't merge.
- **Self-contained:** no CDN, no fonts, no network at view time;
  mermaid is vendored and loaded only by pages that use it.

## Writing pages

- Every page starts with one `# H1` (it becomes the `<title>` and the
  first heading anchor). Headings get GitHub-style slugs
  (`## Epochs, not migrations` → `#epochs-not-migrations`).
- Link between pages with relative `*.md` paths
  (`../reference/config.md#discovery`); the generator rewrites them to
  `*.html` and validates page + anchor.
- Adding a page = create the file under `src/` + add it to `nav.json`.
- Facts about behavior belong in the same commit as the behavior —
  the measured quickstart number states its date, hardware, and
  command, so it can be re-measured honestly.
