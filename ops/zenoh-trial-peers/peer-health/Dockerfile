# syntax=docker/dockerfile:1.7
# SPDX-License-Identifier: Apache-2.0
#
# goat-peer-health container image (goatcore ADR 1232).
#
# Same two-stage shape as ops/zenoh-trial-peers/time-health/Dockerfile:
#   1. rust-builder: cargo build --release --locked
#   2. Runtime: the runtime-cc base (ops/base-images/, goatcore ADR 1220).
#
# No embedded identity material — the leaf cert + key + CA root are bind-mounted
# at deploy time. A normal mTLS Zenoh client peer; reads a router's log on
# standard input or from a bind-mounted file (never the docker socket) and
# publishes ops/peer/health/<site> and ops/peer/verdict/<zid>.

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
RUN cargo build --release --locked

FROM ${BASE_RUNTIME_CC}
WORKDIR /
COPY --from=rust-builder /build/target/release/goat-peer-health /usr/local/bin/goat-peer-health
ENTRYPOINT ["/usr/local/bin/goat-peer-health"]
