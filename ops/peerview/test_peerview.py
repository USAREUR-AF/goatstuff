#!/usr/bin/env python3
# SPDX-License-Identifier: Apache-2.0
#
# Unit tests for peerview-poll's derivations.
#
# WHAT IS TESTED HERE AND WHY. Not the HTTP calls — those are I/O and a mock of
# them tests the mock. What is tested is every place where a wrong answer would
# be PLAUSIBLE rather than obviously broken, because those are the ones that
# ship:
#
#   handle_of        must not merge two people into one (`unit-pl-008` and
#                    `unit-pl-500` must not both read as `unit`). A person count that
#                    silently merges people looks completely normal.
#   split_group      splitting on the first dash gives `SITE` / `PROD-PLATFORM-
#                    ADMIN` on a production goatnet — a scope that exists and
#                    a role that does not, on the goatnet where it matters most.
#   is_probe         four synthetic requests a minute per surface, forever. Miss
#                    them and every unused surface reads as evenly popular.
#   classify         the browse/core line is the whole contention question.
#   ledger folding   an unbounded append-only file on a mgmt host.
#   parse_ts         netbird emits both sub-microsecond precision and a zero
#                    timestamp for never-connected; one raises, the other is
#                    not a date.
#
# Run: python3 ops/peerview/test_peerview.py

import contextlib
import importlib.util
import io
import json
import os
import tempfile
import time
import unittest
from unittest.mock import patch

HERE = os.path.dirname(os.path.abspath(__file__))
REPO = os.path.abspath(os.path.join(HERE, "..", ".."))

_spec = importlib.util.spec_from_file_location(
    "peerview_poll", os.path.join(HERE, "peerview-poll.py"))
pv = importlib.util.module_from_spec(_spec)
_spec.loader.exec_module(pv)


class TestHandle(unittest.TestCase):
    """The person axis. Over-collapsing merges people; under-matching loses them."""

    def test_multi_token_handles_stay_distinct(self):
        # The regression. Two mint records, two people, two handles.
        self.assertEqual(pv.handle_of("peer-unit-pl-008-zagros-2030-01-11-BOX", "zagros"),
                         "unit-pl-008")
        self.assertEqual(pv.handle_of("peer-unit-pl-500-zagros-2030-01-11-BOX", "zagros"),
                         "unit-pl-500")

    def test_simple_handle(self):
        self.assertEqual(pv.handle_of("peer-someone-zagros-2030-01-12-ABC1234", "zagros"),
                         "someone")

    def test_hyphenated_goatnet_is_stripped_whole(self):
        self.assertEqual(pv.handle_of("peer-someone-zagros-prod-2026-08-05-X", "zagros-prod"),
                         "someone")

    def test_unknown_goatnet_keeps_env_rather_than_guessing(self):
        # Degrades to a longer key. Still unique per person, which is all the
        # grouping needs — it must never guess a token off the end.
        self.assertEqual(pv.handle_of("peer-someone-otherenv-2026-08-05-X", "zagros"),
                         "someone-otherenv")

    def test_machine_names_have_no_handle(self):
        for n in ("mgmt-01", "lab-node", "node-b",
                  "relay-health-a", "HOSTNAME", ""):
            self.assertIsNone(pv.handle_of(n, "zagros"), n)


class TestGroupSplit(unittest.TestCase):
    """Scope and role, resolved against the declared taxonomy."""

    @classmethod
    def setUpClass(cls):
        # An invented taxonomy of the real file's shape, so these do not depend
        # on which scopes one instance declares.
        with tempfile.NamedTemporaryFile("w", suffix=".yml", delete=False) as f:
            f.write(
                "netbird_acl_taxonomy:\n"
                "  zagros: {site_prefix: ZAGROS, groups: [{suffix: DEVELOPER}, {suffix: PLATFORM-ADMIN}]}\n"
                "  zagros-prod: {site_prefix: ZAGROS-PROD, groups: [{suffix: PLATFORM-ADMIN}]}\n"
                "netbird_exercises:\n"
                "  site-a: {site_prefix: SITE-A, host_site: zagros-prod}\n"
                "  ex-demo: {site_prefix: EX-DEMO, host_site: zagros}\n"
            )
        cls.tax = pv.load_taxonomy(f.name)
        os.unlink(f.name)

    def test_taxonomy_loads_both_top_level_keys(self):
        # netbird_acl_taxonomy AND netbird_exercises. Reading only the first
        # loses a forward site, which is where the gear is.
        self.assertTrue(self.tax["ok"])
        for p in ("ZAGROS", "ZAGROS-PROD", "SITE-A", "EX-DEMO"):
            self.assertIn(p, self.tax["prefixes"], p)

    def test_the_committed_taxonomy_loads(self):
        real = pv.load_taxonomy(
            os.path.join(REPO, "ops", "ansible", "netbird_acl_taxonomy.yml"))
        self.assertTrue(real["ok"])

    def test_longest_prefix_wins(self):
        # The whole reason prefixes are sorted by length.
        self.assertEqual(pv.split_group("ZAGROS-PROD-PLATFORM-ADMIN", self.tax),
                         ("ZAGROS-PROD", "PLATFORM-ADMIN"))
        self.assertEqual(pv.split_group("ZAGROS-DEVELOPER", self.tax),
                         ("ZAGROS", "DEVELOPER"))

    def test_forward_site_scope(self):
        self.assertEqual(pv.split_group("SITE-A-P2P", self.tax), ("SITE-A", "P2P"))

    def test_universal_group_is_neither(self):
        self.assertEqual(pv.split_group("All", self.tax), (None, None))

    def test_unscoped_group_keeps_its_name(self):
        # `mesh-dns` is a real group with no scope. It must not be forced into
        # one, and it must not vanish.
        self.assertEqual(pv.split_group("mesh-dns", self.tax), (None, "mesh-dns"))


