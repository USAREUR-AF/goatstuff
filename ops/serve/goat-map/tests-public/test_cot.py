# SPDX-License-Identifier: Apache-2.0
"""Tests for the CoT -> GeoTrackV1 transform.

`goatmap/cot.py` is the one piece of this service with real logic and no I/O,
so it is the piece worth pinning. The fixtures below are invented, in the shape
a radar chain publishes -- including 1970 timestamps and an empty-point jamming
message, both of which the transform has to survive.
"""

from __future__ import annotations

import sys
from pathlib import Path

import pytest

sys.path.insert(0, str(Path(__file__).resolve().parents[1]))

from goatmap import cot  # noqa: E402

SOURCE = {"sensor": "radar-a", "site": "site-a", "node": "node-a"}
# Stand-in for this service's own anchor; the ingest leg attributes to itself.
ANCHOR = "1bb8194d38f7ea26ef94e7b7efc5c907"

# Invented: radar-a/cot/track
TRACK = b"""<?xml version="1.0" standalone="yes"?><event time="1970-01-01T02:47:21.682Z" \
start="1970-01-01T02:47:21.682Z" stale="1970-01-01T02:50:21.682Z" type="a-u-X" uid="177" \
how="m-p" version="2.0"><point lat="0.144703641604" lon="0.6869426472395" \
hae="16.7187347412109" ce="9999999" le="9999999"/><detail><uid \
Droid="radar/radar-a:Tracks with Associated Plot Data:177" tadilj="177"/><track course="65" \
speed="12.8682579750276"/></detail></event>"""

# Invented: radar-a/cot/alert -- a jamming status. Parses fine, has a
# <point> element, and carries no position at all.
JAMMING = b"""<?xml version="1.0" standalone="yes"?><event type="a-h-G-U-C-I" how="m-r" \
version="2.0"><point ce="9999999" le="9999999"/><detail><uid \
Droid="radar/radar-a:Jamming Status"/></detail></event>"""


def parse_track(payload: bytes = TRACK, kind: str = "track"):
    return cot.parse(payload, kind=kind, source=SOURCE, producer_anchor=ANCHOR)


class TestPosition:
    def test_extracts_position(self):
        t = parse_track()
        assert t["lat"] == pytest.approx(0.144703641604)
        assert t["lon"] == pytest.approx(0.6869426472395)
        assert t["alt_m"] == pytest.approx(16.7187347412109)

    def test_no_position_is_dropped_not_placed_at_null_island(self):
        # The single most important behaviour here: a message with no lat/lon
        # must not become a dot at 0,0 off the coast of Africa.
        assert cot.parse(JAMMING, kind="alert", source=SOURCE, producer_anchor=ANCHOR) is None

    @pytest.mark.parametrize("bad", [b"", b"not xml at all", b"<event><point/>", b"<other/>"])
    def test_unparseable_returns_none_rather_than_raising(self, bad):
        assert cot.parse(bad, kind="track", source=SOURCE, producer_anchor=ANCHOR) is None

    def test_non_numeric_position_is_dropped(self):
        payload = TRACK.replace(b'lat="0.144703641604"', b'lat="north-ish"')
        assert cot.parse(payload, kind="track", source=SOURCE, producer_anchor=ANCHOR) is None


class TestIdentity:
    def test_id_is_namespaced_by_sensor_label_stays_raw(self):
        t = parse_track()
        assert t["id"] == "radar-a:177"
        assert t["label"] == "177"

    def test_kind_comes_from_the_topic_not_the_payload(self):
        # CoT carries no track/alert distinction; the key leaf supplies it.
        assert parse_track(kind="alert")["kind"] == "alert"

    def test_source_is_carried_through(self):
        assert parse_track()["source"] == SOURCE


class TestCotTypeDecoding:
    @pytest.mark.parametrize(
        "cot_type,affiliation,dimension",
        [
            ("a-u-X", "unknown", "other"),
            ("a-f-A", "friend", "air"),
            ("a-h-G-U-C-I", "hostile", "ground"),
            ("a-n-S", "neutral", "surface"),
            ("a-p-U", "pending", "subsurface"),
        ],
    )
    def test_letters_resolve_server_side(self, cot_type, affiliation, dimension):
        payload = TRACK.replace(b'type="a-u-X"', f'type="{cot_type}"'.encode())
        t = cot.parse(payload, kind="track", source=SOURCE, producer_anchor=ANCHOR)
        assert (t["affiliation"], t["dimension"]) == (affiliation, dimension)

    def test_unrecognised_letters_degrade_to_unknown_not_crash(self):
        payload = TRACK.replace(b'type="a-u-X"', b'type="a-Z-Q"')
        t = cot.parse(payload, kind="track", source=SOURCE, producer_anchor=ANCHOR)
        assert t["affiliation"] == "unknown"
        assert t["dimension"] == "unknown"

    def test_missing_type_degrades(self):
        payload = TRACK.replace(b'type="a-u-X" ', b"")
        t = cot.parse(payload, kind="track", source=SOURCE, producer_anchor=ANCHOR)
        assert t["affiliation"] == "unknown"


class TestKinematics:
    def test_course_and_speed_from_detail_track(self):
        t = parse_track()
        assert t["course_deg"] == pytest.approx(65.0)
        assert t["speed_mps"] == pytest.approx(12.8682579750276)

    def test_absent_detail_track_yields_none_not_zero(self):
        # None means "not reported"; 0 would mean "stationary". The map draws
        # no heading leader for the first and a stationary dot for the second.
        payload = TRACK.replace(b'<track course="65" speed="12.8682579750276"/>', b"")
        t = cot.parse(payload, kind="track", source=SOURCE, producer_anchor=ANCHOR)
        assert t["course_deg"] is None
        assert t["speed_mps"] is None


class TestErrorEstimates:
    def test_cot_unknown_sentinel_becomes_none(self):
        # 9999999 is CoT's "no usable estimate", not a 10 000 km error circle.
        t = parse_track()
        assert t["ce_m"] is None
        assert t["le_m"] is None

    def test_real_error_estimate_is_kept(self):
        payload = TRACK.replace(b'ce="9999999"', b'ce="25.5"')
        t = cot.parse(payload, kind="track", source=SOURCE, producer_anchor=ANCHOR)
        assert t["ce_m"] == pytest.approx(25.5)


class TestClocks:
    def test_sensor_clock_is_preserved_verbatim(self):
        # This fixture reports 1970. The transform must not silently
        # repair it -- downstream honesty depends on seeing it unchanged.
        assert parse_track()["reported_at"] == "1970-01-01T02:47:21.682Z"

    def test_observed_at_is_receipt_time_and_is_utc(self):
        t = parse_track()
        assert t["observed_at"].endswith("Z")
        assert t["observed_at"] > "2026-01-01T00:00:00Z"

    def test_the_two_clocks_are_separate_fields(self):
        t = parse_track()
        assert t["reported_at"] != t["observed_at"]


class TestSchema:
    def test_schema_tag_present(self):
        assert parse_track()["schema"] == "goat.geo.track.v1"

    def test_every_documented_field_is_emitted(self):
        # Guards against a field being dropped and the front end silently
        # rendering a blank cell.
        expected = {
            "schema", "id", "label", "kind", "source", "lat", "lon", "alt_m",
            "course_deg", "speed_mps", "ce_m", "le_m", "cot_type",
            "affiliation", "dimension", "how", "descriptor", "reported_at",
            "stale_at", "observed_at", "producer_anchor", "synthetic", "org",
        }
        assert expected == set(parse_track())

    def test_output_is_json_serialisable(self):
        import json

        json.dumps(parse_track())
