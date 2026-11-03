#!/usr/bin/env bash
# build-fds.sh — compile every .proto in proto/ into a per-file
# FileDescriptorSet (.fds) inside out/. The portal accepts a .fds as
# its schema body — that's the protoc output that includes the full
# descriptor tree, suitable for dynamicpb-based decode and for the
# /api/schemas/render endpoint that walks the descriptor.
#
# Requires `protoc` on PATH (apt: protobuf-compiler / brew: protobuf).
#
# Usage:
#   ./build-fds.sh                    # compile all manifest entries
#   ./build-fds.sh proto/foo.proto    # compile a single file (override)
set -euo pipefail

cd "$(dirname "$0")"

if ! command -v protoc >/dev/null 2>&1; then
  echo "ERROR: protoc not found on PATH. Install protobuf-compiler / brew install protobuf." >&2
  exit 1
fi

mkdir -p out

# When called with explicit args, build those files; otherwise iterate
# the topic manifest.
if [ "$#" -gt 0 ]; then
  for src in "$@"; do
    base=$(basename "${src%.proto}")
    out="out/${base}.fds"
    protoc --descriptor_set_out="${out}" --include_imports --include_source_info \
      --proto_path=proto "${src#proto/}"
    echo "built ${out} ($(wc -c < "${out}" | tr -d ' ') bytes)"
  done
  exit 0
fi

# Manifest path: vendor<TAB>topic<TAB>proto<TAB>msg<TAB>wire_format
# We only need vendor/topic_key/proto_file in this loop; the other two
# columns are consumed by upload-all.sh.
while IFS=$'\t' read -r vendor topic_key proto_file _msg _fmt; do
  case "$vendor" in '#'*|'') continue ;; esac
  base=$(basename "${proto_file%.proto}")
  out="out/${base}.fds"
  protoc --descriptor_set_out="${out}" --include_imports --include_source_info \
    --proto_path=proto "$(basename "${proto_file}")"
  echo "built ${out} for ${topic_key} ($(wc -c < "${out}" | tr -d ' ') bytes)"
done < topic-manifest.tsv
