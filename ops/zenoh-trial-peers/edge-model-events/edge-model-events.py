#!/usr/bin/env python3
# SPDX-License-Identifier: Apache-2.0
"""edge-model-events — a worked example of a model at the edge publishing what it sees.

A camera on a factory line reads barcodes and QR codes and recognises objects.
Each thing it sees is published once, as it happens, as an event on the fabric
under the publisher's own slot, so a subscriber at any other site knows of it
at once.

The "model" here is a replay of a fixture of synthetic detections; nothing is
recognised. `detections()` is the one function a real detector replaces. The
rest is the part this example teaches:

- One stable key per source, `<slot>/edge/<source>/detections/v1`. What varies
  per event (kind, value, sequence number, time) rides in the payload, never in
  the key: a key minted per event grows the router's routing table without
  bound (pubsub-best-practices.md, "The stable-key contract").
- The key starts with the publisher's own slot (or that slot's alias). A
  publish outside the slot is denied silently, so `goat pub` returning 0 is not
  proof the event arrived; the subscriber is.
- Events go on a normal, stored prefix, not `volatile/`: a sighting is a record
  someone may need to replay. A `volatile/` key is routed and never stored or
  audited (zenoh-volatile-class.md).
- Each event is published with `goat pub`, which carries the identity and the
  certificate. One process per event suits a few events a second; a higher rate
  wants one Zenoh session with a declared publisher.

Run (prints what it would publish, touches nothing):
    python3 ops/zenoh-trial-peers/edge-model-events/edge-model-events.py \
        --slot <your-slot> --source line-3-camera --dry-run

Design context: goatcore docs/design/pubsub-best-practices.md,
docs/design/zenoh-volatile-class.md, docs/design/qos-throughput-envelopes.md.
"""
import argparse
import datetime
import json
import pathlib
import re
import shlex
import shutil
import subprocess
import sys
import time

HERE = pathlib.Path(__file__).parent
DEFAULT_FIXTURE = HERE / "fixtures" / "line-camera.jsonl"

KINDS = ("barcode", "qr", "object")

# A key segment the publisher chooses: lower-case letters, digits and hyphens.
# Zenoh reserves `*`, `$`, `?`, `#` and `/` splits segments, so a free-form
# name could turn the key into a wildcard or a different path.
SEGMENT = re.compile(r"^[a-z0-9][a-z0-9-]{0,62}$")

# A slot is a 32-hex-digit UUID anchor or an alias (ADR 1039). `goat pub`
# resolves an alias and refuses an unknown one; this check only keeps a
# placeholder or a path from reaching the wire.
SLOT = re.compile(r"^[A-Za-z0-9][A-Za-z0-9_-]{0,63}$")


class ExampleError(Exception):
    """A refusal with the instruction that fixes it."""


def event_key(slot, source):
    """The one key every event from this source is published on."""
    if not SLOT.match(slot or ""):
        raise ExampleError(
            f"--slot {slot!r} is not a slot: give the slot id or alias from your welcome "
            "package (letters, digits, '-' or '_'), never a placeholder or a path"
        )
    if not SEGMENT.match(source or ""):
        raise ExampleError(
            f"--source {source!r} is not a key segment: use lower-case letters, digits and "
            "hyphens, for example line-3-camera"
        )
    return f"{slot}/edge/{source}/detections/v1"


def detections(fixture):
    """Yield each detection from the fixture, in order.

    A real model replaces this function: yield one dict per thing seen, with
    `kind` (one of KINDS), `value` (what was read or recognised) and
    `confidence` (0 to 1). `after_s` is the replay delay before this
    detection, a property of the fixture only.
    """
    with open(fixture, encoding="utf-8") as fh:
        for lineno, line in enumerate(fh, 1):
            line = line.strip()
            if not line or line.startswith("#"):
                continue
            try:
                row = json.loads(line)
            except json.JSONDecodeError as exc:
                raise ExampleError(f"{fixture}:{lineno}: not JSON ({exc})") from exc
            if row.get("kind") not in KINDS:
                raise ExampleError(f"{fixture}:{lineno}: kind must be one of {', '.join(KINDS)}")
            if not isinstance(row.get("value"), str) or not row["value"]:
                raise ExampleError(f"{fixture}:{lineno}: value must be a non-empty string")
            conf = row.get("confidence")
            if not isinstance(conf, (int, float)) or not 0 <= conf <= 1:
                raise ExampleError(f"{fixture}:{lineno}: confidence must be a number from 0 to 1")
            yield row


def event_payload(detection, source, site, seq, now):
    """The event as published: everything that varies per event is here."""
    return {
        "source": source,
        "site": site,
        "seq": seq,
        "seen_at": now.strftime("%Y-%m-%dT%H:%M:%S.%f")[:-3] + "Z",
        "kind": detection["kind"],
        "value": detection["value"],
        "confidence": detection["confidence"],
    }


def pub_command(goat, key, payload):
    return [goat, "pub", key, "--encoding", "json",
            "--payload", json.dumps(payload, separators=(",", ":"), sort_keys=True)]


def run(args, runner=subprocess.run, sleep=time.sleep, clock=None, out=sys.stdout):
    clock = clock or (lambda: datetime.datetime.now(datetime.timezone.utc))
    key = event_key(args.slot, args.source)
    if args.site is not None and not SEGMENT.match(args.site):
        raise ExampleError(f"--site {args.site!r}: use lower-case letters, digits and hyphens")
    goat = args.goat
    if not args.dry_run and shutil.which(goat) is None:
        raise ExampleError(f"{goat} is not on PATH: install the goat CLI, or pass --goat <path>")

    print(f"publishing on {key}", file=out)
    sent = 0
    for seq, det in enumerate(detections(args.fixture), 1):
        if args.limit and sent >= args.limit:
            break
        if not args.dry_run and args.pace:
            sleep(float(det.get("after_s", 0)))
        payload = event_payload(det, args.source, args.site, seq, clock())
        cmd = pub_command(goat, key, payload)
        if args.dry_run:
            print("would run: " + shlex.join(cmd), file=out)
        else:
            result = runner(cmd)
            if result.returncode != 0:
                raise ExampleError(
                    f"goat pub exited {result.returncode} on event {seq}: read its message above. "
                    "A refusal names the cause; silence after exit 0 is checked at the subscriber"
                )
            print(f"seq {seq} {payload['kind']} {payload['value']}", file=out)
        sent += 1
    print(f"{sent} event(s) {'shown' if args.dry_run else 'published'} on {key}", file=out)
    return sent


def parse_args(argv):
    p = argparse.ArgumentParser(description=__doc__.split("\n")[0])
    p.add_argument("--slot", required=True, help="your slot id or alias, from your welcome package")
    p.add_argument("--source", required=True, help="this camera or model, as a key segment")
    p.add_argument("--site", help="the site the source sits at, carried in the payload")
    p.add_argument("--fixture", type=pathlib.Path, default=DEFAULT_FIXTURE,
                   help="detections to replay (default: the synthetic line camera)")
    p.add_argument("--limit", type=int, default=0, help="stop after this many events (0: all)")
    p.add_argument("--no-pace", dest="pace", action="store_false",
                   help="publish back to back instead of at the fixture's pace")
    p.add_argument("--goat", default="goat", help="the goat CLI to publish with")
    p.add_argument("--dry-run", action="store_true", help="print each publish, run nothing")
    return p.parse_args(argv)


def main(argv=None):
    try:
        run(parse_args(argv))
    except ExampleError as exc:
        print(f"edge-model-events: {exc}", file=sys.stderr)
        return 2
    return 0


if __name__ == "__main__":
    sys.exit(main())
