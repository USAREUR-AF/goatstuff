# SPDX-License-Identifier: Apache-2.0
"""Tests for lasso queries and tiered aggregation.

Three assertions here are security controls rather than behaviour checks:

1. **Aggregation never out-resolves the site anchor quantization.** sites.py
   blurs infrastructure positions because they are a targeting aid; a fine
   grid would hand that precision back by narrowing a site to a sub-quantum
   cell.
2. **Counts do not ride below the `full` tier.** A cell count is a
   denominator (disclosure.py rule 1) — learning a cell holds 4,000 contacts
   reveals the size of a force the viewer cannot enumerate.
3. **Roll-up is worst-wins.** A cell holding one broken thing and nine
   healthy ones is a cell someone needs to look at; an average hides exactly
   the case the surface exists to surface.
"""

from __future__ import annotations

import sys
from pathlib import Path

sys.path.insert(0, str(Path(__file__).resolve().parents[1]))

from goatmap.disclosure import FULL, STATE  # noqa: E402
from goatmap.sites import DEFAULT_QUANT_DEG  # noqa: E402
from goatmap.viewport import (  # noqa: E402
    FEATURE_LIMIT,
    point_in_polygon,
    query_sites,
    query_tracks,
)

# A square around an invented site near null island.
BOX = [[-0.2, -0.2], [1.0, -0.2], [1.0, 1.0], [-0.2, 1.0]]


def _track(i, lat=0.1, lon=0.6, **kw):
    t = {"id": f"t{i}", "lat": lat, "lon": lon, "synthetic": True,
         "affiliation": "unknown"}
    t.update(kw)
    return t


class _Site:
    def __init__(self, sid, lat, lon):
        self.site_id, self.label, self.systems = sid, sid, []
        self._a = (lat, lon)

    @property
    def anchor(self):
        return self._a


# --- point in polygon -------------------------------------------------------


def test_point_inside_and_outside():
    assert point_in_polygon(0.6, 0.1, BOX)
    assert not point_in_polygon(20.0, 0.1, BOX)
    assert not point_in_polygon(0.6, 40.0, BOX)


def test_degenerate_polygon_matches_nothing():
    """A two-point 'polygon' is a line. Returning everything for it would
    turn a mis-drag into a full estate dump."""
    assert not point_in_polygon(0.6, 0.1, [[-0.2, -0.2], [1.0, 0.0]])


# --- tiering ----------------------------------------------------------------


def test_few_tracks_are_enumerated():
    r = query_tracks([_track(i) for i in range(5)], BOX, FULL)
    assert r["mode"] == "features"
    assert len(r["tracks"]) == 5


def test_many_tracks_are_aggregated():
    """Above the limit the payload must stop growing with the estate — this
    is the property that makes the surface usable on a 5 Mbps link."""
    many = [_track(i, lat=0.0 + (i % 40) * 0.01, lon=0.2 + (i % 37) * 0.01)
            for i in range(FEATURE_LIMIT + 50)]
    r = query_tracks(many, BOX, FULL)
    assert r["mode"] == "aggregate"
    assert "tracks" not in r, "aggregate mode must not also ship every feature"
    assert len(r["cells"]) < len(many), "aggregation did not reduce anything"


def test_tracks_outside_the_lasso_are_never_returned():
    """SECURITY. Server-side filtering means server-side: a track outside the
    query must not appear, flagged or otherwise."""
    r = query_tracks([_track(1), _track(2, lat=10.0, lon=10.0)], BOX, FULL)
    ids = {t["id"] for t in r["tracks"]}
    assert ids == {"t1"}
    assert "t2" not in repr(r)


# --- the quantization floor -------------------------------------------------


def test_bin_is_never_finer_than_the_anchor_quantization():
    """SECURITY. sites.py blurs positions deliberately. If aggregation binned
    finer than that blur, an observer could narrow a site to a sub-quantum
    cell and recover what quantize() removed."""
    tight = [[0.60, 0.10], [0.61, 0.10], [0.61, 0.11], [0.60, 0.11]]
    # Every site must fall INSIDE the tight lasso, or they are filtered out
    # before tiering and the aggregate path is never exercised. Step chosen so
    # all of them fit within the box's 0.01 degrees.
    sites = [_Site(f"s{i}", lat=0.100 + i * 0.00002, lon=0.605) for i in range(FEATURE_LIMIT + 5)]
    r = query_sites(sites, tight, lambda s: "ready", FULL)
    assert r["mode"] == "aggregate"
    assert r["bin_deg"] >= DEFAULT_QUANT_DEG, (
        f"bin {r['bin_deg']} is finer than the {DEFAULT_QUANT_DEG} quantum — "
        "aggregation would out-resolve the deliberate position blur")


# --- disclosure -------------------------------------------------------------


def test_counts_are_withheld_below_full():
    """SECURITY (disclosure rule 1). Cell counts are denominators."""
    many = [_track(i, lat=0.0 + (i % 40) * 0.01) for i in range(FEATURE_LIMIT + 50)]
    r = query_tracks(many, BOX, STATE)
    assert r["mode"] == "aggregate"
    assert "count" not in r, "total count leaked to a scoped viewer"
    for cell in r["cells"]:
        assert "count" not in cell, "per-cell count leaked to a scoped viewer"


def test_full_viewers_do_get_counts():
    many = [_track(i, lat=0.0 + (i % 40) * 0.01) for i in range(FEATURE_LIMIT + 50)]
    r = query_tracks(many, BOX, FULL)
    assert r["count"] == len(many)
    assert any("count" in c for c in r["cells"])


# --- roll-up semantics ------------------------------------------------------


def test_one_real_contact_makes_a_cell_not_synthetic():
    """A single real contact among decoys must not be labelled simulation —
    that is the one mislabel this surface must never make."""
    many = [_track(i) for i in range(FEATURE_LIMIT + 50)]
    many[7]["synthetic"] = False
    r = query_tracks(many, BOX, FULL)
    assert any(c["synthetic"] is False for c in r["cells"])


def test_cell_verdict_is_worst_not_typical():
    """One failed site among many ready ones must colour the cell."""
    sites = [_Site(f"s{i}", 0.0 + (i % 30) * 0.02, 0.2 + (i % 29) * 0.02)
             for i in range(FEATURE_LIMIT + 20)]
    verdicts = {s.site_id: "ready" for s in sites}
    verdicts["s3"] = "failed"
    r = query_sites(sites, BOX, lambda s: verdicts[s.site_id], FULL)
    assert r["mode"] == "aggregate"
    assert any(c["verdict"] == "failed" for c in r["cells"]), (
        "a failed site was averaged away — the cell an operator most needs "
        "to see is the one that got hidden")


def test_sites_report_anchors_never_precise_positions():
    """The feature path must serve the quantized anchor, not raw coordinates."""
    s = _Site("s1", 0.10, 0.60)
    r = query_sites([s], BOX, lambda x: "ready", FULL)
    assert r["sites"][0]["lat"] == s.anchor[0]
    assert r["sites"][0]["lon"] == s.anchor[1]
