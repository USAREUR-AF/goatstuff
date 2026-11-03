# syntax=docker/dockerfile:1.7
# SPDX-License-Identifier: Apache-2.0
#
# netvis container image. Single-stage Rust build → Chainguard
# glibc-dynamic distroless runtime per ADR 0700, mirroring
# crates/microscope/Dockerfile.
#
# Build context is the repo root, because the binary include_str!s several
# things that live outside the crate: the design-system tokens, the two ACL
# YAMLs, and the warrant registry (port-sets + rule intents). Those COPY lines are load-bearing — an include_ that crosses the
# crate boundary without its build-context entry compiles locally and fails
# only in CI, which is exactly how microscope's image broke once already:
#
#   docker build -f crates/netvis/Dockerfile -t netvis .

# -----------------------------------------------------------------
# Stage 1: rust binary
# -----------------------------------------------------------------
FROM cgr.dev/chainguard/rust:latest-dev AS rust-builder
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
# Stage 2: runtime — Chainguard glibc-dynamic distroless
# -----------------------------------------------------------------
FROM cgr.dev/chainguard/glibc-dynamic:latest
COPY --from=rust-builder /build/netvis /usr/local/bin/netvis
EXPOSE 9760
ENTRYPOINT ["/usr/local/bin/netvis"]
