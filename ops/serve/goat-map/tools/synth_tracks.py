#!/usr/bin/env python3
# SPDX-License-Identifier: Apache-2.0
"""Synthetic GeoTrackV1 producer.

Publishes simulated tracks onto a goatnet fabric under its OWN slot anchor, so
goat-map (and the access-control work above it) has real traffic to render and
scope without waiting on a physical sensor.

Two properties make this safe to run against a production fabric:

  1. **It cannot impersonate a real producer.** It writes under its own 32-hex
     anchor and the router ACL admits writes nowhere else. The anchor is the
     FIRST chunk of every key it publishes, so a consumer derives provenance
     structurally rather than trusting a field in the payload.

  2. **It cannot be mistaken for real data.** Its org is registered
     `synthetic: true` in the portal, every record carries `synthetic: true`,
     and renderers are required by the schema to mark such tracks distinctly.

The motion model is deliberately crude — constant-speed great-circle legs with
occasional course changes. This is traffic for exercising a pipeline, not a
threat model or a flight simulator, and pretending otherwise by making it
elaborate would invite someone to read meaning into it.

Usage:
  python3 synth_tracks.py --bundle /path/to/unpacked-bundle \
      --endpoint tls/<router-mesh-ip>:7447 --center <lon>,<lat> --tracks 8
"""

from __future__ import annotations

import argparse
import glob
import json
import math
import os
import random
import signal
import sys
import time
from datetime import datetime, timezone

import zenoh

SCHEMA = "goat.geo.track.v1"
EARTH_R = 6_371_000.0

# A small, obviously-nonspecific set. Synthetic tracks are all `unknown`
# affiliation on purpose: inventing hostiles would put fictional threat calls
# on a production picture.
AFFILIATION = "unknown"


def now_iso() -> str:
    return datetime.now(timezone.utc).isoformat(timespec="milliseconds").replace("+00:00", "Z")


class Track:
    """One simulated contact on a constant-speed leg."""

    def __init__(self, uid: int, lat: float, lon: float, rng: random.Random,
                 home: tuple[float, float] | None = None,
                 radius_km: float = 60.0) -> None:
        self.uid = uid
        self.lat = lat
        self.lon = lon
        self.rng = rng
        self.course = rng.uniform(0, 360)
        self.speed = rng.uniform(8, 60)  # m/s — boat through fast aircraft
        self.alt = rng.choice([0.0, 0.0, rng.uniform(50, 3000)])
        self.updates = 0
        # Where this contact belongs, and how far it may stray. Without this a
        # constant-speed leg never turns: at 60 m/s a track leaves a 60 km area
        # in twenty minutes and is 3,900 km away after eighteen hours, so the
        # map looks empty at the site it is configured to watch and the surface
        # reads as broken.
        self.home = home if home is not None else (lat, lon)
        self.radius_km = radius_km

    def _km_from_home(self) -> float:
        dlat = self.lat - self.home[0]
        dlon = (self.lon - self.home[1]) * math.cos(math.radians(self.home[0]))
        return math.hypot(dlat, dlon) * 111.32

    def _bearing_home(self) -> float:
        dlat = self.home[0] - self.lat
        dlon = (self.home[1] - self.lon) * math.cos(math.radians(self.lat))
        return math.degrees(math.atan2(dlon, dlat)) % 360

    def step(self, dt: float) -> None:
        # Occasional course change so trails are not all straight lines.
        if self.rng.random() < 0.05:
            self.course = (self.course + self.rng.uniform(-30, 30)) % 360
        # Turn back at the edge of the operating area. A soft steer rather than
        # a hard clamp or a wrap: clamping parks contacts on a boundary line
        # that looks like a wall on the map, and wrapping teleports them, which
        # would break the trail and any speed a viewer infers from it.
        if self._km_from_home() > self.radius_km:
            self.course = (self._bearing_home() + self.rng.uniform(-25, 25)) % 360
        d = self.speed * dt
        brg = math.radians(self.course)
        lat1, lon1 = math.radians(self.lat), math.radians(self.lon)
        lat2 = math.asin(
            math.sin(lat1) * math.cos(d / EARTH_R)
            + math.cos(lat1) * math.sin(d / EARTH_R) * math.cos(brg)
        )
        lon2 = lon1 + math.atan2(
            math.sin(brg) * math.sin(d / EARTH_R) * math.cos(lat1),
            math.cos(d / EARTH_R) - math.sin(lat1) * math.sin(lat2),
        )
        self.lat, self.lon = math.degrees(lat2), math.degrees(lon2)
        self.updates += 1

    def to_geotrack(self, anchor: str, org: str, source: dict) -> dict:
        return {
            "schema": SCHEMA,
            "id": f"{source['sensor']}:{self.uid}",
            "label": str(self.uid),
            "kind": "track",
            "source": source,
            "lat": round(self.lat, 6),
            "lon": round(self.lon, 6),
            "alt_m": round(self.alt, 1),
            "course_deg": round(self.course, 1),
            "speed_mps": round(self.speed, 2),
            "ce_m": None,
            "le_m": None,
            "cot_type": None,
            "affiliation": AFFILIATION,
            "dimension": "air" if self.alt > 100 else "surface",
            "how": "simulated",
            "descriptor": "SYNTHETIC — simulated track, not a real detection",
            # No sensor clock to speak of. Left null rather than filled with
            # `now`, which would fake a trustworthy reported time.
            "reported_at": None,
            "stale_at": None,
            "observed_at": now_iso(),
            "updates": self.updates,
            "synthetic": True,
            "producer_anchor": anchor,
            "org": org,
        }


