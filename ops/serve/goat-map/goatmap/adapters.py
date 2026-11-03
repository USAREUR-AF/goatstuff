# SPDX-License-Identifier: Apache-2.0
"""Adapters — real published signals into readiness rungs.

See docs/design/site-readiness-chain.md.

Each adapter takes one real payload and answers ONE rung with a tri-state:
True (attained), False (looked, not there), None (did not look / cannot tell).
Keeping `None` distinct from `False` all the way down is the whole reason the
chain can say "unknown" instead of inventing an observation.

WHY NOT JUST USE `overall`. `NodeHealthV1` carries an `overall` that is the max
severity across every probe, and on a healthy duty-cycled site it reads **`down`**
while `mesh`, `zenoh_local`, `zenoh_peer_north`, `time_sync`,
`storage` and `luks_data_volume` are all `ok`. The `down` comes from `kids`,
which stopped because the radar is off, which is that site's normal posture.

Mapping `overall` onto a rung would import that conflation wholesale and paint a
healthy site red. So the adapters read SPECIFIC probes, and split two things the
single number fuses:

  - **infrastructure components** (launcher, fabric client, bridge, converters)
    are expected to run always — they are what makes the node useful at all;
  - **per-sensor kids** follow the sensor's duty cycle. A kid with nothing to
    relay is not a fault, so kids inform `producing` — the decode-and-publish
    path — and never `components`.

Getting that split wrong is the difference between a surface that is trusted and
one that cries wolf for most of every week.
"""

from __future__ import annotations

import time
from typing import Any

# NodeHealthV1 probe states, worst last.
_OK, _DEGRADED, _DOWN, _UNKNOWN = "ok", "degraded", "down", "unknown"

# Probes that describe the node's own ability to do its job. Deliberately does
# NOT include `kids` (duty-cycled) or `radar_link` (that is a device rung).
INFRA_PROBES = ("zenoh_local", "zenoh_peer_north", "storage", "luks_data_volume")

# Probes that say the host is reachable/participating rather than merely alive.
MESH_PROBES = ("mesh",)


def _state(payload: dict[str, Any], probe: str) -> str | None:
    m = (payload.get("metrics") or {}).get(probe)
    if not isinstance(m, dict):
        return None
    s = m.get("state")
    return s if isinstance(s, str) else None


def _tri(state: str | None, degraded_is_ok: bool = True) -> bool | None:
    """Probe state -> tri-state. `unknown` and missing both stay None."""
    if state is None or state == _UNKNOWN:
        return None
    if state == _OK:
        return True
    if state == _DEGRADED:
        # Degraded is still doing its job. Calling it a break would make every
        # imperfect site read as broken and bury the ones that actually are.
        return True if degraded_is_ok else False
    return False  # down


def stale(payload: dict[str, Any], max_age_s: float, now: float | None = None) -> bool:
    """Is this reading too old to believe? Stale evidence answers nothing."""
    ts = payload.get("ts_epoch")
    if not isinstance(ts, (int, float)):
        return True
    return ((now if now is not None else time.time()) - ts) > max_age_s


def components_from_node_health(
    payload: dict[str, Any], max_age_s: float = 120, now: float | None = None
) -> tuple[bool | None, str]:
    """`components` rung from NodeHealthV1 — infrastructure probes only.

    Returns (tri-state, human detail). A stale payload answers None: an old
    reading is not evidence about now, in either direction.
    """
    if not payload or stale(payload, max_age_s, now):
        return None, "no recent node health"

    seen = {p: _state(payload, p) for p in INFRA_PROBES}
    known = {p: s for p, s in seen.items() if s is not None}
    if not known:
        return None, "no infrastructure probes reported"

    bad = sorted(p for p, s in known.items() if s == _DOWN)
    if bad:
        return False, f"down: {', '.join(bad)}"
    degraded = sorted(p for p, s in known.items() if s == _DEGRADED)
    if degraded:
        return True, f"degraded: {', '.join(degraded)}"
    return True, f"{len(known)} infrastructure probes ok"


def node_online_from_node_health(
    payload: dict[str, Any], max_age_s: float = 120, now: float | None = None
) -> tuple[bool | None, str]:
    """Corroborating view of `node_online` from the node's own mesh probe.

    NOTE this is self-reported, so it is corroboration and NEVER the authority:
    a node that is off the mesh publishes nothing at all, and its silence is the
    real signal. The management peer registry is the vantage that survives the
    failure. Use this only to enrich a rung the registry already decided.
    """
    if not payload or stale(payload, max_age_s, now):
        return None, "no recent node health"

    # ARRIVAL IS THE PROOF, and it is the only honest one available here.
    #
    # The node's SELF-REPORTED mesh probe can contradict itself: a node delivers
    # NodeHealthV1 over the mesh while that probe still reads down. Trusting the
    # probe says `unattained` ("we looked, it is offline"), breaks the ladder
    # there, turns every rung above it `unknown`, and rolls the SITE up to
    # `broken` while the node is healthy and publishing. A false alarm is worse
    # than the `unknown` it would replace.
    #
    # A message cannot arrive from a node that is off the mesh. So a fresh
    # payload settles this rung affirmatively, and the probe's own opinion goes
    # in the DETAIL where it informs without overriding the evidence of
    # delivery. This never returns False: absence is handled above as unknown,
    # which is the doctrine this module already states — mesh health is judged
    # from the far end by ABSENCE, never by self-report, because a node whose
    # mesh is genuinely broken cannot tell you so.
    mesh = _state(payload, MESH_PROBES[0])
    if mesh == _DOWN:
        return True, "health arriving; node self-reports its mesh link DOWN"
    if mesh == _DEGRADED:
        return True, "health arriving; node self-reports its mesh link degraded"
    return True, "health arriving from this node"


