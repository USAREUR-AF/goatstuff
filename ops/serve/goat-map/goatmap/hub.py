# SPDX-License-Identifier: Apache-2.0
"""Stream slots for connected browsers.

Each browser connection reads the store on its own clock (`stream` in
`__main__`), so there is no per-client queue to overflow: a slow reader gets the
newest state late, never a backlog, and is never silently disconnected. The
remaining cost per connection is one scan of the store per tick, and
`max_clients` bounds how many there are.

See docs/design/surface-overload-protection.md for where this sits in the
layered defence.
"""

from __future__ import annotations


class HubFull(Exception):
    """Raised when the stream-connection budget is already spent."""


class Hub:
    """Counts stream connections. `max_clients=0` disables the ceiling."""

    def __init__(self, max_clients: int = 0) -> None:
        self._slots: set[object] = set()
        self._max_clients = max_clients

    def register(self) -> object:
        """Claim a stream slot, or raise `HubFull`.

        Refuse rather than queue. A refused browser retries and gets in when a
        slot frees; an admitted-but-starved one shows a map that silently stops
        updating, which is worse than an honest failure.
        """
        if self._max_clients and len(self._slots) >= self._max_clients:
            raise HubFull(
                f"stream connection budget spent ({self._max_clients} concurrent)"
            )
        slot = object()
        self._slots.add(slot)
        return slot

    def unregister(self, slot: object) -> None:
        self._slots.discard(slot)

    def __len__(self) -> int:
        return len(self._slots)
