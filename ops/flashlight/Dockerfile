# syntax=docker/dockerfile:1.7
# SPDX-License-Identifier: Apache-2.0
#
# flashlight container image.
#
# Build context is the REPO ROOT: flashlight path-depends on
# crates/flashlight-analytics/, which sits outside ops/flashlight/.
# panoscope-core comes from the goatcore repository by tag (Cargo.toml),
# so the build fetches it over the network.
#
# Build with:
#   docker build -f ops/flashlight/Dockerfile -t flashlight .
# from the repo root. CI invokes via context: . + file:
# ops/flashlight/Dockerfile in .github/workflows/flashlight-image.yml.
#
# Three-stage build:
#   1. wasm-builder: produces the Tier 2 browser WASM module + JS glue
#      from `crates/flashlight-analytics/` via `cargo build --target
#      wasm32-unknown-unknown` + `wasm-bindgen`. Output dropped at
#      `ops/flashlight/src/assets/wasm/` so the rust-builder stage's
#      `rust-embed` picks it up at compile time. Phase E Tier 2.
#   2. rust-builder: `cargo build --release` produces the binary; the
#      design-system CSS + flashlight.css + app.js are include_str!'d
#      and the wasm-builder's output is rust-embed'd so no separate
#      node toolchain or asset-bundling step is needed.
#   3. Runtime: the runtime-cc base (ops/base-images/, goatcore ADR 1220) — single static
#      binary on a minimal distroless base.

# Base images are build arguments named for their role; the defaults are
# the public images in ops/base-images/ (goatcore docs/design/base-images.md).
ARG BASE_RUST_BUILD=docker.io/library/rust:1.92-bookworm@sha256:e90e846de4124376164ddfbaab4b0774c7bdeef5e738866295e5a90a34a307a2
ARG BASE_RUNTIME_CC=gcr.io/distroless/cc-debian13:nonroot@sha256:08476c244888bfb425bae1ae4dd7c17f093e972aab8426883d97f3c6f280f7c1

# -----------------------------------------------------------------
# WASM builder — Tier 2 module (browser-side flashlight-analytics)
# -----------------------------------------------------------------
# The wasm-builder stays on the official `rust` Debian image whatever
# the rust-build role names, because a Wolfi-packaged rustc (Chainguard's
# `rust:latest-dev`) ships a wolfi-packaged rustc on
# PATH that bypasses rustup, so `rustup target add wasm32-unknown-
# unknown` reports success but cargo still hits `E0463: can't find
# crate for std` at build time. The official Rust image is
# rustup-managed end-to-end and handles `target add` as documented.
# Justified because the produced artifact (WASM bytes + JS glue) is
# COPYed into the rust-builder + runtime stages; the runtime image
# is the runtime-cc role.
FROM rust:1-bookworm AS wasm-builder
WORKDIR /build
RUN rustup target add wasm32-unknown-unknown
# wasm-bindgen-cli must match the wasm-bindgen crate version in Cargo.lock
# EXACTLY — the bindgen schema is unstable across patch releases and the
# CLI refuses a mismatch. A floating `^0.2` here resolved to 0.2.128 on
# while the lock held 0.2.122 and every flashlight image build
# went red with no commit touching either. So read the version out of the
# lock: the lock is copied on its own first so this layer only rebuilds
# when the lock changes, not on every source edit.
COPY crates/Cargo.lock ./crates/Cargo.lock
RUN V="$(awk '/^name = "wasm-bindgen"$/ { getline; gsub(/"/, "", $3); print $3; exit }' crates/Cargo.lock)" \
    && test -n "$V" \
    && echo "wasm-bindgen-cli =$V (from crates/Cargo.lock)" \
    && cargo install --locked wasm-bindgen-cli --version "=$V"

# Pull the full workspace (every member's Cargo.toml + src/). Cargo
# validates the workspace before scoping to `-p flashlight-analytics
# --lib`; if any `members = [...]` entry's Cargo.toml is missing,
# even a fresh workspace member that this build
# doesn't touch, cargo refuses to load. The -p scope keeps the
# actual compile narrow — only the flashlight-analytics dep graph
# compiles, not stethoscope / netvis / etc. — but the manifest
# tree has to be intact.
COPY crates/ ./crates/

