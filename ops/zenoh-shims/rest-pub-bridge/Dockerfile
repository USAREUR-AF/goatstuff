# syntax=docker/dockerfile:1.7
# SPDX-License-Identifier: Apache-2.0
#
# rest-pub-bridge container image.
#
# Two-stage build:
#   1. rust-builder: `cargo build --release --locked` produces the binary.
#   2. Runtime: Chainguard glibc-dynamic per ADR 0700 — single binary on a
#      minimal distroless base.
#
# Vendor cert + key + CA root are mounted in at deploy time by the Ansible
# role; the image carries no embedded identity material.

FROM cgr.dev/chainguard/rust:latest-dev AS rust-builder
USER root
WORKDIR /build
COPY Cargo.toml Cargo.lock* ./
RUN mkdir -p src && echo "fn main() {}" > src/main.rs && cargo fetch && rm -rf src
COPY src/ ./src/
RUN cargo build --release --locked

FROM cgr.dev/chainguard/glibc-dynamic:latest
WORKDIR /
COPY --from=rust-builder /build/target/release/rest-pub-bridge /usr/local/bin/rest-pub-bridge
ENTRYPOINT ["/usr/local/bin/rest-pub-bridge"]
