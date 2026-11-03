# SPDX-License-Identifier: Apache-2.0
"""The stream hub's two ceilings.

goat-map cannot be made to query the fabric by a client — every read is served
from the in-memory store — so the only way client demand grows server cost here
is fan-out: one queue per connected browser, walked on every event. These
assert the ceilings that bound it.

See docs/design/surface-overload-protection.md.
"""

from __future__ import annotations

import asyncio
import sys
from pathlib import Path

import pytest

sys.path.insert(0, str(Path(__file__).resolve().parents[1]))

from goatmap.hub import CLIENT_QUEUE_DEPTH, Hub, HubFull  # noqa: E402


def test_connection_ceiling_refuses_past_the_cap():
    hub = Hub(max_clients=3)
    held = [hub.register() for _ in range(3)]
    assert len(hub) == 3
    with pytest.raises(HubFull):
        hub.register()
    # THE POINT: refusal, not silent admission. An admitted-but-starved client
    # renders a map that quietly stops updating, which an operator reads as
    # "nothing is moving" rather than "I was not connected".
    assert len(hub) == 3, "a refused client must not be counted or queued"
    assert len(held) == 3


def test_slots_return_when_clients_disconnect():
    # Saturation must be transient. A leaked slot permanently reduces capacity
    # until the process restarts, which presents as "the map randomly refuses
    # to stream" long after the burst that caused it.
    hub = Hub(max_clients=2)
    a = hub.register()
    b = hub.register()
    with pytest.raises(HubFull):
        hub.register()
    hub.unregister(a)
    c = hub.register()  # must succeed now
    assert len(hub) == 2
    hub.unregister(b)
    hub.unregister(c)
    assert len(hub) == 0


def test_zero_disables_the_connection_ceiling():
    # Explicit escape hatch, so an operator can turn the ceiling off without
    # editing code — and so the default stays a deliberate number rather than
    # an accident of the constructor.
    hub = Hub(max_clients=0)
    for _ in range(200):
        hub.register()
    assert len(hub) == 200


def test_slow_client_is_dropped_not_grown():
    # The per-client ceiling, independent of the connection ceiling: a reader
    # that never drains must not be allowed to accumulate unboundedly.
    hub = Hub(max_clients=0)
    q = hub.register()
    for i in range(CLIENT_QUEUE_DEPTH):
        hub.broadcast("track", {"i": i})
    assert q.qsize() == CLIENT_QUEUE_DEPTH
    assert len(hub) == 1
    hub.broadcast("track", {"i": "overflow"})
    assert len(hub) == 0, "a client that cannot keep up must be dropped"
    assert q.qsize() == CLIENT_QUEUE_DEPTH, "its queue must not grow past the depth"


def test_broadcast_reaches_every_live_client():
    # Guard against a ceiling that "works" by quietly not delivering.
    hub = Hub(max_clients=4)
    qs = [hub.register() for _ in range(4)]
    hub.broadcast("track", {"id": "t1"})
    for q in qs:
        assert q.get_nowait() == ("track", {"id": "t1"})


def test_queue_is_the_documented_depth():
    hub = Hub()
    q = hub.register()
    assert q.maxsize == CLIENT_QUEUE_DEPTH
    assert isinstance(q, asyncio.Queue)
