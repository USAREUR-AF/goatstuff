# SPDX-License-Identifier: Apache-2.0
"""Tests for region membership, non-geographic layout, and free movement.

The load-bearing assertion is the first one: after the region model, a site's
rendered position must carry NO information about where it actually is. That is
a stronger claim than the quantization it replaces — blur can be attacked with
enough samples; a grid cell index cannot, because the coordinate was never sent.
"""

from __future__ import annotations

import sys
from pathlib import Path

sys.path.insert(0, str(Path(__file__).resolve().parents[1]))

from goatmap.layout import grid, load_regions, tree  # noqa: E402
from goatmap.navmodel import build_index, jump_targets, rid, search  # noqa: E402
from goatmap.sites import Site, System  # noqa: E402


def _estate():
    """Two regions; the far-apart sites deliberately share a region."""
    sites = [
        Site(site_id="site-a", label="Site A", lat=0.1589, lon=0.6503, region_id="delta",
             systems=[
                 System("site-router", "router", peer="rtr.netbird", state="green"),
                 System("svn-001", "node", peer="svn-001.netbird", state="amber",
                        parent="site-router"),
                 System("radar-a", "radar", parent="svn-001", state="idle"),
             ]),
        Site(site_id="site-b", label="Site B", lat=0.1900, lon=-0.2000, region_id="delta",
             systems=[System("pod-a", "pod", peer="pod-a.netbird", state="green")]),
        Site(site_id="rear", label="Rear", lat=52.0, lon=13.0, region_id="north",
             systems=[System("mgmt", "node", peer="mgmt.netbird", state="green")]),
    ]
    regions = load_regions([
        {"region_id": "delta", "label": "Region One", "lat": 0.16, "lon": 0.30,
         "sites": ["site-a", "site-b"]},
        {"region_id": "north", "label": "North", "lat": 52.0, "lon": 13.0,
         "sites": ["rear"]},
    ])
    return regions, {s.site_id: s for s in sites}


def _index():
    regions, by_id = _estate()
    return build_index(
        regions, by_id,
        layout_for_region=lambda r: grid([s for s in r.site_ids if s in by_id]),
        layout_for_site=lambda s: tree([(y.name, y.parent or None) for y in s.systems]),
    )


# ── the security property ────────────────────────────────────────────────────

def test_site_position_carries_no_geographic_information():
    """The whole point of the region model."""
    idx = _index()
    region = idx[rid("region", "delta")]
    positions = region.meta["positions"]
    # Sites are placed on an abstract grid, in unit cells.
    assert set(positions) == {rid("site", "site-a"), rid("site", "site-b")}
    for pos in positions.values():
        assert set(pos) == {"x", "y"}
        assert float(pos["x"]).is_integer() and float(pos["y"]).is_integer()

    # And no site node carries a coordinate at all any more.
    for node in idx.values():
        if node.kind == "site":
            assert "lat" not in node.meta and "lon" not in node.meta

    # Site A and Site B are ~65 km apart; their grid cells are adjacent and say
    # nothing about that. Ordering is alphabetical, not spatial.
    s = positions[rid("site", "site-a")]
    t = positions[rid("site", "site-b")]
    assert abs(s["x"] - t["x"]) <= 1 and abs(s["y"] - t["y"]) <= 1


def test_only_the_region_anchor_is_geographic_and_it_is_coarse():
    idx = _index()
    lat, lon = idx[rid("region", "delta")].meta["lat"], idx[rid("region", "delta")].meta["lon"]
    # Quantized to the region step — an area, not a point.
    assert lat != 0.16 and lon != 0.30
    assert idx[rid("region", "delta")].meta["precision_deg"] == 0.25


# ── layout choice ────────────────────────────────────────────────────────────

def test_grid_is_stable_and_ordered_by_key_not_arrival():
    # Spatial memory has to survive a redeploy, so ordering cannot depend on
    # the order members happened to be declared.
    a = grid(["c", "a", "b"])
    b = grid(["b", "c", "a"])
    assert [p.key for p in a] == [p.key for p in b] == ["a", "b", "c"]
    assert grid([]) == []


def test_grid_stays_near_square():
    assert max(p.x for p in grid([str(i) for i in range(9)])) == 2   # 3x3
    assert max(p.x for p in grid([str(i) for i in range(4)])) == 1   # 2x2


def test_tree_layers_by_depth_and_centres_parents():
    placed = {p.key: p for p in tree([
        ("router", None), ("node-a", "router"), ("node-b", "router"),
        ("sensor", "node-a"),
    ])}
    assert placed["router"].depth == 0
    assert placed["node-a"].depth == 1 and placed["node-b"].depth == 1
    assert placed["sensor"].depth == 2
    # Parent sits between its children — the edges are the information.
    assert placed["node-a"].x <= placed["router"].x <= placed["node-b"].x
    assert placed["node-a"].parent == "router"


def test_tree_survives_a_malformed_declaration():
    # A missing parent is treated as a root rather than raising…
    got = {p.key for p in tree([("orphan", "nobody"), ("root", None)])}
    assert got == {"orphan", "root"}
    # …and a cycle still yields a position for every node instead of hanging.
    cyc = tree([("a", "b"), ("b", "a")])
    assert {p.key for p in cyc} == {"a", "b"}


# ── free movement ────────────────────────────────────────────────────────────

def test_every_level_is_reachable_in_one_step():
    idx = _index()
    sensor = rid("site", "site-a", "radar-a")
    targets = jump_targets(idx, sensor)
    # Up to its site AND its region without walking, plus its siblings.
    assert rid("site", "site-a") in targets
    assert rid("region", "delta") in targets
    assert rid("site", "site-a", "svn-001") in targets
    assert sensor not in targets


def test_sites_can_hop_sideways_without_going_up():
    idx = _index()
    targets = jump_targets(idx, rid("site", "site-a"))
    assert rid("site", "site-b") in targets, "sibling site must be one step away"


def test_search_finds_a_system_from_a_fragment():
    idx = _index()
    hits = search(idx, "rad")
    assert hits and hits[0]["id"] == rid("site", "site-a", "radar-a")
    assert hits[0]["kind"] == "system"
    # Prefix matches rank above substring matches.
    assert search(idx, "sit")[0]["kind"] == "site"
    assert search(idx, "") == []


def test_state_rolls_up_worst_first_through_the_levels():
    idx = _index()
    # svn-001 is amber, so Site A is amber even though the router is green.
    assert idx[rid("site", "site-a")].state == "amber"
    assert idx[rid("site", "site-b")].state == "green"


def test_ids_are_stable_against_insertion():
    # Deep links must survive a new site appearing — ids come from declared
    # names, never list position.
    assert rid("site", "site-a", "radar-a") == "site/site-a/radar-a"
    assert rid("region", "Region One") == "region/region-one"
