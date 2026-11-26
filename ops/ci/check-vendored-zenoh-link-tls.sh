#!/usr/bin/env bash
# SPDX-License-Identifier: Apache-2.0
# Finding F-614. Every copy of the patched zenoh-link-tls at the canonical
# copy's version must be byte-identical to vendor/zenoh-link-tls. No vendored zenoh-link-tls of any
# version may keep the TLS stream in an UnsafeCell shared by the read and
# write tasks.
# Usage: check-vendored-zenoh-link-tls.sh [repo root]
set -euo pipefail
root=$(cd "${1:-.}" && pwd)
canon="$root/vendor/zenoh-link-tls"
rc=0
if [ ! -f "$canon/Cargo.toml" ]; then
  echo "missing canonical copy: vendor/zenoh-link-tls"
  exit 1
fi
canon_version=$(grep -m1 -x 'version = "[^"]*"' "$canon/Cargo.toml")
while IFS= read -r toml; do
  dir="$root/$(dirname "$toml")"
  rel=$(dirname "$toml")
  if grep -q 'UnsafeCell<TlsStream' "$dir/src/unicast.rs"; then
    echo "FAIL $rel: TLS stream still in an UnsafeCell (apply the change described in vendor/zenoh-link-tls/README.md)"
    rc=1
  fi
  [ "$dir" = "$canon" ] && continue
  if grep -qxF "$canon_version" "$dir/Cargo.toml"; then
    if diff -rq "$canon" "$dir" >/dev/null; then
      echo "ok   $rel"
    else
      echo "FAIL $rel differs from vendor/zenoh-link-tls:"
      diff -rq "$canon" "$dir" | sed 's/^/  /' || true
      rc=1
    fi
  else
    echo "ok   $rel (another version: checked for the stream change only)"
  fi
done < <(git -C "$root" ls-files '*vendor/zenoh-link-tls/Cargo.toml' 'vendor/zenoh-link-tls/Cargo.toml' | sort -u)
exit $rc
