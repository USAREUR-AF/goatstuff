# SPDX-License-Identifier: Apache-2.0
"""Tests for the real-signal adapters.

The fixtures are invented, in the shape the publishers emit — including the state
that matters most: `overall: down` on a
node whose mesh, zenoh, time, storage and LUKS probes are all `ok`, because the
radar is off and its kids stopped. Mapping `overall` onto a rung would paint
that healthy site red, and this suite is what stops someone doing it later.
"""

from __future__ import annotations

import sys
from pathlib import Path

sys.path.insert(0, str(Path(__file__).resolve().parents[1]))

from goatmap.adapters import (  # noqa: E402
    components_from_node_health,
    device_detected_from_discovery,
    node_online_from_node_health,
    producing_from_node_health,
    stale,
)
from goatmap.readiness import build_chain  # noqa: E402

NOW = 1_786_100_000.0

# Invented: a site at rest. overall=down, and everything that matters is ok.
NODE_HEALTH_AT_REST = {
    "schema": "NodeHealthV1", "node": "node-a",
    "ts_epoch": int(NOW - 10), "interval_s": 15,
    "overall": "down", "worst": ["host", "kids", "thermal_power"],
    "metrics": {
        "host": {"state": "degraded"},
        "kids": {"state": "down"},
        "luks_data_volume": {"state": "ok"},
        "mesh": {"state": "ok"},
        "radar_link": {"state": "unknown", "detail": "no RADAR_HOST discovered"},
        "storage": {"state": "ok"},
        "thermal_power": {"state": "degraded"},
        "time_sync": {"state": "ok"},
        "zenoh_local": {"state": "ok"},
        "zenoh_peer_north": {"state": "ok"},
    },
}

# Invented: discovery scanned a subnet and found nothing.
DISCOVERY_EMPTY = {
    "schema_version": 1, "node_id": "node-1ab548a7ff0d",
    "discovered_at": "2030-01-07T12:03:37+00:00",
    "scan_subnet": "192.168.170.0/24", "scan_duration_ms": 26431,
    "radars": {"echoshield": [], "radar-a": [], "res": []},
}


def _iso_now(offset_s: float = 0) -> str:
    from datetime import datetime, timezone
    return datetime.fromtimestamp(NOW + offset_s, tz=timezone.utc).isoformat()


# ── the one that matters ─────────────────────────────────────────────────────

def test_a_node_reporting_overall_down_still_has_healthy_components():
    """The at-rest state. `overall` is down; the infrastructure is fine."""
    assert NODE_HEALTH_AT_REST["overall"] == "down"
    ok, detail = components_from_node_health(NODE_HEALTH_AT_REST, now=NOW)
    assert ok is True, "infrastructure probes are all ok — this is not a break"
    assert "ok" in detail


def test_kids_inform_producing_not_components():
    """Kids follow the sensor's duty cycle; infrastructure does not."""
    prod, detail = producing_from_node_health(NODE_HEALTH_AT_REST, now=NOW)
    assert prod is False and "kids" in detail
    # And the full chain still reads at-rest, because a duty-cycled device makes
    # the rungs above `components` unexpected.
    comp, _ = components_from_node_health(NODE_HEALTH_AT_REST, now=NOW)
    det, _ = device_detected_from_discovery(DISCOVERY_EMPTY, "radar-a", now=NOW)
    c = build_chain("node-a", cls="radar", power=True, node_online=True,
                    components_ok=comp, device_detected=det, producing=prod,
                    duty_cycled=True)
    assert c.verdict == "at-rest", "a healthy radar-off site must not read broken"
    assert c.break_at is None


# ── discovery ────────────────────────────────────────────────────────────────

def test_an_empty_scan_is_a_real_negative():
    got, detail = device_detected_from_discovery(DISCOVERY_EMPTY, "radar-a", now=NOW)
    assert got is False
    assert "192.168.170.0/24" in detail, "say what was scanned, not just 'no'"


def test_a_device_type_discovery_does_not_scan_for_is_unknown_not_absent():
    """A coverage gap must never be reported as an absent device."""
    got, detail = device_detected_from_discovery(DISCOVERY_EMPTY, "radar-b", now=NOW)
    assert got is None
    assert "does not scan" in detail


def test_a_found_device_is_attained():
    found = dict(DISCOVERY_EMPTY, radars={"radar-a": [{"ip": "192.168.170.50"}]})
    got, detail = device_detected_from_discovery(found, "radar-a", now=NOW)
    assert got is True and "1 on" in detail


