# SPDX-License-Identifier: Apache-2.0
import sys
from pathlib import Path

sys.path.insert(0, str(Path(__file__).resolve().parents[1]))
from goatmap import adsb, classes  # noqa: E402

BOX = (40.0, 20.0, 50.0, 30.0)


def ac(**kw):
    return {"hex": "abc123", "flight": "TST1 ", "lat": 45.0, "lon": 25.0, "seen_pos": 1.0,
            "alt_baro": 10000, "gs": 100, "track": 90, "nic": 8, **kw}


def test_civil_is_commercial_and_military_is_unknown():
    civil, mil = adsb.tracks({"now": 1795000000000, "ac": [ac(), ac(hex="b", dbFlags=1)]}, BOX)
    assert civil["label"] == "TST1" and civil["alt_m"] == 3048.0 and civil["speed_mps"] == 51.4
    assert classes.from_message(civil) == "commercial"
    assert classes.from_message(mil) == "unknown"


def test_what_is_left_out():
    rows = [ac(lat=60), ac(seen_pos=31), ac(alt_baro="ground"), ac(nic=0), ac(lat=None), ac(hex=""), ac(seen_pos=None)]
    assert adsb.tracks({"ac": rows}, BOX) == []
    assert adsb.tracks({}, BOX) == []


def test_pace_follows_the_quota():
    day = {"x-ratelimit-remaining": "480", "x-ratelimit-reset": "86400"}
    assert adsb.next_wait(5, 200, day) == 180.0
    assert adsb.next_wait(5, 200, {"x-ratelimit-remaining": "499, 500;w=86400", "x-ratelimit-reset": "100"}) == 5
    assert adsb.next_wait(5, 200, {"x-ratelimit-remaining": "0", "x-ratelimit-reset": "3600"}) == 3600
    assert adsb.next_wait(5, 200, {}) == 5
    assert adsb.next_wait(5, 429, {"Retry-After": "900"}) == 900
    assert adsb.next_wait(5, 429, {}) == 600
