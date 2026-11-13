# SPDX-License-Identifier: Apache-2.0
"""goat-map -- dots on a map for radar detections and tracks.

Two Zenoh legs and one HTTP surface:

  INGEST leg   subscribe a key pattern carrying CoT, transform each event to
               GeoTrackV1 (goatmap/cot.py), and publish the normalised form
               onto the FABRIC leg. This is the data transformer.

  FABRIC leg   an mTLS session against the goatnet's own router. Publishes
               GeoTrackV1 and subscribes it back, so the map renders what is
               actually on the fabric rather than what this process happened
               to parse. Optional -- with no fabric configured the store is
               fed inline from the ingest leg and the map still works.

  HTTP         the browser app, a snapshot endpoint and an SSE stream.

The transform is a separate concern from the visualisation on purpose: the
browser renders GeoTrackV1 and knows nothing about CoT, MIL-STD type codes or
Zenoh key grammar, so a second sensor family lands by adding a parser rather
than by touching the front end.

Every goatnet-specific value is an environment variable; nothing here names a
site, host, zone or sensor. See .env.example.
"""

from __future__ import annotations

import asyncio
import json
import logging
import os
import time
from pathlib import Path
from typing import Any

import zenoh
from aiohttp import web

from . import cot as cot_mod
from . import envhealth as envhealth_mod
from . import adapters as adapters_mod
from . import viewport as viewport_mod
from . import attest as attest_mod
from . import disclosure as disc_mod
from . import readiness as readiness_mod
from . import scope as scope_mod
from . import layout as layout_mod
from . import navmodel as nav_mod
from . import sites as sites_mod
from .cot import now_iso
from .hub import Hub, HubFull
from .orgs import OrgRegistry
from .store import TrackStore

LOG = logging.getLogger("goat-map")


def _env(name: str, default: str = "") -> str:
    return os.environ.get(name, default).strip()


WEB_ROOT = Path(_env("GOATMAP_WEB_ROOT", "/app/web"))

# -- ingest leg ---------------------------------------------------------------
INGEST_ENDPOINT = _env("GOATMAP_INGEST_ENDPOINT")
INGEST_MODE = _env("GOATMAP_INGEST_MODE", "client")
SUB_KEY = _env("GOATMAP_SUBSCRIBE_KEY", "sensor/cot/*")

# -- fabric leg (optional) ----------------------------------------------------
FABRIC_ENDPOINT = _env("GOATMAP_FABRIC_ENDPOINT")
FABRIC_CERT = _env("GOATMAP_FABRIC_CERT")
FABRIC_KEY = _env("GOATMAP_FABRIC_KEY")
FABRIC_CA = _env("GOATMAP_FABRIC_CA")
PUBLISH_PREFIX = _env("GOATMAP_PUBLISH_PREFIX")
# What the fabric leg CONSUMES. One chunk for the producer anchor, so a single
# subscription spans every producer and the ACL -- not this pattern -- decides
# which of them actually arrive.
FABRIC_SUBSCRIBE = _env("GOATMAP_FABRIC_SUBSCRIBE", "*/geo/track/v1/*")

# Site availability (docs/design/goat-map-site-availability.md, step 1). A JSON
# file declaring what exists where; empty means the endpoint reports an empty
# inventory rather than 404, so a viewer can tell "nothing declared" from "this
# build cannot answer". Site membership is DECLARED, never inferred from
# traffic — a sensor that is powered off would otherwise silently stop being
# part of its site, which is the failure this surface exists to close.
SITES_FILE = _env("GOATMAP_SITES_FILE")

# Health join (site-availability step 2). The mesh domain of EnvHealthV1,
# published per goatnet by goat-mesh-health. Empty disables the join and
# /api/sites keeps reporting health_joined:false — visibly unhealthed rather
# than implying every site is fine.
#
# A zenoh denial is SILENT: if goat-map's ACL subject lacks read on this
# key the subscription simply delivers nothing, which is indistinguishable from
# a quiet fabric. `health_joined` reports whether a reading has ACTUALLY
# arrived, so that case is visible rather than inferred from config.
SITES_HEALTH_KEY = _env("GOATMAP_SITES_HEALTH_KEY")

# Attestation log — human reports for kit with no telemetry (generators). File
# is append-only JSONL so a crash cannot lose entries and an operator can read
# it with `tail`. Empty disables capture entirely; the endpoints then 404
# rather than pretending to record.
ATTEST_LOG_FILE = _env("GOATMAP_ATTEST_LOG")

# Per-viewer scoping. `off` (default) is byte-identical to pre-scoping
# behaviour; `enforce` is fail-closed and requires the oauth2-proxy sidecar in
# front. See goatmap/scope.py for why only X-Forwarded-Groups is trusted.
RBAC_MODE = _env("GOATMAP_RBAC_MODE", "off")

# Ceiling on concurrent SSE stream connections. Every connected browser costs
# one queue that `Hub.broadcast` walks on every fabric message, so this bounds
# fan-out work, not just sockets. Generous relative to the real operator
# population (this is a mesh-internal, authenticated surface — there is no
# anonymous client) and there purely so an unattended reconnect loop or a wall
# of dashboards cannot grow the cost without limit. 0 disables the ceiling.
MAX_STREAM_CLIENTS = int(_env("GOATMAP_MAX_STREAM_CLIENTS", "64") or 0)

# Host-side files the readiness adapters read. Written by the node itself
# (its discovery service) or mirrored in; empty disables that rung's evidence, which
# renders as unknown rather than as a fault.
DISCOVERY_FILE = _env("GOATMAP_DISCOVERY_FILE")
NODE_HEALTH_FILE = _env("GOATMAP_NODE_HEALTH_FILE")

# Node health arrives on the FABRIC, not from a local file: the publishers run
# on the sensor nodes and their bridge forwards under the slot anchor, while
# this service runs on the observability host. Reading a local path would only
# ever work on the node itself.
#   <anchor>/<node>/<site>/health/v1  ->  NodeHealthV1 (JSON)
NODE_HEALTH_KEY = _env("GOATMAP_NODE_HEALTH_KEY")

