# SPDX-License-Identifier: Apache-2.0
"""Tests for rung provenance and the site reason line.

A chain must not render `device_detected: attained` with the detail "no discovery
output" next to it: a rung INFERRED from a higher rung with text from an observation
that never landed asserts and denies the same fact in one line.

The rule these pin: an inferred rung must never be dressed as a measured one,
and the failed observation must survive as a caveat rather than be deleted —
a device that is demonstrably producing while discovery cannot see it means
discovery is blind, which is worth knowing.
"""

from __future__ import annotations

import sys
from pathlib import Path

sys.path.insert(0, str(Path(__file__).resolve().parents[1]))

from goatmap.readiness import (  # noqa: E402
    ATTAINED,
    IMPLIED,
    MEASURED,
    build_chain,
    roll_up_reason,
)


def _radar_a(**kw):
    """The at-rest shape: producing, but discovery found nothing."""
    base = dict(
        cls="radar", node_online=True, components_ok=True, producing=True,
        evidence={"device_detected": {"detail": "no discovery output"}},
    )
    base.update(kw)
    return build_chain("radar-a", **base)


# ── the defect ───────────────────────────────────────────────────────────────

def test_an_inferred_rung_is_not_labelled_as_measured():
    r = _radar_a().by_name()["device_detected"]
    assert r.state == ATTAINED
    assert r.source == IMPLIED, "the UI styles on this; measured would be a lie"


def test_the_inference_outranks_the_failed_observation_in_the_text():
    """The exact regression: `attained` beside 'no discovery output'."""
    r = _radar_a().by_name()["device_detected"]
    assert r.detail.startswith("implied by"), r.detail
    assert not r.detail.startswith("no discovery output")


def test_the_failed_observation_is_kept_as_a_caveat_not_dropped():
    """Discovery being blind to a producing radar is itself a finding."""
    r = _radar_a().by_name()["device_detected"]
    assert "not directly observed: no discovery output" in r.detail


def test_a_directly_observed_rung_stays_measured():
    c = build_chain("n", cls="node", node_online=True, components_ok=True,
                    components_detail="4 infrastructure probes ok")
    assert c.by_name()["components"].source == MEASURED


def test_power_inferred_from_a_live_node_is_marked_inferred():
    """We never read the generator; we deduced it. Say so."""
    p = build_chain("n", cls="node", node_online=True, components_ok=True).by_name()["power"]
    assert p.state == ATTAINED and p.source == IMPLIED


# ── the site reason ──────────────────────────────────────────────────────────

def _mixed():
    return [
        build_chain("node-a", cls="node", node_online=True, components_ok=True),
        build_chain("node-b", cls="node"),           # nothing reporting
        _radar_a(),
    ]


def test_the_reason_names_the_subject_that_drove_the_verdict():
    """Without this, one dark auxiliary node and a dark site look identical."""
    r = roll_up_reason(_mixed())
    assert "node-b" in r and "unreadable" in r


def test_the_reason_says_how_much_is_still_reporting():
    assert "2 other subjects reporting" in roll_up_reason(_mixed())


def test_a_fully_ready_site_says_so_without_naming_subjects():
    r = roll_up_reason([build_chain("a", cls="node", node_online=True, components_ok=True)])
    assert "ready" in r and "node-a" not in r


def test_no_subjects_is_not_reported_as_healthy():
    assert "no subjects" in roll_up_reason([])


# ── the reason is a disclosure surface too ───────────────────────────────────

def test_counts_are_withheld_below_the_full_tier():
    """Every tally here is a denominator — kit that exists vs kit that works.

    Asserted against the FULL rendering rather than by hunting digits: subject
    names legitimately contain them (`node-b`), so a digit scan flags the
    honest text and would push someone to rename hosts to satisfy a test.
    """
    full = roll_up_reason(_mixed(), with_counts=True)
    scoped = roll_up_reason(_mixed(), with_counts=False)
    assert "2 other subjects reporting" in full
    assert "other subject" not in scoped and "2" not in scoped.replace("node-b", "")


def test_the_scoped_reason_still_names_what_to_go_and_look_at():
    """Withholding the denominator must not make it useless to its own operator."""
    scoped = roll_up_reason(_mixed(), with_counts=False)
    assert "node-b" in scoped and "unreadable" in scoped


def test_a_ready_site_leaks_no_count_either():
    """'all 3 subjects ready' states the size of the estate to anyone who asks."""
    chains = [build_chain(n, cls="node", node_online=True, components_ok=True)
              for n in ("a", "b", "c")]
    assert not any(ch.isdigit() for ch in roll_up_reason(chains, with_counts=False))
    assert "3" in roll_up_reason(chains), "the full tier still gets the denominator"
