# syntax=docker/dockerfile:1.7
# SPDX-License-Identifier: Apache-2.0
#
# goat-loadgen container image.
#
# Two-stage build, identical shape to the synth-vendor + audit-subscriber
# images in this directory:
#   1. rust-builder: `cargo build --release` produces the binary.
#   2. Runtime: the runtime-cc base (ops/base-images/, goatcore ADR 1220) — single binary on a
#      minimal distroless base.
#
# The mTLS leaf cert/key/CA are mounted in at deploy time by the Ansible role;
# the image carries no embedded identity material.

# Base images are build arguments named for their role; the defaults are
# the public images in ops/base-images/ (goatcore docs/design/base-images.md).
ARG BASE_RUST_BUILD=cgr.dev/chainguard/rust:latest-dev@sha256:54120f0b57f28063bd9f23e29113a28826ae08d731bb0bff11d6a6fa53bf74cf
ARG BASE_RUNTIME_CC=cgr.dev/chainguard/glibc-dynamic:latest@sha256:671a621016fb33812136dea6182b1db957996d3c8c618187aaf86abd9b14af53

FROM ${BASE_RUST_BUILD} AS rust-builder
USER root
WORKDIR /build
COPY Cargo.toml Cargo.lock* ./
RUN mkdir -p src && echo "fn main() {}" > src/main.rs && cargo fetch && rm -rf src
COPY src/ ./src/
RUN cargo build --release

FROM ${BASE_RUNTIME_CC}
WORKDIR /
COPY --from=rust-builder /build/target/release/goat-loadgen /usr/local/bin/goat-loadgen
ENTRYPOINT ["/usr/local/bin/goat-loadgen"]
