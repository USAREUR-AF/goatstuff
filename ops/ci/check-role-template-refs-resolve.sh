#!/usr/bin/env bash
# SPDX-License-Identifier: Apache-2.0
#
# check-role-template-refs-resolve.sh — assert every `src:` in an Ansible role
# names a template that exists in that role's templates/ directory.
#
# WHY THIS EXISTS: a rename can move the reference and leave the file, or move
# the file and leave the reference, and nothing else notices. Every YAML file
# still parses, every guard still passes, doc links stay clean. It fails at deploy
# time, on the mgmt host, with "Could not find or access template" — i.e.
# discovered by an operator mid-run rather than by CI.
#
# WHAT IT CHECKS: for each roles/<role>/tasks/*.yml, every bare `src: <x>.j2`
# must exist at roles/<role>/templates/<x>. Paths containing a slash are
# skipped: those are role-relative or absolute lookups whose resolution rules
# differ, and asserting the simple case is worth more than a partial model of
# the complex one.
#
# Deliberately NOT checking `copy:` src or `lookup('template', ...)` — the
# first resolves against files/ with its own search path, the second is
# computed. Extend when a failure justifies it, not before.
set -euo pipefail
cd "$(git rev-parse --show-toplevel)"

python3 - <<'PY'
import re, glob, os, sys
bad = []
checked = 0
for tasks in sorted(glob.glob('ops/ansible/roles/*/tasks/*.yml')):
    role = tasks.split('/')[3]
    text = open(tasks).read()
    for m in re.finditer(r'^\s*src:\s*["\']?([A-Za-z0-9._-]+\.j2)["\']?\s*$', text, re.M):
        t = m.group(1)
        checked += 1
        if not os.path.exists(f'ops/ansible/roles/{role}/templates/{t}'):
            line = text[:m.start()].count('\n') + 1
            bad.append(f'{tasks}:{line} -> roles/{role}/templates/{t}')

if bad:
    print(f'check-role-template-refs: FAIL — {len(bad)} reference(s) name a template that does not exist')
    for b in bad:
        print(f'    {b}')
    print('  A rename moved one half and not the other. This fails at deploy, not here,')
    print('  unless this guard catches it.')
    sys.exit(1)

print(f'check-role-template-refs: OK — {checked} role template reference(s) all resolve')
PY
