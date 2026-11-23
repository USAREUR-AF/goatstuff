# SPDX-License-Identifier: Apache-2.0
"""Track classes, declared sources, marks, and the per-second stream."""

from __future__ import annotations

import asyncio
import importlib
import json
import sys
from pathlib import Path

import pytest

sys.path.insert(0, str(Path(__file__).resolve().parents[1]))

from goatmap import classes  # noqa: E402

SLOT, OWNER = "a" * 32, "b" * 32
REPORTS = f"volatile/{SLOT}/stream/p/reports"
FEED = f"volatile/{SLOT}/stream/feeds-in/feed-1"
SOURCES = [
    {"key": REPORTS, "accept": {"radar-x": {"sensor": "Radar X", "site": "North", "anchor": OWNER}}},
    {"key": FEED, "sensor": "Nav", "label": "Blue", "class": "friendly"},
]


@pytest.mark.parametrize("cot_type,affiliation,want", [
    ("a-f-A-M-F", "friend", "friendly"),
    ("a-f-A-C-F", "friend", "commercial"),
    ("a-n-A-C-F", "neutral", "commercial"),
    ("a-n-A", "neutral", "neutral"),
    ("a-u-A", "unknown", "unknown"),
    ("a-p-A", "pending", "unknown"),
    ("a-h-A", "hostile", "hostile"),
    ("a-h-A-C", "hostile", "hostile"),
    ("a-k-A", "faker", "hostile"),
    (None, None, "unknown"),
])
def test_class_from_message(cot_type, affiliation, want):
    assert classes.from_message({"cot_type": cot_type, "affiliation": affiliation}) == want


def test_a_declared_class_must_be_a_known_class():
    with pytest.raises(ValueError):
        classes.load_sources([{"key": "a/b", "class": "red"}])


@pytest.mark.parametrize("pattern,key,want", [
    ("a/*/c", "a/b/c", True), ("a/*/c", "a/b/x/c", False),
    ("a/**", "a/b/c", True), ("a/b", "a/b/c", False),
])
def test_key_matches(pattern, key, want):
    assert classes.key_matches(pattern, key) is want


@pytest.fixture
def svc(monkeypatch):
    monkeypatch.setenv("GOATMAP_FABRIC_ENDPOINT", "tls/198.51.100.1:7447")
    monkeypatch.setenv("GOATMAP_COT_ENGINE", "builtin")
    monkeypatch.setenv("GOATMAP_RBAC_MODE", "enforce")
    monkeypatch.setenv("GOATMAP_WEB_ROOT", str(Path(__file__).resolve().parents[1] / "web"))
    main = importlib.import_module("goatmap.__main__")
    importlib.reload(main)
    from goatmap.store import TrackStore

    s = main.Service(TrackStore(), main.Hub(), None)
    s.sources = classes.load_sources(SOURCES)
    return main, s


def _report(**kw):
    return {"track": {"source_id": "radar-x"}, "uid": "7", "time_ms": 1795000000000,
            "lat": 45.0, "lon": 29.0, "course": 90, "speed_ms": 20, "type": "a-u-A", **kw}


def test_relayed_reports_take_only_accepted_sources_under_their_own_producer(svc):
    _, s = svc
    lines = [_report(), _report(uid="8"), _report(track={"source_id": "other"}), _report(uid="9", lat=0, lon=0)]
    s._handle_fabric(REPORTS, "\n".join(json.dumps(x) for x in lines).encode())
    tracks = s.store.snapshot()
    assert sorted(t["label"] for t in tracks) == ["7", "8"]
    assert {t["producer_anchor"] for t in tracks} == {OWNER}
    assert {t["source"]["sensor"] for t in tracks} == {"Radar X"}
    assert {t["class"] for t in tracks} == {"unknown"}


def test_a_position_feed_without_an_identifier_is_one_friendly_track(svc):
    _, s = svc
    for lat in (45.0, 45.1):
        s._handle_fabric(FEED, json.dumps({"lat": lat, "lon": 29.0}).encode() + b"\n")
    (t,) = s.store.snapshot()
    assert (t["label"], t["class"], t["producer_anchor"], t["lat"]) == ("Blue", "friendly", SLOT, 45.1)


