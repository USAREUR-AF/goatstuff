#!/usr/bin/env bash
set -euo pipefail
cd "$(dirname "$0")"
if ! command -v ansible-playbook >/dev/null 2>&1; then
  echo "[rollback-ledger/syntax-check] SKIP (ansible-playbook not on PATH)"
  exit 0
fi
echo "[rollback-ledger/syntax-check] ansible-playbook --syntax-check"
ansible-playbook -i 'localhost,' -c local playbook.yml --syntax-check

# The role is a universal meta-dependency, so a governed host's variable shape
# must parse too — not only the ungoverned default. Same fixture, goatnet set.
echo "[rollback-ledger/syntax-check] governed-host variable shape"
ansible-playbook -i 'localhost,' -c local playbook.yml --syntax-check \
  -e consistency_goatnet=zagros -e consistency_member_group=zagros_hosts

echo "[rollback-ledger/syntax-check] PASS"
