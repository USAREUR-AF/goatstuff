#!/usr/bin/env bash
# delete-ghost-topics.sh — remove the three "ghost" topics a prior
# session registered on the live portal whose key patterns do NOT
# match any actual synth-vendor publisher. These were registered as
# a side-effect of uploading schemas under invented names; the
# correct topics + schemas are now in topic-manifest.tsv.
#
# Ghost topics:
#   caracal/tracks/aircraft/v1       — synth publishes caracal/tracks-demo/entity/v1
#   community/weather/station/v1     — synth publishes community/beacon-demo/hello/v1
#   pangolin/inspections/asset/v1    — pangolin-observer is subscribe-only
#
# Idempotent: a 404 on lookup is treated as "already gone."
#
# Usage:
#   PORTAL_URL=https://portal.nova.netbird.nova.example \
#   PORTAL_API_KEY=$NOVA_PORTAL_OPERATOR_KEY \
#     ./delete-ghost-topics.sh

set -euo pipefail

: "${PORTAL_URL:?PORTAL_URL not set}"
: "${PORTAL_API_KEY:?PORTAL_API_KEY not set}"

ghosts=(
  "caracal	caracal/tracks/aircraft/v1"
  "community	community/weather/station/v1"
  "pangolin	pangolin/inspections/asset/v1"
)

curl_auth() {
  curl -sS -H "Authorization: Bearer ${PORTAL_API_KEY}" "$@"
}

for row in "${ghosts[@]}"; do
  vendor=${row%%	*}
  key=${row##*	}
  topic_id=$(curl_auth "${PORTAL_URL}/api/vendors/${vendor}/topics" |
    jq -r --arg k "$key" '.topics[] | select(.key_pattern==$k) | .id' | head -1)
  if [ -z "$topic_id" ]; then
    echo "GONE  ${key}: not present"
    continue
  fi
  echo "DEL   ${key} (id=${topic_id})"
  status=$(curl_auth -X DELETE -o /dev/null -w '%{http_code}' "${PORTAL_URL}/api/vendors/${vendor}/topics/${topic_id}")
  case "$status" in
    20*) echo "      ok (${status})" ;;
    404) echo "      already gone (${status})" ;;
    *)   echo "      ERROR (${status})"; exit 1 ;;
  esac
done
