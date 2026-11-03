# syntax=docker/dockerfile:1.7
# SPDX-License-Identifier: Apache-2.0
#
# goat-time-health container image (ADR 1036/1037, F-184).
#
# Same two-stage shape as ops/zenoh-trial-peers/fabric-health/Dockerfile:
#   1. rust-builder: cargo build --release --locked
#   2. Runtime: Chainguard glibc-dynamic per ADR 0700.
#
# No embedded identity material — the leaf cert + key + CA root are bind-mounted
# at deploy time by the time-health Ansible role. A normal mTLS Zenoh client
# peer; subscribes ops/time/attest/** and publishes ops/time/health/<site>.

FROM cgr.dev/chainguard/rust:latest-dev AS rust-builder
USER root
WORKDIR /build
COPY Cargo.toml Cargo.lock* ./
RUN mkdir -p src && echo "fn main() {}" > src/main.rs && cargo fetch && rm -rf src
COPY src/ ./src/
COPY proto/ ./proto/
RUN cargo build --release --locked

FROM cgr.dev/chainguard/glibc-dynamic:latest
WORKDIR /
COPY --from=rust-builder /build/target/release/goat-time-health /usr/local/bin/goat-time-health
ENTRYPOINT ["/usr/local/bin/goat-time-health"]
