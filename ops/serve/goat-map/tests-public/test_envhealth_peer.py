# SPDX-License-Identifier: Apache-2.0
"""The peer domain (EnvHealthV1 arm 17) is decoded, never dropped as an error.

goat-peer-health publishes `EnvHealthV1{domain:"peer"}` on
ops/peer/health/<site>. A key expression that matches it must not turn every
peer reading into `health_error`, and a peer reading must never replace the
mesh reading the map draws its systems from. The map draws nothing new from a
peer reading: a fault names a Zenoh ID, which joins to no declared system.

The bytes here are built by a small encoder below, field numbers from the
canonical schema (ops/zenoh-trial-peers/time-health/proto/env_health_v1.proto).
"""

from __future__ import annotations

import importlib
import struct
import sys
from pathlib import Path

import pytest

ROOT = Path(__file__).resolve().parents[1]
sys.path.insert(0, str(ROOT))

from goatmap import envhealth  # noqa: E402


# ── a minimal protobuf encoder ───────────────────────────────────────────────

def _varint(v: int) -> bytes:
    v &= (1 << 64) - 1
    out = bytearray()
    while True:
        b = v & 0x7F
        v >>= 7
        if v:
            out.append(b | 0x80)
        else:
            out.append(b)
            return bytes(out)


def _key(fnum: int, wtype: int) -> bytes:
    return _varint(fnum << 3 | wtype)


def _str(fnum: int, s: str | bytes) -> bytes:
    b = s.encode() if isinstance(s, str) else s
    return _key(fnum, 2) + _varint(len(b)) + b


def _dbl(fnum: int, v: float) -> bytes:
    return _key(fnum, 1) + struct.pack("<d", v)


def _int(fnum: int, v: int) -> bytes:
    return _key(fnum, 0) + _varint(v)


def _peer_fault(zid, offset, rate, name=None, direct=None) -> bytes:
    b = _str(1, zid) + _int(2, 1) + _dbl(3, offset) + _dbl(6, rate)
    b += _int(8, 1_794_575_915_152) + _str(10, "5be08d2f")
    if name is not None:
        b += _str(11, name)
    b += _str(13, "peer_at_fault") + _str(14, "red") + _dbl(15, 0.5)
    if direct is not None:
        b += _int(16, 1 if direct else 0)
    return b


def _peer_reading(scope="example-site", peering="") -> bytes:
    detail = (
        _str(1, _peer_fault("3c9a51e0d2b7f4a6c18e05d9b3a7f2e1", 17.7, 12.0, "lab-router-b", False))
        + _str(1, _peer_fault("9f2", 3.0, 2.0))
        + _str(2, "peer_at_fault")
        + _str(3, "2 peer(s) had timestamps rejected in the last 60 s")
        + _str(4, "synced")
        + _int(7, (1 << 64) - 1)  # last_line_age_ms = -1, a negative int64
    )
    if peering:
        detail += _str(9, peering) + _dbl(10, 14.0)
    return (
        _str(1, "peer") + _str(2, scope) + _int(3, 1_794_575_950_000)
        + _str(4, "red") + _int(5, 2) + _int(6, 60) + _str(17, detail)
    )


def _mesh_reading() -> bytes:
    link = _str(1, "node-a.example.org") + _str(3, "green") + _int(4, 1)
    return _str(1, "mesh") + _str(2, "my-goatnet") + _str(4, "green") + _str(12, _str(1, link))


# ── the decoder ──────────────────────────────────────────────────────────────

def test_a_peer_reading_decodes_with_its_rows():
    r = envhealth.decode_any(_peer_reading(peering="partner-one"))
    assert isinstance(r, envhealth.PeerHealth)
    assert (r.domain, r.scope, r.overall_color) == ("peer", "example-site", "red")
    assert r.verdict == "peer_at_fault"
    assert r.peering == "partner-one"
    a, b = r.peers
    assert a.label == "lab-router-b"
    assert a.zid == "3c9a51e0d2b7f4a6c18e05d9b3a7f2e1"
    assert a.offset_seconds == 17.7
    assert a.rejections_per_minute == 12.0
    assert a.direct_session is False
    # No attributed name: the Zenoh ID is the label. No sessions file: unknown.
    assert b.label == "9f2"
    assert b.direct_session is None


def test_a_mesh_reading_still_decodes_as_before():
    r = envhealth.decode_any(_mesh_reading())
    assert isinstance(r, envhealth.MeshHealth)
    assert r.by_peer()["node-a.example.org"].color == "green"
    # And the mesh-only entry point is unchanged.
    assert envhealth.decode(_mesh_reading()).links[0].connected is True


def test_other_domains_and_nonsense_still_raise():
    with pytest.raises(envhealth.DecodeError):
        envhealth.decode_any(_str(1, "time") + _str(11, b""))
    with pytest.raises(envhealth.DecodeError):
        envhealth.decode_any(_str(1, "mesh") + _str(17, b""))
    with pytest.raises(envhealth.DecodeError):
        envhealth.decode_any(_peer_reading()[:-3])
    # The mesh-only decoder still refuses a peer reading.
    with pytest.raises(envhealth.DecodeError):
        envhealth.decode(_peer_reading())


# ── the service's join ───────────────────────────────────────────────────────

@pytest.fixture
def main(monkeypatch):
    monkeypatch.setenv("GOATMAP_INGEST_ENDPOINT", "tcp/198.51.100.1:7447")
    monkeypatch.setenv("GOATMAP_FABRIC_ENDPOINT", "")
    monkeypatch.setenv("GOATMAP_PUBLISH_PREFIX", "")
    m = importlib.import_module("goatmap.__main__")
    return importlib.reload(m)


def test_a_peer_reading_is_kept_and_never_replaces_the_mesh_reading(main):
    h = main.SiteHealth()
    h.on_payload(_mesh_reading())
    mesh = h.reading
    h.on_payload(_peer_reading())
    assert h.reading is mesh, "the map keeps drawing from the mesh reading"
    assert h.error is None, "a peer reading is not an error"
    assert h.peer["example-site"].overall_color == "red"


def test_a_peer_reading_alone_does_not_claim_the_health_join(main):
    """`joined` means a MESH reading arrived; the peer domain cannot stand in."""
    h = main.SiteHealth()
    h.on_payload(_peer_reading())
    assert h.joined is False
    assert h.error is None
