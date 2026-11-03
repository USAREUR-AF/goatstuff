# trashgoat

<img src="assets/trashgoat-icon.png" alt="trashgoat — a wide-eyed goat in a bubble, surrounded by floating trash and a plastic bag" width="160" align="right" />

**Operator-controlled chaos publisher for pub/sub fabrics.**

trashgoat is a small Rust service that lets an operator generate ad-hoc,
deliberately-suspect traffic on a pub/sub fabric — large payloads,
malformed wire formats, intentional rate spikes, chunked-vs-singleton
behaviour — and watch what the fabric, the storage backend, and the
consumers actually do. It is **chaos engineering for pub/sub**.

---

## Why this exists

If you operate a pub/sub fabric (Zenoh, NATS, MQTT, Kafka, anything
with a publish-side and a storage-side), you eventually hit shapes
the docs don't quite cover. A 12.5 MB sample arrives on the wire,
appears to publish cleanly, then never lands in storage. A subscriber
mints a fresh UUID in every published key path and another subscriber
declares on a literal key, sees nothing, files a bug against the wrong
component. A burst of 1,000 publishes spaced 50 ms apart melts a
queue you didn't know had a queue.

You can either:

- Write a one-shot Python script every time you need to reproduce one
  of these shapes, OR
- Have a small service that knows about the common chaos shapes,
  takes a declarative spec from an operator UI, fires the traffic on
  demand, and cleanly retracts every key it published when the
  experiment is done.

trashgoat is the second.

## What trashgoat is NOT

- **Not a load-test tool.** k6 / wrk / drill / siege already do
  steady-state load against HTTP APIs better than trashgoat ever
  will. trashgoat is built for *unusual shapes*, not high rates.
  (There is a hard rate ceiling — default 100 Hz — that the runner
  enforces against every spec.)
- **Not a fuzzer.** trashgoat generates declared shapes; it does not
  evolve inputs against coverage. If you want AFL/libFuzzer-style
  evolution, point one of those at your subscriber directly.
- **Not a production system.** Anything trashgoat publishes is
  *suspect by construction*. Consumers must never depend on its
  output. The name is deliberately undignified.

## Concepts

A **spec** is the unit of configuration. Each spec has four parts:

| Part | What it controls |
|---|---|
| `slug` | Url-safe identifier; second segment of every published key |
| `topic_pattern` | The key under `trashgoat/<slug>/...`; can carry `{seq}`, `{uuid}`, `{ts_ms}` templates |
| `payload_spec` | What bytes to publish (bytes / chunked / json_shape / protobuf_shape) |
| `cadence_spec` | When to publish (oneshot / every / burst) |

Specs walk through four states:

```
draft ──activate──▶ active ◀──resume── paused
                       │                  ▲
                       └─────pause────────┘
                       │
                       └────retire────────▶ retired (terminal)
```

Plus an orthogonal **cleanse** action that issues `z_delete` against
every key the spec ever published. Cleanse works from any state except
`draft` and leaves the spec's lifecycle state unchanged.

## Payload shapes (v1)

```jsonc
// bytes — single publish of N bytes with configurable content
{ "kind": "bytes", "size_bytes": 12500000, "content": "random" }

// chunked — emits N per-chunk publishes + a manifest publish
//          (Pattern E from pub/sub best-practices: "chunk the payload,
//          give consumers a manifest to reassemble")
{ "kind": "chunked", "total_size_bytes": 12500000, "chunk_size_bytes": 1500000, "content": "random" }

// json_shape — for testing decoder failures + schema-evolution
//             $uuid / $now_ms / $seq are substituted at publish time
{ "kind": "json_shape", "template": { "id": "$uuid", "ts": "$now_ms" } }

// protobuf_shape — v1 ships a JSON-envelope stub; full impl will
//                  consume a FileDescriptorSet from a descriptor
//                  cache (e.g. shared with panoscope)
{ "kind": "protobuf_shape", "message_type": "vendor.pkg.v1.Msg", "fields": {} }
```

`content` for `bytes` / `chunked`: `"random"` / `"zeros"` /
`"repeating:<pattern>"`.

## Cadence shapes (v1)

```jsonc
{ "kind": "oneshot" }
{ "kind": "every",  "interval_ms": 1000 }
{ "kind": "burst",  "count": 10, "spacing_ms": 50, "then_idle": true }
```

The runner enforces a max-Hz ceiling. Cadences that would exceed it
get clamped, with a warning logged.

## How it works (the 60-second tour)

