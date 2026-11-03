# syntax=docker/dockerfile:1.7
# SPDX-License-Identifier: Apache-2.0
#
# goat-ntp-health-pub container image (ADR 1036/1037, F-184).
#
# Same two-stage shape as ops/zenoh-trial-peers/fabric-health/Dockerfile:
#   1. rust-builder: cargo build --release --locked
#   2. Runtime: Chainguard glibc-dynamic per ADR 0700.
#
# No embedded identity material — the leaf cert + key + CA root are bind-mounted
# at deploy time by the ntp-health-pub Ansible role. Runs with host networking
# so it can reach the ntp-health-agent on 127.0.0.1:9753.

FROM cgr.dev/chainguard/rust:latest-dev AS rust-builder
USER root
WORKDIR /build
COPY Cargo.toml Cargo.lock* ./
RUN mkdir -p src && echo "fn main() {}" > src/main.rs && cargo fetch && rm -rf src
COPY src/ ./src/
RUN cargo build --release --locked

FROM cgr.dev/chainguard/glibc-dynamic:latest
WORKDIR /
COPY --from=rust-builder /build/target/release/goat-ntp-health-pub /usr/local/bin/goat-ntp-health-pub
ENTRYPOINT ["/usr/local/bin/goat-ntp-health-pub"]