class TestOsFamily(unittest.TestCase):
    """A peer can report an OS that is present and blank.

    `os: " "` is truthy, so it passes the `or "unknown"` guard, and splitting it
    raises IndexError inside the census loop — the service dies and both reading
    surfaces freeze. One field, one peer, the whole surface.
    """

    def test_a_blank_os_is_unknown_not_a_crash(self):
        for raw in (" ", "   ", "\t", "\n"):
            self.assertEqual(pv.os_family(raw), ("unknown", "unknown"), repr(raw))

    def test_absent_and_empty_read_the_same_as_blank(self):
        for raw in (None, ""):
            self.assertEqual(pv.os_family(raw), ("unknown", "unknown"), repr(raw))

    def test_a_real_os_keeps_its_full_string_and_first_token_family(self):
        self.assertEqual(pv.os_family("Ubuntu 24.04"), ("Ubuntu 24.04", "Ubuntu"))
        self.assertEqual(pv.os_family("Alpine Linux 3.24.1"), ("Alpine Linux 3.24.1", "Alpine"))
        # Surrounding whitespace is noise, not part of the name.
        self.assertEqual(pv.os_family("  Debian GNU/Linux 13 "), ("Debian GNU/Linux 13", "Debian"))

    def test_the_literal_unknown_stays_unknown(self):
        self.assertEqual(pv.os_family("unknown"), ("unknown", "unknown"))


class TestClassification(unittest.TestCase):

    @classmethod
    def setUpClass(cls):
        cls.classes, cls.why = pv.load_classes(
            os.path.join(HERE, "peerview-service-classes.yaml"))

    def test_every_class_declares_why(self):
        # A class with no stated reason is a class nobody can argue with.
        for k in self.classes["classes"]:
            self.assertTrue(self.why.get(k), k)

    def test_the_browse_core_line(self):
        self.assertEqual(pv.classify("signal", self.classes), "core")
        self.assertEqual(pv.classify("mgmt", self.classes), "core")
        self.assertEqual(pv.classify("panoscope", self.classes), "browse")
        self.assertEqual(pv.classify("goat-map", self.classes), "browse")
        self.assertEqual(pv.classify("grafana", self.classes), "admin")
        self.assertEqual(pv.classify("goat-get", self.classes), "artifact")

    def test_unmapped_is_named_not_guessed(self):
        self.assertEqual(pv.classify("something-new", self.classes), "unclassified")

    def test_per_goatnet_isitup_names_both_match(self):
        # The isitup service carries a goatnet suffix that differs per goatnet.
        for n in ("isitup-zagros", "isitup-zagros-prod", "isitup"):
            self.assertEqual(pv.classify(n, self.classes), "browse", n)

    def test_probe_paths(self):
        self.assertTrue(pv.is_probe("/healthz", self.classes))
        self.assertTrue(pv.is_probe("/healthz?x=1", self.classes))
        self.assertTrue(pv.is_probe("/-/healthy", self.classes))
        self.assertFalse(pv.is_probe("/api/snapshot?window_hours=6", self.classes))
        self.assertFalse(pv.is_probe("/", self.classes))
        self.assertFalse(pv.is_probe(None, self.classes))

    def test_service_name_normalisation_matches_the_prometheus_relabel(self):
        self.assertEqual(pv.norm_service("nova-portal@file"), "portal")
        self.assertEqual(pv.norm_service("isitup-zagros@file"), "isitup-zagros")
        self.assertEqual(pv.norm_service("netvis@file"), "netvis")
        self.assertIsNone(pv.norm_service(None))


class TestTimestamps(unittest.TestCase):

    def test_never_connected_is_none_not_epoch(self):
        # netbird's zero value. Parsed as a date it would report a peer last
        # seen in the year 1, i.e. maximally dormant, which is the opposite of
        # "we have no reading".
        self.assertIsNone(pv.parse_ts("0001-01-01T00:00:00Z"))
        self.assertIsNone(pv.parse_ts(""))
        self.assertIsNone(pv.parse_ts(None))

    def test_sub_microsecond_precision_parses(self):
        # netbird emits 7 fractional digits; fromisoformat rejects them.
        self.assertIsNotNone(pv.parse_ts("2026-08-14T21:12:13.0203431Z"))
        self.assertIsNotNone(pv.parse_ts("2026-08-14T21:12:13.020343Z"))
        self.assertIsNotNone(pv.parse_ts("2026-08-14T21:12:13Z"))


class TestLedger(unittest.TestCase):
    """Bounded, on a mgmt host, forever."""

    def test_folding_keeps_the_file_bounded(self):
        now = time.time()
        with tempfile.TemporaryDirectory() as d:
            path = os.path.join(d, "ledger.jsonl")
            # 120 days of five-minute rows would be ~34,000 raw rows.
            rows = []
            for i in range(0, 120 * 24 * 12):
                rows.append({"t": int(now - i * 300), "n": 100, "c": 40,
                             "rq": 1.0, "at": 10, "ps": 3, "res": "raw"})
            with open(path, "w") as f:
                for r in sorted(rows, key=lambda r: r["t"]):
                    f.write(json.dumps(r) + "\n")
            out = pv.update_ledger(path, pv.ledger_row(now, {"ok": False}, None), now)
            self.assertLessEqual(len(out), pv.LEDGER_MAX_ROWS)
            # And the oldest history survives as daily rows rather than being
            # dropped: a bounded file that keeps only the last day would make
            # the month-over-month panel permanently empty.
            self.assertTrue(any(r.get("res") == "day" for r in out))
            self.assertGreater((now - min(r["t"] for r in out)) / 86400.0, 60)

    def test_cycle_reports_its_own_sample_size(self):
        now = time.time()
        rows = [{"t": int(now - i * 900), "n": 100, "c": 40 + (i % 5), "rq": 2.0}
                for i in range(4 * 6)]          # six hours only
        cy = pv.build_cycle(rows, now)
        self.assertLess(cy["days"], 1)
        covered = [h for h in cy["hour_of_day"] if h["samples"] > 0]
        self.assertLess(len(covered), 24)       # the page hatches the rest


