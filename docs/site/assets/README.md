# Site assets

Static assets copied verbatim into the built site (`target/site/assets/`)
by `cargo run -p xtask -- docs`. This README is a source-side provenance
note and is **not** copied into the build.

## Vendored: `mermaid.min.js`

Mermaid is vendored so the site is fully self-contained — no CDN, no
network at view time — and so diagram rendering is pinned, not floating.

| field | value |
|---|---|
| package | `mermaid` (npm) |
| version | **11.16.0** (pinned) |
| file | `dist/mermaid.min.js` from the published tarball |
| source | `https://registry.npmjs.org/mermaid/-/mermaid-11.16.0.tgz` |
| sha256 | `74d7c46dabca328c2294733910a8aa1ed0c37451776e8d5295da38a2b758fb9b` |
| license | MIT (Mermaid project) |

To bump: download the new tarball, extract `package/dist/mermaid.min.js`,
replace the file, update this table (version, source URL, sha256), and
check the rendered diagrams on every page that has a mermaid fence.

## First-party

- `style.css` — the whole site stylesheet (light/dark via
  `prefers-color-scheme`), hand-maintained.
- `mermaid-init.js` — theme-aware mermaid initialization; loaded only by
  pages that contain a mermaid fence.
