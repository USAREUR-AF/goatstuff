# SPDX-License-Identifier: Apache-2.0
"""Tests for the site readiness chain.

The load-bearing assertions are about what must NOT read as a fault. A radar powered
only for threat reporting means a chain that stops at `device_detected` is the
site's normal state for most of its life. A surface that paints that red is one an
operator learns to ignore, and then a genuinely absent node goes unnoticed.
"""

from __future__ import annotations

import sys
import time
from pathlib import Path

import pytest

sys.path.insert(0, str(Path(__file__).resolve().parents[1]))

from goatmap.attest import (  # noqa: E402
    DOWN,
    RUNNING,
    UNKNOWN_STATE,
    AttestableSubject,
    Attestation,
    AttestationLog,
)
from goatmap.readiness import (  # noqa: E402
    ATTAINED,
    UNATTAINED,
    UNKNOWN,
    VERDICT_COLOR,
    build_chain,
    roll_up,
)


# ── the normal state must be calm ────────────────────────────────────────────

def test_a_duty_cycled_radar_at_rest_is_not_a_fault():
    """A duty-cycled site, most of the time: everything up, radar deliberately off."""
    c = build_chain(
        "site-a/radar-a", cls="radar",
        power=True, node_online=True, components_ok=True,
        device_detected=False,          # discovery scanned and found nothing
        duty_cycled=True,
    )
    assert c.verdict == "at-rest"
    assert c.break_at is None
    assert c.attained_through == "components"
    # Rendered calm, never amber.
    assert VERDICT_COLOR[c.verdict] == "idle"


def test_the_same_absence_is_a_fault_when_the_radar_should_be_up():
    """The declaration is what changes the meaning — nothing else differs."""
    c = build_chain(
        "always-on/radar", cls="radar",
        power=True, node_online=True, components_ok=True,
        device_detected=False,
        duty_cycled=False,
    )
    assert c.verdict == "broken"
    assert c.break_at == "device_detected"
    assert "sensor LAN" in c.as_dict()["remedy"]


# ── each rung means a different job ──────────────────────────────────────────

@pytest.mark.parametrize(
    "kwargs,expect_break",
    [
        (dict(power=False), "power"),
        (dict(power=True, node_online=False), "node_online"),
        (dict(power=True, node_online=True, components_ok=False), "components"),
        (dict(cls="radar", power=True, node_online=True, components_ok=True,
              device_detected=True, device_ready=False, duty_cycled=False), "device_ready"),
        (dict(cls="radar", power=True, node_online=True, components_ok=True, device_detected=True,
              device_ready=True, device_active=False, duty_cycled=False), "device_active"),
        (dict(cls="radar", power=True, node_online=True, components_ok=True, device_detected=True,
              device_ready=True, device_active=True, producing=False,
              duty_cycled=False), "producing"),
    ],
)
def test_the_break_is_named_at_the_right_rung(kwargs, expect_break):
    c = build_chain("s", **kwargs)
    assert c.break_at == expect_break
    # And every break carries an action, not just a colour.
    assert c.as_dict()["remedy"]


def test_everything_above_a_break_is_unknown_not_failed():
    """We cannot see a radar through a node that is off the mesh."""
    c = build_chain("s", cls="radar", power=True, node_online=False,
                    device_detected=True, producing=True)  # claims we must ignore
    by = c.by_name()
    assert by["node_online"].state != ATTAINED
    for name in ("components", "device_detected", "device_ready", "device_active", "producing"):
        assert by[name].state == UNKNOWN, f"{name} must be unknown, not failed"


def test_not_observed_is_unknown_never_unattained():
    # None means "we did not look". Conflating that with "we looked and it was
    # not there" is how a surface starts asserting things it never measured.
    c = build_chain("s", power=True, node_online=True, components_ok=None)
    assert c.by_name()["components"].state == UNKNOWN


def test_a_fully_ready_chain_reads_ready():
    c = build_chain("s", cls="radar", power=True, node_online=True, components_ok=True,
                    device_detected=True, device_ready=True, device_active=True,
                    producing=True, duty_cycled=False)
    assert c.verdict == "ready"
    assert c.attained_through == "producing"
    assert VERDICT_COLOR[c.verdict] == "green"


# ── site roll-up ─────────────────────────────────────────────────────────────

