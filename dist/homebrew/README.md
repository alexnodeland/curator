# Homebrew distribution

`curator` (a CLI) is distributed as a **build-from-source Homebrew formula** in
the tap [`alexnodeland/homebrew-tap`](https://github.com/alexnodeland/homebrew-tap).
[`Formula/curator.rb`](Formula/curator.rb) here is the **source of truth**; the
tap is a thin mirror. This repo never pushes to the tap.

The formula builds with `--no-default-features --features embed-onnx-dynamic`,
so `cargo` never downloads ONNX Runtime at build time; the installed binary
loads Homebrew's `onnxruntime` at runtime (a `bin` wrapper sets
`ORT_DYLIB_PATH`). `curator` therefore ships the **real** bge-small-en-v1.5
semantic embedder, not the offline `hash` fallback — the pinned ~130 MB model
is fetched on first use into the vault's `.kp/models`.

## Installing (end users)

```sh
brew install alexnodeland/tap/curator
curator init ~/vault
curator --help
```

`brew` pulls in `onnxruntime` and `rust` (build-only) automatically.

## Publishing the formula (maintainer)

Unlike Curio's `version :latest` cask, a **formula pins `url` + `sha256`**, so
each release updates them.

1. Cut the GitHub release: push a `vX.Y.Z` tag (`release.yml` runs the
   real-model e2e gate, builds the binaries + `SHA256SUMS`, and drafts the
   release — see [`docs/release/runbook.md`](../../docs/release/runbook.md)),
   then publish the draft.

2. Get the source-tarball sha256 (GitHub generates the tarball from the tag):

   ```sh
   curl -sL https://github.com/alexnodeland/curator/archive/refs/tags/vX.Y.Z.tar.gz \
     | shasum -a 256
   ```

3. Update [`Formula/curator.rb`](Formula/curator.rb) here — bump the `url` tag
   and replace the `sha256` — then copy it into the tap and push:

   ```sh
   git clone https://github.com/alexnodeland/homebrew-tap
   cp dist/homebrew/Formula/curator.rb homebrew-tap/Formula/curator.rb
   cd homebrew-tap && git add Formula/curator.rb \
     && git commit -m "curator: vX.Y.Z" && git push
   ```

4. Verify on a clean machine:

   ```sh
   brew update
   brew install alexnodeland/tap/curator
   curator --version
   brew test alexnodeland/tap/curator      # runs the formula's test block
   brew audit --formula --online alexnodeland/tap/curator   # optional lint
   ```

## Why build-from-source (not a bottle or binary formula)

`release.yml` builds `curator` binaries for linux-x86_64 + macOS-arm64 only —
no Intel-macOS or universal binary. A binary formula would strand Intel Macs.
Build-from-source with a runtime-loaded system `onnxruntime` covers every
`rust` + `onnxruntime` platform Homebrew supports, from one formula, and keeps
the ML runtime a shared, updatable dependency rather than a bundled download.
