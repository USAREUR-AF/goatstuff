# syntax=docker/dockerfile:1.7
# SPDX-License-Identifier: Apache-2.0
#
# netvis container image. Single-stage Rust build → the distroless
# runtime-cc base (ops/base-images/, goatcore ADR 1220), mirroring
# crates/microscope/Dockerfile.
#
# Build context is the repo root, because the binary include_str!s several
# things that live outside the crate: the design-system tokens, the two ACL
# YAMLs, and the warrant registry (port-sets + rule intents). Those COPY lines are load-bearing — an include_ that crosses the
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
# doctrine.rs compiles the declared vocabulary in, so what this service says a
# group is FOR cannot drift from what netbird-acl-bootstrap.yml creates.
COPY ops/ansible/netbird_acl_taxonomy.yml ./ops/ansible/
COPY ops/ansible/netbird_acl_matrix.yml ./ops/ansible/
# The warrant lens (ADR 1129) compiles in the port-set vocabulary and the rule
# intents, so a pair view can name the set a rule's ports ARE and say whether
# the grant expires. Exactly the failure this file's header warns about: both
# include_str!s compiled locally and broke only the image build.
COPY ops/warrant/port-sets.yaml ./ops/warrant/
COPY ops/warrant/rule-intents/netbird-acl.yaml ./ops/warrant/rule-intents/

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
