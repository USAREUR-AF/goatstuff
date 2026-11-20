#!/bin/bash
# SPDX-License-Identifier: Apache-2.0
#
# Keep ops/goat-topology/static/design-system/ byte-identical to the canonical
# design system in docs/design-system/.
#
# The crate carries a copy so that it builds from its own directory alone
# (cargo install --git, cargo build in a copied directory). This script is the
# only way the copy should change.
#
#   tools/sync-design-system.sh          copy the canonical files over the copy
#   tools/sync-design-system.sh --check  exit 1 if the copy differs (CI)
set -eu
here="$(cd "$(dirname "$0")/.." && pwd)"
canon="$here/../../docs/design-system"
copy="$here/static/design-system"
files="colors_and_type.css components.css"
if [ ! -d "$canon" ]; then
  echo "no canonical design system at $canon (run this inside the goatstuff repository)" >&2
  exit 2
fi
bad=0
for f in $files; do
  if [ "${1:-}" = "--check" ]; then
    if ! cmp -s "$canon/$f" "$copy/$f"; then
      echo "differs: static/design-system/$f vs docs/design-system/$f; run ops/goat-topology/tools/sync-design-system.sh" >&2
      bad=1
    fi
  else
    cp "$canon/$f" "$copy/$f"
    echo "copied docs/design-system/$f -> static/design-system/$f"
  fi
done
exit "$bad"
