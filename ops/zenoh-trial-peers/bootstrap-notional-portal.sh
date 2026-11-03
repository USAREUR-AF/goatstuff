#!/usr/bin/env bash
# SPDX-License-Identifier: Apache-2.0
#
# Idempotent portal-side topic registration for the notional vendor on
# the Zenoh trial fabric. Run AFTER the notional vendor
# has been minted (`bootstrap-portal-vendors.sh` variant or hand-curl
# against `POST /api/vendors` with prefix=notional) AND the synth-vendor
# containers have recreated against the renamed 22-entry compose
# (PR #225 + zenoh-synth-vendors-deploy.yml apply).
#
# This script does NOT mint the vendor cert — the cert tarball is
# fetched once at vendor-create time and distributed OOB. This script
# only registers the per-peer TOPIC catalog so the portal's topics + schema
# + vendor surfaces populate.
#
# Required env vars (sourced from .envrc.local.d/30-site-services.envrc):
#   PORTAL_URL                  — https://portal.nova.netbird.nova.example
#   PORTAL_OPERATOR_API_KEY     — operator super-user key (one per portal)
#
# What it registers (mirrors ops/ansible/group_vars/nova_hosts/synth-vendors.yml):
#   12 sensors/effectors + 7 data-services/C2 publishers + 3 pure sinks =
#   22 topics under prefix `notional/`, each pointing at one of three
#   personality schemas (tracks-demo, power-demo, beacon-demo).
#
# Idempotent via HTTP 409 swallowing — re-run after live changes; only
# new rows post.

set -euo pipefail

: "${PORTAL_URL:?set PORTAL_URL e.g. https://portal.nova.netbird.nova.example}"
: "${PORTAL_OPERATOR_API_KEY:?set PORTAL_OPERATOR_API_KEY}"

VENDOR="notional"

