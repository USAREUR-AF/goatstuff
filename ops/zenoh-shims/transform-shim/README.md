# transform-shim: convert messages between formats on the fabric

A fabric peer that subscribes to keys carrying one message format, converts
each message with the [goat-transformers](https://github.com/USAREUR-AF/goat-transformers)
library, and republishes it under another key. Which keys, which formats and
where the result goes is a route table read from one file at start.

Design: [docs/design/format-transformers.md](../../../docs/design/format-transformers.md).

**Status:** built and tested in CI. Not deployed: no Ansible role, no image
workflow and no goatnet runs it.

## Route table

```json
{
  "instance": "xf-1",
  "site": "zagros",
  "key_prefix": "",
  "routes": [
    {
      "name": "cot-to-geojson",
      "subscribe": "telemetry/cot/zagros/*",
      "from": "cot-xml",
      "to": "geojson",
      "strip_prefix": "telemetry/cot/",
      "publish_prefix": "data/entity/observation/"
    }
  ]
}
```

| Member | Meaning |
|---|---|
| `instance`, `site` | name this instance in its announcement and heartbeat |
| `key_prefix` | put in front of the announcement and heartbeat keys; `<anchor>/` on a fabric where a peer writes only under its own anchor |
| `subscribe` | key expression to subscribe to; begins with `strip_prefix` |
| `from`, `to` | pack names from the bundle |
| `strip_prefix`, `publish_prefix` | literal prefixes ending in `/`; the published key is `publish_prefix` followed by the received key with `strip_prefix` removed |
| `encoding` | Zenoh encoding of the output; default by the wire of `to`, with the pack name as the schema part (`application/geo+json;geojson`) |
| `set` | model keys supplied to every conversion of the route (the library's context): fixed text, or a template that takes values from the arrival of the message, below |

A message on `telemetry/cot/zagros/4411` is published on
`data/entity/observation/zagros/4411`. A format on the `bits` or `protobuf`
wire is read and written as bytes.

### Context from the arrival

A `set` value may hold placeholders. Each is replaced, per message, by
something the receiving peer knows and the payload does not say.

| Placeholder | Value |
|---|---|
| `{received_at}`, `{received_ms}` | when the shim received the message: `YYYY-MM-DDTHH:MM:SS.fffZ`, or epoch milliseconds |
| `{timestamp_at}`, `{timestamp_ms}` | the sample's own Zenoh timestamp, in the same two forms |
| `{key}` | the key the message was published on |
| `{key.N}` | chunk N of that key: `0` is the first, `-1` the last |
| `{capture.N}` | what wildcard N of `subscribe` matched, from 1; the chunks a `**` matched are joined with `/` |
| `{tail}` | the key with `strip_prefix` removed |
| `{encoding}` | the sample's Zenoh encoding |
| `{attachment}` | the sample's attachment, as UTF-8 text |
| `{attachment.NAME}` | member NAME of an attachment that is a JSON object |

`{{` and `}}` write a brace. A placeholder with no value for a message (no
attachment, no timestamp, a shorter key) leaves that model key out of the
context: the target format's default applies, or a kind that requires the key
refuses the message and the refusal is counted. A value of the wrong type for
its key refuses that message the same way.

The route `cot-to-geo-track` of `config.example.json` produces GeoTrackV1 this
way: a message on `telemetry/sensor/radar-a/cot/track` gives a track with
`source.sensor` `radar-a`, `kind` `track`, `id` `radar-a:<uid>` and
`observed_at` the receive time, on `data/geo/track/v1/radar-a/cot/track`.

```json
"set": {
  "geotrack.kind": "{key.-1}",
  "geotrack.sensor": "{capture.1}",
  "geotrack.site": "zagros",
  "geotrack.node": "xf-1",
  "geotrack.observed_at": "{received_at}",
  "geotrack.producer_anchor": "00112233445566778899aabbccddeeff",
  "geotrack.synthetic": "0",
  "geotrack.org": "{attachment.org}"
}
```

The shim refuses to start on: a format the bundle does not hold, an unknown
member, a `set` key no model module declares, a fixed `set` value of the wrong
type, a placeholder it does not know, a `{capture.N}` beyond the wildcards of
`subscribe`, a prefix that is not literal, a
subscription that names the whole keyspace, and any route whose output another
route (or itself) would receive.

## Running

```bash
cd ops/zenoh-shims/transform-shim
cargo build --release
./target/release/transform-shim --config config.example.json --bundle bundle \
  --endpoint tcp/127.0.0.1:7447 --check          # load, check the table, exit
./target/release/transform-shim --config config.example.json --bundle bundle \
  --endpoint tls/<router>:7447 --cert <leaf.pem> --key <leaf-key.pem> --root-ca <ca.pem>
```

Every option has an environment variable, `TRANSFORM_SHIM_<OPTION>`.
`--max-distribution` is `public` by default: the engine then loads no pack or
model module marked `controlled`.

## What it publishes besides converted messages

| Key | When | Payload |
|---|---|---|
| `<key_prefix>services/shims/transform/<instance>/announce` | once at start | JSON: class, version, instance, site, `translates_from`, `translates_to`, `config_digest` (SHA-256 of the route table), `library_rev` |
| `<key_prefix>health/shims/transform/<instance>` | every `--heartbeat-s` seconds (30) | JSON: `state` (`ok` or `degraded`), and per route `received`, `converted`, `refused`, `dropped`, `publish_failed`, `last_error` |

`refused` counts messages the library would not convert. `dropped` counts
messages that arrived while the queue of `--queue-depth` (1024) was full.
`state` is `degraded` when any of the three fault counters rose since the
previous heartbeat. When the heartbeat itself cannot be published the process
exits non-zero.

## The pack bundle

`bundle/` holds the model modules and packs the shim loads, with
`MANIFEST.json` naming the library commit and the SHA-256 of every file. The
shim checks every file at start and stops on a mismatch or an unlisted file.
To change the formats:

```bash
python3 ops/xf/bundle.py build --library <goat-transformers checkout> \
  --out ops/zenoh-shims/transform-shim/bundle --formats cot-xml,cot-protobuf,geojson,canonical-track-v2,geo-track-v1
```

The library commit is stated in `ops/xf/goat-transformers.ref`, in
`Cargo.toml` and in the manifest; `tests/routes.rs` fails when they differ.

## Tests

```bash
cargo test --locked
cargo clippy --locked --all-targets -- -D warnings
```

`tests/routes.rs` covers the convert step, every placeholder, the example
route and every start-up refusal without a fabric session. `tests/fabric.rs`
runs the whole path over a Zenoh TCP transport on the loopback interface: an
in-process peer stands in for the fabric, the shim connects as a client, a
published Cursor on Target event arrives as GeoJSON under the declared key,
and one published with an attachment arrives as GeoTrackV1. No test runs against a router with
mTLS and an access-control list.

## License

Apache-2.0.
