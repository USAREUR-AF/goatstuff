# syntax=docker/dockerfile:1
#
# trust-sandbox web — the warm/tacticool playground front-end: hand-written
# HTML, CSS and ES modules served as they are in the tree (no build step, no
# package manager), by nginx, which also reverse-proxies /api + /ws + /hydra →
# orchestrator and Hydra (see nginx.conf). Build context is ops/serve/trust-sandbox/.
#
# Design: docs/design/trust-sandbox.md
FROM nginx:1.27-alpine
LABEL org.opencontainers.image.title="trust-sandbox-web" \
      org.opencontainers.image.description="Zero-trust identity playground front-end — docs/design/trust-sandbox.md"
COPY nginx.conf /etc/nginx/conf.d/default.conf
COPY app/*.html /usr/share/nginx/html/
COPY app/css/ /usr/share/nginx/html/css/
COPY app/js/ /usr/share/nginx/html/js/
# TS_GOATNET (unset: "goatnet") names the goatnet the pages describe.
COPY app/goatnet-name.sh /docker-entrypoint.d/40-goatnet-name.sh
EXPOSE 80
HEALTHCHECK --interval=30s --timeout=5s --retries=3 \
  CMD wget -qO- http://127.0.0.1/ >/dev/null 2>&1 || exit 1
