# SPDX-License-Identifier: Apache-2.0
"""Tests for per-viewer scoping.

Two assertions here are security controls rather than behaviour checks:

1. **The forgeable header is never read.** `X-Auth-Request-Groups` is not
   stripped by oauth2-proxy in reverse-proxy mode, so trusting it let a floor
   viewer escalate to full. This suite fails if anyone
   reintroduces it.
2. **Scoping happens before roll-up.** A region badge computed over sites the
   viewer cannot see leaks their existence and health even with labels stripped.
"""

from __future__ import annotations

import sys
from pathlib import Path

sys.path.insert(0, str(Path(__file__).resolve().parents[1]))

from goatmap.disclosure import FULL, PRESENCE, STATE, region_summary  # noqa: E402
from goatmap.readiness import build_chain, roll_up  # noqa: E402
from goatmap.scope import (  # noqa: E402
    ENFORCE,
    FORGEABLE_HEADER_DO_NOT_READ,
    OFF,
    ViewerScope,
    region_members,
    scope_estate,
    scope_from_headers,
)
from goatmap.sites import Site  # noqa: E402

SITE_ORGS = {"site-a": "org-a", "site-b": "org-a", "rear": "org-b"}


def _sites():
    return [Site(site_id=s, label=s.title(), lat=1.0, lon=1.0) for s in SITE_ORGS]


# ── the forgery hole ─────────────────────────────────────────────────────────

def test_the_forgeable_groups_header_is_never_trusted():
    """Reading it at all is the hole, not reading it wrongly."""
    headers = {FORGEABLE_HEADER_DO_NOT_READ: "PLATFORM-ADMIN", "x-forwarded-email": "a@b"}
    s = scope_from_headers(headers, mode=ENFORCE, site_orgs=SITE_ORGS)
    assert s.kind == "floor", "a forged groups header must not escalate"
    assert s.tier == PRESENCE


def test_the_trusted_header_does_grant():
    s = scope_from_headers({"x-forwarded-groups": "PLATFORM-ADMIN"}, mode=ENFORCE, site_orgs=SITE_ORGS)
    assert s.kind == "full" and s.tier == FULL


def test_header_lookup_is_case_insensitive():
    s = scope_from_headers({"X-Forwarded-Groups": "data.view.all"}, mode=ENFORCE)
    assert s.kind == "full"


# ── fail-closed ──────────────────────────────────────────────────────────────

def test_enforce_with_no_groups_is_the_floor_not_everything():
    assert scope_from_headers({}, mode=ENFORCE).kind == "floor"


def test_authenticated_but_ungranted_is_still_the_floor():
    """Being logged in is not a grant."""
    s = scope_from_headers({"x-forwarded-groups": "SOME-UNRELATED-GROUP"}, mode=ENFORCE)
    assert s.kind == "floor"


def test_off_mode_is_byte_identical_to_pre_scoping():
    """The shipping default must not silently blank a working surface."""
    s = scope_from_headers({}, mode=OFF)
    assert s.kind == "full" and s.tier == FULL


def test_iam_admin_does_not_imply_full():
    """The matrix grants it `scoped`; only PLATFORM-ADMIN carries the implication."""
    s = scope_from_headers({"x-forwarded-groups": "IAM-ADMIN"}, mode=ENFORCE)
    assert s.kind == "floor"


# ── scoping, and its ordering ────────────────────────────────────────────────

def test_an_org_claim_expands_to_its_sites_server_side():
    s = scope_from_headers({"x-forwarded-groups": "data.view.org:org-a"},
                           mode=ENFORCE, site_orgs=SITE_ORGS)
    assert s.kind == "scoped"
    permitted = {x.site_id for x in scope_estate(_sites(), s, SITE_ORGS)}
    assert permitted == {"site-a", "site-b"}
    assert "rear" not in permitted, "another org's site must never be sent"


def test_a_producer_claim_grants_exactly_one_site():
    s = scope_from_headers({"x-forwarded-groups": "data.view.producer:site-a"},
                           mode=ENFORCE, site_orgs=SITE_ORGS)
    assert {x.site_id for x in scope_estate(_sites(), s, SITE_ORGS)} == {"site-a"}


def test_the_region_badge_is_computed_after_scoping_not_before():
    """The ordering IS the control."""
    # Ground truth: one broken site, and the viewer cannot see it.
    verdicts_all = {"site-a": "ready", "site-b": "ready", "rear": "broken"}
    s = scope_from_headers({"x-forwarded-groups": "data.view.org:org-a"},
                           mode=ENFORCE, site_orgs=SITE_ORGS)
    permitted = [x.site_id for x in scope_estate(_sites(), s, SITE_ORGS)]

    after = region_summary([verdicts_all[i] for i in permitted], s.tier)
    before = region_summary(list(verdicts_all.values()), s.tier)

    assert after["state"] == "ready", "scoped viewer must not learn a hidden site is broken"
    assert before["state"] == "broken"
    assert after["state"] != before["state"], "this is exactly the leak the ordering prevents"


def test_two_viewers_legitimately_see_different_region_states():
    all_v = {"site-a": "ready", "rear": "broken"}
    ro = scope_from_headers({"x-forwarded-groups": "data.view.org:org-a"},
                            mode=ENFORCE, site_orgs=SITE_ORGS)
    admin = scope_from_headers({"x-forwarded-groups": "PLATFORM-ADMIN"}, mode=ENFORCE)
    ro_ids = [x.site_id for x in scope_estate(_sites(), ro, SITE_ORGS)]
    ad_ids = [x.site_id for x in scope_estate(_sites(), admin, SITE_ORGS)]
    assert region_summary([all_v[i] for i in ro_ids if i in all_v], ro.tier)["state"] == "ready"
    assert region_summary([all_v[i] for i in ad_ids if i in all_v], admin.tier)["state"] == "broken"


def test_a_fully_hidden_region_has_no_members_to_render():
    """An empty box still says 'there is something here you cannot see'."""
    class R:
        site_ids = ["rear"]
    s = scope_from_headers({"x-forwarded-groups": "data.view.org:org-a"},
                           mode=ENFORCE, site_orgs=SITE_ORGS)
    permitted = {x.site_id for x in scope_estate(_sites(), s, SITE_ORGS)}
    assert region_members(R(), permitted) == []


def test_scoped_viewers_do_not_receive_denominators():
    """Their tier is `state`, which strips counts — the order of battle."""
    s = scope_from_headers({"x-forwarded-groups": "data.view.producer:site-a"},
                           mode=ENFORCE, site_orgs=SITE_ORGS)
    assert s.tier == STATE
    summary = region_summary(["ready", "broken"], s.tier)
    assert "sites" not in summary and "broken" not in {k for k in summary if k != "state"}


def test_floor_sees_no_sites_at_all():
    s = ViewerScope(kind="floor")
    assert scope_estate(_sites(), s, SITE_ORGS) == []


def test_chain_detail_is_not_the_leak_vector_scope_is():
    """A permitted site still gives its operator what they need to act."""
    c = build_chain("s", cls="radar", power=True, node_online=False).as_dict()
    assert c["break_at"] == "node_online" and c["remedy"]
    assert roll_up([build_chain("s", power=True, node_online=False)]) == "broken"
