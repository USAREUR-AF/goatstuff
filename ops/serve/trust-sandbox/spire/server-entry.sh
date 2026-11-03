#!/bin/sh
# trust-sandbox SPIRE server entrypoint.
#
# 1. start the server   2. wait until healthy   3. mint a one-time agent join
# token (→ shared file)   4. register the orchestrator workload with -dns
# edge-processor (sets the SVID CN that zenohd's ACL matches)   5. export the
# SPIRE bundle + assemble zenohd's client trust bundle (sandbox-CA-root ⊕ SPIRE)
# 6. drop a bootstrap-done marker (the healthcheck gates dependents on it).
set -eu

CONF=/run/spire/config/server.conf
SHARED=/run/spire/shared
TOKEN_FILE="$SHARED/agent-join-token"
DONE="$SHARED/bootstrap-done"

mkdir -p "$SHARED/server-data"
rm -f "$DONE"

echo "[server-entry] starting spire-server"
spire-server run -config "$CONF" &
SRV=$!

echo "[server-entry] waiting for server health"
until spire-server healthcheck >/dev/null 2>&1; do
  # bail if the server died
  kill -0 "$SRV" 2>/dev/null || { echo "[server-entry] server exited early"; wait "$SRV"; exit 1; }
  sleep 1
done
echo "[server-entry] server healthy"

# (Re)mint a join token for the agent. Idempotent across recreate: a fresh token
# each boot is fine — the agent reads whatever is current.
spire-server token generate -spiffeID spiffe://trust.sandbox/agent \
  | sed -n 's/^Token: *//p' > "$TOKEN_FILE"
echo "[server-entry] wrote agent join token"

# Register the orchestrator workload. -dns edge-processor → the issued SVID
# carries CN=edge-processor, the load-bearing axis zenohd's cert_common_names
# ACL matches (the sandbox analog of the fleet's CN-bearing SVID, M-056). The
# unix:uid:0 selector matches the orchestrator across the shared socket. `|| true`
# so a recreate (entry already present) is not fatal.
spire-server entry create \
  -parentID spiffe://trust.sandbox/agent \
  -spiffeID spiffe://trust.sandbox/workload/edge-processor \
  -selector unix:uid:0 \
  -x509SVIDTTL 60 \
  -dns edge-processor \
  -dns edge-processor.trust.sandbox >/dev/null 2>&1 || true
echo "[server-entry] registered orchestrator workload (edge-processor)"

# Build the zenohd client trust bundle: sandbox CA root ⊕ SPIRE CA bundle, so a
# peer presenting EITHER a CA-minted leaf OR a SPIRE SVID passes the handshake.
write_bundle() {
  spire-server bundle show -format pem > /certs/spire-bundle.pem.tmp || return 1
  # Only swap a non-empty read into place. A transient RPC failure that
  # truncated the live bundle would revoke zenohd's whole SPIRE trust anchor.
  [ -s /certs/spire-bundle.pem.tmp ] || { rm -f /certs/spire-bundle.pem.tmp; return 1; }
  mv /certs/spire-bundle.pem.tmp /certs/spire-bundle.pem
  cat /certs/ca-root.pem /certs/spire-bundle.pem > /certs/client-ca-bundle.pem.tmp
  mv /certs/client-ca-bundle.pem.tmp /certs/client-ca-bundle.pem
}
write_bundle
echo "[server-entry] assembled /certs/client-ca-bundle.pem (sandbox-CA ⊕ SPIRE)"

touch "$DONE"
echo "[server-entry] bootstrap complete; tailing server"

# Keep it assembled. `ca_ttl = 168h` in server.conf means SPIRE rotates its CA
# WEEKLY, but the two lines above ran once, at container start — so the file on
# disk is a snapshot of a thing that moves, and zenohd's client trust anchor
# goes stale the moment the CA that minted it ages out.
#
# Without a refresh the bundle expires, the orchestrator's observer session drops
# with `received fatal alert: CertificateExpired` and never comes back, while
# SPIRE stays healthy, nginx serves 200 and oauth2-proxy answers /ping 200 — so
# every probe watching the sandbox calls it up.
#
# Refresh well inside ca_ttl. SPIRE keeps the outgoing CA in the bundle
# alongside the incoming one, so a rewrite never invalidates a live session.
while kill -0 "$SRV" 2>/dev/null; do
  sleep "${SPIRE_BUNDLE_REFRESH_S:-3600}"
  kill -0 "$SRV" 2>/dev/null || break
  if write_bundle; then
    echo "[server-entry] refreshed client-ca-bundle.pem"
  else
    # Loud, and NOT fatal: the previous bundle is still on disk and still
    # valid for the rest of ca_ttl, so failing here must not kill the server.
    echo "[server-entry] WARN bundle refresh failed; keeping previous bundle" >&2
  fi
done

wait "$SRV"
