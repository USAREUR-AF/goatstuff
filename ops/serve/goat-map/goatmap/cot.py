# SPDX-License-Identifier: Apache-2.0
"""Cursor-on-Target XML -> GeoTrackV1.

A sensor chain publishes CoT 2.0 events onto Zenoh
(`<sensor>/cot/track`, `<sensor>/cot/alert`). CoT is a reasonable wire format and a
poor render format: XML, MIL-STD type codes, a `detail` grab-bag. This module
is the whole translation layer -- CoT in, one flat JSON object out.

GeoTrackV1 is deliberately flat and display-shaped: every field is either
directly drawable or directly printable. Where a field needs a lookup table to
mean anything, the lookup happens here, never in the browser.
"""

from __future__ import annotations

import math
import xml.etree.ElementTree as ET
from datetime import datetime, timezone
from typing import Any

SCHEMA = "goat.geo.track.v1"
# Non-positional sensor status. Deliberately a DIFFERENT schema id: a consumer
# that asks for tracks must never be handed a status row it will try to plot.
SCHEMA_STATUS = "goat.sensor.status.v1"

# CoT type is `a-<affiliation>-<dimension>[-...]`. Both letters are lookup
# codes; resolving them here is what keeps the front end free of CoT knowledge.
_AFFILIATION = {
    "p": "pending",
    "u": "unknown",
    "a": "assumed-friend",
    "f": "friend",
    "n": "neutral",
    "s": "suspect",
    "h": "hostile",
    "j": "joker",
    "k": "faker",
    "o": "none",
    "x": "other",
}

_DIMENSION = {
    "P": "space",
    "A": "air",
    "G": "ground",
    "S": "surface",
    "U": "subsurface",
    "F": "sof",
    "X": "other",
}

# CoT's "no usable estimate" sentinel for circular/linear error.
_ERROR_UNKNOWN = 9999999.0


def _f(value: str | None) -> float | None:
    if value is None:
        return None
    try:
        out = float(value)
    except ValueError:
        return None
    return None if math.isnan(out) else out  # drop NaN


def now_iso() -> str:
    return datetime.now(timezone.utc).isoformat(timespec="milliseconds").replace("+00:00", "Z")


def parse(
    xml_bytes: bytes,
    *,
    kind: str,
    source: dict[str, str],
    producer_anchor: str,
    synthetic: bool = False,
    org: str | None = None,
) -> dict[str, Any] | None:
    """Parse one CoT event. Returns None for anything not renderable on a map.

    `kind` is the topic leaf (`track` / `alert`) -- CoT itself carries no such
    distinction, and the map draws the two differently.

    `producer_anchor` is the anchor of whoever is REPUBLISHING this as
    GeoTrackV1 -- i.e. this service -- not the sensor. CoT arriving over an
    unauthenticated transport carries no verifiable producer identity, so the
    only honest attribution is "this service vouched for it"; the sensor chain
    it came from is recorded in `source`, which is descriptive, not an
    authorisation input.
    """
    try:
        event = ET.fromstring(xml_bytes)
    except ET.ParseError:
        return None
    if event.tag != "event":
        return None

    point = event.find("point")
    if point is None:
        return None
    lat = _f(point.get("lat"))
    lon = _f(point.get("lon"))
    # A track with no position is not a dot on a map. Drop it rather than
    # render it at null island.
    #
    # NOT every such message is broken, and the caller must tell the two apart
    # — see `parse_status`. A sensor may emit `<sensor>/cot/alert` with a bare
    # `<point ce="9999999" le="9999999"/>` carrying no lat/lon at all, because
    # it is a SENSOR STATUS (e.g. "Jamming Status"), not a contact. Counting
    # those as unmappable reads as a broken radar when it is working correctly.
    if lat is None or lon is None:
        return None

    cot_type = event.get("type", "")
    parts = cot_type.split("-")
    affiliation = _AFFILIATION.get(parts[1] if len(parts) > 1 else "", "unknown")
    dimension = _DIMENSION.get(parts[2] if len(parts) > 2 else "", "unknown")

    track = event.find("./detail/track")
    detail_uid = event.find("./detail/uid")

    uid = event.get("uid") or ""
    sensor = source.get("sensor", "sensor")

    ce = _f(point.get("ce"))
    le = _f(point.get("le"))

    return {
        "schema": SCHEMA,
        # Unique across sensors; the raw CoT uid stays as the human label.
        "id": f"{sensor}:{uid}",
        "label": uid,
        "kind": kind,
        "source": source,
        "lat": lat,
        "lon": lon,
        "alt_m": _f(point.get("hae")),
        "course_deg": _f(track.get("course")) if track is not None else None,
        "speed_mps": _f(track.get("speed")) if track is not None else None,
        "ce_m": None if ce is None or ce >= _ERROR_UNKNOWN else ce,
        "le_m": None if le is None or le >= _ERROR_UNKNOWN else le,
        "cot_type": cot_type,
        "affiliation": affiliation,
        "dimension": dimension,
        "how": event.get("how"),
        "descriptor": detail_uid.get("Droid") if detail_uid is not None else None,
        # Two clocks, never merged. `reported_at` is whatever the sensor chain
        # claimed; `observed_at` is when this process received the bytes. A
        # sensor clock can be arbitrarily wrong, so anything keyed on the
        # sensor clock (staleness, ordering) uses observed_at -- and the UI
        # says which one it is showing.
        "reported_at": event.get("time"),
        "stale_at": event.get("stale"),
        "observed_at": now_iso(),
        "producer_anchor": producer_anchor,
        "synthetic": synthetic,
        "org": org,
    }