class TestDemand(unittest.TestCase):
    """The join, the probe filter, and the contention test's refusal to guess."""

    @classmethod
    def setUpClass(cls):
        cls.classes, _ = pv.load_classes(
            os.path.join(HERE, "peerview-service-classes.yaml"))
        cls.peers = [{"name": "peer-a-zagros-2026-08-01-X", "ip": "100.127.0.41",
                      "role": "DEVELOPER", "scope": "ZAGROS", "os": "Ubuntu 24.04",
                      "version": "0.76.3", "handle": "a"}]

    def _row(self, ip, svc, path="/", dur_ms=5.0, down=1000, minute="2026-08-15T10:00"):
        return {"ClientHost": ip, "ServiceName": svc, "RequestPath": path,
                "Duration": dur_ms * 1e6, "DownstreamContentSize": down,
                "RequestContentSize": 0, "DownstreamStatus": 200,
                "StartUTC": minute + ":00.000Z"}

    def test_probes_are_removed_before_anything_is_counted(self):
        rows = ([self._row("172.18.0.1", "netvis@file", "/healthz")] * 40 +
                [self._row("100.127.0.41", "nova-panoscope@file", "/api/stats")] * 3)
        d = pv.build_demand(rows, self.peers, self.classes, 300, False)
        self.assertEqual(d["requests_probe"], 40)
        self.assertEqual(d["requests_total"], 3)
        # netvis was probed forty times and used zero times. It must not appear.
        self.assertNotIn("netvis", [s["name"] for s in d["services"]])

    def test_the_mesh_ip_join(self):
        rows = ([self._row("100.127.0.41", "nova-panoscope@file", "/x")] * 5 +
                [self._row("172.18.0.1", "signal@file", "/y")] * 95)
        d = pv.build_demand(rows, self.peers, self.classes, 300, False)
        self.assertEqual(d["requests_attributable"], 5)
        self.assertEqual(d["requests_anonymous"], 95)
        self.assertAlmostEqual(d["attributable_share"], 0.05)
        self.assertEqual(d["peers"][0]["name"], "peer-a-zagros-2026-08-01-X")
        self.assertEqual(d["peers"][0]["role"], "DEVELOPER")

    def test_top_service_is_the_peers_own_busiest_not_the_goatnets(self):
        # The peer touched panoscope once and the map four times, while signal
        # dominates the goatnet. Its row must say goat-map.
        rows = ([self._row("100.127.0.41", "nova-panoscope@file", "/x")] * 1 +
                [self._row("100.127.0.41", "goat-map@file", "/y")] * 4 +
                [self._row("172.18.0.1", "signal@file", "/z")] * 900)
        d = pv.build_demand(rows, self.peers, self.classes, 300, False)
        self.assertEqual(d["peers"][0]["top_service"], "goat-map")

    def test_stream_durations_never_reach_a_percentile(self):
        # A held-open signal stream must not count as an 11-minute "response
        # time" and reach the contention verdict.
        # Requests and bytes must still be counted; only the duration goes.
        rows = ([self._row("172.18.0.1", "signal@file", "/z", dur_ms=675_687.0)] * 50 +
                [self._row("172.18.0.1", "mgmt@file", "/m", dur_ms=4.0)] * 50)
        d = pv.build_demand(rows, self.peers, self.classes, 300, False)
        svc = {s["name"]: s for s in d["services"]}
        self.assertEqual(svc["signal"]["requests"], 50)       # counted
        self.assertIsNone(svc["signal"]["p95_ms"])            # not timed
        self.assertIsNone(svc["signal"]["mean_ms"])
        self.assertEqual(svc["mgmt"]["p95_ms"], 4.0)          # a real response time
        core = next(c for c in d["by_class"] if c["name"] == "core")
        self.assertEqual(core["requests"], 100)               # both counted
        self.assertEqual(core["p95_ms"], 4.0)                 # only one timed

    def test_contention_refuses_a_thin_sample(self):
        rows = [self._row("172.18.0.1", "mgmt@file", "/z",
                          minute="2026-08-15T10:0%d" % i) for i in range(3)]
        d = pv.build_demand(rows, self.peers, self.classes, 300, False)
        self.assertFalse(d["contention"]["testable"])
        self.assertIn("too few", d["contention"]["reason"])

    def test_contention_refuses_a_lopsided_sample(self):
        # Comparing hundreds of timed core requests against a handful is not a
        # comparison. Enough minutes, nowhere near enough on one side.
        rows = []
        for i in range(9):
            m = "2026-08-15T10:%02d" % i
            for _ in range(i * 4):
                rows.append(self._row("100.127.0.41", "nova-panoscope@file", "/x", minute=m))
            # Every minute clears MIN_CORE_PER_MINUTE so every minute is
            # rankable — otherwise the quiet minutes are dropped before the
            # thirds are cut and the earlier "too few minutes" guard answers
            # instead, which is a different refusal than the one under test.
            for _ in range(60 if i >= 6 else 3):   # 3 == MIN_CORE_PER_MINUTE
                rows.append(self._row("172.18.0.1", "mgmt@file", "/z", minute=m))
        d = pv.build_demand(rows, self.peers, self.classes, 540, False)
        self.assertFalse(d["contention"]["testable"])
        self.assertIn("each side", d["contention"]["reason"])

    def test_contention_computes_over_enough_minutes(self):
        rows = []
        for i in range(9):
            m = "2026-08-15T10:%02d" % i
            # busy browsing in the later minutes, and core slower there too
            for _ in range(i * 4):
                rows.append(self._row("100.127.0.41", "nova-panoscope@file", "/x", minute=m))
            for _ in range(40):
                rows.append(self._row("172.18.0.1", "mgmt@file", "/z",
                                      dur_ms=1.0 + i, minute=m))
        d = pv.build_demand(rows, self.peers, self.classes, 540, False)
        ct = d["contention"]
        self.assertTrue(ct["testable"], ct.get("reason"))
        self.assertGreater(ct["busy_browse_rpm"], ct["quiet_browse_rpm"])
        self.assertGreater(ct["busy_core_p95_ms"], ct["quiet_core_p95_ms"])

    def test_no_backend_records_are_their_own_bucket(self):
        rows = [self._row("172.18.0.1", None, "/x")] * 7
        d = pv.build_demand(rows, self.peers, self.classes, 300, False)
        classes = {c["name"]: c["requests"] for c in d["by_class"]}
        self.assertEqual(classes.get("no-backend"), 7)
        self.assertEqual(d["unclassified_services"], [])


