#!/usr/bin/env python3
# SPDX-License-Identifier: Apache-2.0
"""coordinate — a worked example of systems sharing tasks when their station is gone.

A station hands a small, fixed set of tasks (sectors to watch, say) to the
systems in reach. When the station disappears, the systems keep every task
covered with nothing in the middle: each one watches who is alive and runs the
same assignment over that live set, so they agree without exchanging a word.

What this example promises, and what it does not:

- It runs while the systems still share their site's Zenoh router. The routers
  are edge-local, so a site cut off from every other site keeps its local
  pub/sub (goatcore docs/design/zenoh-data-fabric.md, "The topology as
  deployed"). Losing the station, or the link to it, is what it handles.
- It does NOT run with no router in reach. Router-less peer continuity is
  designed, not built (goatcore ADR 1095); with no IP path at all the bearer is
  Reticulum (ADR 1050), whose field bearers are not built either.

The rule (rendezvous, or highest-random-weight, hashing):

- Each system declares a liveliness token at
  `<slot>/presence/coord/<group>/systems/<system>` (the presence convention in
  goatcore crates/panoscope-core/src/presence.rs) and watches the group.
- Each task goes to the live member with the highest sha256(task, member).
  Views that agree give the same answer. When a member leaves, only its tasks
  move; when one joins, only the tasks it now wins move to it.
- In a partition each side covers every task itself: a task may be covered
  twice, never by nobody. When the sides see each other again they recompute
  and the duplicates drop.
- While the station's token is alive and it has published an assignment, the
  systems follow it, except for any task it gave to a member that is gone,
  which falls back to the rule at once.

Each participant publishes only under its own slot, which the per-slot write
ACL makes an identity: a system accepts an assignment only from the slot that
holds the station token.

Offline, to see the rule:
    python3 coordinate.py plan --tasks north,east,south,west --live s1/alpha,s2/bravo,s3/charlie

Live (each participant with its own slot's bundle):
    python3 coordinate.py system --bundle <dir> --endpoint tls/<router>:7447 \
        --group line-3 --system alpha --tasks north,east,south,west
    python3 coordinate.py station --bundle <dir> --endpoint tls/<router>:7447 \
        --group line-3 --tasks north,east,south,west
"""
import argparse
import datetime
import glob
import hashlib
import json
import os
import re
import sys
import threading
import time

PRESENCE = "presence"
SEGMENT = re.compile(r"^[a-z0-9][a-z0-9-]{0,62}$")
ANCHOR = re.compile(r"^[0-9a-f]{32}$")


class CoordError(Exception):
    """A refusal with the instruction that fixes it."""


# ---------------------------------------------------------------------------
# The rule. Pure functions: no fabric, no clock.
# ---------------------------------------------------------------------------

def score(task, member):
    digest = hashlib.sha256(f"{task}\0{member}".encode()).digest()
    return int.from_bytes(digest, "big")


def owner(task, live):
    """The live member that covers `task`, or None when nobody is live."""
    if not live:
        return None
    return max(live, key=lambda m: (score(task, m), m))


def assignment(tasks, live):
    """task -> member for every task, by the rule, over one view of the live set."""
    return {t: owner(t, live) for t in tasks}


def covered_by(me, tasks, live):
    """The tasks `me` covers when its view of the live set is `live`."""
    return sorted(t for t in tasks if owner(t, live) == me)


def station_plan(tasks, live):
    """What the station hands out: round robin over the sorted live set.

    Deliberately a different policy from the rule, so a learner can see the
    systems switch between following the station and deciding for themselves.
    """
    members = sorted(live)
    if not members:
        return {}
    return {t: members[i % len(members)] for i, t in enumerate(tasks)}


# ---------------------------------------------------------------------------
# Keys.
# ---------------------------------------------------------------------------

def check_segment(flag, value):
    if not SEGMENT.match(value or ""):
        raise CoordError(f"{flag} {value!r}: use lower-case letters, digits and hyphens")
    return value


def parse_tasks(text):
    tasks = [t.strip() for t in (text or "").split(",") if t.strip()]
    if not tasks:
        raise CoordError("--tasks: give at least one task, comma-separated")
    for t in tasks:
        check_segment("--tasks entry", t)
    if len(set(tasks)) != len(tasks):
        raise CoordError("--tasks: a task is listed twice")
    return tasks


def system_token(slot, group, system):
    return f"{slot}/{PRESENCE}/coord/{group}/systems/{system}"


def station_token(slot, group):
    return f"{slot}/{PRESENCE}/coord/{group}/station"


def group_watch(group):
    return f"*/{PRESENCE}/coord/{group}/**"


def covering_key(slot, group, system):
    return f"{slot}/coord/{group}/{system}/covering/v1"


def assignment_key(slot, group):
    return f"{slot}/coord/{group}/assignment/v1"


def assignment_watch(group):
    return f"*/coord/{group}/assignment/v1"


