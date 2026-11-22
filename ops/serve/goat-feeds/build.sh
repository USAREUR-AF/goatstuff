#!/bin/bash
# Build goat-feeds for every platform the goat fleet runs services on:
#   linux/amd64  x86 mini PCs, ZimaBoards, cloud hosts
#   linux/arm64  Jetson AGX Orin, Raspberry Pi 4/5, other arm64 boards
# Static (CGO off), the page embedded. Output: dist/goat-feeds-<version>-linux-<arch>
# and a SHA256SUMS file. Usage: ./build.sh [version]   (default: git describe, else dev)
set -euo pipefail
cd "$(dirname "$0")"
version="${1:-$(git describe --tags --always --dirty 2>/dev/null || echo dev)}"
mkdir -p dist
rm -f dist/goat-feeds-*-linux-* dist/SHA256SUMS
for arch in amd64 arm64; do
  out="dist/goat-feeds-${version}-linux-${arch}"
  CGO_ENABLED=0 GOOS=linux GOARCH="$arch" go build -trimpath -ldflags "-s -w -X main.version=${version}" -o "$out" .
  echo "built $out ($(wc -c < "$out" | tr -d ' ') bytes)"
done
(cd dist && shasum -a 256 goat-feeds-"${version}"-linux-* > SHA256SUMS)
