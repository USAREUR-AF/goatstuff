# SPDX-License-Identifier: Apache-2.0
"""Region membership + non-geographic layout within a region.

See docs/design/goat-map-site-availability.md §"Regions, and why position stops
being geographic".

THE SECURITY MODEL CHANGED HERE, and in the strengthening direction. The first
cut rendered each site at a quantized version of its true coordinate, so the
blur was the only thing standing between a viewer and the real emplacement.
Now a site is placed by its REGION only, and within that region its position is
computed from a deterministic layout that has no relationship to where it
actually is. Region membership becomes the entire geographic disclosure, and no
amount of zooming, screenshotting or coordinate arithmetic recovers more —
because more was never sent.

Quantization survives for the region ANCHOR (a region is a real area and has to
sit somewhere on the backdrop), not for sites.

WHY TWO LAYOUTS, not one:

  - Sites within a region are PEERS. They have no parent/child relation, so a
    tree would draw hierarchy that does not exist and invite an operator to read
    meaning into depth. A grid asserts nothing except "these are the members",
    packs densely, and stays stable as members come and go.

  - Systems within a site DO have topology — a site router carries nodes, a node
    carries sensors. A tree there shows something true, and the edges are the
    information. A grid would throw that away.

Both are deterministic: the same inputs always produce the same positions, so a
marker does not migrate between reloads and a deep link stays valid.
"""

from __future__ import annotations

import math
from dataclasses import dataclass, field
from typing import Any, Iterable

# Layout units are ABSTRACT — a unit square per cell, origin top-left. The
# client scales them into the region's on-screen box. Deliberately not degrees:
# emitting anything degree-shaped invites someone to treat it as a position.
CELL = 1.0


@dataclass(frozen=True)
class Placed:
    """A laid-out item: abstract x/y, never a coordinate."""

    key: str
    x: float
    y: float
    depth: int = 0
    parent: str | None = None


def grid(keys: Iterable[str], aspect: float = 1.0) -> list[Placed]:
    """Row-major placement on a near-square grid, ordered by key.

    Ordered by key rather than by arrival so the layout is stable across
    restarts and across replicas — an operator's spatial memory of "the one
    top-left" has to survive a redeploy or it is worse than useless.

    `aspect` biases the grid wider (>1) for landscape region boxes.
    """
    ks = sorted(keys)
    n = len(ks)
    if n == 0:
        return []
    cols = max(1, int(math.ceil(math.sqrt(n * aspect))))
    out: list[Placed] = []
    for i, k in enumerate(ks):
        out.append(Placed(key=k, x=(i % cols) * CELL, y=(i // cols) * CELL))
    return out


def tree(
    nodes: Iterable[tuple[str, str | None]],
    gap_x: float = CELL,
    gap_y: float = CELL,
) -> list[Placed]:
    """Tidy layered tree: depth on y, siblings spread on x, parents centred.

    Input is (key, parent_key) pairs; a parent that is missing from the set is
    treated as a root, so a partially-declared inventory still draws instead of
    raising. Cycles are broken by depth-capping rather than recursing forever —
    a malformed declaration must not take the map down.
    """
    pairs = sorted(nodes)
    parent_of = {k: p for k, p in pairs}
    children: dict[str | None, list[str]] = {}
    for k, p in pairs:
        eff = p if p in parent_of else None
        children.setdefault(eff, []).append(k)
    for v in children.values():
        v.sort()

    placed: dict[str, Placed] = {}
    cursor = 0.0
    max_depth = max(8, len(pairs))  # cycle guard, generous enough to be inert

    def walk(key: str, depth: int) -> float:
        """Place a subtree; return this node's x (centre of its children)."""
        nonlocal cursor
        if key in placed or depth > max_depth:
            return placed[key].x if key in placed else cursor
        kids = children.get(key, [])
        if not kids:
            x = cursor
            cursor += gap_x
        else:
            xs = [walk(c, depth + 1) for c in kids]
            x = (min(xs) + max(xs)) / 2
        placed[key] = Placed(
            key=key, x=x, y=depth * gap_y, depth=depth, parent=parent_of.get(key) or None
        )
        return x

    for root in children.get(None, []):
        walk(root, 0)
    # Anything unreachable (cycle members) still gets a position rather than
    # silently disappearing from the picture.
    for k, _ in pairs:
        if k not in placed:
            placed[k] = Placed(key=k, x=cursor, y=0.0, depth=0, parent=None)
            cursor += gap_x
    return [placed[k] for k, _ in pairs]


@dataclass
class Region:
    """A geographic area holding sites. The ONLY geographic disclosure."""

    region_id: str
    label: str
    lat: float  # region anchor — an area's nominal centre, not a site position
    lon: float
    site_ids: list[str] = field(default_factory=list)
    quant_deg: float = 0.25  # coarser than a site ever was; it is an area

    def anchor(self) -> tuple[float, float]:
        step = self.quant_deg
        return (
            round(math.floor(self.lat / step) * step + step / 2, 4),
            round(math.floor(self.lon / step) * step + step / 2, 4),
        )


def load_regions(spec: list[dict[str, Any]]) -> list[Region]:
    return [
        Region(
            region_id=r["region_id"],
            label=r.get("label") or r["region_id"],
            lat=float(r["lat"]),
            lon=float(r["lon"]),
            site_ids=list(r.get("sites", [])),
            quant_deg=float(r.get("quant_deg", 0.25)),
        )
        for r in spec
    ]
