# Release runbook

How a Curator release is cut. `curator` is a Rust CLI + MCP server; the
shipped channels are **`cargo install`** (primary, full-featured), a
**build-from-source Homebrew formula** (loads Homebrew's `onnxruntime` at
runtime — see [`dist/homebrew/README.md`](../../dist/homebrew/README.md)),
and **release binaries** (linux-x86_64 + macOS-arm64, informational). crates.io
is a deliberate staged follow-up while pre-release.

Two workflows back this: the per-PR gate ([`ci.yml`](../../.github/workflows/ci.yml)
— fmt, clippy, tests, rustdoc, litmus, lean-check, deterministic docs, coverage
gate, license-audit, secret-scan) and the tag-triggered
[`release.yml`](../../.github/workflows/release.yml).

## 0. Preconditions

- `main` is green on CI, and the weekly real-model
  [`e2e-real.yml`](../../.github/workflows/e2e-real.yml) is green.
- No open release blockers; the CLI has no advertised-but-unimplemented
  surface (`curator --help` matches the implemented commands).

## 1. Version

- [ ] `version` in `Cargo.toml` (`[workspace.package]`) is `X.Y.Z`. All crates
      inherit it via `version.workspace = true`.
- [ ] `curator --version` prints `X.Y.Z` from a clean build.

## 2. Tag → automated draft release

Pushing the tag triggers [`release.yml`](../../.github/workflows/release.yml):
it runs `just e2e-real` (the pinned-ONNX real-model end-to-end) as a hard
prerequisite gate, then builds `curator` for `x86_64-unknown-linux-gnu` +
`aarch64-apple-darwin` (`--release --locked`), smoke-tests `--version`,
packages each as `curator-vX.Y.Z-<target>.tar.gz` (binary + README +
`curator.example.toml`), generates `SHA256SUMS`, and opens a **draft** release
with generated notes. Nothing is public until you publish the draft in §4.

```sh
git checkout main && git pull
git tag -a vX.Y.Z -m "Curator vX.Y.Z"
git push origin vX.Y.Z          # → release.yml drafts the release
```

- [ ] The `release.yml` run for the tag is green and a **draft** `vX.Y.Z`
      release exists with both tarballs + `SHA256SUMS` attached.

## 3. Verify the artifacts

- [ ] Download a tarball; `curator --version` prints `X.Y.Z`.
- [ ] `shasum -a 256 -c SHA256SUMS` matches.

## 4. Publish the draft

- [ ] Edit the generated notes if needed (highlights, install snippet).
- [ ] Click **Publish release** — this makes `vX.Y.Z` the `latest` and creates
      the source tarball the Homebrew formula points at.

## 5. Publish the Homebrew formula

Follow [`dist/homebrew/README.md`](../../dist/homebrew/README.md): get the
source-tarball sha256, bump `url` + `sha256` in
[`dist/homebrew/Formula/curator.rb`](../../dist/homebrew/Formula/curator.rb),
copy it into `alexnodeland/homebrew-tap` under `Formula/`, and push.

- [ ] Tap updated with the new `url` + `sha256`.
- [ ] `brew install alexnodeland/tap/curator` installs `vX.Y.Z` on a clean
      machine; `brew test alexnodeland/tap/curator` passes.

## 6. Close out

- [ ] Announce; open the milestone for the next version.
- [ ] (Deferred) crates.io: publish `curator-core → … → curator-cli` in
      dependency order once `publish` is enabled.

## Rollback

Delete or supersede the bad GitHub release; for Homebrew, revert the tap's
`Formula/curator.rb` to the previous `url` + `sha256` (or publish a superseding
`vX.Y.(Z+1)`).
