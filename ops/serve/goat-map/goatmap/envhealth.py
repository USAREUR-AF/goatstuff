# SPDX-License-Identifier: Apache-2.0
"""Minimal EnvHealthV1 decoder — the mesh domain the map joins, and the peer
domain (arm 17, goatcore ADR 1232) so a peer reading is decoded rather than
dropped as an error.

Why hand-rolled rather than `protobuf`: this service has two dependencies, and
adding a code-generation toolchain plus a runtime for ONE message shape is a
poor trade against the project's fewest-moving-parts rule. The wire format is
stable and the subset needed here is small — field numbers are pinned by the
canonical schema at
`ops/zenoh-trial-peers/time-health/proto/env_health_v1.proto` and must be kept
in lock-step with it.

The decoder is deliberately TOLERANT of unknown fields: protobuf is
forward-compatible by design, and a meter that gains a field must not blank this
map. Anything unrecognised is skipped by wire type.

It is deliberately INTOLERANT of nonsense: a truncated or non-EnvHealthV1
payload raises, and the caller renders the site unmetered rather than guessing.
"""

from __future__ import annotations

import struct
from dataclasses import dataclass, field

# Wire types
_VARINT, _I64, _LEN, _I32 = 0, 1, 2, 5

# Field numbers — keep in lock-step with env_health_v1.proto.
_ENV_DOMAIN, _ENV_SCOPE, _ENV_TS, _ENV_COLOR = 1, 2, 3, 4
_ENV_DETAIL_MESH = 12
_ENV_DETAIL_PEER = 17

_PEERD_PEERS, _PEERD_VERDICT, _PEERD_PEERING = 1, 2, 9
_PEERD_TLS_ALERTS, _PEERD_TRUST_VERDICT, _PEERD_DECLARED_ABSENT = 12, 13, 14

_PEER_ZID, _PEER_KIND, _PEER_OFFSET, _PEER_RATE = 1, 2, 3, 6
_PEER_NAME, _PEER_GRADE, _PEER_COLOR, _PEER_DIRECT = 11, 13, 14, 16
_PEER_SOURCE_ADDRESS, _PEER_ABSENT_SINCE = 17, 18

_ALERT_NAME, _ALERT_COUNT, _ALERT_PER_MINUTE, _ALERT_TRUST_REFUSAL = 1, 2, 3, 4

# PeerFaultKindV1
PEER_FAULT_CLOCK_AHEAD = 1
# Clients refuse the router's certificate and this declared peer holds no
# session: a candidate, never a confirmed cause.
PEER_FAULT_TRUST_ROOT_REFUSED_CANDIDATE = 2

_MESH_LINKS, _MESH_VANTAGE = 1, 2
_MESH_ALWAYS_UP_ABSENT, _MESH_CONNECTING_STUCK = 5, 6

_LINK_FQDN, _LINK_COLOR, _LINK_CONNECTED = 1, 3, 4
_LINK_CLASS, _LINK_ABSENT_MS, _LINK_STATUS = 11, 12, 13

_MAX_I64 = (1 << 63) - 1
_TWO64 = 1 << 64


class DecodeError(ValueError):
    """Payload is not decodable as EnvHealthV1."""


def _varint(buf: bytes, i: int) -> tuple[int, int]:
    val = 0
    shift = 0
    while True:
        if i >= len(buf):
            raise DecodeError("truncated varint")
        b = buf[i]
        i += 1
        val |= (b & 0x7F) << shift
        if not b & 0x80:
            return val, i
        shift += 7
        if shift > 63:
            raise DecodeError("varint too long")


def _as_i64(v: int) -> int:
    """protobuf int64 is two's complement in a varint — negatives arrive huge."""
    return v - _TWO64 if v > _MAX_I64 else v


def _fields(buf: bytes):
    """Yield (field_number, wire_type, value) with unknown fields skipped."""
    i = 0
    n = len(buf)
    while i < n:
        key, i = _varint(buf, i)
        fnum, wtype = key >> 3, key & 7
        if wtype == _VARINT:
            val, i = _varint(buf, i)
            yield fnum, wtype, val
        elif wtype == _LEN:
            ln, i = _varint(buf, i)
            if i + ln > n:
                raise DecodeError("truncated length-delimited field")
            yield fnum, wtype, buf[i : i + ln]
            i += ln
        elif wtype == _I64:
            if i + 8 > n:
                raise DecodeError("truncated 64-bit field")
            yield fnum, wtype, struct.unpack_from("<d", buf, i)[0]
            i += 8
        elif wtype == _I32:
            if i + 4 > n:
                raise DecodeError("truncated 32-bit field")
            yield fnum, wtype, struct.unpack_from("<f", buf, i)[0]
            i += 4
        else:
            raise DecodeError(f"unsupported wire type {wtype}")


