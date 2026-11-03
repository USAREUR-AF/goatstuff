# SPDX-License-Identifier: Apache-2.0
#
# goat-map -- CoT -> GeoTrackV1 transform + the map that renders it.
#
# DEBT (deliberate, see README): this is a stock python base built on-box, not
# a Chainguard-first CI-built digest-pinned image per
# docs/design/os-supply-chain-security.md + ADR 0859. First-light shape only.
FROM python:3.12-slim-bookworm

ENV PYTHONUNBUFFERED=1 \
    PYTHONDONTWRITEBYTECODE=1 \
    GOATMAP_WEB_ROOT=/app/web

WORKDIR /app

COPY requirements.txt /app/requirements.txt
RUN pip install --no-cache-dir -r /app/requirements.txt

COPY goatmap /app/goatmap
COPY web /app/web
# The synthetic producer ships in the same image so it deploys, versions and
# rolls back with the consumer instead of living on someone's laptop.
COPY tools /app/tools

# Pre-compress the vendored front end. This page ships ~900 KB of MapLibre +
# pmtiles + CSS deliberately (no CDN, no egress on a mesh-only surface), which
# is invisible on a healthy path and fatal on a degraded one: on a 1228-byte-MTU
# 240 ms link the 793 KB script did not finish at all. aiohttp serves a `.gz`
# sibling when the client offers gzip, so this is a pure build-time win with no
# code change and no runtime CPU.
#
# .pmtiles is NOT compressed here — its tiles are already gzipped internally and
# recompressing would cost size and CPU for nothing.
RUN find /app/web -type f \( -name '*.js' -o -name '*.css' -o -name '*.json' \) \
      -exec gzip -9 -k {} \; \
 && echo "pre-compressed:" && find /app/web -name '*.gz' -exec ls -la {} \; | head -20

RUN useradd --system --uid 65532 --no-create-home goatmap
USER 65532

EXPOSE 8080
CMD ["python", "-m", "goatmap"]
