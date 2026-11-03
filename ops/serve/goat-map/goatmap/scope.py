# SPDX-License-Identifier: Apache-2.0
"""Per-viewer scoping — applied BEFORE roll-up, never after.

See docs/design/site-readiness-chain.md §"The asymmetry problem" and
docs/design/rbac-adherence.md §3.3 (ADR 1073/1074).

THE ORDERING IS THE SECURITY PROPERTY. A region badge computed over sites the
viewer cannot see leaks both the existence and the health of hidden estate — the
badge is a channel even when every label is stripped. So scoping happens first
and roll-ups are computed over the permitted subset only. Two viewers can
legitimately see different states for the same region, and that is correct.

Filtering must also be SERVER-SIDE. Sending a filtered-out site with a `hidden`
flag is a leak waiting for someone to open devtools.

THE HEADER CHOICE IS LOAD-BEARING, and it is not the obvious one. This mirrors
`ops/panoscope/src/viewer.rs` exactly, because that contract was
settled the hard way:

  - `X-Forwarded-Groups` is the ONLY groups source. Our oauth2-proxy sidecar
    runs as a REVERSE PROXY, so it strips any client-supplied `X-Forwarded-*`
    and injects its own from the validated session — the browser cannot forge it.
  - `X-Auth-Request-Groups` is NEVER read. In reverse-proxy mode oauth2-proxy
    only sets that family on the `/oauth2/auth` subrequest RESPONSE, and does
    NOT strip a client-supplied copy from the upstream request. Trusting it let
    a logged-in floor viewer send `X-Auth-Request-Groups: ADMINS` and escalate
    to full. Reading it *at all* is the
    hole.

Mode is off by default: byte-identical to pre-scoping behaviour until an
operator deploys the sidecar and flips the flag. `enforce` is FAIL-CLOSED — a
request with no or unrecognised groups gets the floor, not everything.
"""

from __future__ import annotations

import os
from dataclasses import dataclass, field
from typing import Any, Iterable

from .disclosure import FULL, PRESENCE, STATE

# The only trusted groups header. See the module docstring before changing this.
GROUPS_HEADER = "x-forwarded-groups"
EMAIL_HEADER = "x-forwarded-email"

# Deliberately NOT read. Present as a constant so a CI grep can assert nobody
# reintroduced it, and so the reason travels with the name.
FORGEABLE_HEADER_DO_NOT_READ = "x-auth-request-groups"

OFF, ENFORCE = "off", "enforce"

# Data-access grammar (ADR 1074) — the claims that grant breadth here.
CLAIM_ALL = "data.view.all"
CLAIM_ORG = "data.view.org:"
CLAIM_PRODUCER = "data.view.producer:"

# Platform group whose SoT-declared cross-axis implication is data.view.all:
#
#   ops/permissions/data-access-roles.yaml
#   - { holder: platform-group PLATFORM-ADMIN, implies: data.view.all,
#       basis: 'rename mirror of ADMINS (...)', status: enforced }
#
# `ADMINS` IS NOT THIS GROUP. It is the pre-rename name of PLATFORM-ADMIN and is
# gone from every plane. Naming a retired group here matches no identity, and
# `enforce` would send every admin to the floor, i.e. an empty map.
#
# IAM-ADMIN is deliberately absent: the matrix's second pass narrowed it to
# `scoped`, because identity administration is not data access (ADR 1090). An
# IAM-ADMIN with no data.view.* claim correctly gets the floor.
#
# Overridable so a goatnet mid-rename can dual-accept without a code change.
FULL_GROUPS = frozenset(
    g.strip() for g in os.environ.get(
        "GOATMAP_FULL_GROUPS", "PLATFORM-ADMIN"
    ).split(",") if g.strip()
)


