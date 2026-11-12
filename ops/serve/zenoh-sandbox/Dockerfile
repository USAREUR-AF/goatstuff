# syntax=docker/dockerfile:1
#
# zenoh-sandbox web — goat's warm, explanatory, tacticool browser playground
# for Zenoh pub/sub: hand-written HTML, CSS and ES modules served by nginx as
# they are in the tree. There is no build step and no package manager.
#
# The Zenoh client, @eclipse-zenoh/zenoh-ts at the fleet zenoh version
# (1.9.0), is one vendored file, app/vendor/zenoh-ts/zenoh-ts.js, committed
# with its versions, hashes and licenses beside it; vendor-tool/regenerate.sh
# produces it and CI checks it.
#
# The app connects to /ws on whatever host serves it (same-origin), so there
# is no server URL to configure in the image. Build context is
# ops/serve/zenoh-sandbox/.
#
# Design: docs/design/zenoh-sandbox-ux.md (UX) + docs/design/zenoh-sandbox.md (substrate)
FROM nginx:1.27-alpine
LABEL org.opencontainers.image.title="zenoh-sandbox" \
      org.opencontainers.image.description="Warm, tacticool browser playground for Zenoh pub/sub — docs/design/zenoh-sandbox-ux.md"
COPY nginx.conf /etc/nginx/conf.d/default.conf
COPY app/*.html /usr/share/nginx/html/
COPY app/css/ /usr/share/nginx/html/css/
COPY app/js/ /usr/share/nginx/html/js/
COPY app/vendor/ /usr/share/nginx/html/vendor/
EXPOSE 80
HEALTHCHECK --interval=30s --timeout=5s --retries=3 \
  CMD wget -qO- http://127.0.0.1/ >/dev/null 2>&1 || exit 1
