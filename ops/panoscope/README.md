# panoscope

Live visualization of the nova Zenoh fabric. Read-only observer that
renders peers as nodes, topic flows as animated edges with schema-color
coding, role classification (source / sink / intermediary), and packet-dot
animation per sampled publish.

- **View-model design:** [panoscope view model](../../docs/design/panoscope-view-model.md)
- **History-lens design:** [panoscope history lens](../../docs/design/panoscope-history-lens.md)

## Status

v0.0.1-alpha — backend complete; frontend + deployment pending:

- 82Q.1 Cargo scaffold ✅
- 82Q.2 Zenoh client + topology / flow model + admin + data tails ✅
- 82Q.3 axum HTTP + WebSocket (full + /legend) ✅
- 82Q.4 Schema resolver with portal API + embedded fallback ✅
- 82Q.5 D3 frontend (plain JavaScript modules, no build step) ✅
- 82Q.6 rust-embed bundling ✅
- 82Q.7 docker-compose + Ansible + Traefik wiring ✅
- 82Q.8 Smoke deploy on nova-a — pending (operator-fired)
- 82Q.9 Operator runbook ✅ at [`docs/operations/panoscope-runbook.md`](../../docs/operations/panoscope-runbook.md)

## Modules

- [`src/main.rs`](src/main.rs) — tokio runtime, axum router, route handlers
- [`src/config.rs`](src/config.rs) — env-var-driven Config + WS-tick default
- [`src/model.rs`](src/model.rs) — pure-data types: Topology, FlowState,
  PeerInfo, EdgeState, EMA decay, TokenBucket, role classification
  heuristic. Zenoh-agnostic; unit tests cover the math.
- [`src/schema.rs`](src/schema.rs) — `SchemaResolver` trait +
  `EmbeddedFallbackResolver` (compiled-in prefix map) +
  `PortalSchemaResolver` (HTTP GET against the portal `/api/schemas`
  endpoint with background-task refresh) + `CompositeResolver` (portal
  first, embedded fallback). `ResolverStatus` (portal-live / stale /
  embedded-fallback) surfaces on `/api/legend`.
- [`src/snapshot_view.rs`](src/snapshot_view.rs) — wire shape (NodeView,
  EdgeView) shared by `GET /api/snapshot` and the WS initial-message.
- [`src/zenoh_client.rs`](src/zenoh_client.rs) — Zenoh session open +
  background data-tail (sampled `**` subscribe through per-key
  TokenBuckets, publisher peer + topic recorded into FlowState with
  EMA decay) + admin-tail (`@/**` subscribe, parses peer ZID from key
  path for supplementary peer-existence signal). Emits `WsEvent`s to a
  `tokio::sync::broadcast` channel for live WS fanout. Shared state
  via `Arc<RwLock<...>>`.
- [`src/ws.rs`](src/ws.rs) — `/ws` handler. On connect, sends initial
  `snapshot` message; then streams `delta` messages on a 100ms
  coalescing tick (configurable via `PANOSCOPE_WS_COALESCE_MS`). Lossy
  broadcast — slow consumers receive a `lagged` warning and are
  expected to fetch `/api/snapshot` to resync.
- [`frontend/`](frontend/) — the web pages: plain HTML, CSS and
  JavaScript modules in `frontend/web/`, with d3 v7 as one vendored
  file. Force-directed graph with role-color nodes, schema-color edges,
  packet-dot animation per sampled publish, EMA-thickness edge
  encoding, bottom-strip legend panel. No build step; npm is used only
  for the Playwright tests.
- [`src/embed.rs`](src/embed.rs) + [`build.rs`](build.rs) — rust-embed
  inlines `frontend/web/` into the Rust binary at compile time; build.rs
  makes cargo rebuild when a file there changes. axum `.fallback()`
  serves the pages — `/` returns index.html, `/src/<file>` returns the
  file with the right Content-Type, unknown paths fall back to
  index.html for client-side routing.
- [`Dockerfile`](Dockerfile) + [`docker-compose.dev.yml`](docker-compose.dev.yml)
  — multi-stage container build (rust-build stage for the binary,
  with `frontend/web/` embedded → distroless runtime-cc; bases by role in `ops/base-images/`) and a local-dev compose that pairs panoscope
  with an upstream `eclipse/zenoh:1.9.0` router on the loopback. Run
  `docker compose -f docker-compose.dev.yml up --build` to see
  panoscope render against synthetic traffic.
- Ansible role at [`ops/ansible/roles/panoscope/`](../ansible/roles/panoscope/);
  orchestrator playbook at [`ops/ansible/panoscope.yml`](../ansible/panoscope.yml).

## Endpoints

| Path | Description |
|---|---|
| `GET /healthz` | Returns `{"status":"ok"}` once the zenoh session opens. |
| `GET /version` | Returns crate name + version. |
| `GET /api/stats` | Counters: data + admin samples seen / recorded / dropped + last-sample timestamps. |
| `GET /api/snapshot` | Full snapshot: nodes (peers with role + pub/sub sets) + edges (with projected EPS decayed to query time) + sampling drops. Cold-load surface for the frontend. |
| `GET /api/legend` | Schema-resolver status + sampling-drop counts + EMA tau + WS coalesce tick. The bottom-strip legend panel reads this. |
| `WS /ws` | Live stream — initial `snapshot` then coalesced `delta` messages. See `src/ws.rs` for the message-type tags. |

## Build

```bash
cd ops/panoscope
cargo build --release
```

## Run

```bash
PANOSCOPE_BIND=0.0.0.0:8080 \
PANOSCOPE_ZENOH_ENDPOINT=tcp/127.0.0.1:7447 \
PANOSCOPE_PORTAL_SCHEMAS_URL=https://portal.nova.netbird.nova.example/api/schemas \
cargo run --release
```

Then:

```bash
curl http://localhost:8080/healthz
curl http://localhost:8080/version
```

## Configuration

All config via env vars; see [`src/config.rs`](src/config.rs) for the
field list and defaults.

## License

Apache-2.0.
