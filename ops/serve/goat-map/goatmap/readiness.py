# SPDX-License-Identifier: Apache-2.0
"""Site readiness — the chain from "kit should exist" to "detections flowing".

See docs/design/site-readiness-chain.md.

THE PROBLEM THIS SOLVES. Asking "is the site up?" collapses six different
questions into one, and the answers point at completely different actions:

    declared          we say this kit belongs here
    node_online       the host is on the mesh
    components        the integration components (kids, bridges, converters) run
    device_detected   the end device answers on the sensor LAN
    device_ready      it is reachable AND reports healthy/ready
    device_active     it is actually doing its job
    producing         output is flowing

The chain is UNIVERSAL; the words and the rung subset are per system class. A
radar emits, a camera streams, an EW receiver transmits, a relay serves — and a
generator has two rungs, not eight. Baking `radar_*` into the names would have
made every later system either a lie or a special case. See CLASS_PROFILE.

A radar that is powered down fails four of those. A radar that is emitting into
an empty sky fails one. A node that fell off the mesh fails five. Rolled into a
single red dot they are indistinguishable, and the operator learns nothing about
what to do next.

THE RULE THAT MAKES IT USABLE. A rung that is not attained is only a FAULT if it
was EXPECTED. A site may power its radar only when it needs one, so
`device_detected` through `producing` being unattained is that site's normal
resting state — and a surface that paints that red teaches operators to ignore
it, and then a genuinely absent node goes unnoticed.

So each rung carries an expectation, and the chain reports where it BREAKS:

  - every expected rung attained            -> `ready`
  - attained as far as expected, no further -> `at-rest`   (calm, not a fault)
  - an expected rung unattained             -> `broken` AT that rung, named

Everything above a break is `unknown`, never `failed`. If the node is off the
mesh we cannot see the radar, and claiming the radar is down would be inventing
an observation — the same false-negative-as-fact error that a false green makes
in the other direction.

ALWAYS SOMETHING USEFUL. Even fully at rest the chain carries evidence: which
rung was last attained, when, and what the last observation of each higher rung
was. "Radar last seen 3 days ago, node healthy, kids running" is a useful answer
to "what is happening at this site" — and for a duty-cycled sensor it is the
usual answer.
"""

from __future__ import annotations

from dataclasses import dataclass, field
from typing import Any, Iterable, Sequence

# The chain, in order. Order is load-bearing: a rung can only be judged once
# everything below it is attained, because below-failures make it unobservable.
RUNGS: tuple[str, ...] = (
    "declared",
    "power",
    "node_online",
    "components",
    "device_detected",
    "device_ready",
    "device_active",
    "producing",
)

RUNG_LABEL = {
    "declared": "Declared",
    "power": "Power",
    "node_online": "Node online",
    "components": "Components running",
    "device_detected": "On LAN",
    "device_ready": "Ready",
    "device_active": "Active",
    "producing": "Producing",
}

# Per-class vocabulary. The CHAIN is universal; the words are not. A radar
# emits, a camera streams, an EW system transmits, a relay serves — and calling
# all of them "emitting" would be the sensor-family assumption baked back in.
# Same display-only discipline as the portal lexicon: words change at serve
# time, the rung names on the wire never do.
#
# `rungs` is the SUBSET that applies. Not every system has eight: a generator
# has two, a compute node stops at its components, a radar runs the full chain.
# Rungs outside a class's profile are not rendered and never count as breaks —
# a pod is not "missing" an emitting state it was never going to have.
CLASS_PROFILE: dict[str, dict[str, Any]] = {
    "radar": {
        "rungs": RUNGS,
        "labels": {"device_detected": "Radar on LAN", "device_ready": "Radar ready",
                   "device_active": "Emitting", "producing": "Producing detections"},
        "remedies": {"device_active": "Radar is ready but not radiating — check whether it was commanded on."},
    },
    "eo-ir": {
        "rungs": RUNGS,
        "labels": {"device_detected": "Camera on LAN", "device_ready": "Camera ready",
                   "device_active": "Streaming", "producing": "Producing imagery"},
        "remedies": {"device_active": "Camera is ready but not streaming — check the stream was started."},
    },
    "ew": {
        "rungs": RUNGS,
        "labels": {"device_detected": "Receiver on LAN", "device_ready": "Receiver ready",
                   "device_active": "Transmitting", "producing": "Producing cuts"},
    },
    "effector": {
        "rungs": RUNGS,
        "labels": {"device_detected": "Effector on LAN", "device_ready": "Ready",
                   "device_active": "Armed", "producing": "Reporting engagements"},
        "remedies": {"device_ready": "Effector answers but is not reporting ready — check its own status."},
    },
    "comms": {
        # A relay carries traffic; there is no separate device below it.
        "rungs": ("declared", "power", "node_online", "components", "producing"),
        "labels": {"components": "Services running", "producing": "Carrying traffic"},
    },
    "node": {
        # Compute/integration host. Its job ends at its own components.
        "rungs": ("declared", "power", "node_online", "components"),
        "labels": {"components": "Components running"},
    },
    "pod": {
        "rungs": ("declared", "power", "node_online", "components"),
    },
    "generator": {
        # The thing everything else stands on. Two rungs, usually attested.
        "rungs": ("declared", "power"),
        "labels": {"power": "Running"},
    },
}