def test_stale_discovery_answers_unknown():
    old = dict(DISCOVERY_EMPTY, discovered_at=_iso_now(-4000))
    got, detail = device_detected_from_discovery(old, "radar-a", now=NOW)
    assert got is None and "stale" in detail


def test_unreadable_discovery_timestamp_is_unknown_not_trusted():
    bad = dict(DISCOVERY_EMPTY, discovered_at="not-a-date")
    got, _ = device_detected_from_discovery(bad, "radar-a", now=NOW)
    assert got is None


# ── staleness and absence ────────────────────────────────────────────────────

def test_stale_node_health_answers_nothing_in_either_direction():
    old = dict(NODE_HEALTH_AT_REST, ts_epoch=int(NOW - 9999))
    assert stale(old, 120, NOW)
    assert components_from_node_health(old, now=NOW)[0] is None
    assert producing_from_node_health(old, now=NOW)[0] is None


def test_a_payload_with_no_timestamp_is_stale_by_default():
    assert stale({}, 120, NOW) is True
    assert components_from_node_health({}, now=NOW)[0] is None


def test_degraded_infrastructure_is_still_doing_its_job():
    """Otherwise every imperfect site reads broken and buries the real ones."""
    p = dict(NODE_HEALTH_AT_REST)
    p["metrics"] = dict(p["metrics"], storage={"state": "degraded"})
    ok, detail = components_from_node_health(p, now=NOW)
    assert ok is True and "degraded: storage" in detail


def test_infrastructure_down_is_a_real_break():
    p = dict(NODE_HEALTH_AT_REST)
    p["metrics"] = dict(p["metrics"], zenoh_local={"state": "down"})
    ok, detail = components_from_node_health(p, now=NOW)
    assert ok is False and "zenoh_local" in detail


def test_no_infrastructure_probes_at_all_is_unknown():
    p = dict(NODE_HEALTH_AT_REST, metrics={"kids": {"state": "ok"}})
    assert components_from_node_health(p, now=NOW)[0] is None


def test_self_reported_mesh_is_corroboration_only():
    # It answers, but the docstring and the caller must treat the registry as
    # the authority — a node off the mesh publishes nothing at all.
    got, detail = node_online_from_node_health(NODE_HEALTH_AT_REST, now=NOW)
    assert got is True and detail


def test_arrival_beats_the_nodes_own_opinion_of_its_mesh():
    """A node delivers NodeHealthV1 over the mesh while its own `mesh` probe still
    reads down. Returning False would read as "we looked and it is offline", break
    the ladder there, turn every rung above unknown, and roll the SITE up to
    `broken` while the node is healthy and publishing.

    A message cannot arrive from a node that is off the mesh. Arrival settles
    the rung; the probe's opinion belongs in the detail. This must never return
    False — absence is unknown, which the caller handles separately.
    """
    claims_down = dict(NODE_HEALTH_AT_REST,
                       metrics=dict(NODE_HEALTH_AT_REST["metrics"],
                                    mesh={"state": "down"}))
    got, detail = node_online_from_node_health(claims_down, now=NOW)
    assert got is True, "a node that just published cannot be offline"
    assert "DOWN" in detail, "the probe's claim must still be surfaced, not hidden"


def test_absent_health_is_unknown_never_offline():
    """The other half of the same rule: silence is not evidence of offline.

    A node whose mesh is genuinely broken cannot tell you so, which is why the
    authority is the far-end vantage and absence stays unknown.
    """
    got, _ = node_online_from_node_health({}, now=NOW)
    assert got is None


# ── fabric-delivered node health ─────────────────────────────────────────────

def test_the_fabric_payload_is_the_same_shape_the_adapters_parse():
    """The shape of `<anchor>/node-a/site-a/health/v1`.

    The bridge forwards the node's own publish unchanged, so the adapters need
    no second parser — but this pins that, because a divergence would show up
    as every rung quietly reading unknown.
    """
    fabric_doc = dict(NODE_HEALTH_AT_REST)
    fabric_doc["governor"] = {"class": "essential"}   # extra field the bridge adds
    ok, _ = components_from_node_health(fabric_doc, now=NOW)
    assert ok is True, "an unknown extra field must not break parsing"


def test_a_wrong_schema_is_ignored_rather_than_half_parsed():
    """Silently ignoring the wrong schema beats a confident wrong answer."""
    assert components_from_node_health({"schema": "SomethingElseV1"}, now=NOW)[0] is None
