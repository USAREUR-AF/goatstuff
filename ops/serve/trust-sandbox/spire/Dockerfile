# syntax=docker/dockerfile:1
#
# trust-sandbox SPIRE image — alpine + the static spire-server / spire-agent
# binaries lifted from the official (distroless, shell-less) images. We need a
# shell for the bootstrap dance (token-gen, registration, trust-bundle concat),
# so we run the static Go binaries on a minimal alpine base. Used for both the
# SPIRE server and agent roles in docker-compose.yml.
#
# Design: docs/design/trust-sandbox.md §SPIRE
FROM ghcr.io/spiffe/spire-server:1.9.6 AS server
FROM ghcr.io/spiffe/spire-agent:1.9.6 AS agent

FROM alpine:3.21
RUN apk add --no-cache ca-certificates
COPY --from=server /opt/spire/bin/spire-server /usr/local/bin/spire-server
COPY --from=agent  /opt/spire/bin/spire-agent  /usr/local/bin/spire-agent
# Sanity: both binaries run on this base.
RUN spire-server --version && spire-agent --version