def test_undeclared_records_and_empty_samples_are_ignored(svc):
    _, s = svc
    s._handle_fabric(f"volatile/{SLOT}/elsewhere", json.dumps(_report()).encode())
    s._handle_fabric(REPORTS, b"")
    s._handle_fabric(REPORTS, b"[1, 2]")
    assert len(s.store) == 0


def test_a_payload_cannot_state_its_own_class(svc):
    _, s = svc
    s._handle_fabric(f"{OWNER}/geo/track/v1/t", json.dumps(
        {"schema": "goat.geo.track.v1", "id": "t", "lat": 45.0, "lon": 29.0, "class": "friendly",
         "observed_at": "2026-01-01T00:00:00Z", "kind": "track", "affiliation": "unknown"}).encode())
    assert s.store.snapshot()[0]["class"] == "unknown"


def test_a_mark_outlives_updates_and_clears_back(svc):
    _, s = svc
    s._handle_fabric(REPORTS, json.dumps(_report()).encode())
    tid = s.store.snapshot()[0]["id"]
    before = s.store.rev(tid)
    s.mark(tid, "hostile", "a@example.org")
    assert s.store.rev(tid) > before
    s._handle_fabric(REPORTS, json.dumps(_report(lat=45.2)).encode())
    t = s.store.get(tid)
    assert (t["class"], t["class_from"]) == ("hostile", "marked by a@example.org")
    assert s.mark(tid, None, "a@example.org")["class"] == "unknown"
    assert s.mark("gone", "hostile", "a@example.org") is None


async def _client(main, s):
    from aiohttp.test_utils import TestClient, TestServer

    c = TestClient(TestServer(main.build_app(s, s.store, s.hub)))
    await c.start_server()
    return c


def _hdr(groups):
    return {"X-Forwarded-Groups": groups, "X-Forwarded-Email": "a@example.org"}


def test_who_may_mark(svc):
    main, s = svc
    s._handle_fabric(REPORTS, json.dumps(_report()).encode())
    tid = s.store.snapshot()[0]["id"]

    async def go():
        c = await _client(main, s)

        async def post(groups, body, **kw):
            return (await c.post("/api/track-class", headers=_hdr(groups), **(kw or {"json": body}))).status
        ok = {"id": tid, "class": "hostile"}
        grant = f"data.view.producer:{OWNER}"
        try:
            assert await post(grant, ok) == 403
            assert await post("TACTICAL-ADMIN", ok) == 404          # cannot see the track
            assert await post(f"TACTICAL-ADMIN,{grant}", ok) == 200
            assert s.store.get(tid)["class"] == "hostile"
            assert await post("PLATFORM-ADMIN", {"id": tid, "class": "red"}) == 400
            assert await post("PLATFORM-ADMIN", None, data="{}") == 415
        finally:
            await c.close()
    asyncio.run(go())


def test_stream_ticks_every_interval_and_is_scoped(svc):
    main, s = svc
    s._handle_fabric(REPORTS, json.dumps(_report()).encode())

    async def go():
        c = await _client(main, s)

        async def event(r):
            name = (await r.content.readline()).decode().split(": ", 1)[1].strip()
            data = json.loads((await r.content.readline()).decode().split(": ", 1)[1])
            await r.content.readline()
            return name, data
        try:
            r = await c.get("/api/stream?interval_s=0.5", headers=_hdr(f"data.view.producer:{OWNER}"))
            name, d = await event(r)
            assert name == "snapshot" and len(d["tracks"]) == 1 and d["interval_s"] == 0.5
            name, d = await event(r)                                # nothing changed: still a tick
            assert name == "tick" and d["upserts"] == [] and d["removed"] == []
            s._handle_fabric(REPORTS, json.dumps(_report(lat=45.3)).encode())
            s._handle_fabric(REPORTS, json.dumps(_report(uid="new")).encode())
            s._handle_fabric(FEED, json.dumps({"lat": 45.0, "lon": 29.0}).encode())   # not granted
            name, d = await event(r)
            assert [(t["label"], "trail" in t) for t in d["upserts"]] == [("7", False), ("new", True)]
            s.store._seen_at = {k: -1e9 for k in s.store._seen_at}
            s.store.expire()
            name, d = await event(r)
            assert len(d["removed"]) == 2
            r.close()
        finally:
            await c.close()
    asyncio.run(go())
