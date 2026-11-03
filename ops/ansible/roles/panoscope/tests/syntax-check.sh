#!/usr/bin/env bash
# SPDX-License-Identifier: Apache-2.0
# Syntax-only check for the panoscope role. Per ADR 50H — every load-
# bearing role ships a `tests/syntax-check.sh` that runs in CI to catch
# YAML / Jinja parse errors at PR time without needing a real host.
set -euo pipefail

cd "$(dirname "$0")"

if ! command -v ansible-playbook >/dev/null 2>&1; then
  echo "[panoscope/syntax-check] SKIP (ansible-playbook not on PATH)"
  exit 0
fi

echo "[panoscope/syntax-check] ansible-playbook --syntax-check on parse-only fixture"
ansible-playbook -i 'localhost,' -c local playbook.yml --syntax-check