def open_session(bundle: str, anchor: str, endpoint: str) -> zenoh.Session:
    conf = zenoh.Config()
    conf.insert_json5("mode", json.dumps("client"))
    conf.insert_json5("connect/endpoints", json.dumps([endpoint]))
    conf.insert_json5("scouting/multicast/enabled", "false")
    # enable_mtls is REQUIRED — without it the client offers no certificate and
    # the router refuses the session with a bare "unable to connect", which
    # looks exactly like a network problem and costs an hour to diagnose.
    conf.insert_json5("transport/link/tls/enable_mtls", "true")
    # The router's cert is issued for its hostname; dialling it by mesh IP
    # (which is what works when netbird's DNS forwarder is in the way) would
    # otherwise fail name verification.
    conf.insert_json5("transport/link/tls/verify_name_on_connect", "false")
    # DISCOVER the CA root rather than hardcoding one goatnet's filename.
    #
    # A hardcoded per-goatnet CA filename makes the tool unrunnable anywhere else:
    # the path does not exist and zenoh fails with a bare "Unable to connect to
    # any of [tls/...]" — indistinguishable from the router being down, which is
    # the confusion the mTLS note in goat-map.md §10 warns about.
    #
    # Every org bundle carries exactly one `*-ca-root.pem`, so glob for it and
    # say so plainly when it is missing.
    ca_candidates = sorted(glob.glob(os.path.join(bundle, "*-ca-root.pem")))
    if not ca_candidates:
        raise SystemExit(
            f"no *-ca-root.pem in {bundle} — an org bundle always carries one; "
            "without it the TLS session fails as a bare 'unable to connect'"
        )
    conf.insert_json5(
        "transport/link/tls/root_ca_certificate",
        json.dumps(ca_candidates[0]),
    )
    conf.insert_json5(
        "transport/link/tls/connect_certificate",
        json.dumps(os.path.join(bundle, f"{anchor}-cert.pem")),
    )
    conf.insert_json5(
        "transport/link/tls/connect_private_key",
        json.dumps(os.path.join(bundle, f"{anchor}-key.pem")),
    )
    return zenoh.open(conf)


def main() -> int:
    ap = argparse.ArgumentParser(description=__doc__)
    ap.add_argument("--bundle", required=True, help="unpacked org bundle dir (cert+key+CA)")
    ap.add_argument("--anchor", help="32-hex slot anchor (default: inferred from bundle)")
    ap.add_argument("--endpoint", required=True, help="e.g. tls/100.127.0.224:7447")
    ap.add_argument("--org", default="SYNTHETIC — simulated tracks (NOT REAL DATA)")
    ap.add_argument("--center", required=True,
                    help="lon,lat — REQUIRED. No default: a synthetic feed that\n"
                         "silently picks a real location will pick the wrong\n"
                         "environment's.")
    ap.add_argument("--radius-km", type=float, default=60.0,
                    help="contacts turn back beyond this range from --center; "
                         "keeps a demo feed inside the area it is meant to show")
    ap.add_argument("--spread-km", type=float, default=6.0)
    ap.add_argument("--tracks", type=int, default=8)
    ap.add_argument("--interval", type=float, default=1.0)
    ap.add_argument("--sensor", default="synth")
    ap.add_argument("--site", default="synthetic-site")
    ap.add_argument("--node", default="synthetic-tracks")
    ap.add_argument("--seed", type=int, default=None)
    args = ap.parse_args()

    anchor = args.anchor
    if not anchor:
        certs = [f for f in os.listdir(args.bundle) if f.endswith("-cert.pem")]
        if len(certs) != 1:
            print(f"cannot infer anchor from {args.bundle}: {certs}", file=sys.stderr)
            return 2
        anchor = certs[0][: -len("-cert.pem")]

    lon0, _, lat0 = args.center.partition(",")
    lon0, lat0 = float(lon0), float(lat0)
    rng = random.Random(args.seed)

    deg = args.spread_km / 111.32
    tracks = [
        Track(
            900 + i,
            lat0 + rng.uniform(-deg, deg),
            lon0 + rng.uniform(-deg, deg) / max(math.cos(math.radians(lat0)), 0.1),
            rng,
            home=(lat0, lon0),
            radius_km=args.radius_km,
        )
        for i in range(args.tracks)
    ]

    source = {
        "sensor": args.sensor,
        "site": args.site,
        "node": args.node,
        "goatnet": "zagros-prod",
    }

    session = open_session(args.bundle, anchor, args.endpoint)
    # Slot-rooted: the anchor is the first chunk, which is what makes the
    # producer identity structural rather than self-asserted.
    prefix = f"{anchor}/geo/track/v1"
    print(f"publishing {len(tracks)} SYNTHETIC tracks to {prefix}/<uid> via {args.endpoint}")

    running = True

    def stop(*_):
        nonlocal running
        running = False

    signal.signal(signal.SIGINT, stop)
    signal.signal(signal.SIGTERM, stop)

    pubs = {t.uid: session.declare_publisher(f"{prefix}/{t.uid}") for t in tracks}
    n = 0
    while running:
        for t in tracks:
            t.step(args.interval)
            pubs[t.uid].put(json.dumps(t.to_geotrack(anchor, args.org, source)).encode())
            n += 1
        if n % (len(tracks) * 30) < len(tracks):
            print(f"  {n} samples published", flush=True)
        time.sleep(args.interval)

    session.close()
    print("stopped")
    return 0


if __name__ == "__main__":
    sys.exit(main())
