# SPDX-License-Identifier: Apache-2.0
"""Disclosure tiers — how much of the readiness picture a given viewer receives.

See docs/design/site-readiness-chain.md §"The asymmetry problem".

THE TENSION. A readiness surface has to make weakness legible, because that is
the entire point — an operator cannot fix what they cannot see. But a clear
picture of weakness is also a targeting and timing product: where to strike, and
when. One screenshot of a fleet readiness view is worth more to an adversary
than most documents, and it does not need to be exfiltrated by an intruder — a
legitimate viewer's screen is enough.

So the surface is built to be maximally useful to someone who already knows the
estate, and close to useless to someone who does not.

THREE RULES, in priority order.

1. **The denominator is the crown jewel.** Readiness is only exploitable when
   you know what SHOULD be there. "Three systems online" tells an adversary
   very little; "three of eight online" hands over the order of battle and the
   size of the gap in one number. So `declared` counts are the most tightly held
   element in the model — more restricted than any failure state — and they are
   never emitted alongside live state below the top tier.

2. **Aggregate weakness outranks component weakness.** One radar down is
   tactical and perishable. "This region is 40% ready" is strategic, durable,
   and fits on one screen. This inverts the usual dashboard instinct that
   summaries are for wide audiences: here the ROLL-UP is more sensitive than the
   detail, so tallies of how many things are broken are withheld at exactly the
   altitudes where dashboards normally shout them.

3. **Absence patterns are themselves intelligence.** A region rendering six
   sites with four of them dark discloses posture even with every label
   stripped. So below the top tier a viewer sees the worst STATE of what they
   are scoped to, never a census of how much of it is failing.

WHAT THIS IS NOT. It is not obscurity as a substitute for access control. The
zero-trust plumbing already decides WHO sees a site at all — per-viewer scoping,
server-side filtering, the data-access role grammar. This module decides HOW
MUCH DETAIL a permitted viewer receives, so that a screen capture of a
legitimate session degrades gracefully instead of becoming a complete product.
Both layers are required; neither substitutes for the other.
"""

from __future__ import annotations

from typing import Any

# Tiers, least to most disclosing.
PRESENCE = "presence"    # something is here, and one colour. No structure.
STATE = "state"          # rung states + remedies. No denominators, no tallies.
FULL = "full"            # everything, including declared counts and gaps.

TIERS = (PRESENCE, STATE, FULL)
_RANK = {PRESENCE: 0, STATE: 1, FULL: 2}


def at_least(tier: str, floor: str) -> bool:
    return _RANK.get(tier, 0) >= _RANK[floor]


def redact_chain(chain_dict: dict[str, Any], tier: str) -> dict[str, Any]:
    """Reduce one readiness chain to what `tier` permits.

    `presence`  -> subject + verdict. Enough to know something is there and
                   roughly how it is; not enough to plan against.
    `state`     -> rung states and the remedy for a break, so an operator can
                   act. Still no declared-vs-actual arithmetic.
    `full`      -> unmodified.
    """
    if tier == FULL:
        return chain_dict

    out: dict[str, Any] = {
        "subject": chain_dict.get("subject"),
        "verdict": chain_dict.get("verdict"),
        "disclosure": tier,
    }
    if tier == PRESENCE:
        # Deliberately NOT break_at: naming the rung that failed is naming the
        # weakness precisely enough to act on from outside.
        return out

    out["break_at"] = chain_dict.get("break_at")
    out["remedy"] = chain_dict.get("remedy")
    out["attained_through"] = chain_dict.get("attained_through")
    # Rung states, minus provenance detail that identifies people and minus
    # free-text notes that routinely carry more than intended ("gen 2 dead,
    # running on the last one").
    out["rungs"] = [
        {
            "name": r.get("name"),
            "label": r.get("label"),
            "state": r.get("state"),
            "expected": r.get("expected"),
            "source": r.get("source"),
        }
        for r in chain_dict.get("rungs", [])
    ]
    return out


def redact_site(site_dict: dict[str, Any], tier: str) -> dict[str, Any]:
    """Reduce a site payload. Strips the counts that form the denominator."""
    out = dict(site_dict)
    out["disclosure"] = tier
    if tier == FULL:
        return out

    # RULE 1: the denominator never leaves below `full`. `counts` carries
    # total-vs-degraded, which is the order of battle plus the gap.
    out.pop("counts", None)

    if tier == PRESENCE:
        out.pop("systems", None)
        return out

    # `state`: systems are named with their state — an operator working the site
    # needs that — but the list is not accompanied by a declared total, so a
    # capture cannot tell whether it is seeing all of them.
    out["systems"] = [
        {"name": s.get("name"), "class": s.get("class"), "state": s.get("state")}
        for s in site_dict.get("systems", [])
    ]
    return out


def region_summary(site_verdicts: list[str], tier: str) -> dict[str, Any]:
    """A region badge. The one place tallies are most dangerous.

    Below `full` this returns the WORST state and nothing else — no count of
    sites, no count of broken sites, no ratio. A viewer scoped to the region
    still learns "something here needs attention", which is what lets them act;
    a capture learns nothing about how much of the region is degraded, which is
    what would let an adversary prioritise.
    """
    worst = "unknown"
    for v in ("broken", "unknown", "at-rest", "ready"):
        if v in site_verdicts:
            worst = v
            break

    out: dict[str, Any] = {"state": worst, "disclosure": tier}
    if tier == FULL:
        out["sites"] = len(site_verdicts)
        out["broken"] = sum(1 for v in site_verdicts if v == "broken")
        out["at_rest"] = sum(1 for v in site_verdicts if v == "at-rest")
    return out


def redact_attestations(rows: list[dict[str, Any]], tier: str) -> list[dict[str, Any]]:
    """Attestations carry names and free text — both leak beyond their subject.

    Below `full`, attribution is dropped (a name is a person to target or
    approach) and notes are dropped (they are written for colleagues and
    routinely say more than the state field). The STATE survives, because that
    is the operational content.

    `never_attested` is withheld below `full` for the same reason as the
    denominator: a list of what nobody is watching is a list of where to look.
    """
    if tier == FULL:
        return rows
    out = []
    for r in rows:
        a = r.get("attestation")
        red = {
            "subject_id": r.get("subject_id"),
            "label": r.get("label"),
            "rung": r.get("rung"),
            "stale": r.get("stale"),
        }
        if a and tier == STATE:
            red["attestation"] = {
                "state": a.get("state"),
                "age_s": a.get("age_s"),
                "expired": a.get("expired"),
                "source": "attested",
            }
        out.append(red)
    return out
