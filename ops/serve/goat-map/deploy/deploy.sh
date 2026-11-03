#!/usr/bin/env bash
# SPDX-License-Identifier: Apache-2.0
#
# Deploy goat-map to one host of one goatnet.
#
#   deploy/deploy.sh <goatnet> <ssh-target> [--build] [--unfence]
#   deploy/deploy.sh my-goatnet ops@my-mesh-host --build
#
# --unfence is required to deploy an UNFENCED profile over a host currently
# running the oauth2-proxy fence. Without it that downgrade is refused, so a
# stale branch cannot strip authentication from a live surface by accident.
#
# Self-contained: no dependency on the goat monorepo, no Ansible. The goatnet
# profile in deploy/goatnets/<goatnet>.env is the only place a site, host,
# zone or endpoint is named.
#
# --build builds the image on the target host. Without it the profile's
# GOATMAP_IMAGE must reference a registry image the host can pull; that is
# the intended steady state (pinned by digest), and --build is the bootstrap
# path before the first CI image exists for the host's architecture.
set -euo pipefail

REPO_ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
REMOTE_DIR="${GOATMAP_REMOTE_DIR:-/opt/goat-map}"
STAGE_DIR="${GOATMAP_STAGE_DIR:-goat-map-src}"

usage() { sed -n '3,20p' "${BASH_SOURCE[0]}"; exit 2; }

[ $# -ge 2 ] || usage
GOATNET="$1"; TARGET="$2"; shift 2
BUILD=0
UNFENCE=0
for arg in "$@"; do
  case "$arg" in
    --build) BUILD=1 ;;
    --unfence) UNFENCE=1 ;;
    *) echo "unknown option: $arg" >&2; usage ;;
  esac
done

PROFILE="${REPO_ROOT}/deploy/goatnets/${GOATNET}.env"
if [ ! -f "$PROFILE" ]; then
  echo "no profile for goatnet '${GOATNET}' at ${PROFILE}" >&2
  echo "available:" >&2
  ls -1 "${REPO_ROOT}/deploy/goatnets/" | sed 's/\.env$//' | sed 's/^/  /' >&2
  exit 1
fi

# Refuse the one misconfiguration that would silently make a mesh-only service
# publicly reachable on whatever else the host is attached to.
BIND_IP="$(grep -E '^GOATMAP_BIND_IP=' "$PROFILE" | cut -d= -f2- | tr -d '[:space:]')"
if [ -z "$BIND_IP" ] || [ "$BIND_IP" = "0.0.0.0" ]; then
  echo "refusing: GOATMAP_BIND_IP in ${PROFILE} is '${BIND_IP:-<empty>}'." >&2
  echo "It must be the host's mesh IP — mesh membership is the only access gate." >&2
  exit 1
fi

echo "==> goat-map → ${GOATNET} on ${TARGET} (bind ${BIND_IP})"

echo "==> staging source"
rsync -az --delete \
  --exclude '.git' --exclude '__pycache__' --exclude '.pytest_cache' --exclude '*.pyc' \
  -e "ssh -o ConnectTimeout=10" \
  "${REPO_ROOT}/" "${TARGET}:${STAGE_DIR}/"

echo "==> installing profile as .env"
scp -q "$PROFILE" "${TARGET}:${STAGE_DIR}/.env"

# Secrets are NEVER in the committed profile. They come from the operator's
# environment at deploy time and are appended to the on-host .env only.
# --- the oauth2-proxy fence ------------------------------------------------
# A profile that asks for the fence must get the fence. Rendering a half
# configured proxy, or silently skipping it, would leave the surface open
# while the profile claims otherwise — the exact failure this fence exists to
# close.
if grep -qE '^COMPOSE_PROFILES=.*fenced' "$PROFILE"; then
  : "${GOATMAP_OAUTH2_CLIENT_SECRET:?fenced profile needs GOATMAP_OAUTH2_CLIENT_SECRET in the environment}"
  : "${GOATMAP_OAUTH2_COOKIE_SECRET:?fenced profile needs GOATMAP_OAUTH2_COOKIE_SECRET in the environment}"
  hostname="$(grep -E '^GOATMAP_HOSTNAME=' "$PROFILE" | cut -d= -f2- | tr -d '[:space:]')"
  issuer="$(grep -E '^GOATMAP_OIDC_ISSUER_URL=' "$PROFILE" | cut -d= -f2- | tr -d '[:space:]')"
  client="$(grep -E '^GOATMAP_OAUTH2_CLIENT_ID=' "$PROFILE" | cut -d= -f2- | tr -d '[:space:]')"
  groups="$(grep -E '^GOATMAP_OAUTH2_ALLOWED_GROUPS=' "$PROFILE" | cut -d= -f2- | tr -d '[:space:]')"
  upstream="$(grep -E '^GOATMAP_CONTAINER=' "$PROFILE" | cut -d= -f2- | tr -d '[:space:]')"
  for v in hostname issuer client groups upstream; do
    [ -n "${!v}" ] || { echo "fenced profile is missing $v" >&2; exit 1; }
  done
  echo "==> rendering the oauth2-proxy fence for ${hostname}"
  sed -e "s|__ISSUER_URL__|${issuer}|" \
      -e "s|__CLIENT_ID__|${client}|" \
      -e "s|__CLIENT_SECRET__|${GOATMAP_OAUTH2_CLIENT_SECRET}|" \
      -e "s|__COOKIE_SECRET__|${GOATMAP_OAUTH2_COOKIE_SECRET}|" \
      -e "s|__ALLOWED_GROUPS__|${groups}|" \
      -e "s|__HOSTNAME__|${hostname}|" \
      -e "s|__UPSTREAM_CONTAINER__|${upstream}|" \
      "${REPO_ROOT}/deploy/oauth2-proxy.env.template" > /tmp/goat-map-oauth2.env
  scp -q /tmp/goat-map-oauth2.env "${TARGET}:${STAGE_DIR}/oauth2-proxy.env"
  shred -u /tmp/goat-map-oauth2.env 2>/dev/null || rm -f /tmp/goat-map-oauth2.env
