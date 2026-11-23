# SPDX-License-Identifier: Apache-2.0
"""Track classes: friendly, commercial, unknown, hostile (and neutral).

A track's class comes from the first of these that answers:

  1. a mark: a person allowed to classify set it on the map; it lasts until the
     track expires, because a radar gives an expired track's number to the next
     contact
  2. the message: the affiliation its CoT type states, including `unknown`
     stated by a source that tags its tracks
  3. the declared source: the operator declared what a message that states no
     affiliation on that key is (for example the position feed of our own aircraft)

A source that tags its tracks is declared `tagged`: each message's own tag is
used, and an untagged one is unknown, never the declared class.

A class is never read from a payload's own `class` member.
"""

from __future__ import annotations

from typing import Any

FRIENDLY, COMMERCIAL, NEUTRAL, UNKNOWN, HOSTILE = (
    "friendly", "commercial", "neutral", "unknown", "hostile")
CLASSES = (FRIENDLY, COMMERCIAL, NEUTRAL, UNKNOWN, HOSTILE)

# Affiliation word (goatmap/cot.py) to class. `joker` is a friendly playing a
# suspect and `faker` a friendly playing a hostile, so an exercise's red
# aircraft draws as hostile.
_BY_AFFILIATION = {
    "friend": FRIENDLY, "assumed-friend": FRIENDLY, "neutral": NEUTRAL,
    "hostile": HOSTILE, "suspect": HOSTILE, "faker": HOSTILE, "joker": HOSTILE,
}


def from_message(track: dict[str, Any]) -> str:
    """The class the message itself states. CoT function code `C` (civilian)
    on a friend or neutral contact is commercial."""
    cls = _BY_AFFILIATION.get(track.get("affiliation") or "", UNKNOWN)
    parts = (track.get("cot_type") or "").split("-")
    if cls in (FRIENDLY, NEUTRAL) and len(parts) > 3 and parts[2] in ("A", "G", "S") and parts[3] == "C":
        return COMMERCIAL
    return cls


def stated(track: dict[str, Any]) -> bool:
    """Whether the message itself says what the track is. A bare `a-u-…` from a
    radar is the absence of an identification, so it does not count."""
    return from_message(track) != UNKNOWN


def key_matches(pattern: str, key: str) -> bool:
    """Zenoh key expression against a key: `*` is one chunk, `**` any number."""

    def go(p: list[str], k: list[str]) -> bool:
        if not p:
            return not k
        if p[0] == "**":
            return any(go(p[1:], k[i:]) for i in range(len(k) + 1))
        return bool(k) and p[0] in ("*", k[0]) and go(p[1:], k[1:])

    return go(pattern.split("/"), key.split("/"))


def load_sources(rows: Any) -> list[dict[str, Any]]:
    """Declared track sources. A bad row raises: a class nobody can rely on is
    worse than a service that says its declaration is wrong."""
    out = []
    for row in rows or []:
        if not isinstance(row, dict) or not row.get("key"):
            raise ValueError(f"track source without a key: {row!r}")
        for r in (row, *(row.get("accept") or {}).values()):
            if r.get("class") is not None and r["class"] not in CLASSES:
                raise ValueError(f"track source {row['key']}: unknown class {r['class']!r}")
            if r.get("tagged") and r.get("class"):
                raise ValueError(f"track source {row['key']}: tagged and class exclude each other")
        out.append(row)
    return out


def source_for(sources: list[dict[str, Any]], key: str) -> dict[str, Any] | None:
    return next((s for s in sources if key_matches(s["key"], key)), None)


class Yields:
    """A source with `yield_to` is a second copy of another key's tracks: it is
    read only while that key has been silent for YIELD_S seconds."""

    YIELD_S = 30.0

    def __init__(self, sources: list[dict[str, Any]]) -> None:
        rows = [r for s in sources for r in (s, *(s.get("accept") or {}).values())]
        self._last: dict[str, float | None] = {r["yield_to"]: None for r in rows if r.get("yield_to")}

    def saw(self, key: str, now: float) -> None:
        for pattern in self._last:
            if key_matches(pattern, key):
                self._last[pattern] = now

    def blocked(self, info: dict[str, Any], now: float) -> bool:
        last = self._last.get(info.get("yield_to") or "")
        return last is not None and now - last < self.YIELD_S


class Marks:
    """Classes people set on tracks, held for the life of each track."""

    def __init__(self) -> None:
        self._marks: dict[str, dict[str, str]] = {}

    def set(self, tid: str, cls: str | None, by: str, at: str) -> None:
        if cls is None:
            self._marks.pop(tid, None)
        elif cls in CLASSES:
            self._marks[tid] = {"class": cls, "by": by, "at": at}
        else:
            raise ValueError(f"unknown class {cls!r}")

    def drop(self, tid: str) -> None:
        self._marks.pop(tid, None)

    def apply(self, track: dict[str, Any], declared: str | None = None) -> dict[str, Any]:
        mark = self._marks.get(track["id"])
        track["declared_class"] = declared
        # What the source says, kept beside a person's mark so a disagreement shows.
        track["source_class"] = from_message(track) if stated(track) else declared or UNKNOWN
        if mark:
            track["class"] = mark["class"]
            track["class_from"] = f"marked by {mark['by'] or 'a viewer'}"
        elif stated(track):
            track["class"], track["class_from"] = from_message(track), "tagged by the source"
        elif declared:
            track["class"], track["class_from"] = declared, "declared source"
        else:
            track["class"], track["class_from"] = UNKNOWN, "message"
        return track
