#!/usr/bin/env bash
# SPDX-License-Identifier: Apache-2.0
#
# One-shot bootstrap for the synth-vendor publishers on the
# Zenoh trial fabric. Registers two vendors (`caracal`, `community`) via
# the portal API, downloads the per-vendor cert tarballs, and declares
# the three demo topics that the synth-vendor systemd units publish.
#
# Idempotent: skips vendor creation if the prefix is already registered
# (HTTP 409). Topic creation is also retried-safe via the portal's
# upsert-on-conflict path (creates a new topic row but topics aren't
# rate-limited or de-duped — operator can re-run; duplicates surface in
# the portal UI for manual cleanup, design-doc tracking under Block 82Q
# follow-ons).
#
# Required env vars:
#   PORTAL_URL                  — e.g. https://portal.nova.netbird.nova.example
#   PORTAL_OPERATOR_API_KEY     — operator super-user key for the portal.
#                                 Sourced from .envrc.local.d/30-site-services.envrc
#                                 on operator laptops (direnv-loaded). The
#                                 canonical source-of-truth is the portal
#                                 container's compose env on the live host
#                                 (/opt/goat-zenoh-portal/.env ) plus
#                                 1Password as the backup channel — fish from
#                                 there if direnv hasn't been populated yet.
#   SYNTH_VENDOR_CERT_OUT_DIR   — local dir to drop cert tarballs into
#                                 (default: ./synth-vendor-certs)
#   SCP_TARGET                  — `user@host:/path` to SCP the unpacked
#                                 cert tree to. Set empty to skip SCP.
#                                 (default: operator@<mesh-ip>:/opt/synth-vendors/certs)
#
# Usage:
#   export PORTAL_URL=...
#   export PORTAL_OPERATOR_API_KEY=...
#   ./bootstrap-portal-vendors.sh

set -euo pipefail

: "${PORTAL_URL:?set PORTAL_URL e.g. https://portal.nova.netbird.nova.example}"
: "${PORTAL_OPERATOR_API_KEY:?set PORTAL_OPERATOR_API_KEY (matches portal compose env)}"
PORTAL_URL="${PORTAL_URL%/}"
CERT_OUT_DIR="${SYNTH_VENDOR_CERT_OUT_DIR:-./synth-vendor-certs}"
SCP_TARGET="${SCP_TARGET:-goat@100.127.0.190:/opt/synth-vendors/certs}"

# Vendor list — must match ops/ansible/roles/zenoh-synth-vendors/defaults/main.yml.
# All four are operator-managed demo vendors; the portal marks them
# `synthetic: true` so the UI badges them and panoscope can filter via
# its "Hide synthetic" toggle.
VENDORS=( "caracal" "community" "homemade-sensors" "pangolin" )

# Topic list — (vendor, key_pattern, format, schema_ref, description).
# schema_ref points at the JSON schema in this repo so the portal UI
# surfaces it; future /api/schemas endpoint will serve the actual JSON.
TOPICS=(
  "caracal|caracal/tracks-demo/entity/v1|json|ops/zenoh-trial-peers/synth-vendor/schemas/tracks-demo-v1.json|Synthetic CoT-shaped entity track @ 1 Hz from synth-vendor tracks-demo"
  "caracal|caracal/power-demo/site-nova/v1|json|ops/zenoh-trial-peers/synth-vendor/schemas/power-demo-v1.json|Synthetic per-site power telemetry @ 0.1 Hz from synth-vendor power-demo (also subscribes to community/**)"
  "community|community/beacon-demo/hello/v1|json|ops/zenoh-trial-peers/synth-vendor/schemas/beacon-demo-v1.json|Synthetic low-rate beacon + cross-vendor subscriber to caracal/** from synth-vendor beacon-demo"
  "homemade-sensors|homemade-sensors/garage/temperature/v1|json|ops/zenoh-trial-peers/synth-vendor/schemas/garage-temp-v1.json|Synthetic DHT22-shaped garage thermometer @ 1/30 Hz from synth-vendor garage-temp"
  "homemade-sensors|homemade-sensors/garage/door-state/v1|json|ops/zenoh-trial-peers/synth-vendor/schemas/garage-door-v1.json|Synthetic reed-switch garage door state @ 0.1 Hz from synth-vendor garage-door (subscribes to the sibling temperature topic for intra-vendor correlation)"
  "homemade-sensors|homemade-sensors/yard/soil-moisture/v1|cbor|ops/zenoh-trial-peers/synth-vendor/schemas/yard-soil-v1.json|Synthetic capacitive soil-moisture probe @ 1/300 Hz, CBOR-encoded — exercises the inspector's CBOR decoder"
  "pangolin|pangolin/observer/heartbeat/v1|json|ops/zenoh-trial-peers/synth-vendor/schemas/observer-v1.json|Pangolin = synthetic subscriber-only vendor that listens on ** and emits a low-rate heartbeat summarising what it has seen"
)

