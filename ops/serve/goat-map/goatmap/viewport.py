# SPDX-License-Identifier: Apache-2.0
"""Lasso queries and tiered aggregation — the map stops loading everything.

THE PROBLEM. goat-map rendered every track and every site the moment the page
opened. That is fine for six synthetic contacts and unusable for a real
estate: unreadable on screen, and unloadable over a thin operator link.

THE SHAPE. Blank by default; the operator draws a lasso; the server returns
what is inside it. Three properties follow, and only the first is cosmetic:

  1. The default state transfers nothing.
  2. FILTERING IS SERVER-SIDE. Returning everything and hiding it in the
     browser would mean the process has already shipped the whole picture to
     someone who may not be entitled to it — the exact failure the per-viewer
     scoping work exists to prevent. A lasso is a query, not a view setting.
  3. The payload is BOUNDED. Above a threshold the server aggregates instead
     of enumerating, so a lasso around the whole world costs the same as one
     around a city.

TIERING IS COUNT-DRIVEN, NOT ZOOM-DRIVEN. Zoom is a proxy for "how much will
this return"; the count is the thing itself. A wide lasso over empty ocean
should still show its three contacts individually, and a tight one over a
dense site should still aggregate. One threshold, one rule, no zoom tables.

TWO INVARIANTS INHERITED FROM ELSEWHERE, both load-bearing:

  * AGGREGATE AFTER SCOPING. A cell count computed over estate the viewer
    cannot see discloses that it exists. Same ordering rule as the readiness
    roll-up (scope.py): scope first, summarise second, always.
  * BIN SIZE IS NEVER FINER THAN THE ANCHOR QUANTIZATION. sites.py blurs site
    positions on purpose — infrastructure location is a targeting aid. A fine
    aggregation grid would hand back the precision quantize() removed, by
    letting an observer narrow a site to a sub-quantum cell. The floor is
    enforced in code below, not left to whoever picks a zoom level.
"""

from __future__ import annotations

from typing import Any, Iterable, Sequence

from .disclosure import FULL
from .sites import DEFAULT_QUANT_DEG

# Above this many matches, aggregate instead of enumerating. Chosen for the
# WIRE, not the screen: ~400 GeoTrackV1 records is a few hundred KB, which is
# about one second on a healthy mesh link and a few seconds on the degraded
# operator path. A human cannot read 400 dots anyway, so the display limit and
# the transfer limit happen to agree.
FEATURE_LIMIT = 400

# Aggregation grid, coarsest first. The finest is clamped to the site anchor
# quantization so aggregation can never out-resolve quantize().
_BIN_LADDER_DEG = (10.0, 5.0, 2.0, 1.0, 0.5, 0.2, 0.1)


def _bin_floor() -> float:
    """The finest bin we will ever use.

    Never finer than the site anchor quantum. This is the whole defence
    against using aggregation to triangulate a blurred position: if the cell
    is at least as large as the blur, the cell tells you nothing the anchor
    did not already.
    """
    return max(_BIN_LADDER_DEG[-1], DEFAULT_QUANT_DEG)


def point_in_polygon(lon: float, lat: float, poly: Sequence[Sequence[float]]) -> bool:
    """Ray-casting point-in-polygon. No dependencies, no projection.

    Deliberately planar on lon/lat rather than spherical. A lasso is an
    operator gesture on a rendered map, not a geodesic claim, and the two
    disagree only near the poles and the antimeridian — neither of which any
    goatnet occupies. Pretending to geodesic accuracy here would add a
    dependency and a class of bug in exchange for nothing.
    """
    inside = False
    n = len(poly)
    if n < 3:
        return False
    j = n - 1
    for i in range(n):
        xi, yi = poly[i][0], poly[i][1]
        xj, yj = poly[j][0], poly[j][1]
        if (yi > lat) != (yj > lat):
            denom = (yj - yi) or 1e-12
            if lon < (xj - xi) * (lat - yi) / denom + xi:
                inside = not inside
        j = i
    return inside


def _bin_size_for(count: int, poly: Sequence[Sequence[float]]) -> float:
    """Pick a grid size: coarse enough to bound the cell count, never finer
    than the quantization floor.

    Driven by the lasso's own extent rather than a zoom level, so the result
    is the same whatever the client's viewport happens to be — two operators
    drawing the same shape see the same thing.
    """
    lons = [p[0] for p in poly]
    lats = [p[1] for p in poly]
    span = max(max(lons) - min(lons), max(lats) - min(lats), 1e-6)
    floor = _bin_floor()
    # Aim for roughly a 12x12 grid across the lasso: dense enough to show
    # structure, sparse enough that the response stays small.
    target = span / 12.0
    for size in _BIN_LADDER_DEG:
        if size <= target:
            return max(size, floor)
    return max(_BIN_LADDER_DEG[-1], floor)