# Topic catalog — fields:
#   key_pattern | format | personality | role | description
#
# personality drives the schema_ref pointer (tracks-demo / power-demo /
# beacon-demo — all three live in ops/zenoh-trial-peers/synth-vendor/schemas/).
TOPICS=(
  # The fmt column carries the PORTAL topic.format value — not the wire
  # format. We register each topic as `protobuf:<MsgName>` so the portal's
  # schema viewer + per-language autogen-code surfaces light up against
  # the uploaded protobuf-FDS schema body (see sibling
  # synth-vendor-schemas/bootstrap-notional-schemas.sh). The synth-vendor
  # binary still emits JSON on the wire today; that mismatch only affects
  # the inspector's "decode samples" path, not the data-dictionary view.
  # A future expansion flips the wire format to protobuf to close the gap.
  #
  # Sensors
  "notional/tarsier/alpha-fob/contacts/v1|protobuf:notional.tracks_demo.v1.Entity|tracks-demo|sensor|Notional ground radar (alpha FOB) — entity contacts @ 1 Hz"
  "notional/tarsier/bravo-fob/contacts/v1|protobuf:notional.tracks_demo.v1.Entity|tracks-demo|sensor|Notional ground radar (bravo FOB) — entity contacts @ 1 Hz"
  "notional/mantis/alpha-fob/imagery/v1|protobuf:notional.tracks_demo.v1.Entity|tracks-demo|sensor|Notional EO/IR camera imagery feed (alpha FOB)"
  "notional/echidna/alpha-fob/detections/v1|protobuf:notional.tracks_demo.v1.Entity|tracks-demo|sensor|Notional acoustic detection feed (alpha FOB)"
  "notional/narwhal/alpha-fob/events/v1|protobuf:notional.power_demo.v1.PowerState|power-demo|sensor|Notional alarm/event stream (alpha FOB)"
  "notional/kingfisher/bravo-fob/emissions/v1|protobuf:notional.tracks_demo.v1.Entity|tracks-demo|sensor|Notional RF-emitter geolocation feed (bravo FOB)"
  "notional/axolotl/charlie-fob/composite/v1|protobuf:notional.tracks_demo.v1.Entity|tracks-demo|sensor|Notional sensor-fusion composite track (charlie FOB)"
  # Effectors
  "notional/mongoose/alpha-fob/status/v1|protobuf:notional.power_demo.v1.PowerState|power-demo|effector|Notional effector status telemetry (alpha FOB)"
  "notional/wolverine/alpha-fob/engagements/v1|protobuf:notional.power_demo.v1.PowerState|power-demo|effector|Notional effector engagement record (alpha FOB)"
  "notional/ibex/alpha-fob/launched/v1|protobuf:notional.power_demo.v1.PowerState|power-demo|effector|Notional munitions-launched events (alpha FOB)"
  "notional/aurochs/charlie-fob/launched/v1|protobuf:notional.power_demo.v1.PowerState|power-demo|effector|Notional munitions-launched events (charlie FOB)"
  "notional/basilisk/alpha-fob/deployed/v1|protobuf:notional.power_demo.v1.PowerState|power-demo|effector|Notional deployed-asset state (alpha FOB)"
  # Data services
  "notional/octopus/alpha-fob/tracks/v1|protobuf:notional.tracks_demo.v1.Entity|tracks-demo|data-service|Notional track-fusion broker output (alpha FOB)"
  "notional/cuttlefish/theater/confirmed/v1|protobuf:notional.tracks_demo.v1.Entity|tracks-demo|data-service|Notional theater-wide confirmed-track stream"
  "notional/chameleon/theater/normalized/v1|protobuf:notional.tracks_demo.v1.Entity|tracks-demo|data-service|Notional cross-format normalization output (theater)"
  # C2
  "notional/condor/theater/orders/v1|protobuf:notional.power_demo.v1.PowerState|power-demo|c2|Notional theater-level fire-control orders"
  "notional/peregrine/alpha-fob/orders/v1|protobuf:notional.power_demo.v1.PowerState|power-demo|c2|Notional FOB-level fire-control orders (alpha)"
  "notional/falcon/bravo-fob/orders/v1|protobuf:notional.power_demo.v1.PowerState|power-demo|c2|Notional FOB-level fire-control orders (bravo)"
  # Observer
  "notional/omniwatch/theater/heartbeat/v1|protobuf:notional.beacon_demo.v1.Hello|beacon-demo|observer|Notional theater observer — periodic heartbeat + wildcard subscribe to **"
  # Sinks (subscribe-only)
  "notional/glass-display/theater/unused/v1|protobuf:notional.beacon_demo.v1.Hello|beacon-demo|sink|Notional operator glass-display sink (subscribe-only; the published path is structural placeholder)"
  "notional/archive-vault/theater/unused/v1|protobuf:notional.beacon_demo.v1.Hello|beacon-demo|sink|Notional archive vault sink (subscribe-only)"
  "notional/audit-log/theater/unused/v1|protobuf:notional.beacon_demo.v1.Hello|beacon-demo|sink|Notional audit-log forwarder sink (subscribe-only)"
)

# api METHOD PATH [BODY]
# Returns body on stdout; HTTP_STATUS=<n> on stderr.
api() {
  local method="$1" path="$2" body="${3:-}"
  local tmp; tmp=$(mktemp)
  local code
  if [ -n "$body" ]; then
    code=$(curl -sk -o "$tmp" -w "%{http_code}" -X "$method" \
      -H "Authorization: Bearer ${PORTAL_OPERATOR_API_KEY}" \
      -H "Content-Type: application/json" \
      -d "$body" \
      "${PORTAL_URL}${path}")
  else
    code=$(curl -sk -o "$tmp" -w "%{http_code}" -X "$method" \
      -H "Authorization: Bearer ${PORTAL_OPERATOR_API_KEY}" \
      "${PORTAL_URL}${path}")
  fi
  cat "$tmp"; rm -f "$tmp"
  printf "HTTP_STATUS=%s\n" "$code" >&2
  case "$code" in 2*) return 0 ;; *) return 1 ;; esac
}