# --- non-positional messages -------------------------------------------------

def parse_status(
    xml_bytes: bytes, *, kind: str, source: dict, producer_anchor: str,
    synthetic: bool = False, org: str | None = None,
) -> dict | None:
    """A CoT event that carries NO position — sensor status, not a contact.

    WHY THIS EXISTS. `parse` drops anything without lat/lon, which is right for
    a map. But a sensor's `cot/alert` stream can be exactly that shape, and it
    is not noise: a typical payload is

        <event type="a-h-G-U-C-I" how="m-r">
          <point ce="9999999" le="9999999"/>
          <detail><uid Droid="radar/sensor-a:Jamming Status"/></detail>
        </event>

    A status indication can be the most operationally significant thing a
    sensor reports, and it must not land in a discard counter. "Cannot be
    drawn as a dot" and "is not worth knowing" are different claims, and only
    the first one is true here.

    Returns None for anything that IS positional (that is `parse`'s job) or
    that does not parse, so the two paths never both claim a message.
    """
    try:
        event = ET.fromstring(xml_bytes)
    except ET.ParseError:
        return None
    if event.tag != "event":
        return None

    point = event.find("point")
    if point is not None and _f(point.get("lat")) is not None \
            and _f(point.get("lon")) is not None:
        return None  # positional — belongs to parse()

    detail_uid = event.find("./detail/uid")
    droid = detail_uid.get("Droid") if detail_uid is not None else None
    # `radar/sensor-a:Jamming Status` -> `Jamming Status`. The label is the half
    # after the last colon; without one, fall back to the whole string so an
    # unfamiliar producer still names itself rather than rendering blank.
    label = droid.rsplit(":", 1)[-1].strip() if droid else (event.get("type") or "status")

    cot_type = event.get("type", "")
    parts = cot_type.split("-")

    return {
        "schema": SCHEMA_STATUS,
        "id": f"{source.get('sensor', 'sensor')}:status:{label}",
        "label": label,
        "kind": kind,
        "source": source,
        "cot_type": cot_type,
        "affiliation": _AFFILIATION.get(parts[1] if len(parts) > 1 else "", "unknown"),
        "how": event.get("how"),
        "descriptor": droid,
        "reported_at": event.get("time"),
        "observed_at": now_iso(),
        "producer_anchor": producer_anchor,
        "synthetic": synthetic,
        "org": org,
    }