# Region declarations (JSON, same file as sites or its own). Regions are the
# ONLY geographic disclosure: within a region a site is placed on an abstract
# grid that encodes nothing about where it is. See goatmap/layout.py.
REGIONS_FILE = _env("GOATMAP_REGIONS_FILE") or SITES_FILE
# Portal, for resolving anchor -> (org display name, synthetic flag).
PORTAL_URL = _env("GOATMAP_PORTAL_URL")
PORTAL_TOKEN = _env("GOATMAP_PORTAL_TOKEN")
PORTAL_REFRESH_S = float(_env("GOATMAP_PORTAL_REFRESH_S", "300"))
# This service's OWN anchor, stamped on records it republishes from the
# unauthenticated ingest leg. Those records are attributable to this service
# and to nothing stronger -- see goatmap/cot.py.
INGEST_ANCHOR = _env("GOATMAP_PRODUCER_ANCHOR", "0" * 32)

# auto | fabric | ingest
STORE_SOURCE = _env("GOATMAP_STORE_SOURCE", "auto")

# -- labels + view ------------------------------------------------------------
SENSOR = _env("GOATMAP_SENSOR", "unknown-sensor")
SITE = _env("GOATMAP_SITE", "unknown-site")
NODE = _env("GOATMAP_NODE", "unknown-node")
GOATNET = _env("GOATMAP_GOATNET", "unknown-goatnet")
TTL_S = float(_env("GOATMAP_TTL_S", "120"))
# Report degraded after this long with no inbound message. Must exceed the
# sensor's quietest normal gap or a healthy-but-idle feed reads as broken.
STALE_AFTER_S = float(_env("GOATMAP_STALE_AFTER_S", "180"))
HTTP_PORT = int(_env("GOATMAP_HTTP_PORT", "8080"))
CENTER = _env("GOATMAP_CENTER", "0,0")
ZOOM = float(_env("GOATMAP_ZOOM", "10"))
# Which extract under web/basemap/ this goatnet renders. Every goatnet-specific
# value lives in deploy/goatnets/<goatnet>.env and nothing else names a site, so
# one goatnet never opens over another's ground.
# No default — an unset value serves no basemap and says so, rather than
# silently falling back to some other environment's geography.
BASEMAP = _env("GOATMAP_BASEMAP", "")

SOURCE = {"sensor": SENSOR, "site": SITE, "node": NODE, "goatnet": GOATNET}

FABRIC_ENABLED = bool(FABRIC_ENDPOINT)
STORE_FROM_FABRIC = FABRIC_ENABLED if STORE_SOURCE == "auto" else STORE_SOURCE == "fabric"


# Hub lives in goatmap/hub.py so its ceilings stay testable without `zenoh`
# installed — see that module's docstring.


def _session(endpoint: str, mode: str, tls: dict[str, str] | None = None) -> zenoh.Session:
    conf = zenoh.Config()
    conf.insert_json5("mode", json.dumps(mode))
    conf.insert_json5("connect/endpoints", json.dumps([endpoint]))
    # A leaf consumer never advertises itself as a route for other peers.
    conf.insert_json5("scouting/multicast/enabled", "false")
    if tls:
        for key, value in tls.items():
            # json5 literals, not quoted strings: zenoh types enable_mtls and
            # verify_name_on_connect as booleans and rejects `"true"` with
            # `invalid type: string "true", expected a boolean`.
            raw = value if value in ("true", "false") else json.dumps(value)
            conf.insert_json5(f"transport/link/tls/{key}", raw)
    return zenoh.open(conf)