# Confirm vendor exists; refuse to proceed otherwise (avoid 22 wasted POSTs).
# Portal has no GET /api/vendors/<prefix> — list + grep instead.
echo "== pre-flight: notional vendor exists in portal vendor list =="
vendors_json=$(curl -sk -H "Authorization: Bearer ${PORTAL_OPERATOR_API_KEY}" "${PORTAL_URL}/api/vendors")
if ! echo "$vendors_json" | jq -e ".vendors[] | select(.prefix == \"${VENDOR}\")" >/dev/null 2>&1; then
  echo "FAIL: vendor '${VENDOR}' not registered in portal. Mint it first via"
  echo "  POST /api/vendors body: {prefix:notional, display_name:..., auto_approve:true, synthetic:true}"
  echo "  (One-time bundle_url for cert tarball is consumed at mint; capture both api_key and bundle_url before they're invalidated.)"
  exit 1
fi
echo "ok"

echo
echo "== fetching existing topics on portal =="
# Pre-fetch the existing topic registry so we can PATCH when a row already
# exists (e.g. an earlier run created the topic with format=json before the
# protobuf:<MsgName> shape was canonical). Saves operators a manual cleanup
# pass after re-running the script against a previously-seeded portal.
# Parallel arrays + a linear lookup instead of an associative array:
# operator-run on macOS /bin/bash 3.2 (#227). N is tens of topics.
EXISTING_TOPIC_KEYS=()
EXISTING_TOPIC_IDS=()
existing_topic_id_for() {
  # Search backwards: repeated keys keep the last row, like the old map.
  local i=${#EXISTING_TOPIC_KEYS[@]}
  while [ "$i" -gt 0 ]; do
    i=$((i - 1))
    if [ "${EXISTING_TOPIC_KEYS[$i]}" = "$1" ]; then
      printf '%s' "${EXISTING_TOPIC_IDS[$i]}"
      return 0
    fi
  done
  return 1
}
while IFS=$'\t' read -r id key; do
  EXISTING_TOPIC_KEYS+=("$key")
  EXISTING_TOPIC_IDS+=("$id")
done < <(curl -sk -H "Authorization: Bearer ${PORTAL_OPERATOR_API_KEY}" \
  "${PORTAL_URL}/api/vendors/${VENDOR}/topics" | jq -r '.topics[] | "\(.id)\t\(.key_pattern)"')
echo "  ${#EXISTING_TOPIC_KEYS[@]} topic(s) currently registered"

echo
echo "== upserting ${#TOPICS[@]} topics under vendor '${VENDOR}' =="
created=0; patched=0; unchanged=0; failed=0
for row in "${TOPICS[@]}"; do
  IFS='|' read -r key fmt personality _role desc <<<"${row}"
  schema_ref="attest/schema/${VENDOR}/${personality}/v1/v1"
  existing_id="$(existing_topic_id_for "$key" || true)"
  if [ -n "$existing_id" ]; then
    # Topic exists — PATCH the format + description + schema_ref. The
    # portal's PATCH /topics/{id} accepts any subset of mutable fields;
    # we send all three so a re-run reconciles to the manifest exactly.
    patch_body=$(jq -nc \
      --arg fmt "$fmt" \
      --arg ref "$schema_ref" \
      --arg desc "$desc" \
      '{format:$fmt, schema_ref:$ref, description:$desc}')
    if api PATCH "/api/vendors/${VENDOR}/topics/${existing_id}" "$patch_body" >/dev/null 2>/dev/null; then
      echo "  ~ ${key} (id=${existing_id}, format → ${fmt})"; patched=$((patched+1))
    else
      echo "  ! ${key} PATCH failed"; failed=$((failed+1))
    fi
  else
    body=$(jq -nc \
      --arg key "$key" \
      --arg fmt "$fmt" \
      --arg ref "$schema_ref" \
      --arg desc "$desc" \
      --argjson pts true \
      '{key_pattern:$key, format:$fmt, schema_ref:$ref, description:$desc, publish_to_storage:$pts}')
    if api POST "/api/vendors/${VENDOR}/topics" "$body" >/dev/null 2>/dev/null; then
      echo "  + ${key} (format=${fmt})"; created=$((created+1))
    else
      echo "  ! ${key} POST failed"; failed=$((failed+1))
    fi
  fi
done

echo
echo "summary: created=${created} patched=${patched} unchanged=${unchanged} failed=${failed}"
exit $failed
