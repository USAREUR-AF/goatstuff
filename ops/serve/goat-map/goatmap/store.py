# SPDX-License-Identifier: Apache-2.0
"""In-memory track store: latest position per track, a short trail, TTL expiry.

Deliberately not durable. The fabric is the source of truth; this process is a
window onto it. A restart re-fills from the next few seconds of traffic, which
is the correct behaviour for a live picture and avoids a storage decision the
brokerless design has not made yet (see docs/design/zenoh-brokerless-consequences.md).
"""

from __future__ import annotations

import time
from collections import deque
from typing import Any, Callable

# How many past positions to keep per track for the trail.
TRAIL_LEN = 24

# Track history for the overlay: how far back it reaches, how often a track
# adds a point (a radar reporting every second over 24 h would otherwise keep
# 86,400 points per track), and a ceiling on how many tracks it remembers.
HISTORY_S = 24 * 3600
HISTORY_STEP_S = 5.0
HISTORY_MAX_TRACKS = 5000

# The fields a history entry keeps besides its points: enough to scope it to a
# viewer (org, producer_anchor) and to draw it (affiliation, synthetic, label).
_HISTORY_META = ("label", "org", "producer_anchor", "affiliation", "class", "synthetic", "source")


class TrackStore:
    def __init__(self, ttl_s: float = 120.0, trail_len: int = TRAIL_LEN,
                 history_s: float = HISTORY_S, history_step_s: float = HISTORY_STEP_S,
                 history_max_tracks: int = HISTORY_MAX_TRACKS) -> None:
        self._ttl = ttl_s
        self._trail_len = trail_len
        self._tracks: dict[str, dict[str, Any]] = {}
        self._seen_at: dict[str, float] = {}
        # Bumped on every change; a stream sends what changed since its last tick.
        self._rev = 0
        self._revs: dict[str, int] = {}
        self.total_ingested = 0
        self.total_dropped = 0
        # History outlives the TTL on purpose: a track that went quiet an hour
        # ago is gone from the live picture but still belongs in "the last 6 h".
        # Points are wall-clock seconds (receipt time, like every age here).
        self.history_s = float(history_s)
        self._history_step = float(history_step_s)
        self._history_max = int(history_max_tracks)
        self._history: dict[str, dict[str, Any]] = {}

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
        self.touch(tid)
        self._record_history(track)
        return track

    def get(self, tid: str) -> dict[str, Any] | None:
        return self._tracks.get(tid)

    def touch(self, tid: str) -> None:
        """Record that a track changed, so streams send it on their next tick."""
        self._rev += 1
        self._revs[tid] = self._rev

    def rev(self, tid: str) -> int:
        return self._revs.get(tid, 0)

    def _record_history(self, track: dict[str, Any], now: float | None = None) -> None:
        if self.history_s <= 0:
            return
        now = time.time() if now is None else now
        tid = track["id"]
        h = self._history.get(tid)
        if h is None:
            if len(self._history) >= self._history_max:
                # Forget the track heard from longest ago, not the newest one.
                oldest = min(self._history, key=lambda k: self._history[k]["last"])
                del self._history[oldest]
            h = {"points": deque(), "last": now, "opened": now}
            self._history[tid] = h
        for k in _HISTORY_META:
            if k in track:
                h[k] = track[k]
        pt = (round(now, 1), float(track["lon"]), float(track["lat"]))
        pts = h["points"]
        if pts and now - h["opened"] < self._history_step:
            # Inside the sampling step: keep the newest position, not a new point.
            pts[-1] = pt
        else:
            pts.append(pt)
            h["opened"] = now
        h["last"] = now

    def prune_history(self, now: float | None = None) -> None:
        """Drop history points older than the retention window."""
        now = time.time() if now is None else now
        cutoff = now - self.history_s
        for tid in list(self._history):
            pts = self._history[tid]["points"]
            while pts and pts[0][0] < cutoff:
                pts.popleft()
            if not pts:
                del self._history[tid]

    def history(self, window_s: float, now: float | None = None) -> list[dict[str, Any]]:
        """Every track heard within the last `window_s` seconds, with its points.

        Each entry carries `id`, the scoping and drawing fields, and `points`
        as [lon, lat, t] with t in Unix seconds. Not scoped: the caller scopes.
        """
        now = time.time() if now is None else now
        cutoff = now - max(0.0, min(float(window_s), self.history_s))
        out = []
        for tid, h in self._history.items():
            pts = [[lon, lat, t] for (t, lon, lat) in h["points"] if t >= cutoff]
            if not pts:
                continue
            entry = {k: h[k] for k in _HISTORY_META if k in h}
            entry["id"] = tid
            entry["points"] = pts
            out.append(entry)
        return out

    def expire(self, on_expire: Callable[[str], None] | None = None) -> list[str]:
        """Drop tracks not heard from within the TTL. Returns dropped ids."""
        cutoff = time.monotonic() - self._ttl
        gone = [tid for tid, seen in self._seen_at.items() if seen < cutoff]
        for tid in gone:
            self._tracks.pop(tid, None)
            self._seen_at.pop(tid, None)
            self._revs.pop(tid, None)
            self.total_dropped += 1
            if on_expire:
                on_expire(tid)
        self.prune_history()
        return gone

    def snapshot(self) -> list[dict[str, Any]]:
        return list(self._tracks.values())

    def age_s(self, tid: str) -> float | None:
        seen = self._seen_at.get(tid)
        return None if seen is None else time.monotonic() - seen

    def __len__(self) -> int:
        return len(self._tracks)
