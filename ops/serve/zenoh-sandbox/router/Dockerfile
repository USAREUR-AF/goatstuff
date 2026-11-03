# syntax=docker/dockerfile:1
#
# zenoh-sandbox router — eclipse/zenoh + remote_api plugin baked in for local dev.
# Downloads the musl standalone zip from the matching zenoh-ts 1.9.0 release
# (same artifact + SHA256 the Ansible role uses), verifies the checksum, and
# extracts libzenoh_plugin_remote_api.so into the image. Supports amd64 + arm64
# (Windows/Linux Docker Desktop and Mac Apple Silicon).
#
# Design: docs/design/zenoh-sandbox.md §"Version pinning" (SHA256 source of truth)
FROM alpine:3.21 AS plugin-fetcher
ARG ZTS_VERSION=1.9.0
ARG TARGETARCH
RUN apk add --no-cache curl unzip && \
    case "$TARGETARCH" in \
      amd64) arch="x86_64"  ; sha256="c97c703ffeaa0dcc1692d10dcc91f57e276ec6ee593d56fc5b79135fd8553a67" ;; \
      arm64) arch="aarch64" ; sha256="87861d616043ccd6c6ac0d28a647b98a410e745b3866e77e32ce7805b9e1a744" ;; \
      *) echo "unsupported arch: $TARGETARCH" && exit 1 ;; \
    esac && \
    url="https://github.com/eclipse-zenoh/zenoh-ts/releases/download/${ZTS_VERSION}/zenoh-ts-${ZTS_VERSION}-${arch}-unknown-linux-musl-standalone.zip" && \
    curl -fsSL "$url" -o /tmp/b.zip && \
    echo "${sha256}  /tmp/b.zip" | sha256sum -c && \
    unzip -j /tmp/b.zip "libzenoh_plugin_remote_api.so" -d /plugins/

FROM eclipse/zenoh:1.9.0
COPY --from=plugin-fetcher /plugins/libzenoh_plugin_remote_api.so /opt/zenoh-sandbox/plugins/libzenoh_plugin_remote_api.so
COPY router/zenoh-sandbox-dev.json5 /etc/zenoh/zenoh-sandbox-dev.json5
CMD ["--config", "/etc/zenoh/zenoh-sandbox-dev.json5"]
