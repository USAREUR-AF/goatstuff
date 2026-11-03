#!/usr/bin/env bash
# deploy-tak.sh — Deploy TAK Server via Ansible
#
# Thin wrapper around the Ansible playbook. Use this for convenience
# or run the playbook directly for more control.
#
# Usage:
#   ops/tak/deploy-tak.sh                                    # Full deploy
#   ops/tak/deploy-tak.sh --federation <PARTNER-ENDPOINT>      # With federation

set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "$0")" && pwd)"
ANSIBLE_DIR="$SCRIPT_DIR/../ansible"

extra_args=""

if [ "${1:-}" = "--federation" ]; then
  federation_host="${2:?Usage: deploy-tak.sh --federation <endpoint> [port]}"
  federation_port="${3:-9000}"
  extra_args="-e federation_host=$federation_host -e federation_port=$federation_port"
fi

cd "$ANSIBLE_DIR"
ansible-playbook tak-server.yml $extra_args
