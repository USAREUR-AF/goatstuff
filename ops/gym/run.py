#!/usr/bin/env python3
"""Run the gym drills that can be run by a machine.

WHY THIS EXISTS
---------------
`docs/design/goat-gym.md` records that no `goat gym` subcommand exists and that
`gym.json` is served so one could be added, and `docs/project/in-flight.md`
records the consequence: "nothing executes the drills, so a `do:` command that
stopped working is invisible". A trainee who hits a broken drill blames
themselves, which is the worst possible failure mode for a training surface.

WHAT THIS IS NOT
----------------
It is NOT a claim that the gym was already a test suite. A sweep
found that **all 82 drills carry prose `do:` fields and none is a command** —
"Open your welcome guide's Repositories page, find the example that matches what
you are about to build, and run its first scenario end to end." Most drills
teach judgement and have no machine equivalent, and pretending otherwise would
turn a good training surface into a bad test suite.

So this runner does not parse `do:`. It reads a separate, OPTIONAL `check:`
block that a module author adds only where a real command genuinely stands in
for the drill. A module with no `check:` is not a failure and is not a gap — it
is a drill for a person, reported as `human`.

THE SCHEMA
----------
    drill:
      do: <prose — unchanged, what a person does>
      pass: <prose — unchanged, how a person knows they passed>
      safety: read-only | writes | admin
      needs: mesh                      # optional
      check:                           # optional; this runner's input
        run: "python3 ops/warrant/generate.py --check"
        expect: exit-zero              # exit-zero | exit-nonzero | contains
        contains: "..."                # required when expect: contains
        why: >-
          One sentence on why this command stands in for the drill. Required,
          because a check whose relationship to its drill is not stated will
          drift away from it silently and nobody will notice.

SAFETY
------
Only `safety: read-only` drills run by default. `writes` and `admin` drills are
skipped unless explicitly opted into with --safety, because a training surface
whose exercises mutate production is a training surface people skip — and a
RUNNER that does it unasked is worse.

`needs: mesh` drills are skipped unless --mesh is passed. The runner does not
probe for mesh connectivity and then decide: an ambiguous "am I on the mesh"
heuristic that guesses wrong produces a false failure, which is the one result
that makes people stop trusting a check.

Usage:
    python3 ops/gym/run.py                       # read-only, no-mesh checks
    python3 ops/gym/run.py --mesh                # also drills needing a peer
    python3 ops/gym/run.py --safety read-only,admin
    python3 ops/gym/run.py --module warrant-generates-the-acl
    python3 ops/gym/run.py --list                # what would run, run nothing
    python3 ops/gym/run.py --json
"""

from __future__ import annotations

import argparse
import json
import os
import subprocess
import sys
import time

try:
    import yaml
except ImportError:
    print("ops/gym/run.py needs PyYAML (pip install pyyaml)", file=sys.stderr)
    raise SystemExit(2)

REPO_ROOT = os.path.abspath(os.path.join(os.path.dirname(os.path.abspath(__file__)), "..", ".."))
GYM_YAML = os.path.join(REPO_ROOT, "ops", "gym", "gym.yaml")

EXPECT_KINDS = ("exit-zero", "exit-nonzero", "contains")

# A drill is graded PASS / FAIL / SKIP / HUMAN. "HUMAN" is a first-class
# result, not a gap: it means the drill teaches something a command cannot
# stand in for. Reporting it as a gap would pressure authors into writing
# checks that do not really test the drill, which is worse than none.
PASS, FAIL, SKIP, HUMAN = "pass", "fail", "skip", "human"


def load_modules() -> list[dict]:
    with open(GYM_YAML, encoding="utf-8") as fh:
        return yaml.safe_load(fh).get("modules", [])


def grade(check: dict, rc: int, out: str) -> tuple[bool, str]:
    """Compare a completed run against its declared expectation."""
    expect = check.get("expect", "exit-zero")
    if expect == "exit-zero":
        return (rc == 0, f"exit {rc}")
    if expect == "exit-nonzero":
        return (rc != 0, f"exit {rc}")
    if expect == "contains":
        needle = check.get("contains", "")
        return (needle in out, f"exit {rc}; {'found' if needle in out else 'missing'} {needle!r}")
    return (False, f"unknown expect: {expect!r}")


