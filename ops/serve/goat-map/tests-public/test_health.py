# SPDX-License-Identifier: Apache-2.0
"""Health must distinguish 'nothing to show' from 'nothing working'.

If the Zenoh session fails to open at startup while the upstream is briefly down,
and the failure is logged once and never retried, the map stays empty with
`/healthz` reporting `up`: a green light over a dead input. A health signal which
cannot go red is decoration.
"""

from __future__ import annotations

import importlib
import sys
from pathlib import Path

import pytest

ROOT = Path(__file__).resolve().parents[1]
sys.path.insert(0, str(ROOT))


@pytest.fixture
def svc(monkeypatch):
    """A Service with no real Zenoh — only the health logic is under test."""
    monkeypatch.setenv("GOATMAP_INGEST_ENDPOINT", "tcp/198.51.100.1:7447")
    monkeypatch.setenv("GOATMAP_STALE_AFTER_S", "180")
    monkeypatch.setenv("GOATMAP_FABRIC_ENDPOINT", "")
    monkeypatch.setenv("GOATMAP_PUBLISH_PREFIX", "")

    main = importlib.import_module("goatmap.__main__")
    importlib.reload(main)

    from goatmap.store import TrackStore

    return main, main.Service(TrackStore(), main.Hub(), None)


def test_a_session_that_never_opened_reads_degraded(svc):
    main, s = svc
    s.ingest = None
    s.ingest_error = "Unable to connect"
    # Old behaviour returned "up" here — the exact 92-minute lie.
    status, reasons = s.health()
    assert status == "degraded"
    assert any("ingest session down" in r for r in reasons)


def test_connected_but_silent_reads_degraded(svc):
    main, s = svc
    s.ingest = object()  # stand-in for a live session
    s.last_recv_at = None
    # Long enough that a first message was due.
    s.started_at -= 1_000
    status, reasons = s.health()
    assert status == "degraded"
    assert any("no message ever received" in r for r in reasons)


def test_a_stalled_feed_reads_degraded(svc):
    import time

    main, s = svc
    s.ingest = object()
    s.last_recv_at = time.monotonic() - 600
    status, reasons = s.health()
    assert status == "degraded"
    assert any("no message for" in r for r in reasons)


def test_flowing_feed_reads_up(svc):
    import time

    main, s = svc
    s.ingest = object()
    s.last_recv_at = time.monotonic()
    assert s.health() == ("up", [])


def test_quiet_startup_is_not_yet_degraded(svc):
    """A freshly started service has not failed just because the sensor has
    not spoken yet — only once a first message was actually due."""
    main, s = svc
    s.ingest = object()
    s.last_recv_at = None
    # Just started: inside the staleness window.
    status, _ = s.health()
    assert status == "up"


def test_reasons_are_specific_enough_to_act_on(svc):
    main, s = svc
    s.ingest = None
    s.ingest_error = "Unable to connect to any of [tcp/198.51.100.1:7447]"
    _, reasons = s.health()
    # A prober should not need a second call to learn what broke.
    assert any("198.51.100.1" in r for r in reasons)
