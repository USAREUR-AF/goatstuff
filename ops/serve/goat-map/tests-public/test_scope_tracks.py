# SPDX-License-Identifier: Apache-2.0
"""Tests for per-viewer scoping of TRACK data.

The companion suite `test_scope.py` covers the readiness surface. This one
covers `/api/tracks` and the SSE stream, which must read the viewer too —
otherwise `enforce` scopes readiness badges while every authenticated viewer
still receives every track.

Three assertions here are security controls, not behaviour checks:

1. **Unverified provenance is denied to scoped viewers.** A track whose anchor
   resolves to no org in the registry has no evidence of belonging to anyone;
   showing it to an org-scoped viewer would make an org grant a channel for
   whatever the registry could not name.
2. **The denominator does not ride below `full`.** Fleet volume counters
   describe the whole picture regardless of scope (disclosure rule 1).
3. **`expire` is not sent for tracks the viewer never received.** The id alone
   would let a viewer count the estate they cannot see.
"""

from __future__ import annotations

import sys
from pathlib import Path

sys.path.insert(0, str(Path(__file__).resolve().parents[1]))

from goatmap.disclosure import FULL, PRESENCE, STATE  # noqa: E402
from goatmap.scope import (  # noqa: E402
    FORGEABLE_HEADER_DO_NOT_READ,
    ENFORCE,
    OFF,
    ViewerScope,
    scope_from_headers,
    scope_stats,
    scope_tracks,
    track_envelope,
)

ALPHA = {"id": "t1", "org": "alpha", "producer_anchor": "anchor-a"}
BETA = {"id": "t2", "org": "beta", "producer_anchor": "anchor-b"}
UNVERIFIED = {"id": "t3", "org": None, "producer_anchor": "anchor-x"}
ALL = [ALPHA, BETA, UNVERIFIED]

STATS = {
    "uptime_s": 10.0, "messages": 40000, "fabric_messages": 900,
    "published": 12, "unmappable": 3, "expired": 7, "msgs_per_s": 4.0,
    "clients": 2, "tracks": 3, "fabric": "up", "store_source": "fabric",
    "last_message_at": 1.0,
}


def _hdrs(groups: str) -> dict[str, str]:
    return {"X-Forwarded-Groups": groups, "X-Forwarded-Email": "a@example.test"}


# --- mode off: byte-identical to no scoping --------------------------------


def test_off_mode_sees_every_track_including_unverified():
    """`off` must not quietly start filtering — that would be a silent change
    of meaning for every existing deployment."""
    scope = scope_from_headers(_hdrs(""), mode=OFF)
    permitted, withheld = scope_tracks(ALL, scope)
    assert permitted == ALL
    assert withheld == 0


def test_off_mode_keeps_full_stats():
    scope = scope_from_headers(_hdrs(""), mode=OFF)
    assert scope_stats(STATS, scope, 3) == STATS


# --- fail-closed floor ------------------------------------------------------


def test_enforce_no_groups_sees_no_tracks():
    """Authenticated but unclaimed is the floor, not everything."""
    scope = scope_from_headers(_hdrs(""), mode=ENFORCE)
    permitted, withheld = scope_tracks(ALL, scope)
    assert permitted == []
    assert withheld == 3
    assert scope.tier == PRESENCE


def test_enforce_unrecognised_groups_sees_no_tracks():
    scope = scope_from_headers(_hdrs("SOME-UNRELATED-GROUP"), mode=ENFORCE)
    assert scope_tracks(ALL, scope)[0] == []


# --- scoped by org ----------------------------------------------------------


def test_org_claim_sees_only_that_org():
    scope = scope_from_headers(_hdrs("data.view.org:alpha"), mode=ENFORCE)
    permitted, withheld = scope_tracks(ALL, scope)
    assert permitted == [ALPHA]
    assert withheld == 2


def test_unverified_provenance_denied_to_scoped_viewer():
    """SECURITY. `org is None` means the registry could not name the anchor.
    An org grant must not become a channel for the unnameable."""
    scope = scope_from_headers(_hdrs("data.view.org:alpha"), mode=ENFORCE)
    assert UNVERIFIED not in scope_tracks(ALL, scope)[0]


def test_unverified_visible_to_full_viewer():
    """Full viewers must still see unverified tracks — that is precisely the
    population an operator needs to notice and chase down."""
    scope = scope_from_headers(_hdrs("data.view.all"), mode=ENFORCE)
    assert scope_tracks(ALL, scope)[0] == ALL


# --- scoped by producer anchor ---------------------------------------------


