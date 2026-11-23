# SPDX-License-Identifier: Apache-2.0
"""The stream hub's two ceilings.

goat-map cannot be made to query the fabric by a client — every read is served
from the in-memory store — so the only way client demand grows server cost here
is fan-out: one queue per connected browser, walked on every event. These
assert the ceilings that bound it.

See docs/design/surface-overload-protection.md.
"""

from __future__ import annotations

import sys
from pathlib import Path

import pytest

sys.path.insert(0, str(Path(__file__).resolve().parents[1]))

from goatmap.hub import Hub, HubFull  # noqa: E402


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
