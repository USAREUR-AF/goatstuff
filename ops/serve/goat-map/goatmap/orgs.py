# SPDX-License-Identifier: Apache-2.0
"""Anchor -> org resolution, from the portal registry.

Why this exists rather than trusting the payload: `synthetic` decides whether a
dot is drawn as simulation or as a real detection, and `org` decides who a
viewer thinks published it. Neither may be self-asserted by the sender. A
producer that could mark itself synthetic could hide real activity behind a
"simulated" badge; one that could mark itself real could inject fiction into a
live picture.

The anchor is structural (the Zenoh key's first chunk, ACL-enforced). This maps
that anchor to what the REGISTRY says about it — the same split ADR 1051 draws:
the cert says what a peer was born as, the registry says what it currently is.

Fails soft and loudly: if the portal is unreachable the last good snapshot is
kept and its age is exposed, because a viz plane that blanks out when a
metadata service hiccups is worse than one that says "this is 4 minutes stale".
"""

from __future__ import annotations

import json
import logging
import time
import urllib.request
from typing import Any

LOG = logging.getLogger("goat-map.orgs")


class OrgRegistry:
    def __init__(self, portal_url: str, token: str, refresh_s: float = 300.0) -> None:
        self._url = portal_url.rstrip("/")
        self._token = token
        self._refresh_s = refresh_s
        self._by_anchor: dict[str, dict[str, Any]] = {}
        self._fetched_at: float | None = None
        self._error: str | None = None

    @property
    def enabled(self) -> bool:
        return bool(self._url and self._token)

    def get(self, anchor: str) -> dict[str, Any] | None:
        return self._by_anchor.get(anchor)

    def refresh(self) -> None:
        if not self.enabled:
            return
        req = urllib.request.Request(
            f"{self._url}/api/orgs",
            headers={"Authorization": f"Bearer {self._token}"},
        )
        try:
            with urllib.request.urlopen(req, timeout=10) as resp:
                body = resp.read().decode()
        except Exception as exc:
            # Keep the previous snapshot. Losing the registry must not turn
            # every known producer into an unknown one.
            self._error = str(exc)
            LOG.warning("org registry refresh failed (keeping snapshot): %s", exc)
            return

        rows = self._parse(body)
        if not rows:
            self._error = "registry returned no orgs"
            LOG.warning("org registry returned no rows; keeping previous snapshot")
            return

        self._by_anchor = {
            r["prefix"]: {
                "display_name": r.get("display_name") or r["prefix"],
                "synthetic": bool(r.get("synthetic", False)),
            }
            for r in rows
            if r.get("prefix")
        }
        self._fetched_at = time.monotonic()
        self._error = None
        synth = sum(1 for v in self._by_anchor.values() if v["synthetic"])
        LOG.info("org registry: %d orgs (%d synthetic)", len(self._by_anchor), synth)

    @staticmethod
    def _parse(body: str) -> list[dict[str, Any]]:
        """The portal serves JSON-lines here, not a JSON array; accept either
        rather than depending on which."""
        body = body.strip()
        if not body:
            return []
        try:
            loaded = json.loads(body)
            if isinstance(loaded, list):
                return loaded
            if isinstance(loaded, dict) and isinstance(loaded.get("orgs"), list):
                return loaded["orgs"]
            if isinstance(loaded, dict):
                return [loaded]
        except ValueError:
            pass  # not one JSON document: try the JSON-lines format below
        rows = []
        for line in body.splitlines():
            line = line.strip()
            if not line:
                continue
            try:
                rows.append(json.loads(line))
            except ValueError:
                continue
        return rows

    def status(self) -> dict[str, Any]:
        age = None if self._fetched_at is None else round(time.monotonic() - self._fetched_at, 1)
        return {
            "enabled": self.enabled,
            "orgs": len(self._by_anchor),
            "synthetic_orgs": sum(1 for v in self._by_anchor.values() if v["synthetic"]),
            "age_s": age,
            "error": self._error,
        }
