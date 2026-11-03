# SPDX-License-Identifier: Apache-2.0
"""The navigation model — how a viewer moves between levels without walking.

See docs/design/goat-map-site-availability.md §"Moving without walking".

The design originally said drill-down was the ONLY interaction, on the grounds
that stacking panels is what makes dashboards unusable. That was the wrong
generalisation: what makes them unusable is *modal* state — being somewhere and
having to unwind to get somewhere else. Depth is fine; being trapped is not.

So the whole estate is served as ONE FLAT ADDRESSABLE INDEX. Every region, site
and system has a stable id, and every id is reachable from any other in one
step. Drill-down becomes a convenient path through that index, not the only
road, and the client needs no history stack to answer "show me that other
sensor" — it just addresses it.

Two consequences worth stating because they are easy to lose later:

  - **Stable ids are load-bearing.** They are the deep-link contract, the jump
    target, and the client's cache key. They are derived from declared
    identifiers, never from list position, so inserting a site does not
    renumber its neighbours and invalidate every saved link.

  - **Cross-links are computed server-side.** A system knows its site, its
    region and its siblings without the client re-deriving the tree — which
    also means an RBAC-scoped viewer gets cross-links only to things they may
    see, rather than being handed a graph with holes to notice.
"""

from __future__ import annotations

from dataclasses import dataclass, field
from typing import Any


def rid(*parts: str) -> str:
    """A stable, addressable id. Derived from declared names, never position."""
    return "/".join(p.strip().replace(" ", "-").lower() for p in parts if p)


@dataclass
class NavNode:
    id: str
    kind: str            # region | site | system
    label: str
    state: str = "unmetered"
    parent: str | None = None
    children: list[str] = field(default_factory=list)
    # Free-movement affordances: everything a viewer might jump to from here
    # WITHOUT going up first.
    siblings: list[str] = field(default_factory=list)
    meta: dict[str, Any] = field(default_factory=dict)

    def as_dict(self) -> dict[str, Any]:
        d = {
            "id": self.id,
            "kind": self.kind,
            "label": self.label,
            "state": self.state,
            "parent": self.parent,
            "children": self.children,
            "siblings": self.siblings,
        }
        if self.meta:
            d["meta"] = self.meta
        return d


def build_index(regions, sites_by_id, layout_for_region, layout_for_site) -> dict[str, NavNode]:
    """Flatten the estate into one id -> node map.

    `layout_for_region(region)` -> list[Placed] over site ids
    `layout_for_site(site)`     -> list[Placed] over system names
    """
    index: dict[str, NavNode] = {}

    for region in regions:
        r_id = rid("region", region.region_id)
        member_sites = [s for s in region.site_ids if s in sites_by_id]
        site_ids = [rid("site", s) for s in member_sites]
        lat, lon = region.anchor()
        index[r_id] = NavNode(
            id=r_id, kind="region", label=region.label, parent=None,
            children=site_ids,
            meta={
                "lat": lat, "lon": lon,
                "precision_deg": region.quant_deg,
                "layout": "grid",
                "positions": {
                    rid("site", p.key): {"x": p.x, "y": p.y}
                    for p in layout_for_region(region)
                },
                "site_count": len(member_sites),
            },
        )

        for s_name in member_sites:
            site = sites_by_id[s_name]
            s_id = rid("site", s_name)
            sys_ids = [rid("site", s_name, sy.name) for sy in site.systems]
            placements = {
                rid("site", s_name, p.key): {"x": p.x, "y": p.y, "depth": p.depth}
                for p in layout_for_site(site)
            }
            index[s_id] = NavNode(
                id=s_id, kind="site", label=site.label, state=site.state,
                parent=r_id, children=sys_ids,
                siblings=[i for i in site_ids if i != s_id],
                meta={
                    # NO coordinates. A site's position is its cell in the
                    # region grid, which encodes nothing about where it is.
                    "layout": "tree",
                    "positions": placements,
                    "counts": site.counts(),
                },
            )

            for sy in site.systems:
                y_id = rid("site", s_name, sy.name)
                index[y_id] = NavNode(
                    id=y_id, kind="system", label=sy.name, state=sy.state,
                    parent=s_id,
                    siblings=[i for i in sys_ids if i != y_id],
                    meta={"class": sy.cls},
                )

    return index


def jump_targets(index: dict[str, NavNode], node_id: str) -> list[str]:
    """Everywhere reachable in ONE step — the anti-linear-navigation contract.

    Parent, children, siblings, and (from a system) the whole region, so a
    viewer never has to climb to descend somewhere else.
    """
    node = index.get(node_id)
    if node is None:
        return []
    out: list[str] = []
    if node.parent:
        out.append(node.parent)
        gp = index.get(node.parent)
        if gp and gp.parent:
            out.append(gp.parent)
    out.extend(node.children)
    out.extend(node.siblings)
    seen: dict[str, None] = {}
    for i in out:
        if i != node_id and i in index:
            seen.setdefault(i, None)
    return list(seen)


def search(index: dict[str, NavNode], q: str, limit: int = 20) -> list[dict[str, Any]]:
    """Substring match over labels and ids — the jump-anywhere affordance.

    Deliberately dumb: an operator typing three letters of a sensor name wants
    it, not a ranked relevance model that might bury it. Exact-prefix hits sort
    first so the obvious answer stays at the top.
    """
    ql = q.strip().lower()
    if not ql:
        return []
    hits = [n for n in index.values() if ql in n.label.lower() or ql in n.id]
    hits.sort(key=lambda n: (not n.label.lower().startswith(ql), n.kind, n.id))
    return [n.as_dict() for n in hits[:limit]]
