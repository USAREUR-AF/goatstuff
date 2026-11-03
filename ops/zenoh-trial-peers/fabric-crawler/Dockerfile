# syntax=docker/dockerfile:1.7
# SPDX-License-Identifier: Apache-2.0
#
# goat-fabric-crawler container image
# (docs/design/zenoh-cardinality-budget-and-lazy-crawler.md §4).
#
# Same two-stage shape as ops/zenoh-trial-peers/time-health/Dockerfile:
#   1. rust-builder: cargo build --release --locked
#   2. Runtime: Chainguard glibc-dynamic per ADR 0700.
#
# No embedded identity material — the leaf cert + key + CA root are bind-mounted
# at deploy time by the fabric-crawler Ansible role. A normal mTLS Zenoh client
# peer; crawls per-owner with the lazy >threshold cutoff and publishes
# ops/fabric/cardinality/<site> + ops/fabric/census/<site>.
#
# HEALTHCHECK: the runtime stage is distroless — no shell, no curl, no wget, and
# the crawler serves no HTTP. The binary probes itself instead; wire it from
# compose (left to the deployer so the interval/start_period are the operator's):
#   test: ["CMD", "/usr/local/bin/goat-fabric-crawler", "--healthcheck"]
# It reads the heartbeat the daemon writes each cycle (path resolved from the
# env — see the --healthcheck doc comment in src/main.rs) and reports LIVENESS
# only: a crawler censusing rocksdb with a dead zenoh session is healthy, since
# router state belongs on the data surface, not on container health.

# Builder is debian rust, NOT the chainguard rust:latest-dev the sibling
# services use: librocksdb-sys (the rocksdb-direct census source) runs bindgen
# at build time, which needs libclang — absent from the chainguard dev image
# (CI-verified). Precedent for a non-chainguard BUILDER stage when
# the C/C++ toolchain demands it: ops/zenoh/pqc-image/Dockerfile (rust:alpine).
# Chainguard-first (ADR 0700) governs the SHIPPED runtime stage, unchanged below.
FROM rust:1-bookworm AS rust-builder
RUN apt-get update && apt-get install -y --no-install-recommends clang libclang-dev && rm -rf /var/lib/apt/lists/*
WORKDIR /build
COPY Cargo.toml Cargo.lock* ./
RUN mkdir -p src && echo "fn main() {}" > src/main.rs && cargo fetch && rm -rf src
COPY src/ ./src/
COPY proto/ ./proto/
RUN cargo build --release --locked

FROM cgr.dev/chainguard/glibc-dynamic:latest
WORKDIR /
COPY --from=rust-builder /build/target/release/goat-fabric-crawler /usr/local/bin/goat-fabric-crawler
ENTRYPOINT ["/usr/local/bin/goat-fabric-crawler"]
