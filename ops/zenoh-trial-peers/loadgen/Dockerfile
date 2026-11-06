# syntax=docker/dockerfile:1.7
# SPDX-License-Identifier: Apache-2.0
#
# goat-loadgen container image.
#
# Two-stage build, identical shape to the synth-vendor + audit-subscriber
# images in this directory:
#   1. rust-builder: `cargo build --release` produces the binary.
#   2. Runtime: the runtime-cc base (ops/base-images/, goatcore ADR 1220) — single binary on a
#      minimal distroless base.
#
# The mTLS leaf cert/key/CA are mounted in at deploy time by the Ansible role;
# the image carries no embedded identity material.

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
RUN cargo build --release

FROM ${BASE_RUNTIME_CC}
WORKDIR /
COPY --from=rust-builder /build/target/release/goat-loadgen /usr/local/bin/goat-loadgen
ENTRYPOINT ["/usr/local/bin/goat-loadgen"]