class Service:
    def __init__(self, store: TrackStore, hub: Hub, loop: asyncio.AbstractEventLoop) -> None:
        self.store = store
        self.hub = hub
        self.loop = loop
        self.ingest: zenoh.Session | None = None
        self.fabric: zenoh.Session | None = None
        self.fabric_error: str | None = None
        self.ingest_error: str | None = None
        self.started_at = time.monotonic()
        self.cot_msgs = 0
        # Parsed fine, carries no position -- e.g. a jamming-status message
        # whose CoT <point> has no lat/lon. Not an error; not a dot.
        self.unmappable = 0
        # Latest non-positional status per id (jamming, health, mode). Small,
        # bounded by the number of distinct status labels a sensor emits.
        self.statuses: dict[str, dict] = {}
        self.status_msgs = 0
        self.published = 0
        self.fabric_msgs = 0
        self.last_msg_at: str | None = None
        # Wall clock is for display; staleness is judged on the monotonic
        # clock so an NTP step cannot fake or mask a stall.
        self.last_recv_at: float | None = None
        self.orgs = OrgRegistry(PORTAL_URL, PORTAL_TOKEN, PORTAL_REFRESH_S)
        # Shared with the HTTP layer so /api/sites reports the SAME reading the
        # fabric subscriber received — one instance, no second copy to drift.
        self.site_health = SiteHealth()
        self.node_health = NodeHealthCache()
        # Producers seen on the wire that the registry cannot name. Surfaced,
        # never silently rendered as if they were understood.
        self.unverified_anchors: set[str] = set()

    # ---------------------------------------------------------------- open --

    def open(self) -> None:
        """First attempt. Failures here are NOT fatal and NOT final -- the
        supervisor task retries them for the life of the process.

        This used to give up permanently: a session that failed to open was
        logged once and never retried, so a restart while the upstream router
        happened to be down left the service serving an empty map forever,
        with /healthz still reporting `up`: a green health check over a dead
        input.
        """
        if FABRIC_ENABLED:
            self._open_fabric()
        if INGEST_ENDPOINT:
            self._open_ingest()
        if not INGEST_ENDPOINT and not FABRIC_ENABLED:
            LOG.error("no ingest and no fabric leg configured -- nothing to show")

    async def refresh_orgs(self) -> None:
        while True:
            await asyncio.get_running_loop().run_in_executor(None, self.orgs.refresh)
            await asyncio.sleep(PORTAL_REFRESH_S)

    async def supervise(self) -> None:
        """Keep both legs open. Zenoh reconnects a session it has already
        established; nothing but this reopens one that never opened."""
        delay = 5
        while True:
            await asyncio.sleep(delay)
            reopened = False
            if INGEST_ENDPOINT and self.ingest is None:
                self._open_ingest()
                reopened = self.ingest is not None
            if FABRIC_ENABLED and self.fabric is None:
                self._open_fabric()
                reopened = reopened or self.fabric is not None
            # Back off while a leg stays down so a long outage does not spin,
            # and snap back to a short interval once something reconnects.
            if reopened:
                delay = 5
            elif self.ingest is None or (FABRIC_ENABLED and self.fabric is None):
                delay = min(delay * 2, 60)
            else:
                delay = 5

    def _open_fabric(self) -> None:
        try:
            self.fabric = _session(FABRIC_ENDPOINT, "client", self._tls_client())
            LOG.info("fabric session open (%s) publishing %s", FABRIC_ENDPOINT, PUBLISH_PREFIX)
        except Exception as exc:
            # A missing grant or a wrong CN must not take the map down; it
            # degrades to the ingest leg and says so on /healthz.
            self.fabric = None
            self.fabric_error = str(exc)
            LOG.error("fabric session failed, degrading to ingest-only: %s", exc)
            return

        if STORE_FROM_FABRIC:
            self.fabric.declare_subscriber(FABRIC_SUBSCRIBE, self._on_fabric)
            LOG.info("store fed from fabric subscription %s", FABRIC_SUBSCRIBE)

        # Site-availability health join (step 2). Rides the SAME fabric session
        # and identity as the track leg — no second cert, no second subject. The
        # ACL must grant this subject read on the health key; a denial is silent
        #, so /api/sites reports whether a reading actually ARRIVED
        # rather than whether one was configured.
        if NODE_HEALTH_KEY:
            # Same fabric session and identity as the track leg — no second
            # cert, no second ACL subject. A denial here is silent, so
            # the readiness view reports rungs as unknown rather than implying
            # the nodes are unhealthy.
            self.fabric.declare_subscriber(
                NODE_HEALTH_KEY,
                lambda sample: self.node_health.on_payload(bytes(sample.payload)),
            )
            LOG.info("node health subscribed from %s", NODE_HEALTH_KEY)

        if SITES_HEALTH_KEY and self.site_health is not None:
            self.fabric.declare_subscriber(
                SITES_HEALTH_KEY,
                lambda sample: self.site_health.on_payload(bytes(sample.payload)),
            )
            LOG.info("site health joined from %s", SITES_HEALTH_KEY)

    def _tls_client(self) -> dict[str, str]:
        # enable_mtls is REQUIRED. Without it the client presents no
        # certificate and the router refuses the session with a bare "unable to
        # connect" that is indistinguishable from a network fault.
        tls: dict[str, str] = {"enable_mtls": "true", "verify_name_on_connect": "false"}
        if FABRIC_CA:
            tls["root_ca_certificate"] = FABRIC_CA
        if FABRIC_CERT:
            tls["connect_certificate"] = FABRIC_CERT
        if FABRIC_KEY:
            tls["connect_private_key"] = FABRIC_KEY
        return tls

    def _open_ingest(self) -> None:
        try:
            self.ingest = _session(INGEST_ENDPOINT, INGEST_MODE)
        except Exception as exc:
            self.ingest = None
            self.ingest_error = str(exc)
            LOG.error("ingest session failed (will retry): %s", exc)
            return
        self.ingest_error = None
        LOG.info("ingest session open (mode=%s endpoint=%s)", INGEST_MODE, INGEST_ENDPOINT)
        self.ingest.declare_subscriber(SUB_KEY, self._on_cot)
        LOG.info("subscribed %s", SUB_KEY)

        # Seed from whatever the ingest router already holds so a fresh start
        # is not a blank map for one sensor update interval.
        try:
            for reply in self.ingest.get(SUB_KEY, timeout=3.0):
                ok = getattr(reply, "ok", None)
                if ok is not None:
                    self._handle_cot(str(ok.key_expr), bytes(ok.payload))
        except Exception as exc:  # a cold router is not a startup failure
            LOG.warning("seed query failed (continuing live-only): %s", exc)

    # ------------------------------------------------------------ callbacks --

    def _on_cot(self, sample) -> None:
        try:
            key, payload = str(sample.key_expr), bytes(sample.payload)
        except Exception:
            return
        self.loop.call_soon_threadsafe(self._handle_cot, key, payload)

    def _on_fabric(self, sample) -> None:
        try:
            key, payload = str(sample.key_expr), bytes(sample.payload)
        except Exception:
            return
        self.loop.call_soon_threadsafe(self._handle_fabric, key, payload)

    def _handle_cot(self, key: str, payload: bytes) -> None:
        self.cot_msgs += 1
        self.last_msg_at = now_iso()
        self.last_recv_at = time.monotonic()
        kind = key.rsplit("/", 1)[-1]
        org = self.orgs.get(INGEST_ANCHOR)
        track = cot_mod.parse(
            payload,
            kind=kind,
            source=SOURCE,
            producer_anchor=INGEST_ANCHOR,
            synthetic=bool(org["synthetic"]) if org else False,
            org=org["display_name"] if org else None,
        )
        if track is None:
            # Not a contact. Before calling it unusable, ask whether it is a
            # non-positional SENSOR STATUS — a sensor's alert stream can be
            # exactly that. Counting such reports as "unmappable" makes a
            # working radar look broken and throws away its most significant
            # output.
            status = cot_mod.parse_status(
                payload, kind=kind, source=SOURCE,
                producer_anchor=INGEST_ANCHOR,
                synthetic=bool(org["synthetic"]) if org else False,
                org=org["display_name"] if org else None,
            )
            if status is not None:
                self.statuses[status["id"]] = status
                self.status_msgs += 1
                return
            self.unmappable += 1
            return
        self._publish(track)
        if not STORE_FROM_FABRIC or self.fabric is None:
            self._store(track)

    def _handle_fabric(self, key: str, payload: bytes) -> None:
        self.fabric_msgs += 1
        self.last_recv_at = time.monotonic()
        try:
            track = json.loads(payload)
        except ValueError:
            return
        if not isinstance(track, dict) or "id" not in track:
            return

        # PROVENANCE IS STRUCTURAL, NOT ASSERTED. The anchor is the key's first
        # chunk, and the router ACL admits writes nowhere but a peer's own
        # anchor -- so this cannot be forged by the payload. Whatever the body
        # claimed is discarded.
        anchor = key.split("/", 1)[0]
        track["producer_anchor"] = anchor

        # Likewise `synthetic` and `org`: taken from the REGISTRY, never from
        # the sender. A real producer must not be able to label itself
        # notional (nor a synthetic one label itself real).
        org = self.orgs.get(anchor)
        if org is None:
            self.unverified_anchors.add(anchor)
            # Unknown producer: do not assert either way. It is not marked
            # synthetic (which would imply "safe to ignore") and it is
            # surfaced in the honesty counters instead.
            track["org"] = None
            track.setdefault("synthetic", False)
        else:
            # Clear any earlier unverified mark: at startup the first samples
            # can arrive before the first registry fetch lands, and a producer
            # that stayed flagged after it was resolved would report a
            # provenance gap that no longer exists.
            self.unverified_anchors.discard(anchor)
            track["org"] = org["display_name"]
            track["synthetic"] = bool(org["synthetic"])

        # Namespace the id by producer so two orgs reusing a sensor uid cannot
        # collide into one dot on the map.
        track["id"] = f"{anchor[:8]}:{track['id']}"
        self._store(track)

    def _store(self, track: dict[str, Any]) -> None:
        self.hub.broadcast("upsert", self.store.upsert(track))

    def _publish(self, track: dict[str, Any]) -> None:
        """Put the normalised form on the fabric.

        Per-track key (not one shared key) so a consumer can subscribe to a
        single track and so a query returns the whole current picture rather
        than only the last track to move.
        """
        if self.fabric is None or not PUBLISH_PREFIX:
            return
        leaf = track["id"].replace("/", "_").replace(":", "/")
        try:
            self.fabric.put(f"{PUBLISH_PREFIX}/{leaf}", json.dumps(track).encode())
            self.published += 1
        except Exception as exc:
            LOG.warning("republish failed: %s", exc)

    # ----------------------------------------------------------------- meta --

    def health(self) -> tuple[str, list[str]]:
        """Honest health: `up` only when an input is actually delivering.

        A map that renders nothing looks identical whether the sky is empty or
        the feed is dead, so the SERVICE must distinguish them, or every
        green light stays green through an outage. Reasons are returned, not
        just a verdict, so a prober says what is wrong without a second call.
        """
        reasons: list[str] = []
        if INGEST_ENDPOINT and self.ingest is None:
            reasons.append(f"ingest session down ({self.ingest_error or 'not connected'})")
        if FABRIC_ENABLED and self.fabric is None:
            reasons.append(f"fabric session down ({self.fabric_error or 'not connected'})")

        # Connected but silent is the failure that hides. Only assert it once
        # the process has been up long enough for a first message to be due.
        age = None if self.last_recv_at is None else time.monotonic() - self.last_recv_at
        if age is None:
            if time.monotonic() - self.started_at > STALE_AFTER_S:
                reasons.append(f"no message ever received in {STALE_AFTER_S:.0f}s")
        elif age > STALE_AFTER_S:
            reasons.append(f"no message for {age:.0f}s")

        return ("up" if not reasons else "degraded", reasons)

    def stats(self) -> dict[str, Any]:
        uptime = time.monotonic() - self.started_at
        return {
            "uptime_s": round(uptime, 1),
            "messages": self.cot_msgs,
            "unmappable": self.unmappable,
            # Split out so "we dropped it" and "it was status, not a contact"
            # are never the same number again.
            "status_messages": self.status_msgs,
            "statuses": sorted(self.statuses.values(), key=lambda x: x["label"]),
            "published": self.published,
            "fabric_messages": self.fabric_msgs,
            "msgs_per_s": round(self.cot_msgs / uptime, 2) if uptime > 0 else 0.0,
            "tracks": len(self.store),
            "expired": self.store.total_dropped,
            "last_message_at": self.last_msg_at,
            "clients": len(self.hub),
            # Say plainly which leg the picture is coming from, and why, so a
            # degraded deployment is visible instead of merely quieter.
            "store_source": "fabric" if (STORE_FROM_FABRIC and self.fabric) else "ingest",
            "fabric": "up" if self.fabric else ("failed" if FABRIC_ENABLED else "disabled"),
            "fabric_error": self.fabric_error,
            "ingest_error": self.ingest_error,
            "stale_after_s": STALE_AFTER_S,
            "registry": self.orgs.status(),
            "unverified_producers": sorted(self.unverified_anchors),
            "node_health": self.node_health.status(),
        }


