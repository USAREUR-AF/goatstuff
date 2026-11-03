# syntax=docker/dockerfile:1.7
# SPDX-License-Identifier: Apache-2.0
#
# goat-crypto-posture container image (ADR 1079).
#
# Same two-stage shape as the sibling fabric-health image. The builder needs
# the aws-lc-rs (non-FIPS) C toolchain — cmake/clang are present in the
# Chainguard rust:latest-dev image. No embedded identity material: the mTLS
# leaf + key + CA root are bind-mounted at deploy time by the crypto-posture
# Ansible role.

FROM cgr.dev/chainguard/rust:latest-dev AS rust-builder
USER root
WORKDIR /build
COPY Cargo.toml Cargo.lock* ./
RUN mkdir -p src && echo "fn main() {}" > src/main.rs && cargo fetch && rm -rf src
COPY src/ ./src/
RUN cargo build --release --locked

FROM cgr.dev/chainguard/glibc-dynamic:latest
WORKDIR /
COPY --from=rust-builder /build/target/release/goat-crypto-posture /usr/local/bin/goat-crypto-posture
ENTRYPOINT ["/usr/local/bin/goat-crypto-posture"]
