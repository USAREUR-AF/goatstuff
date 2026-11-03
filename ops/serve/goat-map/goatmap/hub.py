# SPDX-License-Identifier: Apache-2.0
"""SSE fan-out to connected browsers.

Lives outside `__main__` for one practical reason: `__main__` imports `zenoh`,
so anything defined there can only be tested where the fabric client is
installed. The hub has no fabric dependency at all — it is a queue set — and
the ceilings it enforces are exactly the kind of thing that must stay covered
by tests that run everywhere.

See docs/design/surface-overload-protection.md for where this sits in the
layered defence.
"""

from __future__ import annotations

import asyncio
from typing import Any

#: Per-client queue depth. A single slow reader cannot grow its own backlog
#: without limit; it is dropped instead.
CLIENT_QUEUE_DEPTH = 256


class HubFull(Exception):
    """Raised when the stream-connection budget is already spent."""


class Hub:
    """Fan-out to connected browsers.

    TWO ceilings, and they bound different things:

    - **per-client queue depth** (`CLIENT_QUEUE_DEPTH`) — one slow reader
      cannot grow its own backlog without limit; it is dropped instead.
    - **number of clients** (`max_clients`) — the queues themselves are the
      cost. `broadcast` walks every client on every event, so N connections
      make each fabric message N times more expensive in both CPU and
      resident memory. Without a ceiling, a wall of dashboards left open (or
      one browser reconnecting in a loop) grows that cost until the process
      is doing more fan-out than mapping.

    `max_clients=0` disables the connection ceiling; the per-client depth is
    always enforced.
    """

    def __init__(self, max_clients: int = 0) -> None:
        self._clients: set[asyncio.Queue] = set()
        self._max_clients = max_clients

    def register(self) -> asyncio.Queue:
        """Claim a stream slot, or raise `HubFull`.

        Refuse rather than queue. A refused browser retries and gets in when a
        slot frees; an admitted-but-starved one shows a map that silently stops
        updating, which is worse than an honest failure.
        """
        if self._max_clients and len(self._clients) >= self._max_clients:
            raise HubFull(
                f"stream connection budget spent ({self._max_clients} concurrent)"
            )
        q: asyncio.Queue = asyncio.Queue(maxsize=CLIENT_QUEUE_DEPTH)
        self._clients.add(q)
        return q

    def unregister(self, q: asyncio.Queue) -> None:
        self._clients.discard(q)

    def broadcast(self, event: str, data: Any) -> None:
        payload = (event, data)
        for q in list(self._clients):
            try:
                q.put_nowait(payload)
            except asyncio.QueueFull:
                self._clients.discard(q)

    def __len__(self) -> int:
        return len(self._clients)
