# Format transformers: converting messages with goat-transformers

| Field | Value |
|---|---|
| Status | Parts 1 to 3 are built and tested in CI. Nothing here is deployed, and every switch defaults to off. Parts 4 and 5 are designed. |
| Code | `ops/zenoh-shims/transform-shim/`, `ops/serve/goat-map/goatmap/cot_library.py`, `ops/xf/` |
| Library | [goat-transformers](https://github.com/USAREUR-AF/goat-transformers), commit in `ops/xf/goat-transformers.ref` |
| Related | goatcore [zenoh-data-fabric.md](https://github.com/usareur-af/goatcore/blob/main/docs/design/zenoh-data-fabric.md), [zenoh-brokerless-consequences.md](https://github.com/usareur-af/goatcore/blob/main/docs/design/zenoh-brokerless-consequences.md), ADRs [0112](https://github.com/usareur-af/goatcore/blob/main/docs/adr/0112-tech-strategy-workload-shape-language-boundary.md), [0209](https://github.com/usareur-af/goatcore/blob/main/docs/adr/0209-shim-pattern.md), [0217](https://github.com/usareur-af/goatcore/blob/main/docs/adr/0217-shim-repo-containment.md), [1123](https://github.com/usareur-af/goatcore/blob/main/docs/adr/1123-client-demand-must-not-amplify.md) |

## 1. What this is

goat-transformers converts messages between formats (Cursor on Target, GeoJSON,
SAPIENT, MAVLink and others). A format is a data file, called a pack, and an
engine in each of seven languages reads the same packs. This document states
how goat uses that library: which piece of goat calls which engine, where the
code lives, and what is built.

A conversion decodes the source format into one shared model and encodes the
model into the target format. Two formats exchange exactly the model keys both
packs map. Values that only the caller knows, such as the time a message
arrived, are passed to the conversion as context.

| Part | What | Engine | Status |
|---|---|---|---|
| 1 | `transform-shim`, a fabric peer that subscribes in one format and republishes in another | Rust | built, tested in CI, not deployed |
| 2 | goat-map's Cursor on Target to GeoTrackV1 step through the library, selected by `GOATMAP_COT_ENGINE=library` | Python | built, tested in CI, default `builtin` |
| 3 | The `geo-track-v1` pack and the `geotrack` model module, written here and taken into the library with the three `cot-xml` keys they need | data | in the library since commit `5459b24`; both bundles are built from the library alone |
| 4 | `goat xf` on the operator's machine | Rust | designed; no goat code is needed |
| 5 | The portal inspector decoding any pack-described format | Go | designed |

## 2. Where each piece lives

All code is in goatstuff. goatcore states the rules it follows and is not
changed.

| Piece | Home | The sentence that decides it |
|---|---|---|
| `transform-shim` | `ops/zenoh-shims/transform-shim/` | goatstuff README: "The inspection surfaces, demos and example applications that run on a goat data fabric ... none of it is needed to run a goatnet", and its table row "synthetic vendors and trial peers ... `ops/zenoh-trial-peers/`, `ops/zenoh-shims/`". The two existing shims, `rest-pub-bridge` and `feeds-shim`, are there |
| Its language | Rust | ADR 0112: "Sidecar / interceptor / shim *on the data path* (even if logically thin)" is Rust |
| Its key prefixes | route table | zenoh-data-fabric.md: "Wire-faithful vendor surfaces ride `telemetry/<format>/`; canonical data lives under `data/`" and "Translation lives at the boundary, in adapter components" |
| goat-map's library path | `ops/serve/goat-map/` | goat-map README: "a second sensor family lands by adding a parser (`goatmap/cot.py` is the whole of the current one) rather than by touching the front end" |
| Its language | Python | ADR 0112 gives Rust to a hot path and leaves a request-shaped service its own language; the library's `docs/languages.md`: "A converter inside a request-shaped service or a tool is free to use that service's language" |
| A format whose specification is not public | not here | ADR 0217: "Each commercial vendor gets its own dedicated repo"; the library's `docs/boundary.md`: such a pack "is kept in a private overlay repository". The shim and goat-map load with the distribution limit `public` |

ADR 0217 names a repository `goat-open-standards` for open-standard shims and
a shared `goat-shim-framework/` in the trunk. Neither exists in the public
repositories, so this change follows the place the existing shims use. The
library takes the role the ADR gives the open-standards bundle: one description
per open format, shared by every consumer.

## 3. transform-shim

The fabric has no central component, so a transformer is a peer like any
other: it subscribes, converts and publishes. `transform-shim` does that from
a route table. Each route names a key expression to subscribe to, the format
found there, the format to publish, and the literal prefix that replaces the
start of the key.

```json
{
  "name": "cot-to-geojson",
  "subscribe": "telemetry/cot/zagros/*",
  "from": "cot-xml",
  "to": "geojson",
  "strip_prefix": "telemetry/cot/",
  "publish_prefix": "data/entity/observation/"
}
```

A message on `telemetry/cot/zagros/4411` is published on
`data/entity/observation/zagros/4411` with the encoding
`application/geo+json;geojson`. The full member list is in the shim's
`README.md`.

A route's `set` member gives the conversion the model keys that only the
receiving peer knows (the library's context). A value is fixed text or a
template: `{received_at}` and `{received_ms}` are the receive clock,
`{key}`, `{key.N}` and `{capture.N}` the key the message arrived on, its
chunks and what each wildcard of `subscribe` matched, `{timestamp_at}`,
`{encoding}`, `{attachment}` and `{attachment.NAME}` the sample's own
timestamp, encoding and attachment. A placeholder with no value for a message
leaves that key out, and the target format decides: a default applies, or a
kind that requires the key refuses the message. This is what lets a route
produce `geo-track-v1`, whose `kind` is the last chunk of the key and whose
`observed_at` is the receive time; `config.example.json` holds such a route.

### 3.1 Load on the fabric

One received message produces at most one published message. The shim issues
no query and declares no queryable, so nothing a client does can make it ask
the fabric for anything (ADR 1123).

- The subscriber callback never waits. It places the sample in a bounded queue
  (`--queue-depth`, default 1024); a sample that arrives when the queue is full
  is dropped and counted. This follows zenoh-data-fabric.md: "a consumer that
  stops draining can block the router's receive path".
- A subscription made only of wildcard chunks is refused at start, and every
  subscription begins with a literal prefix (zenoh-data-fabric.md, "No one
  subscription may match the whole keyspace").
- A route whose published keys intersect any route's subscription is refused
  at start, so the table cannot loop or multiply a message.

### 3.2 Failures

Every refusal is counted per route, logged, and published.

| Event | Counter | Log |
|---|---|---|
| The library refuses a message (malformed, a required key missing) | `refused` | warning on the 1st, 2nd, 4th, 8th ... refusal of the route, with the library's error |
| The queue is full | `dropped` | in the heartbeat |
| A publish fails | `publish_failed` | error, same spacing |
| A route table the shim would have to guess about | none: the process does not start | the finding, naming the route |
| The heartbeat cannot be published | none: the process exits non-zero | the error |

Exiting when the session cannot carry the heartbeat is convention 4 of ADR
0209: "Zenoh disconnect: shim exits; orchestrator restarts."

### 3.3 Identity, access control and quality of service

The shim holds one certificate and opens one mTLS client session, with the
same configuration block as `rest-pub-bridge`. It needs these grants and no
others:

| Grant | Key expressions |
|---|---|
| declare subscriber | each route's `subscribe` |
| put | each route's `publish_prefix` followed by `**` |
| put | `<key_prefix>services/shims/transform/<instance>/announce` and `<key_prefix>health/shims/transform/<instance>` |

On a goatnet whose routers use the slot pool of ADR 1025, a peer writes only
under its own 32-hex anchor. There the shim is one registered system:
`publish_prefix` and `key_prefix` begin with its anchor, and each subscription
to another system's keys is a row in `zenoh_peer_cross_tenant_read`. The
top-level `telemetry/` and `data/` keys of the example are the shape
zenoh-data-fabric.md reserves; which shape a goatnet uses is that goatnet's
configuration, and the shim builds in neither.

Quality of service is fixed by the prefix a route publishes under
(zenoh-data-fabric.md, "QoS class is bound to the prefix"). The shim sets no
priority of its own.

### 3.4 Announcement and heartbeat

The shim follows conventions 2 and 3 of ADR 0209. At start it publishes its
record on `services/shims/transform/<instance>/announce`: class, version,
instance, site, the formats it translates from and to, the SHA-256 of its
route table, and the library commit. Every 30 seconds it publishes the
counters of section 3.2 on `health/shims/transform/<instance>`, with `state`
set to `degraded` when a fault counter rose since the previous heartbeat.

The heartbeat is JSON, not `EnvHealthV1`. `EnvHealthV1` grades a domain of the
environment (fabric, time, mesh, presence) and each domain is an arm of one
protobuf message shared by every renderer; a shim is a service, and ADR 0209
gives services the `health/shims/` key.

### 3.5 Packs and the library commit

The shim loads its formats from `bundle/`: the model modules and packs for
`cot-xml`, `cot-protobuf`, `geojson`, `canonical-track-v2` and `geo-track-v1`,
and a `MANIFEST.json` with the library commit and the SHA-256 of every file.
At start it checks every file and stops on a mismatch or on a file the
manifest does not list. The engine is a Cargo git dependency at the same
commit. `--max-distribution` defaults to `public`, which makes the engine
refuse any pack or module marked `controlled`.

### 3.6 Tests

Measured on this change: `cargo test --locked` in the crate, 16 tests pass.
`tests/routes.rs` covers the convert step for text and binary formats, fixed
context, every placeholder of a `set` template, the example route that
produces `geo-track-v1`, each start-up refusal, an altered bundle, and a
`controlled` pack at the `public` limit. `tests/fabric.rs` runs the whole path
over a Zenoh TCP transport on the loopback interface, with an in-process peer
in place of the fabric: a Cursor on Target event arrives as GeoJSON, and one
published with an attachment arrives as GeoTrackV1 carrying the key's chunks,
the receive time and the attachment's member. No test runs against a router with mTLS and an access-control list.

### 3.7 Not built

- An Ansible role, an image workflow and a registry entry. The shim runs
  nowhere.
- A schema declaration under `attest/schema/` for each published key.

## 4. goat-map

goat-map turns each Cursor on Target event into GeoTrackV1 with hand-written
code, `goatmap/cot.py`. `GOATMAP_COT_ENGINE=library` makes it use the library
instead: the Python engine decodes the event with the `cot-xml` pack and
encodes it with the `geo-track-v1` pack. The default is `builtin`, the
existing code. `/healthz` reports the value as `cot_engine`. An unknown value,
or a pack bundle that does not load, stops the service at start.

The service passes what only it knows as context: the kind (from the topic),
the source labels, the receive time, its own anchor, and the registry's
synthetic flag and organization name. The identity `<sensor>:<uid>` is derived
by the `geotrack` model module. Status messages with no position stay with
`cot.parse_status`.

### 4.1 Comparison of the two paths

Measured by `tools/cot_parity.py` over 26 Cursor on Target inputs (the ones
`tests-public/test_cot.py` and `test_cot_status.py` use, and variants of
them), and recorded in `tests-public/cot_parity_baseline.json`; a test fails
when the result changes.

Both paths accept 16 inputs and refuse 9; they disagree on one. The 16
accepted inputs give 368 members (23 each).

| Stage | Equal | Equal value, other form | null where the packs write no member | Different |
|---|---|---|---|---|
| The packs alone | 271 | 15 | 49 | 33 |
| The service's library path | 353 | 15 | 0 | 0 |

At the previous library commit (`0be9fc9`, with the files of section 5) the
packs alone gave 271, 15, 19 and 63: 30 `ce_m` and `le_m` members that were
`9999999` are now absent rather than different, and the service's path is
unchanged.

"Other form" is `course_deg` written `65` where the built-in path writes
`65.0`: one JSON number. What the packs alone leave, by cause, and the named
step of `cot_library.COMPLETION` that closes it in the service:

| Members | Count | Cause | Step | What the library needs |
|---|---|---|---|---|
| `affiliation`, `dimension` | 32 different | each is a token of the type, looked up in a table, `unknown` when missing or unlisted | `complete_type_letters` | a derive function that takes a token of a string, and a value-table entry for a value with no entry |
| `ce_m`, `le_m` | 1 different | a value above `9999999`: the built-in path says null for any value at or above it; the pack leaves only the text `9999999` absent and keeps `10000000` | `complete_error_sentinel` | a `field` option that leaves a key absent for every value at or above a stated one, or goat-map taking the library's rule (only `9999999` means not known) |
| `org`, `ce_m` and `le_m` when `9999999`, and `course_deg`, `speed_mps`, `descriptor` when the event has none | 49 absent | the `json` wire writes no member for an absent key; the map's tracks carry every member, null when not reported | `complete_null_members` | a `json` pack directive that writes `null` for an absent mapped member |

Each step has its own test, and a test removes each step in turn and checks
that only its members change.

The packs give the rest. `descriptor` is `detail/uid/@Droid`, and
`reported_at` and `stale_at` are the sensor's text as written, through the
core keys `display_name`, `time_text` and `stale_text` of the library's
`cot-xml` pack. `ce_m` and `le_m` of `9999999` are null through the pack's
`absent` option (spec 0.9) and `complete_null_members`. An event with no type gives `cot_type` `""` through a `default` line.

The one disagreement: an event with no `uid`. The built-in path publishes the
id `radar-a:`, which the schema's id pattern does not allow; the packs give
such an event no identity and refuse it.

### 4.2 Packaging

`requirements.txt` installs the engine from the library's source archive at
the pinned commit; it has no dependencies. The packs are in `xf-bundle/`
(`cot-xml`, `geo-track-v1`, and the modules `core`, `track` and `geotrack`,
all from the library) with a manifest that
`cot_library.load_engine` checks; the Dockerfile copies it to
`/app/xf-bundle`.

Measured on this change: the image builds from the Dockerfile with podman
(205 MB). In a container of it, running as uid 65532, the library path
converts an event; the service started with `GOATMAP_COT_ENGINE=library`
serves the page and reports `cot_engine` `library` on `/healthz`; with no
setting it uses `goatmap.cot`; with an unknown value it stops with the
error. The repository has no container test for goat-map, so these four
checks were run by hand.

## 5. Packs written here, now in the library

Written here and taken into the library at commit `5459b24`:

| In the library | What |
|---|---|
| `model/geotrack.def` | module `geotrack`: ten keys the republishing host supplies, and the derivation of the identity |
| `packs/geo-track-v1/` | the pack for GeoTrackV1 on the `json` wire, its notes and four vectors |
| `packs/cot-xml/` | the core keys `time_text`, `stale_text` (an event's `time` and `stale` as written) and `display_name` (`detail/uid/@Droid`), five `geo-*` vectors, and the core key `access` (`@access`) |

What was proposed as a module `cotxml` became three core keys, so the library
has no `cotxml` module and `geo-track-v1` uses `geotrack` only. The library
also decodes `9999999` in `hae`, `ce` and `le` as absent, which section 4.1
measures. `ops/xf/upstream-proposed/`, its test, and the handling in
`bundle.py` that took a file from there before the library's are deleted. On
the shim's bundle, the library's 72 conversion vectors between the five
bundled formats pass on the Python engine.

## 6. `goat xf`: converting on an operator's machine

Designed; no goat code is needed. The `goat` CLI runs any executable named
`goat-<name>` on `PATH` as `goat <name>` (`goat plugin list`). The library's
Rust command-line tool is named `goat-xf`, so an operator who installs it runs:

```bash
goat sub 'telemetry/cot/zagros/*' --raw | goat xf convert --from cot-xml --to geojson --packs <dir>
```

A built-in verb such as `goat topic convert` is not added. It would put the
engine and a pack bundle inside the `goat` binary, and it would need an entry
in the error catalog for each library error, a row in the CLI and web parity
matrix, and translations, for a command that touches no goatnet.
Not built: publishing `goat-xf` beside `goat` on a goatnet's download page.

## 7. The portal inspector

Designed, not built. The inspector (`ops/goat-zenoh-portal/internal/decoder/`
in goatcore, Go) decodes a sample when the topic's organization has uploaded a
protobuf descriptor set or an XSD. With the library's Go engine it could also
decode any topic whose declared format names a pack: `Decode(format, bytes)`
yields the model keys and the per-format residue, which the inspector renders
as a table. A topic declared `pack:cot-xml` then needs no upload.

The Go engine is standard library only, so it adds one module to the portal's
`go.mod` and no transitive dependency. Two things have a cost:

- **Load time.** The engine parses packs at start. Measured with the
  library's Go tool on a laptop: the five formats of the shim's bundle load in
  0.13 s using 6 MB; all 60 public packs (22.8 MB of `pack.def`, of which
  `isa-message`, `a-gra` and `uci` are 19.6 MB) take 1.1 s of processor time
  and 265 to 286 MB. The portal would load a named bundle, not every pack, and
  would load it on first use so that its start does not wait.
- **Which packs.** A bundle is built into the image, so the set of formats the
  inspector decodes is the same on every goatnet and changes with the image.
  A pack for a format whose specification is not public cannot be in a public
  image; the inspector would need a second, mounted bundle directory and the
  engine's `controlled` limit for a deployment that has such packs.

## 8. The library commit and the bundles

The library is used at one commit, stated in `ops/xf/goat-transformers.ref`
(`5459b24`). `ops/xf/bundle.py build` copies the chosen packs and the model
modules they use from a library checkout at that commit into a consumer's
directory and writes the manifest; every file's origin is `library`; `bundle.py check` recomputes it. Moving the commit is
one change: the `.ref` file, the `rev` in the shim's `Cargo.toml` and `Cargo.lock`,
the archive URL in goat-map's `requirements.txt`, and both bundles rebuilt.
The shim's `tests/routes.rs` and `ops/xf/tests/` fail when they disagree.