_ESTATE_CACHE: dict = {}


def _read_estate() -> dict:
    """The declared estate: regions, sites, attestable subjects. One file.

    Cached because it is config read on every request path; a redeploy restarts
    the process, which is the intended way to pick up a change.
    """
    if _ESTATE_CACHE:
        return _ESTATE_CACHE
    if not SITES_FILE:
        return {}
    try:
        with open(SITES_FILE, encoding="utf-8") as fh:
            _ESTATE_CACHE.update(json.load(fh))
    except Exception as exc:  # noqa: BLE001
        LOG.warning("estate declaration unreadable (%s): %s", SITES_FILE, exc)
    return _ESTATE_CACHE


def _load_site_inventory() -> list:
    """Read the declared site inventory, failing soft and loudly.

    A missing or malformed file yields an EMPTY inventory rather than killing
    the service: the track picture is the primary job and must not go dark
    because a secondary declaration is broken. The endpoint reports the count,
    so "nothing declared" stays distinguishable from "declared and healthy".
    """
    if not SITES_FILE:
        return []
    try:
        spec = _read_estate()
        return sites_mod.load_sites(spec.get("sites", []))
    except Exception as exc:  # noqa: BLE001 — any failure degrades to empty
        LOG.warning("site inventory unreadable (%s): %s", SITES_FILE, exc)
        return []


