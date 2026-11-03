#!/usr/bin/env bash
# SPDX-License-Identifier: Apache-2.0
#
# bootstrap-notional-schemas.sh — build the 3 notional-personality .fds
# descriptor sets from .proto sources under proto-notional/, then upload
# each as the schema body for every notional topic that uses that
# personality (per notional-topic-manifest.tsv).
#
# Idempotent: the portal's POST /api/vendors/<v>/topics/<id>/schemas
# returns 200 OK on duplicate upload of the same content; it errors on
# different content for the same (topic_id, version). Re-runs after .proto
# edits update the body; re-runs without edits no-op.
#
# Requires:
#   - protoc on PATH (libprotoc 3.x+)
#   - PORTAL_URL + PORTAL_OPERATOR_API_KEY env (sourced from
#     .envrc.local.d/30-site-services.envrc)
#   - All 22 notional topics already registered in the portal
#     (see sibling script ../bootstrap-notional-portal.sh)
#
# Wire-format gotcha: the synth-vendor binary emits JSON, not protobuf,
# on the wire. The uploaded .fds is for the portal's data-dictionary +
# autogen-code surfaces ONLY. The topic.format field stays `json` so the
# inspector's decode-samples path does NOT try to protobuf-decode JSON
# bytes. A future expansion converts the wire format too.

set -euo pipefail

: "${PORTAL_URL:?set PORTAL_URL}"
: "${PORTAL_OPERATOR_API_KEY:?set PORTAL_OPERATOR_API_KEY}"

cd "$(dirname "$0")"

PROTO_DIR="proto-notional"
FDS_OUT="${FDS_OUT:-/tmp/notional-fds}"
mkdir -p "$FDS_OUT"

echo "== building .fds descriptor sets =="
for p in "${PROTO_DIR}"/notional_*.proto; do
  base="$(basename "$p" .proto)"
  out="${FDS_OUT}/${base}.fds"
  protoc --descriptor_set_out="$out" --include_imports "$p"
  echo "  + ${out} ($(wc -c < "$out" | tr -d ' ') bytes)"
done

echo
echo "== fetching topic IDs from portal =="
# Parallel arrays + a linear lookup instead of an associative array:
# operator-run on macOS /bin/bash 3.2 (#227). N is tens of topics.
TOPIC_KEYS=()
TOPIC_IDS=()
topic_id_for() {
  # Search backwards: repeated keys keep the last row, like the old map.
  local i=${#TOPIC_KEYS[@]}
  while [ "$i" -gt 0 ]; do
    i=$((i - 1))
    if [ "${TOPIC_KEYS[$i]}" = "$1" ]; then
      printf '%s' "${TOPIC_IDS[$i]}"
      return 0
    fi
  done
  return 1
}
while IFS=$'\t' read -r id key; do
  TOPIC_KEYS+=("$key")
  TOPIC_IDS+=("$id")
done < <(curl -sk -H "Authorization: Bearer ${PORTAL_OPERATOR_API_KEY}" \
  "${PORTAL_URL}/api/vendors/notional/topics" | \
  jq -r '.topics[] | "\(.id)\t\(.key_pattern)"')
echo "  ${#TOPIC_KEYS[@]} topics on portal"

echo
echo "== uploading schemas =="
uploaded=0; failed=0
while IFS=$'\t' read -r key proto_rel msgname _fmt; do
  case "$key" in '#'*|'') continue ;; esac
  topic_id="$(topic_id_for "$key" || true)"
  if [ -z "$topic_id" ]; then
    echo "  ! MISS $key — not registered in portal"
    failed=$((failed+1)); continue
  fi
  base="$(basename "$proto_rel" .proto)"
  fds="${FDS_OUT}/${base}.fds"
  resp_tmp="$(mktemp)"
  status=$(curl -sk -o "$resp_tmp" -w "%{http_code}" \
    -H "Authorization: Bearer ${PORTAL_OPERATOR_API_KEY}" \
    -F "file=@${fds}" \
    -F "version=v1" \
    -F "compatibility=none" \
    -F "format=protobuf-fds" \
    "${PORTAL_URL}/api/vendors/notional/topics/${topic_id}/schemas")
  case "$status" in
    2*) echo "  + $key (${msgname})"; uploaded=$((uploaded+1)) ;;
    *)  echo "  ! $key — status=$status $(cat "$resp_tmp")"; failed=$((failed+1)) ;;
  esac
  rm -f "$resp_tmp"
done < notional-topic-manifest.tsv

echo
echo "summary: uploaded=$uploaded failed=$failed"
exit $failed
