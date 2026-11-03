#!/usr/bin/env python3
"""Tests for the budget-notify one-time-notice guarantees.

Every ledger entry is a notice that can never be sent again, so the three
behaviours worth pinning are the ones that decide whether an entry is written:
a send that succeeded (write it), a send or resolver that failed (do NOT), and
a band JSON too old to mail off at all (decline the run).

Run: python3 -m unittest discover -s ops/zenoh-trial-peers/budget-notify
"""
import importlib.util
import json
import os
import pathlib
import sqlite3
import tempfile
import time
import unittest
import unittest.mock

HERE = pathlib.Path(__file__).parent
_spec = importlib.util.spec_from_file_location("budget_notify", HERE / "budget-notify.py")
bn = importlib.util.module_from_spec(_spec)
_spec.loader.exec_module(bn)


def write_registry(path, rows):
    db = sqlite3.connect(path)
    db.execute("CREATE TABLE slots (slot_id TEXT, assigned_to TEXT)")
    db.executemany("INSERT INTO slots VALUES (?, ?)", rows)
    db.commit()
    db.close()


def band(owners, age_seconds=0, source_state="live"):
    return {
        "budget_source_state": source_state,
        "sampled_at": int(time.time()) - age_seconds,
        "owners_budget": owners,
    }


def owner_row(owner, granted=100, stored=100):
    return {"owner": owner, "granted": granted, "stored_keys": stored,
            "budget_state": "at_cap"}


class NotifyCase(unittest.TestCase):
    def setUp(self):
        self.tmp = tempfile.TemporaryDirectory()
        self.addCleanup(self.tmp.cleanup)
        d = pathlib.Path(self.tmp.name)
        self.fabric, self.state = d / "fabric.json", d / "state.json"
        self.registry, self.emails = d / "registry.db", d / "emails.json"
        for var in ("SMTP_HOST", "SMTP_FROM"):
            os.environ[var] = "set-for-tests"

    def run_notifier(self, fabric_doc, extra=()):
        self.fabric.write_text(json.dumps(fabric_doc))
        argv = ["budget-notify.py", "--fabric", str(self.fabric), "--state", str(self.state),
                "--registry", str(self.registry), "--emails", str(self.emails),
                "--best-practices-url", "https://example.invalid/bp"] + list(extra)
        with unittest.mock.patch("sys.argv", argv):
            return bn.main()

    def ledger(self):
        return json.loads(self.state.read_text()) if self.state.exists() else {}


class TestLedgerPerSend(NotifyCase):
    """(a) The ledger is persisted after EACH successful send."""

    def setUp(self):
        super().setUp()
        write_registry(self.registry, [("anchor-a", "alice"), ("anchor-b", "bob")])
        self.emails.write_text(json.dumps({"alice": "alice@example.invalid",
                                           "bob": "bob@example.invalid"}))

    def test_smtp_failure_midbatch_keeps_the_delivered_owner_ledgered(self):
        # alice delivers, bob's send raises — the classic partial batch.
        def flaky(cfg, to, subject, body):
            if to.startswith("bob"):
                raise OSError("connection reset")

        with unittest.mock.patch.object(bn, "send", side_effect=flaky):
            rc = self.run_notifier(band([owner_row("anchor-a"), owner_row("anchor-b")]))

        self.assertEqual(rc, 0)
        led = self.ledger()
        self.assertEqual(led.get("anchor-a@100"), ["at_cap"],
                         "a delivered send must be ledgered even though a later send failed")
        self.assertNotIn("anchor-b@100", led,
                         "a failed send must NOT be ledgered — it retries next run")

    def test_second_run_resends_only_the_failed_owner(self):
        with unittest.mock.patch.object(bn, "send", side_effect=lambda c, to, s, b: (
                (_ for _ in ()).throw(OSError("reset")) if to.startswith("bob") else None)):
            self.run_notifier(band([owner_row("anchor-a"), owner_row("anchor-b")]))

        sent = []
        with unittest.mock.patch.object(bn, "send", side_effect=lambda c, to, s, b: sent.append(to)):
            self.run_notifier(band([owner_row("anchor-a"), owner_row("anchor-b")]))

        self.assertEqual(sent, ["bob@example.invalid"],
                         "the already-delivered owner must not be re-sent next tick")


