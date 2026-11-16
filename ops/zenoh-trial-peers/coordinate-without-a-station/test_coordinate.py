#!/usr/bin/env python3
# SPDX-License-Identifier: Apache-2.0
"""Tests for the coordinate worked example.

The rule is a pure function, so it is tested exhaustively over every live set
drawn from a small roster: coverage, stability under join and leave, a task
covered twice but never by nobody in a partition, and a clean merge. The
Coordinator, which turns tokens and assignments into what one system covers,
is tested with events and no fabric.

One test runs the live code against real Zenoh sessions on the loopback
interface: one session in router mode stands in for the site router, and the
station and three systems connect to it as clients, with no certificates. It is skipped when
the Zenoh client library is not installed. It shows the glue works against the
library; it says nothing about a goatnet's router, ACL or certificates.

Run: python3 -m pytest -q ops/zenoh-trial-peers/coordinate-without-a-station
"""
import importlib.util
import io
import itertools
import json
import pathlib
import socket
import tempfile
import threading
import time
import unittest

HERE = pathlib.Path(__file__).parent
_spec = importlib.util.spec_from_file_location("coordinate", HERE / "coordinate.py")
co = importlib.util.module_from_spec(_spec)
_spec.loader.exec_module(co)

SLOTS = ["0" * 31 + str(i) for i in range(1, 7)]
ROSTER = [f"{s}/sys-{i}" for i, s in enumerate(SLOTS, 1)]
TASKS = [f"sector-{i}" for i in range(1, 10)]


def subsets(items, min_size=0):
    for n in range(min_size, len(items) + 1):
        yield from itertools.combinations(items, n)