DEFAULT_CLASS = "node"


def profile_for(cls: str) -> dict[str, Any]:
    return CLASS_PROFILE.get(cls, CLASS_PROFILE[DEFAULT_CLASS])


def label_for(cls: str, rung: str) -> str:
    return profile_for(cls).get("labels", {}).get(rung) or RUNG_LABEL.get(rung, rung)


def remedy_for(cls: str, rung: str) -> str:
    return profile_for(cls).get("remedies", {}).get(rung) or RUNG_REMEDY.get(rung, "")

# What a broken rung means, in the operator's terms. The point of the chain is
# that these are all different jobs.
RUNG_REMEDY = {
    "power": "Generator or mains is down — everything above this is dark because of it.",
    "node_online": "Host is off the mesh — check power, link, and enrolment.",
    "components": "Host is up but its integration components are not running.",
    "device_detected": "Device does not answer on the sensor LAN — check power and cabling.",
    "device_ready": "Device answers but is not reporting ready — check its own status.",
    "device_active": "Device is ready but not active — check whether it was commanded on.",
    "producing": "Active, but nothing is arriving — check the decode and publish path.",
}

ATTAINED, UNATTAINED, UNKNOWN = "attained", "unattained", "unknown"

# How we know. Kept on the wire and rendered differently, because a human
# saying "the generator is running" and a sensor reporting it are not the same
# claim and must never look the same. An attestation is a person's word, with a
# name and a timestamp on it; a measurement is a machine's observation.
MEASURED, ATTESTED, NO_SOURCE = "measured", "attested", "none"
# Derived from a higher rung rather than observed at this one. Kept distinct
# from MEASURED because "we inferred the generator is running because the node
# is publishing" and "we read the generator" are different strengths of claim,
# and an operator deciding whether to drive out to a site needs to know which
# one they are looking at.
IMPLIED = "implied"


@dataclass
class Rung:
    name: str
    state: str = UNKNOWN          # attained | unattained | unknown
    expected: bool = True         # is this rung supposed to be attained NOW?
    detail: str = ""              # a short, concrete observation
    age_s: float | None = None    # how old the evidence is
    last_attained_at: str | None = None  # ISO, when it was last true
    # Provenance. `attested` rungs are a person's word and are rendered as such;
    # they also EXPIRE, because a human report is a snapshot of a moment and
    # silently ages into fiction if treated as standing fact.
    source: str = NO_SOURCE
    attested_by: str = ""
    attested_at: str | None = None
    expires_at: str | None = None
    # Named digital signal that WOULD supersede this attestation once it exists.
    # Written down now so the upgrade is a wiring job rather than a redesign.
    future_signal: str = ""

    def as_dict(self) -> dict[str, Any]:
        d = {
            "name": self.name,
            "label": RUNG_LABEL.get(self.name, self.name),
            "state": self.state,
            "expected": self.expected,
            "detail": self.detail,
            "age_s": self.age_s,
            "last_attained_at": self.last_attained_at,
            "source": self.source,
        }
        if self.source == ATTESTED:
            d.update({
                "attested_by": self.attested_by,
                "attested_at": self.attested_at,
                "expires_at": self.expires_at,
            })
        if self.future_signal:
            d["future_signal"] = self.future_signal
        return d


