# syntax=docker/dockerfile:1.7
# SPDX-License-Identifier: Apache-2.0
#
# goat-loadgen container image.
#
# Two-stage build, identical shape to the synth-vendor + audit-subscriber
# images in this directory:
#   1. rust-builder: `cargo build --release` produces the binary.
#   2. Runtime: Chainguard glibc-dynamic per ADR 0700 — single binary on a
#      minimal distroless base.
#
# The mTLS leaf cert/key/CA are mounted in at deploy time by the Ansible role;
# the image carries no embedded identity material.

FROM cgr.dev/chainguard/rust:latest-dev AS rust-builder
USER root
WORKDIR /build
COPY Cargo.toml Cargo.lock* ./
RUN mkdir -p src && echo "fn main() {}" > src/main.rs && cargo fetch && rm -rf src
COPY src/ ./src/
RUN cargo build --release

FROM cgr.dev/chainguard/glibc-dynamic:latest
WORKDIR /
COPY --from=rust-builder /build/target/release/goat-loadgen /usr/local/bin/goat-loadgen
ENTRYPOINT ["/usr/local/bin/goat-loadgen"]