@dataclass
class ViewerScope:
    """What one viewer may see, and at what fidelity."""

    kind: str = "floor"                    # full | scoped | floor
    orgs: frozenset[str] = field(default_factory=frozenset)
    sites: frozenset[str] = field(default_factory=frozenset)
    email: str = ""
    # Producer anchors granted explicitly by `data.view.producer:<anchor>`.
    # Kept SEPARATE from `sites` even though the readiness path folds producers
    # into sites: there, a producer claim names a site; here it must match a
    # track's `producer_anchor`, which is a Zenoh key chunk. Same claim, two
    # namespaces — collapsing them would let a site_id grant tracks, or an
    # anchor grant a site. `sites` is still populated exactly as before, so
    # readiness behaviour is unchanged.
    producers: frozenset[str] = field(default_factory=frozenset)

    @property
    def tier(self) -> str:
        """Disclosure tier follows breadth.

        Full scope earns `full` — including the denominators. A scoped viewer
        gets `state`: enough to act on their own estate without the totals that
        would let a capture size the whole force. Floor gets `presence`.
        """
        return {"full": FULL, "scoped": STATE}.get(self.kind, PRESENCE)

    def may_see_site(self, site_id: str, org: str = "") -> bool:
        if self.kind == "full":
            return True
        if self.kind == "floor":
            return False
        return site_id in self.sites or (bool(org) and org in self.orgs)

    def may_see_track(self, org: str | None, producer_anchor: str = "") -> bool:
        """Whether this viewer may see one track.

        UNVERIFIED PROVENANCE IS DENIED to anyone but a full viewer. A track
        whose anchor resolves to no org in the portal registry (`org is None`,
        surfaced as *unverified*) cannot be shown to a scoped viewer, because
        there is no evidence it belongs to their org — and an org claim must
        not become a channel for whatever the registry could not name. That is
        the fail-closed reading, and it is deliberately stricter than the
        readiness path, where an empty org merely fails the org test.

        The anchor is the Zenoh key's first chunk, which the router admits
        writes to only from the peer that owns it, so a producer grant cannot
        be forged by the payload. `org` comes from the registry, never the
        sender. Both inputs are therefore trustworthy for an access decision.
        """
        if self.kind == "full":
            return True
        if self.kind == "floor":
            return False
        if producer_anchor and producer_anchor in self.producers:
            return True
        return bool(org) and org in self.orgs


def _groups(headers: dict[str, str]) -> list[str]:
    """Groups from the trusted header only, comma-separated."""
    raw = ""
    for k, v in headers.items():
        if k.lower() == GROUPS_HEADER:
            raw = v or ""
            break
    return [g.strip() for g in raw.split(",") if g.strip()]


def scope_from_headers(
    headers: dict[str, str],
    mode: str = OFF,
    site_orgs: dict[str, str] | None = None,
) -> ViewerScope:
    """Derive a scope. `off` → full; `enforce` → fail-closed to floor.

    `site_orgs` maps site_id -> owning org so an org claim expands to the sites
    it covers server-side. A viewer never receives a site to filter themselves.
    """
    email = ""
    for k, v in headers.items():
        if k.lower() == EMAIL_HEADER:
            email = (v or "").strip().lower()
            break

    if mode != ENFORCE:
        # Byte-identical to pre-scoping behaviour, and explicitly `full` rather
        # than an accidental floor that would silently blank a working surface.
        return ViewerScope(kind="full", email=email)

    claims = _groups(headers)
    if not claims:
        return ViewerScope(kind="floor", email=email)

    if CLAIM_ALL in claims or FULL_GROUPS.intersection(claims):
        return ViewerScope(kind="full", email=email)

    orgs = frozenset(
        c[len(CLAIM_ORG):] for c in claims if c.startswith(CLAIM_ORG) and c[len(CLAIM_ORG):]
    )
    producers = frozenset(
        c[len(CLAIM_PRODUCER):] for c in claims
        if c.startswith(CLAIM_PRODUCER) and c[len(CLAIM_PRODUCER):]
    )
    if not orgs and not producers:
        # Authenticated but nothing that grants breadth — the floor, not
        # everything. Fail-closed is the point of `enforce`.
        return ViewerScope(kind="floor", email=email)

    sites = set(producers)
    for site_id, org in (site_orgs or {}).items():
        if org in orgs:
            sites.add(site_id)
    return ViewerScope(
        kind="scoped", orgs=orgs, sites=frozenset(sites),
        producers=producers, email=email,
    )