def parse_token(key, group):
    """('system', member) / ('station', slot) / None for a key in the group watch.

    A member is `<slot>/<system>`: the slot is who holds the identity, the
    system name tells apart several systems a test might run under one slot.
    """
    chunks = key.split("/")
    if len(chunks) < 2 or PRESENCE not in chunks:
        return None
    i = chunks.index(PRESENCE)
    root, label = "/".join(chunks[:i]), chunks[i + 1:]
    if not root or label[:2] != ["coord", group]:
        return None
    rest = label[2:]
    if rest == ["station"]:
        return ("station", root)
    if len(rest) == 2 and rest[0] == "systems":
        return ("system", f"{root}/{rest[1]}")
    return None


# ---------------------------------------------------------------------------
# One system's view, driven by events. No fabric here either.
# ---------------------------------------------------------------------------

class Coordinator:
    """What one system covers, given the tokens and assignments it has seen."""

    def __init__(self, slot, system, group, tasks):
        self.me = f"{slot}/{system}"
        self.group = group
        self.tasks = list(tasks)
        self.live = set()
        self.station_root = None
        self.station_assignment = None

    @property
    def self_seen(self):
        # Presence is advisory: a router without the liveliness grant denies a
        # token silently. A system takes tasks only once it sees its own.
        return self.me in self.live

    def on_token(self, key, alive):
        parsed = parse_token(key, self.group)
        if parsed is None:
            return
        kind, ident = parsed
        if kind == "system":
            (self.live.add if alive else self.live.discard)(ident)
        elif alive:
            self.station_root = ident
        elif ident == self.station_root:
            self.station_root = None
            self.station_assignment = None

    def on_assignment(self, key, payload):
        root = key.split("/coord/", 1)[0]
        if self.station_root is None or root != self.station_root:
            return False  # only the slot holding the station token may assign
        plan = payload.get("assignment")
        if not isinstance(plan, dict):
            return False
        self.station_assignment = {t: m for t, m in plan.items() if t in self.tasks}
        return True

    def decide(self):
        """(mode, tasks): mode is waiting, station or self."""
        if not self.self_seen:
            return "waiting", []
        live = sorted(self.live)
        if self.station_root and self.station_assignment is not None:
            mine = []
            for t in self.tasks:
                given = self.station_assignment.get(t)
                if given in self.live:
                    if given == self.me:
                        mine.append(t)
                elif owner(t, live) == self.me:
                    mine.append(t)  # orphaned by the station: cover it by the rule
            return "station", sorted(mine)
        return "self", covered_by(self.me, self.tasks, live)


# ---------------------------------------------------------------------------
# The fabric. Only this part needs the Zenoh client library.
# ---------------------------------------------------------------------------

def now():
    return datetime.datetime.now(datetime.timezone.utc).strftime("%Y-%m-%dT%H:%M:%SZ")


def bundle_paths(bundle):
    ca = sorted(glob.glob(os.path.join(bundle, "*-ca-root.pem")))
    certs = sorted(glob.glob(os.path.join(bundle, "*-cert.pem")))
    if not ca:
        raise CoordError(f"no *-ca-root.pem in {bundle}: a slot bundle carries one")
    if len(certs) != 1:
        raise CoordError(f"{bundle}: expected exactly one <slot>-cert.pem, found {len(certs)}")
    slot = os.path.basename(certs[0])[: -len("-cert.pem")]
    if not ANCHOR.match(slot):
        raise CoordError(f"{certs[0]}: the file name does not start with a 32-hex slot anchor")
    key = os.path.join(bundle, f"{slot}-key.pem")
    if not os.path.exists(key):
        raise CoordError(f"{bundle}: {slot}-key.pem is missing")
    return slot, ca[0], certs[0], key


def open_session(endpoint, tls=None, mode="client", listen=None):
    import zenoh  # imported here so the rule and its tests need no client library
    conf = zenoh.Config()
    conf.insert_json5("mode", json.dumps(mode))
    conf.insert_json5("connect/endpoints", json.dumps([endpoint] if endpoint else []))
    if listen:
        conf.insert_json5("listen/endpoints", json.dumps([listen]))
    conf.insert_json5("scouting/multicast/enabled", "false")
    if tls:
        ca, cert, key = tls
        # enable_mtls is required: without it the client offers no certificate
        # and the router refuses with a bare "unable to connect".
        conf.insert_json5("transport/link/tls/enable_mtls", "true")
        conf.insert_json5("transport/link/tls/verify_name_on_connect", "false")
        conf.insert_json5("transport/link/tls/root_ca_certificate", json.dumps(ca))
        conf.insert_json5("transport/link/tls/connect_certificate", json.dumps(cert))
        conf.insert_json5("transport/link/tls/connect_private_key", json.dumps(key))
    return zenoh.open(conf)


