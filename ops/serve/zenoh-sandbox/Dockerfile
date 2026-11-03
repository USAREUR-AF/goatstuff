# syntax=docker/dockerfile:1
#
# zenoh-sandbox web — goat's warm, explanatory, tacticool browser playground
# for Zenoh pub/sub, built on @eclipse-zenoh/zenoh-ts and served by nginx.
# Source lives in-repo at ./app (Vite + Vue 3). This REPLACES the earlier
# "fetch the upstream Nuxt demo" build — we own the experience now.
#
# Design: docs/design/zenoh-sandbox-ux.md (UX) + docs/design/zenoh-sandbox.md (substrate)
#
# The app pins @eclipse-zenoh/zenoh-ts to the fleet zenoh version (=1.9.0) in
# app/package.json; the published npm package ships the prebuilt keyexpr WASM,
# so this image needs no Rust toolchain — just Node. esnext build target +
# vite-plugin-wasm + vite-plugin-top-level-await let the WASM glue compile.
# The app connects to /ws on whatever host serves it (same-origin), so there
# is no build-time server-URL to bake.

# ---------------------------------------------------------------------------
FROM node:20-bookworm-slim AS build
ENV CI=1
WORKDIR /app

# Install deps first for layer caching (no committed lockfile → npm install).
# package.json carries `overrides: {"@swc/core": "1.15.47"}`: the plugin
# vite-plugin-top-level-await 1.6.0 (July 2025, still latest) calls @swc/core's
# printSync in a shape that @swc/core 1.16.0 rejects with
# `missing field 'type'`, so every unpinned build  failed while
# the app itself had not changed . Drop the override when the
# plugin releases against swc 1.16.
COPY app/package.json ./
RUN npm install --no-audit --no-fund

# Build the static site.
COPY app/ ./
RUN npm run build \
 && test -f dist/index.html \
 && test -f dist/community.html

# ---------------------------------------------------------------------------
FROM nginx:1.27-alpine
LABEL org.opencontainers.image.title="zenoh-sandbox" \
      org.opencontainers.image.description="Warm, tacticool browser playground for Zenoh pub/sub — docs/design/zenoh-sandbox-ux.md"
COPY nginx.conf /etc/nginx/conf.d/default.conf
COPY --from=build /app/dist /usr/share/nginx/html
EXPOSE 80
HEALTHCHECK --interval=30s --timeout=5s --retries=3 \
  CMD wget -qO- http://127.0.0.1/ >/dev/null 2>&1 || exit 1
