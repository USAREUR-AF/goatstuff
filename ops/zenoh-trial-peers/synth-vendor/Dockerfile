# syntax=docker/dockerfile:1.7
# SPDX-License-Identifier: Apache-2.0
#
# synth-vendor container image.
#
# Two-stage build:
#   1. rust-builder: `cargo build --release --locked` produces the binary.
#   2. Runtime: the runtime-cc base (ops/base-images/, goatcore ADR 1220) — single binary on a
#      minimal distroless base.
#
# Cert tarballs are mounted in at deploy time by the Ansible role; the
# image carries no embedded identity material.

# Base images are build arguments named for their role; the defaults are
# the public images in ops/base-images/ (goatcore docs/design/base-images.md).
ARG BASE_RUST_BUILD=cgr.dev/chainguard/rust:latest-dev@sha256:54120f0b57f28063bd9f23e29113a28826ae08d731bb0bff11d6a6fa53bf74cf
ARG BASE_RUNTIME_CC=cgr.dev/chainguard/glibc-dynamic:latest@sha256:671a621016fb33812136dea6182b1db957996d3c8c618187aaf86abd9b14af53

FROM ${BASE_RUST_BUILD} AS rust-builder
USER root
WORKDIR /build
COPY Cargo.toml Cargo.lock* build.rs ./
COPY proto-notional/ ./proto-notional/
RUN mkdir -p src && echo "fn main() {}" > src/main.rs && cargo fetch && rm -rf src
COPY src/ ./src/
RUN cargo build --release --locked

FROM ${BASE_RUNTIME_CC}
WORKDIR /
COPY --from=rust-builder /build/target/release/synth-vendor /usr/local/bin/synth-vendor
# Fleet roster for the local panoscope demo — one container drives the whole
# 22-peer notional ecosystem (SYNTH_VENDOR_ROSTER=/etc/synth-vendor/demo-roster.json).
# Single-peer mode (the nova-a Ansible role) ignores it and uses the CLI flags.
COPY demo-roster.json /etc/synth-vendor/demo-roster.json
ENTRYPOINT ["/usr/local/bin/synth-vendor"]
