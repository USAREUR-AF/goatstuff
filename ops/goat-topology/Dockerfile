# syntax=docker/dockerfile:1
#
# goat-topology — ADR 1160. One binary, three modes (publish | serve | inspect).
#
# BUILD CONTEXT IS THE REPO ROOT, not this directory. src/serve.rs
# `include_str!`s the design system from ../../../docs/design-system/, so the
# build tree has to mirror the repo for those paths to resolve — the same shape
# flashlight uses, and for the same reason: one source of truth for the CSS
# bytes rather than a vendored copy that drifts (docs/design/design-system.md
# §"How consumers receive the CSS bytes").
#
#   docker build -f ops/goat-topology/Dockerfile .        # from the repo root
#
# Layout under /build:
#   /build/ops/goat-topology/{Cargo.toml,Cargo.lock,src/,static/,proto/}
#   /build/docs/design-system/{colors_and_type,components}.css

# `rust:1-bookworm`, not a pinned minor: the dependency tree resolves crates
# whose manifests need a recent Cargo (rand_pcg 0.10 failed to parse on 1.83),
# and this crate has no reason to hold an old toolchain — it is the same base
# flashlight uses. `rust-version` in Cargo.toml still records the floor the
# SOURCE needs.
FROM rust:1-bookworm AS build
WORKDIR /build

# --- dependency layer ------------------------------------------------------
# Stub out every include_str! target so the dependency build resolves without
# the real sources. A source-only edit then reuses this layer.
COPY ops/goat-topology/Cargo.toml ops/goat-topology/Cargo.lock ./ops/goat-topology/
# Every include_str! target needs to exist for the dependency build. Keep this
# list in step with the FILES table in src/assets.rs — a missing stub fails the
# build loudly here rather than shipping an empty asset.
RUN mkdir -p ops/goat-topology/src ops/goat-topology/static/render docs/design-system \
    && echo 'fn main() {}' > ops/goat-topology/src/main.rs \
    && : > ops/goat-topology/static/index.html \
    && : > ops/goat-topology/static/app.css \
    && : > ops/goat-topology/static/app.js \
    && : > ops/goat-topology/static/lib.js \
    && for m in verdict graph findings feed routers planner; do \
         : > ops/goat-topology/static/render/$m.js; done \
    && : > docs/design-system/colors_and_type.css \
    && : > docs/design-system/components.css
RUN --mount=type=cache,target=/root/.cargo/registry,sharing=locked \
    --mount=type=cache,target=/root/.cargo/git,sharing=locked \
    --mount=type=cache,target=/build/ops/goat-topology/target,sharing=locked \
    cd ops/goat-topology && cargo build --release --locked

# --- real sources ----------------------------------------------------------
COPY docs/design-system/colors_and_type.css docs/design-system/components.css ./docs/design-system/
COPY ops/goat-topology/src/ ./ops/goat-topology/src/
COPY ops/goat-topology/static/ ./ops/goat-topology/static/
COPY ops/goat-topology/proto/ ./ops/goat-topology/proto/
RUN --mount=type=cache,target=/root/.cargo/registry,sharing=locked \
    --mount=type=cache,target=/root/.cargo/git,sharing=locked \
    --mount=type=cache,target=/build/ops/goat-topology/target,sharing=locked \
    cd ops/goat-topology \
    && touch src/main.rs \
    && cargo build --release --locked \
    && strip target/release/goat-topology \
    && cp target/release/goat-topology /goat-topology

FROM gcr.io/distroless/cc-debian13:nonroot@sha256:cc66d222f0a7ec996b78a99fe76a969bc80b539a5016ad866c14d1949bc6d0ab
COPY --from=build /goat-topology /usr/local/bin/goat-topology
USER nonroot
# No default subcommand on purpose: `publish` runs on router hosts and `serve`
# on the mgmt-host, and neither is a safe default for the other's host.
ENTRYPOINT ["/usr/local/bin/goat-topology"]
