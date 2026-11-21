#!/usr/bin/env python3
# SPDX-License-Identifier: Apache-2.0
"""Compare the two CoT -> GeoTrackV1 paths, member by member.

Runs every CoT input the public tests use (`tests-public/test_cot.py`,
`test_cot_status.py`) through `goatmap.cot.parse` and through the
goat-transformers packs (`goatmap.cot_library`), and counts, per member of
GeoTrackV1:

  equal       same value and same JSON type
  form        same value, different form: a number written 65 for 65.0
  absent      the built-in path writes null, the packs write no member
  different   anything else

Every member that is not equal is counted under its cause (`by_cause`), and
every input the two paths disagree on is listed with its cause.

It reports two stages of the library path: what the packs alone produce
(`convert`), and the service's path with its completion step (`parse`).
`observed_at` is the receive clock; both paths are given the same instant.

  cot_parity.py            print the report as JSON
  cot_parity.py --check    compare with tests-public/cot_parity_baseline.json
  cot_parity.py --write    record the baseline
"""

from __future__ import annotations

import argparse
import json
import sys
from pathlib import Path

ROOT = Path(__file__).resolve().parents[1]
sys.path.insert(0, str(ROOT))
sys.path.insert(0, str(ROOT / "tests-public"))

from goatmap import cot, cot_library  # noqa: E402

BASELINE = ROOT / "tests-public" / "cot_parity_baseline.json"
CLOCK = "2030-01-05T12:00:00.000Z"
SOURCE = {"sensor": "radar-a", "site": "site-a", "node": "node-a"}
ANCHOR = "1bb8194d38f7ea26ef94e7b7efc5c907"


def corpus() -> list[tuple[str, bytes, str]]:
    """(name, payload, kind) for every CoT input the public tests use."""
    import test_cot
    import test_cot_status

    track = test_cot.TRACK
    out = [
        ("track", track, "track"),
        ("track-as-alert", track, "alert"),
        ("jamming-no-position", test_cot.JAMMING, "alert"),
        ("empty", b"", "track"),
        ("not-xml", b"not xml at all", "track"),
        ("unclosed", b"<event><point/>", "track"),
        ("other-root", b"<other/>", "track"),
        ("non-numeric-lat", track.replace(b'lat="0.144703641604"', b'lat="north-ish"'), "track"),
        ("unknown-type-letters", track.replace(b'type="a-u-X"', b'type="a-Z-Q"'), "track"),
        ("missing-type", track.replace(b'type="a-u-X" ', b""), "track"),
        ("no-detail-track",
         track.replace(b'<track course="65" speed="12.8682579750276"/>', b""), "track"),
        ("real-ce", track.replace(b'ce="9999999"', b'ce="25.5"'), "track"),
        ("status-alert", test_cot_status.ALERT, "alert"),
        ("status-track", test_cot_status.TRACK, "track"),
        ("status-no-droid",
         b'<event type="a-h-G-U-C-I" how="m-r"><point ce="9999999"/></event>', "alert"),
        ("truncated", b"<not-xml", "track"),
    ]
    out += [
        ("no-droid", track.replace(
            b'<uid Droid="radar/radar-a:Tracks with Associated Plot Data:177" tadilj="177"/>',
            b""), "track"),
        ("error-above-the-sentinel", track.replace(b'le="9999999"', b'le="10000000"'), "track"),
        ("type-one-token", track.replace(b'type="a-u-X"', b'type="a"'), "track"),
        ("time-without-fraction",
         track.replace(b'time="1970-01-01T02:47:21.682Z"', b'time="1970-01-01T02:47:21Z"'),
         "track"),
        ("time-not-an-instant",
         track.replace(b'time="1970-01-01T02:47:21.682Z"', b'time="soon"'), "track"),
        ("no-uid", track.replace(b' uid="177"', b""), "track"),
    ]
    for cot_type in ("a-f-A", "a-h-G-U-C-I", "a-n-S", "a-p-U"):
        out.append((f"type-{cot_type}",
                    track.replace(b'type="a-u-X"', f'type="{cot_type}"'.encode()), "track"))
    return out


# Why a member the packs produce is not equal to the built-in path's.
CAUSES = {
    "affiliation": "no function takes a token of the type; completed by complete_type_letters",
    "dimension": "no function takes a token of the type; completed by complete_type_letters",
    "ce_m": "a value above the 9999999 sentinel is kept; completed by complete_error_sentinel",
    "le_m": "a value above the 9999999 sentinel is kept; completed by complete_error_sentinel",
    "course_deg": "the json wire writes 65 where Python writes 65.0: one JSON number",
}
ABSENT = "the json wire writes no member for an absent key; completed by complete_null_members"
# Inputs one path accepts and the other refuses, and why.
DISAGREE = {
    "no-uid": "the built-in path publishes id `<sensor>:`, which the schema refuses; "
              "the packs refuse the event",
}


def grade(want, got: dict, member: str) -> str:
    if member not in got:
        return "absent" if want is None else "different"
    have = got[member]
    if have == want and type(have) is type(want):
        return "equal"
    if have == want and not isinstance(want, bool) and not isinstance(have, bool):
        return "form"
    return "different"


def measure() -> dict:
    cot.now_iso = lambda: CLOCK  # one clock for both paths
    stages = {"packs": {}, "service": {}}
    messages = {"inputs": 0, "both_refuse": 0, "both_accept": 0, "disagree": []}
    for stage in stages.values():
        stage.update(members=0, equal=0, form=0, absent=0, different=0, by_member={},
                     by_cause={})
    for name, payload, kind in corpus():
        messages["inputs"] += 1
        args = dict(kind=kind, source=SOURCE, producer_anchor=ANCHOR)
        builtin = cot.parse(payload, **args)
        context = {
            "geotrack.kind": kind, "geotrack.sensor": SOURCE["sensor"],
            "geotrack.site": SOURCE["site"], "geotrack.node": SOURCE["node"],
            "geotrack.observed_at": CLOCK, "geotrack.producer_anchor": ANCHOR,
            "geotrack.synthetic": 0,
        }
        results = {
            "packs": cot_library.convert(payload, context),
            "service": cot_library.parse(payload, **args),
        }
        if (builtin is None) != (results["service"] is None):
            messages["disagree"].append({"input": name, "cause": DISAGREE.get(name, "unexplained")})
            continue
        if builtin is None:
            messages["both_refuse"] += 1
            continue
        messages["both_accept"] += 1
        for stage_name, got in results.items():
            stage = stages[stage_name]
            for member, want in builtin.items():
                verdict = grade(want, got, member)
                stage["members"] += 1
                stage[verdict] += 1
                if verdict != "equal":
                    row = stage["by_member"].setdefault(member, {})
                    row[verdict] = row.get(verdict, 0) + 1
                    cause = ABSENT if verdict == "absent" else CAUSES.get(member, "unexplained")
                    stage["by_cause"][cause] = stage["by_cause"].get(cause, 0) + 1
    return {"messages": messages, **stages}


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__.splitlines()[0])
    parser.add_argument("--check", action="store_true")
    parser.add_argument("--write", action="store_true")
    args = parser.parse_args()
    report = measure()
    text = json.dumps(report, indent=2, sort_keys=True) + "\n"
    if args.write:
        BASELINE.write_text(text, encoding="utf-8")
    if args.check and json.loads(BASELINE.read_text(encoding="utf-8")) != report:
        sys.stderr.write("cot_parity: the result differs from the recorded baseline\n")
        sys.stdout.write(text)
        return 1
    if not args.check:
        sys.stdout.write(text)
    return 0


if __name__ == "__main__":
    sys.exit(main())
