# syntax=docker/dockerfile:1.7
# SPDX-License-Identifier: Apache-2.0
#
# goat-mesh-health container image (ADR 1042/1036).
#
# Same two-stage shape as ops/zenoh-trial-peers/time-health/Dockerfile:
#   1. rust-builder: cargo build --release --locked
#   2. Runtime: Chainguard glibc-dynamic per ADR 0700.
#
# No embedded identity material — the leaf cert + key + CA root are bind-mounted
# at deploy time by the mesh-health Ansible role, as is the read-only netbird
# status snapshot the host-side mesh-health-snapshot timer maintains. A normal
# mTLS Zenoh client peer; publishes ops/mesh/health/<site>. The canonical wire
# schema lives at ops/zenoh-trial-peers/time-health/proto/env_health_v1.proto
# (hand-written prost structs here; no protoc/build.rs).

FROM cgr.dev/chainguard/rust:latest-dev AS rust-builder
USER root
WORKDIR /build
COPY Cargo.toml Cargo.lock* ./
RUN mkdir -p src && echo "fn main() {}" > src/main.rs && cargo fetch && rm -rf src
COPY src/ ./src/
RUN cargo build --release --locked

FROM cgr.dev/chainguard/glibc-dynamic:latest
WORKDIR /
COPY --from=rust-builder /build/target/release/goat-mesh-health /usr/local/bin/goat-mesh-health
ENTRYPOINT ["/usr/local/bin/goat-mesh-health"]
