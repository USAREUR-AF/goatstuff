# syntax=docker/dockerfile:1.7
# SPDX-License-Identifier: Apache-2.0
#
# goat-trashgoat container image — operator-controlled chaos publisher
# per docs/design/trashgoat.md (Block 92).
#
# Two-stage build, mirroring synth-vendor + audit-subscriber:
#   1. rust-builder: `cargo build --release --locked` produces the binary.
#   2. Runtime: Chainguard glibc-dynamic per ADR 0700 — single binary on a
#      minimal distroless base.
#
# Cert tarball is mounted in at deploy time by the Ansible role; the
# image carries no embedded identity material.

FROM cgr.dev/chainguard/rust:latest-dev AS rust-builder
USER root
WORKDIR /build
COPY Cargo.toml Cargo.lock* ./
RUN mkdir -p src && echo "fn main() {}" > src/main.rs && cargo fetch && rm -rf src
COPY src/ ./src/
RUN cargo build --release --locked

FROM cgr.dev/chainguard/glibc-dynamic:latest
WORKDIR /
COPY --from=rust-builder /build/target/release/goat-trashgoat /usr/local/bin/goat-trashgoat
ENTRYPOINT ["/usr/local/bin/goat-trashgoat"]
