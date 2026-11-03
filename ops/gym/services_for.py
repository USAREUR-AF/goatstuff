#!/usr/bin/env python3
"""goat gym — a goatnet's service list, for building the site outside Ansible.

WHY THIS EXISTS. The gym's surface links are only rendered when the generator
is handed a service list, and at deploy time Ansible hands it one built from
the goatnet's own `isitup_services`. CI was rendering with no list at all, so
every check that runs against the built site was blind to the entire class of
link that a reader actually clicks, including links to machine doors that answer
404.

WHAT THIS IS NOT. It is not a second home for that fact. The Ansible task in
`roles/gym/tasks/main.yml` remains the authority: it templates the rows through
Ansible with every variable in scope, and its output is what ships. This reads
the same rows statically and can only resolve the two variables the rows'
own file always defines. It therefore REFUSES rather than guesses when a field
the gym renders still carries unresolved templating — a silently mangled URL in
a link checker is worse than no link checker.

Usage:
    services_for.py <group_vars-dir-name>      # e.g. zagros_hosts
    services_for.py --list                     # every goatnet that declares rows
"""

from __future__ import annotations

import argparse
import glob
import json
import os
import sys

try:
    import yaml
except ImportError:  # pragma: no cover
    sys.exit("services_for.py needs PyYAML (pip install pyyaml)")

HERE = os.path.dirname(os.path.abspath(__file__))
REPO = os.path.abspath(os.path.join(HERE, "..", ".."))
GROUP_VARS = os.path.join(REPO, "ops", "ansible", "group_vars")

# The fields the gym renders. Templating left in any of these would reach a
# reader, so they are the ones worth refusing over.
RENDERED = ("id", "name", "desc", "fqdn", "link_path", "kind", "access", "category")


def envs() -> dict[str, str]:
    """group_vars directory name -> the file declaring its `isitup_services`."""
    found: dict[str, str] = {}
    for path in sorted(glob.glob(os.path.join(GROUP_VARS, "*", "*.yml"))):
        try:
            with open(path) as source_file:
                doc = yaml.safe_load(source_file)
        except Exception:
            continue
        if isinstance(doc, dict) and isinstance(doc.get("isitup_services"), list):
            found[os.path.basename(os.path.dirname(path))] = path
    return found


def hosting() -> list[str]:
    """The goatnets that actually serve the gym (`gym_enabled: true`).

    Checking a goatnet that will never render these pages invents failures for
    a reader who does not exist — and zagros-prod, for one, deliberately does not
    host the gym. The flag may live in any of the goatnet's group_vars files.
    """
    out = set()
    for path in sorted(glob.glob(os.path.join(GROUP_VARS, "*", "*.yml"))):
        try:
            with open(path) as source_file:
                doc = yaml.safe_load(source_file)
        except Exception:
            continue
        if isinstance(doc, dict) and doc.get("gym_enabled") is True:
            out.add(os.path.basename(os.path.dirname(path)))
    return sorted(out & set(envs()))


def rows_for(env: str) -> list[dict]:
    path = envs().get(env)
    if not path:
        sys.exit(f"services_for.py: {env} declares no isitup_services")
    with open(path) as source_file:
        doc = yaml.safe_load(source_file)
    sid, zone = doc.get("site_id"), doc.get("site_dns_zone")
    if not sid or not zone:
        sys.exit(f"services_for.py: {env} declares rows but no site_id/site_dns_zone to "
                 "resolve them against")

    def resolve(value):
        if not isinstance(value, str):
            return value
        return value.replace("{{ site_id }}", str(sid)).replace("{{ site_dns_zone }}", str(zone))

    out = []
    for row in doc["isitup_services"]:
        if not isinstance(row, dict) or not row.get("id") or not row.get("fqdn"):
            continue
        resolved = {k: resolve(v) for k, v in row.items()}
        for field in RENDERED:
            value = resolved.get(field)
            if isinstance(value, str) and "{{" in value:
                sys.exit(f"services_for.py: {env} row {row['id']!r} leaves {field} templated "
                         f"({value!r}) — resolve it here or the site is built on a guess")
        out.append(resolved)

    # The status page carries no row for itself; the Ansible task adds it back
    # for the same reason, and the gym must be able to link it.
    if doc.get("isitup_enabled"):
        out.append({"id": "isitup", "name": "Status page",
                    "desc": "Every service this goatnet runs, and whether it is answering.",
                    "fqdn": doc.get("isitup_fqdn") or f"isitup.{sid}.{zone}",
                    "access": "admins", "kind": "ui", "category": "admin"})
    return out


def main() -> int:
    ap = argparse.ArgumentParser()
    ap.add_argument("env", nargs="?", help="group_vars directory name, e.g. zagros_hosts")
    ap.add_argument("--list", action="store_true", help="print every goatnet declaring rows")
    ap.add_argument("--list-hosting", action="store_true",
                    help="print only the goatnets that serve the gym (gym_enabled: true)")
    args = ap.parse_args()

    if args.list_hosting:
        print("\n".join(hosting()))
        return 0
    if args.list:
        print("\n".join(sorted(envs())))
        return 0
    if not args.env:
        ap.error("give a goatnet, or --list")
    json.dump(rows_for(args.env), sys.stdout, indent=1)
    print()
    return 0


if __name__ == "__main__":
    sys.exit(main())
