#!/usr/bin/env python3
"""goat gym — the Foundations catalogue gate, and its link verifier.

TWO JOBS, DELIBERATELY SEPARATE.

`check` runs everywhere, including CI, and is about SHAPE: every technology
names what it is, how we use it, where our use differs, at least one reading
with a reason, and at least one activity. A link with no reason is a bookmark,
and a bookmark is what this section exists instead of.

`--verify` reaches the internet and is therefore NOT part of any gate. It
records the date each URL last answered, in `verified.json`, and the site shows
that date. An upstream page that has moved is a fact about the world, not a
reason to fail somebody's build — and a check that fails when a third party
reorganises their documentation is a check people learn to ignore.

Usage:
    upstream_check.py                 # shape gate
    upstream_check.py --verify        # reach every URL, record the date
    upstream_check.py --report        # what is stale or unreachable
"""

from __future__ import annotations

import argparse
import json
import os
import subprocess
import sys

try:
    import yaml
except ImportError:  # pragma: no cover
    sys.exit("upstream_check.py needs PyYAML")

HERE = os.path.dirname(os.path.abspath(__file__))
CATALOGUE = os.path.join(HERE, "upstream.yaml")
VERIFIED = os.path.join(HERE, "upstream-verified.json")

KINDS = {"docs", "tutorial", "spec", "paper", "book", "video", "reference", "source"}
# Whether we actually run the thing. An entry that quietly implies production
# use is worse than no entry — a reader plans their learning around it.
STATUS = {"in-use", "partial", "not-deployed"}


def load() -> dict:
    with open(CATALOGUE) as fh:
        return yaml.safe_load(fh)


def check(cat: dict) -> list[str]:
    errors: list[str] = []
    cats = {c["id"] for c in cat.get("categories", [])}
    for c in cat.get("categories", []):
        for key in ("id", "title", "blurb"):
            if not c.get(key):
                errors.append(f"category {c.get('id', '?')}: missing {key}")

    seen: set[str] = set()
    for tech in cat.get("technologies", []):
        tid = tech.get("id", "?")
        if tid in seen:
            errors.append(f"technology {tid}: duplicate id")
        seen.add(tid)

        for key in ("name", "one_line", "what_it_is", "how_we_use_it", "differences"):
            if not tech.get(key):
                errors.append(f"technology {tid}: missing {key}")
        if tech.get("status") not in STATUS:
            errors.append(f"technology {tid}: status must be one of {sorted(STATUS)} — "
                          "say whether we actually run this")
        if tech.get("category") not in cats:
            errors.append(f"technology {tid}: unknown category {tech.get('category')!r}")

        reading = tech.get("reading") or []
        if not reading:
            errors.append(f"technology {tid}: no reading — then it is not a foundation, it is a note")
        for i, r in enumerate(reading):
            where = f"technology {tid} reading[{i}]"
            for key in ("title", "url", "kind", "why"):
                if not r.get(key):
                    errors.append(f"{where}: missing {key}")
            if r.get("kind") and r["kind"] not in KINDS:
                errors.append(f"{where}: kind must be one of {sorted(KINDS)}")
            if r.get("url", "").startswith("http") is False:
                errors.append(f"{where}: url must be absolute")
            if not isinstance(r.get("minutes"), int):
                errors.append(f"{where}: minutes must be an integer (0 = reference, dip in)")

        acts = tech.get("activities") or []
        if not acts:
            errors.append(f"technology {tid}: no activities — reading upstream docs is not knowing either")
        for i, a in enumerate(acts):
            if not a.get("do") or not a.get("gets"):
                errors.append(f"technology {tid} activity[{i}]: needs both `do` and `gets`")

    return errors


def verify(cat: dict) -> dict:
    """Reach every URL once. Records status, never fails a build."""
    out: dict[str, dict] = {}
    stamp = subprocess.run(["date", "-u", "+%Y-%m-%d"], capture_output=True,
                           text=True).stdout.strip()
    urls = [(t["id"], r["url"]) for t in cat.get("technologies", [])
            for r in (t.get("reading") or [])]
    for tid, url in urls:
        code = subprocess.run(
            ["curl", "-sSL", "-o", "/dev/null", "-w", "%{http_code}",
             "--max-time", "20", "-A", "goat-gym-link-check", url],
            capture_output=True, text=True).stdout.strip()
        ok = code.startswith("2") or code.startswith("3")
        out[url] = {"code": code, "ok": ok, "checked": stamp, "tech": tid}
        print(f"{'ok  ' if ok else 'DEAD'} {code:>3}  {url}")
    with open(VERIFIED, "w") as fh:
        json.dump(out, fh, indent=1, sort_keys=True)
    dead = [u for u, v in out.items() if not v["ok"]]
    print(f"\n{len(urls) - len(dead)}/{len(urls)} reachable, recorded {stamp}")
    for u in dead:
        print(f"  unreachable: {u}")
    return out


def main() -> int:
    ap = argparse.ArgumentParser()
    ap.add_argument("--verify", action="store_true")
    ap.add_argument("--report", action="store_true")
    args = ap.parse_args()
    cat = load()

    if args.verify:
        verify(cat)
        return 0

    if args.report:
        if not os.path.exists(VERIFIED):
            print("no verification on record — run --verify")
            return 0
        with open(VERIFIED) as source_file:
            data = json.load(source_file)
        dead = {u: v for u, v in data.items() if not v["ok"]}
        print(f"{len(data)} links, {len(dead)} unreachable at last check")
        for u, v in dead.items():
            print(f"  {v['code']}  {u}  ({v['tech']})")
        return 0

    errors = check(cat)
    techs = cat.get("technologies", [])
    links = sum(len(t.get("reading") or []) for t in techs)
    acts = sum(len(t.get("activities") or []) for t in techs)
    print(f"foundations: {len(techs)} technologies, {links} readings, {acts} activities")
    for e in errors:
        print(f"foundations: REFUSED — {e}", file=sys.stderr)
    return 1 if errors else 0


if __name__ == "__main__":
    sys.exit(main())
