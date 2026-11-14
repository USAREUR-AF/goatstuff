#!/usr/bin/env python3
"""goat gym — registry gate and coverage report.

WHAT THIS REFUSES, and the failure each one prevents:

  a written module with no walkthrough
        The first version of this gym shipped pages that said "the explanation
        lives elsewhere" and printed a file path. That is not training, it is a
        citation with the reader's time spent on it. A module that opens must
        teach: summary, steps you can follow, and what to do about the quirks.

  a quirk with no evidence, or no `so:`
        Evidence keeps it from being folklore. `so:` is what turns a fact into
        an instruction — "this is true" without "so do this" leaves the reader
        exactly where they started.

  a drill with no pass condition
        An exercise nobody can grade is a suggestion.

  a `deeper:` document that does not exist
        The document library is built from these. A missing file becomes a
        missing page, silently.

  a `surface:` id no goatnet declares
        The join to the live service list is what makes coverage measurable.

  a repo path on a RENDERED page
        Most readers have no repository. A path is a dead end wearing the
        clothes of a reference. This is checked against the built site, not the
        registry, because that is where the reader meets it.

  a link to a MACHINE DOOR on a rendered page
        A service row marked `kind: api` has no web page. Through the ingress
        those names route nothing and answer 404. A gym that links every row it is
        handed sends a reader following "Open it on this goatnet" to a 404. Same reason as the rule above: the
        check is against the built site, because that is where the reader
        meets it.

Coverage is REPORTED, not enforced: failing a build over content nobody has
written yet teaches people to bypass the gate.

Usage:
    check.py                     # gate + coverage
    check.py --site DIR          # also check a rendered site for repo paths
    check.py --json
"""

from __future__ import annotations

import argparse
import glob
import json
import os
import re
import sys

try:
    import yaml
except ImportError:  # pragma: no cover
    sys.exit("check.py needs PyYAML (pip install pyyaml)")

HERE = os.path.dirname(os.path.abspath(__file__))
REPO = os.path.abspath(os.path.join(HERE, "..", ".."))
REGISTRY = os.path.join(HERE, "gym.yaml")

SAFETY = {"read-only", "writes", "admin"}
LEVELS = {"basics", "working", "mastery"}
STATUS = {"written", "planned"}

# A path shaped like something in this repository, as it would appear in text.
REPO_PATH = re.compile(
    r"\b(?:docs|ops|terraform[\w-]*|\.github)/[\w./-]+\.(?:md|ya?ml|py|sh|go|j2|rs|json)\b")


def harvest_isitup_ids() -> dict[str, set[str]]:
    found: dict[str, set[str]] = {}
    for path in sorted(glob.glob(os.path.join(REPO, "ops", "ansible", "group_vars", "*", "*.yml"))):
        try:
            with open(path) as source_file:
                doc = yaml.safe_load(source_file)
        except Exception:
            continue
        if not isinstance(doc, dict):
            continue
        rows = doc.get("isitup_services")
        if not isinstance(rows, list):
            continue
        env = os.path.basename(os.path.dirname(path))
        for row in rows:
            if isinstance(row, dict) and row.get("id"):
                found.setdefault(str(row["id"]), set()).add(env)
        # The status page carries no row for itself — it is the thing rendering
        # the rows. It is still a service a module may train on, and the gym
        # still has to be able to link to it, so admit it where the goatnet
        # enables it (the hostname comes from the role's own convention).
        if doc.get("isitup_enabled"):
            found.setdefault("isitup", set()).add(env)
    return found


def harvest_machine_doors() -> dict[str, str]:
    """Concrete hostnames that answer 404 to a browser: `kind: api` rows.

    The fqdn is templated per goatnet, so it is resolved against the SAME file's
    own `site_id` / `site_dns_zone` — every declaring file carries both, and no
    other substitution is needed for these rows.
    """
    doors: dict[str, str] = {}
    for path in sorted(glob.glob(os.path.join(REPO, "ops", "ansible", "group_vars", "*", "*.yml"))):
        try:
            with open(path) as source_file:
                doc = yaml.safe_load(source_file)
        except Exception:
            continue
        if not isinstance(doc, dict) or not isinstance(doc.get("isitup_services"), list):
            continue
        sid, zone = doc.get("site_id"), doc.get("site_dns_zone")
        for row in doc["isitup_services"]:
            if not isinstance(row, dict) or (row.get("kind") or "ui") == "ui":
                continue
            fqdn = str(row.get("fqdn") or "")
            if sid:
                fqdn = fqdn.replace("{{ site_id }}", str(sid))
            if zone:
                fqdn = fqdn.replace("{{ site_dns_zone }}", str(zone))
            if fqdn and "{{" not in fqdn:
                doors[fqdn] = str(row.get("id") or "?")
    return doors


