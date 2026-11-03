# goat-map — dots on a map for things sensors detect

A live geographic picture of what a sensor chain is putting on the Zenoh
fabric: position, heading, speed, trail, staleness. It renders what is on the
bus right now and forgets it when the bus stops saying it.

Named the way the rest of the family is (`goat-top`, `goat-who`,
`goat-client`): the `goat-` prefix is the product, the suffix is the function.

**Status:** mesh-only. Reads the fabric over **mTLS** under
its own minted identity, with provenance derived structurally from the Zenoh
key rather than trusted from the payload.

## What it does

```
     sensor chain (decode → CoT)
                 │
                 ▼  INGEST LEG — subscribe CoT
        ┌────────────────────┐
        │      goat-map      │   CoT XML → GeoTrackV1
        └────────────────────┘
                 │  FABRIC LEG — publish GeoTrackV1, subscribe it back
                 ▼
        the goatnet's own router  ──▶ any other consumer
                 │
                 ▼  HTTP: map + snapshot + SSE
              browser
```

The transform and the visualisation are separate concerns on purpose. The
browser renders `GeoTrackV1` and knows nothing about CoT, MIL-STD type codes
or Zenoh key grammar, so a second sensor family lands by adding a parser
(`goatmap/cot.py` is the whole of the current one) rather than by touching the
front end.

With no fabric leg configured the service runs ingest-only: the map still
works, and `/healthz` says which leg the picture is coming from.

## GeoTrackV1

Flat and display-shaped — every field is directly drawable or directly
printable, and anything that needed a lookup table (CoT affiliation and
battle-dimension letters) is resolved server-side.

```json
{
  "schema": "goat.geo.track.v1",
  "id": "radar-a:202", "label": "202", "kind": "track",
  "source": {"sensor": "radar-a", "site": "site-a",
             "node": "node-a", "goatnet": "my-goatnet"},
  "lat": 0.5120, "lon": 0.7340, "alt_m": 21.6,
  "course_deg": 305.0, "speed_mps": 49.9,
  "ce_m": null, "le_m": null,
  "cot_type": "a-u-X", "affiliation": "unknown", "dimension": "other",
  "how": "m-p", "descriptor": "radar/radar-a:202",
  "reported_at": "1970-01-01T03:00:10.297Z",
  "observed_at": "2030-01-01T12:14:10.529Z",
  "trail": [[0.73, 0.51]], "updates": 37
}
```

**Two clocks, never merged.** `reported_at` is whatever the sensor chain
claimed — it can be arbitrarily wrong (the example above reads 1970), so it is
not usable for staleness or ordering. `observed_at` is receipt at this service, and every
age, fade and TTL is computed from that. The UI says which one it is showing
rather than implying the other. Do not "fix" the sensor clock in the
transform: a repaired timestamp is indistinguishable from a real one
downstream.

## Endpoints

| Path | What |
| --- | --- |
| `/` | the map |
| `/api/meta` | schema, source labels, subscribed key, publish prefix, TTL, initial view |
| `/api/tracks` | full snapshot + counters |
| `/api/stream` | SSE: `snapshot`, `upsert`, `expire`, `stats` |
| `/healthz` | `status: up` plus counters, `store_source` and `fabric` state |

`unmappable` is not an error count. Some CoT messages parse fine and carry no
position — a sensor's status message may have a `<point>` with no lat/lon.
It is not an error, and it is not a dot.

## Basemap

Self-hosted: a Protomaps extract of the area of interest plus vendored
MapLibre GL, pmtiles.js, glyphs and sprites. The page makes **no off-box
request** — the surface is mesh-only and the hosts have no general egress, so
a CDN-dependent basemap would render as a blank page exactly when it matters.
CI fails the build if an external URL appears in the style JSON.

Extracts are **per goatnet**, supplied under `web/basemap/` and selected by
`GOATMAP_BASEMAP` in that goatnet's env file; none ships with this repository.
A basemap filename is a site name, and site names belong in the env file.

A new area is one command against a Protomaps daily build:

```bash
pmtiles extract https://build.protomaps.com/<YYYYMMDD>.pmtiles <name>.pmtiles \
  --bbox=<minlon>,<minlat>,<maxlon>,<maxlat> --maxzoom=14
```

Watch the size: the same footprint over dense urban ground is several times
larger than over open country (a 0.9°×0.55° urban box runs to about 44 MB, a
0.5°×0.23° one to about 15 MB).

## Deploying

Every goatnet-specific value lives in one file, `deploy/goatnets/<goatnet>.env`.
Nothing else in this repo names a site, host, zone or sensor.

```bash
deploy/deploy.sh my-goatnet ops@my-mesh-host --build
```

`--build` builds on the target host; it is the bootstrap path before a CI
image exists for that architecture. The steady state is a profile pinning
`GOATMAP_IMAGE` to a multi-arch **list** digest.

The script refuses a profile whose `GOATMAP_BIND_IP` is empty or `0.0.0.0` —
the port must bind the host's mesh IP.

### Adding a goatnet

Copy `.env.example` to `deploy/goatnets/<goatnet>.env` and fill in the ingest
endpoint, subscribe key, labels, map centre, bind IP and port. If the goatnet
wants a friendly hostname rather than IP:port, its monorepo side needs a
Traefik route, an LE cert SAN entry, an isitup row and a mesh DNS record —
see `docs/design/goat-map.md` in the goat monorepo.

### Where it runs

On the **observability host**, not the mgmt host: the mgmt host carries the whole
mgmt stack and this has no business competing with it. The mgmt host only fronts
TLS and proxies to the obs host over the mesh. Container ceilings are 256 MB / 0.5 CPU.

## Tests

```bash
pip install pytest jsonschema && pytest tests-public -q
```

`goatmap/cot.py` is pure and is what the tests cover first — including 1970
timestamps and empty-point payloads. All track data in the tests is invented.