@dataclass
class MeshLink:
    peer_fqdn: str = ""
    color: str = ""
    connected: bool = False
    peer_class: str = ""
    absent_ms: int = -1
    status: str = ""


@dataclass
class MeshHealth:
    domain: str = ""
    scope: str = ""
    ts_unix_ms: int = 0
    overall_color: str = ""
    vantage_host: str = ""
    links: list[MeshLink] = field(default_factory=list)
    always_up_absent: list[str] = field(default_factory=list)
    connecting_stuck: list[str] = field(default_factory=list)

    def by_peer(self) -> dict[str, MeshLink]:
        return {l.peer_fqdn: l for l in self.links if l.peer_fqdn}


@dataclass
class PeerFault:
    zid: str = ""
    kind: int = 0
    name: str = ""
    offset_seconds: float = 0.0
    rejections_per_minute: float = 0.0
    grade: str = ""
    color: str = ""
    # None = unknown: the reader had no sessions file.
    direct_session: bool | None = None
    # A candidate declared by address has zid "" and this set.
    source_address: str = ""
    # Candidates: first reading the peer was seen absent (since reader start).
    absent_since_unix_ms: int | None = None

    @property
    def candidate(self) -> bool:
        """A trust-refusal candidate: not confirmed, not a machine at fault."""
        return self.kind == PEER_FAULT_TRUST_ROOT_REFUSED_CANDIDATE

    @property
    def label(self) -> str:
        """The attributed name, else the Zenoh ID, else the source address."""
        return self.name or self.zid or self.source_address


@dataclass
class TlsAlert:
    """One TLS alert name the router received in the window (identity-free)."""
    name: str = ""
    count_in_window: int = 0
    per_minute: float = 0.0
    trust_refusal: bool = False


@dataclass
class PeerHealth:
    domain: str = ""
    scope: str = ""
    ts_unix_ms: int = 0
    overall_color: str = ""
    verdict: str = ""
    peering: str = ""
    peers: list[PeerFault] = field(default_factory=list)
    # The trust detector: clean|unattributed|candidates|unmeasured; "" from a
    # reader that predates it.
    trust_verdict: str = ""
    tls_alerts: list[TlsAlert] = field(default_factory=list)
    # None = cannot be judged.
    declared_peers_absent: int | None = None


def _decode_peer_fault(buf: bytes) -> PeerFault:
    p = PeerFault()
    for fnum, wtype, val in _fields(buf):
        if fnum == _PEER_ZID and wtype == _LEN:
            p.zid = val.decode("utf-8", "replace")
        elif fnum == _PEER_OFFSET and wtype == _I64:
            p.offset_seconds = val
        elif fnum == _PEER_RATE and wtype == _I64:
            p.rejections_per_minute = val
        elif fnum == _PEER_NAME and wtype == _LEN:
            p.name = val.decode("utf-8", "replace")
        elif fnum == _PEER_GRADE and wtype == _LEN:
            p.grade = val.decode("utf-8", "replace")
        elif fnum == _PEER_COLOR and wtype == _LEN:
            p.color = val.decode("utf-8", "replace")
        elif fnum == _PEER_DIRECT and wtype == _VARINT:
            p.direct_session = bool(val)
        elif fnum == _PEER_KIND and wtype == _VARINT:
            p.kind = _as_i64(val)
        elif fnum == _PEER_SOURCE_ADDRESS and wtype == _LEN:
            p.source_address = val.decode("utf-8", "replace")
        elif fnum == _PEER_ABSENT_SINCE and wtype == _VARINT:
            p.absent_since_unix_ms = _as_i64(val)
    return p


def _decode_tls_alert(buf: bytes) -> TlsAlert:
    a = TlsAlert()
    for fnum, wtype, val in _fields(buf):
        if fnum == _ALERT_NAME and wtype == _LEN:
            a.name = val.decode("utf-8", "replace")
        elif fnum == _ALERT_COUNT and wtype == _VARINT:
            a.count_in_window = val
        elif fnum == _ALERT_PER_MINUTE and wtype == _I64:
            a.per_minute = val
        elif fnum == _ALERT_TRUST_REFUSAL and wtype == _VARINT:
            a.trust_refusal = bool(val)
    return a


def _decode_peer(buf: bytes, out: PeerHealth) -> None:
    for fnum, wtype, val in _fields(buf):
        if fnum == _PEERD_PEERS and wtype == _LEN:
            out.peers.append(_decode_peer_fault(val))
        elif fnum == _PEERD_VERDICT and wtype == _LEN:
            out.verdict = val.decode("utf-8", "replace")
        elif fnum == _PEERD_PEERING and wtype == _LEN:
            out.peering = val.decode("utf-8", "replace")
        elif fnum == _PEERD_TLS_ALERTS and wtype == _LEN:
            out.tls_alerts.append(_decode_tls_alert(val))
        elif fnum == _PEERD_TRUST_VERDICT and wtype == _LEN:
            out.trust_verdict = val.decode("utf-8", "replace")
        elif fnum == _PEERD_DECLARED_ABSENT and wtype == _VARINT:
            out.declared_peers_absent = val


