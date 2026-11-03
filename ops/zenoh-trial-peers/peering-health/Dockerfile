# syntax=docker/dockerfile:1.7
# SPDX-License-Identifier: Apache-2.0
#
# goat-peering-health container image
# (docs/operations/peering-denial-observability.md / ADR 1036).
#
# Same two-stage shape as ops/zenoh-trial-peers/node-net-health/Dockerfile:
#   1. rust-builder: cargo build --release --locked
#   2. Runtime: Chainguard glibc-dynamic per ADR 0700.
#
# No embedded identity material — the leaf cert + key + CA root are bind-mounted
# at deploy time, as is the read-only peering-denial snapshot the host-side
# peering-denials-snapshot timer maintains (ops/peering-denials/pd-snapshot.py).
# A normal mTLS Zenoh client peer; publishes ops/peering/health/<edge>. The
# canonical wire schema lives at
# ops/zenoh-trial-peers/time-health/proto/env_health_v1.proto (hand-written
# prost structs here; no protoc/build.rs).

FROM cgr.dev/chainguard/rust:latest-dev AS rust-builder
USER root
WORKDIR /build
COPY Cargo.toml Cargo.lock* ./
RUN mkdir -p src && echo "fn main() {}" > src/main.rs && cargo fetch && rm -rf src
COPY src/ ./src/
RUN cargo build --release --locked

FROM cgr.dev/chainguard/glibc-dynamic:latest
WORKDIR /
COPY --from=rust-builder /build/target/release/goat-peering-health /usr/local/bin/goat-peering-health
ENTRYPOINT ["/usr/local/bin/goat-peering-health"]
