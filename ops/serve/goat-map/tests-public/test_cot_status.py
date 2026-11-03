# SPDX-License-Identifier: Apache-2.0
"""Non-positional CoT — sensor status, not a contact.

The fixtures are invented, in the shape a radar chain publishes. The alert shape
below must not count as `unmappable`, which would read as a broken radar when the
radar is working fine.

The distinction being tested is "cannot be drawn as a dot" vs "is not worth
knowing". Only the first is true of a jamming report.
"""

from __future__ import annotations

import sys
from pathlib import Path

sys.path.insert(0, str(Path(__file__).resolve().parents[1]))

from goatmap import cot  # noqa: E402

SOURCE = {"sensor": "radar-a", "site": "site-a", "node": "node-a"}
ANCHOR = "1bb8194d38f7ea26ef94e7b7efc5c907"

# Invented radar-a/cot/alert.
ALERT = (
    b'<?xml version="1.0" standalone="yes"?>'
    b'<event type="a-h-G-U-C-I" how="m-r" version="2.0">'
    b'<point ce="9999999" le="9999999"/>'
    b'<detail><uid Droid="radar/radar-a:Jamming Status"/></detail></event>'
)

# Invented radar-a/cot/track.
TRACK = (
    b'<?xml version="1.0" standalone="yes"?>'
    b'<event time="1970-01-01T00:06:46.844Z" start="1970-01-01T00:06:46.844Z" '
    b'stale="1970-01-01T00:09:46.844Z" type="a-u-X" uid="2" how="m-p" version="2.0">'
    b'<point lat="0.14612078819247" lon="0.726832671190834" hae="16.4099178314209" '
    b'ce="9999999" le="9999999"/>'
    b'<detail><uid Droid="radar/radar-a:Tracks with Associated Plot Data:2" tadilj="2"/>'
    b'<track course="177" speed="26.51361388595204" version="0.1"/></detail></event>'
)


def _status(payload):
    return cot.parse_status(
        payload, kind="alert", source=SOURCE, producer_anchor=ANCHOR
    )


def _track(payload):
    return cot.parse(payload, kind="track", source=SOURCE, producer_anchor=ANCHOR)


# --- the alert is status, not a discard ------------------------------------


def test_jamming_alert_is_recognised_as_status():
    s = _status(ALERT)
    assert s is not None, "the jamming alert must not be silently dropped"
    assert s["label"] == "Jamming Status"
    assert s["schema"] == cot.SCHEMA_STATUS


def test_alert_is_not_a_track():
    """It has no lat/lon, so the positional parser must still refuse it —
    otherwise it would be drawn at null island."""
    assert _track(ALERT) is None


def test_status_label_survives_an_unfamiliar_producer():
    """No `Droid` attribute at all: name it from the CoT type rather than
    rendering a blank row, so an unknown status is visible as unknown."""
    s = _status(b'<event type="a-h-G-U-C-I" how="m-r"><point ce="9999999"/></event>')
    assert s is not None and s["label"] == "a-h-G-U-C-I"


# --- the two paths must never both claim a message -------------------------


def test_positional_track_is_refused_by_the_status_parser():
    """SECURITY-ADJACENT HONESTY: if both paths accepted a message it would be
    counted twice and shown twice, in two different shapes."""
    assert _status(TRACK) is None


def test_track_still_parses_with_position():
    t = _track(TRACK)
    assert t is not None
    assert round(t["lat"], 6) == 0.146121
    assert round(t["lon"], 6) == 0.726833
    assert t["course_deg"] == 177
    assert t["schema"] == cot.SCHEMA


def test_the_two_schemas_are_distinct():
    """A consumer asking for tracks must never receive a status row and try to
    plot it."""
    assert cot.SCHEMA != cot.SCHEMA_STATUS


# --- the 1970 clock is the RADAR's, and it is uptime not a date ------------


def test_sensor_clock_is_preserved_verbatim_and_not_trusted():
    """A chain can stamp `1970-01-01T00:06:46` — 6m46s past the epoch,
    i.e. time-since-boot written into a UTC field. It is kept verbatim as
    `reported_at` for forensics and never used for ordering or staleness;
    `observed_at` is receipt and is what the UI ages from."""
    t = _track(TRACK)
    assert t["reported_at"] == "1970-01-01T00:06:46.844Z"
    assert t["observed_at"] != t["reported_at"]
    assert t["observed_at"].startswith("20")


def test_malformed_xml_is_neither_track_nor_status():
    """A genuine parse failure must still land in `unmappable` — the point of
    this change is to stop conflating status with broken, not to stop counting
    broken."""
    assert _track(b"<not-xml") is None
    assert _status(b"<not-xml") is None
