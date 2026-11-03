# SPDX-License-Identifier: Apache-2.0
"""In-memory track store: latest position per track, a short trail, TTL expiry.

Deliberately not durable. The fabric is the source of truth; this process is a
window onto it. A restart re-fills from the next few seconds of traffic, which
is the correct behaviour for a live picture and avoids a storage decision the
brokerless design has not made yet (see docs/design/zenoh-brokerless-consequences.md).
"""

from __future__ import annotations

import time
from typing import Any, Callable

# How many past positions to keep per track for the trail.
TRAIL_LEN = 24


class TrackStore:
    def __init__(self, ttl_s: float = 120.0, trail_len: int = TRAIL_LEN) -> None:
        self._ttl = ttl_s
        self._trail_len = trail_len
        self._tracks: dict[str, dict[str, Any]] = {}
        self._seen_at: dict[str, float] = {}
        self.total_ingested = 0
        self.total_dropped = 0

    def upsert(self, track: dict[str, Any]) -> dict[str, Any]:
        tid = track["id"]
        prev = self._tracks.get(tid)
        trail = list(prev.get("trail", [])) if prev else []
        # Only extend the trail on actual movement -- a stationary emitter
        # re-reporting the same fix should not grow a trail of one point.
        if not trail or trail[-1] != [track["lon"], track["lat"]]:
            trail.append([track["lon"], track["lat"]])
        track["trail"] = trail[-self._trail_len :]
        track["first_seen_at"] = prev.get("first_seen_at", track["observed_at"]) if prev else track["observed_at"]
        track["updates"] = (prev.get("updates", 0) + 1) if prev else 1
        self._tracks[tid] = track
        self._seen_at[tid] = time.monotonic()
        self.total_ingested += 1
        return track

    def expire(self, on_expire: Callable[[str], None] | None = None) -> list[str]:
        """Drop tracks not heard from within the TTL. Returns dropped ids."""
        cutoff = time.monotonic() - self._ttl
        gone = [tid for tid, seen in self._seen_at.items() if seen < cutoff]
        for tid in gone:
            self._tracks.pop(tid, None)
            self._seen_at.pop(tid, None)
            self.total_dropped += 1
            if on_expire:
                on_expire(tid)
        return gone

    def snapshot(self) -> list[dict[str, Any]]:
        return list(self._tracks.values())

    def age_s(self, tid: str) -> float | None:
        seen = self._seen_at.get(tid)
        return None if seen is None else time.monotonic() - seen

    def __len__(self) -> int:
        return len(self._tracks)
