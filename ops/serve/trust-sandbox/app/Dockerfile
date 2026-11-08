# syntax=docker/dockerfile:1
#
# trust-sandbox web — Vite + Vue 3 SPA, the warm/tacticool playground front-end,
# served by nginx (which also reverse-proxies /api + /ws + /hydra → orchestrator
# and Hydra; see nginx.conf). Build context is ops/serve/trust-sandbox/.
#
# Design: docs/design/trust-sandbox.md
FROM node:20-bookworm-slim AS build
ENV CI=1
WORKDIR /app
# package-lock.json too, and `npm ci` not `npm install`: with only package.json
# copied, npm resolved the `^` ranges fresh at build time and the lockfile was
# never consulted -- so pinning a transitive there changed nothing in the image.
COPY app/package.json app/package-lock.json ./
RUN npm ci --no-audit --no-fund
COPY app/ ./
RUN npm run build \
 && test -f dist/index.html \
 && test -f dist/vision.html

FROM nginx:1.27-alpine
LABEL org.opencontainers.image.title="trust-sandbox-web" \
      org.opencontainers.image.description="Zero-trust identity playground front-end — docs/design/trust-sandbox.md"
COPY nginx.conf /etc/nginx/conf.d/default.conf
COPY --from=build /app/dist /usr/share/nginx/html
# TS_GOATNET (unset: "goatnet") names the goatnet the pages describe.
COPY app/goatnet-name.sh /docker-entrypoint.d/40-goatnet-name.sh
EXPOSE 80
HEALTHCHECK --interval=30s --timeout=5s --retries=3 \
  CMD wget -qO- http://127.0.0.1/ >/dev/null 2>&1 || exit 1
