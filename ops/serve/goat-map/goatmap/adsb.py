# SPDX-License-Identifier: Apache-2.0
"""ADS-B aircraft onto the fabric as one goat.geo.trackset.v1 key.

Runs as its own process (`python -m goatmap.adsb`), never inside the map, so
the map does not depend on it. It asks a public aggregator (the readsb v2 JSON
served by adsb.lol, airplanes.live and others) for a wide circle and keeps the
aircraft inside ADSB_BOX, so the request does not state the area of interest.

A civil aircraft carries CoT type a-n-A-C-F, which the map classes commercial.
One the aggregator lists as military is a-u-A-M-F: unknown.

Environment: ADSB_URL, ADSB_BOX=lat0,lon0,lat1,lon1, ADSB_KEY, ADSB_PERIOD_S,
ADSB_ZENOH_CONFIG (a zenoh client config file), ADSB_DRY_RUN=1 to print.
"""

from __future__ import annotations

import asyncio
import json
import logging
import os
from datetime import datetime, timezone
from typing import Any

from . import cot as cot_mod

LOG = logging.getLogger("goatmap.adsb")
MAX_AGE_S = 30.0
MAX_TRACKS = 400
KT, FT = 0.514444, 0.3048


def _iso(ms: float) -> str:
    return datetime.fromtimestamp(ms / 1000, timezone.utc).isoformat(timespec="milliseconds")


def _num(v: Any) -> float | None:
    return float(v) if isinstance(v, (int, float)) and not isinstance(v, bool) else None


def tracks(body: dict[str, Any], box: tuple[float, float, float, float]) -> list[dict[str, Any]]:
    now = _num(body.get("now")) or datetime.now(timezone.utc).timestamp() * 1000
    out = []
    for a in body.get("ac") or []:
        lat, lon, seen = _num(a.get("lat")), _num(a.get("lon")), _num(a.get("seen_pos"))
        hexid = str(a.get("hex") or "")
        if lat is None or lon is None or not hexid or seen is None or seen > MAX_AGE_S:
            continue
        if not (box[0] <= lat <= box[2] and box[1] <= lon <= box[3]):
            continue
        if a.get("alt_baro") == "ground" or a.get("nic") == 0:
            continue  # on the ground, or the aircraft states its position is not assured
        alt = _num(a.get("alt_geom"))
        alt = _num(a.get("alt_baro")) if alt is None else alt
        gs = _num(a.get("gs"))
        mil = bool(int(_num(a.get("dbFlags")) or 0) & 1)
        out.append({
            "schema": cot_mod.SCHEMA, "id": f"adsb:{hexid}", "kind": "track",
            "label": str(a.get("flight") or "").strip() or str(a.get("r") or hexid),
            "source": {"sensor": "ADS-B", "site": "", "node": "", "goatnet": ""},
            "lat": lat, "lon": lon, "alt_m": None if alt is None else round(alt * FT, 1),
            "course_deg": _num(a.get("track")), "speed_mps": None if gs is None else round(gs * KT, 1),
            "ce_m": _num(a.get("rc")), "le_m": None,
            "cot_type": "a-u-A-M-F" if mil else "a-n-A-C-F",
            "affiliation": "unknown" if mil else "neutral", "dimension": "air",
            "how": "m-g", "descriptor": a.get("t"), "stale_at": None,
            "reported_at": _iso(now - seen * 1000), "observed_at": cot_mod.now_iso(),
        })
    return out[:MAX_TRACKS]


async def run() -> None:
    import aiohttp

    url, key = os.environ["ADSB_URL"], os.environ["ADSB_KEY"]
    box = tuple(float(x) for x in os.environ["ADSB_BOX"].split(","))
    period = max(2.0, float(os.environ.get("ADSB_PERIOD_S", "5")))
    put = None
    if os.environ.get("ADSB_DRY_RUN") != "1":
        import zenoh

        session = zenoh.open(zenoh.Config.from_file(os.environ["ADSB_ZENOH_CONFIG"]))
        pub = session.declare_publisher(key, priority=zenoh.Priority.BACKGROUND,
                                        congestion_control=zenoh.CongestionControl.DROP)
        put = pub.put
    wait, n = period, 0
    async with aiohttp.ClientSession(timeout=aiohttp.ClientTimeout(total=10)) as http:
        while True:
            try:
                async with http.get(url, headers={"User-Agent": "goat-map-adsb"}) as r:
                    r.raise_for_status()
                    ts = tracks(await r.json(content_type=None), box)
                body = json.dumps({"schema": "goat.geo.trackset.v1", "tracks": ts}).encode()
                put(body) if put else print(body.decode()[:2000], flush=True)
                if n % 60 == 0:
                    LOG.info("published %d aircraft, %d bytes", len(ts), len(body))
                n, wait = n + 1, period
            except Exception as exc:  # the aggregator is best effort: slow down and go on
                wait = min(120.0, wait * 2)
                LOG.warning("no update (%s), next try in %.0f s", exc, wait)
            await asyncio.sleep(wait)


if __name__ == "__main__":
    logging.basicConfig(level=logging.INFO, format="%(asctime)s %(levelname)s %(message)s")
    asyncio.run(run())