@dataclass
class Chain:
    """One subject's readiness chain — a site, or one radar within it."""

    subject: str
    rungs: list[Rung] = field(default_factory=list)
    cls: str = DEFAULT_CLASS

    def by_name(self) -> dict[str, Rung]:
        return {r.name: r for r in self.rungs}

    @property
    def attained_through(self) -> str | None:
        """The highest rung attained with everything below it attained too."""
        top = None
        for name in tuple(profile_for(self.cls).get("rungs", RUNGS)):
            r = self.by_name().get(name)
            if r is None or r.state != ATTAINED:
                break
            top = name
        return top

    @property
    def break_at(self) -> str | None:
        """First EXPECTED rung we LOOKED AT and found missing.

        Only `unattained` counts. A rung that is `unknown` is not a break — we
        did not look, or could not tell, and "nobody has walked out to check the
        generator" is not the generator failing. Treating unknown as broken
        would make every un-instrumented thing a permanent alarm, which is the
        same false-alarm class the expectation rule exists to prevent.
        """
        for name in tuple(profile_for(self.cls).get("rungs", RUNGS)):
            r = self.by_name().get(name)
            if r is None:
                continue
            if r.expected and r.state == UNATTAINED:
                return name
        return None

    @property
    def verdict(self) -> str:
        """`ready` | `at-rest` | `broken` | `unknown`.

        `at-rest` is a first-class answer, not a degraded one: the chain got as
        far as anyone expects it to right now. It is what a healthy duty-cycled site looks
        like for most of its life, and it must never render as a fault.
        """
        if not self.rungs:
            return "unknown"
        b = self.break_at
        if b is not None:
            return "broken"
        # Nothing is known-missing, but if something we EXPECT is unobserved we
        # cannot claim the chain is fine. `unknown` is the honest verdict, and
        # it is distinct from `broken` because the action differs: go and look.
        applicable_rungs = tuple(profile_for(self.cls).get("rungs", RUNGS))
        for name in applicable_rungs:
            r = self.by_name().get(name)
            if r is not None and r.expected and r.state == UNKNOWN:
                return "unknown"
        # Nothing expected is missing. Did we reach the top, or stop early on
        # purpose?
        top = self.attained_through
        applicable = tuple(profile_for(self.cls).get("rungs", RUNGS))
        if top == applicable[-1]:
            return "ready"
        return "at-rest"

    def as_dict(self) -> dict[str, Any]:
        b = self.break_at
        return {
            "subject": self.subject,
            "class": self.cls,
            "verdict": self.verdict,
            "attained_through": self.attained_through,
            "break_at": b,
            "remedy": remedy_for(self.cls, b or ""),
            "rungs": [{**r.as_dict(), "label": label_for(self.cls, r.name)} for r in self.rungs],
        }


def build_chain(
    subject: str,
    *,
    cls: str = DEFAULT_CLASS,
    declared: bool = True,
    power: bool | None = None,
    node_online: bool | None = None,
    components_ok: bool | None = None,
    components_detail: str = "",
    device_detected: bool | None = None,
    device_ready: bool | None = None,
    device_active: bool | None = None,
    producing: bool | None = None,
    duty_cycled: bool = True,
    evidence: dict[str, dict[str, Any]] | None = None,
) -> Chain:
    """Assemble a chain from observations.

    `None` means NOT OBSERVED and becomes `unknown` — never `unattained`. The
    distinction matters: "we did not look" and "we looked and it was not there"
    justify different actions, and conflating them is how a monitoring surface
    starts asserting things it never measured.

    `duty_cycled` marks a sensor that is deliberately powered only when needed
    (a radar powered only for threat reporting). Its radar rungs stop being EXPECTED, so the
    chain reads `at-rest` rather than `broken` when it is off. Set it False for
    a sensor that is supposed to be radiating continuously, and the same absence
    becomes a fault — which is the entire point of declaring it.
    """
    ev = evidence or {}
    obs = {
        "declared": declared,
        "power": power,
        "node_online": node_online,
        "components": components_ok,
        "device_detected": device_detected,
        "device_ready": device_ready,
        "device_active": device_active,
        "producing": producing,
    }
    details = {"components": components_detail}

    prof = profile_for(cls)
    applicable: tuple[str, ...] = tuple(prof.get("rungs", RUNGS))
    # Lower rungs are IMPLIED by any higher rung that is directly observed.
    # You cannot run components on an unpowered host, so a node publishing its
    # own health is direct evidence that power is on — requiring a human to
    # confirm that would be circular, and would leave a demonstrably-working
    # site reading `unknown` until someone walked outside.
    #
    # The implication is marked as derived, never presented as a measurement of
    # the rung itself. What an attestation still adds is WHICH source is
    # carrying, and its condition — "running on the last generator, 1/4 tank"
    # is not something the node's own liveness can tell you.
    highest_observed = None
    for i, name in enumerate(applicable):
        if obs.get(name) is True:
            highest_observed = i
    implied: set[str] = set()
    if highest_observed is not None:
        for i, name in enumerate(applicable[:highest_observed]):
            if obs.get(name) is None:
                obs[name] = True
                implied.add(name)
                details[name] = f"implied by {RUNG_LABEL.get(applicable[highest_observed], 'a higher rung')}"

    rungs: list[Rung] = []
    below_broken = False
    for name in applicable:
        v = obs.get(name)
        if below_broken:
            # Unobservable, not failed. We cannot see a radar through a node
            # that is off the mesh, and saying otherwise would be fiction.
            state = UNKNOWN
        elif v is None:
            state = UNKNOWN
        else:
            state = ATTAINED if v else UNATTAINED

        # Radar rungs are only EXPECTED when the sensor is not duty-cycled.
        expected = True
        if duty_cycled and name in ("device_detected", "device_ready", "device_active", "producing"):
            expected = False

        e = ev.get(name, {})
        detail = e.get("detail") or details.get(name, "")
        source = NO_SOURCE
        if name in implied and state == ATTAINED:
            # The implication OUTRANKS the evidence text here. The evidence
            # described an observation that did not land ("no discovery
            # output"); printing it beside `attained` reads as a contradiction
            # and is exactly the confident-wrong-answer this model exists to
            # avoid. The failed observation is still worth saying — a device
            # that is demonstrably producing while discovery cannot see it
            # means DISCOVERY is blind, which is a finding in its own right —
            # so it is kept, demoted to a parenthetical.
            source = IMPLIED
            detail = details.get(name, "")
            if e.get("detail"):
                detail = f"{detail} (not directly observed: {e['detail']})"
        elif e.get("detail") or details.get(name):
            source = MEASURED
        rungs.append(
            Rung(
                name=name,
                state=state,
                expected=expected,
                detail=detail,
                source=source,
                age_s=e.get("age_s"),
                last_attained_at=e.get("last_attained_at"),
            )
        )
        if state != ATTAINED:
            below_broken = True

    return Chain(subject=subject, rungs=rungs, cls=cls)


