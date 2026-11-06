# syntax=docker/dockerfile:1.7
# SPDX-License-Identifier: Apache-2.0
#
# microscope container image (ADR 1090). Single-stage Rust build →
# distroless runtime-cc base (ops/base-images/, goatcore ADR 1220), mirroring
# crates/stethoscope/Dockerfile (incl. its single-RUN correctness note —
# no stub-warm two-RUN pattern).
#
# Build context is the repo root (render.rs include_str!s the
# design-system CSS):
#   docker build -f crates/microscope/Dockerfile -t microscope .

# Base images are build arguments named for their role; the defaults are
# the public images in ops/base-images/ (goatcore docs/design/base-images.md).
ARG BASE_RUST_BUILD=docker.io/library/rust:1.92-bookworm@sha256:e90e846de4124376164ddfbaab4b0774c7bdeef5e738866295e5a90a34a307a2
ARG BASE_RUNTIME_CC=gcr.io/distroless/cc-debian13:nonroot@sha256:08476c244888bfb425bae1ae4dd7c17f093e972aab8426883d97f3c6f280f7c1

# -----------------------------------------------------------------
# Stage 1: rust binary
# -----------------------------------------------------------------
FROM ${BASE_RUST_BUILD} AS rust-builder
USER root
WORKDIR /build

COPY crates/ ./crates/
COPY docs/design-system/colors_and_type.css ./docs/design-system/
COPY docs/design-system/components.css ./docs/design-system/
# netvis compiles the declared mesh vocabulary in (netvis.rs) so the page's
# prose cannot drift from what netbird-acl-bootstrap.yml acts on. Same
# include_str! pattern as the design-system CSS above: the file must be in
# context or the build fails at compile time — which is the intended
# behaviour, since a netvis with no taxonomy would silently lose the
# "what each group is for" column.
COPY ops/ansible/netbird_acl_taxonomy.yml ./ops/ansible/
COPY ops/ansible/netbird_acl_matrix.yml ./ops/ansible/

RUN --mount=type=cache,target=/usr/local/cargo/registry \
    --mount=type=cache,target=/usr/local/cargo/git \
    cd crates && cargo build --release -p microscope \
    && cp target/release/microscope /build/microscope \
    && ls -la /build/microscope

# -----------------------------------------------------------------
# Stage 2: runtime — the distroless runtime-cc base (ops/base-images/)
# -----------------------------------------------------------------
FROM ${BASE_RUNTIME_CC}
COPY --from=rust-builder /build/microscope /usr/local/bin/microscope
EXPOSE 9750
ENTRYPOINT ["/usr/local/bin/microscope"]