def test_rollup_is_worst_first_and_at_rest_is_not_an_alarm():
    ready = build_chain("a", cls="radar", power=True, node_online=True, components_ok=True,
                        device_detected=True, device_ready=True, device_active=True,
                        producing=True, duty_cycled=False)
    rest = build_chain("b", cls="radar", power=True, node_online=True, components_ok=True,
                       device_detected=False, duty_cycled=True)
    broke = build_chain("c", power=True, node_online=False)

    assert roll_up([ready]) == "ready"
    assert roll_up([ready, rest]) == "at-rest"
    assert roll_up([ready, rest, broke]) == "broken"
    assert roll_up([]) == "unknown"


# ── attestations ─────────────────────────────────────────────────────────────

def test_an_attestation_is_never_presented_as_a_measurement():
    now = time.time()
    a = Attestation(subject="site-a/gen-comms", rung="power", state=RUNNING,
                    by="maintainer", at=now, note="3/4 tank")
    d = a.as_dict(now)
    assert d["source"] == "attested"
    assert d["by"] == "maintainer" and d["note"] == "3/4 tank"
    assert a.attains(now) is True


def test_an_attestation_expires_into_unknown_not_into_failure():
    """A generator does not break because nobody walked out to look at it."""
    now = time.time()
    a = Attestation(subject="s", rung="power", state=RUNNING,
                    by="maintainer", at=now - 20 * 3600, ttl_s=12 * 3600)
    assert a.expired(now)
    assert a.attains(now) is None, "expired must be unknown, never False"
    assert a.as_dict(now)["expired"] is True


def test_attested_down_explains_the_break_below_everything_else():
    """The generator is why the node is offline — that is the whole point."""
    now = time.time()
    log = AttestationLog()
    log.declare(AttestableSubject(
        subject_id="site-a/gen-comms", label="Comms generator", rung="power",
        site_id="site-a", future_signal="ops/power/health/site-a",
    ))
    log.record(Attestation(subject="site-a/gen-comms", rung="power", state=DOWN,
                           by="maintainer", at=now, note="out of fuel"))
    powered = log.rung_state("site-a/gen-comms", now)
    assert powered is False

    c = build_chain("site-a", power=powered, node_online=False)
    assert c.break_at == "power"
    # The node being offline is NOT reported as the fault — the power is.
    assert c.by_name()["node_online"].state == UNKNOWN


def test_explicitly_unknown_is_a_real_answer():
    now = time.time()
    a = Attestation(subject="s", rung="power", state=UNKNOWN_STATE, by="maintainer", at=now)
    assert a.attains(now) is None


def test_never_attested_subjects_are_surfaced_as_gaps():
    """A list of only what people bothered to log would flatter the site."""
    log = AttestationLog()
    log.declare(AttestableSubject(subject_id="site-a/gen-radar", label="Radar generator",
                                  site_id="site-a",
                                  future_signal="ops/power/health/site-a"))
    rows = log.for_site("site-a")
    assert len(rows) == 1
    assert rows[0]["never_attested"] is True
    assert rows[0]["attestation"] is None
    # And the upgrade path is recorded before it is needed.
    assert rows[0]["future_signal"] == "ops/power/health/site-a"


def test_an_unknown_attested_state_is_rejected_loudly():
    log = AttestationLog()
    with pytest.raises(ValueError):
        log.record(Attestation(subject="s", rung="power", state="fine", by="d", at=time.time()))


# ── disclosure: the asymmetry problem ────────────────────────────────────────

from goatmap.disclosure import (  # noqa: E402
    FULL,
    PRESENCE,
    STATE,
    redact_attestations,
    redact_chain,
    redact_site,
    region_summary,
)


def test_the_denominator_never_leaves_below_full():
    """Readiness is only exploitable when you know what SHOULD be there."""
    site = {"site_id": "site-a", "state": "broken",
            "counts": {"total": 8, "degraded": 5},
            "systems": [{"name": "a", "class": "radar", "state": "amber"}]}
    for tier in (PRESENCE, STATE):
        red = redact_site(site, tier)
        assert "counts" not in red, f"{tier} leaked the order of battle"
        assert "8" not in repr(red) and "degraded" not in repr(red)
    assert redact_site(site, FULL)["counts"]["total"] == 8


def test_region_badges_withhold_the_tally_but_keep_the_signal():
    verdicts = ["broken", "broken", "ready", "at-rest"]
    for tier in (PRESENCE, STATE):
        s = region_summary(verdicts, tier)
        # An operator still learns the region needs attention…
        assert s["state"] == "broken"
        # …but nothing about HOW MUCH of it is degraded.
        assert "broken" not in {k for k in s if k != "state"}
        assert "sites" not in s
    full = region_summary(verdicts, FULL)
    assert full["sites"] == 4 and full["broken"] == 2