class TestMinuteHistory(unittest.TestCase):
    """Persisted contention samples expire using the producer's UTC minute keys."""

    def setUp(self):
        tmp = tempfile.TemporaryDirectory()
        self.addCleanup(tmp.cleanup)
        self.path = os.path.join(tmp.name, "minutes.json")
        self.now = pv.parse_ts("2026-10-27T12:00:00Z")

    def minute(self, age_minutes):
        return time.strftime("%Y-%m-%dT%H:%M", time.gmtime(self.now - age_minutes * 60))

    def save(self, buckets):
        with open(self.path, "w") as fh:
            json.dump(buckets, fh)

    def stored(self):
        with open(self.path) as fh:
            return json.load(fh)

    def merge(self, browse=None, core_ms=None):
        with patch.object(pv.time, "time", return_value=self.now):
            return pv._merge_minute_history(
                self.path, {} if browse is None else browse, {} if core_ms is None else core_ms)

    def test_production_iso_history_expires_from_demand_and_disk(self):
        rows = []
        for i in range(6):
            for service, count in (("core@file", 30), ("browse@file", i * 10)):
                rows.extend([{"ClientHost": "192.0.2.10", "ServiceName": service,
                              "Duration": (i + 1) * 1e6, "DownstreamStatus": 200,
                              "StartUTC": self.minute(5 - i) + ":00.000Z"}] * count)
        classes = {"classes": {"core": ["core"], "browse": ["browse"]}, "probe_paths": []}
        with patch.object(pv.time, "time", return_value=self.now):
            first = pv.build_demand(rows, [], classes, 300, False, self.path)
        self.assertTrue(first["contention"]["testable"])
        self.assertEqual(len(self.stored()), 6)
        with patch.object(pv.time, "time", return_value=self.now + 86400):
            expired = pv.build_demand([], [], classes, 300, False, self.path)
        self.assertEqual(expired["requests_total"], 0)
        self.assertFalse(expired["contention"]["testable"])
        self.assertIn("only 0 minute(s)", expired["contention"]["reason"])
        self.assertEqual(self.stored(), {})

    def test_six_hour_boundary_is_inclusive(self):
        fresh, boundary, expired = self.minute(1), self.minute(360), self.minute(361)
        self.save({m: {"b": 1, "c": [2]} for m in (fresh, boundary, expired)})
        browse, core_ms = self.merge()
        self.assertEqual(set(browse), {fresh, boundary})
        self.assertEqual(set(core_ms), {fresh, boundary})
        self.assertEqual(set(self.stored()), {fresh, boundary})

    @unittest.skipUnless(hasattr(time, "tzset"), "requires process timezone selection")
    def test_minute_keys_are_utc_independent_of_host_timezone(self):
        for zone in ("UTC0", "EST5EDT"):
            with self.subTest(zone=zone):
                self.save({self.minute(360): {"b": 1, "c": [2]},
                           self.minute(361): {"b": 3, "c": [4]}})
                try:
                    with patch.dict(os.environ, {"TZ": zone}):
                        time.tzset()
                        browse, _ = self.merge()
                finally:
                    time.tzset()
                self.assertEqual(set(browse), {self.minute(360)})

    def test_numeric_epoch_second_keys_still_expire(self):
        fresh = str(int(self.now) - 60)
        expired = str(int(self.now) - pv.MINUTE_HISTORY_S - 60)
        self.save({m: {"b": 1, "c": [2]} for m in (fresh, expired)})
        browse, core_ms = self.merge()
        self.assertEqual(browse, {fresh: 1})
        self.assertEqual(core_ms, {fresh: [2]})
        self.assertEqual(set(self.stored()), {fresh})

    def test_malformed_keys_are_discarded_with_a_diagnostic(self):
        fresh = self.minute(1)
        bad = ("", "not-a-minute", "2026-99-27T12:00")
        self.save({m: {"b": 1, "c": [2]} for m in (*bad, fresh)})
        with contextlib.redirect_stderr(io.StringIO()) as stderr:
            browse, core_ms = self.merge()
        self.assertEqual(set(browse), {fresh})
        self.assertEqual(set(core_ms), {fresh})
        self.assertEqual(set(self.stored()), {fresh})
        self.assertIn("invalid minute", stderr.getvalue())

    def test_overlapping_windows_replace_samples_and_keep_fresh_history(self):
        current, previous = self.minute(0), self.minute(1)
        self.save({current: {"b": 9, "c": [99]}, previous: {"b": 2, "c": [3]}})
        samples = list(range(pv.MINUTE_SAMPLE_CAP + 10))
        browse, core_ms = self.merge({current: 4}, {current: samples})
        self.assertEqual(browse, {current: 4, previous: 2})
        self.assertEqual(core_ms[previous], [3])
        self.assertEqual(core_ms[current], samples)
        self.assertEqual(self.stored()[current], {"b": 4, "c": samples[:pv.MINUTE_SAMPLE_CAP]})

    def test_missing_history_preserves_current_window(self):
        current = self.minute(0)
        browse, core_ms = self.merge({current: 4}, {current: [5]})
        self.assertEqual(browse, {current: 4})
        self.assertEqual(core_ms, {current: [5]})
        self.assertEqual(self.stored(), {current: {"b": 4, "c": [5]}})

    def test_unreadable_history_preserves_current_window(self):
        with open(self.path, "w") as fh:
            fh.write("invalid json")
        current = self.minute(0)
        with contextlib.redirect_stderr(io.StringIO()) as stderr:
            browse, core_ms = self.merge({current: 4}, {current: [5]})
        self.assertEqual(browse, {current: 4})
        self.assertEqual(core_ms, {current: [5]})
        self.assertIn("minute history unreadable", stderr.getvalue())

    def test_unwritable_history_preserves_current_window(self):
        self.path = os.path.join(self.path, "missing-parent.json")
        current = self.minute(0)
        with contextlib.redirect_stderr(io.StringIO()) as stderr:
            browse, core_ms = self.merge({current: 4}, {current: [5]})
        self.assertEqual(browse, {current: 4})
        self.assertEqual(core_ms, {current: [5]})
        self.assertIn("could not persist minute history", stderr.getvalue())

    def test_disabled_history_keeps_single_window_behavior(self):
        old = self.minute(361)
        browse, core_ms = {old: 4}, {old: [5]}
        self.assertEqual(pv._merge_minute_history(None, browse, core_ms), (browse, core_ms))