class NodeHealthCache:
    """Latest NodeHealthV1 per node, from the fabric.

    Keyed by the payload's own `node` field rather than the key expression, so a
    re-anchored or re-sited publisher still lands in the right slot. Staleness
    is judged by the adapters against `ts_epoch`, not by arrival time here — an
    old reading that keeps being re-delivered is still old.
    """

    def __init__(self) -> None:
        self.by_node: dict[str, dict] = {}
        self.errors = 0

    def on_payload(self, raw: bytes) -> None:
        try:
            doc = json.loads(raw)
        except Exception:  # noqa: BLE001
            self.errors += 1
            return
        if not isinstance(doc, dict) or doc.get("schema") != "NodeHealthV1":
            # Not ours. Silently ignoring the wrong schema is safer than
            # half-parsing it into a confident wrong answer.
            self.errors += 1
            return
        node = doc.get("node")
        if node:
            self.by_node[str(node)] = doc

    def get(self, node: str) -> dict:
        return self.by_node.get(node, {})

    def status(self) -> dict[str, Any]:
        """Which nodes we are actually hearing from, and how old each is.

        A declared node whose chain reads `unknown` has two very different
        causes — it is not publishing, or we are not receiving it — and the
        chain itself cannot tell them apart. This is the cheapest place to
        answer that: `heard` is arrival-side ground truth, independent of the
        declared estate, so comparing the two lists localises the gap without
        anyone guessing. Ages are computed from the payload's own `ts_epoch`,
        so a stalled publisher whose messages keep being redelivered still
        shows as old.
        """
        now = time.time()
        heard = {}
        for node, doc in sorted(self.by_node.items()):
            ts = doc.get("ts_epoch")
            heard[node] = round(now - ts, 1) if isinstance(ts, (int, float)) else None
        return {"heard": heard, "errors": self.errors}


class SiteHealth:
    """Last mesh reading, and whether one has ever actually arrived.

    `joined` is deliberately about ARRIVAL, not configuration. A zenoh denial is
    silent, so a missing ACL grant looks exactly like a quiet fabric —
    reporting "configured" would turn a permissions bug into a green map.
    """

    def __init__(self) -> None:
        self.reading: envhealth_mod.MeshHealth | None = None
        self.received_at: float | None = None
        self.error: str | None = None
        # Peer-domain readings (arm 17), latest per scope. Kept so a key
        # expression that also matches ops/peer/health/** decodes them instead
        # of reporting an error; the map draws nothing new from them: a
        # peer fault is a Zenoh ID, which joins to no declared system here.
        self.peer: dict[str, envhealth_mod.PeerHealth] = {}

    def on_payload(self, payload: bytes) -> None:
        try:
            reading = envhealth_mod.decode_any(payload)
            if isinstance(reading, envhealth_mod.PeerHealth):
                # Never replaces the mesh reading the map is built on.
                self.peer[reading.scope] = reading
                return
            self.reading = reading
            self.received_at = time.time()
            self.error = None
        except Exception as exc:  # noqa: BLE001
            # Keep the previous reading; say why the new one failed. A bad
            # payload must not blank a picture that was working.
            self.error = str(exc)
            LOG.warning("mesh health payload undecodable: %s", exc)

    @property
    def joined(self) -> bool:
        return self.reading is not None

    def age_s(self) -> float | None:
        return None if self.received_at is None else round(time.time() - self.received_at, 1)

    def apply(self, sites: list) -> None:
        """Fold the reading onto the inventory. Unseen systems stay unmetered."""
        if self.reading is None:
            return
        links = self.reading.by_peer()
        for site in sites:
            site.systems = [
                sites_mod.System(
                    name=s.name, cls=s.cls, peer=s.peer,
                    state=(links[s.peer].color if s.peer in links else "unmetered"),
                )
                for s in site.systems
            ]


def _load_regions() -> list:
    """Declared regions. Missing file = no regions, and /api/estate says so."""
    if not REGIONS_FILE:
        return []
    try:
        return layout_mod.load_regions(_read_estate().get("regions", []))
    except Exception as exc:  # noqa: BLE001
        LOG.warning("region inventory unreadable (%s): %s", REGIONS_FILE, exc)
        return []


def _load_attest_log() -> "attest_mod.AttestationLog":
    """Rebuild the log from JSONL. Last word per subject wins.

    Append-only on disk so a crash cannot lose an entry and an operator can read
    the file with `tail`. A malformed line is skipped LOUDLY rather than
    aborting the load — one bad record must not blind the whole site, which is
    the same rule the peer-class and registry loaders follow.
    """
    log = attest_mod.AttestationLog()

    # Declared subjects come from the estate file (config), not the log
    # (events). Keeping them separate is what lets a never-attested generator
    # appear as a GAP rather than simply not existing.
    for row in (_read_estate().get("attestable") or []):
        try:
            log.declare(attest_mod.AttestableSubject(**row))
        except Exception as exc:  # noqa: BLE001
            LOG.warning("attestable subject unusable (%s): %s", row, exc)

    if not ATTEST_LOG_FILE:
        return log
    try:
        with open(ATTEST_LOG_FILE, encoding="utf-8") as fh:
            for lineno, line in enumerate(fh, 1):
                line = line.strip()
                if not line:
                    continue
                try:
                    rec = json.loads(line)
                    log.record(attest_mod.Attestation(**rec["attestation"]))
                except Exception as exc:  # noqa: BLE001
                    LOG.warning("attestation log line %d unusable: %s", lineno, exc)
    except FileNotFoundError:
        pass  # no attestation log yet
    except Exception as exc:  # noqa: BLE001
        LOG.warning("attestation log unreadable (%s): %s", ATTEST_LOG_FILE, exc)
    return log


def _append_attestation(rec: dict) -> None:
    with open(ATTEST_LOG_FILE, "a", encoding="utf-8") as fh:
        fh.write(json.dumps(rec, separators=(",", ":")) + "\n")
        fh.flush()
        os.fsync(fh.fileno())