# api METHOD PATH [BODY]
#
# Writes response body to stdout. Writes HTTP status code (digits only)
# to stderr as "HTTP_STATUS=<n>" on the LAST line of stderr so the caller
# can branch on it. Returns 0 for any 2xx; returns 1 otherwise — but
# stdout always carries the server's body so callers can surface the
# actual error rather than guessing "409." Critical: a 400 + 500 are
# both "not 2xx," and treating both as 409 (as the original loop did)
# masked the pangolin / unknown-field decode failure during the
# first deploy.
api() {
  local method="$1" path="$2" body="${3:-}"
  local tmpbody tmpstatus
  tmpbody=$(mktemp)
  # -w "%{http_code}" prints the code to stdout AFTER the body; -s
  # silences progress. We deliberately do NOT pass -f: that conflates
  # error bodies with curl's own failure exit, hiding the server's
  # actual error message. Capture the body + status separately and let
  # the caller branch.
  if [ -n "$body" ]; then
    tmpstatus=$(curl -s -o "$tmpbody" -w "%{http_code}" -X "$method" \
      -H "Authorization: Bearer ${PORTAL_OPERATOR_API_KEY}" \
      -H "Content-Type: application/json" \
      -d "$body" \
      "${PORTAL_URL}${path}")
  else
    tmpstatus=$(curl -s -o "$tmpbody" -w "%{http_code}" -X "$method" \
      -H "Authorization: Bearer ${PORTAL_OPERATOR_API_KEY}" \
      "${PORTAL_URL}${path}")
  fi
  cat "$tmpbody"
  rm -f "$tmpbody"
  printf "HTTP_STATUS=%s\n" "$tmpstatus" >&2
  case "$tmpstatus" in
    2*) return 0 ;;
    *)  return 1 ;;
  esac
}

# Capture the last "HTTP_STATUS=..." line from stderr emitted by api().
# Usage: status=$(extract_status "$err"); echo "$err" prints body unchanged.
extract_status() {
  printf '%s\n' "$1" | awk -F= '/^HTTP_STATUS=/{s=$2} END{print s}'
}

mkdir -p "$CERT_OUT_DIR"

echo "== creating vendors =="
# Pairwise parallel arrays instead of associative arrays: operator-run
# on macOS /bin/bash 3.2 (#227). One row per vendor minted THIS run;
# iteration is by index.
NEW_VENDOR_NAMES=()
NEW_VENDOR_BUNDLE_URLS=()
for v in "${VENDORS[@]}"; do
  echo "-- $v"
  body=$(printf '{"prefix":"%s","display_name":"%s (synth-vendor demo)","auto_approve":true,"synthetic":true}' "$v" "$v")
  # Capture stdout (body) + stderr (HTTP_STATUS) separately. Use temp
  # file for stderr so we don't conflate the two streams.
  err_tmp=$(mktemp)
  if resp=$(api POST "/api/vendors" "$body" 2>"$err_tmp"); then
    bundle_url=$(echo "$resp" | jq -r '.bundle_url')
    NEW_VENDOR_NAMES+=("$v")
    NEW_VENDOR_BUNDLE_URLS+=("$bundle_url")
    echo "   created (one-time bundle_url captured)"
  else
    status=$(extract_status "$(<"$err_tmp")")
    case "$status" in
      409)
        # Already registered. Bundle token was one-shot at first mint,
        # so we can't re-fetch the cert tarball — assume the operator
        # already has the cert tree at SCP_TARGET. Topic re-registration
        # below is still safe.
        echo "   already registered (HTTP 409); skipping cert mint — topics will still be (re)declared"
        ;;
      *)
        # ANY other failure surfaces the real status + body, so a 400
        # decode error (e.g. portal binary doesn't know a new field) or
        # a 500 doesn't get silently mislabeled. Operator decides
        # whether to abort or proceed.
        echo "   FAILED (HTTP ${status:-?}): $resp"
        echo "   NOTE: leaving the vendor uncreated; topics under <${v}/**> won't be ACL-allowed; downstream synth-vendor containers for this vendor will fail."
        ;;
    esac
  fi
  rm -f "$err_tmp"
