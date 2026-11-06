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
# Three-stage build:
#   1. node-builder: `npm run build` produces frontend/dist/ via the
#      same Vite pipeline operators use locally.
#   2. rust-builder: `cargo build --release` produces the binary; the
#      build.rs picks up the dist/ from stage 1 and rust-embed inlines
#      it into the binary.
#   3. Runtime: the runtime-cc base (ops/base-images/, goatcore ADR 1220) — single static
#      binary on a minimal distroless base.

# Base images are build arguments named for their role; the defaults are
# the public images in ops/base-images/ (goatcore docs/design/base-images.md).
ARG BASE_NODE_BUILD=docker.io/library/node:22-bookworm-slim@sha256:5ec23b7c622b00547753bc6ffd78085dd868a68aeb41c8e0a1418f033e13d96d
ARG BASE_RUST_BUILD=docker.io/library/rust:1.92-bookworm@sha256:e90e846de4124376164ddfbaab4b0774c7bdeef5e738866295e5a90a34a307a2
ARG BASE_RUNTIME_CC=gcr.io/distroless/cc-debian13:nonroot@sha256:08476c244888bfb425bae1ae4dd7c17f093e972aab8426883d97f3c6f280f7c1

# -----------------------------------------------------------------
# Stage 1: frontend bundle
# -----------------------------------------------------------------
# The stage switches to root: `npm ci` writes node_modules into the
# workdir, which a base whose default user is non-root cannot create.
FROM ${BASE_NODE_BUILD} AS node-builder
USER root
WORKDIR /build
COPY ops/panoscope/frontend/package.json ops/panoscope/frontend/package-lock.json ./frontend/
RUN cd frontend && npm ci
COPY ops/panoscope/frontend/ ./frontend/
RUN cd frontend && npm run build

# -----------------------------------------------------------------
# Stage 2: rust binary (with embedded frontend)
# -----------------------------------------------------------------
# Same dev-variant rationale: cargo writes to /build/.../target.
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
RUN mkdir -p ops/panoscope/src ops/panoscope/frontend/dist \
    && echo "fn main() {}" > ops/panoscope/src/main.rs \
    && echo '{"name":"stub","version":"0.0.0"}' > ops/panoscope/frontend/package.json \
    && echo '<html></html>' > ops/panoscope/frontend/dist/index.html
# BuildKit cache mounts persist cargo registry + git + target/ across
# the build's RUN steps; combined with the workflow's GHA layer cache,
# repeat builds for code-only changes drop from ~12min to ~2min.
RUN --mount=type=cache,target=/root/.cargo/registry,sharing=locked \
    --mount=type=cache,target=/root/.cargo/git,sharing=locked \
    --mount=type=cache,target=/build/ops/panoscope/target,sharing=locked \
    cd ops/panoscope \
    && cargo build --release --locked \
    && find target -maxdepth 4 -name '*panoscope*' -prune -exec rm -rf {} +
# Real source + frontend dist from stage 1.
COPY ops/panoscope/src/ ./ops/panoscope/src/
COPY ops/panoscope/frontend/package.json ./ops/panoscope/frontend/package.json
COPY --from=node-builder /build/frontend/dist ./ops/panoscope/frontend/dist
# build.rs sees the existing dist/ and skips npm; no node toolchain
# is required in this stage. We must copy the binary out of the cache
# mount before the RUN finishes — cache mounts aren't part of the
# final image layer.
RUN --mount=type=cache,target=/root/.cargo/registry,sharing=locked \
    --mount=type=cache,target=/root/.cargo/git,sharing=locked \
    --mount=type=cache,target=/build/ops/panoscope/target,sharing=locked \
    cd ops/panoscope \
    && cargo build --release --locked \
    && cp target/release/panoscope /build/panoscope

# -----------------------------------------------------------------
# Stage 3: runtime
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
