# syntax=docker/dockerfile:1.7
# SPDX-License-Identifier: Apache-2.0
#
# panoscope container image.
#
# Build context is the REPO ROOT. panoscope-core comes from the goatcore
# repository by tag (Cargo.toml), so the build fetches it over the network.
#
# Build with:
#   docker build -f ops/panoscope/Dockerfile -t panoscope .
# from the repo root. CI invokes this via context: . + file:
# ops/panoscope/Dockerfile in .github/workflows/panoscope-image.yml.
#
# Two-stage build:
#   1. rust-builder: `cargo build --release` produces the binary;
#      rust-embed inlines frontend/web/ (plain HTML, CSS and JavaScript
#      modules, no frontend build step) into it.
#   2. Runtime: the runtime-cc base (ops/base-images/, goatcore ADR 1220) — single static
#      binary on a minimal distroless base.

# Base images are build arguments named for their role; the defaults are
# the public images in ops/base-images/ (goatcore docs/design/base-images.md).
ARG BASE_RUST_BUILD=cgr.dev/chainguard/rust:latest-dev@sha256:bb029397076cc17b3a96b4d5183b4c362d8323a6d36394c71a21e8e5706664af
ARG BASE_RUNTIME_CC=cgr.dev/chainguard/glibc-dynamic:latest@sha256:671a621016fb33812136dea6182b1db957996d3c8c618187aaf86abd9b14af53

# -----------------------------------------------------------------
# Stage 1: rust binary (with embedded frontend)
# -----------------------------------------------------------------
# The stage switches to root: cargo writes to /build/.../target, which a
# base whose default user is non-root cannot create.
FROM ${BASE_RUST_BUILD} AS rust-builder
USER root
WORKDIR /build

# Stub-build deps first so they cache as a separate Docker layer. The
# Cargo.lock-derived dep graph is ~400 crates (zenoh transport+link+ACL
# tree); compiling them takes ~10 min. Without this split the entire
# tree recompiles on every src/ change. With it, only the panoscope
# crate itself recompiles on src changes (~30s).
#
# Layout under /build mirrors repo paths:
#   /build/ops/panoscope/{Cargo.toml,...}
COPY ops/panoscope/Cargo.toml ops/panoscope/Cargo.lock ./ops/panoscope/
COPY ops/panoscope/build.rs ./ops/panoscope/
RUN mkdir -p ops/panoscope/src \
    && echo "fn main() {}" > ops/panoscope/src/main.rs
# BuildKit cache mounts persist cargo registry + git + target/ across
# the build's RUN steps; combined with the workflow's GHA layer cache,
# repeat builds for code-only changes drop from ~12min to ~2min.
RUN --mount=type=cache,target=/root/.cargo/registry,sharing=locked \
    --mount=type=cache,target=/root/.cargo/git,sharing=locked \
    --mount=type=cache,target=/build/ops/panoscope/target,sharing=locked \
    cd ops/panoscope \
    && cargo build --release --locked \
    && find target -maxdepth 4 -name '*panoscope*' -prune -exec rm -rf {} +
# Real source + the frontend files rust-embed inlines. We must copy the
# binary out of the cache mount before the RUN finishes — cache mounts
# aren't part of the final image layer.
COPY ops/panoscope/src/ ./ops/panoscope/src/
COPY ops/panoscope/frontend/web/ ./ops/panoscope/frontend/web/
RUN --mount=type=cache,target=/root/.cargo/registry,sharing=locked \
    --mount=type=cache,target=/root/.cargo/git,sharing=locked \
    --mount=type=cache,target=/build/ops/panoscope/target,sharing=locked \
    cd ops/panoscope \
    && cargo build --release --locked \
    && cp target/release/panoscope /build/panoscope

# -----------------------------------------------------------------
# Stage 2: runtime
# -----------------------------------------------------------------
FROM ${BASE_RUNTIME_CC}

# Non-root user comes from the base image (`nonroot`, uid 65532).
USER nonroot

WORKDIR /app
COPY --from=rust-builder /build/panoscope /app/panoscope

EXPOSE 8080

# Healthcheck via /healthz — the panoscope binary itself exposes this
# once the zenoh session opens. wget/curl aren't shipped on glibc-
# dynamic, so the actual healthcheck happens at the compose layer via
# docker-compose `healthcheck:` directive against a probe container,
# or at the Ansible role layer via `uri:` module post-deploy.

ENTRYPOINT ["/app/panoscope"]
