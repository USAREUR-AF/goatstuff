# flashlight — surgical inspection into the goat data fabric

| Field | Value |
|---|---|
| Status | Authored 2026-05-24. Phase A (planning), Phase B (`panoscope-core` factor-out, PR #522), Phase C (Tier 1 deployable surface, PRs #533/#536/#538/#543/#547), and Phase D (four v1 capability impls in `flashlight-analytics` + `wasm-bindgen-test` runtime equivalence + `POST /api/compute`) all landed 2026-05-27 → 2026-05-28. Phase E in progress: PR #598 (WASM build pipeline + `GET /api/wasm/{filename}` serving), PR #609 (Tier 2 boot logic + first `[T2] rate` badge), PR #618 (anomaly + schema-diff badges), this PR (Tier 2 sparkline canvas — second Canvas 2D rendering `SparklineSnapshot.points`, default 5 min × 60 bins, hidden until snapshot lands). Remaining Phase E: BroadcastChannel ownership, explicit operator opt-in button + settings panel, decoded payload viewer (row-click modal pulling FDS from the portal + calling `decode_payload_json`), Playwright tests for Tier-1-only vs Tier-2-loaded modes. Phase C live-fire on nova-a is gated only on the operator firing `flashlight.yml`. **Phase F — inspection-and-navigation rebuild (track `flashlight-payload-nav`, 2026-05-31 → 2026-06-01):** a first-use review found flashlight surfaced rate/key/byte-count but never the actual payload values (its stated reason to exist), three of eight URL filters were no-ops or silently widened to `**`, and the T2 chips ran on absent/synthetic inputs. Phase F closes the gap: (F-1) `SampleStore` + `GET /api/sample/{seq}` + raw row-click viewer + format-icon column — the live wire stays byte-free, bytes are fetched on demand *(merged, PR #699)*; (F-2) directory landing + header jump box (`GET /api/directory`) for navigate-anywhere across topics + vendors/groups *(merged, PR #699)*; (F-3a) the dead T2 chips removed — z-score + drift, `rate` + sparkline kept *(merged, PR #705)*; (F-3b) the `role`/`site`/`format` post-filters now actually filter — flashlight hydrates a local peer-topology from the schema-decls on its broad subscription (`schema_decl.rs`, mirroring panoscope's field handling) and the SSE stream drops events whose publisher doesn't satisfy the filter (`subscription::post_filters_pass`); an event with no attributed publisher is dropped while a post-filter is active, the correct behavior vs. the prior silent widen-to-`**` *(this change-set)*. (F-3c) role-based browse — `/api/directory` carries a topology-derived `peers` array and the landing renders Sources/Derivers/Sinks cards, each key one-click into a focused tail *(this change-set)*. (F-3d, merged #720, deployed `sha-caa8c73`) payload-viewer redesign + decode — the viewer is a panel **pinned at the top** that **auto-follows the newest event while running and freezes to the clicked row when paused**. (F-3e, this change-set) live-fire fixes: the viewer now shows the **deserialized message AND the serialized bytes together** (decoded pane over raw-hex pane, not an either/or toggle); the declared `protobuf:<Name>` is resolved from the **portal registry** (`find_topic_by_key`) rather than the schema-decl topology (synth vendors don't put `format` in their decls, so the first deploy showed hex for decodable topics); and **`/` focuses the jump box** like panoscope. (Optional later: factor the schema-decl parser into `panoscope-core`.) (F-3f, 2026-06-12, F-195) decode parity with panoscope — `decode_payload` required a fully-qualified message name (`get_message_by_name`), but topics declare a bare `protobuf:Entity` while the FDS packages it (`notional.tracks_demo.v1.Entity`), so the lookup missed and every packaged proto fell to the numbered wire view; added the tier-2 bare-leaf-name fallback (`all_messages().find(|m| m.name() == name)`) mirroring `panoscope::decoder::select_message`, panoscope's tier-3 first-message guess deliberately omitted to keep flashlight's honest wire fallback. **Deployed + live-fired on ZAGROS 2026-06-12 (`sha-37ed6bf`, M-059): the live tarsier `Entity` topic decodes to named fields (`entityId`/`lat`/`lon`/`kind`/…).** The deploy also surfaced F-196 — flashlight had no retry on the initial zenoh connect, so a boot-race left it silently HTTP-only. (F-3g, 2026-06-12, F-196) durable fix — `AppState.events` is now a reconnectable `Arc<RwLock<Option<Arc<Sender>>>>` slot filled by a background task that retries `zenoh::open` with exponential backoff (1→30 s) until the subscription establishes (boot/reboot race self-heals, no manual restart, HTTP serving throughout), and `/healthz` returns 503 when an endpoint is configured but the subscription isn't established (was unconditional `"ok"`) so isitup stops false-greening an HTTP-only flashlight. **Deployed + live-fired on ZAGROS 2026-06-12 (`sha-44bc32c`, M-060): forced the race, `/healthz=503` while cut off, self-healed to 200 with no manual restart.** |
| Scope | Focus-oriented real-time inspection surface for the goat Zenoh fabric. URL declares the focus; server narrows the subscription; only the focused slice flows. Sibling-of-panoscope: panoscope visualizes the *whole* ecosystem; flashlight inspects *one slice* of it under degraded network and/or degraded compute. |
| Related | [`minimalism-under-network-constraints.md`](https://github.com/usareur-af/goatdocs/blob/main/docs/design/minimalism-under-network-constraints.md) (the web doctrine flashlight instantiates), [ADR 0863](https://github.com/usareur-af/goatcore/blob/main/docs/adr/0863-resilient-end-node-design-philosophy.md) (parent philosophy — cross-cutting end-node principles), [ADR 0861](https://github.com/usareur-af/goatcore/blob/main/docs/adr/0861-minimalism-under-network-constraints.md) (web instantiation — doctrine adoption decision record), [ADR 0862](https://github.com/usareur-af/goatcore/blob/main/docs/adr/0862-two-tier-web-architecture-canonical-coprocessor.md) (Tier-1-canonical / Tier-2-coprocessor architectural decision), `panoscope.md` (the workhorse sibling this complements), [`zenoh-trial-fabric.md`](https://github.com/usareur-af/goatcore/blob/main/docs/design/zenoh-trial-fabric.md), [`portal-schema-publishing.md`](https://github.com/usareur-af/goatcore/blob/main/docs/design/portal-schema-publishing.md), [ADR 0112](https://github.com/usareur-af/goatcore/blob/main/docs/adr/0112-tech-strategy-workload-shape-language-boundary.md) (Rust workload-shape rule), [ADR 0859](https://github.com/usareur-af/goatcore/blob/main/docs/adr/0859-easy-rollback-engineering-principle.md) (deploy-history ledger). |

## Doctrine adoption

This surface adopts [`minimalism-under-network-constraints.md`](https://github.com/usareur-af/goatdocs/blob/main/docs/design/minimalism-under-network-constraints.md). No deviations from the 14 principles, conscious rejections, or three locked-in decisions. flashlight is the first instance from which the doctrine was extracted; reading the doctrine and then this doc gives the worked example.

## What this is

> **Display lexicon (2026-06-17):** flashlight's participant-noun wording is goatnet-appropriate — "vendor" on nova-a, "org" on ZAGROS (jump box, directory prompt/heading, empty-state, counts). `render::Lexicon` substitutes server-side strings + emits a `window.GOAT_LEX` global the inlined `app.js` reads; `FLASHLIGHT_LEX_ORG` is wired by the role from the same `portal_lex_org` group_var the portal reads. Display-only — the `?vendor=` deep-link param, `vendor`/`key` filter kind, and portal `list_vendors` call are untouched. See [`portal-lexicon.md`](https://github.com/usareur-af/goatcore/blob/main/docs/design/portal-lexicon.md) §"Flashlight + stethoscope display layer".

A web service at `flashlight.nova.netbird.goat.example` (nova v1; prod hosts add as sibling siblings) that does one thing well: **let an operator point at a slice of the Zenoh fabric and watch what's happening there, with maximum efficiency under any network and compute condition.**

The slice is declared in the URL — `?key=vendor-b/observations/v1`, `?vendor=vendor-a`, `?role=sensor&site=alpha`, `?since=10m` — and the server narrows its Zenoh subscription to match. Only the focused slice flows over the wire. The page loads instantly with Tier 1 (HTML + Server-Sent Events + ~20 KB vanilla JS) and is immediately useful. Tier 2 (Rust→WASM analytics module) lazy-loads when conditions allow and adds protobuf decode, sliding-window statistics, schema diff, and other compute-heavy capabilities — but never blocks Tier 1, never replaces a Tier 1 surface, and gracefully evicts when conditions degrade.

The name *flashlight* matches the metaphor: a focused, hand-held tool you point at a specific thing. Panoscope is the floodlight that shows the whole room; flashlight is the focused beam you use to inspect a corner.

## Complementarity with panoscope

flashlight is not a replacement for panoscope and not a degraded panoscope. They serve distinct operator questions:

| Panoscope answers | Flashlight answers |
|---|---|
| *What's the shape of the fabric right now?* | *What's happening at this specific topic / vendor / role / site?* |
| *Who's publishing? Who's subscribing? What's flowing?* | *What are the actual sample values? How is the schema being used? What's the rate over the last 10 minutes?* |
| *Where are the bottlenecks / gaps / surprises?* | *Given that one peer / topic looks anomalous, what is it actually doing?* |
| Full-fabric snapshot, all 22+ peers, all topics. | Focused slice, declared in URL, server-side narrowed at the subscription. |
| Heavier viz (D3 force layout, hull rendering, animation). | Lean text + sparkline + optional analytics widgets. |

The two surfaces link to each other:

- **panoscope → flashlight:** per-edge "inspect this slice ↗" button on every topic-broker pin and every selected peer/topic in panoscope. The link builds the flashlight URL (`?key=...` or `?vendor=...`) from the panoscope selection. Operator drills from overview to focused inspection in one click.
- **flashlight → panoscope:** "panoscope ↗" link in the flashlight header. Operator zooms back out to context in one click. Hover-title carries the one-sentence explainer.
- **flashlight → stethoscope (Block 94 [`zenoh-surface-deep-links.md`](https://github.com/usareur-af/goatcore/blob/main/docs/design/zenoh-surface-deep-links.md)):** "stethoscope ↗" link next to the panoscope link in the header. Same focus QS is passed through verbatim — flashlight's `?vendor=` and `?key=` map directly onto stethoscope's target picker; flashlight-only params (`?role=`, `?site=`, `?format=`, `?compute=`) are silently dropped by stethoscope per the contract's "unknown params silently dropped" rule.

**Sibling-link labels (2026-05-31 operator UX feedback).** First live-fire of the Block 94 hops showed both verbose labels were unhelpful: the question form on the stethoscope link (`is this healthy? ↗`) misread as a diagnostic assertion when the actual hop is just navigation to the diagnostic *tool*, and the verbose forms ate header real estate needed for the T2 augmentation badges. Both links now use the bare surface name (`panoscope ↗`, `stethoscope ↗`) and rely on a hover-title for the one-sentence explainer. Compositional benefit: the format scales to additional siblings without re-allocating the line.

**Header-chip explainers (2026-05-31 UX polish).** Each T2 augmentation badge (`rate` — the `z-score` + `drift` chips were removed in Phase F-3; see Tier 2 surfaces), the connection-status pill (`live` / `reconnecting` / `offline`), the focus echo, and the sibling links all gained a `title` attribute carrying a one-sentence operator-facing explainer. Same hover-only-on-intent posture as the sibling-link explainers — the chips' compact numeric values stay legible at a glance, and the longer "what does this number mean" answer is one hover away. No popover machinery; just the browser's native title-tooltip behavior.

**Sparkline peak/now stats (2026-05-31 UX polish).** Each sparkline section's label now carries a right-anchored `peak X · now Y` text label next to the title. Cheaper to read than canvas y-axis ticks at the 48-pixel sparkline height, and removes the "what's the magnitude here?" ambiguity the canvas alone produced. `app.js`'s `drawSparkline()` + `paintT2Sparkline()` update the stats text on each redraw via a small `fmtRate()` helper (integer ≥ 10, one decimal ≥ 0.1, two decimals smaller).

**Live event-table row clarification (2026-05-31 UX polish).** Operator feedback: rows in the live event table read as a topic list when the same key repeated across rows, hiding the per-event semantics. Three minimal additions: column header renamed `key` → `key (per event)`; controls-bar label `showing N of N` becomes `events · showing N of N`; hover-titles on every column header + the row count carry the explicit "one row per message; same key repeats when its publisher emits multiple" hint.

**Block 94 wiring:** the stethoscope header link reads the `FLASHLIGHT_SIBLING_STETHOSCOPE_URL` env var (Ansible role default at the canonical mesh-DNS pattern, `https://stethoscope.<site_dns_zone>`); empty value suppresses the button (URL-only contract, no runtime coupling). Same shape as the existing `FLASHLIGHT_PANOSCOPE_ORIGIN` substrate, generalized via a small `SiblingOrigins` struct in [`render.rs`](../../ops/flashlight/src/render.rs). Per-row diagnose / tail buttons on the live event table are NOT in this contract — flashlight rows already point at one topic, and the operator's natural next move from a flashlight row is to inspect the payload, not to bounce to a sibling. The header-level switch is the documented hop.

The two surfaces share crates (see below) but never share runtime state. They are distinct services with distinct deployments, distinct image SHAs in the ADR 0859 ledger, distinct rollback ledgers, and distinct CI pipelines.

## Architecture

### Tier 1 — canonical workhorse

- **Backend.** Rust + `axum`. Same shape as panoscope's backend (axum + tokio + rust-embed). Subscribes to Zenoh under a per-request narrowed key expression derived from the URL filter; emits structured JSON events over SSE.
- **Frontend.** Server-rendered HTML from Rust templates (`maud` or `askama`); ~20 KB total payload including a small standalone `.js` file. Vanilla JavaScript only. Opens `EventSource`, listens, pokes the DOM. *(built)* Stream handlers only mutate in-memory state; the event table and the sparkline repaint at most once per animation frame (`scheduleRender` → `requestAnimationFrame`). A full table rebuild per event made a hidden tab replay every buffered event's rebuild back to back on return, and `requestAnimationFrame` does not fire while the tab is hidden, so a backlog now costs one repaint. *(measured, local mock at 200 events/s, tab frozen 30 s via CDP `Page.setWebLifecycleState`: long tasks after return 239 ms → 0 ms.)*
- **Streaming.** Server-Sent Events with Brotli compression (per [`minimalism-under-network-constraints.md`](https://github.com/usareur-af/goatdocs/blob/main/docs/design/minimalism-under-network-constraints.md) Decision 1). Long-lived; `Last-Event-ID` for resume; Service Worker owns the EventSource and rebroadcasts via `BroadcastChannel` (per Principle 8). **The live SSE event carries no payload bytes** — only `seq` (a monotonic ingest id), `key`, `ts_ms`, `src_zid`, `payload_len`, and a content-classified `format` hint. This is the doctrine in action: bytes don't hit the wire until the operator asks for a specific sample.
- **On-demand payload retrieval (`SampleStore` + `GET /api/sample/{seq}`).** A bounded in-memory ring (default 1024 samples × ≤4 KiB preview each; `FLASHLIGHT_SAMPLE_STORE_CAP` / `FLASHLIGHT_SAMPLE_PREVIEW_BYTES`) retains the leading bytes of every observed sample, keyed by `seq`, fed by the shared broad subscriber. When the operator clicks an event row, the browser fetches `GET /api/sample/{seq}`, which returns `{ format, payload_len, preview_len, truncated, hex, text }` — `hex` always (faithful bytes), `text` only for texty formats. A click on a sample that has since evicted out of the ring returns `404` with a structured "expired" body, which the UI surfaces honestly. This is what makes flashlight an *inspection* surface (you see the actual values) rather than a rate meter (key + byte-count). [ADR 0863](https://github.com/usareur-af/goatcore/blob/main/docs/adr/0863-resilient-end-node-design-philosophy.md) end-node minimalism — the ring is the cheapest substrate that lets bytes stay off the live wire while still being one click away.
- **Visual surfaces.** SVG for icons, status pills, format pills (reusing panoscope's set). Canvas 2D for the event-rate sparkline and event-rate ribbon. No D3, no force layout, no hull rendering.
- **First-paint budget.** ≤200 KB total over the wire (HTML + JS + initial SSE handshake). Speculation Rules `prerender` on sibling-focus links for near-instant focus transitions.

### Tier 2 — coprocessor

- **Build target.** Single Rust→WASM module compiled from `flashlight-analytics` crate. `wasm32-unknown-unknown` target; `wasm-bindgen --target web --no-typescript` produces `flashlight_analytics_bg.wasm` + `flashlight_analytics.js` ES-module glue. Built by the flashlight Dockerfile's `wasm-builder` stage, embedded into the runtime binary via `rust-embed`, served by `GET /api/wasm/{filename}`. Streaming-compiled in the browser via `WebAssembly.compileStreaming`. Cache-Control is short — the wasm-bindgen default filename isn't content-hashed, so a fresh build under the same name needs to be reloadable.
- **Load gate.** Lazy. Auto-load when `navigator.connection.effectiveType === '4g' && !navigator.connection.saveData`; otherwise operator clicks an "enable analytics ↗" button. Operator-controllable override toggle (force-load / force-disable) in a settings panel.
- **Data path.** Tier 1 owns the SSE connection and a small ring buffer of recent events. When Tier 2 loads, JavaScript forwards each new event into the WASM module via `instance.exports.push_event(ptr, len)` — one copy per event, events are small (~200 bytes), no `SharedArrayBuffer` / COOP-COEP headers needed. The WASM module never opens its own network connection.
- **Visual surfaces.** Tier 2 augments the page with additional panels (decoded payload viewer, rolling-stats panel, sparkline with higher-fidelity rendering, schema-diff panel). It never replaces a Tier 1 panel. CSS-class `.tier-2-only` makes augmentation surfaces visible only when WASM is loaded; the same surfaces are hidden in Tier 1.
- **Eviction.** On WASM crash / OOM / timeout, the module is dropped; `.tier-2-only` surfaces hide; Tier 1 continues unchanged. No re-mount of Tier 1 components.

### Same Rust code, two compile targets

Per Principle 13 (compute placement is operator-controllable):

- `flashlight-analytics` (Rust crate) compiles to **both** `wasm32-unknown-unknown` (Tier 2 in the browser) AND native (linked into the flashlight backend as the implementation of `POST /api/compute`).
- The operator chooses where compute runs: pull it into the browser via Tier 2 on a strong device; push it to the server via `/api/compute` on a degraded device.
- The same canned-input equivalence test (per Principle 12) runs against both targets and asserts byte-identical output.
- **Endpoint shape (Phase D as-landed):** `POST /api/compute` accepts `{ "events": [Event, ...] }` and returns the same `AnalyticsSnapshot` JSON the browser-side `flashlight_analytics::Engine::snapshot()` produces. Stateless — server runs each request's events through a fresh Engine. Optional `?window_ms=` query parameter overrides the default 5-minute rolling window (matches `Engine::with_window_ms`).

## URL filter grammar

The URL declares the focused subscription. The grammar is small, learnable from the URL bar, and shareable.

### Path

`GET /` — the focused-view page. The same path always; the slice is in the query string.

### Query parameters

| Parameter | Type | Example | Effect |
|---|---|---|---|
| `key` | Zenoh key expression | `?key=vendor-b/observations/v1` | Subscribe to exactly this key. Supports Zenoh wildcards: `?key=vendor-b/**`. |
| `vendor` | vendor short_tag | `?vendor=vendor-a` | Anchor-aware vendor match: passes keys shaped `<vendor>/**` (per-vendor goatnets) AND `<slot-anchor>/<vendor>/**` (slot-pool goatnets, ADR 1025). Evaluated as a sample-time predicate via `panoscope_core::key_vendor_matches` — deliberately not a `*/v/**` key expression, which would over-match non-anchor first segments (`ops/v/…`). No portal round-trip (shape-classified per ADR 1039's disjoint-shapes invariant). |
| `role` | role class | `?role=sensor` | Subscribe to all peers carrying `role_class: sensor` in their schema-decl. Server-side resolution via panoscope-core's peer registry. |
| `site` | site short_name | `?site=alpha` | Subscribe to all peers tagged with this site. |
| `since` | duration | `?since=10m` | Replay window: include events from this far back (server reads recent NDJSON from MinIO `audit-integrity` à la panoscope's MinIO replay). Defaults to live-only (no replay). Max 1 hour. |
| `until` | duration or absolute | `?until=now` | End of replay window. Defaults to live (open-ended). |
| `format` | wire format | `?format=protobuf` | Filter to topics declared with this wire format. |
| `compute` | `browser` \| `server` | `?compute=server` | Force compute placement (Tier 2 vs `/api/compute`). Default = browser if Tier 2 is loaded, server otherwise. |

Multiple parameters AND together: `?vendor=vendor-a&role=sensor` = peers from vendor-a AND of role sensor. Repeated parameters OR within: `?vendor=vendor-a&vendor=vendor-b` = either vendor.

### Examples

```
flashlight.nova.*/?key=vendor-b/observations/v1            → one topic
flashlight.nova.*/?vendor=vendor-a                          → one vendor's traffic
flashlight.nova.*/?role=sensor&site=alpha              → sensors at one site
flashlight.nova.*/?key=notional/**&since=10m               → notional fabric, last 10min
flashlight.nova.*/?vendor=vendor-b&format=protobuf          → vendor-b's protobuf topics
flashlight.nova.*/?role=c2&since=30m&compute=server        → C2 traffic last 30min, compute server-side
```

The URL is the share. Pasting any of the above into chat gives another operator the same view.

### Validation

The server validates the filter, returns `400` with a structured error if invalid, and includes a *minimal* example URL in the error body for the operator's next attempt. Empty URL (`flashlight.nova.*/?`) lands on the **directory landing** (see "Discovery + navigation" below) — *never* on a "subscribed to everything" view. Empty filter = empty subscription; the directory gives the operator somewhere to go instead of an empty page.

## Discovery + navigation

The URL is the share, but an operator who *doesn't already know* a key or vendor needs a way in. Two surfaces, both present without Tier 2, both fed by `GET /api/directory` (vendors + their topics from the portal registry via the shared `panoscope-core` `PortalClient`):

- **Directory landing.** When the focus is empty, the page renders a browseable catalog instead of an empty tail: each vendor (group) is a card listing its topics with a format icon, declared format, description, and publish count. Every topic is a one-click link to `/?key=<key_pattern>`; every vendor name links to `/?vendor=<prefix>`. The live-tail surfaces (sparkline, event table) are hidden in this mode; the status pill reads `browse`.
- **Header jump box.** A custom type-ahead present on *every* view (tail or directory), so the operator pivots to any topic/vendor/peer without editing the URL bar. The search index is every vendor prefix + every topic key + every peer's keys, each carrying the peer's id/handle as a searchable alias. Matching is **case-insensitive substring in both directions** — a typed fragment matches anywhere in the candidate (the leading `<peer-id>/` anchor *or* any middle/trailing key segment), so `vendor-b/contacts` finds `<uuid>/vendor-b/…/contacts/v1` and a peer-id fragment surfaces that peer's keys. (This replaced a browser-native `<datalist>`, which only prefix-matched the whole option value and so missed peer-id-anchored keys and mid-key fragments.) Ranked by match position, then value length. Submitting (Enter) navigates to the highlighted match, else the top match, else the raw text typed — resolving to `/?vendor=<x>` when it exactly matches a known vendor prefix, otherwise `/?key=<x>`. Navigation is a single MPA navigation — the URL-is-the-share model is preserved. Pressing **`/`** (when not already typing in a field) focuses the jump box — the same keyboard affordance panoscope uses.

- **Role-based browse.** `/api/directory` also carries a `peers` array built from the schema-decl-hydrated topology (see "Peer topology + post-filters"), and the landing renders a **Sources / Derivers / Sinks** section below the vendor cards — each peer card lists its tailable keys (publish keys for sources/derivers, subscribe keys for pure sinks), one click into a focused view. This is independent of the portal: a deployment with a live fabric but no portal still gets role browse. Unknown-role peers (no declared pub/sub) are omitted as noise.

Degradation: when the portal is unconfigured (`FLASHLIGHT_PORTAL_URL` / `FLASHLIGHT_PORTAL_API_KEY` unset) `/api/directory` returns `{ "enabled": false }` for the vendor section, the jump box still works for raw keys, and role browse still renders from topology if any peers have been observed.

### Peer topology + post-filters

The `?role=` / `?site=` / `?format=` filters resolve against a peer registry, not a key pattern. flashlight hydrates that registry itself: synth-vendor **schema declarations** (published on `attest/schema/<vendor>/<system>` keys, JSON, ~30 s cadence) flow through the same broad `**` subscription that feeds the live tail, so `schema_decl.rs` parses each decl into a local `panoscope-core::Topology` (peer → declared `role_class` / `site` / `format` + `publishes[]`). The field handling deliberately mirrors panoscope's `apply_schema_declaration` — same key prefix, same 90 s staleness gate, same JSON field names — so the two surfaces classify a peer identically; the schema-decl JSON is the stable published contract. (A later cleanup may factor the parser into `panoscope-core` so there is a single copy instead of two that must move together.)

The SSE handler then applies the post-filters per event: an event's key is matched against every peer's declared publish patterns, and the event passes only if some matching publisher satisfies every active filter dimension. An event whose key matches **no** declared publisher is **dropped** while a post-filter is active — flashlight can't confirm it satisfies the filter, so it must not leak through. This is the correct behavior versus the prior bug where a role/site/format filter silently widened the subscription to `**` and streamed the whole fabric. A peer that just came online (no schema-decl yet) is invisible to `?role=` until its first declaration — the documented v1 limitation below.

## Reusable crate structure

Two new shared crates land as part of this work. Each has a single responsibility and is independently versioned.

### `panoscope-core` (factor-out from existing `ops/panoscope/`)

Substrate-layer concerns shared between panoscope and flashlight:

- Zenoh client setup (mTLS, cert loading, mode selection).
- Audit-event parsing (the NDJSON shape from Block 82S audit-subscriber).
- Role taxonomy (source / deriver / sink / topic-broker-pin / infrastructure).
- Peer registry data structures (with schema-decl + admin-namespace discovery paths).
- MinIO replay-on-boot machinery (the recently-landed `minio_replay.rs` substrate).

Panoscope and flashlight depend on `panoscope-core` for these concerns. Behavioral change is none — the factor-out preserves existing panoscope behavior exactly; equivalence is asserted via the existing panoscope test suite.

### `flashlight-analytics` (new, dual-target)

Compute-heavy analytics utilities. Compiles to:

- `wasm32-unknown-unknown` for Tier 2 in the browser.
- Native (x86_64, aarch64) for linking into the flashlight backend's `POST /api/compute` endpoint.

Capabilities (v1 scope):

- **Protobuf decode against arbitrary `.fds` descriptors** (`prost-reflect`). Same descriptor cache machinery panoscope just landed in its decoded-stream view, but exposed as a library function.
- **Sliding-window rolling statistics** — rate, p50/p95/p99 inter-arrival time, byte-count percentiles, over operator-specified window (default 5 min).
- **Schema-version diff** between any two uploaded `.fds` descriptors.
- **Anomaly detection** — z-score over a sliding window; threshold-based.

Each capability has the same Rust function signature regardless of compile target. WASM exports + native `axum` handler share the implementation. Equivalence-test substrate (per Principle 12) runs canned input streams through both and asserts byte-identical output.

### What stays NOT in shared crates

flashlight-specific surface code lives in the flashlight crate alone:
- URL filter grammar parsing.
- The SSE event shape specific to flashlight.
- The HTML / JS / CSS assets.
- The Tier 2 boot logic and `BroadcastChannel` wiring.

Panoscope-specific surface code (D3, hull rendering, force layout, selection-focus reorganization) stays in panoscope alone.

## Tier 1 surfaces

What the page does without Tier 2 loaded — i.e., the canonical, always-works experience.

- **Header bar:** focus declaration (echo of the URL filter, human-readable), connection-status pill (LIVE / RECONNECTING / OFFLINE / STALE), Brotli-compression indicator, "panoscope ↗" + "stethoscope ↗" sibling links (Block 94; per-link hover-title carries the explainer).
- **Event-rate sparkline (Canvas 2D):** rolling rate of events in the focused slice over the last 60 s. Redraws each new event.
- **Live event table:** last N events (N=100 default; configurable via `?n=` up to 500). Columns: timestamp, key expression, **format icon**, payload length. The format icon is content-classified from the payload bytes at ingest into the panoscope-derived set — `{ }` (JSON object) / `[*]` (JSON array) / `txt` (text) / `0x` (binary or protobuf) / `·` (empty). (The authoritative wire format — e.g. the protobuf message name — comes from the portal registry in the Tier 2 decode view; this column is the cheap at-a-glance heuristic.) Auto-scrolls; pause/resume + clear buttons.
- **Per-key tail:** if `?key=` is a single exact key, the table is just for that key. If a wildcard / vendor / role filter is in use, the table is mixed and each row carries its key.
- **Filter controls:** form for editing the URL filter inline (`vendor` dropdown populated from portal, `role` dropdown from taxonomy, `since` text input). Changes navigate to the new URL (single MPA navigation; Speculation Rules `prerender` makes the transition near-instant per Principle 9). *(Built alongside the directory landing — see "Discovery + navigation" below.)*
- **Payload viewer (pinned, auto-following).** A panel pinned at the **top** of the focused view shows the current sample's payload via `GET /api/sample/{seq}`. While the tail is running it **auto-follows the newest event** (throttled ~600 ms so a fast slice doesn't decode every frame); **pausing — or clicking a row — freezes it** on the selected sample (`↺ live` resumes following). The auto-follow fetch is **single-flight** and render-eligibility is keyed to a monotonic request token, not to view-seq equality: when the link's fetch round-trip outlasts the refresh interval the panel **lags the live tail by one RTT** rather than going blank, and it paints the freshest sample it actually fetched even though newer events kept arriving. (The earlier `viewSeq`-equality guard discarded every in-flight fetch on arrival under sustained high rate over a slow link, so the panel painted nothing until traffic lulled and then froze on a stale sample — fixed 2026-06-16.) It shows the **deserialized message and the serialized bytes together**, in two stacked panes: a **decoded** pane (protobuf-deserialized JSON when the topic has a schema — see below; or UTF-8 text for texty topics; or a schema-less structural protobuf view labelled `protobuf structure · no schema` when the bytes parse as protobuf but no descriptor is available; hidden only when none apply) above a **raw** pane (always the on-wire bytes as a hexdump). seq · key · size · decode-note are in the panel header. Pinned-top placement (operator UX 2026-06-01) replaced an earlier bottom-docked panel that drifted off-screen as the tail grew. An evicted sample shows "sample expired" rather than hanging.

What is NOT in Tier 1 (and is explicitly badged "requires analytics module — load now?"):

- Decoded protobuf payload view.
- Sliding-window statistical overlays on the sparkline.
- Schema diff between two versions.
- Anomaly highlights.

## Tier 2 surfaces

What lights up when the WASM module is loaded — augments Tier 1, never replaces.

- **Decoded payload viewer (landed F-3d — as a server-side Tier 1 surface, not WASM-gated).** The decode runs **server-side** in the `/api/sample/{seq}` handler: it resolves the topic's declared `protobuf:<Name>` from the **portal topic registry** (`find_topic_by_key` — the authoritative schema source; synth-vendor schema-decls carry role/site but not `format`, so a topology `format_hint` is only a fallback), pulls the `.fds` descriptor from the portal (`fetch_fds`), and decodes via the native `flashlight-analytics::decode`. Format + descriptor are memoized together per key (`DecodeCtx`, negative-cached for "no schema") so auto-follow refreshes don't re-hit the portal. The deserialized JSON renders in the decoded pane alongside the raw hex. This deliberately does **not** require the Tier 2 WASM module — decoding one followed/clicked sample server-side is cheaper than shipping a descriptor pool to the browser, and keeps the decoded view working on constrained clients. (The Tier 2 WASM path remains available for bulk per-event analytics, where pushing compute to the browser pays off.)
  - **Schema-less structural fallback.** When no `.fds` is available (topic not in the portal registry, no descriptor uploaded, portal unconfigured, or the schema decode errors), a binary payload that *parses cleanly as protobuf wire format* still gets a deserialized view — a structural decode (`flashlight_analytics::decode::decode_wire_structure`, à la `protoc --decode_raw`) keyed by **field number** (names are unknowable without a schema), with length-delimited fields rendered as nested message / UTF-8 string / hex. The decoded pane carries the label `protobuf structure · no schema` and the meta note says so, so it's never confused with a true decode. It is deliberately **strict** — it returns nothing unless the *entire* buffer parses as protobuf, so arbitrary non-protobuf binary shows hex rather than plausible-but-fake fields (the honest-signals bar, Principle 11). This is what makes the payload area show *a deserialized message* rather than only hex when a goatnet's topics aren't registered with a protobuf schema; the full schema decode is still preferred whenever the portal has the descriptor. Resilient by construction — works with the portal absent, consistent with the end-node minimalism doctrine.
- **Rolling-stats panel:** rate / p50 / p95 / p99 inter-arrival time / byte-count percentiles, over a configurable window. Re-computed in WASM on each event.
- **High-fidelity sparkline overlay:** the Canvas 2D sparkline from Tier 1 stays; Tier 2 adds an overlay with rolling-window percentile bands.
- **Schema-diff viewer:** if the topic has multiple uploaded `.fds` versions, side-by-side message-field diff.
- **Anomaly highlights:** rows in the live event table that fall outside the z-score threshold get a subtle red border. Operator-configurable threshold.

All Tier 2 surfaces carry a tiny `[T2]` badge so the operator knows which surfaces are analytics-module-derived (and would disappear if Tier 2 unloaded). Honest naming, per Principle 11.

**Header-chip status (Phase F-3, 2026-06-01).** The Phase E build shipped three compact header chips — `rate`, `z-score`, `drift`. The `z-score` and `drift` chips were **removed**: the drift chip compared a wire `format` the live SSE event never carried (so it was structurally always `0 / N`), and the z-score was a weak self-referential signal on event rate, shown more prominently than the data itself — both failed the honest-signals bar (Principle 11). The `rate` chip and the Tier 2 sparkline stay (they reflect real arrival timing). The analytics engine still *computes* anomaly + schema-diff (`snapshot.anomaly` / `snapshot.schema_diff` remain in the WASM/native output, equivalence-gated); they are simply not surfaced until they have trustworthy inputs — anomaly over a real metric, schema-diff over the portal-declared format (which arrives with the decode view). The aspirational surfaces above (anomaly row-highlights, schema-diff viewer) are the form they return in.

## Compute placement

The `?compute=` URL parameter selects where analytics compute runs:

- `?compute=browser` (default if Tier 2 is loaded): WASM runs in the browser. Lower latency per result, no server round-trip, works in island mode.
- `?compute=server` (default if Tier 2 is not loaded): the flashlight backend runs the same Rust code natively against the same event stream and returns results via SSE alongside the raw events. Useful on degraded clients (low memory, slow CPU).

Switching `?compute=` is an MPA navigation. No state is lost — both modes operate on the same focused subscription.

Equivalence is CI-asserted (per Principle 12): canned event-stream fixtures feed both the WASM build and the native build, and the test asserts byte-identical output.

## Equivalence-test substrate

Per Principle 12, where features have a Tier 1 + Tier 2 path OR a WASM + native path, CI asserts identical output.

### Per-feature equivalence tests

- **Rolling stats:** `flashlight-analytics::rolling_stats` runs against a 1000-event canned NDJSON stream; output (rate, p50, p95, p99) is byte-identical between WASM and native builds. Test fixture under `crates/flashlight-analytics/tests/equivalence/`.
- **Protobuf decode:** same 100-message canned stream + 5 reference `.fds` descriptors; decoded JSON is byte-identical between WASM and native.
- **Anomaly detection:** same canned stream + thresholds; flagged-event indices are byte-identical.

### Tier 1 / Tier 2 overlap tests

Where a feature exists in both tiers (e.g., the event-rate sparkline has a basic Tier 1 implementation and a percentile-band-overlay Tier 2 implementation), a Playwright test loads the page in Tier 1-only mode (WASM blocked via Content Security Policy) and asserts the Tier 1 surface still works for the operator's primary task. This is the "useful without Tier 2" gate from Principle 2.

CI matrix:

- `flashlight-analytics` unit tests, native target.
- `flashlight-analytics` unit tests, `wasm32-unknown-unknown` target (run via `wasm-bindgen-test` or `wasmtime` headless).
- Equivalence harness: native vs WASM, byte-identical output.
- Playwright: Tier 1-only loads the page with WASM blocked; primary surfaces work.
- Playwright: Tier 2 loaded; augmentation surfaces appear; no Tier 1 surface disappears.

## Deployment shape

flashlight ships as a standalone service following the canonical mgmt-host service pattern (panoscope is the immediate precedent).

- **Image:** `ghcr.io/<org>/flashlight` (Chainguard Rust → static distroless, multi-arch arm64+amd64), built by `.github/workflows/flashlight-image.yml` on every merge that touches the flashlight crate or its assets.
- **Ansible role:** `ops/ansible/roles/flashlight/` — image pin + runtime ledger (per ADR 0859) + compose overlay + env-var threading for Zenoh cert paths, portal API key, MinIO replay creds.
- **Compose:** `ops/compose/mgmt-host/docker-compose.flashlight.yml`. Single container; reuses the existing flashlight bind mounts for offline-CA root + per-instance leaf cert.
- **Traefik router:** mesh-only allowlist matching the panoscope shape. Adds `flashlight.nova.netbird.goat.example` SAN to the mesh-only certificate.
- **Mesh DNS:** `flashlight.nova.netbird.goat.example` → nova-a mesh IP via CoreDNS (`ops/dns/nova/mesh-hosts`).
- **isitup row:** `ops/ansible/group_vars/nova_hosts/vars.yml` `isitup_services` list, classification `public` (mesh-internal but no admin gate — same as panoscope).
- **Apply playbook:** `ops/ansible/flashlight.yml`.
- **Rollback:** `ops/scripts/rollback.sh flashlight [N]` per ADR 0859. Tier-0 adopter from day one.

Production hosts (vega, prov-*) inherit the role with their own `*_hosts/flashlight.yml` group_var; deferred until nova-a live-fire validates the surface.

## Phased build plan

Each phase lands as one or more PRs with the doc/in-flight delta in the same change-set per documentation discipline.

### Phase A — Planning (this PR)

Five docs in one PR. **No code.** This is the change-set you're reading.

- `docs/design/minimalism-under-network-constraints.md` (new — the doctrine).
- `docs/design/flashlight.md` (new — this doc).
- `docs/project/in-flight.md` entry (new — Block 89).
- `CLAUDE.md` Data substrates pointer (alongside panoscope).
- `docs/project/active-work/tracks/flashlight-and-minimalism.yaml` (new track).

Gate to Phase B: operator review + approval of the doctrine + this design doc.

### Phase B — `panoscope-core` factor-out

Mutates panoscope; preserves behavior exactly. New `crates/panoscope-core/` shared crate; panoscope migrates to depend on it. Equivalence asserted by panoscope's existing test suite + a Playwright smoke against nova-a's running panoscope. Zero new flashlight code in this phase.

Gate to Phase C: panoscope on nova-a re-deployed with the factored crate; 24 h burn-in clean.

### Phase C — Tier 1 first useful demo

Flashlight backend (axum), URL-filter grammar, SSE handler, Tier 1 HTML+JS, Brotli compression, Service Worker + EventSource ownership, Speculation Rules, Ansible role, image build, mesh DNS, Traefik router. No Tier 2, no analytics, no `flashlight-analytics` crate yet. The single deliverable: a focused live tail of any URL-declared slice, usable on nova-a from any browser.

Gate to Phase D: live-fire on nova-a; an operator points at three different slices and the experience matches the doc.

### Phase D — `flashlight-analytics` dual-target crate

New `crates/flashlight-analytics/` crate. v1 capabilities: protobuf decode, rolling stats, schema diff, z-score anomaly detection. Equivalence-test substrate. WASM build pipeline. `/api/compute` server endpoint.

Gate to Phase E: equivalence tests pass on CI for all four capabilities; binary-identical output asserted.

### Phase E — Tier 2 integration

Tier 2 boot logic, `BroadcastChannel`, lazy-load gating, augmentation surfaces, `?compute=` URL parameter, Playwright tests for Tier 1-only + Tier 2-loaded modes. `[T2]` badging on augmentation surfaces.

Gate to canon: live-fire on nova-a; an operator on a strong device gets analytics, an operator on a constrained device gets the same surfaces server-side via `?compute=server`, an operator with WASM blocked still gets a useful Tier 1 view.

## What's NOT in v1

Explicit non-scope to preserve the "lean by construction" property:

- **Saved focus presets.** v1 has no preset storage — the URL IS the preset. Operators bookmark URLs in their browser. Server-side preset library is a v2 conversation.
- **Multi-slice composition.** v1 inspects one slice per page. Comparing two slices side-by-side opens a second flashlight in another tab (each Tier 1 surface is independent; no special multi-pane mode).
- **Mutating actions.** flashlight is read-only — it observes, it never publishes, never PATCHes, never POSTs back to the fabric. Same posture as panoscope. (Operator mutations live on the portal admin surfaces.)
- **Authentication beyond mesh-only allowlist.** v1 trusts the mesh perimeter like panoscope and the other inspection surfaces. If a future deployment context needs OIDC group-gating, the oauth2-proxy substrate is the integration point — but not v1.
- **Mobile UI.** v1 targets desktop browsers; the Tier 1 doctrine should produce a usable-on-mobile experience by accident, but no mobile-specific affordances. snitch-app (Block 77) is the mobile inspection surface.

## Open questions

Recorded so they're addressed before Phase B lands, not after:

- **WASM module versioning vs Tier 1 deployment.** If Tier 1 ships with an updated event schema and Tier 2 is cached at an older version, what does the operator see? Likely answer: include a Tier 1 schema-version header in the SSE handshake; Tier 2 refuses to attach if mismatched and surfaces "analytics module is stale — reload".
- **Sample payload retention bound for `?since=` replay.** MinIO `audit-integrity` retains payloads ≤ a configurable size. flashlight's replay path must handle the "payload exists in MinIO at zero bytes" case gracefully.
- **`role=` resolution latency.** The peer-role mapping comes from schema-decls (refresh every 30 s on the synth-vendor side). A peer that just came online has no schema-decl yet and is invisible to `?role=` filters until its first publish. Acceptable for v1; documented limitation.
- **Compute-server endpoint security.** `/api/compute` is a server-side compute endpoint. Mesh-only allowlist limits exposure but doesn't rate-limit. v1 may need a basic per-IP rate limit if degraded operators repeatedly invoke heavy compute.

## Living document

Updates land via PR. The first material revision target is when Phase C ships and an operator has actually used the surface.
