#!/usr/bin/env bash
# SPDX-License-Identifier: Apache-2.0
#
# retire-vendor.sh — retire a vendor from the goat-zenoh-portal in one
# shot. Bundles the portal-side DELETE (which cascades to topics +
# schemas + code-examples + per-vendor router rocksdb dir + ACL stanzas
# via the 83F revoke-path symmetry) with a print-out of the two
# operator-side follow-ons we cannot do from here (panoscope blocklist
# edit + ansible re-fire).
#
# What this DOES delete (portal-cascade, immediate):
#   - vendors row + bundle_token + api_key_hash
#   - all topics under <prefix>/**  (with FK CASCADE)
#   - all schema-body files under <SchemaDir>/<topic_id>/<version>.*
#   - all topic_code_examples rows
#   - the per-vendor rocksdb dir on the Zenoh router
#   - the four ACL marker stanzas in the router config (writes, reads,
#     storages, storage block) + SIGHUP'd zenohd
#
# What this DOES NOT delete (by design):
#   - audit-integrity events under <prefix>/**  in MinIO
#     → Object-Lock COMPLIANCE retention per ADR 0811 is irreversible
#     → events naturally roll off ≤7d (nova-a) or ≤1y (goat-prod) after
#       the last publish; that's the audit-trail invariant we promise.
#   - the panoscope replay registry — until you add <prefix> to
#     panoscope_minio_vendor_blocklist in group_vars/nova_hosts/panoscope.yml
#     and re-fire panoscope.yml, the retired vendor will keep
#     resurfacing as an audit-replay ghost on the COP at every restart.
#
# Design-of-record: docs/operations/retire-vendor-runbook.md
# Composes with: docs/design/panoscope.md §"Vendor blocklist (replay filter)"

set -euo pipefail

PORTAL_URL_DEFAULT="https://portal.nova.netbird.nova.example"
PORTAL_RESOLVE_DEFAULT="100.127.0.191"  # nova-a mesh IP (mesh CoreDNS handles the public URL otherwise)

