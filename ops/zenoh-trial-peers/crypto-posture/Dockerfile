# syntax=docker/dockerfile:1.7
# SPDX-License-Identifier: Apache-2.0
#
# goat-crypto-posture container image (ADR 1079).
#
# Same two-stage shape as the sibling fabric-health image. The builder needs
# the aws-lc-rs (non-FIPS) C toolchain; the stage installs cmake and clang
# when the rust-build base lacks them. No embedded identity material: the mTLS
# leaf + key + CA root are bind-mounted at deploy time by the crypto-posture
# Ansible role.

# Base images are build arguments named for their role; the defaults are
# the public images in ops/base-images/ (goatcore docs/design/base-images.md).
ARG BASE_RUST_BUILD=docker.io/library/rust:1.92-bookworm@sha256:e90e846de4124376164ddfbaab4b0774c7bdeef5e738866295e5a90a34a307a2
ARG BASE_RUNTIME_CC=gcr.io/distroless/cc-debian13:nonroot@sha256:08476c244888bfb425bae1ae4dd7c17f093e972aab8426883d97f3c6f280f7c1

FROM ${BASE_RUST_BUILD} AS rust-builder
USER root
RUN if ! command -v cmake >/dev/null 2>&1 && command -v apt-get >/dev/null 2>&1; then \
      apt-get update && apt-get install -y --no-install-recommends cmake clang \
      && rm -rf /var/lib/apt/lists/*; \
    fi
WORKDIR /build
COPY Cargo.toml Cargo.lock* ./
RUN mkdir -p src && echo "fn main() {}" > src/main.rs && cargo fetch && rm -rf src
COPY src/ ./src/
RUN cargo build --release --locked

FROM ${BASE_RUNTIME_CC}
WORKDIR /
COPY --from=rust-builder /build/target/release/goat-crypto-posture /usr/local/bin/goat-crypto-posture
ENTRYPOINT ["/usr/local/bin/goat-crypto-posture"]
