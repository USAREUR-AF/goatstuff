#!/usr/bin/env bash
# prep.sh — run ONCE while you still have internet. Fetches the one thing the
# offline kit needs that is not in the tree: the zenoh-bridge-remote-api
# binary for this machine -> ./bin/. The web app needs no preparation; it is
# served from ../app as committed.
# After this, serve.sh runs the whole sandbox with NO internet and NO Docker.
#
# Design: docs/design/zenoh-sandbox-ux.md ("Offline / field demo").
set -euo pipefail
HERE="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
ZTS_VERSION="1.9.0"   # tracks the fleet zenoh pin

echo "==> fetching the standalone remote-api router for this machine"
os="$(uname -s)"; arch="$(uname -m)"
case "$os/$arch" in
  Darwin/arm64)  asset="zenoh-ts-${ZTS_VERSION}-aarch64-apple-darwin-standalone.zip" ;;
  Darwin/x86_64) asset="zenoh-ts-${ZTS_VERSION}-x86_64-apple-darwin-standalone.zip" ;;
  Linux/x86_64)  asset="zenoh-ts-${ZTS_VERSION}-x86_64-unknown-linux-musl-standalone.zip" ;;
  Linux/aarch64) asset="zenoh-ts-${ZTS_VERSION}-aarch64-unknown-linux-musl-standalone.zip" ;;
  *) echo "unsupported platform $os/$arch — grab a zenoh-bridge-remote-api from"; \
     echo "  https://github.com/eclipse-zenoh/zenoh-ts/releases/tag/${ZTS_VERSION} and drop it in $HERE/bin/"; exit 1 ;;
esac
url="https://github.com/eclipse-zenoh/zenoh-ts/releases/download/${ZTS_VERSION}/${asset}"
mkdir -p "$HERE/bin"
tmp="$(mktemp -d)"; trap 'rm -rf "$tmp"' EXIT
echo "    downloading $asset"
curl -fsSL "$url" -o "$tmp/b.zip"
unzip -o "$tmp/b.zip" -d "$tmp" >/dev/null
cp "$tmp/zenoh-bridge-remote-api" "$HERE/bin/zenoh-bridge-remote-api"
chmod +x "$HERE/bin/zenoh-bridge-remote-api"
# macOS Gatekeeper: clear the download quarantine so it runs offline without a prompt.
[ "$os" = "Darwin" ] && xattr -dr com.apple.quarantine "$HERE/bin/zenoh-bridge-remote-api" 2>/dev/null || true
echo "    router -> $HERE/bin/zenoh-bridge-remote-api"

echo
echo "✅ offline kit ready. From now on (no internet needed): ./serve.sh"
