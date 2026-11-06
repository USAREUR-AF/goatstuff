# syntax=docker/dockerfile:1.7
# SPDX-License-Identifier: Apache-2.0
#
# goat-mesh-health container image (ADR 1042/1036).
#
# Same two-stage shape as ops/zenoh-trial-peers/time-health/Dockerfile:
#   1. rust-builder: cargo build --release --locked
#   2. Runtime: the runtime-cc base (ops/base-images/, goatcore ADR 1220).
#
# No embedded identity material — the leaf cert + key + CA root are bind-mounted
# at deploy time by the mesh-health Ansible role, as is the read-only netbird
# status snapshot the host-side mesh-health-snapshot timer maintains. A normal
# mTLS Zenoh client peer; publishes ops/mesh/health/<site>. The canonical wire
# schema lives at ops/zenoh-trial-peers/time-health/proto/env_health_v1.proto
# (hand-written prost structs here; no protoc/build.rs).

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
COPY --from=rust-builder /build/target/release/goat-mesh-health /usr/local/bin/goat-mesh-health
ENTRYPOINT ["/usr/local/bin/goat-mesh-health"]