EXPECT_KINDS = {"exit-zero", "exit-nonzero", "contains"}

# A display `run:` may carry a placeholder — `goat admin peer ls --env
# <your-goatnet>` is exactly right on a page a person reads. An executable
# `check.run` may not: the runner shells it out verbatim, so a placeholder
# becomes a literal argument and the drill fails for a reason that has nothing
# to do with what it tests. The two fields look alike and that is the trap this
# catches, because the obvious way to write a check is to copy the `run:` line.
_PLACEHOLDER = re.compile(r"<[a-z][a-z0-9 _-]*>", re.I)


def check_drill_check(w: str, drill: dict, errors: list[str]) -> None:
    """Validate a drill's optional `check:` block.

    Absent is fine and is not a gap — most drills teach judgement and have no
    machine equivalent. Present and malformed is an error, because a check that
    silently does not run is worse than no check at all.
    """
    check = drill.get("check")
    if check is None:
        return
    if not isinstance(check, dict):
        errors.append(f"{w}: drill `check` must be a mapping")
        return

    run = check.get("run")
    if not run or not isinstance(run, str):
        errors.append(f"{w}: drill check has no `run` command")
    elif _PLACEHOLDER.search(run):
        errors.append(
            f"{w}: drill check `run` contains a placeholder "
            f"({_PLACEHOLDER.search(run).group(0)}) — that field is executed "
            f"verbatim. Put the illustrative form in the drill's `run:` field, "
            f"which is for display.")

    expect = check.get("expect", "exit-zero")
    if expect not in EXPECT_KINDS:
        errors.append(f"{w}: drill check `expect` must be one of {sorted(EXPECT_KINDS)}")
    elif expect == "contains" and not check.get("contains"):
        errors.append(f"{w}: drill check `expect: contains` needs a `contains` string")

    if not check.get("why"):
        errors.append(
            f"{w}: drill check has no `why` — state why this command stands in "
            f"for the drill, or the two drift apart and nobody notices")


def check_registry(reg: dict, errors: list[str]) -> dict:
    modules = {m["id"]: m for m in reg.get("modules", [])}
    lanes = reg.get("lanes", [])
    isitup = harvest_isitup_ids()
    covered: set[str] = set()
    written = 0
    quirks = 0

    laned: set[str] = set()
    for lane in lanes:
        for key in ("id", "title", "audience", "outcome", "modules"):
            if not lane.get(key):
                errors.append(f"lane {lane.get('id', '?')}: missing {key}")
        for mid in lane.get("modules", []):
            if mid not in modules:
                errors.append(f"lane {lane.get('id')}: unknown module {mid!r}")
            laned.add(mid)

    for mid, mod in modules.items():
        w = f"module {mid}"
        if mid not in laned:
            errors.append(f"{w}: in no lane — unreachable")
        if mod.get("level") not in LEVELS:
            errors.append(f"{w}: level must be one of {sorted(LEVELS)}")
        if not isinstance(mod.get("minutes"), int):
            errors.append(f"{w}: minutes must be an integer")
        status = mod.get("status")
        if status not in STATUS:
            errors.append(f"{w}: status must be one of {sorted(STATUS)}")
        # frontier: the tooling the module teaches is not built yet. Only a planned
        # module can be frontier territory; a written one teaches something that runs.
        if "frontier" in mod:
            if not isinstance(mod["frontier"], bool):
                errors.append(f"{w}: frontier must be true or false")
            elif mod["frontier"] and status != "planned":
                errors.append(f"{w}: frontier is for a planned module; a written module teaches something that runs")

        for item in mod.get("deeper") or []:
            doc = item.get("doc")
            if doc and not os.path.exists(os.path.join(REPO, doc)):
                errors.append(f"{w}: deeper document does not exist — {doc}")
            if not item.get("why"):
                errors.append(f"{w}: a `deeper` entry has no `why` — say what the reader gets there")

        for sid in mod.get("surface") or []:
            if sid not in isitup:
                errors.append(f"{w}: surface {sid!r} is declared by no goatnet")
            else:
                covered.add(sid)

        if status != "written":
            continue

        written += 1
        if not mod.get("summary"):
            errors.append(f"{w}: written with no summary")
        steps = mod.get("walkthrough") or []
        if len(steps) < 2:
            errors.append(f"{w}: written with fewer than two steps — a pointer is not a module")
        for i, step in enumerate(steps):
            if not step.get("do"):
                errors.append(f"{w}: step[{i}] has no `do`")
            if not (step.get("note") or step.get("run")):
                errors.append(f"{w}: step[{i}] has neither a note nor a command — it says nothing")

        qs = mod.get("quirks")
        if qs is None:
            errors.append(f"{w}: quirks key missing — use `quirks: []` to say none are recorded")
        else:
            for i, q in enumerate(qs):
                quirks += 1
                if not q.get("claim"):
                    errors.append(f"{w} quirk[{i}]: no claim")
                if not q.get("evidence"):
                    errors.append(f"{w} quirk[{i}]: no evidence — that makes it folklore")
                if not q.get("so"):
                    errors.append(f"{w} quirk[{i}]: no `so:` — a fact without an instruction "
                                  "leaves the reader where they started")

        drill = mod.get("drill")
        if not isinstance(drill, dict):
            errors.append(f"{w}: written with no drill")
        else:
            if not drill.get("do"):
                errors.append(f"{w}: drill has no `do`")
            if not drill.get("pass"):
                errors.append(f"{w}: drill has no pass condition")
            if drill.get("safety") not in SAFETY:
                errors.append(f"{w}: drill safety must be one of {sorted(SAFETY)}")
            check_drill_check(w, drill, errors)

    return {"modules": len(modules), "written": written, "lanes": len(lanes),
            "quirks": quirks, "services_live": len(isitup),
            "services_covered": len(covered),
            "services_uncovered": sorted(set(isitup) - covered)}


