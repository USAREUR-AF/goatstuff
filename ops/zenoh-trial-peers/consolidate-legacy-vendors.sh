#!/usr/bin/env bash
# SPDX-License-Identifier: Apache-2.0
#
# consolidate-legacy-vendors.sh — operator-fired one-shot cleanup of the
# pre-notional synth-vendor registry entries on the nova-a trial fabric.
#
# Context: the original 4-vendor seed (caracal, community, homemade-sensors,
# pangolin) became vestigial when PR #225 / #231 collapsed every notional
# publisher under a single `notional` vendor identity. The 7 topics those
# vendors owned have zero live producers — see the CLAUDE.md Block 82R
# canon line "Stale caracal / community / homemade-sensors / pangolin portal
# entries linger as empty per-vendor namespaces — operator-side deregistration
# is queued separately." This script IS that operator-side deregistration.
#
# What it does (idempotent):
#   1. For each legacy vendor, list its non-retired topics + retire them via
#      DELETE /api/vendors/{v}/topics/{id} (soft-delete; sets retired_at).
#   2. DELETE /api/vendors/{v} on each legacy vendor — refuses with 409 if any
#      live topics remain (defensive; the prior step should have retired them
#      all). Once the row is gone the schema FK CASCADE cleans up the now-
#      retired topic rows + their schemas + code-example rows.
#
# Required env:
#   PORTAL_OPERATOR_API_KEY   bearer token authorizing the DELETE endpoints
#
# Optional env:
#   PORTAL                    override target URL (default nova-a portal)
#   DRY_RUN=1                 print intended actions; make no calls
#
# Re-runnable: after the first successful run, every subsequent run is a
# no-op (each legacy vendor returns 404 from the API; we treat that as
# already-consolidated and continue).

set -euo pipefail

PORTAL="${PORTAL:-https://portal.nova.netbird.nova.example}"
LEGACY_VENDORS=(caracal community homemade-sensors pangolin)

if [ -z "${PORTAL_OPERATOR_API_KEY:-}" ]; then
  echo "error: PORTAL_OPERATOR_API_KEY env var required" >&2
  exit 1
fi

dry() {
  if [ "${DRY_RUN:-0}" = "1" ]; then
    echo "  [dry-run] $*"
    return 1
  fi
  return 0
}

curl_auth() {
  curl -sk \
    -H "Authorization: Bearer ${PORTAL_OPERATOR_API_KEY}" \
    -H "Content-Type: application/json" \
    -w "\n%{http_code}\n" \
    "$@"
}

retire_topic() {
  local vendor="$1" topic_id="$2" key_pattern="$3"
  printf "  - retire topic %s/%s (id=%s)\n" "$vendor" "$key_pattern" "$topic_id"
  if dry "DELETE ${PORTAL}/api/vendors/${vendor}/topics/${topic_id}"; then
    local out code
    out=$(curl_auth -X DELETE "${PORTAL}/api/vendors/${vendor}/topics/${topic_id}")
    code=$(echo "$out" | tail -n1)
    if [ "$code" != "204" ] && [ "$code" != "404" ]; then
      echo "      ERROR: status=$code body=$(echo "$out" | head -n-1)" >&2
      return 1
    fi
    echo "      ok ($code)"
  fi
}

delete_vendor() {
  local vendor="$1"
  printf "  - delete vendor %s\n" "$vendor"
  if dry "DELETE ${PORTAL}/api/vendors/${vendor}"; then
    local out code
    out=$(curl_auth -X DELETE "${PORTAL}/api/vendors/${vendor}")
    code=$(echo "$out" | tail -n1)
    case "$code" in
      204) echo "      ok (deleted)" ;;
      404) echo "      already gone (404; treating as success)" ;;
      409) echo "      ERROR: still has live topics — re-run to retire any new ones, then retry" >&2; return 1 ;;
      *)   echo "      ERROR: status=$code body=$(echo "$out" | head -n-1)" >&2; return 1 ;;
    esac
  fi
}

main() {
  echo "Consolidating legacy synthetic vendors → notional umbrella"
  echo "  portal: ${PORTAL}"
  echo "  legacy vendors: ${LEGACY_VENDORS[*]}"
  if [ "${DRY_RUN:-0}" = "1" ]; then echo "  DRY_RUN=1 — no calls will be made"; fi
  echo

  for vendor in "${LEGACY_VENDORS[@]}"; do
    echo "Vendor: ${vendor}"

    # Retire any non-retired topics first.
    topics_json=$(curl -sk -H "Authorization: Bearer ${PORTAL_OPERATOR_API_KEY}" \
                  "${PORTAL}/api/vendors/${vendor}/topics" 2>/dev/null || echo '{"topics":[]}')

    # 404 from a deleted vendor returns an error body — treat as already gone.
    if echo "$topics_json" | jq -e '.error' >/dev/null 2>&1; then
      echo "  vendor already absent (or unreachable); skipping topic-retire"
    else
      ids=$(echo "$topics_json" | jq -r '.topics[]? | "\(.id)\t\(.key_pattern)"')
      if [ -z "$ids" ]; then
        echo "  no live topics"
      else
        while IFS=$'\t' read -r id key; do
          [ -z "$id" ] && continue
          retire_topic "$vendor" "$id" "$key"
        done <<< "$ids"
      fi
    fi

    delete_vendor "$vendor"
    echo
  done

  echo "Done. Verify:"
  echo "  curl -sk -H \"Authorization: Bearer \$PORTAL_OPERATOR_API_KEY\" \\"
  echo "    ${PORTAL}/api/vendors | jq '.vendors[].prefix'"
  echo "  expected: notional, aardwolf (+ any real partner vendors)"
}

main "$@"
