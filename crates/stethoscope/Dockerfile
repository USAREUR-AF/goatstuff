# syntax=docker/dockerfile:1.7
# SPDX-License-Identifier: Apache-2.0
#
# stethoscope container image. Single-stage Rust build → Chainguard
# glibc-dynamic distroless runtime per ADR 0700.
#
# Build context is the repo root — the render.rs `include_str!`
# references docs/design-system/{colors_and_type,components}.css and
# the workspace lives at crates/, so we need both paths visible.
# Build with:
#   docker build -f crates/stethoscope/Dockerfile -t stethoscope .
# from the repo root.
#
# History note: an earlier version of this Dockerfile used a two-RUN
# pattern (stub-build deps for cache reuse, then COPY real src + build
# again). That pattern shipped a 335 KB stub binary in the published
# image because the second build was a fingerprint cache-hit on the
# first build's stub binary (likely COPY-preserved mtimes confused
# cargo's freshness check). Replaced with a single-RUN COPY-then-build
# so the binary that gets `cp`'d is guaranteed to be the real one. The
# trade-off is slower cold builds (one full ~12 min compile, no stub-
# warm shortcut) but correctness over speed wins here. Cargo registry
# + git caches are still cache-mounted because those are content-
# addressed and safe to share across builds; only target/ is excluded.

# -----------------------------------------------------------------
# Stage 1: rust binary
# -----------------------------------------------------------------
FROM cgr.dev/chainguard/rust:latest-dev AS rust-builder
USER root
WORKDIR /build

# Copy everything the build needs in one shot — Cargo manifests, src,
# design-system CSS that render.rs include_str!s at compile time.
COPY crates/ ./crates/
COPY docs/design-system/colors_and_type.css ./docs/design-system/
COPY docs/design-system/components.css ./docs/design-system/

# Build the release binary. cp the binary OUT of the workspace's
# target/ to a stable path before this layer ends. The ls -la at the
# end is a smoke verification — if the binary is suspiciously small
# the build log surfaces it immediately instead of shipping a stub.
RUN --mount=type=cache,target=/usr/local/cargo/registry \
    --mount=type=cache,target=/usr/local/cargo/git \
    cd crates && cargo build --release -p stethoscope \
    && cp target/release/stethoscope /build/stethoscope \
    && ls -la /build/stethoscope

# -----------------------------------------------------------------
# Stage 2: runtime — Chainguard glibc-dynamic distroless
# -----------------------------------------------------------------
FROM cgr.dev/chainguard/glibc-dynamic:latest
COPY --from=rust-builder /build/stethoscope /usr/local/bin/stethoscope
EXPOSE 8080
ENTRYPOINT ["/usr/local/bin/stethoscope"]
