# syntax=docker/dockerfile:1.7
# SPDX-License-Identifier: Apache-2.0
#
# goat-audit-subscriber container image.
#
# Same two-stage shape as ops/zenoh-trial-peers/synth-vendor/Dockerfile:
#   1. rust-builder: cargo build --release --locked
#   2. Runtime: Chainguard glibc-dynamic per ADR 0700.
#
# No embedded identity material — leaf cert + key + CA root are bind-
# mounted at deploy time by the audit-subscriber Ansible role.

FROM cgr.dev/chainguard/rust:latest-dev AS rust-builder
USER root
WORKDIR /build
COPY Cargo.toml Cargo.lock* ./
RUN mkdir -p src && echo "fn main() {}" > src/main.rs && cargo fetch && rm -rf src
COPY src/ ./src/
RUN cargo build --release --locked

FROM cgr.dev/chainguard/glibc-dynamic:latest
WORKDIR /
COPY --from=rust-builder /build/target/release/goat-audit-subscriber /usr/local/bin/goat-audit-subscriber
EXPOSE 9303
ENTRYPOINT ["/usr/local/bin/goat-audit-subscriber"]