def check_site(site_dir: str, errors: list[str]) -> int:
    """No repo path may reach a reader on a page the GYM wrote.

    Scope matters here. The gym's own pages must never hand a reader a path —
    most readers have no repository, so a path is a dead end. The hosted
    project documents are a different case: they are somebody's engineering
    prose, and a path inside one is context the author wrote deliberately.
    Stripping those would mangle the document to satisfy a rule about
    navigation, so the library is out of scope and says so.
    """
    pages = 0
    doors = harvest_machine_doors()
    library = os.path.join(os.path.abspath(site_dir), "library")
    for dp, _, fs in os.walk(site_dir):
        in_library = os.path.abspath(dp).startswith(library)
        for f in fs:
            if not f.endswith(".html"):
                continue
            with open(os.path.join(dp, f), encoding="utf-8", errors="replace") as source_file:
                text = source_file.read()
            # The machine-door rule applies everywhere, library included: a
            # hosted document that links one is as dead an end as a gym page.
            for fqdn, sid in doors.items():
                if f'href="https://{fqdn}' in text:
                    errors.append(f"rendered page {f}: links {sid!r} at {fqdn} — that row is "
                                  "`kind: api`, a machine door with no web page behind it")
            if in_library:
                continue
            pages += 1
            for m in sorted(set(REPO_PATH.findall(text)))[:3]:
                errors.append(f"rendered page {f}: shows a repository path — {m}")
    return pages


def main() -> int:
    ap = argparse.ArgumentParser()
    ap.add_argument("--json", action="store_true")
    ap.add_argument("--registry", default=REGISTRY)
    ap.add_argument("--site", help="also check a rendered site for repo paths")
    args = ap.parse_args()

    with open(args.registry) as source_file:
        reg = yaml.safe_load(source_file)
    errors: list[str] = []
    stats = check_registry(reg, errors)

    pages = check_site(args.site, errors) if args.site else 0
    stats["pages_checked"] = pages
    stats["errors"] = errors

    if args.json:
        print(json.dumps(stats, indent=2))
    else:
        print(f"gym: {stats['written']} of {stats['modules']} modules written across "
              f"{stats['lanes']} lanes, {stats['quirks']} evidenced quirks")
        print(f"gym: isitup coverage {stats['services_covered']}/{stats['services_live']} "
              f"live service ids")
        if stats["services_uncovered"]:
            print("gym: NO MODULE COVERS — " + ", ".join(stats["services_uncovered"]))
        if pages:
            print(f"gym: {pages} rendered pages checked for repository paths")
        for e in errors:
            print(f"gym: REFUSED — {e}", file=sys.stderr)

    return 1 if errors else 0


if __name__ == "__main__":
    sys.exit(main())