done

echo
echo "== downloading + unpacking cert tarballs =="
vi=0
while [ "$vi" -lt "${#NEW_VENDOR_NAMES[@]}" ]; do
  v="${NEW_VENDOR_NAMES[$vi]}"
  url="${NEW_VENDOR_BUNDLE_URLS[$vi]}"
  vi=$((vi + 1))
  echo "-- $v <- $url"
  mkdir -p "$CERT_OUT_DIR/$v"
  # The bundle URL is portal-rooted but absolute may be a path; prefix
  # PORTAL_URL if it starts with '/'.
  case "$url" in
    http*) full_url="$url" ;;
    /*)    full_url="${PORTAL_URL}${url}" ;;
    *)     full_url="${PORTAL_URL}/${url}" ;;
  esac
  curl -fsS "$full_url" -o "$CERT_OUT_DIR/$v.tar.gz"
  tar -xzf "$CERT_OUT_DIR/$v.tar.gz" -C "$CERT_OUT_DIR/$v"
  ls "$CERT_OUT_DIR/$v/"
done

echo
echo "== declaring topics =="
for entry in "${TOPICS[@]}"; do
  IFS='|' read -r v key_pattern fmt schema_ref desc <<< "$entry"
  echo "-- $v / $key_pattern"
  body=$(jq -n \
    --arg key "$key_pattern" \
    --arg fmt "$fmt" \
    --arg ref "$schema_ref" \
    --arg d "$desc" \
    '{key_pattern: $key, format: $fmt, schema_ref: $ref, description: $d, publish_to_storage: true}')
  # Auth: operator bearer is enough; we don't need the per-vendor api_key.
  err_tmp=$(mktemp)
  if topic_resp=$(api POST "/api/vendors/${v}/topics" "$body" 2>"$err_tmp"); then
    : # 2xx — created
  else
    status=$(extract_status "$(<"$err_tmp")")
    case "$status" in
      409) echo "   already declared (HTTP 409); leaving as-is" ;;
      *)   echo "   FAILED (HTTP ${status:-?}): $topic_resp" ;;
    esac
  fi
  rm -f "$err_tmp"
done

# An earlier version crashed here with "unbound variable" when every
# vendor was already-registered (an empty associative array under
# nounset). The maps are indexed parallel arrays now (#227), whose
# length is safe when empty on every bash — but the "no new bundles"
# path is the common idempotent re-run case, so it stays explicit.
new_bundle_count="${#NEW_VENDOR_NAMES[@]}"
if [ "$new_bundle_count" -eq 0 ]; then
  echo
  echo "== no new vendor bundles minted this run — skipping SCP =="
  echo "(re-run case: every vendor was already registered. Existing cert tree at $SCP_TARGET is assumed intact.)"
  echo
  echo "== done =="
  echo "Next: run ansible-playbook ops/ansible/zenoh-synth-vendors-deploy.yml"
  exit 0
fi

if [ -n "$SCP_TARGET" ] && [ "$new_bundle_count" -gt 0 ]; then
  echo
  echo "== SCPing cert tree to $SCP_TARGET =="
  ssh "${SCP_TARGET%%:*}" "sudo mkdir -p ${SCP_TARGET#*:} && sudo chown -R \$USER ${SCP_TARGET#*:}" || true
  vi=0
  while [ "$vi" -lt "${#NEW_VENDOR_NAMES[@]}" ]; do
    scp -r "$CERT_OUT_DIR/${NEW_VENDOR_NAMES[$vi]}" "$SCP_TARGET/"
    vi=$((vi + 1))
  done
fi

echo
echo "== done =="
echo "Next: run ansible-playbook ops/ansible/zenoh-synth-vendors-deploy.yml"