class TestCost(unittest.TestCase):

    def test_baseline_divides_by_connected_not_enrolled(self):
        census = {"ok": True, "peers_connected": 40, "peers_total": 400}
        demand = {"by_class": [{"name": "core", "rps": 40.0, "bps": 4_000_000},
                               {"name": "browse", "rps": 1.0, "bps": 100_000}],
                  "peers_seen": 4}
        c = pv.build_cost(census, demand, {}, {})
        self.assertEqual(c["baseline_bps_per_peer"], 100_000)   # 4M / 40 connected
        self.assertEqual(c["active_bps_per_active_peer"], 25_000)  # 100k / 4 active

    def test_absent_source_yields_null_not_zero(self):
        c = pv.build_cost({"ok": False}, None, {}, {})
        for k in ("baseline_bps", "baseline_bps_per_peer", "baseline_share",
                  "active_bps", "active_bps_per_active_peer"):
            self.assertIsNone(c[k], k)


class TestBelongsTo(unittest.TestCase):
    """peerview reads the peer-name grammar in Python; the grammar's generated
    implementations are Go and Rust. There is no Python emitter, so the only
    thing keeping this reading honest is the SoT's own conformance vectors —
    read from ops/identity/peer-name-grammar.yaml, not copied into this file,
    so a vector added there is enforced here on the next run."""

    VECTORS = os.path.join(REPO, "ops", "identity", "peer-name-grammar.yaml")

    @unittest.skipUnless(os.path.exists(VECTORS), "the grammar SoT is not in this tree")
    def test_sot_belongs_to_vectors(self):
        cases = self._vectors()
        self.assertGreaterEqual(len(cases), 8, "the SoT vector list shrank")
        for c in cases:
            with self.subTest(why=c.get("why", "")[:60]):
                self.assertEqual(
                    pv.belongs_to(c["name"], c["handle"], c["org_short_tag"], c["site"]),
                    c["want"], c["name"])

    def _vectors(self):
        """Parse the belongs_to vector block without a yaml dependency.

        The poller itself parses its taxonomy the same hand-rolled way (no
        PyYAML on the mgmt hosts), so this stays consistent with the runtime
        rather than introducing a test-only dependency.
        """
        cases, cur, inside = [], None, False
        with open(self.VECTORS) as fh:
            lines = fh.readlines()
        for raw in lines:
            line = raw.rstrip("\n")
            if line.startswith("  belongs_to:"):
                inside = True
                continue
            if inside and line and not line.startswith(" "):
                break
            if not inside:
                continue
            if line.startswith("    - "):
                if cur:
                    cases.append(cur)
                cur = {"org_short_tag": "", "why": ""}
                line = "      " + line[6:]
            if cur is None:
                continue
            for key, cast in (("name", str), ("handle", str),
                              ("org_short_tag", str), ("site", str),
                              ("want", bool)):
                pre = f"      {key}: "
                if line.startswith(pre):
                    v = line[len(pre):].strip().strip('"')
                    cur[key] = (v == "true") if cast is bool else v
            if line.startswith("      why:"):
                # A vector block ends at the next `- name:`; `why` is last and
                # may be a folded scalar, which nothing here needs to read.
                cur.setdefault("why", "")
        if cur:
            cases.append(cur)
        return [c for c in cases if "name" in c and "want" in c]