def build_app(svc: Service, store: TrackStore, hub: Hub) -> web.Application:
    site_inventory = _load_site_inventory()
    attest_log = _load_attest_log()
    region_inventory = _load_regions()
    # The Service owns it; the app reads the same instance.
    site_health = svc.site_health
    if site_inventory:
        LOG.info("site inventory: %d site(s) declared", len(site_inventory))
    app = web.Application()
    lon, _, lat = CENTER.partition(",")

    async def meta(_: web.Request) -> web.Response:
        return web.json_response(
            {
                "schema": cot_mod.SCHEMA,
                "source": SOURCE,
                "goatnet": GOATNET,
                "subscribe_key": SUB_KEY,
                "publish_prefix": PUBLISH_PREFIX or None,
                "ttl_s": TTL_S,
                "center": [float(lon or 0), float(lat or 0)],
                "zoom": ZOOM,
                "basemap": BASEMAP or None,
            }
        )

    def _viewer(request: web.Request) -> scope_mod.ViewerScope:
        return scope_mod.scope_from_headers(
            {k: v for k, v in request.headers.items()},
            mode=RBAC_MODE,
            site_orgs={s.site_id: getattr(s, "org", "") for s in site_inventory},
        )

    async def tracks(request: web.Request) -> web.Response:
        return web.json_response(
            scope_mod.track_envelope(store.snapshot(), _viewer(request), svc.stats())
        )

    async def healthz(_: web.Request) -> web.Response:
        # Non-200 when degraded: a health endpoint that always returns 200 is
        # decoration. This is what makes the container healthcheck able to see
        # a dead input instead of only a dead process.
        status, reasons = svc.health()
        return web.json_response(
            {"status": status, "reasons": reasons, **svc.stats()},
            status=200 if status == "up" else 503,
        )

    async def stream(request: web.Request) -> web.StreamResponse:
        # Claim the stream slot BEFORE preparing the response. Once `prepare`
        # has run the status line is already on the wire, so a refusal after
        # that point could only be an abrupt disconnect — indistinguishable to
        # the browser from a network fault. Registering first lets a refusal be
        # an honest 503 the client can act on.
        try:
            q = hub.register()
        except HubFull as exc:
            LOG.warning("stream refused: %s", exc)
            return web.json_response(
                {
                    "error": "stream connection budget spent",
                    "detail": str(exc),
                    "retry_after_s": 15,
                },
                status=503,
                headers={"Retry-After": "15"},
            )
        resp = web.StreamResponse(
            headers={
                "Content-Type": "text/event-stream",
                "Cache-Control": "no-cache",
                "X-Accel-Buffering": "no",
            }
        )
        try:
            await resp.prepare(request)
        except Exception:
            # Never leak the slot if the client vanished between claiming it
            # and the response being prepared.
            hub.unregister(q)
            raise
        # The hub broadcasts one event to every connection, so scoping has to
        # happen per-connection at send time. The scope is fixed for the life
        # of the stream: it comes from the headers oauth2-proxy validated when
        # the connection opened, and a long-lived SSE connection has no way to
        # re-read them. A revoked grant therefore takes effect on reconnect,
        # not mid-stream — acceptable because the fence expires the session
        # cookie (8h) and the browser reconnects, but it IS a lag, so it is
        # written down rather than assumed away.
        viewer = _viewer(request)
        # Which track ids this connection has actually been sent. An `expire`
        # for a track the viewer never received would disclose that the id
        # exists, which is enough to count the hidden estate over time.
        sent: set[str] = set()
        try:
            permitted, _w = scope_mod.scope_tracks(store.snapshot(), viewer)
            sent.update(str(t.get("id")) for t in permitted)
            await _send(resp, "snapshot",
                        scope_mod.track_envelope(store.snapshot(), viewer, svc.stats()))
            while True:
                try:
                    event, data = await asyncio.wait_for(q.get(), timeout=10.0)
                except asyncio.TimeoutError:
                    await resp.write(b": keepalive\n\n")
                    continue
                if event == "upsert":
                    if not viewer.may_see_track(
                        data.get("org"), data.get("producer_anchor", "")
                    ):
                        continue
                    sent.add(str(data.get("id")))
                elif event == "expire":
                    if str(data.get("id")) not in sent:
                        continue
                    sent.discard(str(data.get("id")))
                elif event == "stats":
                    data = scope_mod.scope_stats(data, viewer, len(sent))
                await _send(resp, event, data)
        except (ConnectionResetError, asyncio.CancelledError):
            pass  # disconnect or task cancellation: unregister the stream in finally
        finally:
            hub.unregister(q)
        return resp

    @web.middleware
    async def cache_headers(request: web.Request, handler):
        """Immutable-per-deploy assets get a long cache; live data never does.

        Without this every reload refetches ~900 KB of vendored front end. On a
        healthy link that is merely wasteful; on the degraded path that prompted
        this (1228-byte MTU, 240 ms RTT, 7x jitter) it was the difference
        between a slow page and a page that never finished.

        The vendored bundle and the basemap change only when the image changes,
        so they are safe to pin hard. /api/** and / must never be cached — a
        stale track picture is worse than a slow one.
        """
        resp = await handler(request)
        path = request.path
        if path.startswith(("/vendor/", "/basemap/")):
            resp.headers["Cache-Control"] = "public, max-age=604800, immutable"
        elif path.startswith("/api/") or path == "/":
            resp.headers["Cache-Control"] = "no-store"
        elif path.endswith((".js", ".css", ".html")):
            # OUR OWN CODE, which changes on every deploy. It used to fall
            # through here with no Cache-Control at all, and "no header" does
            # not mean "do not cache": browsers then apply HEURISTIC freshness,
            # typically a fraction of the time since Last-Modified, and serve a
            # stale bundle without asking: the deploy succeeded, the surface
            # reports healthy, and what the browser runs is the old thing.
            #
            # `no-cache` stores the file but REVALIDATES every time, which is
            # not the same as `no-store`. With the ETag and Last-Modified aiohttp
            # already sends, an unchanged asset costs a ~200-byte 304 rather
            # than a refetch — so the degraded-link saving this middleware
            # exists for survives, and a deploy can no longer be invisible.
            resp.headers["Cache-Control"] = "no-cache"
        return resp

    app.middlewares.append(cache_headers)

    async def index(_: web.Request) -> web.FileResponse:
        # add_static serves no directory index, so "/" is wired explicitly
        # rather than left to 403.
        return web.FileResponse(WEB_ROOT / "index.html")

    async def sites(_: web.Request) -> web.Response:
        # Serves ANCHORS, never precise coordinates: quantization happens in
        # sites.py on the server, so the browser never holds a position that a
        # screenshot could leak. See sites.py for why that is a security
        # boundary rather than a formatting choice.
        site_health.apply(site_inventory)
        return web.json_response(
            {
                "sites": [s.as_dict() for s in site_inventory],
                "declared": len(site_inventory),
                # Whether a reading has ACTUALLY arrived — not whether one was
                # configured. A silent zenoh denial is otherwise
                # indistinguishable from a quiet fabric, and would render as a
                # confidently green map built on nothing.
                "health_joined": site_health.joined,
                "health_age_s": site_health.age_s(),
                "health_error": site_health.error,
                "health_key": SITES_HEALTH_KEY or None,
            }
        )

    def _estate_index() -> dict:
        """Rebuild the flat index against the CURRENT health reading."""
        site_health.apply(site_inventory)
        by_id = {s.site_id: s for s in site_inventory}
        return nav_mod.build_index(
            region_inventory,
            by_id,
            layout_for_region=lambda r: layout_mod.grid(
                [sid for sid in r.site_ids if sid in by_id]
            ),
            layout_for_site=lambda s: layout_mod.tree(
                [(y.name, y.parent or None) for y in s.systems]
            ),
        )

    async def estate(_: web.Request) -> web.Response:
        # ONE flat addressable index of the whole estate. The client needs no
        # history stack: every id is reachable from every other in one step, so
        # drill-down is a convenience rather than the only road.
        idx = _estate_index()
        return web.json_response(
            {
                "nodes": {k: v.as_dict() for k, v in idx.items()},
                "regions": [k for k, v in idx.items() if v.kind == "region"],
                "health_joined": site_health.joined,
                "health_age_s": site_health.age_s(),
            }
        )

    async def jump(request: web.Request) -> web.Response:
        # Everywhere reachable in one step from here — the explicit
        # anti-linear-navigation contract.
        node_id = request.query.get("from", "")
        idx = _estate_index()
        if node_id not in idx:
            return web.json_response({"error": "unknown id", "id": node_id}, status=404)
        return web.json_response(
            {
                "from": idx[node_id].as_dict(),
                "targets": [idx[t].as_dict() for t in nav_mod.jump_targets(idx, node_id)],
            }
        )

    async def find(request: web.Request) -> web.Response:
        return web.json_response(
            {"results": nav_mod.search(_estate_index(), request.query.get("q", ""))}
        )

    def _read_json(path: str) -> dict:
        """Best-effort read. Missing/broken -> {} so the rung reads unknown
        rather than inventing a fault from our own inability to look."""
        if not path:
            return {}
        try:
            with open(path, encoding="utf-8") as fh:
                return json.load(fh)
        except Exception as exc:  # noqa: BLE001
            LOG.debug("readiness input unreadable (%s): %s", path, exc)
            return {}

    def _chain_for(site, system) -> readiness_mod.Chain:
        """Compose one system's chain from whatever evidence exists."""
        # A device's evidence comes from the node that hosts it: a radar's
        # components are its parent node's components. `parent` is declared, so
        # this never guesses.
        host_node = getattr(system, "parent", "") or system.name
        nh = svc.node_health.get(host_node) or _read_json(NODE_HEALTH_FILE)
        disc = _read_json(DISCOVERY_FILE)

        comp, comp_detail = adapters_mod.components_from_node_health(nh)
        prod, prod_detail = adapters_mod.producing_from_node_health(nh)

        det, det_detail = (None, "")
        if system.cls in ("radar", "eo-ir", "ew", "effector"):
            det, det_detail = adapters_mod.device_detected_from_discovery(
                disc, getattr(system, "device_type", "") or system.name)

        # Power comes from an attestation when one exists. None (never attested,
        # or expired) leaves the rung unknown — the honest answer.
        powered = attest_log.rung_state(f"{site.site_id}/power")

        # node_online: authority is the mesh vantage, which this service does
        # not hold. Until that is wired, the node's own health arriving at all
        # is corroboration; its absence is genuinely unknown, not "offline".
        online, _ = adapters_mod.node_online_from_node_health(nh)

        return readiness_mod.build_chain(
            system.name, cls=system.cls,
            power=powered, node_online=online,
            components_ok=comp, components_detail=comp_detail,
            device_detected=det,
            producing=prod,
            duty_cycled=getattr(system, "duty_cycled", True),
            evidence={
                "device_detected": {"detail": det_detail},
                "producing": {"detail": prod_detail},
            },
        )

    async def readiness(request: web.Request) -> web.Response:
        """Per-site readiness chains, SCOPED FIRST then rolled up.

        The ordering is the control: a site the viewer may not see contributes
        nothing to any badge, because a badge over hidden estate discloses it.
        """
        viewer = scope_mod.scope_from_headers(
            {k: v for k, v in request.headers.items()},
            mode=RBAC_MODE,
            site_orgs={s.site_id: getattr(s, "org", "") for s in site_inventory},
        )
        permitted = scope_mod.scope_estate(
            site_inventory, viewer,
            {s.site_id: getattr(s, "org", "") for s in site_inventory},
        )

        out = []
        for site in permitted:
            chains = [_chain_for(site, sy) for sy in site.systems]
            verdict = readiness_mod.roll_up(chains)
            payload = {
                "site_id": site.site_id,
                "label": site.label,
                "verdict": verdict,
                "color": readiness_mod.VERDICT_COLOR.get(verdict, "unmetered"),
                "reason": readiness_mod.roll_up_reason(
                    chains, with_counts=viewer.tier == disc_mod.FULL),
                "chains": [disc_mod.redact_chain(c.as_dict(), viewer.tier) for c in chains],
                "attestations": disc_mod.redact_attestations(
                    attest_log.for_site(site.site_id), viewer.tier),
            }
            out.append(payload)

        return web.json_response({
            "sites": out,
            "disclosure": viewer.tier,
            "rbac_mode": RBAC_MODE,
            # Named so the UI can say the picture is partial rather than
            # implying it is the whole estate.
            "scope": viewer.kind,
        })

    async def attestations(request: web.Request) -> web.Response:
        # What a person can report on at this site, INCLUDING the gaps: subjects
        # nobody has ever attested are listed, because a view of only what people
        # bothered to log would flatter the site.
        site = request.query.get("site", "")
        return web.json_response({"subjects": attest_log.for_site(site)})

    async def attest_post(request: web.Request) -> web.Response:
        # Capture path. Deliberately tiny: subject, one of four states, who, and
        # an optional note. An operator standing at a generator in the dark is
        # choosing between four words, not filling in a form.
        if not ATTEST_LOG_FILE:
            return web.json_response({"error": "attestation capture not configured"}, status=404)
        try:
            body = await request.json()
        except Exception:  # noqa: BLE001
            return web.json_response({"error": "expected JSON"}, status=400)

        subject = (body.get("subject") or "").strip()
        state = (body.get("state") or "").strip()
        by = (body.get("by") or "").strip()
        if subject not in attest_log.subjects:
            return web.json_response(
                {"error": "unknown subject", "subject": subject,
                 "known": sorted(attest_log.subjects)}, status=400)
        if state not in attest_mod.STATES:
            return web.json_response(
                {"error": "unknown state", "expected": list(attest_mod.STATES)}, status=400)
        if not by:
            # Attribution is not optional: an unattributed report is a rumour,
            # and the whole point is that a person put their name on it.
            return web.json_response({"error": "`by` is required"}, status=400)

        subj = attest_log.subjects[subject]
        a = attest_mod.Attestation(
            subject=subject, rung=subj.rung, state=state, by=by,
            at=time.time(), note=(body.get("note") or "")[:280], ttl_s=subj.ttl_s,
        )
        attest_log.record(a)
        _append_attestation({"kind": "attestation", "attestation": {
            "subject": a.subject, "rung": a.rung, "state": a.state, "by": a.by,
            "at": a.at, "note": a.note, "ttl_s": a.ttl_s}})
        LOG.info("attestation: %r = %r by %r", subject, state, by)
        return web.json_response({"recorded": a.as_dict()})

    async def query(request: web.Request) -> web.Response:
        """Lasso query — the map's only bulk data path.

        POST {"polygon": [[lon,lat],...], "planes": ["tracks","sites"]}

        Nothing is returned without a polygon. That is the point: the page
        opens blank and stays blank until an operator asks a question, so the
        default state ships no data at all. See viewport.py for why tiering is
        count-driven and why aggregation happens after scoping.
        """
        try:
            body = await request.json()
        except Exception:
            raise web.HTTPBadRequest(text="body must be JSON")
        poly = body.get("polygon") or []
        if not isinstance(poly, list) or len(poly) < 3:
            raise web.HTTPBadRequest(text="polygon needs at least 3 [lon,lat] points")
        # Bound the input. A million-vertex polygon is a cheap way to burn
        # server CPU inside the point-in-polygon loop; a lasso drawn by a
        # human never approaches this.
        if len(poly) > 512:
            raise web.HTTPBadRequest(text="polygon too complex (max 512 points)")

        viewer = _viewer(request)
        planes = body.get("planes") or ["tracks", "sites"]
        out: dict[str, Any] = {"scope": viewer.kind, "disclosure": viewer.tier}

        if "tracks" in planes:
            # SCOPE FIRST, then query. Handing viewport.py the full store and
            # filtering afterwards would mean the aggregate counts were
            # computed over data the viewer may not see.
            permitted, _withheld = scope_mod.scope_tracks(store.snapshot(), viewer)
            out["tracks"] = viewport_mod.query_tracks(permitted, poly, viewer.tier)

        if "sites" in planes:
            site_health.apply(site_inventory)
            permitted_sites = scope_mod.scope_estate(
                site_inventory, viewer,
                {s.site_id: getattr(s, "org", "") for s in site_inventory},
            )

            def _verdict(site: Any) -> str:
                chains = [_chain_for(site, sy) for sy in site.systems]
                return readiness_mod.roll_up(chains)

            out["sites"] = viewport_mod.query_sites(
                permitted_sites, poly, _verdict, viewer.tier)

        return web.json_response(out)

    app.router.add_post("/api/query", query)
    app.router.add_get("/api/readiness", readiness)
    app.router.add_get("/api/attestations", attestations)
    app.router.add_post("/api/attest", attest_post)
    app.router.add_get("/api/estate", estate)
    app.router.add_get("/api/jump", jump)
    app.router.add_get("/api/find", find)
    app.router.add_get("/api/sites", sites)
    app.router.add_get("/api/meta", meta)
    app.router.add_get("/api/tracks", tracks)
    app.router.add_get("/api/stream", stream)
    app.router.add_get("/healthz", healthz)
    app.router.add_get("/", index)
    app.router.add_static("/", WEB_ROOT, show_index=False)
    return app