def producing_from_node_health(
    payload: dict[str, Any], max_age_s: float = 120, now: float | None = None
) -> tuple[bool | None, str]:
    """`producing` from the kids probe — the decode-and-publish path.

    Kids follow the sensor's duty cycle, which is exactly why they belong here
    and not in `components`. With the device off they are legitimately down and
    the chain reads at-rest; with the device emitting, kids down is a real break
    in the publish path.
    """
    if not payload or stale(payload, max_age_s, now):
        return None, "no recent node health"
    s = _state(payload, "kids")
    if s is None or s == _UNKNOWN:
        return None, "kids state unknown"
    if s == _OK:
        return True, "kids relaying"
    return False, f"kids {s}"


def device_detected_from_discovery(
    payload: dict[str, Any], device_type: str, max_age_s: float = 900,
    now: float | None = None,
) -> tuple[bool | None, str]:
    """`device_detected` from a node discovery service's active-radars.json.

    Real shape:
        {"schema_version":1, "node_id":..., "discovered_at":"...",
         "scan_subnet":"192.168.170.0/24", "radars":{"radar-a":[], "radar-b":[], ...}}

    An empty list is a REAL negative — discovery scanned the subnet and the
    device did not answer. A missing key is not: it means this build does not
    look for that device type, which is a gap in coverage rather than an absent
    device, and must not be reported as one.
    """
    if not payload:
        return None, "no discovery output"

    at = payload.get("discovered_at")
    if at:
        try:
            from datetime import datetime
            ts = datetime.fromisoformat(at.replace("Z", "+00:00")).timestamp()
            if ((now if now is not None else time.time()) - ts) > max_age_s:
                return None, "discovery output stale"
        except (ValueError, TypeError):
            return None, "discovery timestamp unreadable"

    radars = payload.get("radars")
    if not isinstance(radars, dict) or device_type not in radars:
        # Coverage gap, not an absence.
        return None, f"discovery does not scan for {device_type}"

    found = radars.get(device_type) or []
    subnet = payload.get("scan_subnet", "the sensor LAN")
    if found:
        return True, f"{len(found)} on {subnet}"
    return False, f"scanned {subnet}, none answered"


def node_online_from_mesh(
    mesh: Any, peer_fqdn: str, max_age_s: float = 300, now: float | None = None
) -> tuple[bool | None, str]:
    """`node_online` from the MESH VANTAGE — the authority, not self-reporting.

    This is the answer to a node that publishes nothing. A node with no
    NodeHealthV1 publisher can still be Connected over P2P in NetBird; reading
    `node_online` from a node's own health would report that live peer as
    unknown indefinitely. The vantage needs nothing installed on
    the peer, which is what lets a site say something useful about kit that
    reports nothing at all.

    Three states, and the third is the one that matters:
      - the peer is in the reading and connected  -> True
      - the peer is in the reading and not        -> False, a real negative
      - the peer is ABSENT from the reading       -> None

    Absence is not offline. A peer missing from the vantage's own list means
    the vantage did not assess it — a roster typo, a peer enrolled after the
    snapshot, a scope mismatch — and calling that "offline" would invent an
    observation, the same error `unknown`-vs-`unattained` exists to prevent
    everywhere else in this model.

    A stale reading answers nothing in either direction: a vantage that stopped
    publishing an hour ago cannot tell us who is up NOW, and its last frame
    would otherwise freeze into permanent fact.
    """
    if mesh is None or not peer_fqdn:
        return None, "no mesh vantage reading"
    ts_ms = getattr(mesh, "ts_unix_ms", 0) or 0
    if ts_ms:
        age = (time.time() if now is None else now) - (ts_ms / 1000.0)
        if age > max_age_s:
            return None, f"mesh vantage reading stale ({age / 60:.0f}m old)"
    try:
        link = mesh.by_peer().get(peer_fqdn)
    except Exception:  # noqa: BLE001 - a malformed reading must not decide a rung
        return None, "mesh vantage reading unreadable"
    if link is None:
        # Say WHICH name we looked for: a silent miss here reads identically to
        # a down peer on the surface, and the usual cause is a roster typo.
        return None, f"{peer_fqdn} not assessed by the vantage"
    if link.connected:
        return True, f"connected ({link.status or 'mesh vantage'})"
    return False, f"not connected ({link.status or 'mesh vantage'})"


def power_from_attestation(attains: bool | None, detail: str = "") -> tuple[bool | None, str]:
    """`power` from an attestation. Thin on purpose — the honesty lives in
    `attest.Attestation.attains`, which returns None when expired rather than
    letting a stale human report stand in as current fact."""
    return attains, detail or "attested"