class TestOwner(unittest.TestCase):
    """The owner join. Every failure mode here is a plausible-looking lie: an
    owner asserted for a handle nobody declared, a machine reported ownerless
    when its custodian is recorded, or a whole fleet reported ownerless because
    the registry file was missing on the host."""

    ROSTER = {
        "generated_at": "2026-08-11T10:08:51Z",
        "orgs": [
            {"id": "example-org", "members": [
                {"handle": "a-person", "type": "person", "state": "active",
                 "custody": "—", "vendor_tag": None},
                {"handle": "a-machine", "type": "system", "state": "active",
                 "custody": "a-person", "vendor_tag": None},
                {"handle": "orphan-machine", "type": "system", "state": "active",
                 "custody": "—", "vendor_tag": None},
                {"handle": "tagged", "type": "person", "state": "active",
                 "custody": "—", "vendor_tag": "acme"},
            ]},
            {"id": "other-org", "members": [
                # The same handle re-declared per-org / per-env, departed here.
                {"handle": "a-person", "type": "person", "state": "departed",
                 "custody": "—", "vendor_tag": None},
            ]},
        ],
    }

    def _roster(self, doc=None):
        with tempfile.TemporaryDirectory() as d:
            p = os.path.join(d, "roster.json")
            with open(p, "w") as f:
                json.dump(self.ROSTER if doc is None else doc, f)
            return pv.load_roster(p)

    def _own(self, name, roster=None, site="zagros", declared=None, portal=None):
        """Resolve with the resolver UNASKED unless a portal row is given —
        i.e. exercising the local fallback, which is what most of these pin."""
        return pv.owner_of(name, pv.handle_of(name, site),
                           roster if roster is not None else self._roster(), site,
                           declared, portal)

    def test_the_shared_resolver_answers_first(self):
        # A registry answer carries the live Ory id and the console deep-link,
        # and the local roster is not consulted for it.
        o = self._own("peer-a-person-zagros-2026-06-10-mac", portal={
            "attribution": "registry", "handle": "a-person", "org": "example-org",
            "status": "active", "ory_id": "abc-123",
            "id_console_path": "/users/abc-123", "reason": None})
        self.assertEqual(o["kind"], "registry")
        self.assertEqual(o["handle"], "a-person")
        self.assertEqual(o["ory_id"], "abc-123")
        self.assertEqual(o["source"], "portal-resolver")

    def test_a_lead_is_never_graded_as_a_record(self):
        # `name` from the resolver: the roster names the handle, no live
        # identity matched. It must not read like a confirmed person.
        o = self._own("peer-a-person-zagros-2026-06-10-mac", portal={
            "attribution": "name", "handle": "a-person", "org": "example-org",
            "status": "active", "ory_id": None, "id_console_path": None,
            "reason": None})
        self.assertEqual(o["kind"], "name")
        self.assertIsNone(o["ory_id"])
        self.assertIn("lead", o["why"])

    def test_local_fallback_can_never_claim_the_recorded_grade(self):
        # The resolver could not be asked. The same roster is on disk, so the
        # handle still resolves — but `registry` requires reconciling against a
        # LIVE identity, which only the resolver does.
        o = self._own("peer-a-person-zagros-2026-06-10-mac",
                      portal={"attribution": "unread", "handle": None,
                              "reason": "portal unreachable"})
        self.assertEqual(o["kind"], "name")
        self.assertEqual(o["handle"], "a-person")
        self.assertEqual(o["source"], "local-roster")

    def test_person_resolves_to_themselves(self):
        o = self._own("peer-a-person-zagros-2026-06-10-mac")
        self.assertEqual(o["kind"], "name")
        self.assertEqual(o["handle"], "a-person")

    def test_machine_resolves_to_its_custodian(self):
        # The whole reason the join exists: a peer that is a thing still has a
        # person answerable for it, and it is not the thing's own handle.
        o = self._own("peer-a-machine-zagros-2026-06-10-box")
        self.assertEqual(o["kind"], "custody")
        self.assertEqual(o["handle"], "a-person")
        self.assertEqual(o["custody_of"] or o["claimed"], "a-machine")

    def test_a_resolver_answer_for_a_machine_is_still_upgraded_to_its_custodian(self):
        # The gap this surface fills in the shared resolver: it attributes a
        # machine's peer to the MACHINE's handle, which cannot answer for
        # anything. Even a `registry`-grade answer gets re-pointed.
        o = self._own("peer-a-machine-zagros-2026-06-10-box", portal={
            "attribution": "registry", "handle": "a-machine", "org": "example-org",
            "status": "active", "ory_id": "m-1", "id_console_path": "/users/m-1",
            "reason": None})
        self.assertEqual(o["kind"], "custody")
        self.assertEqual(o["handle"], "a-person")

    def test_machine_without_custodian_is_unresolved_not_self_owned(self):
        o = self._own("peer-orphan-machine-zagros-2026-06-10-box")
        self.assertEqual(o["kind"], "unmatched")
        self.assertIsNone(o["handle"])

    def test_a_resolver_custody_answer_names_the_custodian(self):
        # A machine the portal enrolled lives in the registry, not in this
        # host's roster file, so only the resolver can name its custodian.
        o = self._own("machine-box-1-box-1", portal={
            "attribution": "custody", "handle": "a-person", "org": "example-org",
            "status": "active", "ory_id": "p-1", "id_console_path": "/users/p-1",
            "reason": None, "custody_of": "box-1"})
        self.assertEqual(o["kind"], "custody")
        self.assertEqual(o["handle"], "a-person")
        self.assertEqual(o["custody_of"], "box-1")
        self.assertEqual(o["source"], "portal-resolver")

    def test_a_machine_name_resolves_through_the_local_roster(self):
        # Resolver unreadable: a declared system still names its custodian when
        # its peer carries the portal's `machine-<handle>-<hostname>` form.
        o = self._own("machine-a-machine-box")
        self.assertEqual(o["kind"], "custody")
        self.assertEqual(o["handle"], "a-person")

    def test_unmatched_says_what_kind_of_name_it_was(self):
        # Three different jobs share the unmatched kind: a roster catch-up, a
        # machine with no custodian found, and an owner nobody declared.
        self.assertEqual(
            self._own("peer-nobody-declared-this-zagros-2026-06-10-mac")["shape"], "person")
        self.assertEqual(self._own("machine-unknown-box")["shape"], "machine")
        self.assertEqual(self._own("mgmt-01")["shape"], "none")

    def test_an_unmatched_machine_carries_the_resolvers_reason(self):
        o = self._own("machine-box-1-box-1", portal={
            "attribution": "unmatched", "handle": None, "custody_of": "box-1",
            "reason": "machine box-1 is enrolled, but its custodian matches no "
                      "declared roster member"})
        self.assertEqual(o["kind"], "unmatched")
        self.assertIn("custodian matches no declared roster member", o["why"])

    def test_org_tagged_name_resolves(self):
        o = self._own("peer-tagged-acme-zagros-2026-06-10-mac")
        self.assertEqual(o["kind"], "name")
        self.assertEqual(o["handle"], "tagged")

    def test_undeclared_handle_is_never_asserted_as_an_owner(self):
        # The failure that would look completely normal on the page: a handle
        # parsed out of a peer name, rendered as if it were a declared identity.
        o = self._own("peer-nobody-declared-this-zagros-2026-06-10-mac")
        self.assertEqual(o["kind"], "unmatched")
        self.assertIsNone(o["handle"])
        self.assertEqual(o["claimed"], "nobody-declared-this")

    def test_a_date_suffix_does_not_cost_a_peer_its_owner(self):
        # A second bundle the same day mints `…-2026-07-08b-…`. The loose
        # date-anchored read finds no handle in that at all, so before the
        # grammar matcher this peer read as an unowned machine.
        name = "peer-a-person-zagros-2026-07-08b-Garnet"
        self.assertIsNone(pv.handle_of(name, "zagros"))
        self.assertEqual(self._own(name)["handle"], "a-person")

    def test_another_goatnets_peer_is_not_claimed(self):
        o = self._own("peer-a-person-nova-2026-06-10-mac")
        self.assertEqual(o["kind"], "unmatched")

    def _declared(self, text):
        with tempfile.TemporaryDirectory() as d:
            p = os.path.join(d, "declared-owners.txt")
            with open(p, "w") as f:
                f.write(text)
            return pv.load_declared_owners(p)

    def test_declared_owner_covers_a_peer_with_no_onboarding_name(self):
        dec = self._declared("# substrate\nmgmt-*: a-person\n")
        o = self._own("mgmt-01", declared=dec)
        self.assertEqual(o["kind"], "declared")
        self.assertEqual(o["handle"], "a-person")
        self.assertIsNone(o["why"])

    def test_declared_owner_never_overrides_the_registry(self):
        # The rule that keeps a config file from reassigning somebody's laptop.
        dec = self._declared("peer-*: someone-else\n")
        o = self._own("peer-a-person-zagros-2026-06-10-mac", declared=dec)
        self.assertEqual(o["kind"], "name")
        self.assertEqual(o["handle"], "a-person")

    def test_longest_declared_pattern_wins(self):
        dec = self._declared("mgmt-*: class-owner\nmgmt-01: host-owner\n")
        self.assertEqual(self._own("mgmt-01", declared=dec)["handle"],
                         "host-owner")

    def test_declared_owner_outside_the_registry_says_so(self):
        dec = self._declared("mgmt-01: nobody-declared-this\n")
        o = self._own("mgmt-01", declared=dec)
        self.assertEqual(o["kind"], "declared")
        self.assertIn("not itself a declared identity", o["why"])

    def test_infrastructure_answers_to_its_scope_admins(self):
        # The mgmt host has no person in its name and never will, but "nobody
        # is answerable for the mgmt host" is false. The mesh already says who.
        o = pv.owner_of("mgmt-01", None, self._roster(), "zagros", None, None,
                        "INFRA", "ZAGROS", {"ZAGROS-PLATFORM-ADMIN", "ZAGROS-INFRA"})
        self.assertEqual(o["kind"], "team")
        self.assertEqual(o["handle"], "ZAGROS-PLATFORM-ADMIN")
        self.assertEqual(o["source"], "mesh-role")

    def test_a_breakglass_node_is_the_platform_admins(self):
        # A rescue node must not read as unowned. The taxonomy makes it
        # operator-enrolled and non-delegable (ADR 1114), so the scope's
        # platform admins are answerable by construction.
        o = pv.owner_of("lab-node", None, self._roster(), "zagros-prod", None, None,
                        "BREAKGLASS", "ZAGROS-PROD", {"ZAGROS-PROD-PLATFORM-ADMIN"})
        self.assertEqual(o["kind"], "team")
        self.assertEqual(o["handle"], "ZAGROS-PROD-PLATFORM-ADMIN")

    def test_delegable_network_gear_is_not_the_platform_admins(self):
        # Bearer gear is driven by a site-scoped network_admin entitlement
        # (ADR 1113); naming the platform admins would manufacture an answer.
        for role in ("NETWORK", "SENSOR", "EFFECTOR-CUE", "KID", "POD", "CONTROL"):
            o = pv.owner_of("shell-svn-001", None, self._roster(), "zagros-prod",
                            None, None, role, "ZAGROS-PROD", {"ZAGROS-PROD-PLATFORM-ADMIN"})
            self.assertEqual(o["kind"], "unmatched", role)

    def test_the_team_is_the_peers_own_scope_not_the_host_goatnets(self):
        o = pv.owner_of("some-exercise-box", None, self._roster(), "zagros", None, None,
                        "EXTERNAL-INFRA", "EX-DEMO",
                        {"EX-DEMO-PLATFORM-ADMIN", "ZAGROS-PLATFORM-ADMIN"})
        self.assertEqual(o["handle"], "EX-DEMO-PLATFORM-ADMIN")

    def test_a_developer_box_is_never_handed_to_the_admin_team(self):
        # The live shape this guards: a peer with no principal in its name but
        # a non-infra role. Attributing those to the platform admins would
        # manufacture an owner. The role is read as an authorization fact only —
        # it is never evidence of what the machine is or does.
        for role in ("DEVELOPER", "OBSERVER", "MOON", "QUARANTINED"):
            o = pv.owner_of("RONIN", None, self._roster(), "zagros", None, None,
                            role, "ZAGROS", {"ZAGROS-PLATFORM-ADMIN"})
            self.assertEqual(o["kind"], "unmatched", role)

    def test_a_team_group_that_does_not_exist_is_not_named(self):
        # Same rule as everywhere else here: no plausible-looking owner that
        # points at nothing.
        o = pv.owner_of("mgmt-01", None, self._roster(), "zagros", None, None,
                        "INFRA", "ZAGROS", {"ZAGROS-INFRA"})
        self.assertEqual(o["kind"], "unmatched")

    def test_an_unappointed_admin_group_still_answers(self):
        # An empty scope-local PLATFORM-ADMIN is the normal state at standup —
        # it means nobody has been appointed, not that the group is wrong.
        # Membership is not consulted; existence is.
        o = pv.owner_of("mgmt-01", None, self._roster(), "zagros", None, None,
                        "INFRA", "ZAGROS", {"ZAGROS-PLATFORM-ADMIN"})
        self.assertEqual(o["kind"], "team")

    def test_a_person_beats_the_team_tier(self):
        o = pv.owner_of("peer-a-person-zagros-2026-06-10-mac", "a-person",
                        self._roster(), "zagros", None, None,
                        "INFRA", "ZAGROS", {"ZAGROS-PLATFORM-ADMIN"})
        self.assertEqual(o["kind"], "name")
        self.assertEqual(o["handle"], "a-person")

    def test_asked_and_could_not_ask_are_different_facts(self):
        # `unmatched` is a statement about the peer; `unread` is a statement
        # about this host. Collapsing them reports a failure to measure as a
        # measurement.
        self.assertEqual(self._own("mgmt-01")["kind"], "unmatched")
        missing = pv.load_roster("/nonexistent/roster.json")
        self.assertFalse(missing["ok"])
        self.assertEqual(self._own("peer-a-person-zagros-2026-06-10-mac", missing)["kind"],
                         "unread")
        self.assertEqual(self._own("mgmt-01", missing)["kind"], "unread")

    def test_active_declaration_wins_over_departed_duplicate(self):
        r = self._roster()
        self.assertEqual(r["by_handle"]["a-person"]["state"], "active")
        self.assertEqual(r["repeated_handles"], 1)
        self.assertEqual(r["member_count"], 4)

    def test_roster_age_is_graded_not_left_as_a_date(self):
        # The shape this guards: a roster a week behind the repository, with the
        # date printed verbatim — unalarming, and live people read "not in roster"
        # behind it.
        import time as _t
        now = _t.time()
        day = 86400.0
        iso = lambda ago: _t.strftime("%Y-%m-%dT%H:%M:%SZ", _t.gmtime(now - ago))
        # 7.2 days is a stale copy many people behind the portal's on the same
        # host. It must not read `fresh`.
        for ago_days, want in ((0.5, "fresh"), (2.9, "fresh"), (4, "ageing"),
                               (7.2, "ageing"), (30, "stale")):
            age, grade = pv.roster_freshness(iso(ago_days * day), now)
            self.assertEqual(grade, want, f"{ago_days}d")
            self.assertAlmostEqual(age, ago_days, delta=0.1)

    def test_an_unstamped_roster_is_unknown_not_old(self):
        # An artifact predating the provenance stamp is UNMEASURED. Grading it
        # `stale` would invent a fact; grading it `fresh` would hide one.
        for stamp in (None, "", "not-a-date"):
            age, grade = pv.roster_freshness(stamp)
            self.assertEqual(grade, "unknown", repr(stamp))
            self.assertIsNone(age)

    def test_the_registry_load_carries_its_provenance(self):
        doc = dict(self.ROSTER)
        doc["source_digest"] = "d59946d0a1e958d1"
        r = self._roster(doc)
        self.assertEqual(r["source_digest"], "d59946d0a1e958d1")
        self.assertEqual(r["freshness"], "stale")  # the fixture stamp is 2026-08-11
        self.assertIsNotNone(r["age_days"])

    def test_unparseable_registry_does_not_read_as_empty(self):
        with tempfile.TemporaryDirectory() as d:
            p = os.path.join(d, "roster.json")
            with open(p, "w") as f:
                f.write("{not json")
            r = pv.load_roster(p)
        self.assertFalse(r["ok"])
        self.assertIn("parse", r["reason"])


if __name__ == "__main__":
    unittest.main(verbosity=2)
