# syntax=docker/dockerfile:1.7
# SPDX-License-Identifier: Apache-2.0
#
# goat-surface-visit container image (ADR 1163).
#
# Same two-stage shape as ops/zenoh-trial-peers/fabric-health/Dockerfile:
#   1. rust-builder: cargo build --release --locked
#   2. Runtime: the runtime-cc base (ops/base-images/, goatcore ADR 1220).
#
# NO IDENTITY MATERIAL IS BAKED IN. The Zenoh leaf cert, its key, the CA root
# and the age recipients file are all bind-mounted at deploy time. The age
# recipients are PUBLIC keys only — this container encrypts visit records to the
# operator roster and holds nothing that can decrypt them.
#
# It also needs the door's request log, bind-mounted READ-ONLY. That is the
# whole input: it reads what oauth2-proxy already writes and never talks to the
# fenced application.

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
COPY --from=rust-builder /build/target/release/goat-surface-visit /usr/local/bin/goat-surface-visit
ENTRYPOINT ["/usr/local/bin/goat-surface-visit"]