class TestResolverErrorNotLedgered(NotifyCase):
    """(b) A resolver EXCEPTION must not consume the one-time notice."""

    def test_unreadable_registry_leaves_the_notice_pending(self):
        self.emails.write_text(json.dumps({}))
        # registry.db never created => sqlite cannot open it read-only.
        with unittest.mock.patch.object(bn, "send") as sender:
            rc = self.run_notifier(band([owner_row("anchor-a")]))

        self.assertEqual(rc, 0)
        sender.assert_not_called()
        self.assertEqual(self.ledger(), {},
                         "a registry outage must leave the notice pending, not ledger it")

    def test_kratos_outage_leaves_the_notice_pending(self):
        write_registry(self.registry, [("anchor-a", "alice")])
        self.emails.write_text(json.dumps({}))  # forces the Kratos tier
        with unittest.mock.patch.object(bn, "kratos_email", return_value=("", False)):
            self.run_notifier(band([owner_row("anchor-a")]),
                              extra=["--kratos-admin-url", "http://kratos.invalid:4434"])
        self.assertEqual(self.ledger(), {},
                         "an unreachable Kratos is not proof that the owner has no address")

    def test_genuine_no_address_is_ledgered_once(self):
        write_registry(self.registry, [("anchor-a", "alice")])
        self.emails.write_text(json.dumps({}))
        # Kratos consulted successfully, matched nobody: a real no-address.
        with unittest.mock.patch.object(bn, "kratos_email", return_value=("", True)):
            self.run_notifier(band([owner_row("anchor-a")]),
                              extra=["--kratos-admin-url", "http://kratos.invalid:4434"])
        self.assertEqual(self.ledger().get("anchor-a@100"), ["at_cap"],
                         "a proven no-address owner is ledgered so the log line is one-time too")


class TestStalenessRefusal(NotifyCase):
    """(c) A frozen band JSON means a dead crawler — decline, never mail."""

    def setUp(self):
        super().setUp()
        write_registry(self.registry, [("anchor-a", "alice")])
        self.emails.write_text(json.dumps({"alice": "alice@example.invalid"}))

    def test_stale_band_declines_without_sending(self):
        with unittest.mock.patch.object(bn, "send") as sender:
            rc = self.run_notifier(band([owner_row("anchor-a")], age_seconds=3600))
        self.assertEqual(rc, 0)
        sender.assert_not_called()
        self.assertEqual(self.ledger(), {})

    def test_fresh_band_sends(self):
        with unittest.mock.patch.object(bn, "send") as sender:
            self.run_notifier(band([owner_row("anchor-a")], age_seconds=30))
        sender.assert_called_once()

    def test_band_without_sampled_at_declines(self):
        doc = band([owner_row("anchor-a")])
        del doc["sampled_at"]
        with unittest.mock.patch.object(bn, "send") as sender:
            self.run_notifier(doc)
        sender.assert_not_called()

    def test_staleness_check_disabled_by_zero(self):
        with unittest.mock.patch.object(bn, "send") as sender:
            self.run_notifier(band([owner_row("anchor-a")], age_seconds=3600),
                              extra=["--max-sample-age", "0"])
        sender.assert_called_once()


class TestPostgresRegistry(NotifyCase):
    """portal_store: postgres — the owner resolves through `docker exec psql`."""

    def fake_psql(self, rows=None, rc=0):
        calls = []

        def run(argv, input=None, **kw):
            calls.append((argv, input))
            return unittest.mock.Mock(returncode=rc, stdout=json.dumps(rows or []),
                             stderr="" if rc == 0 else "connection refused")
        return calls, run

    def test_resolves_and_sends_read_only_with_a_bound_parameter(self):
        self.emails.write_text(json.dumps({"alice": "alice@example.invalid"}))
        calls, run = self.fake_psql([{"assigned_to": "alice", "lifecycle_class": None}])
        with unittest.mock.patch.object(bn.subprocess, "run", side_effect=run), \
                unittest.mock.patch.object(bn, "send") as sender:
            argv = ["budget-notify.py", "--fabric", str(self.fabric), "--state", str(self.state),
                    "--registry-store", "postgres", "--emails", str(self.emails),
                    "--best-practices-url", "https://example.invalid/bp"]
            self.fabric.write_text(json.dumps(band([owner_row("anchor-a")])))
            with unittest.mock.patch("sys.argv", argv):
                rc = bn.main()
        self.assertEqual(rc, 0)
        sender.assert_called_once()
        self.assertEqual(self.ledger().get("anchor-a@100"), ["at_cap"])
        argv, sql = calls[0]
        self.assertEqual(argv[:3], ["docker", "exec", "-i"])
        self.assertIn("PGOPTIONS=-c default_transaction_read_only=on", argv)
        self.assertIn("owner=anchor-a", argv)
        self.assertNotIn("anchor-a", sql, "the anchor is a psql variable, never formatted into SQL")

    def test_unreachable_postgres_leaves_the_notice_pending(self):
        self.emails.write_text(json.dumps({"alice": "alice@example.invalid"}))
        _, run = self.fake_psql(rc=2)
        with unittest.mock.patch.object(bn.subprocess, "run", side_effect=run), \
                unittest.mock.patch.object(bn, "send") as sender:
            argv = ["budget-notify.py", "--fabric", str(self.fabric), "--state", str(self.state),
                    "--registry-store", "postgres", "--emails", str(self.emails),
                    "--best-practices-url", "https://example.invalid/bp"]
            self.fabric.write_text(json.dumps(band([owner_row("anchor-a")])))
            with unittest.mock.patch("sys.argv", argv):
                rc = bn.main()
        self.assertEqual(rc, 0)
        sender.assert_not_called()
        self.assertEqual(self.ledger(), {})


if __name__ == "__main__":
    unittest.main()