```
                       operator
                          │
                          ▼
                ┌──────────────────┐
                │   portal UI /    │ creates spec → SQLite
                │   /admin/trash…  │ activate/pause → emits control event
                └──────────────────┘
                          │
                publish on `ops/trashgoat/control/<id>/v1`
                          │
                          ▼
                ┌──────────────────┐         publishes on
                │  goat-trashgoat  │ ─────────────────────────────▶ subscribers
                │  (this binary)   │  `trashgoat/<slug>/...`        + storage
                └──────────────────┘                                + audit
                          │
                          │  on cleanse:
                          ▼
                z_delete on every key the spec ever touched
```

Two namespaces, two ACL rules:

- **Data**: `trashgoat/**` — the runner has WRITE here, nothing else.
- **Control**: `ops/trashgoat/control/**` — the runner has READ
  here, the portal has WRITE.

The runner's identity is one offline-CA-minted X.509 leaf with
URI-SAN `<authority>:vendor=trashgoat` (or equivalent for your
fabric's identity model).

## Quick start (local dev)

```bash
# 1. Build
cargo build --release

# 2. Run against a local Zenoh router (mTLS off for dev)
./target/release/goat-trashgoat \
  --endpoint tcp/127.0.0.1:7447 \
  --control-key 'ops/trashgoat/control/**'

# 3. From another shell, publish a control event simulating activate:
zenoh-rest put ops/trashgoat/control/1/v1 '{
  "spec_id": 1,
  "spec": {
    "id": 1,
    "slug": "first-shot",
    "topic_pattern": "trashgoat/first-shot/v1",
    "payload_spec": {"kind":"bytes","size_bytes":1024,"content":"random"},
    "cadence_spec": {"kind":"oneshot"},
    "state": "active"
  },
  "command": "activate"
}'

# 4. Watch trashgoat/first-shot/v1 in any subscriber.
```

For production use the operator UI + the portal substrate it expects
(see the parent project's docs for the portal piece).

## Operational discipline

Each runner instance is **single-vendor** — it publishes only under
its own URI-SAN-bound namespace. No impersonation.

trashgoat keeps a bounded per-spec key-history (default 10 000 keys)
for cleanse bookkeeping. Specs that template `{uuid}` into the topic
path can produce unbounded distinct keys; the oldest entries fall off
the tail when the cap is hit, and cleanse logs a warning that some
keys are no longer tracked.

Logs are tracing-style; default filter `goat_trashgoat=info,zenoh=warn`.
Reasonable shape for production: ship logs to your existing
audit/observability substrate via the runner's stdout.

## Phased rollout status

| PR | Scope | Status |
|---|---|---|
| #1 | Design doc + namespace + lifecycle + control wire contract | Done |
| #2 | Portal-side SQLite spec store + CRUD/lifecycle API + UI + control-topic publish | Done |
| #3 | This binary — runner + cadence + payload + ansible role + CI workflow | Done |
| #4 (future) | Structured form editors for payload/cadence + per-spec stats + protobuf encoding via real descriptor set | Backlog |

## Extraction status

trashgoat currently lives inside the goat monorepo at
`ops/goat-trashgoat/`. The intent is to lift it out into its own
repository (`goat-maintainer/goat-trashgoat`) as a standalone chaos-engineering
resource that any pub/sub fabric operator can pick up. The crate is
already its own Cargo workspace (`[workspace] resolver = "2"` at the
top of `Cargo.toml`), and this directory carries everything the
extracted repo will need:

```
ops/goat-trashgoat/
├── Cargo.toml      # standalone workspace
├── Cargo.lock      # pinned dependencies
├── Dockerfile      # Chainguard rust → glibc-dynamic distroless
├── LICENSE         # Apache-2.0
├── README.md       # this file
├── assets/
│   └── trashgoat-icon.svg
└── src/
    ├── main.rs       # entry + Zenoh session setup
    ├── control.rs    # control-topic dispatcher
    ├── runner.rs     # in-memory spec table + per-spec publisher tasks
    ├── payload.rs    # 4 payload generators
    └── cadence.rs    # 3 cadence schedules
```

The companion deploy substrate (Ansible role + apply playbook + CI
image workflow) lives in the parent repo. On extraction, the
Ansible role + CI workflow would lift alongside this directory.

The pub/sub fabric is currently Zenoh-specific. A clean extraction
would also introduce a small fabric trait (open session, publish
bytes, delete key, subscribe to control topic) and ship Zenoh as the
first implementation — keeping the door open for NATS, MQTT, Kafka,
etc. without rewriting the spec model.

## License

Apache-2.0. See [LICENSE](LICENSE).

## Project home

[github.com/usareur-af/goatcore](https://github.com/usareur-af/goatcore) — main repo while trashgoat is incubated.