RUN --mount=type=cache,target=/root/.cargo/registry,sharing=locked \
    --mount=type=cache,target=/root/.cargo/git,sharing=locked \
    --mount=type=cache,target=/build/crates/target,sharing=locked \
    cd crates \
    && cargo build --release --target wasm32-unknown-unknown \
        -p flashlight-analytics --lib \
    && wasm-bindgen \
        --target web \
        --no-typescript \
        --out-dir /build/wasm-out \
        target/wasm32-unknown-unknown/release/flashlight_analytics.wasm

# -----------------------------------------------------------------
# Rust builder — flashlight server binary (links flashlight-analytics
# native rlib + embeds the wasm-builder's Tier 2 output via rust-embed).
# -----------------------------------------------------------------
FROM ${BASE_RUST_BUILD} AS rust-builder
USER root
WORKDIR /build

# Stub-build deps first so they cache as a separate Docker layer. Same
# pattern panoscope's Dockerfile uses; flashlight's dep graph is
# narrower but still benefits from the split (zenoh transport + axum
# tree dominates compile time).
#
# Layout under /build mirrors the repo so flashlight's path dependency
# on flashlight-analytics resolves naturally:
#   /build/crates/flashlight-analytics/{Cargo.toml,src/}
#   /build/ops/flashlight/{Cargo.toml,Cargo.lock,src/}
#   /build/docs/design-system/{colors_and_type,components}.css
# flashlight-analytics path-dep landed with Phase D's /api/compute
# endpoint — same crate as the browser-side WASM module, linked here
# as the native rlib target.
COPY crates/flashlight-analytics/Cargo.toml ./crates/flashlight-analytics/
COPY crates/flashlight-analytics/src/ ./crates/flashlight-analytics/src/
COPY ops/flashlight/Cargo.toml ops/flashlight/Cargo.lock ./ops/flashlight/
RUN mkdir -p ops/flashlight/src/assets docs/design-system \
    && echo "fn main() {}" > ops/flashlight/src/main.rs \
    && echo "" > docs/design-system/colors_and_type.css \
    && echo "" > docs/design-system/components.css \
    && echo "" > ops/flashlight/src/assets/flashlight.css \
    && echo "" > ops/flashlight/src/assets/app.js
# BuildKit cache mounts persist cargo registry + git + target/ across
# the build's RUN steps; combined with the workflow's GHA layer cache,
# repeat builds for code-only changes drop sharply.
RUN --mount=type=cache,target=/root/.cargo/registry,sharing=locked \
    --mount=type=cache,target=/root/.cargo/git,sharing=locked \
    --mount=type=cache,target=/build/ops/flashlight/target,sharing=locked \
    cd ops/flashlight \
    && cargo build --release --locked \
    && find target -maxdepth 4 -name '*flashlight*' -prune -exec rm -rf {} +

# Real source + design-system CSS sources.
COPY ops/flashlight/src/ ./ops/flashlight/src/
COPY docs/design-system/colors_and_type.css docs/design-system/components.css ./docs/design-system/

# Pull the Tier 2 WASM artifacts produced by the wasm-builder stage
# into the path rust-embed scans. The folder may exist with .gitkeep
# in the repo; this COPY overwrites it with the real artifacts.
COPY --from=wasm-builder /build/wasm-out/ ./ops/flashlight/src/assets/wasm/

# Copy the binary out of the cache mount before the RUN finishes —
# cache mounts aren't part of the final image layer.
RUN --mount=type=cache,target=/root/.cargo/registry,sharing=locked \
    --mount=type=cache,target=/root/.cargo/git,sharing=locked \
    --mount=type=cache,target=/build/ops/flashlight/target,sharing=locked \
    cd ops/flashlight \
    && cargo build --release --locked \
    && cp target/release/flashlight /build/flashlight

# -----------------------------------------------------------------
# Runtime
# -----------------------------------------------------------------
FROM ${BASE_RUNTIME_CC}

USER nonroot
WORKDIR /app
COPY --from=rust-builder /build/flashlight /app/flashlight

EXPOSE 8080

# Healthcheck lives at /healthz inside the binary. The runtime-cc
# base ships no shell/wget/curl; the compose layer's
# `healthcheck:` is what runs the probe.
ENTRYPOINT ["/app/flashlight"]