class RuleTest(unittest.TestCase):
    def test_every_task_has_exactly_one_owner_in_every_live_set(self):
        for live in subsets(ROSTER, 1):
            plan = co.assignment(TASKS, list(live))
            self.assertEqual(set(plan), set(TASKS))
            self.assertTrue(set(plan.values()) <= set(live), live)
            covering = [t for m in live for t in co.covered_by(m, TASKS, list(live))]
            self.assertEqual(sorted(covering), sorted(TASKS), live)

    def test_no_live_member_means_no_owner(self):
        self.assertEqual(co.assignment(TASKS, []), {t: None for t in TASKS})

    def test_order_of_the_view_does_not_matter(self):
        for live in subsets(ROSTER[:5], 2):
            self.assertEqual(co.assignment(TASKS, list(live)),
                             co.assignment(TASKS, list(reversed(live))))

    def test_a_leave_moves_only_the_leavers_tasks(self):
        for live in subsets(ROSTER, 2):
            before = co.assignment(TASKS, list(live))
            for gone in live:
                after = co.assignment(TASKS, [m for m in live if m != gone])
                for t in TASKS:
                    if before[t] != gone:
                        self.assertEqual(after[t], before[t], (live, gone, t))

    def test_a_join_moves_tasks_only_to_the_joiner(self):
        for live in subsets(ROSTER, 1):
            before = co.assignment(TASKS, list(live))
            for new in set(ROSTER) - set(live):
                after = co.assignment(TASKS, list(live) + [new])
                for t in TASKS:
                    self.assertIn(after[t], (before[t], new), (live, new, t))

    def test_a_partition_covers_every_task_on_each_side(self):
        for live in subsets(ROSTER, 2):
            live = list(live)
            for k in range(1, len(live)):
                for side_a in itertools.combinations(live, k):
                    side_b = [m for m in live if m not in side_a]
                    for side in (list(side_a), side_b):
                        mine = [t for m in side for t in co.covered_by(m, TASKS, side)]
                        self.assertEqual(sorted(mine), sorted(TASKS))

    def test_a_partition_can_double_cover_but_never_leaves_a_gap(self):
        live = ROSTER[:4]
        side_a, side_b = live[:2], live[2:]
        cover = {t: 0 for t in TASKS}
        for side in (side_a, side_b):
            for m in side:
                for t in co.covered_by(m, TASKS, side):
                    cover[t] += 1
        self.assertTrue(all(n >= 1 for n in cover.values()))
        self.assertEqual(set(cover.values()), {2}, "each side covers everything, so twice each")

    def test_merge_returns_to_one_owner_each(self):
        for live in subsets(ROSTER, 2):
            live = list(live)
            side_a, side_b = live[: len(live) // 2], live[len(live) // 2:]
            # During the partition each side decides alone; on merge all see `live`.
            merged = [t for m in side_a + side_b for t in co.covered_by(m, TASKS, live)]
            self.assertEqual(sorted(merged), sorted(TASKS))
            self.assertEqual(co.assignment(TASKS, live),
                             co.assignment(TASKS, side_b + side_a))

    def test_station_plan_is_round_robin_over_the_sorted_live_set(self):
        plan = co.station_plan(["a", "b", "c"], ["s2/x", "s1/y"])
        self.assertEqual(plan, {"a": "s1/y", "b": "s2/x", "c": "s1/y"})
        self.assertEqual(co.station_plan(["a"], []), {})


class KeyTest(unittest.TestCase):
    def test_keys(self):
        s = SLOTS[0]
        self.assertEqual(co.system_token(s, "line-3", "alpha"),
                         f"{s}/presence/coord/line-3/systems/alpha")
        self.assertEqual(co.station_token(s, "line-3"), f"{s}/presence/coord/line-3/station")
        self.assertEqual(co.covering_key(s, "line-3", "alpha"), f"{s}/coord/line-3/alpha/covering/v1")
        self.assertEqual(co.group_watch("line-3"), "*/presence/coord/line-3/**")

    def test_parse_token(self):
        s = SLOTS[0]
        self.assertEqual(co.parse_token(co.system_token(s, "g", "alpha"), "g"), ("system", f"{s}/alpha"))
        self.assertEqual(co.parse_token(co.station_token(s, "g"), "g"), ("station", s))
        for key in (co.system_token(s, "other", "alpha"), f"{s}/presence/tarsier",
                    "presence/coord/g/station", f"{s}/presence/coord/g/systems/a/b", f"{s}/coord/g"):
            self.assertIsNone(co.parse_token(key, "g"), key)

    def test_tasks_and_segments_are_checked(self):
        self.assertEqual(co.parse_tasks("a, b,c"), ["a", "b", "c"])
        for bad in ("", "a,a", "A", "a/b", "a*"):
            with self.subTest(bad=bad), self.assertRaises(co.CoordError):
                co.parse_tasks(bad)


class CoordinatorTest(unittest.TestCase):
    def setUp(self):
        self.tasks = ["north", "east", "south", "west"]
        self.c = co.Coordinator(SLOTS[0], "alpha", "g", self.tasks)
        self.me = f"{SLOTS[0]}/alpha"

    def join(self, slot, system, alive=True):
        self.c.on_token(co.system_token(slot, "g", system), alive)

    def test_waits_until_it_sees_its_own_token(self):
        self.join(SLOTS[1], "bravo")
        self.assertEqual(self.c.decide(), ("waiting", []))
        self.join(SLOTS[0], "alpha")
        mode, mine = self.c.decide()
        self.assertEqual(mode, "self")
        self.assertEqual(mine, co.covered_by(self.me, self.tasks, sorted(self.c.live)))

    def test_alone_it_covers_everything(self):
        self.join(SLOTS[0], "alpha")
        self.assertEqual(self.c.decide(), ("self", sorted(self.tasks)))

    def test_follows_the_station_then_decides_alone_when_it_goes(self):
        self.join(SLOTS[0], "alpha")
        self.join(SLOTS[1], "bravo")
        bravo = f"{SLOTS[1]}/bravo"
        self.c.on_token(co.station_token(SLOTS[5], "g"), True)
        plan = {"north": self.me, "east": bravo, "south": self.me, "west": bravo}
        self.assertTrue(self.c.on_assignment(co.assignment_key(SLOTS[5], "g"), {"assignment": plan}))
        self.assertEqual(self.c.decide(), ("station", ["north", "south"]))
        self.c.on_token(co.station_token(SLOTS[5], "g"), False)
        mode, mine = self.c.decide()
        self.assertEqual(mode, "self")
        self.assertEqual(mine, co.covered_by(self.me, self.tasks, sorted([self.me, bravo])))

    def test_an_assignment_from_another_slot_is_ignored(self):
        self.join(SLOTS[0], "alpha")
        self.c.on_token(co.station_token(SLOTS[5], "g"), True)
        forged = {"assignment": {t: f"{SLOTS[3]}/x" for t in self.tasks}}
        self.assertFalse(self.c.on_assignment(co.assignment_key(SLOTS[4], "g"), forged))
        self.assertEqual(self.c.decide()[0], "self")

    def test_no_assignment_is_accepted_without_a_station_token(self):
        self.join(SLOTS[0], "alpha")
        self.assertFalse(self.c.on_assignment(co.assignment_key(SLOTS[5], "g"),
                                              {"assignment": {"north": self.me}}))

    def test_a_task_the_station_gave_to_a_gone_member_is_covered_by_the_rule(self):
        self.join(SLOTS[0], "alpha")
        self.c.on_token(co.station_token(SLOTS[5], "g"), True)
        ghost = f"{SLOTS[2]}/charlie"
        plan = {t: ghost for t in self.tasks}
        self.c.on_assignment(co.assignment_key(SLOTS[5], "g"), {"assignment": plan})
        self.assertEqual(self.c.decide(), ("station", sorted(self.tasks)))

    def test_every_task_stays_covered_across_station_loss_for_any_roster(self):
        names = [(SLOTS[i], f"s{i}") for i in range(4)]
        for live in subsets(names, 1):
            coords = []
            for slot, name in live:
                c = co.Coordinator(slot, name, "g", self.tasks)
                for s2, n2 in live:
                    c.on_token(co.system_token(s2, "g", n2), True)
                c.on_token(co.station_token(SLOTS[5], "g"), True)
                members = [f"{s}/{n}" for s, n in live]
                c.on_assignment(co.assignment_key(SLOTS[5], "g"),
                                {"assignment": co.station_plan(self.tasks, members)})
                coords.append(c)
            for station_alive in (True, False):
                if not station_alive:
                    for c in coords:
                        c.on_token(co.station_token(SLOTS[5], "g"), False)
                covered = sorted(t for c in coords for t in c.decide()[1])
                self.assertEqual(covered, sorted(self.tasks), (live, station_alive))

    def test_a_token_from_another_group_is_ignored(self):
        self.join(SLOTS[0], "alpha")
        self.c.on_token(co.system_token(SLOTS[1], "other", "bravo"), True)
        self.assertEqual(self.c.live, {self.me})


class FakeDecl:
    def __init__(self):
        self.undeclared = False

    def undeclare(self):
        self.undeclared = True


class DeniedTokenSession:
    """A session whose router denies the presence token: the call succeeds, nothing is seen."""

    def __init__(self):
        self.puts, self.decls = [], []

    def liveliness(self):
        return self

    def _decl(self, *args, **kwargs):
        d = FakeDecl()
        self.decls.append(d)
        return d

    declare_subscriber = _decl
    declare_token = _decl

    def put(self, key, payload, **kwargs):
        self.puts.append((key, payload))


class RefuseWithoutOwnTokenTest(unittest.TestCase):
    """Presence is best-effort advisory, so a system acts only on a token it has seen."""

    def test_a_system_that_never_sees_its_token_refuses_and_takes_no_task(self):
        session = DeniedTokenSession()
        with self.assertRaises(co.CoordError) as cm:
            co.run_system(session, SLOTS[0], "alpha", "g", ["north", "east"],
                          self_check=0.3, out=io.StringIO())
        msg = str(cm.exception)
        self.assertIn("never appeared", msg)
        self.assertIn("liveliness grant", msg)
        self.assertIn(co.system_token(SLOTS[0], "g", "alpha"), msg)
        self.assertEqual(session.puts, [], "no covering was published")
        self.assertTrue(all(d.undeclared for d in session.decls), "token and watches undeclared")

    def test_a_denied_token_leaves_no_task_uncovered(self):
        tasks = ["north", "east", "south", "west"]
        roster = [(SLOTS[0], "alpha"), (SLOTS[1], "bravo"), (SLOTS[2], "charlie")]
        denied = roster[2]
        coords = {}
        for slot, name in roster:
            c = co.Coordinator(slot, name, "g", tasks)
            for s2, n2 in roster:
                if (s2, n2) != denied:  # nobody, the denied system included, sees its token
                    c.on_token(co.system_token(s2, "g", n2), True)
            coords[name] = c
        self.assertEqual(coords["charlie"].decide(), ("waiting", []))
        cover = {t: 0 for t in tasks}
        for c in coords.values():
            for t in c.decide()[1]:
                cover[t] += 1
        self.assertEqual(cover, {t: 1 for t in tasks}, "every task covered, by the systems seen")

    def test_a_system_never_takes_a_task_before_its_token_is_seen_in_any_roster(self):
        tasks = ["north", "east", "south"]
        for others in subsets(ROSTER[1:4]):
            c = co.Coordinator(SLOTS[0], "sys-1", "g", tasks)
            for m in others:
                slot, name = m.split("/")
                c.on_token(co.system_token(slot, "g", name), True)
            c.on_token(co.station_token(SLOTS[5], "g"), True)
            c.on_assignment(co.assignment_key(SLOTS[5], "g"),
                            {"assignment": {t: f"{SLOTS[0]}/sys-1" for t in tasks}})
            self.assertEqual(c.decide(), ("waiting", []), others)


class CliTest(unittest.TestCase):
    def test_plan_prints_one_owner_per_task(self):
        out = io.StringIO()
        rc = co.main(["plan", "--tasks", "north,east", "--live", "s1/alpha,s2/bravo"], out=out)
        self.assertEqual(rc, 0)
        self.assertEqual(len(out.getvalue().splitlines()), 2)

    def test_bundle_must_hold_one_slot_cert_key_and_ca(self):
        with tempfile.TemporaryDirectory() as d:
            with self.assertRaises(co.CoordError):
                co.bundle_paths(d)
            slot = SLOTS[0]
            for name in ("goatnet-ca-root.pem", f"{slot}-cert.pem", f"{slot}-key.pem"):
                pathlib.Path(d, name).write_text("synthetic, not a certificate\n")
            self.assertEqual(co.bundle_paths(d)[0], slot)

    def test_refusal_exits_2(self):
        with tempfile.TemporaryDirectory() as d:
            self.assertEqual(co.main(["system", "--bundle", d, "--endpoint", "tls/192.0.2.10:7447",
                                      "--group", "g", "--system", "alpha", "--tasks", "a"]), 2)


def free_port():
    with socket.socket() as s:
        s.bind(("127.0.0.1", 0))
        return s.getsockname()[1]


@unittest.skipUnless(importlib.util.find_spec("zenoh"), "the Zenoh client library is not installed")
class LoopbackTest(unittest.TestCase):
    """Real Zenoh sessions on loopback: the station goes and the systems still cover everything."""

    def wait_for(self, predicate, timeout=20.0):
        end = time.monotonic() + timeout
        while time.monotonic() < end:
            if predicate():
                return True
            time.sleep(0.2)
        return False

    def test_station_loss_on_loopback(self):
        port = free_port()
        hub = co.open_session(None, mode="router", listen=f"tcp/127.0.0.1:{port}")
        sessions, stops, threads = [hub], [], []
        latest = {}

        def on_cover(sample):
            body = json.loads(bytes(sample.payload.to_bytes()))
            latest[body["system"]] = body

        watcher = hub.declare_subscriber("*/coord/g/*/covering/v1", on_cover)
        tasks = ["north", "east", "south", "west"]

        def start(target, *args):
            s = co.open_session(f"tcp/127.0.0.1:{port}", mode="client")
            sessions.append(s)
            stop = threading.Event()
            stops.append(stop)
            t = threading.Thread(target=target, args=(s, *args),
                                 kwargs={"heartbeat": 1.0, "stop": stop, "out": io.StringIO()},
                                 daemon=True)
            t.start()
            threads.append(t)
            return stop

        try:
            station_stop = start(co.run_station, SLOTS[5], "g", tasks)
            for i, name in enumerate(("alpha", "bravo", "charlie")):
                start(co.run_system, SLOTS[i], name, "g", tasks)

            def covered(mode):
                rows = [latest.get(n) for n in ("alpha", "bravo", "charlie")]
                if not all(r and r["mode"] == mode and len(r["live"]) == 3 for r in rows):
                    return False
                return sorted(t for r in rows for t in r["covering"]) == sorted(tasks)

            self.assertTrue(self.wait_for(lambda: covered("station")), latest)
            station_stop.set()
            self.assertTrue(self.wait_for(lambda: covered("self")), latest)
        finally:
            for stop in stops:
                stop.set()
            for t in threads:
                t.join(timeout=5)
            watcher.undeclare()
            for s in reversed(sessions):
                s.close()


if __name__ == "__main__":
    unittest.main()