def _decode_link(buf: bytes) -> MeshLink:
    link = MeshLink()
    for fnum, wtype, val in _fields(buf):
        if fnum == _LINK_FQDN and wtype == _LEN:
            link.peer_fqdn = val.decode("utf-8", "replace")
        elif fnum == _LINK_COLOR and wtype == _LEN:
            link.color = val.decode("utf-8", "replace")
        elif fnum == _LINK_CONNECTED and wtype == _VARINT:
            link.connected = bool(val)
        elif fnum == _LINK_CLASS and wtype == _LEN:
            link.peer_class = val.decode("utf-8", "replace")
        elif fnum == _LINK_ABSENT_MS and wtype == _VARINT:
            link.absent_ms = _as_i64(val)
        elif fnum == _LINK_STATUS and wtype == _LEN:
            link.status = val.decode("utf-8", "replace")
    return link


def _decode_mesh(buf: bytes, out: MeshHealth) -> None:
    for fnum, wtype, val in _fields(buf):
        if fnum == _MESH_LINKS and wtype == _LEN:
            out.links.append(_decode_link(val))
        elif fnum == _MESH_VANTAGE and wtype == _LEN:
            out.vantage_host = val.decode("utf-8", "replace")
        elif fnum == _MESH_ALWAYS_UP_ABSENT and wtype == _LEN:
            out.always_up_absent.append(val.decode("utf-8", "replace"))
        elif fnum == _MESH_CONNECTING_STUCK and wtype == _LEN:
            out.connecting_stuck.append(val.decode("utf-8", "replace"))


def decode(payload: bytes) -> MeshHealth:
    """Decode an EnvHealthV1 mesh reading. Raises DecodeError on nonsense."""
    out = MeshHealth()
    saw_mesh = False
    for fnum, wtype, val in _fields(payload):
        if fnum == _ENV_DOMAIN and wtype == _LEN:
            out.domain = val.decode("utf-8", "replace")
        elif fnum == _ENV_SCOPE and wtype == _LEN:
            out.scope = val.decode("utf-8", "replace")
        elif fnum == _ENV_TS and wtype == _VARINT:
            out.ts_unix_ms = _as_i64(val)
        elif fnum == _ENV_COLOR and wtype == _LEN:
            out.overall_color = val.decode("utf-8", "replace")
        elif fnum == _ENV_DETAIL_MESH and wtype == _LEN:
            _decode_mesh(val, out)
            saw_mesh = True
    if out.domain and out.domain != "mesh":
        raise DecodeError(f"not a mesh reading: domain={out.domain!r}")
    if not saw_mesh and not out.domain:
        raise DecodeError("no EnvHealthV1 fields recognised")
    return out


def decode_any(payload: bytes) -> MeshHealth | PeerHealth:
    """Decode an EnvHealthV1 reading of a domain this map knows: mesh or peer.

    Raises DecodeError on nonsense and on any other domain, as `decode` does.
    """
    head = MeshHealth()
    peer_detail: bytes | None = None
    for fnum, wtype, val in _fields(payload):
        if fnum == _ENV_DOMAIN and wtype == _LEN:
            head.domain = val.decode("utf-8", "replace")
        elif fnum == _ENV_DETAIL_PEER and wtype == _LEN:
            peer_detail = val
    if head.domain != "peer" and peer_detail is None:
        return decode(payload)
    if head.domain not in ("", "peer"):
        raise DecodeError(f"peer detail under domain={head.domain!r}")
    out = PeerHealth()
    for fnum, wtype, val in _fields(payload):
        if fnum == _ENV_DOMAIN and wtype == _LEN:
            out.domain = val.decode("utf-8", "replace")
        elif fnum == _ENV_SCOPE and wtype == _LEN:
            out.scope = val.decode("utf-8", "replace")
        elif fnum == _ENV_TS and wtype == _VARINT:
            out.ts_unix_ms = _as_i64(val)
        elif fnum == _ENV_COLOR and wtype == _LEN:
            out.overall_color = val.decode("utf-8", "replace")
    if peer_detail is not None:
        _decode_peer(peer_detail, out)
    return out


def join_states(health: MeshHealth, peer_by_system: dict[str, str]) -> dict[str, str]:
    """Map system name -> honesty colour, from a system's declared peer FQDN.

    A system whose peer is not in the reading stays **unmetered** rather than
    defaulting to green: the reading covers what the vantage can see, and a
    system missing from it is precisely the case this must not paper over.
    """
    links = health.by_peer()
    out: dict[str, str] = {}
    for system, peer in peer_by_system.items():
        if not peer:
            continue
        link = links.get(peer)
        out[system] = link.color if link else "unmetered"
    return out
