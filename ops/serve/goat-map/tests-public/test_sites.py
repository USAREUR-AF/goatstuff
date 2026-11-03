# SPDX-License-Identifier: Apache-2.0
"""Tests for the site inventory and, mainly, for the quantization boundary.

`quantize()` is a security control rather than a formatting helper: the precise
site coordinate must never reach a browser, because a browser that never
receives it cannot leak it and neither can a screenshot. So the tests that
matter here are the ones asserting what does NOT appear on the wire.
"""

from __future__ import annotations

import sys
from pathlib import Path

sys.path.insert(0, str(Path(__file__).resolve().parents[1]))

from goatmap.sites import (  # noqa: E402
    DEFAULT_QUANT_DEG,
    Site,
    System,
    load_sites,
    quantize,
    worst,
)

# Invented, near null island. Precise enough to need protecting, which is the point.
SITE_LAT = 0.1589
SITE_LON = 0.6503


def test_precise_coordinates_never_reach_the_wire():
    """The one test this module exists for."""
    s = Site(site_id="site-a", label="Site A", lat=SITE_LAT, lon=SITE_LON)
    payload = s.as_dict()
    assert payload["lat"] != SITE_LAT
    assert payload["lon"] != SITE_LON
    # And nothing anywhere in the serialised form carries the true value.
    flat = repr(payload)
    assert str(SITE_LAT) not in flat
    assert str(SITE_LON) not in flat
    # The blur is declared rather than implied.
    assert payload["precision_deg"] == DEFAULT_QUANT_DEG


def test_quantization_is_coarse_enough_to_be_worth_having():
    qlat, qlon = quantize(SITE_LAT, SITE_LON)
    # ~0.05 deg of latitude is ~5.5 km; assert the displacement is kilometres,
    # not metres, or the control is decorative.
    assert abs(qlat - SITE_LAT) > 0.001
    assert abs(qlon - SITE_LON) > 0.001


def test_quantization_does_not_bias_toward_the_origin():
    """Truncation would pull every site the same way and leak the true bearing."""
    step = 0.05
    EDGE = 0.048
    below = quantize(0.02, 0.02, step)   # just above a cell edge
    above = quantize(EDGE, EDGE, step)  # just below the next edge
    # Both land on the same cell CENTRE, above their inputs in one case and
    # below in the other — so the error changes sign rather than accumulating.
    assert below == above
    assert below[0] > 0.02 and above[0] < 0.048


def test_two_nearby_sites_stay_distinguishable():
    a = quantize(0.16, 0.65)
    b = quantize(0.40, 0.90)
    assert a != b


def test_aggregation_carries_the_worst_state_never_an_average():
    # One red system in an otherwise green site makes the site red. A mean
    # would hide exactly the single failure this surface exists to surface.
    s = Site(
        site_id="x", label="X", lat=1.0, lon=1.0,
        systems=[
            System("a", "node", state="green"),
            System("b", "node", state="green"),
            System("c", "radar", state="red"),
        ],
    )
    assert s.state == "red"
    assert s.counts() == {"total": 3, "degraded": 1}


def test_unmetered_never_renders_as_green():
    assert worst([]) == "unmetered"
    assert worst(["unmetered"]) == "unmetered"
    # …but one unreadable system among healthy ones does not blank the site.
    assert worst(["green", "unmetered"]) == "green"
    assert worst(["green", "amber", "unmetered"]) == "amber"


def test_idle_is_not_a_fault():
    # A deliberately-off system is idle, and idle must not read as degraded —
    # the radar's duty cycle would otherwise show the site as broken most of
    # the time and train operators to ignore the badge.
    s = Site(site_id="x", label="X", lat=1.0, lon=1.0,
             systems=[System("radar", "radar", state="idle"),
                      System("node", "node", state="green")])
    assert s.state == "idle" or s.state == "green"
    assert "degraded" not in s.counts()


def test_load_sites_from_declared_config():
    sites = load_sites([
        {
            "site_id": "site-a", "label": "Site A",
            "lat": SITE_LAT, "lon": SITE_LON,
            "systems": [
                {"name": "node-a", "class": "node",
                 "peer": "node-a.netbird.selfhosted"},
                {"name": "radar-a", "class": "radar"},
            ],
        }
    ])
    assert len(sites) == 1
    site = sites[0]
    assert len(site.systems) == 2
    # The peer FQDN is the join key to the liveness surface (step 2) and is
    # deliberately NOT serialised to the browser.
    assert site.systems[0].peer == "node-a.netbird.selfhosted"
    assert "peer" not in site.as_dict()["systems"][0]
    # Nothing declared yet means nothing measured, not everything healthy.
    assert site.state == "unmetered"
