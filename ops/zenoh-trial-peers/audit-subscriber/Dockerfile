# syntax=docker/dockerfile:1.7
# SPDX-License-Identifier: Apache-2.0
#
# goat-audit-subscriber container image.
#
# Same two-stage shape as ops/zenoh-trial-peers/synth-vendor/Dockerfile:
#   1. rust-builder: cargo build --release --locked
#   2. Runtime: the runtime-cc base (ops/base-images/, goatcore ADR 1220).
#
# No embedded identity material — leaf cert + key + CA root are bind-
# mounted at deploy time by the audit-subscriber Ansible role.

# Base images are build arguments named for their role; the defaults are
# the public images in ops/base-images/ (goatcore docs/design/base-images.md).
ARG BASE_RUST_BUILD=cgr.dev/chainguard/rust:latest-dev@sha256:bb029397076cc17b3a96b4d5183b4c362d8323a6d36394c71a21e8e5706664af
ARG BASE_RUNTIME_CC=cgr.dev/chainguard/glibc-dynamic:latest@sha256:671a621016fb33812136dea6182b1db957996d3c8c618187aaf86abd9b14af53

FROM ${BASE_RUST_BUILD} AS rust-builder
USER root
WORKDIR /build
COPY Cargo.toml Cargo.lock* ./
RUN mkdir -p src && echo "fn main() {}" > src/main.rs && cargo fetch && rm -rf src
COPY src/ ./src/
COPY fixtures/ ./fixtures/
RUN cargo build --release --locked

FROM ${BASE_RUNTIME_CC}
# A metadata-only writer: it needs --receiving-site and --state-dir. The
# goatcore audit-subscriber role reads this label and refuses the image unless
# audit_subscriber_v2_enabled is true (docs/operations/audit-replay-cutover.md).
LABEL io.goat.audit-subscriber.cutover="v2"
WORKDIR /
COPY --from=rust-builder /build/target/release/goat-audit-subscriber /usr/local/bin/goat-audit-subscriber
EXPOSE 9303
ENTRYPOINT ["/usr/local/bin/goat-audit-subscriber"]