def run_one(mod: dict, timeout: int) -> dict:
    drill = mod.get("drill") or {}
    check = drill.get("check")
    rec = {"module": mod["id"], "safety": drill.get("safety"),
           "needs": drill.get("needs"), "result": HUMAN,
           "detail": "", "seconds": 0.0}

    if not check:
        rec["detail"] = "no check declared — this drill is for a person"
        return rec

    started = time.monotonic()
    try:
        proc = subprocess.run(check["run"], shell=True, cwd=REPO_ROOT,
                              capture_output=True, text=True, timeout=timeout)
        rc, out = proc.returncode, (proc.stdout or "") + (proc.stderr or "")
    except subprocess.TimeoutExpired:
        rec.update(result=FAIL, detail=f"timed out after {timeout}s",
                   seconds=round(time.monotonic() - started, 2))
        return rec

    ok, detail = grade(check, rc, out)
    rec.update(result=PASS if ok else FAIL, detail=detail,
               seconds=round(time.monotonic() - started, 2))
    if not ok:
        # Only on failure, and trimmed — a runner that prints every command's
        # full output buries the one line that matters.
        tail = [ln for ln in out.strip().split("\n") if ln.strip()][-4:]
        rec["output_tail"] = tail
    return rec


def main() -> int:
    ap = argparse.ArgumentParser(description=__doc__.split("\n")[0])
    ap.add_argument("--mesh", action="store_true",
                    help="also run drills marked `needs: mesh`")
    ap.add_argument("--safety", default="read-only",
                    help="comma-separated safety classes to run (default: read-only)")
    ap.add_argument("--module", action="append", default=[],
                    help="run only this module id (repeatable)")
    ap.add_argument("--timeout", type=int, default=120,
                    help="per-check timeout in seconds (default 120)")
    ap.add_argument("--list", action="store_true",
                    help="show what would run; run nothing")
    ap.add_argument("--json", action="store_true", help="machine-readable output")
    args = ap.parse_args()

    allowed = {s.strip() for s in args.safety.split(",") if s.strip()}
    modules = load_modules()

    selected, skipped = [], []
    for mod in modules:
        drill = mod.get("drill") or {}
        if not drill:
            continue
        if args.module and mod["id"] not in args.module:
            continue
        if not drill.get("check"):
            continue
        if drill.get("safety") not in allowed:
            skipped.append((mod["id"], f"safety {drill.get('safety')} not in --safety"))
            continue
        if drill.get("needs") == "mesh" and not args.mesh:
            skipped.append((mod["id"], "needs mesh; pass --mesh"))
            continue
        selected.append(mod)

    if args.list:
        print(f"{len(selected)} check(s) would run:")
        for m in selected:
            print(f"  {m['id']:<34} {m['drill']['check']['run']}")
        if skipped:
            print(f"\n{len(skipped)} skipped:")
            for mid, why in skipped:
                print(f"  {mid:<34} {why}")
        return 0

    results = [run_one(m, args.timeout) for m in selected]
    for mid, why in skipped:
        results.append({"module": mid, "result": SKIP, "detail": why, "seconds": 0.0})

    if args.json:
        print(json.dumps(results, indent=2))
    else:
        width = max((len(r["module"]) for r in results), default=10)
        for r in results:
            mark = {PASS: "PASS", FAIL: "FAIL", SKIP: "skip", HUMAN: "human"}[r["result"]]
            print(f"  {mark:<5} {r['module']:<{width}}  {r['detail']}")
            for ln in r.get("output_tail", []):
                print(f"          | {ln}")

        counts = {k: sum(1 for r in results if r["result"] == k)
                  for k in (PASS, FAIL, SKIP)}
        human = sum(1 for m in modules
                    if (m.get("drill") or {}) and not (m.get("drill") or {}).get("check"))
        print(f"\n{counts[PASS]} passed, {counts[FAIL]} failed, {counts[SKIP]} skipped.")
        print(f"{human} of {sum(1 for m in modules if m.get('drill'))} drills have no "
              f"check and are for a person to run — that is a property of the drill, "
              f"not a gap in coverage.")

    return 1 if any(r["result"] == FAIL for r in results) else 0


if __name__ == "__main__":
    sys.exit(main())
