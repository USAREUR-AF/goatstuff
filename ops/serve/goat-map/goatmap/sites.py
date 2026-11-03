# SPDX-License-Identifier: Apache-2.0
"""Site inventory and anchor quantization — step 1 of site availability.

See docs/design/goat-map-site-availability.md. This answers "what capability
exists at each site", which the track view cannot: tracks are what we DETECT,
and a sensor that is powered off produces none while the site is perfectly fine.

Two things live here, and the first is a security property rather than a UI
choice.

QUANTIZATION HAPPENS HERE, NOT IN THE BROWSER. A map that pinpoints
infrastructure is a targeting aid, so systems render at a coarse site anchor and
the precise coordinate never leaves the server. A browser that never receives it
cannot leak it, and neither can a screenshot. The asymmetry against tracks is
deliberate: what we detect is precise, where we are is not — a detection blurred
is useless, an emplacement blurred is the point.

`quantize()` is therefore not a rounding helper. It is the boundary, and the
only function that should ever see a precise site coordinate.
"""

from __future__ import annotations

import math
from dataclasses import dataclass, field
from typing import Any, Iterable

# Degrees. ~0.05° is roughly 5.5 km of latitude — coarse enough that a rendered
# marker cannot be walked back to a building, fine enough that two sites in
# different towns stay distinguishable. Longitude convergence at high latitude
# makes the ground distance SMALLER there, never larger, so this is a floor on
# blur rather than a promise of it.
DEFAULT_QUANT_DEG = 0.05

# Worst-state ordering. Aggregation carries the WORST state, never an average:
# one red system in a green site makes the site red, because the purpose is
# noticing trouble rather than scoring it. A mean would hide precisely the
# single failure this surface exists to surface.
_SEVERITY = {"unmetered": -1, "green": 0, "idle": 1, "amber": 2, "red": 3}


def quantize(lat: float, lon: float, step_deg: float = DEFAULT_QUANT_DEG) -> tuple[float, float]:
    """Snap a coordinate to a coarse grid, at the publisher.

    Rounds to the nearest grid cell centre rather than truncating: truncation
    biases every site consistently toward the origin, which over a cluster of
    sites leaks the direction of the true positions.
    """
    if step_deg <= 0:
        raise ValueError("step_deg must be positive")
    qlat = math.floor(lat / step_deg) * step_deg + step_deg / 2
    qlon = math.floor(lon / step_deg) * step_deg + step_deg / 2
    # Round to a sane number of decimals so the wire form does not imply
    # survey precision it does not have.
    return (round(qlat, 4), round(qlon, 4))


def worst(states: Iterable[str]) -> str:
    """Worst state among the inputs; `unmetered` when there is nothing to judge.

    `unmetered` loses to every grounded state — a site with one unreadable
    system and three green ones is green with a caveat, not unknown — but wins
    when it is all we have, because "we cannot see" must never render as green.
    """
    ranked = [_SEVERITY.get(s, -1) for s in states]
    grounded = [r for r in ranked if r >= 0]
    if not grounded:
        return "unmetered" if ranked else "unmetered"
    top = max(grounded)
    for name, rank in _SEVERITY.items():
        if rank == top:
            return name
    return "unmetered"


@dataclass(frozen=True)
class System:
    """One thing at a site: a sensor, a router, a pod, a node."""

    name: str
    cls: str                      # radar | relay | node | pod | router
    peer: str = ""                # netbird FQDN, the join key to liveness
    state: str = "unmetered"      # filled by the health join (step 2)
    # Topology within the site: a router carries nodes, a node carries sensors.
    # Empty = a root of the site's tree. This is a DECLARED relationship, not an
    # inferred one — inferring parentage from traffic would make the picture
    # rearrange itself whenever something went quiet.
    parent: str = ""


@dataclass
class Site:
    site_id: str
    label: str
    lat: float                    # PRECISE — never serialised; see as_dict()
    lon: float
    # Declared region membership — after the region model, this is the ONLY
    # geographic disclosure a site makes. See layout.py.
    region_id: str = ""
    systems: list[System] = field(default_factory=list)
    quant_deg: float = DEFAULT_QUANT_DEG

    @property
    def anchor(self) -> tuple[float, float]:
        return quantize(self.lat, self.lon, self.quant_deg)

    @property
    def state(self) -> str:
        return worst(s.state for s in self.systems)

    def counts(self) -> dict[str, int]:
        """Counts ride alongside the state so a cluster is never a featureless dot."""
        out: dict[str, int] = {"total": len(self.systems)}
        degraded = sum(1 for s in self.systems if _SEVERITY.get(s.state, -1) >= 2)
        if degraded:
            out["degraded"] = degraded
        return out

    def as_dict(self) -> dict[str, Any]:
        """The wire form. Carries the ANCHOR only — the precise coordinate stays here."""
        lat, lon = self.anchor
        return {
            "site_id": self.site_id,
            "label": self.label,
            "lat": lat,
            "lon": lon,
            # Named so the UI can say the position is approximate rather than
            # implying survey accuracy it does not have.
            "precision_deg": self.quant_deg,
            "state": self.state,
            "counts": self.counts(),
            "systems": [
                {"name": s.name, "class": s.cls, "state": s.state,
                 "parent": s.parent or None}
                for s in self.systems
            ],
        }


def load_sites(spec: list[dict[str, Any]]) -> list[Site]:
    """Build the inventory from config (a declared fact, not discovered).

    Site membership is declarative for the same reason peer classes are: infer
    it from behaviour and an absent system silently stops being part of the
    site it belongs to, which is the failure this whole surface exists to close.
    """
    sites: list[Site] = []
    for row in spec:
        sites.append(
            Site(
                site_id=row["site_id"],
                label=row.get("label") or row["site_id"],
                region_id=row.get("region_id", ""),
                lat=float(row["lat"]),
                lon=float(row["lon"]),
                quant_deg=float(row.get("quant_deg", DEFAULT_QUANT_DEG)),
                systems=[
                    System(
                        name=s["name"],
                        cls=s.get("class", "node"),
                        peer=s.get("peer", ""),
                        parent=s.get("parent", ""),
                    )
                    for s in row.get("systems", [])
                ],
            )
        )
    return sites
