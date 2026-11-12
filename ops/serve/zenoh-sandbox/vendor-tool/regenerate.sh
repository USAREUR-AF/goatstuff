#!/usr/bin/env bash
# SPDX-License-Identifier: Apache-2.0
#
# regenerate.sh: produce the vendored zenoh-ts browser module from the
# packages pinned in packages.lock.json.
#
#   regenerate.sh           write app/vendor/zenoh-ts/zenoh-ts.js and licenses/
#   regenerate.sh --check   write nothing; exit 1 if the committed files differ
#
# Steps: fetch.py downloads each pinned tarball and verifies its SHA-512;
# the Go program in this directory bundles the unpacked tree into one ES
# module. Needs python3, Go and network access to the npm registry and the Go
# module proxy. Needs no npm and no Node.
set -euo pipefail

HERE="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
DEST="$(cd "$HERE/.." && pwd)/app/vendor/zenoh-ts"
BUNDLE="zenoh-ts.js"

mode="write"
case "${1:-}" in
  "") ;;
  --check) mode="check" ;;
  *) echo "usage: regenerate.sh [--check]" >&2; exit 2 ;;
esac

command -v python3 >/dev/null || { echo "regenerate: python3 not found" >&2; exit 1; }
command -v go >/dev/null || { echo "regenerate: go not found" >&2; exit 1; }

work="$(mktemp -d)"
trap 'rm -rf "$work"' EXIT

python3 -I "$HERE/fetch.py" "$HERE/packages.lock.json" "$work/tree"
( cd "$HERE" && go run . "$work/tree" "$work/$BUNDLE" )

sha256() {
  if command -v sha256sum >/dev/null; then sha256sum "$1" | cut -d' ' -f1
  else shasum -a 256 "$1" | cut -d' ' -f1; fi
}

if [ "$mode" = "check" ]; then
  rc=0
  if ! cmp -s "$work/$BUNDLE" "$DEST/$BUNDLE"; then
    echo "regenerate: $DEST/$BUNDLE differs from the bundle built from packages.lock.json" >&2
    echo "  committed: $(sha256 "$DEST/$BUNDLE" 2>/dev/null || echo missing)" >&2
    echo "  rebuilt:   $(sha256 "$work/$BUNDLE")" >&2
    rc=1
  fi
  if ! diff -r "$work/tree/licenses" "$DEST/licenses" >&2; then
    echo "regenerate: $DEST/licenses differs from the license files in the pinned tarballs" >&2
    rc=1
  fi
  want="$(sha256 "$work/$BUNDLE")"
  if ! grep -q "$want" "$DEST/README.md"; then
    echo "regenerate: $DEST/README.md does not state the bundle sha256 $want" >&2
    rc=1
  fi
  [ "$rc" -eq 0 ] && echo "regenerate: committed bundle matches (sha256 $want)"
  exit "$rc"
fi

mkdir -p "$DEST"
rm -rf "$DEST/licenses"
cp -R "$work/tree/licenses" "$DEST/licenses"
cp "$work/$BUNDLE" "$DEST/$BUNDLE"
echo "regenerate: wrote $DEST/$BUNDLE"
echo "  sha256 $(sha256 "$DEST/$BUNDLE")  (record this in $DEST/README.md)"
