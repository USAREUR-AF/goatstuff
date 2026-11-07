#!/usr/bin/env bash
# backup-tak.sh — Backup TAK Server database and certificates
#
# Creates a timestamped backup of:
#   - PostgreSQL database (pg_dump)
#   - Certificate directory (Root CA, server cert, all client certs)
#
# Backups are stored locally in ops/tak/backups/.
# The Root CA private key should also be backed up to 1Password manually.
#
# Usage:
#   ops/tak/backup-tak.sh

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

# A deployment that exports other names (the network host's password as
# NETHOST_PASS, ...) keeps them through its ops/env-aliases.d/*.sh, sourced by
# goatcore's ops/scripts/lib/env-aliases.sh in an assembled tree.
if [ -f "$REPO_DIR/ops/scripts/lib/env-aliases.sh" ]; then
  # shellcheck source=/dev/null
  . "$REPO_DIR/ops/scripts/lib/env-aliases.sh"
fi
# The SSH account defaults to the deployment's deploy.ssh_user
# (ops/goatbuilder/project.yml, read by ops/scripts/goatnet-facts.py).
DEPLOY_SSH_USER="$(python3 "$REPO_DIR/ops/scripts/goatnet-facts.py" deploy ssh_user 2>/dev/null || true)"

TAK_HOST="${TAK_HOST:?TAK_HOST not set — set in ops/tak/.env}"
TAK_USER="${TAK_USER:-${DEPLOY_SSH_USER:-goat}}"
TAK_PG_PASSWORD="${TAK_PG_PASSWORD:?TAK_PG_PASSWORD not set}"

SSH_TARGET="$TAK_USER@$TAK_HOST"
TIMESTAMP=$(date +%Y%m%d-%H%M%S)
BACKUP_DIR="$SCRIPT_DIR/backups/$TIMESTAMP"

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

mkdir -p "$BACKUP_DIR"

# Verify connectivity
if ! ssh_cmd 'echo ok' >/dev/null 2>&1; then
  err "Cannot SSH to TAK NETHOST ($SSH_TARGET)"
  exit 1
fi

# Backup PostgreSQL
log "Backing up TAK PostgreSQL database..."
ssh_cmd "sudo docker exec tak-db pg_dump -U martiuser -d cot" > "$BACKUP_DIR/tak-db.sql"
log "Database backup: $BACKUP_DIR/tak-db.sql ($(wc -c < "$BACKUP_DIR/tak-db.sql" | tr -d ' ') bytes)"

# Backup certificates
log "Backing up TAK certificates..."
ssh_cmd 'sudo tar czf /tmp/tak-certs-backup.tar.gz -C /opt/tak/certs .' 2>/dev/null
scp_cmd "$SSH_TARGET:/tmp/tak-certs-backup.tar.gz" "$BACKUP_DIR/"
ssh_cmd 'sudo rm -f /tmp/tak-certs-backup.tar.gz'
log "Certificate backup: $BACKUP_DIR/tak-certs-backup.tar.gz"

# Summary
log ""
log "=== Backup complete ==="
log "Location: $BACKUP_DIR"
log ""
log "Contents:"
ls -lh "$BACKUP_DIR"
log ""
log "REMINDER: Back up the Root CA private key to 1Password if not already done."
