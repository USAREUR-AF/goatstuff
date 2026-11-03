# syntax=docker/dockerfile:1.7
# SPDX-License-Identifier: Apache-2.0
#
# goat-surface-visit container image (ADR 1163).
#
# Same two-stage shape as ops/zenoh-trial-peers/fabric-health/Dockerfile:
#   1. rust-builder: cargo build --release --locked
#   2. Runtime: Chainguard glibc-dynamic per ADR 0700.
#
# NO IDENTITY MATERIAL IS BAKED IN. The Zenoh leaf cert, its key, the CA root
# and the age recipients file are all bind-mounted at deploy time. The age
# recipients are PUBLIC keys only — this container encrypts visit records to the
# operator roster and holds nothing that can decrypt them.
#
# It also needs the door's request log, bind-mounted READ-ONLY. That is the
# whole input: it reads what oauth2-proxy already writes and never talks to the
# fenced application.

FROM cgr.dev/chainguard/rust:latest-dev AS rust-builder
USER root
WORKDIR /build
COPY Cargo.toml Cargo.lock* ./
RUN mkdir -p src && echo "fn main() {}" > src/main.rs && cargo fetch && rm -rf src
COPY src/ ./src/
RUN cargo build --release --locked

FROM cgr.dev/chainguard/glibc-dynamic:latest
WORKDIR /
COPY --from=rust-builder /build/target/release/goat-surface-visit /usr/local/bin/goat-surface-visit
ENTRYPOINT ["/usr/local/bin/goat-surface-visit"]