def run_system(session, slot, system, group, tasks, heartbeat=5.0, self_check=10.0,
               stop=None, out=sys.stdout):
    import zenoh
    stop = stop or threading.Event()
    coord = Coordinator(slot, system, group, tasks)
    lock = threading.Lock()
    changed = threading.Event()

    def on_token(sample):
        with lock:
            coord.on_token(str(sample.key_expr), sample.kind == zenoh.SampleKind.PUT)
        changed.set()

    def on_assignment(sample):
        try:
            payload = json.loads(bytes(sample.payload.to_bytes()))
        except (ValueError, AttributeError):
            return
        with lock:
            coord.on_assignment(str(sample.key_expr), payload)
        changed.set()

    watch = session.liveliness().declare_subscriber(group_watch(group), on_token, history=True)
    assign_sub = session.declare_subscriber(assignment_watch(group), on_assignment)
    token = session.liveliness().declare_token(system_token(slot, group, system))
    pub_key = covering_key(slot, group, system)
    try:
        deadline = time.monotonic() + self_check
        while not stop.is_set():
            with lock:
                seen = coord.self_seen
            if seen:
                break
            if time.monotonic() > deadline:
                raise CoordError(
                    f"this system's own token {system_token(slot, group, system)} never appeared "
                    "in its watch: the router may lack the liveliness grant for this slot, which "
                    "denies a token silently. Not taking any task"
                )
            changed.wait(0.2)
            changed.clear()
        last, seq, next_beat = None, 0, 0.0
        while not stop.is_set():
            with lock:
                mode, mine = coord.decide()
                live = sorted(coord.live)
            t = time.monotonic()
            if (mode, mine) != last or t >= next_beat:
                seq += 1
                body = {"group": group, "system": system, "slot": slot, "mode": mode,
                        "covering": mine, "live": live, "seq": seq, "at": now()}
                session.put(pub_key, json.dumps(body, sort_keys=True), encoding="application/json")
                if (mode, mine) != last:
                    print(f"{body['at']} {mode:7} covering {','.join(mine) or '-'} "
                          f"(live: {len(live)})", file=out, flush=True)
                last, next_beat = (mode, mine), t + heartbeat
            changed.wait(0.5)
            changed.clear()
    finally:
        token.undeclare()
        assign_sub.undeclare()
        watch.undeclare()


def run_station(session, slot, group, tasks, heartbeat=5.0, stop=None, out=sys.stdout):
    import zenoh
    stop = stop or threading.Event()
    live, lock, changed = set(), threading.Lock(), threading.Event()

    def on_token(sample):
        parsed = parse_token(str(sample.key_expr), group)
        if parsed and parsed[0] == "system":
            with lock:
                (live.add if sample.kind == zenoh.SampleKind.PUT else live.discard)(parsed[1])
            changed.set()

    watch = session.liveliness().declare_subscriber(group_watch(group), on_token, history=True)
    token = session.liveliness().declare_token(station_token(slot, group))
    key = assignment_key(slot, group)
    try:
        last, next_beat = None, 0.0
        while not stop.is_set():
            with lock:
                plan = station_plan(tasks, live)
            t = time.monotonic()
            if plan != last or t >= next_beat:
                session.put(key, json.dumps({"group": group, "assignment": plan, "at": now()},
                                            sort_keys=True), encoding="application/json")
                if plan != last:
                    print(f"{now()} station assigns {json.dumps(plan, sort_keys=True)}",
                          file=out, flush=True)
                last, next_beat = plan, t + heartbeat
            changed.wait(0.5)
            changed.clear()
    finally:
        token.undeclare()
        watch.undeclare()


def parse_args(argv):
    p = argparse.ArgumentParser(description=__doc__.split("\n")[0])
    sub = p.add_subparsers(dest="role", required=True)
    plan = sub.add_parser("plan", help="print the rule's assignment for a live set; no fabric")
    plan.add_argument("--tasks", required=True)
    plan.add_argument("--live", required=True, help="comma-separated members, <slot>/<system>")
    for role in ("system", "station"):
        r = sub.add_parser(role)
        r.add_argument("--bundle", required=True, help="this slot's unpacked bundle: cert, key, CA root")
        r.add_argument("--endpoint", required=True, help="the site router, tls/<host>:7447")
        r.add_argument("--group", required=True)
        r.add_argument("--tasks", required=True)
        r.add_argument("--heartbeat", type=float, default=5.0)
        if role == "system":
            r.add_argument("--system", required=True, help="this system's name in the group")
            r.add_argument("--self-check", type=float, default=10.0,
                           help="seconds to wait for this system's own token to appear")
    return p.parse_args(argv)


def main(argv=None, out=sys.stdout):
    try:
        a = parse_args(argv)
        tasks = parse_tasks(a.tasks)
        if a.role == "plan":
            live = [m.strip() for m in a.live.split(",") if m.strip()]
            for t, m in assignment(tasks, live).items():
                print(f"{t:12} {m}", file=out)
            return 0
        check_segment("--group", a.group)
        slot, ca, cert, key = bundle_paths(a.bundle)
        session = open_session(a.endpoint, tls=(ca, cert, key))
        try:
            if a.role == "system":
                check_segment("--system", a.system)
                run_system(session, slot, a.system, a.group, tasks, a.heartbeat, a.self_check, out=out)
            else:
                run_station(session, slot, a.group, tasks, a.heartbeat, out=out)
        finally:
            session.close()
    except CoordError as exc:
        print(f"coordinate: {exc}", file=sys.stderr)
        return 2
    except KeyboardInterrupt:
        return 0
    return 0


if __name__ == "__main__":
    sys.exit(main())
