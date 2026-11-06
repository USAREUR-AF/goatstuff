# feeds-shim — MediaMTX → Zenoh feed-catalog bridge

Bridges MediaMTX's REST API to the Zenoh control plane for the Block 35
video fabric. Polls MediaMTX `/v3/paths/list` and publishes per-feed
advertisements at `attest/queryable/feeds/<site>/<feed-id>` so the
catalog is queryable by any Zenoh peer (operators, map clients,
analytics agents).

See `docs/design/video-fabric.md`
for the architectural shape.

## Status

⏳ **v0.1 scaffold** — the catalog-publish path is
working. Control-plane subscriber paths (PTZ / record start/stop)
deferred until a real source class with PTZ capability is integrated
(per Spike C). The advertisement schema version is `1`; lock with the
data-dictionary entry `attest/schema/feeds/v1` before this ships to
production (Block 35E task).

## What it does today (v0.1)

- Polls MediaMTX `/v3/paths/list` every `--poll-sec` seconds (default 5)
- For each path: publishes a JSON advertisement at
  `attest/queryable/feeds/<site>/<path-name>` with
  `{name, site, ready, tracks: [{codec, codec_props}], inbound_bytes,
   outbound_bytes, source_type, schema_version}`
- Identity-bound via Zenoh standard authn (peer's session config)

## What's deferred

- `cmd/feeds/<id>/ptz`, `cmd/feeds/<id>/record/start`,
  `cmd/feeds/<id>/record/stop` — translate to MediaMTX API calls.
  Lands once the first source class with PTZ capability is integrated
  (Spike C output).
- `lineage/forward/feeds/<id>/<segment-id>` — emit per recorded segment.
  Lands with the Block 17D recording-integrity ledger ingestion.
- Block 27 wide events for feed lifecycle (online, offline, key-rotation).

## Running locally

```bash
cd ops/zenoh-shims/feeds-shim
cargo build --release

# Pointing at the test fixture's MediaMTX from ops/ansible/roles/mediamtx/tests
./target/release/feeds-shim \
  --mediamtx-url http://127.0.0.1:9997 \
  --site local-mac \
  --poll-sec 5
```

Output: structured JSON logs to stderr, advertisements published to the
Zenoh peer mesh via the default discovery (multicast on the local link).

For testing without a Zenoh router available, set
`FEEDS_SHIM_ZENOH_CONFIG` pointing at a config file with `mode: peer` +
`scouting: { multicast: { enabled: true } }`.

## Deployment

Future: Ansible role `ops/ansible/roles/feeds-shim/` (not yet written)
deploys this binary alongside the MediaMTX role on each video-bearing
site. Container image build follows the ADR 0708 build-from-source
pattern (`.github/workflows/feeds-shim-image.yml` — also pending).

## Schema (advertisement payload v1)

```json
{
  "name": "test1",
  "site": "vega",
  "ready": true,
  "tracks": [
    {
      "codec": "H264",
      "codec_props": {
        "width": 1280,
        "height": 720,
        "profile": "High 4:4:4 Predictive",
        "level": "3.1"
      }
    },
    {
      "codec": "MPEG-4 Audio",
      "codec_props": {
        "sampleRate": 48000,
        "channelCount": 1
      }
    }
  ],
  "inbound_bytes": 1234567890,
  "outbound_bytes": 234567890,
  "source_type": "rtspSession",
  "schema_version": 1
}
```

Schema is locked when `attest/schema/feeds/v1` is published per the
data-dictionary discipline (Block 23, [`docs/design/data-dictionary.md`](https://github.com/usareur-af/goatcore/blob/main/docs/design/data-dictionary.md)).

## Certificate verification and vendor/

The shim stays on Zenoh 1.0.4. Rustls verifies certificate paths with
rustls-webpki 0.103.16, the release that refuses the distrusted emSign
intermediates (RUSTSEC-2026-0382). Three Zenoh 1.0.4 adapter crates
(`zenoh-link-commons`, `zenoh-link-tls`, `zenoh-link-quic`) asked for
rustls-webpki 0.102, which has no patched release. They are vendored under
`vendor/` with their Rust source byte-for-byte as published; only their
manifests change, to select 0.103.16. `vendor/provenance.json` holds the
registry archive checksums, the upstream revision and a hash of every file;
the upstream licence and notice texts are kept.

Checks, run from this directory (CI runs the same in `feeds-shim.yml`):

```sh
python3 tests/check_vendor_provenance.py --self-test
python3 tests/check_vendor_provenance.py
cargo test --locked
python3 tests/transport_compatibility.py
cargo clippy --locked --all-targets -- -D warnings
```

`cargo test` runs the Zenoh any-name verifier against a valid, an untrusted
and a distrusted certificate. `transport_compatibility.py` builds an upstream
Zenoh 1.0.4 router without the patches and runs ten TLS and QUIC cells against
the real shim: name checking, compressed payload integrity, wrong-name and
unknown-issuer refusal, and a missing client certificate.

## License

Apache-2.0.
