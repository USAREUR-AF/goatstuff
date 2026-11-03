#!/usr/bin/env bash
# generate-certs.sh — Generate TAK certificates via Ansible
#
# Thin wrapper. For client certs, use enroll-client.sh instead.
#
# Usage:
#   ops/tak/generate-certs.sh                # Regenerate CA + server + admin
#   ops/tak/generate-certs.sh --client NAME  # Use enroll-client.sh instead

set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "$0")" && pwd)"
ANSIBLE_DIR="$SCRIPT_DIR/../ansible"

if [ "${1:-}" = "--client" ]; then
  echo "Use ops/tak/enroll-client.sh ${2:-<name>} instead."
  exit 1
fi

cd "$ANSIBLE_DIR"
ansible-playbook tak-server.yml --tags certs
