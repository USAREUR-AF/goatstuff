# dataflows — static slide deck explaining data-movement patterns
#
# Build context: repo root (set in docker-compose.yml).
# Run: docker compose -f ops/serve/dataflows/docker-compose.yml build
#
# Serves the "Data Flow Patterns" presentation on port 80.
# Traefik routes dataflows.nova.netbird.nova.example → this container.
#
# Sibling of ops/serve/brief/ — same nginx:alpine shell, no PNG diagrams
# or PDF, just the self-contained slide deck + one interactive iframe.

FROM nginx:alpine

RUN printf \
'server {\n\
  listen 80;\n\
  root /usr/share/nginx/html;\n\
  location / {\n\
    try_files $uri $uri/ /index.html;\n\
  }\n\
  default_type application/octet-stream;\n\
  types {\n\
    text/html                 html;\n\
    text/css                  css;\n\
    application/javascript    js;\n\
    image/svg+xml             svg;\n\
  }\n\
  add_header X-Content-Type-Options nosniff;\n\
  add_header X-Frame-Options SAMEORIGIN;\n\
}\n' \
    > /etc/nginx/conf.d/default.conf

WORKDIR /usr/share/nginx/html

COPY ops/serve/dataflows/index.html  ./
COPY ops/serve/dataflows/styles/     ./styles/
COPY ops/serve/dataflows/js/         ./js/
COPY ops/serve/dataflows/iframes/    ./iframes/

EXPOSE 80
