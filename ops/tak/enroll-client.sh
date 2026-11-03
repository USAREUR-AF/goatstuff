#!/usr/bin/env bash
# enroll-client.sh — Generate a TAK client certificate and connection data package
#
# Creates a .p12 client certificate and a connection data package (.zip) that
# ATAK/WinTAK can import directly. The data package contains the client cert,
# the CA truststore, and a server connection profile.
#
# Usage:
#   ops/tak/enroll-client.sh <username>
#   ops/tak/enroll-client.sh operator-01
#
# Output:
#   ops/tak/certs/<username>.p12       — client certificate
#   ops/tak/certs/<username>-dp.zip    — ATAK data package (cert + profile)

set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "$0")" && pwd)"
REPO_DIR="$(cd "$SCRIPT_DIR/../.." && pwd)"

# Source credentials
if [ -f "$REPO_DIR/.envrc" ]; then
  set -a
  source "$REPO_DIR/.envrc" 2>/dev/null || true
  [ -f "$REPO_DIR/.envrc.local" ] && source "$REPO_DIR/.envrc.local" 2>/dev/null || true
  set +a
fi

if [ -f "$SCRIPT_DIR/.env" ]; then
  set -a
  source "$SCRIPT_DIR/.env"
  set +a
fi

USERNAME="${1:?Usage: enroll-client.sh <username>}"

TAK_HOST="${TAK_HOST:?TAK_HOST not set — set in ops/tak/.env}"
TAK_USER="${TAK_USER:-goat}"
TAK_HOSTNAME="${TAK_HOSTNAME:-tak.netbird.legacy.example}"

SSH_TARGET="$TAK_USER@$TAK_HOST"

log() { echo "[$(date +%H:%M:%S)] $*"; }
err() { echo "[$(date +%H:%M:%S)] ERROR: $*" >&2; }

ssh_cmd() {
  if [ -n "${NETHOST_PASS:-}" ]; then
    sshpass -p "$NETHOST_PASS" ssh -o StrictHostKeyChecking=no -o ConnectTimeout=10 "$SSH_TARGET" "$@"
  else
    ssh -o StrictHostKeyChecking=no -o ConnectTimeout=10 "$SSH_TARGET" "$@"
  fi
}

scp_cmd() {
  if [ -n "${NETHOST_PASS:-}" ]; then
    sshpass -p "$NETHOST_PASS" scp -o StrictHostKeyChecking=no -o ConnectTimeout=10 "$@"
  else
    scp -o StrictHostKeyChecking=no -o ConnectTimeout=10 "$@"
  fi
}

# Verify TAK Server is running
if ! ssh_cmd 'sudo docker inspect takserver --format "{{.State.Status}}"' 2>/dev/null | grep -q running; then
  err "TAK Server container not running. Run deploy-tak.sh first."
  exit 1
fi

# Generate client certificate
log "Generating certificate for: $USERNAME"
ssh_cmd "sudo docker exec takserver bash -c '
  cd /opt/tak/certs &&
  ./makeCert.sh client $USERNAME &&
  chmod 644 files/$USERNAME.p12
'"

# Create ATAK connection data package on the TAK NETHOST
log "Creating ATAK data package..."
ssh_cmd "sudo bash -c '
  TMPDIR=\$(mktemp -d)
  mkdir -p \$TMPDIR/certs

  # Copy client cert and CA truststore into the data package
  cp /opt/tak/certs/files/$USERNAME.p12 \$TMPDIR/certs/
  cp /opt/tak/certs/files/truststore-*.p12 \$TMPDIR/certs/ 2>/dev/null || true

  # Create server connection preference file
  cat > \$TMPDIR/certs/${USERNAME}_preference.pref << PREF
<?xml version=\"1.0\" encoding=\"UTF-8\"?>
<preferences>
  <preference version=\"1\" name=\"cot_streams\">
    <entry key=\"count\" class=\"class java.lang.Integer\">1</entry>
    <entry key=\"description0\" class=\"class java.lang.String\">goat TAK</entry>
    <entry key=\"enabled0\" class=\"class java.lang.Boolean\">true</entry>
    <entry key=\"connectString0\" class=\"class java.lang.String\">$TAK_HOST:8089:ssl</entry>
  </preference>
  <preference version=\"1\" name=\"com.atakmap.app_preferences\">
    <entry key=\"displayServerConnectionWidget\" class=\"class java.lang.Boolean\">true</entry>
    <entry key=\"caLocation\" class=\"class java.lang.String\">/cert/truststore-goatCA.p12</entry>
    <entry key=\"clientPassword\" class=\"class java.lang.String\">atakatak</entry>
    <entry key=\"certificateLocation\" class=\"class java.lang.String\">/cert/$USERNAME.p12</entry>
  </preference>
</preferences>
PREF

  # Create the MANIFEST
  cat > \$TMPDIR/MANIFEST/manifest.xml << MANIFEST || true
  mkdir -p \$TMPDIR/MANIFEST
  cat > \$TMPDIR/MANIFEST/manifest.xml << MANIFEST
<?xml version=\"1.0\" encoding=\"UTF-8\"?>
<MissionPackageManifest version=\"2\">
  <Configuration>
    <Parameter name=\"uid\" value=\"$USERNAME-dp\"/>
    <Parameter name=\"name\" value=\"goat TAK — $USERNAME\"/>
  </Configuration>
  <Contents>
    <Content ignore=\"false\" zipEntry=\"certs/$USERNAME.p12\"/>
    <Content ignore=\"false\" zipEntry=\"certs/${USERNAME}_preference.pref\"/>
  </Contents>
</MissionPackageManifest>
MANIFEST

  # Package as zip
  cd \$TMPDIR
  zip -r /tmp/${USERNAME}-dp.zip certs/ MANIFEST/
  rm -rf \$TMPDIR
'"

# Copy files back to local machine
mkdir -p "$SCRIPT_DIR/certs"
scp_cmd "$SSH_TARGET:/opt/tak/certs/files/$USERNAME.p12" "$SCRIPT_DIR/certs/"
scp_cmd "$SSH_TARGET:/tmp/$USERNAME-dp.zip" "$SCRIPT_DIR/certs/"
ssh_cmd "sudo rm -f /tmp/$USERNAME-dp.zip"

log ""
log "=== Client enrollment complete ==="
log ""
log "Files:"
log "  Certificate:    ops/tak/certs/$USERNAME.p12"
log "  Data Package:   ops/tak/certs/$USERNAME-dp.zip"
log ""
log "For ATAK:"
log "  1. Transfer $USERNAME-dp.zip to the Android device"
log "  2. Open ATAK → Import Manager → Import from file"
log "  3. Select the .zip — ATAK imports the cert and server connection automatically"
log ""
log "For WebTAK:"
log "  1. Import $USERNAME.p12 into the browser certificate store"
log "  2. Navigate to https://$TAK_HOSTNAME:8443/webtak/index.html"
log ""
log "For WinTAK:"
log "  1. Import $USERNAME-dp.zip via Settings → Network → Import"