def scope_estate(
    sites: Iterable[Any],
    scope: ViewerScope,
    site_org: dict[str, str] | None = None,
) -> list[Any]:
    """Filter sites to what this viewer may see. Call this FIRST.

    Everything downstream — region badges, counts, worst-state — must be
    computed from the returned list, never from the full inventory. That
    ordering is the whole control: a badge over hidden estate discloses it.
    """
    orgs = site_org or {}
    return [s for s in sites if scope.may_see_site(getattr(s, "site_id", ""), orgs.get(getattr(s, "site_id", ""), ""))]


def scope_tracks(tracks: Iterable[dict], scope: ViewerScope) -> tuple[list[dict], int]:
    """Filter tracks server-side. Returns (permitted, withheld_count).

    SERVER-SIDE IS THE POINT. Emitting a withheld track with a `hidden` flag,
    or letting the browser filter, is a leak waiting for devtools.

    The withheld count is returned but NOT unconditionally emitted — see
    `track_envelope`. Disclosure rule 1: the denominator is the crown jewel.
    """
    permitted, withheld = [], 0
    for t in tracks:
        if scope.may_see_track(t.get("org"), t.get("producer_anchor", "")):
            permitted.append(t)
        else:
            withheld += 1
    return permitted, withheld


# Fleet-volume fields. These describe the whole picture regardless of what the
# viewer is scoped to, so they are denominators in the disclosure sense and are
# withheld below `full` — a scoped viewer who can see two tracks but is told
# the service has processed 40,000 messages learns the size of the estate they
# cannot see.
_VOLUME_FIELDS = (
    "messages", "fabric_messages", "published", "unmappable",
    "expired", "msgs_per_s", "clients",
)


def scope_stats(stats: dict, scope: ViewerScope, visible: int) -> dict:
    """Stats as this viewer may receive them.

    Operational honesty is preserved at every tier — uptime, which leg the
    picture comes from, whether the fabric is up, when the last message landed.
    A scoped viewer must still be able to tell "quiet" from "broken", because a
    surface that hides its own degradation is worse than one that shows less.
    """
    if scope.kind == "full":
        return stats
    out = {k: v for k, v in stats.items() if k not in _VOLUME_FIELDS}
    out["tracks"] = visible
    return out


def track_envelope(tracks: Iterable[dict], scope: ViewerScope, stats: dict) -> dict:
    """The `/api/tracks` and SSE-snapshot body, scoped.

    `partial` is the honesty signal and is safe at every tier: it tells a
    viewer their picture is incomplete WITHOUT quantifying what is missing, so
    the UI can say "partial view" instead of implying it is the whole estate.
    The count itself rides only at `full`, where the viewer already has the
    denominators anyway.
    """
    permitted, withheld = scope_tracks(tracks, scope)
    body: dict[str, Any] = {
        "tracks": permitted,
        "stats": scope_stats(stats, scope, len(permitted)),
        "scope": scope.kind,
        "disclosure": scope.tier,
        "partial": withheld > 0,
    }
    if scope.tier == FULL:
        body["withheld"] = withheld
    return body


def region_members(region: Any, permitted_site_ids: set[str]) -> list[str]:
    """A region's members, intersected with what the viewer may see.

    A region whose every site is hidden returns empty, and the caller must then
    omit the region entirely rather than render an empty one — an empty box
    still says "there is something here you cannot see".
    """
    return [s for s in getattr(region, "site_ids", []) if s in permitted_site_ids]
