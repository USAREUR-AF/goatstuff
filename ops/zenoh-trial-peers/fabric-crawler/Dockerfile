# syntax=docker/dockerfile:1.7
# SPDX-License-Identifier: Apache-2.0
#
# goat-fabric-crawler container image
# (docs/design/zenoh-cardinality-budget-and-lazy-crawler.md §4).
#
# Same two-stage shape as ops/zenoh-trial-peers/time-health/Dockerfile:
#   1. rust-builder: cargo build --release --locked
#   2. Runtime: the runtime-cc base (ops/base-images/, goatcore ADR 1220).
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

# Base images are build arguments named for their role; the defaults are
# the public images in ops/base-images/ (goatcore docs/design/base-images.md).
ARG BASE_RUNTIME_CC=cgr.dev/chainguard/glibc-dynamic:latest@sha256:671a621016fb33812136dea6182b1db957996d3c8c618187aaf86abd9b14af53

# Builder is debian rust with clang installed, not the rust-build role:
# librocksdb-sys (the rocksdb-direct census source) runs bindgen at build
# time, which needs libclang, and Wolfi rust images (one overlay choice for
# rust-build) do not ship it. The SHIPPED runtime stage is the runtime-cc role.
FROM rust:1-bookworm AS rust-builder
RUN apt-get update && apt-get install -y --no-install-recommends clang libclang-dev && rm -rf /var/lib/apt/lists/*
WORKDIR /build
COPY Cargo.toml Cargo.lock* ./
RUN mkdir -p src && echo "fn main() {}" > src/main.rs && cargo fetch && rm -rf src
COPY src/ ./src/
COPY proto/ ./proto/
RUN cargo build --release --locked

FROM ${BASE_RUNTIME_CC}
WORKDIR /
COPY --from=rust-builder /build/target/release/goat-fabric-crawler /usr/local/bin/goat-fabric-crawler
ENTRYPOINT ["/usr/local/bin/goat-fabric-crawler"]
