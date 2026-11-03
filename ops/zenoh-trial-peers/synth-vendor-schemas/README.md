# synth-vendor-schemas

Canonical `.proto` data definitions for the seven topics that
`ops/zenoh-trial-peers/synth-vendor` publishes on the nova trial
fabric. Companion to [`docs/design/portal-schema-publishing.md`](https://github.com/usareur-af/goatcore/blob/main/docs/design/portal-schema-publishing.md)
and [`docs/design/zenoh-trial-fabric-synth-vendors.md`](../../../docs/design/zenoh-trial-fabric-synth-vendors.md).

These schemas are **canon** for the trial fabric — when a real vendor
asks "what does a published-data definition look like in the goat
fabric?", this directory is the worked example. The proto files
describe the wire-level field semantics; the producer side
([`../synth-vendor/src/main.rs`](../synth-vendor/src/main.rs)) emits
them as JSON or CBOR (see the wire-format column in
[`topic-manifest.tsv`](topic-manifest.tsv)). The same `.proto` would
also describe a producer that emits protobuf-wire bytes — the
data dictionary is independent of the serialization on the wire.

## Layout

```
synth-vendor-schemas/
├── proto/
│   ├── caracal_tracks_demo_entity_v1.proto
│   ├── caracal_power_demo_site_nova_v1.proto
│   ├── community_beacon_demo_hello_v1.proto
│   ├── homemade_sensors_garage_temperature_v1.proto
│   ├── homemade_sensors_garage_door_state_v1.proto
│   ├── homemade_sensors_yard_soil_moisture_v1.proto
│   └── pangolin_observer_heartbeat_v1.proto
├── topic-manifest.tsv     # topic → proto → message-name → wire-format
├── build-fds.sh           # protoc → out/*.fds
├── upload-all.sh          # POST .fds to live portal (idempotent)
├── delete-ghost-topics.sh # remove the 3 stale ghost-topic entries
└── out/                   # generated FileDescriptorSet bodies (gitignored)
```

## Topics in scope

| Vendor | Key pattern | Message | Wire format |
|---|---|---|---|
| caracal | `caracal/tracks-demo/entity/v1` | `caracal.tracks_demo.v1.Entity` | json |
| caracal | `caracal/power-demo/site-nova/v1` | `caracal.power_demo.v1.PowerState` | json |
| community | `community/beacon-demo/hello/v1` | `community.beacon_demo.v1.Hello` | json |
| homemade-sensors | `homemade-sensors/garage/temperature/v1` | `homemade_sensors.garage.v1.TemperatureReading` | json |
| homemade-sensors | `homemade-sensors/garage/door-state/v1` | `homemade_sensors.garage.v1.DoorState` | json |
| homemade-sensors | `homemade-sensors/yard/soil-moisture/v1` | `homemade_sensors.yard.v1.SoilMoisture` | cbor |
| pangolin | `pangolin/observer/heartbeat/v1` | `pangolin.observer.v1.Heartbeat` | json |

Source of truth for this list is `topic-manifest.tsv`; the above
table is for orientation. The `synth-vendor` Rust producer's
`Personality` enum + per-personality `render_*` functions define the
exact wire shape.

## Building + uploading

```bash
# 1. Compile every .proto to a FileDescriptorSet (.fds) under out/
./build-fds.sh

# 2. Reconcile against the live portal. Idempotent: skips topics whose
#    v1 schema body already matches by sha256, leaves a SKIP marker
#    when content differs, surfaces 404s for topics that aren't
#    registered yet.
export PORTAL_URL=https://portal.nova.netbird.nova.example
export PORTAL_API_KEY="$NOVA_PORTAL_OPERATOR_KEY"
./upload-all.sh
```

`build-fds.sh` invokes `protoc --include_imports --include_source_info`
so the output FileDescriptorSet carries enough metadata for the portal's
`/api/schemas/render/{key...}` endpoint to walk the descriptor and emit
the structured tree the `schema.html` viewer renders. Comments in the
`.proto` source are NOT preserved in the descriptor by default — they
live here in-repo for human readers.

Post-F-158 (2026-05-24), single-file
`.proto` uploads with only `google/protobuf/*` imports no longer need
`build-fds.sh` on the vendor side — the portal compiles in-process via
`bufbuild/protocompile`. This directory's multi-file shape still uses
the script because some schemas import sibling files; the script
remains the canonical multi-file recipe.

## One-time ghost-topic cleanup

Before this directory landed, a prior session uploaded schemas under
invented topic names (`caracal/tracks/aircraft/v1`,
`community/weather/station/v1`, `pangolin/inspections/asset/v1`) that
no producer publishes to. Once on each portal where those ghosts
exist, run:

```bash
./delete-ghost-topics.sh
```

After cleanup, `upload-all.sh` populates the four topics that don't
yet have schemas and confirms the one (`homemade-sensors/garage/
temperature/v1`) whose previous upload happened to land on a real
topic — see `topic-manifest.tsv`.

## Wire-format vs. data-dictionary

The synth-vendor publishes JSON (and CBOR for one personality) on the
wire — the topic's `format` field on the portal stays as `json` or
`cbor`. Attaching a protobuf `.fds` does NOT change the wire format;
it only attaches a structured data-dictionary so:

- Operators can browse field semantics via the portal schema viewer
  (`/schema.html?key=<topic>`) — Swagger-style rendering of the
  message tree.
- Consumer code can fetch the descriptor at `/api/schemas/<key>` and
  decode either the JSON (using field-name matching) or future
  protobuf-wire payloads (using `dynamicpb`).

If a producer migrates to protobuf wire format, the operator can flip
`format` to `protobuf:<MsgName>` (`./upload-all.sh --set-format`) and
the inspector's protobuf-decode path lights up automatically.

## Adding a new schema

1. Drop a new `.proto` under `proto/` with a `package
   <vendor>.<system>.v<N>;` line matching the topic.
2. Append a row to `topic-manifest.tsv`.
3. Register the topic via the portal's Topics page (or POST to
   `/api/vendors/<vendor>/topics`) so the upload script has a target.
4. Run `./build-fds.sh && ./upload-all.sh`.