def roll_up(chains: Iterable[Chain]) -> str:
    """Site verdict from its subjects' chains — worst first, at-rest is not a fault.

    Ordering is deliberate: `broken` beats everything; `unknown` beats `at-rest`
    (not knowing is worse than knowingly idle); `at-rest` beats `ready` only in
    the sense that it is reported, never as an alarm.
    """
    verdicts = [c.verdict for c in chains]
    if not verdicts:
        return "unknown"
    for v in ("broken", "unknown", "at-rest"):
        if v in verdicts:
            return v
    return "ready"


def roll_up_reason(chains: Sequence[Chain], with_counts: bool = True) -> str:
    """Name the subjects that drove the site verdict, and how many did not.

    A site badge alone is a worse answer than it looks: one unreadable
    auxiliary node drags a whole working site to `unknown`, and an operator
    seeing only the badge cannot tell that from a site that is genuinely dark.
    Saying "unknown — node-b unreadable; 2 of 3 subjects ready" keeps the
    conservative verdict while making it actionable.

    `with_counts` is a DISCLOSURE control, not formatting. Every count here is
    a denominator — "2 of 3 ready" states how much kit exists and how much of
    it works, which is the order of battle plus the gap. Below the `full` tier
    the reason still names the driving subject, because the operator who owns
    that site needs to know which one to go and look at, but the tallies are
    withheld.
    """
    chains = list(chains)
    if not chains:
        return "no subjects declared" if with_counts else "nothing reporting"
    verdict = roll_up(chains)
    drivers = [c.subject for c in chains if c.verdict == verdict]
    word = {"unknown": "unreadable", "broken": "broken",
            "at-rest": "at rest", "ready": "ready"}[verdict]
    if verdict == "ready":
        if not with_counts:
            return "everything expected is running"
        return f"all {len(chains)} subject{'s' if len(chains) != 1 else ''} ready"
    named = ", ".join(drivers[:3]) + (f" +{len(drivers) - 3} more" if len(drivers) > 3 else "")
    if not with_counts:
        return f"{named} {word}"
    rest = len(chains) - len(drivers)
    tail = f"; {rest} other subject{'s' if rest != 1 else ''} reporting" if rest else ""
    return f"{named} {word}{tail}"


# Verdict -> the honesty colour the rest of the surface already speaks.
# `at-rest` is IDLE, never amber: a deliberately-off sensor is not a degradation.
VERDICT_COLOR = {
    "ready": "green",
    "at-rest": "idle",
    "broken": "amber",
    "unknown": "unmetered",
}