def test_producer_claim_matches_anchor_not_org():
    scope = scope_from_headers(_hdrs("data.view.producer:anchor-b"), mode=ENFORCE)
    assert scope_tracks(ALL, scope)[0] == [BETA]


def test_producer_claim_can_reach_unverified_track():
    """An explicit anchor grant is evidence about THAT anchor, so it survives
    the unverified rule — unlike an org claim, which is evidence about an org
    the track cannot be shown to belong to."""
    scope = scope_from_headers(_hdrs("data.view.producer:anchor-x"), mode=ENFORCE)
    assert scope_tracks(ALL, scope)[0] == [UNVERIFIED]


def test_site_id_does_not_grant_tracks():
    """SECURITY. The readiness path folds producer claims into `sites`. Track
    matching must use `producers`, or a site_id would grant track data."""
    scope = ViewerScope(kind="scoped", orgs=frozenset(),
                        sites=frozenset({"anchor-a"}), producers=frozenset())
    assert scope_tracks(ALL, scope)[0] == []


# --- ADMINS / full ----------------------------------------------------------


def test_platform_admin_is_full():
    scope = scope_from_headers(_hdrs("PLATFORM-ADMIN"), mode=ENFORCE)
    assert scope.kind == "full"
    assert scope_tracks(ALL, scope)[0] == ALL


# --- disclosure: the denominator --------------------------------------------


def test_volume_counters_withheld_below_full():
    """SECURITY (disclosure rule 1). Fleet volume describes the whole estate
    regardless of what the viewer is scoped to."""
    scope = scope_from_headers(_hdrs("data.view.org:alpha"), mode=ENFORCE)
    out = scope_stats(STATS, scope, 1)
    for leaked in ("messages", "fabric_messages", "published", "unmappable",
                   "expired", "msgs_per_s", "clients"):
        assert leaked not in out, f"{leaked} leaks the size of the hidden estate"


def test_scoped_stats_report_the_viewers_own_count():
    scope = scope_from_headers(_hdrs("data.view.org:alpha"), mode=ENFORCE)
    assert scope_stats(STATS, scope, 1)["tracks"] == 1


def test_operational_honesty_survives_scoping():
    """A scoped viewer must still be able to tell 'quiet' from 'broken'."""
    scope = scope_from_headers(_hdrs("data.view.org:alpha"), mode=ENFORCE)
    out = scope_stats(STATS, scope, 1)
    for kept in ("fabric", "store_source", "uptime_s", "last_message_at"):
        assert kept in out


# --- the envelope -----------------------------------------------------------


def test_envelope_flags_partial_without_quantifying_it():
    scope = scope_from_headers(_hdrs("data.view.org:alpha"), mode=ENFORCE)
    body = track_envelope(ALL, scope, STATS)
    assert body["partial"] is True
    assert "withheld" not in body, "the count is a denominator below full"
    assert body["scope"] == "scoped"
    assert body["disclosure"] == STATE


def test_envelope_gives_full_viewers_the_count():
    scope = scope_from_headers(_hdrs("data.view.all"), mode=ENFORCE)
    body = track_envelope(ALL, scope, STATS)
    assert body["partial"] is False
    assert body["withheld"] == 0
    assert body["disclosure"] == FULL


def test_envelope_never_emits_a_withheld_track():
    """SECURITY. Server-side means server-side: no hidden-flagged payloads."""
    scope = scope_from_headers(_hdrs("data.view.org:alpha"), mode=ENFORCE)
    body = track_envelope(ALL, scope, STATS)
    ids = {t["id"] for t in body["tracks"]}
    assert "t2" not in ids and "t3" not in ids
    assert "beta" not in repr(body)


# --- the forgeable header ---------------------------------------------------


def test_forgeable_header_cannot_grant_track_access():
    """SECURITY. Regression guard on the track path specifically."""
    # Referenced via the constant, never the literal: `scope.py` exports it so
    # a CI grep can assert nobody reintroduced the string, and spelling it out
    # here would defeat that guard from inside the suite that defends it.
    headers = {
        FORGEABLE_HEADER_DO_NOT_READ: "data.view.all,ADMINS",
        "X-Forwarded-Email": "a@example.test",
    }
    scope = scope_from_headers(headers, mode=ENFORCE)
    assert scope.kind == "floor"
    assert scope_tracks(ALL, scope)[0] == []


def test_dead_legacy_group_grants_nothing():
    """`ADMINS` is the pre-rename name of PLATFORM-ADMIN. A dead group name must
    grant nothing, loudly."""
    scope = scope_from_headers(_hdrs("ADMINS"), mode=ENFORCE)
    assert scope.kind == "floor"
    assert scope_tracks(ALL, scope)[0] == []