def _worst(states: Iterable[str], order: Sequence[str]) -> str:
    """Worst-wins roll-up over a cell.

    Worst rather than typical, for the same reason scope.py keeps the minimum
    cwnd: a cell containing one broken thing and nine healthy ones is a cell
    an operator needs to look at. An average would hide exactly the case the
    surface exists to surface.
    """
    worst = None
    for s in states:
        if s in order and (worst is None or order.index(s) < order.index(worst)):
            worst = s
    return worst or "unknown"


def query_tracks(
    tracks: Iterable[dict[str, Any]],
    poly: Sequence[Sequence[float]],
    tier: str,
) -> dict[str, Any]:
    """Tracks inside the lasso: enumerated if few, binned if many.

    `tracks` MUST already be scope-filtered by the caller. This function does
    not know about viewers and must not — putting the scope check here would
    give two places that decide what a viewer may see, and two places is how
    they drift apart.
    """
    inside = [t for t in tracks
              if t.get("lat") is not None and t.get("lon") is not None
              and point_in_polygon(t["lon"], t["lat"], poly)]

    if len(inside) <= FEATURE_LIMIT:
        return {"mode": "features", "tracks": inside, "count": len(inside)}

    size = _bin_size_for(len(inside), poly)
    cells: dict[tuple[float, float], dict[str, Any]] = {}
    for t in inside:
        key = (round(t["lat"] // size * size, 6), round(t["lon"] // size * size, 6))
        c = cells.setdefault(key, {
            "lat": key[0] + size / 2, "lon": key[1] + size / 2,
            "count": 0, "synthetic": True, "affiliations": set(),
        })
        c["count"] += 1
        # A cell is only "all simulated" if every contact in it is. One real
        # contact in a cell of decoys must not be labelled simulation.
        if not t.get("synthetic"):
            c["synthetic"] = False
        if t.get("affiliation"):
            c["affiliations"].add(t["affiliation"])

    out = []
    for c in cells.values():
        cell = {"lat": c["lat"], "lon": c["lon"], "synthetic": c["synthetic"],
                "affiliation": _worst(c["affiliations"],
                                      ("hostile", "suspect", "unknown",
                                       "pending", "neutral", "assumed-friend",
                                       "friend"))}
        # COUNTS ARE DENOMINATORS (disclosure.py rule 1). A scoped viewer
        # learning that a cell holds 4,000 contacts learns the size of a force
        # they are not cleared to enumerate. Below `full` they get presence
        # and the worst affiliation, which is what supports a decision.
        if tier == FULL:
            cell["count"] = c["count"]
        out.append(cell)

    return {"mode": "aggregate", "cells": out, "bin_deg": size,
            **({"count": len(inside)} if tier == FULL else {})}


def query_sites(
    sites: Iterable[Any],
    poly: Sequence[Sequence[float]],
    verdict_of: Any,
    tier: str,
) -> dict[str, Any]:
    """Site readiness inside the lasso, same two tiers.

    `sites` MUST already be scope-filtered (scope.scope_estate). `verdict_of`
    maps a site to its readiness verdict so this module stays ignorant of the
    readiness chain — it aggregates verdicts, it does not compute them.

    Site positions come from `anchor`, never precise coordinates: sites.py
    quantizes on purpose and this path must not reintroduce what it removed.
    """
    inside = []
    for s in sites:
        lat, lon = s.anchor
        if point_in_polygon(lon, lat, poly):
            inside.append(s)

    order = ("failed", "unknown", "at-rest", "degraded", "ready")

    if len(inside) <= FEATURE_LIMIT:
        return {"mode": "features",
                "sites": [{"site_id": s.site_id, "label": s.label,
                           "lat": s.anchor[0], "lon": s.anchor[1],
                           "verdict": verdict_of(s)} for s in inside],
                "count": len(inside)}

    size = _bin_size_for(len(inside), poly)
    cells: dict[tuple[float, float], dict[str, Any]] = {}
    for s in inside:
        lat, lon = s.anchor
        key = (round(lat // size * size, 6), round(lon // size * size, 6))
        c = cells.setdefault(key, {"lat": key[0] + size / 2,
                                   "lon": key[1] + size / 2,
                                   "count": 0, "verdicts": []})
        c["count"] += 1
        c["verdicts"].append(verdict_of(s))

    out = []
    for c in cells.values():
        cell = {"lat": c["lat"], "lon": c["lon"],
                "verdict": _worst(c["verdicts"], order)}
        if tier == FULL:
            cell["count"] = c["count"]
        out.append(cell)

    return {"mode": "aggregate", "cells": out, "bin_deg": size,
            **({"count": len(inside)} if tier == FULL else {})}