usage() {
    cat <<EOF
Retire a vendor from the goat-zenoh-portal.

Usage: $(basename "$0") <vendor-prefix> [options]

Required:
  <vendor-prefix>           Exact vendor prefix to retire (e.g. caracal, community, drs)

Required env:
  PORTAL_OPERATOR_API_KEY   Operator API key for the portal /api/* surface

Options:
  --portal-url <url>        Portal base URL (default: ${PORTAL_URL_DEFAULT})
  --resolve <ip>            Force DNS resolve to this IP (default: ${PORTAL_RESOLVE_DEFAULT}).
                            Pass empty string to disable: --resolve ''
  --yes, -y                 Skip confirmation prompt
  --help, -h                Show this message

Flow:
  1. Pre-flight: portal /api/health reachable, vendor exists in /api/vendors
  2. Summary: list the vendor's current topics + schemas + bundle metadata
  3. Confirm (unless --yes)
  4. DELETE /api/vendors/<prefix>  (portal cascades; see header for scope)
  5. Print the three follow-on steps the operator must take:
       a. Add <prefix> to panoscope_minio_vendor_blocklist (nova_hosts)
       b. Re-fire ansible-playbook panoscope.yml
       c. Audit events under <prefix>/** roll off in ≤7d (no action)

Exit codes:
  0  retired (HTTP 200/204 from portal)
  1  usage error
  2  pre-flight failed (portal unreachable / unauth / vendor not found)
  3  user declined confirmation
  4  portal DELETE returned non-2xx (printed for diagnosis)
EOF
}

VENDOR=""
PORTAL_URL="${PORTAL_URL_DEFAULT}"
RESOLVE_IP="${PORTAL_RESOLVE_DEFAULT}"
ASSUME_YES=0

while [[ $# -gt 0 ]]; do
    case "$1" in
        --help|-h)        usage; exit 0 ;;
        --yes|-y)         ASSUME_YES=1; shift ;;
        --portal-url)     PORTAL_URL="$2"; shift 2 ;;
        --resolve)        RESOLVE_IP="$2"; shift 2 ;;
        --)               shift; break ;;
        -*)               echo "unknown flag: $1" >&2; usage >&2; exit 1 ;;
        *)                if [[ -z "$VENDOR" ]]; then VENDOR="$1"; shift; else echo "unexpected arg: $1" >&2; exit 1; fi ;;
    esac
done

if [[ -z "$VENDOR" ]]; then
    echo "error: <vendor-prefix> is required" >&2
    usage >&2
    exit 1
fi

if [[ -z "${PORTAL_OPERATOR_API_KEY:-}" ]]; then
    echo "error: PORTAL_OPERATOR_API_KEY env var must be set" >&2
    exit 1
fi

# curl helpers — keep the --resolve hop optional so this works off-mesh
# too (e.g. once goat-net public DNS lights up).
curl_args=(--silent --show-error --max-time 15 -H "Authorization: Bearer ${PORTAL_OPERATOR_API_KEY}")
host_no_scheme="${PORTAL_URL#*://}"
host_no_path="${host_no_scheme%%/*}"
if [[ -n "$RESOLVE_IP" ]]; then
    curl_args+=(--resolve "${host_no_path}:443:${RESOLVE_IP}")
fi

echo "==> Pre-flight: portal reachability + auth"
health_code=$(curl "${curl_args[@]}" -o /dev/null -w '%{http_code}' "${PORTAL_URL}/api/health" || true)
if [[ "$health_code" != "200" ]]; then
    echo "error: portal /api/health returned HTTP ${health_code} (expected 200)" >&2
    echo "       check PORTAL_OPERATOR_API_KEY + portal URL + mesh connectivity" >&2
    exit 2
fi
echo "    portal /api/health: 200"

echo "==> Looking up vendor '${VENDOR}'"
vendor_resp=$(curl "${curl_args[@]}" -w '\n%{http_code}' "${PORTAL_URL}/api/vendors/${VENDOR}" || true)
vendor_code=$(printf '%s' "$vendor_resp" | tail -n1)
vendor_body=$(printf '%s' "$vendor_resp" | sed '$d')
if [[ "$vendor_code" != "200" ]]; then
    echo "error: portal /api/vendors/${VENDOR} returned HTTP ${vendor_code}" >&2
    if [[ "$vendor_code" == "404" ]]; then
        echo "       (vendor '${VENDOR}' is not registered — nothing to retire)" >&2
    fi
    exit 2
fi

# Pretty-print a brief summary of what we're about to nuke. The
# vendor JSON shape includes display_name + bundle_minted_at; the
# topics live under a separate endpoint.
echo "    vendor metadata:"
printf '%s' "$vendor_body" | python3 -c '
import sys, json
v = json.loads(sys.stdin.read())
for k in ("prefix", "display_name", "synthetic", "bundle_minted_at", "created_at"):
    if k in v:
        print(f"      {k}: {v[k]}")
' 2>/dev/null || printf '%s\n' "$vendor_body" | sed 's/^/      /'

topics_resp=$(curl "${curl_args[@]}" "${PORTAL_URL}/api/vendors/${VENDOR}/topics" || true)
topic_count=$(printf '%s' "$topics_resp" | python3 -c 'import sys,json; d=json.load(sys.stdin); print(len(d if isinstance(d,list) else d.get("topics",[])))' 2>/dev/null || echo "?")
echo "    topics registered under <${VENDOR}>/**: ${topic_count}"

if [[ "$ASSUME_YES" -eq 0 ]]; then
    echo
    echo "About to: DELETE ${PORTAL_URL}/api/vendors/${VENDOR}"
    echo "  Cascades: topics + schemas + code-examples + router rocksdb + ACL stanzas + zenohd SIGHUP"
    echo "  Does NOT touch: audit-integrity events (Object-Lock COMPLIANCE, naturally roll off ≤7d)"
    echo
    read -r -p "Proceed? [yes/NO] " confirm
    if [[ "$confirm" != "yes" ]]; then
        echo "aborted" >&2
        exit 3
    fi
fi

echo "==> DELETE /api/vendors/${VENDOR}"
delete_resp=$(curl "${curl_args[@]}" -X DELETE -w '\n%{http_code}' "${PORTAL_URL}/api/vendors/${VENDOR}" || true)
delete_code=$(printf '%s' "$delete_resp" | tail -n1)
delete_body=$(printf '%s' "$delete_resp" | sed '$d')

if [[ "$delete_code" != "200" && "$delete_code" != "204" ]]; then
    echo "error: portal DELETE returned HTTP ${delete_code}" >&2
    if [[ -n "$delete_body" ]]; then
        echo "       body: ${delete_body}" >&2
    fi
    if [[ "$delete_code" == "409" ]]; then
        echo "       (vendor has non-retired topics — see portal 'retire all topics first' guard)" >&2
    fi
    exit 4
fi
echo "    portal DELETE: ${delete_code}  (portal cascade done; router rocksdb wiped; zenohd SIGHUP'd)"

cat <<EOF

==> Done. Three follow-on steps remain (operator-fired):

  1. Add '${VENDOR}' to panoscope_minio_vendor_blocklist in:
       ops/ansible/group_vars/nova_hosts/panoscope.yml

     Otherwise panoscope keeps resurfacing this vendor as an
     audit-replay ghost at every restart until the audit events
     roll off in ≤7d.

  2. Re-fire the panoscope role so the new blocklist takes effect:
       cd ops/ansible
       OBJC_DISABLE_INITIALIZE_FORK_SAFETY=YES \\
         CLOUDFLARE_API_TOKEN=\$CLOUDFLARE_API_TOKEN_NOVA \\
         ansible-playbook -i inventory/hosts.ini \\
           -i inventory/mgmt-hosts.ini -i inventory/mgmt-hosts-mesh.ini \\
           panoscope.yml

  3. Audit events under '${VENDOR}/**' stay in MinIO audit-integrity
     under Object-Lock COMPLIANCE retention until they roll off
     naturally (≤7d on nova-a, ≤1y in goat-prod). This is the audit
     invariant per ADR 0811 — no action needed, just awareness of the
     storage footprint.

See docs/operations/retire-vendor-runbook.md for the full picture.
EOF
