#!/usr/bin/env python3
# SPDX-License-Identifier: Apache-2.0
"""Run every path-filtered workflow on main once a week, and file what is red.

    weekly-path-filtered.py list     [--workflows DIR]
    weekly-path-filtered.py dispatch [--workflows DIR] [--ref main] [--out runs.json]
    weekly-path-filtered.py report   --runs runs.json [--wait-minutes 300] [--open-issues]

A workflow with a `paths` or `paths-ignore` filter runs only when a change touches those
paths. When an upstream dependency moves (a package removed from a mirror, an image tag
withdrawn), nothing in the repository changes, so the job is not run and the breakage is
found by whichever unrelated pull request next touches the paths. This run finds it the
same week.

list: every workflow with a path filter, and whether it can be dispatched
(`workflow_dispatch`). A filtered workflow without it is listed as not covered.
dispatch: `gh workflow run <file> --ref <ref>` for each dispatchable one; records the
run each dispatch started (the newest workflow_dispatch run created after it).
report: waits for those runs to finish, prints each conclusion, and with --open-issues
opens one issue per red workflow ("Weekly path-filtered run: <file> is red on main"),
or comments on the one already open; a workflow green again closes its open issue.

Exit 1 when any run is red or did not finish, else 0.
"""
import argparse
import datetime as dt
import json
import os
import subprocess
import sys
import time

import yaml

TITLE = "Weekly path-filtered run: {wf} is red on main"


def gh(args, check=True):
    r = subprocess.run(["gh"] + args, capture_output=True, text=True)
    if check and r.returncode != 0:
        raise SystemExit(f"gh {' '.join(args[:3])}: {(r.stderr or r.stdout).strip()[:300]}")
    return r


def triggers(doc):
    # YAML reads a bare `on:` key as True.
    on = doc.get("on", doc.get(True))
    if isinstance(on, str):
        return {on: None}
    if isinstance(on, list):
        return {k: None for k in on}
    return on or {}


def classify(path):
    with open(path, encoding="utf-8") as fh:
        doc = yaml.safe_load(fh) or {}
    on = triggers(doc)
    filtered = any(isinstance(v, dict) and ("paths" in v or "paths-ignore" in v)
                   for k, v in on.items() if k in ("push", "pull_request", "pull_request_target"))
    return filtered, "workflow_dispatch" in on


def workflows(d):
    out = []
    for f in sorted(os.listdir(d)):
        if f.endswith((".yml", ".yaml")) and not f.startswith("_"):
            filtered, dispatchable = classify(os.path.join(d, f))
            if filtered:
                out.append((f, dispatchable))
    return out


def cmd_list(a):
    wfs = workflows(a.workflows)
    for f, disp in wfs:
        print(f"{'dispatch' if disp else 'NOT COVERED (no workflow_dispatch)'}  {f}")
    print(f"{sum(d for _, d in wfs)} of {len(wfs)} path-filtered workflows can be run on main")
    return 0


def newest_dispatch_run(wf, after):
    r = gh(["run", "list", "--workflow", wf, "--event", "workflow_dispatch", "--limit", "5",
            "--json", "databaseId,createdAt"], check=False)
    runs = json.loads(r.stdout or "[]") if r.returncode == 0 else []
    runs = [x for x in runs if x["createdAt"] >= after]
    return max(runs, key=lambda x: x["createdAt"])["databaseId"] if runs else None


def cmd_dispatch(a):
    started = {}
    for f, disp in workflows(a.workflows):
        if not disp:
            print(f"skip   {f}: no workflow_dispatch")
            continue
        after = dt.datetime.now(dt.timezone.utc).strftime("%Y-%m-%dT%H:%M:%SZ")
        r = gh(["workflow", "run", f, "--ref", a.ref], check=False)
        if r.returncode != 0:
            print(f"FAIL   {f}: could not dispatch: {r.stderr.strip()[:200]}")
            started[f] = None
            continue
        run_id = None
        for _ in range(12):
            time.sleep(a.poll_seconds)
            run_id = newest_dispatch_run(f, after)
            if run_id:
                break
        print(f"run    {f}: {run_id or 'started, run not found'}")
        started[f] = run_id
    with open(a.out, "w") as fh:
        json.dump(started, fh, indent=2, sort_keys=True)
    return 0


def find_issue(wf):
    title = TITLE.format(wf=wf)
    r = gh(["issue", "list", "--state", "open", "--search", f'"{title}" in:title', "--json", "number,title"], check=False)
    for i in json.loads(r.stdout or "[]") if r.returncode == 0 else []:
        if i["title"] == title:
            return i["number"]
    return None


def cmd_report(a):
    with open(a.runs, encoding="utf-8") as fh:
        runs = json.load(fh)
    deadline = time.time() + a.wait_minutes * 60
    result = {}
    pending = {wf: rid for wf, rid in runs.items() if rid}
    for wf, rid in runs.items():
        if not rid:
            result[wf] = ("not started", None)
    while pending and time.time() < deadline:
        for wf, rid in list(pending.items()):
            r = gh(["run", "view", str(rid), "--json", "status,conclusion,url"], check=False)
            v = json.loads(r.stdout or "{}") if r.returncode == 0 else {}
            if v.get("status") == "completed":
                result[wf] = (v.get("conclusion") or "unknown", v.get("url"))
                del pending[wf]
        if pending:
            time.sleep(a.poll_seconds)
    for wf, rid in pending.items():
        result[wf] = ("did not finish", f"run {rid}")
    red = 0
    for wf in sorted(result):
        concl, url = result[wf]
        ok = concl in ("success", "skipped", "neutral")
        red += 0 if ok else 1
        print(f"{'ok  ' if ok else 'RED '} {wf}: {concl} {url or ''}")
        if not a.open_issues:
            continue
        num = find_issue(wf)
        if ok and num:
            gh(["issue", "close", str(num), "--comment", f"Green again on main in the weekly run: {url}"], check=False)
        elif not ok:
            body = (f"`{wf}` is {concl} on `main` in the weekly path-filtered run ({url}). Nothing in a pull "
                    f"request touched its paths, so an upstream change (a dependency, a mirror, an image) is the "
                    f"likely cause. Fix it here, outside any pull request that happens to touch those paths.")
            if num:
                gh(["issue", "comment", str(num), "--body", body], check=False)
            else:
                gh(["issue", "create", "--title", TITLE.format(wf=wf), "--body", body], check=False)
    print(f"{len(result) - red} green, {red} red or unfinished, of {len(result)} runs")
    return 1 if red else 0


def main(argv=None):
    p = argparse.ArgumentParser(description=__doc__.splitlines()[0])
    sub = p.add_subparsers(dest="cmd", required=True)
    for n in ("list", "dispatch", "report"):
        s = sub.add_parser(n)
        s.add_argument("--workflows", default=".github/workflows")
        s.add_argument("--poll-seconds", type=float, default=10)
    sub.choices["dispatch"].add_argument("--ref", default="main")
    sub.choices["dispatch"].add_argument("--out", default="weekly-runs.json")
    sub.choices["report"].add_argument("--runs", required=True)
    sub.choices["report"].add_argument("--wait-minutes", type=float, default=300)
    sub.choices["report"].add_argument("--open-issues", action="store_true")
    a = p.parse_args(argv)
    return {"list": cmd_list, "dispatch": cmd_dispatch, "report": cmd_report}[a.cmd](a)


if __name__ == "__main__":
    sys.exit(main())