async def _send(resp: web.StreamResponse, event: str, data: Any) -> None:
    await resp.write(f"event: {event}\ndata: {json.dumps(data)}\n\n".encode())


async def _reaper(store: TrackStore, hub: Hub, svc: Service) -> None:
    while True:
        await asyncio.sleep(5)
        for tid in store.expire():
            hub.broadcast("expire", {"id": tid})
        hub.broadcast("stats", svc.stats())


async def main() -> None:
    logging.basicConfig(
        level=_env("GOATMAP_LOG_LEVEL", "INFO"),
        format="%(asctime)s %(levelname)s %(name)s %(message)s",
    )
    loop = asyncio.get_running_loop()
    store = TrackStore(ttl_s=TTL_S)
    hub = Hub(max_clients=MAX_STREAM_CLIENTS)
    svc = Service(store, hub, loop)
    svc.open()

    runner = web.AppRunner(build_app(svc, store, hub))
    await runner.setup()
    await web.TCPSite(runner, "0.0.0.0", HTTP_PORT).start()
    LOG.info("goat-map serving on :%d (goatnet=%s site=%s)", HTTP_PORT, GOATNET, SITE)

    asyncio.create_task(svc.supervise())
    asyncio.create_task(svc.refresh_orgs())
    await _reaper(store, hub, svc)


if __name__ == "__main__":
    try:
        asyncio.run(main())
    except KeyboardInterrupt:
        pass  # Ctrl-C: exit quietly
