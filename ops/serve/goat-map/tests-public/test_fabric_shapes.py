# SPDX-License-Identifier: Apache-2.0
"""The fabric leg takes the three shapes producers publish.

A sensor node publishes either one goat.geo.track.v1 per message, one
goat.geo.trackset.v1 carrying every current track on a single stable key, or
CoT events. The store must receive the tracks in all three, with the producer
taken from the key, and must drop a placeholder fix at exactly 0,0.
"""

from __future__ import annotations

import importlib
import json
import sys
from pathlib import Path

import pytest

ROOT = Path(__file__).resolve().parents[1]
sys.path.insert(0, str(ROOT))

ANCHOR = "a" * 32
COT = (
    b'<?xml version="1.0"?><event version="2.0" uid="{uid}" type="a-u-A" how="m-r" '
    b'time="2026-01-01T00:00:00Z" start="2026-01-01T00:00:00Z" stale="2026-01-01T00:05:00Z">'
    b'<point lat="{lat}" lon="{lon}" hae="100" ce="10" le="10"/><detail/></event>'
)


def _cot(uid, lat, lon):
    return COT.replace(b"{uid}", uid.encode()).replace(b"{lat}", str(lat).encode()).replace(
        b"{lon}", str(lon).encode())


def _track(tid, lat, lon):
    return {"id": tid, "lat": lat, "lon": lon, "observed_at": "2026-01-01T00:00:00Z",
            "schema": "goat.geo.track.v1", "kind": "track"}


@pytest.fixture
def svc(monkeypatch):
    monkeypatch.setenv("GOATMAP_INGEST_ENDPOINT", "")
    monkeypatch.setenv("GOATMAP_FABRIC_ENDPOINT", "tls/198.51.100.1:7447")
    monkeypatch.setenv("GOATMAP_FABRIC_SUBSCRIBE", "*/geo/track/v1/*, */*/site/geo/v1 ,*/node/cot")
    monkeypatch.setenv("GOATMAP_PUBLISH_PREFIX", "")
    monkeypatch.setenv("GOATMAP_COT_ENGINE", "builtin")
    main = importlib.import_module("goatmap.__main__")
    importlib.reload(main)
    from goatmap.store import TrackStore

    store = TrackStore()
    return main, main.Service(store, main.Hub(), None), store


def test_subscriptions_are_split_on_commas(svc):
    main, _, _ = svc
    assert main.FABRIC_SUBSCRIBES == ["*/geo/track/v1/*", "*/*/site/geo/v1", "*/node/cot"]


def test_single_track(svc):
    _, s, store = svc
    s._handle_fabric(f"{ANCHOR}/geo/track/v1/t1", json.dumps(_track("t1", 45.1, 29.6)).encode())
    assert len(store) == 1


def test_trackset_stores_every_track_and_drops_null_island(svc):
    _, s, store = svc
    body = {"schema": "goat.geo.trackset.v1",
            "tracks": [_track("t1", 45.1, 29.6), _track("t2", 45.2, 29.7), _track("t0", 0.0, 0.0)]}
    s._handle_fabric(f"{ANCHOR}/node/site/geo/v1", json.dumps(body).encode())
    assert len(store) == 2
    assert all(t["producer_anchor"] == ANCHOR for t in store._tracks.values())


def test_cot_event_is_parsed_with_provenance_from_the_key(svc):
    _, s, store = svc
    s._handle_fabric(f"{ANCHOR}/node/cot", _cot("u1", 44.3, 28.4))
    s._handle_fabric(f"{ANCHOR}/node/cot", _cot("u0", 0, 0))
    assert len(store) == 1
    (t,) = store._tracks.values()
    assert t["producer_anchor"] == ANCHOR and t["id"].startswith(ANCHOR[:8] + ":")


def test_every_shape_records_its_key_and_standard(svc):
    _, s, store = svc
    s._handle_fabric(f"{ANCHOR}/geo/track/v1/t1", json.dumps(_track("t1", 45.1, 29.6)).encode())
    body = {"schema": "goat.geo.trackset.v1", "tracks": [_track("t2", 45.2, 29.7)]}
    s._handle_fabric(f"{ANCHOR}/node/site/geo/v1", json.dumps(body).encode())
    s._handle_fabric(f"{ANCHOR}/node/cot", _cot("u1", 44.3, 28.4))
    origins = {t["id"].split(":", 1)[1]: t["origin"] for t in store._tracks.values()}
    assert origins["t1"] == {"key": f"{ANCHOR}/geo/track/v1/t1", "standard": "goat.geo.track.v1 (JSON)"}
    assert origins["t2"] == {"key": f"{ANCHOR}/node/site/geo/v1", "standard": "goat.geo.trackset.v1 (JSON)"}
    (cot_id,) = [k for k in origins if k not in ("t1", "t2")]
    assert origins[cot_id] == {"key": f"{ANCHOR}/node/cot", "standard": "Cursor-on-Target (CoT XML)"}
