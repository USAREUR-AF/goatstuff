# syntax=docker/dockerfile:1.7
# SPDX-License-Identifier: Apache-2.0
#
# synth-vendor container image.
#
# Two-stage build:
#   1. rust-builder: `cargo build --release --locked` produces the binary.
#   2. Runtime: Chainguard glibc-dynamic per ADR 0700 — single binary on a
#      minimal distroless base.
#
# Cert tarballs are mounted in at deploy time by the Ansible role; the
# image carries no embedded identity material.

FROM cgr.dev/chainguard/rust:latest-dev AS rust-builder
USER root
WORKDIR /build
COPY Cargo.toml Cargo.lock* build.rs ./
COPY proto-notional/ ./proto-notional/
RUN mkdir -p src && echo "fn main() {}" > src/main.rs && cargo fetch && rm -rf src
COPY src/ ./src/
RUN cargo build --release --locked

FROM cgr.dev/chainguard/glibc-dynamic:latest
WORKDIR /
COPY --from=rust-builder /build/target/release/synth-vendor /usr/local/bin/synth-vendor
# Fleet roster for the local panoscope demo — one container drives the whole
# 22-peer notional ecosystem (SYNTH_VENDOR_ROSTER=/etc/synth-vendor/demo-roster.json).
# Single-peer mode (the nova-a Ansible role) ignores it and uses the CLI flags.
COPY demo-roster.json /etc/synth-vendor/demo-roster.json
ENTRYPOINT ["/usr/local/bin/synth-vendor"]