else
  # THE FENCE IS A RATCHET. Removing it must be a deliberate act, never a
  # side effect of deploying an older branch.
  #
  # WHY: deploying from a branch that predates the fence takes the branch below,
  # prints a warning and proceeds — `--remove-orphans` reaps the oauth2-proxy
  # container and the app re-publishes its own host port. The surface is then
  # unauthenticated and nothing says so; a warning scrolls past in a
  # backgrounded deploy log.
  #
  # A warning is not a control. If the live deployment is fenced and the
  # incoming profile is not, that is a downgrade, and downgrades stop here.
  live_profiles="$(ssh "$TARGET" \
    "sudo -n grep -hE '^COMPOSE_PROFILES=' '${REMOTE_DIR}/.env' 2>/dev/null || true")"
  if printf '%s' "$live_profiles" | grep -q 'fenced' && [ "$UNFENCE" = 0 ]; then
    cat >&2 <<EOF
==> REFUSING: ${TARGET} currently runs goat-map BEHIND the oauth2-proxy fence,
    and this profile (${PROFILE##*/}) does not request it.

    Deploying it would strip authentication from a live surface — almost
    always the sign of a stale branch rather than an intent to unfence.

    Rebase onto main (the fence is there), or, if you genuinely mean to
    remove it, pass --unfence.
EOF
    exit 1
  fi
  if printf '%s' "$live_profiles" | grep -q 'fenced'; then
    echo "==> --unfence: REMOVING the fence from ${TARGET} at your request" >&2
  fi
  echo "==> WARNING: profile does not request the fence — this surface will be UNAUTHENTICATED" >&2
fi

if [ -n "${GOATMAP_PORTAL_TOKEN:-}" ]; then
  echo "==> appending portal token from environment (not committed)"
  ssh "$TARGET" "printf 'GOATMAP_PORTAL_TOKEN=%s\n' '${GOATMAP_PORTAL_TOKEN}' >> '${STAGE_DIR}/.env'"
else
  echo "==> NOTE: GOATMAP_PORTAL_TOKEN unset — producer provenance will be UNVERIFIED" >&2
fi

echo "==> publishing to ${REMOTE_DIR} and starting"
# shellcheck disable=SC2029  # deliberate client-side expansion of the vars below
ssh "$TARGET" "sudo -n mkdir -p '${REMOTE_DIR}' \
  && sudo -n rsync -a --delete '${STAGE_DIR}/' '${REMOTE_DIR}/' \
  && cd '${REMOTE_DIR}' \
  && sudo -n docker compose up -d $([ "$BUILD" = 1 ] && echo --build) --remove-orphans"

echo "==> waiting for health"
# /healthz is the one route the fence leaves open (counters only, no track
# data), so this probe works fenced or not.
for _ in $(seq 1 30); do
  if curl -sf --max-time 4 "http://${BIND_IP}:$(grep -E '^GOATMAP_PORT=' "$PROFILE" | cut -d= -f2- | tr -d '[:space:]')/healthz" >/dev/null; then
    echo "==> healthy"
    curl -s "http://${BIND_IP}:$(grep -E '^GOATMAP_PORT=' "$PROFILE" | cut -d= -f2- | tr -d '[:space:]')/healthz"
    echo
    exit 0
  fi
  sleep 2
done

echo "==> did NOT become healthy in 60s — logs:" >&2
ssh "$TARGET" "sudo -n docker logs --tail 40 \$(grep -E '^GOATMAP_CONTAINER=' '${REMOTE_DIR}/.env' | cut -d= -f2-)" >&2
exit 1
