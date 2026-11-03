# SPDX-License-Identifier: Apache-2.0
"""Tests for the composed readiness view — the shape the UI actually renders.

The endpoint's job is to compose inventory + adapters + attestations into
chains, SCOPE FIRST, then roll up. These tests pin the composition rules that
the UI depends on being true, using the payload shapes the publishers emit.
"""

from __future__ import annotations

import sys
import time
from pathlib import Path

sys.path.insert(0, str(Path(__file__).resolve().parents[1]))

from goatmap.adapters import (  # noqa: E402
    components_from_node_health,
    device_detected_from_discovery,
    producing_from_node_health,
)
from goatmap.attest import (  # noqa: E402
    DOWN,
    AttestableSubject,
    Attestation,
    AttestationLog,
)
from goatmap.disclosure import PRESENCE, STATE, redact_chain  # noqa: E402
from goatmap.readiness import VERDICT_COLOR, build_chain, roll_up  # noqa: E402
from goatmap.scope import ENFORCE, scope_estate, scope_from_headers  # noqa: E402
from goatmap.sites import Site, System  # noqa: E402

from test_adapters import DISCOVERY_EMPTY, NODE_HEALTH_AT_REST, NOW  # noqa: E402


def _site_a():
    return Site(site_id="site-a", label="Site A", lat=0.16, lon=0.65, region_id="delta",
                systems=[
                    System("node-a", "node", peer="svn.netbird"),
                    System("radar-a", "radar"),
                ])


def _compose(site, powered=None):
    """Mirror of the endpoint's composition, so the rules are pinned once."""
    comp, comp_d = components_from_node_health(NODE_HEALTH_AT_REST, now=NOW)
    prod, prod_d = producing_from_node_health(NODE_HEALTH_AT_REST, now=NOW)
    chains = []
    for sy in site.systems:
        det, det_d = (None, "")
        if sy.cls in ("radar", "eo-ir", "ew", "effector"):
            det, det_d = device_detected_from_discovery(DISCOVERY_EMPTY, "radar-a", now=NOW)
        chains.append(build_chain(
            sy.name, cls=sy.cls, power=powered, node_online=True,
            components_ok=comp, components_detail=comp_d,
            device_detected=det, producing=prod, duty_cycled=True,
            evidence={"device_detected": {"detail": det_d}},
        ))
    return chains


def test_an_at_rest_site_composes_to_at_rest_not_broken():
    """The whole point: `overall: down` upstream, calm surface downstream."""
    chains = _compose(_site_a(), powered=True)
    verdict = roll_up(chains)
    assert verdict == "at-rest"
    assert VERDICT_COLOR[verdict] == "idle", "at-rest must render calm, never amber"
    assert all(c.break_at is None for c in chains)


def test_a_node_class_system_reaches_ready_while_the_radar_rests():
    """Different classes reach different tops, and both are correct."""
    chains = {c.subject: c for c in _compose(_site_a(), powered=True)}
    assert chains["node-a"].verdict == "ready", "a node ends at components"
    assert chains["radar-a"].verdict == "at-rest", "a duty-cycled radar stops early"


def test_an_attested_generator_down_breaks_the_chain_at_power():
    log = AttestationLog()
    log.declare(AttestableSubject(subject_id="site-a/power", label="Comms gen",
                                  site_id="site-a"))
    log.record(Attestation(subject="site-a/power", rung="power", state=DOWN,
                           by="maintainer", at=time.time(), note="out of fuel"))
    chains = _compose(_site_a(), powered=log.rung_state("site-a/power"))
    assert roll_up(chains) == "broken"
    assert all(c.break_at == "power" for c in chains)
    # And the cause is named rather than the symptom.
    assert "generator" in chains[0].as_dict()["remedy"].lower()


def test_never_attested_power_is_implied_by_the_node_working():
    """The node is publishing health, which IS evidence it has power.

    Requiring an attestation to know that would be circular and would leave a
    demonstrably-working site reading unknown until someone walked outside.
    What the attestation still adds is WHICH generator is carrying and its
    condition — that the node's own liveness cannot tell you.
    """
    chains = _compose(_site_a(), powered=None)
    for c in chains:
        p = c.by_name()["power"]
        assert p.state == "attained"
        assert "implied by" in p.detail
    assert roll_up(chains) == "at-rest", "not broken, and not falsely unknown"


def test_the_view_is_scoped_before_it_is_rolled_up():
    sites = [_site_a(), Site(site_id="rear", label="Rear", lat=52.0, lon=13.0)]
    orgs = {"site-a": "org-a", "rear": "org-b"}
    viewer = scope_from_headers({"x-forwarded-groups": "data.view.org:org-a"},
                                mode=ENFORCE, site_orgs=orgs)
    permitted = scope_estate(sites, viewer, orgs)
    assert [s.site_id for s in permitted] == ["site-a"]
    assert viewer.tier == STATE


def test_a_scoped_viewer_gets_actionable_detail_but_no_denominators():
    c = build_chain("s", cls="radar", power=True, node_online=False).as_dict()
    red = redact_chain(c, STATE)
    assert red["break_at"] == "node_online" and red["remedy"]
    assert all("age_s" not in r for r in red["rungs"]), "no evidence ages below full"


def test_presence_tier_carries_no_rungs_for_the_ui_to_draw():
    """The UI must degrade to a verdict, not to a half-drawn chain."""
    c = build_chain("s", cls="radar", power=True, node_online=False).as_dict()
    red = redact_chain(c, PRESENCE)
    assert "rungs" not in red and "break_at" not in red
    assert red["verdict"] == "broken"
