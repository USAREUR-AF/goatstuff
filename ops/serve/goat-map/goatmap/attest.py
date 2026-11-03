# SPDX-License-Identifier: Apache-2.0
"""Manual attestations — the parts of a site that nothing reports digitally yet.

See docs/design/site-readiness-chain.md §"Attested rungs".

Some of the most consequential kit on a site has no telemetry at all. The
generators powering the comms equipment, and the generators powering the
tactical radars, are the clearest cases: when
one runs dry, everything above it goes dark, and the surface currently shows a
node that "mysteriously" fell off the mesh. The cause was knowable — it just was
not knowable to us.

So a person can put it on the record. Three rules make that safe:

1. **An attestation is never a measurement.** It carries a name and a time and
   is rendered differently everywhere. Presenting someone's word as a sensor
   reading is how a surface starts asserting things nobody observed.

2. **It expires.** "Generator refuelled" is true at 09:00 and says nothing at
   all about 22:00. An attestation past its TTL degrades to `unknown` — never
   silently carried forward, and never quietly dropped either: the surface says
   "last attested 14h ago by X, expired", which is itself useful.

3. **A digital signal always wins.** Every attestable subject names the signal
   that would supersede it, so wiring one up later is a config change rather
   than a redesign, and the manual entry becomes corroboration instead of
   evidence. The upgrade path is written down before it is needed, because the
   alternative is discovering at integration time that the manual scheme
   modelled something the sensor cannot express.

The vocabulary is deliberately small. An operator standing at a generator in
the dark should be choosing between four words, not filling in a form.
"""

from __future__ import annotations

import time
from dataclasses import dataclass, field
from datetime import datetime, timezone
from typing import Any

# What a person can say about a subject. Small on purpose.
RUNNING = "running"        # it is on and doing its job
DEGRADED = "degraded"      # on, but not right — low fuel, overheating, intermittent
DOWN = "down"              # off, failed, or empty
UNKNOWN_STATE = "unknown"  # explicitly "I could not tell" — a real answer

STATES = (RUNNING, DEGRADED, DOWN, UNKNOWN_STATE)

# Attested state -> whether the rung it backs counts as attained.
_ATTAINS = {RUNNING: True, DEGRADED: True, DOWN: False, UNKNOWN_STATE: None}

# Default lifetime of a human report. Twelve hours is a shift: long enough that
# nobody is re-attesting hourly, short enough that a stale note cannot carry a
# site through a night in which everything changed.
DEFAULT_TTL_S = 12 * 3600


@dataclass
class Attestation:
    """One person's report about one thing, at one time."""

    subject: str            # e.g. "site-a/generator-comms"
    rung: str               # which readiness rung this backs, e.g. "power"
    state: str              # running | degraded | down | unknown
    by: str                 # who said it — attribution is the point
    at: float               # unix seconds
    note: str = ""          # free text: "3/4 tank", "runs 20 min then cuts"
    ttl_s: int = DEFAULT_TTL_S

    @property
    def expires_at(self) -> float:
        return self.at + self.ttl_s

    def expired(self, now: float | None = None) -> bool:
        return (now if now is not None else time.time()) >= self.expires_at

    def age_s(self, now: float | None = None) -> float:
        return round((now if now is not None else time.time()) - self.at, 1)

    def attains(self, now: float | None = None) -> bool | None:
        """Does this back its rung as attained? None = unknown/expired.

        An expired attestation returns None rather than False: the generator did
        not become broken because nobody walked out to look at it. Treating
        silence as failure is as dishonest as treating it as health.
        """
        if self.expired(now):
            return None
        return _ATTAINS.get(self.state)

    def as_dict(self, now: float | None = None) -> dict[str, Any]:
        return {
            "subject": self.subject,
            "rung": self.rung,
            "state": self.state,
            "by": self.by,
            "at": _iso(self.at),
            "age_s": self.age_s(now),
            "note": self.note,
            "expires_at": _iso(self.expires_at),
            "expired": self.expired(now),
            "source": "attested",
        }


def _iso(ts: float) -> str:
    return datetime.fromtimestamp(ts, tz=timezone.utc).isoformat(timespec="seconds")


@dataclass
class AttestableSubject:
    """Something a person can report on, and the signal that would replace them.

    `future_signal` is not documentation. It is the integration contract: when
    that signal exists, it is wired to this subject and the manual entry stops
    being evidence. Naming it now forces the question "what would the sensor
    actually tell us?" while the model is still cheap to change.
    """

    subject_id: str
    label: str
    rung: str = "power"
    site_id: str = ""
    future_signal: str = ""
    ttl_s: int = DEFAULT_TTL_S

    def as_dict(self) -> dict[str, Any]:
        return {
            "subject_id": self.subject_id,
            "label": self.label,
            "rung": self.rung,
            "site_id": self.site_id,
            "future_signal": self.future_signal or None,
            "ttl_s": self.ttl_s,
        }


@dataclass
class AttestationLog:
    """Latest attestation per subject. Append-only in spirit; last word wins."""

    subjects: dict[str, AttestableSubject] = field(default_factory=dict)
    latest: dict[str, Attestation] = field(default_factory=dict)

    def declare(self, s: AttestableSubject) -> None:
        self.subjects[s.subject_id] = s

    def record(self, a: Attestation) -> None:
        if a.state not in STATES:
            raise ValueError(f"unknown attested state {a.state!r}; expected one of {STATES}")
        self.latest[a.subject] = a

    def for_site(self, site_id: str, now: float | None = None) -> list[dict[str, Any]]:
        """Every attestable subject at a site, WITH its gaps.

        Subjects that have never been attested are included with
        `attestation: None`, because "nobody has ever reported on the radar
        generator" is exactly the kind of hole this exists to make visible. A
        list of only the things people bothered to log would flatter the site.
        """
        out: list[dict[str, Any]] = []
        for s in sorted(self.subjects.values(), key=lambda x: x.subject_id):
            if site_id and s.site_id != site_id:
                continue
            a = self.latest.get(s.subject_id)
            out.append({
                **s.as_dict(),
                "attestation": a.as_dict(now) if a else None,
                "stale": bool(a and a.expired(now)),
                "never_attested": a is None,
            })
        return out

    def rung_state(self, subject_id: str, now: float | None = None) -> bool | None:
        a = self.latest.get(subject_id)
        return a.attains(now) if a else None
