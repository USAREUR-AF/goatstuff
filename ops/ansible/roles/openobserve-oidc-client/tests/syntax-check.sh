#!/usr/bin/env bash
set -euo pipefail
cd "$(dirname "$0")"
if ! command -v ansible-playbook >/dev/null 2>&1; then
  echo "[openobserve-oidc-client/syntax-check] SKIP"; exit 0
fi
ansible-playbook -i 'localhost,' -c local playbook.yml --syntax-check
bash ../../../tests/check-register-uniqueness.sh
echo "[openobserve-oidc-client/syntax-check] PASS"