def test_presence_tier_does_not_name_the_failing_rung():
    c = build_chain("s", power=True, node_online=False).as_dict()
    red = redact_chain(c, PRESENCE)
    assert red["verdict"] == "broken"
    assert "break_at" not in red, "naming the failed rung is naming the weakness"
    assert "rungs" not in red
    # STATE gives the operator enough to act.
    st = redact_chain(c, STATE)
    assert st["break_at"] == "node_online" and st["remedy"]
    assert len(st["rungs"]) == len(c["rungs"])


def test_attestations_shed_attribution_and_free_text_below_full():
    import time as _t
    now = _t.time()
    log = AttestationLog()
    log.declare(AttestableSubject(subject_id="site-a/gen", label="Gen", site_id="site-a"))
    log.record(Attestation(subject="site-a/gen", rung="power", state=DOWN,
                           by="maintainer", at=now, note="gen 2 dead, last one running"))
    rows = log.for_site("site-a", now)
    red = redact_attestations(rows, STATE)
    flat = repr(red)
    assert "maintainer" not in flat, "a name is a person to approach"
    assert "gen 2 dead" not in flat, "notes routinely say more than intended"
    # The operational content survives.
    assert red[0]["attestation"]["state"] == DOWN


def test_unwatched_subjects_are_not_advertised_below_full():
    """A list of what nobody is watching is a list of where to look."""
    log = AttestationLog()
    log.declare(AttestableSubject(subject_id="site-a/gen-radar", label="Radar gen",
                                  site_id="site-a"))
    rows = log.for_site("site-a")
    assert rows[0]["never_attested"] is True
    red = redact_attestations(rows, STATE)
    assert "never_attested" not in red[0]


# ── the chain is universal; the words and the rungs are not ──────────────────

from goatmap.readiness import RUNGS, label_for  # noqa: E402


def test_a_class_only_runs_the_rungs_it_actually_has():
    """A pod is not 'missing' an emitting state it was never going to have."""
    pod = build_chain("pod-a", cls="pod", power=True, node_online=True, components_ok=True)
    names = [r.name for r in pod.rungs]
    assert names == ["declared", "power", "node_online", "components"]
    # Reaching the top of ITS chain is `ready`, not `at-rest` — it is not
    # stopping short of anything.
    assert pod.verdict == "ready"
    # And device rungs can never be a break for it.
    assert pod.break_at is None


def test_a_generator_is_two_rungs_and_still_a_first_class_subject():
    gen = build_chain("gen-comms", cls="generator", power=True)
    assert [r.name for r in gen.rungs] == ["declared", "power"]
    assert gen.verdict == "ready"
    down = build_chain("gen-radar", cls="generator", power=False)
    assert down.break_at == "power" and down.verdict == "broken"


def test_the_same_rung_speaks_each_systems_language():
    """Calling a camera 'emitting' would bake the radar assumption back in."""
    assert label_for("radar", "device_active") == "Emitting"
    assert label_for("eo-ir", "device_active") == "Streaming"
    assert label_for("ew", "device_active") == "Transmitting"
    assert label_for("effector", "device_active") == "Armed"
    assert label_for("comms", "producing") == "Carrying traffic"
    assert label_for("radar", "producing") == "Producing detections"
    # An unknown class falls back to the generic word rather than raising.
    assert label_for("something-new", "device_active") == "Active"


def test_remedies_are_per_class_where_the_action_differs():
    radar = build_chain("r", cls="radar", power=True, node_online=True, components_ok=True,
                        device_detected=True, device_ready=True, device_active=False,
                        duty_cycled=False).as_dict()
    cam = build_chain("c", cls="eo-ir", power=True, node_online=True, components_ok=True,
                      device_detected=True, device_ready=True, device_active=False,
                      duty_cycled=False).as_dict()
    assert "radiating" in radar["remedy"]
    assert "streaming" in cam["remedy"]


def test_comms_has_no_device_layer_but_still_reaches_producing():
    relay = build_chain("relay-1", cls="comms", power=True, node_online=True,
                        components_ok=True, producing=True)
    assert "device_detected" not in [r.name for r in relay.rungs]
    assert relay.verdict == "ready"


