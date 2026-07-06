# Curator — container image.
#
# Multi-stage: a pinned Rust builder compiles the release `curator`
# binary (default features — the in-process ONNX embedder is compiled
# in; the ort download feature fetches ONNX Runtime binaries at BUILD
# time, so the build needs network but the image is self-contained), a
# slim Debian runtime carries only the binary + TLS roots.
#
#   docker build -t curator .
#   docker run --rm curator --version
#
# The runtime contract (see compose.yaml for the worked example):
#   - config is a bind mount:    /work/curator.toml (no secrets in it)
#   - the vault is a bind mount: /work/vault
#   - derived state is a volume: /work/state  (index.db + models/ —
#     disposable by design; `curator index rebuild` regenerates it)
#   - secrets ONLY via env (CURATOR_MCP_TOKEN, CURATOR_ZOTERO_KEY —
#     legacy KP_* names still honored), never files or argv
#   - `mcp serve --http` requires [mcp].http_bind = "0.0.0.0:8377" in
#     the mounted config to be reachable from outside the container
#     (the shipped default binds loopback, which is correct everywhere
#     except inside a container).

# --- build stage -----------------------------------------------------------
# Pinned to the workspace's proven toolchain line (rust-version = 1.89
# is the floor; 1.94 is what CI and the reference builds run).
FROM rust:1.94-slim-trixie AS builder

# native-tls build inputs (the ort download feature and the Zotero
# client link against OpenSSL) + g++ (the prebuilt ONNX Runtime static
# libs link against libstdc++).
RUN apt-get update \
    && apt-get install -y --no-install-recommends \
        ca-certificates \
        g++ \
        libssl-dev \
        pkg-config \
    && rm -rf /var/lib/apt/lists/*

WORKDIR /src
COPY . .

# --locked: the committed Cargo.lock is the build; a drifted lockfile
# fails here, not at runtime.
RUN cargo build --release --locked -p curator-cli

# --- runtime stage ---------------------------------------------------------
FROM debian:trixie-slim

# ca-certificates: HTTPS to the Zotero API + the one-time model fetch.
# libssl3t64: native-tls at runtime.
RUN apt-get update \
    && apt-get install -y --no-install-recommends \
        ca-certificates \
        libssl3t64 \
    && rm -rf /var/lib/apt/lists/* \
    && useradd --system --create-home --uid 10001 curator \
    # Pre-create the mount points OWNED BY the runtime user: a named
    # volume initializes from the image's directory (content AND
    # ownership), so /work/state is writable by uid 10001 from first
    # `up` — without this, the volume comes up root-owned and every
    # index write fails with EACCES.
    && mkdir -p /work/state /work/vault \
    && chown -R curator /work

COPY --from=builder /src/target/release/curator /usr/local/bin/curator

USER curator
WORKDIR /work

# The MCP surface's HTTP port ([mcp].http_bind in the mounted config).
EXPOSE 8377

ENTRYPOINT ["/usr/local/bin/curator"]
CMD ["--help"]
