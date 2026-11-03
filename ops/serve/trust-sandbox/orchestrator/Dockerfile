# syntax=docker/dockerfile:1
#
# trust-sandbox orchestrator — the privileged broker. Multi-stage: build the
# Rust binary, then assemble a slim runtime that ALSO carries the spire-agent
# binary (the orchestrator shells out to `spire-agent api fetch x509` to obtain
# real SVIDs — far more robust than wrestling the workload-API client crate).
#
# Build context is ops/serve/trust-sandbox/ (see docker-compose.yml).
# Design: docs/design/trust-sandbox.md
# Rust 1.90: some transitive deps (serde_spanned 1.1.x) require the edition2024
# Cargo feature, stabilized in 1.85.
FROM rust:1.90-bookworm AS build
WORKDIR /build
RUN apt-get update && apt-get install -y --no-install-recommends \
        protobuf-compiler cmake \
 && rm -rf /var/lib/apt/lists/*

# Dependency cache layer: compile the (large) zenoh dep tree against a dummy
# main so source edits don't trigger a full rebuild. Cargo.lock pins the tree.
COPY orchestrator/Cargo.toml orchestrator/Cargo.lock ./
RUN mkdir src && echo 'fn main() {}' > src/main.rs \
 && cargo build --release --locked || true

# Real build.
COPY orchestrator/src ./src
RUN touch src/main.rs && cargo build --release --locked \
 && cp target/release/trust-sandbox-orchestrator /usr/local/bin/trust-sandbox-orchestrator

# ---------------------------------------------------------------------------
FROM ghcr.io/spiffe/spire-agent:1.9.6 AS spire
FROM debian:bookworm-slim
LABEL org.opencontainers.image.title="trust-sandbox-orchestrator" \
      org.opencontainers.image.description="Privileged broker for the trust-sandbox zero-trust identity playground — docs/design/trust-sandbox.md"
RUN apt-get update && apt-get install -y --no-install-recommends ca-certificates \
 && rm -rf /var/lib/apt/lists/*
COPY --from=spire /opt/spire/bin/spire-agent /usr/local/bin/spire-agent
COPY --from=build /usr/local/bin/trust-sandbox-orchestrator /usr/local/bin/trust-sandbox-orchestrator
ENTRYPOINT ["/usr/local/bin/trust-sandbox-orchestrator"]
