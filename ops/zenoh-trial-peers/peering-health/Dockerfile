# syntax=docker/dockerfile:1.7
# SPDX-License-Identifier: Apache-2.0
#
# goat-peering-health container image
# (docs/operations/peering-denial-observability.md / ADR 1036).
#
# Same two-stage shape as ops/zenoh-trial-peers/node-net-health/Dockerfile:
#   1. rust-builder: cargo build --release --locked
#   2. Runtime: the runtime-cc base (ops/base-images/, goatcore ADR 1220).
#
# No embedded identity material — the leaf cert + key + CA root are bind-mounted
# at deploy time, as is the read-only peering-denial snapshot the host-side
# peering-denials-snapshot timer maintains (ops/peering-denials/pd-snapshot.py).
# A normal mTLS Zenoh client peer; publishes ops/peering/health/<edge>. The
# canonical wire schema lives at
# ops/zenoh-trial-peers/time-health/proto/env_health_v1.proto (hand-written
# prost structs here; no protoc/build.rs).

# Base images are build arguments named for their role; the defaults are
# the public images in ops/base-images/ (goatcore docs/design/base-images.md).
ARG BASE_RUST_BUILD=docker.io/library/rust:1.92-bookworm@sha256:e90e846de4124376164ddfbaab4b0774c7bdeef5e738866295e5a90a34a307a2
ARG BASE_RUNTIME_CC=gcr.io/distroless/cc-debian13:nonroot@sha256:08476c244888bfb425bae1ae4dd7c17f093e972aab8426883d97f3c6f280f7c1

FROM ${BASE_RUST_BUILD} AS rust-builder
USER root
WORKDIR /build
COPY Cargo.toml Cargo.lock* ./
RUN mkdir -p src && echo "fn main() {}" > src/main.rs && cargo fetch && rm -rf src
COPY src/ ./src/
RUN cargo build --release --locked

FROM ${BASE_RUNTIME_CC}
WORKDIR /
COPY --from=rust-builder /build/target/release/goat-peering-health /usr/local/bin/goat-peering-health
ENTRYPOINT ["/usr/local/bin/goat-peering-health"]
