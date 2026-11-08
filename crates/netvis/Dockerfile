# syntax=docker/dockerfile:1.7
# SPDX-License-Identifier: Apache-2.0
#
# netvis container image. Single-stage Rust build → the distroless
# runtime-cc base (ops/base-images/, goatcore ADR 1220), mirroring
# crates/microscope/Dockerfile.
#
# Build context is the repo root, because the binary include_str!s two things
# that live outside the crate: the design-system tokens and the port-set
# vocabulary. The goatnet's ACL declaration and rule intents are NOT built in:
# the netvis role mounts them (NETVIS_ACL_DECLARATION_DIR,
# NETVIS_RULE_INTENTS_FILE). Those COPY lines are load-bearing — an include_ that crosses the
# crate boundary without its build-context entry compiles locally and fails
# only in CI, which is exactly how microscope's image broke once already:
#
#   docker build -f crates/netvis/Dockerfile -t netvis .

# Base images are build arguments named for their role; the defaults are
# the public images in ops/base-images/ (goatcore docs/design/base-images.md).
ARG BASE_RUST_BUILD=cgr.dev/chainguard/rust:latest-dev@sha256:bb029397076cc17b3a96b4d5183b4c362d8323a6d36394c71a21e8e5706664af
ARG BASE_RUNTIME_CC=cgr.dev/chainguard/glibc-dynamic:latest@sha256:671a621016fb33812136dea6182b1db957996d3c8c618187aaf86abd9b14af53

# -----------------------------------------------------------------
# Stage 1: rust binary
# -----------------------------------------------------------------
FROM ${BASE_RUST_BUILD} AS rust-builder
USER root
WORKDIR /build

COPY crates/ ./crates/
# The page shell serves the design-system tokens.
COPY docs/design-system/colors_and_type.css ./docs/design-system/
# The warrant lens (ADR 1129) compiles in the port-set vocabulary, product
# data that is the same for every goatnet.
COPY ops/warrant/port-sets.yaml ./ops/warrant/

RUN --mount=type=cache,target=/usr/local/cargo/registry \
    --mount=type=cache,target=/usr/local/cargo/git \
    cd crates && cargo build --release -p netvis \
    && cp target/release/netvis /build/netvis \
    && ls -la /build/netvis

# -----------------------------------------------------------------
# Stage 2: runtime — the distroless runtime-cc base (ops/base-images/)
# -----------------------------------------------------------------
FROM ${BASE_RUNTIME_CC}
COPY --from=rust-builder /build/netvis /usr/local/bin/netvis
EXPOSE 9760
ENTRYPOINT ["/usr/local/bin/netvis"]
