#!/usr/bin/env bash
# upload-all.sh — reconcile every topic in topic-manifest.tsv against the
# live portal, uploading the matching .fds + setting topic format to
# `protobuf:<MsgName>` so the inspector decoded-JSON path lights up.
#
# Idempotent: skips uploads where the same (topic_id, v1) row already
# exists (409 Conflict). Upload bumps the topic's schema_ref pointer.
#
# Requires PORTAL_URL + PORTAL_API_KEY env vars. The api key must be
# the operator key (scoped-vendor keys can't update topics across
# multiple vendors in one run).
#
# Usage:
#   PORTAL_URL=https://portal.nova.netbird.nova.example \
#   PORTAL_API_KEY=$NOVA_PORTAL_OPERATOR_KEY \
#     ./upload-all.sh
#
# Add `--set-format` to also PATCH the topic format to
# `protobuf:<MsgName>`. WARNING: this changes the inspector's decode
# path. The synth-vendor publishes JSON/CBOR on the wire today, so
# flipping the format without flipping the producer breaks decode.
# Use only when you're ready to migrate the producer to protobuf
# wire format.
set -euo pipefail

cd "$(dirname "$0")"

: "${PORTAL_URL:?PORTAL_URL not set}"
: "${PORTAL_API_KEY:?PORTAL_API_KEY not set}"

SET_FORMAT=0
for arg in "$@"; do
  case "$arg" in
    --set-format) SET_FORMAT=1 ;;
    *) echo "unknown flag: $arg" >&2; exit 1 ;;
  esac
done

if [ ! -d out ]; then
  echo "out/ does not exist — run ./build-fds.sh first." >&2
  exit 1
fi

curl_auth() {
  curl -sS -H "Authorization: Bearer ${PORTAL_API_KEY}" "$@"
}

# Helper: fetch topic id for (vendor, key_pattern). Returns the id on
# stdout or empty string if the topic does not exist.
lookup_topic_id() {
  local vendor="$1" key="$2"
  curl_auth "${PORTAL_URL}/api/vendors/${vendor}/topics" |
    jq -r --arg k "$key" '.topics[] | select(.key_pattern==$k) | .id' |
    head -1
}

upload_one() {
  local vendor="$1" key="$2" proto_file="$3" msg_name="$4"
  local base fds
  base=$(basename "${proto_file%.proto}")
  fds="out/${base}.fds"
  if [ ! -f "$fds" ]; then
    echo "MISS  ${key}: ${fds} not built" >&2
    return 1
  fi
  local topic_id
  topic_id=$(lookup_topic_id "$vendor" "$key" || true)
  if [ -z "$topic_id" ]; then
    echo "MISS  ${key}: topic not registered on portal; register it first via the Topics page or POST /api/vendors/${vendor}/topics"
    return 1
  fi

  # Probe existing schemas for this topic.
  local existing
  existing=$(curl_auth "${PORTAL_URL}/api/vendors/${vendor}/topics/${topic_id}/schemas" |
    jq -r '.schemas[] | select(.version=="v1") | .sha256' | head -1)
  local local_sha
  local_sha=$(shasum -a 256 "$fds" | awk '{print $1}')
  if [ -n "$existing" ]; then
    if [ "$existing" = "$local_sha" ]; then
      echo "MATCH ${key}: v1 already uploaded with matching sha256"
    else
      echo "SKIP  ${key}: v1 exists with different content (remote=${existing:0:12}.. local=${local_sha:0:12}..); resolve manually (delete remote or bump local to v2)"
    fi
  else
    echo "UPLD  ${key}: posting v1 ($(wc -c < "$fds" | tr -d ' ') bytes)"
    local resp
    resp=$(curl_auth -X POST \
      -F "file=@${fds}" \
      -F "version=v1" \
      -F "compatibility=none" \
      -F "format=protobuf-fds" \
      "${PORTAL_URL}/api/vendors/${vendor}/topics/${topic_id}/schemas")
    local err
    err=$(echo "$resp" | jq -r '.error.message // empty')
    if [ -n "$err" ]; then
      echo "ERROR ${key}: $err"
      return 1
    fi
  fi

  if [ "$SET_FORMAT" = "1" ]; then
    echo "FMTSET ${key}: PATCH format → protobuf:${msg_name}"
    curl_auth -X PATCH \
      -H "Content-Type: application/json" \
      -d "{\"format\":\"protobuf:${msg_name}\"}" \
      "${PORTAL_URL}/api/vendors/${vendor}/topics/${topic_id}" >/dev/null
  fi
}

ok=0
fail=0
while IFS=$'\t' read -r vendor topic_key proto_file msg_name _fmt; do
  case "$vendor" in '#'*|'') continue ;; esac
  if upload_one "$vendor" "$topic_key" "$proto_file" "$msg_name"; then
    ok=$((ok+1))
  else
    fail=$((fail+1))
  fi
done < topic-manifest.tsv

echo
echo "summary: ${ok} ok, ${fail} failed"
if [ "$fail" -ne 0 ]; then
  exit 1
fi