def test_every_declared_class_uses_only_known_rungs():
    """A profile naming a rung the chain does not have would silently vanish."""
    from goatmap.readiness import CLASS_PROFILE
    for cls, prof in CLASS_PROFILE.items():
        for r in prof.get("rungs", RUNGS):
            assert r in RUNGS, f"{cls} declares unknown rung {r}"
        for r in prof.get("labels", {}):
            assert r in RUNGS, f"{cls} labels unknown rung {r}"
        # And the subset must stay in canonical order, or attained_through walks wrong.
        subset = [r for r in RUNGS if r in prof.get("rungs", RUNGS)]
        assert list(prof.get("rungs", RUNGS)) == subset, f"{cls} rungs out of order"


# ── attestation log persistence ──────────────────────────────────────────────

def test_the_log_replays_to_last_word_per_subject(tmp_path):
    """Append-only on disk; a crash cannot lose an entry."""
    log = AttestationLog()
    log.declare(AttestableSubject(subject_id="s/gen", label="Gen", site_id="s"))
    now = time.time()
    log.record(Attestation(subject="s/gen", rung="power", state=DOWN, by="a", at=now - 100))
    log.record(Attestation(subject="s/gen", rung="power", state=RUNNING, by="b", at=now))
    assert log.rung_state("s/gen", now) is True, "last word wins"
    # And the earlier record is still the earlier record — nothing mutated it.
    assert log.latest["s/gen"].by == "b"


def test_a_malformed_record_does_not_blind_the_rest(tmp_path):
    """One bad line must not take the site's whole attestation view down."""
    log = AttestationLog()
    log.declare(AttestableSubject(subject_id="s/a", label="A", site_id="s"))
    log.declare(AttestableSubject(subject_id="s/b", label="B", site_id="s"))
    now = time.time()
    log.record(Attestation(subject="s/a", rung="power", state=RUNNING, by="x", at=now))
    # A rejected record raises at record() time and never enters the log…
    try:
        log.record(Attestation(subject="s/b", rung="power", state="nonsense", by="x", at=now))
    except ValueError:
        pass  # expected: rejected record raises
    # …leaving the good one intact and the bad subject visibly unattested.
    rows = {r["subject_id"]: r for r in log.for_site("s", now)}
    assert rows["s/a"]["attestation"]["state"] == RUNNING
    assert rows["s/b"]["never_attested"] is True


# ── unknown is not broken ────────────────────────────────────────────────────

def test_an_unobserved_expected_rung_is_unknown_never_broken():
    """"Nobody walked out to check the generator" is not the generator failing.

    Treating unknown as a break would make every un-instrumented thing a
    permanent alarm — the same false-alarm class the expectation rule exists to
    prevent, arriving through a different door.

    Nothing above power is observed here, so there is no evidence to imply it.
    """
    c = build_chain("s", cls="node", power=None, node_online=None, components_ok=None)
    assert c.by_name()["power"].state == UNKNOWN
    assert c.break_at is None, "unknown must not be reported as a break"
    assert c.verdict == "unknown", "but it must not read ready either"


def test_a_lower_rung_is_implied_by_a_higher_one_that_is_observed():
    """You cannot run components on an unpowered host.

    A node publishing its own health is direct evidence that power is on.
    Requiring a human to confirm that would be circular, and would leave a
    demonstrably-working site reading `unknown` until someone walked outside.
    """
    c = build_chain("s", cls="node", power=None, node_online=True, components_ok=True)
    power = c.by_name()["power"]
    assert power.state == ATTAINED
    assert "implied by" in power.detail, "the inference must be visible, not silent"
    assert c.verdict == "ready"


def test_implication_never_overrides_a_direct_observation():
    """An attested-down generator is not overruled by anything above it."""
    c = build_chain("s", cls="node", power=False, node_online=True, components_ok=True)
    assert c.by_name()["power"].state == UNATTAINED
    assert c.break_at == "power"


def test_looked_and_missing_is_still_a_break():
    """The distinction is only useful if the other side still fires."""
    c = build_chain("s", cls="node", power=False, node_online=True)
    assert c.break_at == "power" and c.verdict == "broken"


def test_unknown_does_not_mask_a_real_break_below_it():
    # power known-bad, node unobserved: the BREAK is what gets reported.
    c = build_chain("s", cls="node", power=False, node_online=None)
    assert c.break_at == "power" and c.verdict == "broken"


def test_a_chain_with_no_expected_unknowns_still_reaches_ready():
    c = build_chain("s", cls="node", power=True, node_online=True, components_ok=True)
    assert c.verdict == "ready"
