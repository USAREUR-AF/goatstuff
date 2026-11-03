# SPDX-License-Identifier: Apache-2.0
"""Tests for `node_online` taken from the mesh vantage.

A node with no health publisher would render `unknown` on every rung while NetBird
reports it `Connected` over P2P, if `node_online` were read from the node's OWN
health publish: its silence would count as absence of knowledge forever.

The vantage fixes that for every declared peer without installing anything on
the peer. What these pin is the boundary that makes it safe: the vantage may
say connected or not connected, but a peer it never assessed is still
`unknown` — absence from a list is not evidence of being down.
"""

from __future__ import annotations

import sys
from pathlib import Path

sys.path.insert(0, str(Path(__file__).resolve().parents[1]))

from goatmap.adapters import node_online_from_mesh  # noqa: E402
from goatmap.envhealth import MeshHealth, MeshLink  # noqa: E402
from goatmap.readiness import build_chain  # noqa: E402

NOW = 1_786_100_000.0
PI = "node-b.netbird.selfhosted"
SVN = "node-a.netbird.selfhosted"


def _mesh(age_s: float = 30, links=None) -> MeshHealth:
    return MeshHealth(
        domain="mesh", scope="my-goatnet",
        ts_unix_ms=int((NOW - age_s) * 1000),
        links=links if links is not None else [
            MeshLink(peer_fqdn=SVN, connected=True, peer_class="always-up", status="P2P"),
            MeshLink(peer_fqdn=PI, connected=True, peer_class="always-up", status="P2P"),
        ],
    )


# ── the case that prompted this ──────────────────────────────────────────────

def test_a_peer_that_publishes_nothing_is_still_known_to_be_up():
    """pi-1 has no health publisher; the vantage still answers for it."""
    got, detail = node_online_from_mesh(_mesh(), PI, now=NOW)
    assert got is True
    assert "connected" in detail


def test_the_vantage_moves_the_unknown_up_the_chain_it_does_not_erase_it():
    """What the vantage actually buys, stated exactly.

    pi-1 still has no health publisher, so `components` remains unknowable and
    the chain verdict stays `unknown`. That is correct: we now know the node is
    UP, and we still do not know whether its components are running. The gain
    is that the first unknown moves from `node_online` to `components`, which
    turns "we cannot see this node at all" into "the node is up, its software
    is unreported" — a different problem with a different fix.
    """
    online, detail = node_online_from_mesh(_mesh(), PI, now=NOW)
    c = build_chain("node-b", cls="node", node_online=online)
    rungs = c.by_name()
    assert rungs["node_online"].state == "attained"
    assert rungs["components"].state == "unknown"
    assert c.verdict == "unknown", "honest: liveness is not health"
    assert c.break_at is None, "unknown is never a break"
    assert "connected" in detail


# ── the boundary that keeps it honest ────────────────────────────────────────

def test_a_peer_the_vantage_never_assessed_is_unknown_not_offline():
    """Absence from a list is not evidence of being down."""
    got, detail = node_online_from_mesh(_mesh(), "never-declared.netbird.selfhosted", now=NOW)
    assert got is None
    assert "not assessed" in detail


def test_the_missing_peer_name_is_named_so_a_roster_typo_is_visible():
    """A silent miss looks identical to a down peer on the surface."""
    _, detail = node_online_from_mesh(_mesh(), "node-b-typo.netbird.selfhosted", now=NOW)
    assert "node-b-typo.netbird.selfhosted" in detail


def test_a_disconnected_peer_is_a_real_negative():
    m = _mesh(links=[MeshLink(peer_fqdn=PI, connected=False, status="disconnected")])
    got, detail = node_online_from_mesh(m, PI, now=NOW)
    assert got is False and "not connected" in detail


def test_a_real_negative_breaks_the_chain_at_node_online():
    m = _mesh(links=[MeshLink(peer_fqdn=PI, connected=False)])
    online, _ = node_online_from_mesh(m, PI, now=NOW)
    c = build_chain("node-b", cls="node", power=True, node_online=online)
    assert c.break_at == "node_online"


# ── staleness ────────────────────────────────────────────────────────────────

def test_a_stale_vantage_answers_nothing_in_either_direction():
    """A vantage that stopped publishing cannot say who is up NOW."""
    got, detail = node_online_from_mesh(_mesh(age_s=3600), PI, now=NOW)
    assert got is None and "stale" in detail


def test_a_reading_with_no_timestamp_is_still_read():
    """Absent ts is not proof of staleness; the vantage's own list still counts."""
    m = _mesh()
    m.ts_unix_ms = 0
    assert node_online_from_mesh(m, PI, now=NOW)[0] is True


# ── defensive ────────────────────────────────────────────────────────────────

def test_no_reading_at_all_is_unknown():
    assert node_online_from_mesh(None, PI, now=NOW)[0] is None


def test_an_empty_peer_name_never_matches_something_by_accident():
    assert node_online_from_mesh(_mesh(), "", now=NOW)[0] is None


def test_a_malformed_reading_does_not_decide_a_rung():
    class Broken:
        ts_unix_ms = int(NOW * 1000)

        def by_peer(self):
            raise ValueError("truncated protobuf")

    got, detail = node_online_from_mesh(Broken(), PI, now=NOW)
    assert got is None and "unreadable" in detail
