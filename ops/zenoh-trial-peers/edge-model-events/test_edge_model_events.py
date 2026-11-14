#!/usr/bin/env python3
# SPDX-License-Identifier: Apache-2.0
"""Tests for the edge-model-events worked example.

What is pinned is what the example teaches: every event from one source goes
on one stable key under the publisher's slot, everything that varies rides in
the payload, a bad slot or source never reaches the wire, and a failed publish
stops the run instead of being counted.

Run: python3 -m pytest -q ops/zenoh-trial-peers/edge-model-events
"""
import datetime
import importlib.util
import io
import json
import pathlib
import shlex
import subprocess
import tempfile
import unittest

HERE = pathlib.Path(__file__).parent
_spec = importlib.util.spec_from_file_location("edge_model_events", HERE / "edge-model-events.py")
eme = importlib.util.module_from_spec(_spec)
_spec.loader.exec_module(eme)

SLOT = "0123456789abcdef0123456789abcdef"
T0 = datetime.datetime(2026, 11, 14, 12, 0, 0, 250000, tzinfo=datetime.timezone.utc)


def args(*extra):
    return eme.parse_args(["--slot", SLOT, "--source", "line-3-camera", "--site", "site-a",
                           "--goat", "goat", *extra])


class Recorder:
    def __init__(self, codes=()):
        self.calls, self.codes = [], list(codes)

    def __call__(self, cmd):
        self.calls.append(cmd)
        return subprocess.CompletedProcess(cmd, self.codes.pop(0) if self.codes else 0)


def run_live(a, runner):
    # --goat must resolve for a live run; point it at an interpreter that exists.
    a.goat = "python3"
    return eme.run(a, runner=runner, sleep=lambda s: None, clock=lambda: T0, out=io.StringIO())


class KeyTest(unittest.TestCase):
    def test_one_stable_key_under_the_slot(self):
        self.assertEqual(eme.event_key(SLOT, "line-3-camera"),
                         f"{SLOT}/edge/line-3-camera/detections/v1")

    def test_an_alias_is_accepted_as_the_slot(self):
        self.assertTrue(eme.event_key("tarsier", "cam").startswith("tarsier/"))

    def test_placeholder_or_path_slot_is_refused(self):
        for bad in ("<your-slot>", "a/b", "", "*", "acme/**"):
            with self.subTest(bad=bad), self.assertRaises(eme.ExampleError):
                eme.event_key(bad, "cam")

    def test_source_that_would_change_the_key_is_refused(self):
        for bad in ("Line 3", "cam/1", "cam*", "cam$", "", "-cam"):
            with self.subTest(bad=bad), self.assertRaises(eme.ExampleError):
                eme.event_key(SLOT, bad)


class PublishTest(unittest.TestCase):
    def test_every_event_uses_the_same_key_and_the_payload_varies(self):
        rec = Recorder()
        sent = run_live(args(), rec)
        self.assertEqual(sent, 6)
        keys = {c[2] for c in rec.calls}
        self.assertEqual(keys, {f"{SLOT}/edge/line-3-camera/detections/v1"})
        payloads = [json.loads(c[c.index("--payload") + 1]) for c in rec.calls]
        self.assertEqual([p["seq"] for p in payloads], [1, 2, 3, 4, 5, 6])
        self.assertEqual({p["kind"] for p in payloads}, {"barcode", "qr", "object"})
        self.assertEqual(payloads[0]["seen_at"], "2026-11-14T12:00:00.250Z")
        self.assertEqual(payloads[0]["site"], "site-a")
        for c in rec.calls:
            self.assertEqual(c[1], "pub")
            self.assertEqual(c[c.index("--encoding") + 1], "json")

    def test_events_are_kept_not_volatile(self):
        rec = Recorder()
        run_live(args("--limit", "1"), rec)
        self.assertFalse(rec.calls[0][2].startswith("volatile/"))
        self.assertNotIn("--volatile-stream", rec.calls[0])

    def test_failed_publish_stops_the_run(self):
        rec = Recorder(codes=[0, 1])
        with self.assertRaises(eme.ExampleError) as cm:
            run_live(args(), rec)
        self.assertEqual(len(rec.calls), 2)
        self.assertIn("event 2", str(cm.exception))

    def test_limit(self):
        rec = Recorder()
        self.assertEqual(run_live(args("--limit", "2"), rec), 2)

    def test_dry_run_runs_nothing(self):
        rec = Recorder()
        out = io.StringIO()
        sent = eme.run(args("--dry-run"), runner=rec, sleep=lambda s: None, clock=lambda: T0, out=out)
        self.assertEqual(sent, 6)
        self.assertEqual(rec.calls, [])
        self.assertIn("would run: goat pub", out.getvalue())
        first = [l for l in out.getvalue().splitlines() if l.startswith("would run: ")][0]
        cmd = shlex.split(first[len("would run: "):])
        self.assertEqual(json.loads(cmd[cmd.index("--payload") + 1])["seq"], 1)

    def test_missing_goat_is_named(self):
        a = args()
        a.goat = "goat-not-installed-here"
        with self.assertRaises(eme.ExampleError) as cm:
            eme.run(a, runner=Recorder(), out=io.StringIO())
        self.assertIn("not on PATH", str(cm.exception))


class FixtureTest(unittest.TestCase):
    def bad_fixture(self, line):
        tmp = tempfile.NamedTemporaryFile("w", suffix=".jsonl", delete=False)
        tmp.write(line + "\n")
        tmp.close()
        return pathlib.Path(tmp.name)

    def test_bad_rows_are_refused_with_the_line(self):
        for line in ('{"kind": "face", "value": "x", "confidence": 0.5}',
                     '{"kind": "qr", "value": "", "confidence": 0.5}',
                     '{"kind": "qr", "value": "x", "confidence": 2}',
                     'not json'):
            path = self.bad_fixture(line)
            with self.subTest(line=line), self.assertRaises(eme.ExampleError) as cm:
                list(eme.detections(path))
            self.assertIn(":1:", str(cm.exception))
            path.unlink()

    def test_shipped_fixture_is_valid(self):
        self.assertEqual(len(list(eme.detections(eme.DEFAULT_FIXTURE))), 6)


class MainTest(unittest.TestCase):
    def test_refusal_exits_2(self):
        self.assertEqual(eme.main(["--slot", "<your-slot>", "--source", "cam", "--dry-run"]), 2)


if __name__ == "__main__":
    unittest.main()
