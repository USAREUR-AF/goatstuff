# stethoscope — passive-default diagnostic surface for the goat data fabric

| Field | Value |
|---|---|
| Status | Design (authored 2026-05-24). Phase A planning only; Phases B-E gated per phase. |
| Scope | Server-side diagnostic surface that auscultates a single vendor / system / topic and reports a structured health verdict. Passive checks always run; active "blood dye" probes are off by default and toggled on per-diagnosis-session. Modular check architecture — each diagnostic is a self-contained plugin with explicit cost + applicability + on/off knobs. |
| Related | `panoscope.md` (the live whole-fabric view that links into stethoscope), [`flashlight.md`](flashlight.md) (the client-side focused inspection sibling, distinct purpose), [`portal-schema-publishing.md`](https://github.com/usareur-af/goatcore/blob/main/docs/design/portal-schema-publishing.md) (the registry stethoscope queries), [`portal-vendor-progress.md`](https://github.com/usareur-af/goatcore/blob/main/docs/design/portal-vendor-progress.md) (the fleet-wide swimlane stethoscope drills down from), [`portal-live-troubleshooting.md`](https://github.com/usareur-af/goatcore/blob/main/docs/design/portal-live-troubleshooting.md) (the log/probe/echo cluster stethoscope composes), [`zenoh-audit-external-subscriber.md`](https://github.com/usareur-af/goatcore/blob/main/docs/design/zenoh-audit-external-subscriber.md) (the audit-tail data source for passive checks), [ADR 0844](https://github.com/usareur-af/goatcore/blob/main/docs/adr/0844-zenoh-deployment-topology-and-acl-v1.md) (reserved prefixes; stethoscope adds one). |

## What this is

> **Display lexicon (2026-06-17):** stethoscope's participant-noun wording is goatnet-appropriate — "vendor"/"Vendor"/"VENDOR" on nova-a, "org"/"Org"/"ORG" on ZAGROS (hero kind eyebrow, picker kind-chip + placeholders + hint, and the `stethoscope-checks` verdict messages via a `CheckCtx.org` field). `config::Lexicon` substitutes server-side + a `window.STETHOSCOPE_LEX` global feeds `stream.js`; `STETHOSCOPE_LEX_ORG` is wired by the role from the same `portal_lex_org` group_var. Display-only — the `vendor` `TargetKind`/`data-kind`, the `?vendor=` deep-link param, the `<anchor>/<rest>` key shape, and portal `list_vendors`/`/api/vendors` are untouched. See [`portal-lexicon.md`](https://github.com/usareur-af/goatcore/blob/main/docs/design/portal-lexicon.md) §"Flashlight + stethoscope display layer".

A web service at `stethoscope.nova.netbird.nova.example` (nova v1; prod hosts add as siblings) that answers one question for one target at a time:

> **"Is this vendor / system / topic actually working the way the operator expects?"**

The target is declared in the URL — `?vendor=example-vendor`, `?topic=example-vendor/observations/v1`, `?system=...` — and the server runs a battery of diagnostic checks against it. Each check returns a structured verdict (Pass / Warn / Fail / Skipped / N/A). The page renders one card per check with a status pill, a one-line summary, expandable details, and per-check controls. A roll-up at the top shows overall target health.

The name *stethoscope* matches the metaphor: a focused diagnostic instrument that listens to one organ at a time. Panoscope is the floodlight that shows the whole room; flashlight is the focused beam an operator drives into one corner; stethoscope is the instrument you place on the target and read the readout from.

Two operator questions stethoscope answers that no other surface answers cleanly today:

- **"Is this behaving as I expect?"** — every check has a target-aware verdict, so the operator sees per-aspect verdicts (registered? schema uploaded? data flowing? decodes against schema? subscribers visible? latency reasonable?) rather than having to crawl three surfaces and synthesize.
- **"Do my expectations actually make sense?"** — checks compose into auto-derived findings: "topic registered 30 d ago, audit-tail shows zero events" → *advisory: declared-but-never-published*; "schema vN uploaded but live wire bytes don't decode" → *advisory: schema-doesn't-match-bytes*. The operator doesn't have to author SLOs upfront; the system surfaces structurally-inferable expectation violations.

## Complementarity with sibling surfaces

stethoscope is distinct from every other inspection surface — not a replacement for any of them.

| Surface | Question it answers | Interaction model |
|---|---|---|
| **Portal** | What's *declared* about this vendor/topic? | Static, declarative. Registry lookup. |
| **Panoscope** | What does the whole fabric look like *right now*? | Live overview, all peers/topics, hover/select for drill-down. |
| **Flashlight** | What's happening in *this slice* under bad network or degraded compute? | Client-side focused navigation, operator drives the view. |
| **Stethoscope** | Is *this target* actually healthy? | Server-side diagnostic verdict, less driving, more reading. |

Cross-linking from sibling surfaces (every selected peer / topic / vendor builds the stethoscope URL):

- **panoscope → stethoscope:** per-edge "diagnose ↗" button on every topic-broker pin and side-panel "diagnose this peer ↗" / "diagnose this topic ↗" buttons. The link builds `?vendor=…` or `?topic=…` from the panoscope selection.
- **portal → stethoscope:** every vendor-catalog row and every topic-detail page gets a "diagnose ↗" button. Same URL shape.
- **flashlight → stethoscope:** the flashlight header carries "is this healthy? ↗" — same target, different surface, operator switches mode without changing the focus.
- **progress.html (Block 88) → stethoscope:** each vendor's row in the fleet swimlane drills down to that vendor's stethoscope page.
- **stethoscope → siblings:** stethoscope header carries "see in panoscope ↗" and "watch in flashlight ↗" buttons. Operator returns to context or zooms to live tail in one click.

**Block 94 wiring (cross-surface deep-link contract):** the link matrix above is governed by [`zenoh-surface-deep-links.md`](https://github.com/usareur-af/goatcore/blob/main/docs/design/zenoh-surface-deep-links.md). Stethoscope's contribution: a `Siblings` config block (`STETHOSCOPE_SIBLING_{PANOSCOPE,FLASHLIGHT}_URL` env vars), a `?key=<topic>` alias for the existing `?topic=<topic>` per the contract canon (both accepted; `key=` wins on conflict; legacy `topic=` stays indefinitely so wireframe-bundle URLs and operator muscle memory keep working), and the two topbar buttons named above. Outbound URLs preserve the current target — `?key=<topic>` for topic targets, `?vendor=<v>` for vendor targets, `?vendor=<vendor-prefix>` for system targets (siblings have no system-level selection in v1; vendor is the next-best parent). Each button is individually conditional on the corresponding sibling URL being set — a stethoscope deployed without a sibling just doesn't render that button (URL-only contract, no runtime coupling).

The four surfaces share crates (see Reusable crate structure below) but never share runtime state. They are distinct services with distinct deployments, distinct image SHAs in the ADR 0859 ledger, distinct rollback ledgers, and distinct CI pipelines.

## Architecture

### Server-side check execution

stethoscope is a Rust + `axum` web service. A diagnosis is one HTTP request:

```
GET /diagnose?vendor=<v>            → render the page, return SSE event stream
GET /api/diagnose?vendor=<v>        → JSON verdict + per-check breakdown
```

When the page loads, the server:

1. Parses the URL target (`?vendor` / `?topic` / `?system`).
2. Pulls the **enabled check set** from runtime config + per-session operator toggles.
3. Filters checks by `applicable_to(target)` — a topic-target skips vendor-only checks, etc.
4. Runs the applicable passive checks in parallel; streams verdicts back via SSE as each completes.
5. If active probes are toggled on for this session, runs the active checks one-by-one after passives (active checks emit traffic onto the fabric — serializing them keeps the diagnosis attributable).

The page renders a card per check as each verdict streams in. Operator sees passive verdicts within ~200 ms, active verdicts within seconds.

**Vendor/system targets are anchor-aware.** On slot-pool goatnets (ADR 1025) wire keys lead with the UUID slot anchor (`<anchor>/<vendor>/…`), so a vendor name never appears as a first key segment and the portal files topics under anchor-named vendor rows. Every check that scopes by vendor therefore matches both namespace flavours via `panoscope_core::key_vendor_matches` / `strip_anchor`: the audit-tail filter passes events whose raw first segment OR anchor-stripped vendor segment equals the target; system-prefix matching compares anchor-stripped too; and registration/topic-roster additionally sweep the anchor-named vendor rows for topics whose vendor segment matches the handle (bounded by the assigned-slot count). Without this, a healthy slot-pool vendor reads flow-rate FAIL + "no topics declared" while panoscope shows its traffic live — F-177's silent-emptiness shape at the inspection layer. Distinct from ADR 1039 alias resolution, which canonicalizes the leading segment of *topic* targets only (a vendor handle is not a slot alias; one vendor can hold several slots).

### Plugin interface for checks

Each diagnostic is a self-contained Rust module conforming to a small trait. Adding a new check = one new file + one line in a registry. Removing a check = delete the file.

```rust
#[async_trait]
pub trait Check: Send + Sync {
    fn id(&self) -> &'static str;            // "registration", "decode-roundtrip", etc.
    fn name(&self) -> &'static str;          // human-readable
    fn kind(&self) -> Kind;                  // Passive | Active
    fn cost(&self) -> Cost;                  // None | NetworkRead | NetworkProbe | Compute
    fn applicable_to(&self, target: &Target) -> bool;
    async fn run(&self, ctx: &CheckCtx, target: &Target) -> Verdict;
}

pub enum Kind   { Passive, Active }
pub enum Cost   { None, NetworkRead, NetworkProbe, Compute(u32) /* est ms */ }
pub enum Target { Vendor(String), Topic(KeyPattern), System(String) }
pub struct Verdict {
    pub status: Status,                       // Pass | Warn | Fail | Skipped | NotApplicable
    pub summary: String,                      // one-line operator-facing
    pub details: Option<serde_json::Value>,   // structured drill-down
    pub findings: Vec<Finding>,               // optional advisories
    pub duration_ms: u64,
}
```

The `CheckCtx` carries handles the check needs: portal API client, MinIO audit-tail reader, netbird-mgmt client, optional `stethoscope` Zenoh session (only present when active probes are enabled), and per-target memoization cache (so e.g. "registered?" and "schema uploaded?" share one portal round-trip).

Findings are advisory — they don't change the Pass/Fail status but carry the "your expectations may not match reality" subtext (e.g. "topic was declared 31 d ago with zero events" attaches to a passing-but-flat Flow check).

### Modularity + toggleability

Three layers of on/off control, each addressing a different concern:

- **Deploy-time config** (`stethoscope.yaml` / env vars): the master enabled-set. A site that doesn't care about anomaly-detection can ship without those checks at all. Default-set = MVP-6 (below) + a small extension shelf.
- **Per-target applicability** (`applicable_to`): a check declares which target kinds it runs against. A topic-level "subscriber visibility" check skips vendor-target diagnoses. This is checks pruning themselves, not the operator.
- **Per-session operator toggle** (UI): the diagnosis page has a "configure checks" drawer. Each check renders as a row with on/off, a `[blood-dye]` badge if active-only, an estimated cost pill, and a short description. Operator flips off checks that are "taking up attention or compute and network that would be wasted" (the user's exact phrasing — the toggle is precisely for this case). State is per-session, in-memory; the next diagnosis starts from deploy-defaults.

Adding a new diagnostic capability is a one-file PR:

```
crates/stethoscope-checks/src/checks/<id>.rs   ← new module implementing Check
crates/stethoscope-checks/src/registry.rs       ← one-line addition
```

CI requires a fixture test per check (passing target, failing target, skipped target) so a check landing without a regression-test scaffold fails the build — same pattern as portal's `log_analyzer_test.go` signature-fixture rule.

### Data sources, not pollers

stethoscope reads from data sources sibling services already maintain — it does not duplicate scraping or replicate registries.

- **Portal registry** (`GET /api/vendors`, `GET /api/topics`, `GET /api/schemas/...`) — registration + schema-body + format declarations + descriptions.
- **MinIO `audit-integrity` bucket** (the Block 82S audit-subscriber's NDJSON) — flow rate, payload samples for decode, pub→audit latency, per-key_pattern histograms. Read via the same stdlib SigV4 substrate the portal vendor-progress tailer uses; deploy-time-gated by a read-only MinIO service user.
- **Panoscope `/api/snapshot`** — subscriber visibility, peer registry, per-peer last-seen, role classifications. stethoscope queries panoscope as a cooperating sibling; panoscope's snapshot is the authoritative who-is-out-there view.
- **netbird-mgmt** (`GET /api/peers`) — peer-name existence (closes the "is the vendor's netbird peer even on the mesh?" gap that L2 of progress.html answers).
- **Zenoh session** (active checks only) — present only when the operator toggles blood dye on; identity is the dedicated `stethoscope` cert (see below).

Each data source degrades to "skipped (data source unavailable)" when its env vars are unset, so a partial deploy renders a partial diagnosis without crashing. The one exception is a *contradiction*: declaring the audit tail enabled with an empty endpoint or credential is a configuration error, not a degrade. `AuditTail::new` and `Config::from_env` refuse it at startup, and the deploy play asserts against it before the role runs (ahead of the rollback-ledger row) and again as the role's first task, before `.env` is rendered, so a tail can never report as on while silently reading nothing.

## MVP-6 check catalog

These six ship in Phase C as the canonical first set. Each maps directly to an operator pain point.

| # | ID | Kind | Target kinds | Pain point |
|---|---|---|---|---|
| 1 | `registration` | Passive | Vendor, Topic | "is the data format and schema registered?" |
| 2 | `schema-body` | Passive | Topic | "is the schema *body* (`.fds`) uploaded? what version? what compatibility?" |
| 3 | `decode-roundtrip` | Passive | Topic | "can recent wire bytes be deserialized against the uploaded descriptor?" |
| 4 | `flow-rate` | Passive | Vendor, Topic | "is data flowing? how many messages per window?" |
| 5 | `subscriber-visibility` | Passive | Topic | "do declared subscribers exist? when did each last act?" |
| 6 | `pub-to-audit-latency` | Passive | Topic | "what's the publisher-ts → audit-tail-ts proxy latency?" |

Per-check behavior:

1. **Registration** — portal lookup. Verdict: exists / declared format / has description / has `schema_ref`. Pass = exists + format declared. Warn = exists but `schema_ref` empty. Fail = doesn't exist.
2. **Schema-body** — portal `GET /api/schemas/{key...}`. Verdict: any `.fds` uploaded / current version / compatibility level / last-upload date. Pass = `.fds` present + within compat policy. Warn = `.fds` uploaded > 90 d ago without bumps. Skipped = topic format isn't `protobuf:<MsgName>` (no `.fds` needed).
3. **Decode-roundtrip** — verdict: %-decoded out of N recent samples. Pass = ≥95%. Warn = 50-95%. Fail = <50% (raises *schema-doesn't-match-bytes*). Skipped = topic format isn't `protobuf:*`, or no payload-bearing samples to measure. **As-built (track `vendor-decode-rca`, 2026-05-28):** rather than pull audit-tail samples and decode locally via `dynamicpb`, the check calls `GET /api/topics/decoded?key=<key>&n=20` and counts the samples the portal decoded server-side against the declared `format` vs the ones it errored on — one round-trip, and the decode honors `format` uniformly with the rest of the portal. This check is the read-side guard that would have caught the vendor state where schema-body was green but `tracks`/`position_attitude` bytes didn't decode; failures link to the `vendor-decode-rca` diagnostic tool.
4. **Flow-rate** — audit-tail event count over 1m / 15m / 1h windows. Verdict: rate-per-window. Pass = ≥1 event in last 15m. Warn = events in last 1h but none in last 15m. Fail = no events in last 1h. Skipped = audit-tail data source disabled. **Finding** attached when a topic is registered > 30 d ago with zero lifetime events → *declared-but-never-published*.
5. **Subscriber-visibility** — panoscope `/api/snapshot` cross-referenced with the target topic's key_pattern. Verdict: subscribers listed with last-seen ts. Pass = ≥1 subscriber observed in last 15m. Warn = subscriber declared but no activity. Fail = no subscribers visible. **Caveat in the verdict:** Zenoh is stateless pub/sub — visibility ≠ delivery confirmation. The card carries a one-line "active echo probe confirms delivery" link to opt into Check #7 (deferred — see What's NOT in v1).
6. **Pub-to-audit-latency** — for each audit-tail sample, compare publisher's envelope ts (when present) to the audit-subscriber's ingest ts. Verdict: p50/p95 ms. Pass = p95 < 1000 ms. Warn = p95 < 5000 ms. Fail = p95 ≥ 5000 ms. Skipped = envelope ts not present. **This is a proxy** for pub→subscriber latency; true end-to-end latency requires the active echo path (deferred).

### Findings (auto-derived advisories)

A small set of cross-check inferences renders as findings on the relevant card:

- **declared-but-never-published** — Registration passes + Flow shows zero lifetime events + topic age > 30 d. Attaches to Flow.
- **schema-doesn't-match-bytes** — Schema-body has a current `.fds` + Decode-roundtrip < 50%. Attaches to Decode.
- **stale-schema** — Schema-body's last-upload > 180 d while live publishes continue. Attaches to Schema-body.
- **publisher-without-subscribers** — Flow passes + Subscriber-visibility fails. Attaches to Subscriber-visibility.

Findings carry severity (`info` / `warn`) but never independently fail a check. They are the "do my expectations make sense?" half of the surface.

### Extension shelf

A second set of checks ships with the v1 binary but is **default-off** in the deploy config — operators flip them on per-site. The shelf is where new diagnostic capabilities land before promotion to the default set:

- `wire-format-vs-declared` — sample wire bytes against the topic's declared format string. Warn when declared `protobuf:Foo` but bytes start with `{` (likely JSON), or vice versa.
- `audit-coverage` — confirms the audit-subscriber is seeing any traffic for `<vendor>/**` (catches "is the per-vendor storage stanza in the router config?" gaps like F-146).
- `peer-name-exists` — netbird-mgmt confirms `peer-<vendor>-*` is on the mesh (L2 of Block 88 progress).
- `cert-uri-san-matches-publishes` — audit-tail's `src_zid` cross-checked against the vendor's URI-SAN authority. Catches publishes-from-wrong-identity.

A check on the shelf can be promoted to the default set when it proves useful across three diagnosis sessions without false positives. Demotion is symmetric.

## Active probe model

Passive checks are always-on. Active checks — the "blood dye" — are **off by default** and require operator action to enable.

### Why opt-in

Active probes inject traffic onto the fabric. They consume bandwidth, get audited, and (for some probe shapes) appear in subscribers' views. Running them as background telemetry would (a) burn budget the operator may not have, (b) clutter audit logs with not-real-traffic, (c) potentially affect downstream consumers expecting only real publishes. The user's framing — "allow it all, but not by default, only turned on for diagnosis" — is the right call.

### Identity + capability

A dedicated `stethoscope` cert under offline-CA holds the active-probe identity. Its URI-SAN authority is `stethoscope` (added to the reserved prefix list alongside `system` + `attest`). The Zenoh ACL grants `stethoscope` allow-write on a new reserved prefix:

```
diagnose/<vendor>/<probe-id>/v1
```

Vendors get an automatic allow-read on `diagnose/<their-vendor>/**` so they observe probes targeting them (transparency by construction). Cross-vendor probe injection isn't possible — the URI-SAN gate refuses any publish outside `diagnose/<v>` to the wrong vendor.

This is one fleet-wide ACL change (one new allow stanza for `stethoscope`, one new vendor-template allow-read pattern), not a per-vendor stanza addition. Existing vendors don't need their ACL touched.

### Active checks (v1 set)

- **synthetic-publish-trace** — stethoscope publishes a tagged message on `diagnose/<v>/<probe-id>/v1`; audit-tail confirms the message landed; latency measured from publish → audit ingest. Functional equivalent of an end-to-end "did anything make it" probe without touching the vendor's own namespace.
- **echo-loopback** — stethoscope subscribes on `<v>/system/echo/response/v1`, publishes a tagged challenge on `<v>/system/echo/request/v1`, waits for the vendor's echoer (per the portal echo-loopback stub design) to reply. Verdict: round-trip latency + reply received y/n. Skipped if the vendor doesn't implement an echoer.
- **queryable-aliveness** — Zenoh queryable on `diagnose/<v>/probe-aliveness/v1` — anyone declaring this queryable on the vendor side responds; absence = no aliveness check available. Optional.

Each active probe is its own audit event with `audit_subscriber_id=stethoscope-probe` and a `diagnosis_session_id` UUID, so the diagnosis itself is fully traceable. Operators can `grep diagnosis_session_id` in the audit bucket to reconstruct any probe-injected diagnosis.

### Operator UX for blood dye

The diagnosis page has a top-bar toggle: **`Blood-dye probes: OFF`** (default) / **`Blood-dye probes: ON for this session`**. Flipping ON expands a sub-drawer listing the available active checks for this target with per-check toggles + cost pills. Operator confirms; active probes run after passives. The toggle reverts to OFF when the page is closed — there is no persistent "always probe" state.

A site that wants to disable active probes entirely (or that runs without the `stethoscope` cert deployed) renders the toggle as disabled with a one-line "active probes not provisioned at this site" hint.

## Reusable crate structure

Two new shared crates land as part of this work. Each has a single responsibility and is independently versioned.

### `panoscope-core` (factor-out from existing `ops/panoscope/`)

Already planned by flashlight's Phase B. stethoscope depends on the same crate for: Zenoh client setup, audit-event parsing, peer registry data structures, MinIO replay machinery. **The factor-out is the same factor-out.** stethoscope does not duplicate; it lands AFTER `panoscope-core` exists and lifts those substrates verbatim.

### `stethoscope-checks` (new)

Houses the plugin trait, the registry, and the per-check modules. Native-only — no WASM target (unlike `flashlight-analytics`). Capabilities (v1 scope):

- Plugin trait (`Check`, `Verdict`, `Target`, `CheckCtx`).
- Registry (`Registry::register(check)`, `Registry::applicable_for(target)`).
- MVP-6 + extension-shelf check modules.
- Per-check fixture-test scaffolding.

`stethoscope` (the binary) depends on `stethoscope-checks` + `panoscope-core` + a thin axum HTTP layer.

### What stays NOT in shared crates

Stethoscope-specific surface code lives in the stethoscope crate alone: HTML/CSS/JS assets, the SSE event shape, the per-session toggle state, the report rendering. Panoscope's D3 / hull rendering and flashlight's Tier 2 WASM boot logic stay in their respective crates.

## Deployment shape

stethoscope ships as a standalone service following the canonical mgmt-host pattern. Same shape as panoscope and flashlight.

- **Image:** `ghcr.io/usareur-af/stethoscope` (Chainguard Rust → static distroless, multi-arch arm64+amd64), built by `.github/workflows/stethoscope-image.yml` on every merge that touches the crate or assets.
- **Ansible role:** `ops/ansible/roles/stethoscope/` — image pin + runtime ledger (per ADR 0859) + compose overlay + env-var threading for portal API key, MinIO read-only creds, panoscope snapshot URL, netbird PAT, and (when active probes are provisioned) `stethoscope` Zenoh cert paths.
- **Compose:** `ops/compose/mgmt-host/docker-compose.stethoscope.yml`. Single container. Reuses existing bind mounts for offline-CA root + the `stethoscope` leaf cert (when present).
- **Traefik router:** mesh-only allowlist matching the panoscope shape. Adds `stethoscope.nova.netbird.nova.example` SAN to the mesh-only certificate.
- **Mesh DNS:** `stethoscope.nova.netbird.nova.example` → nova-a mesh IP via CoreDNS (`ops/dns/nova/mesh-hosts`).
- **isitup row:** `ops/ansible/group_vars/nova_hosts/vars.yml` `isitup_services` list, classification `public` (mesh-internal, no admin gate — same as panoscope).
- **Apply playbook:** `ops/ansible/stethoscope.yml`.
- **Rollback:** `ops/scripts/rollback.sh stethoscope [N]` per ADR 0859. Tier-0 adopter from day one.

Active-probe provisioning is a separate operator step (mint the `stethoscope` cert under nova offline-CA + register the `diagnose/**` ACL stanza + drop the cert files into the host bind mount). Without those steps the binary still runs and renders passive checks; the blood-dye toggle just disables itself.

Production hosts inherit the role with their own `*_hosts/stethoscope.yml` group_var; deferred until nova-a live-fire validates the surface.

## Phased build plan

Each phase lands as one or more PRs with the doc/in-flight delta in the same change-set per documentation discipline.

### Phase A — Planning (this PR)

Four artifacts in one PR. **No code.** This is the change-set you're reading.

- `docs/design/stethoscope.md` (new — this doc).
- `docs/project/in-flight.md` Block 90 entry (new).
- `CLAUDE.md` Data substrates pointer (alongside panoscope / flashlight).
- `docs/project/active-work/tracks/stethoscope.yaml` (new track).

Gate to Phase B: operator review + approval of this design doc.

### Phase B — Plugin substrate + first three passive checks

New `crates/stethoscope-checks/` with the trait + registry + fixture-test scaffolding. New `crates/stethoscope/` binary with axum HTTP layer + URL parsing + SSE streaming + the page renderer. Three passive checks land: `registration`, `schema-body`, `flow-rate`. Skeleton page renders a working diagnosis against nova-a. No active probes.

Visual reference for the page renderer: the design-tool wireframe bundle at `../design-assets/stethoscope/` — wireframe-level HTML/CSS/JS prototype covering the 13 states the page needs to handle, the streaming-in interaction beats, the configure-checks drawer, the blood-dye opt-in sequence, and per-card drill-down drawers. Read `../design-assets/stethoscope/INSTRUCTIONS-FOR-IMPLEMENTING-AGENT.md` first — it ties the bundle back to this design doc and names what's normative versus what's reference-only.

Depends on Phase B of flashlight (`panoscope-core` factored out) — if flashlight's Phase B hasn't landed yet when stethoscope reaches Phase B, the factor-out lands in stethoscope's PR instead. Either path is fine; the crate's home is the same.

Gate to Phase C: live-fire on nova-a, operator points stethoscope at three different vendors, page renders verdicts that match reality.

#### Phase B as-built (2026-05-26)

Landed:

- **`crates/` standalone workspace** at the repo root — sibling-of-`ops/panoscope/`, not a member of the `goat-relay` root workspace. Trust-domain separation + build isolation. Members: `panoscope-core`, `stethoscope-checks`, `stethoscope`.
- **`crates/panoscope-core/`** (v0.1) — narrow first-instance substrate: `portal::PortalClient` (list_topics / get_topic / list_schemas / fetch_fds against goat-zenoh-portal), `audit_event::AuditEvent` (mirrors the Block 82S writer shape), `audit_tail::AuditTail` (generic S3 NDJSON walker over `audit-integrity`). Panoscope's existing code was **not** migrated in this PR — see "Deliberate deviation from Phase A planning" below. **Closed 2026-05-27:** PR #522 (flashlight Phase B) moved the broader substrate (peer registry / role taxonomy / Zenoh TLS config) into the same crate; PR #527 (panoscope-migrate-to-panoscope-core) finished the panoscope-side consolidation by replacing the inline S3 walker with `panoscope_core::AuditTail`. Panoscope is now end-to-end on the shared substrate.
- **`crates/stethoscope-checks/`** — `Check` trait + `Verdict` + `Finding` + `Registry`. Registry produces `FuturesUnordered<BoxFuture<CheckOutcome>>` driven on the calling task, **no `tokio::spawn`** (repo `clippy.toml` forbids per ADR 0709 D7 layer 3). Three checks: `registration`, `schema-body`, `flow-rate`. Each has a fixture-test pair (CI gate).
- **`crates/stethoscope/`** — axum binary, server-rendered HTML, SSE diagnose stream, ~150-line vanilla-JS client. Block 91 design-system tokens embedded at compile time via `include_str!` from `docs/design-system/`. URL-parameterized target model (`?vendor=` / `?topic=` / `?system=`). Phase B skips blood-dye + configure-checks + drill-down drawers (those Phase C+D surfaces ship behind the same DOM structure when their checks land).
- **Deploy substrate**: `ops/ansible/roles/stethoscope/` (defaults + tasks + templates + handlers + `deploy-history.yml` per ADR 0859) + `ops/ansible/stethoscope.yml` + nova-a `group_vars/nova_hosts/stethoscope.yml` + mesh DNS row + isitup row + `.github/workflows/stethoscope-image.yml` building `ghcr.io/usareur-af/stethoscope`.
- **New findings vs the original MVP-6 list** (both surfaced experientially by a vendor schema RCA):
  - **`published-but-never-declared`** on the `registration` check (vendor target) — dual of `declared-but-never-published`. Cross-references live audit-tail vendor keys against the registry. The vendor's `acme/radar1radar/...` (in audit) vs `acme/radar1/...` (in registry) is exactly this shape.
  - **Schema-body name-vs-FDS sub-check** — for `format: protobuf:<MsgName>` declarations, verify the named message exists as a top-level message in the uploaded `FileDescriptorSet`. The vendor's `protobuf:MyMessage` (where the FDS contained `Status` / `SoftwareVersion` / `StatusFlags` / `TopLevelBits` but no `MyMessage`) is exactly this shape.

Live-fire validated:

- `GET /api/diagnose?vendor=acme` → `registration: PASS — Vendor 'acme' exists with 3 topics.`
- `GET /api/diagnose?topic=acme/radar1radar/RADAR-EXAMPLE/tracks` → `registration: FAIL — Topic ... is not registered under vendor 'acme'.` (catches Finding 1 of the RCA structurally)
- `GET /api/diagnose?topic=acme/radar1/RADAR-EXAMPLE/tracks` → `schema-body: FAIL — Format declares 'protobuf:MyMessage' but the uploaded .fds contains no top-level message named 'MyMessage'. Found: Status, SoftwareVersion, StatusFlags, TopLevelBits.` (catches Finding 3 of the RCA structurally)

Tests: 37 unit tests pass (6 panoscope-core + 12 stethoscope-checks + 19 stethoscope). `cargo clippy --workspace --all-targets -- -D warnings` clean. Integration tests + criterion benches + Playwright e2e tests queued for follow-on PRs per the higher engineering bar (excellent code / tests / performance tests / eventually e2e tests).

**Live 2026-05-27 at `https://stethoscope.nova.netbird.nova.example/`.** The first published image carried a stub binary (335 KB hello-world from a cargo-cache-warming Dockerfile pattern that produced a freshness-check cache-hit on the second build, so the `cp` step copied the stub instead of the real artifact); the rebuild after the Dockerfile collapsed to a single-RUN COPY-then-build pattern with a binary-size smoke check at the end. Container responds `stethoscope 0.1.0 ok` on `/healthz`, isitup check loop confirms HTTP 200, panoscope + portal + audit-tail substrates are all reachable from inside the container.

##### Target picker — ported from the design bundle after Phase B live-fire

The initial Phase B implementation lit up the designed diagnosis page when given a target via URL (`?vendor=` / `?topic=` / `?system=`) but skipped its `TargetPicker` popover (`docs/design-assets/stethoscope/project/app.jsx:54-118`). With the picker missing, the empty-target URL state `?` rendered as a stub error message — "no target — pass exactly one of …" — which made the surface unusable as a destination unless the operator already knew the exact target string. The design brief had carved out a "no browse home screen — the URL is the API" position, but the prototype's actual TargetPicker bridges that gap: a popover labeled **"Place the stethoscope on…"** (the operator's exact metaphor) with an input + Read button, Topic/Vendor/System kind chips, a "Recent" presets list, and Enter/Esc keyboard handling.

The picker now lives on every page, in two modes:

- **Closed (on the diagnose page)** — anchored under the hero target value, hidden until the operator clicks the `change ›` affordance, scrim closes it on outside click. Per `app.jsx:138-142`.
- **Landing (URL state `?`)** — mounted open in-flow with no scrim, no animation flash, input auto-focused. The URL state `?` now means "I haven't decided yet — help me pick," not "you misused the API."

Recent targets persist in `localStorage` under `stethoscope.recent` as a `[{kind, value}]` array (dedup on `(kind, value)`, cap 5, push on successful navigation). No backend catalog endpoint in v1 — the design uses free-text + local recents only; a v2 follow-on can add a catalog typeahead against `panoscope_core::PortalClient::list_topics` if operators need fuller discoverability.

Implementation is a faithful port to the production Rust+axum+vanilla-JS stack: CSS lifted verbatim from `docs/design-assets/stethoscope/project/stethoscope.css:172-335` (Block 91 tokens resolve directly), HTML structure shaped by `render_picker_html(current_kind, current_value, mode)` in `render.rs`, behavior in ~120 lines of vanilla JS appended to `assets/stream.js`. Routes return 200 for the missing-target path (it's a real page now); `Multiple`/`Empty` still 400.

##### Picker autocomplete + auto-detect (live-fire follow-on, 2026-05-27)

The first live-fire test of the picker surfaced two gaps in one diagnose attempt: an operator typed `acme` (the vendor name they were investigating) and got "no topic named `acme`" because the picker submitted `acme` as a topic (the default kind). Accurate but unhelpful — the operator's mental model is "type the thing you want to look at," and asking them to pre-classify the thing as vendor / system / topic adds a step where it isn't needed. Three additions to fix:

- **Auto-detect kind from input shape** on submit: no slash → vendor; 1 slash → system; 2+ slashes → topic. Matches the portal's `key_pattern` convention. Operators who *want* explicit control click a kind chip — that pins the kind and disables auto-detect for the rest of the picker session.
- **Default chip stays `Topic`** with chip order `Topic / Vendor / System`. Autocomplete searches topics dynamically as the operator types; if they submit without typing past the first slash segment, auto-detect downgrades to a vendor lookup. (An earlier iteration of this PR briefly flipped the default to Vendor; that reversed the desired discovery flow — type the specific thing first, fall back to vendor only when the input shape says vendor — and was reverted in the follow-on fix.)
- **Live typeahead dropdown** populated from a new `/api/index` endpoint: `{vendors, topics, systems}` JSON, cached in-process for 30s. Filters on every keystroke (case-insensitive substring match), capped at 20 results with a `+N more — keep typing to narrow` indicator below. **Breadth-first ordering** across `(kind, first-segment)` groups so the dropdown shows diverse roots before going deep into any one — typing `r` shows one or two entries from each matching vendor before drilling deeper, matching the operator-visible mental model of "I have many places to start, show me variety not just `acme/...` × 20."
- **Bare-word topic gets a clearer error.** When the operator submits a topic value with no slash (e.g. `acme`), Registration returns `'acme' isn't a topic key — topic keys are shaped <vendor>/<topic>/<version>. Did you mean the vendor 'acme'?` rather than the tautological "Topic 'acme' is not registered under vendor 'acme'" that fell out of `vendor_prefix()` returning the whole string when no slash is present.

> **Shipping note (2026-05-27 evening live-fire).** The first deploy of the autocomplete work had two regressions: the default chip was flipped to vendor (above), and the typeahead dropdown never appeared because the JS cleared the inline `display` style with `''`, leaving the stylesheet's `display: none` in effect — the dropdown needs an explicit `display: flex`. Both fixes are part of the same Phase B as-built.

The `/api/index` snapshot serializes vendors via `PortalClient::list_vendors()` (added to panoscope-core for this PR — the portal already exposed `GET /api/vendors`, the client just didn't bind it), and synthesizes systems from topic keys (`vendor/<first-segment-of-suffix>` for each topic) until the portal grows a dedicated systems table. Portal-disabled installs return `{portal_disabled: true}` with empty arrays; the picker labels the dropdown "autocomplete unavailable — portal not configured" rather than looking broken.

What this means in practice: operator hits the landing page, types `acme`, sees `acme` (vendor) at the top of the dropdown plus any topics under `acme` — and hitting Enter routes to `/?vendor=acme` not `/?topic=acme`. The original vendor RCA flow that triggered this work is now a ~5-second navigation from picker → vendor → drill into the failing topic card.

##### Topic-roster + cardinality + remediation + nav (live-fire follow-on, 2026-05-28)

Second live-fire of the picker (sha-33928da deployed on nova-a) surfaced four gaps that the picker alone couldn't close — they're about what the *cards* say once the operator lands on a target. Bundled into one PR because they share substrate (remediation + clickable findings) and rely on each other for the end-to-end vendor RCA story to land on the vendor surface.

- **`topic-roster` check** (vendor + system applicability). Walks each registered child topic, runs `schema-body` inline per child, and emits one Finding chip per topic carrying the child's verdict color (Pass = green, Warn = amber, Fail = red, Skipped = blue) plus an `href` deep-link to `/?topic=<key>`. Roll-up is worst-wins so the vendor card itself is FAIL when any child FAILed. This is the gap that hid the vendor RCA from the vendor view: the existing `registration` check at vendor level is shallow ("Vendor 'acme' exists with 3 topics") and `schema-body` is topic-only, so a vendor whose every child topic FAILed schema-body still rolled up to PASS. Topic-roster turns the vendor card into a one-screen summary of all child topic state.
- **`cardinality` check** (vendor + system applicability). Reads the last 24h of audit-tail events under the target, groups by key_expr, computes (distinct_topics, total_events, median events_per_topic). Verdict is PASS for low cardinality (≤ 20 distinct), PASS for healthy bounded fan-out (median > 1 event/topic), WARN for the fan-out anti-pattern (median ≤ 1 event/topic AND distinct > 20). The WARN case maps to this shape: many distinct `example-vendor/entities/v3/<uuid>` topics each carrying ~1 sample before the UUID rotated. Per [`docs/design/pubsub-best-practices.md`](https://github.com/usareur-af/goatcore/blob/main/docs/design/pubsub-best-practices.md) §60 ("one topic per logical entity, stable key per entity"), per-entity fan-out is fine when entities are long-lived and stable; per-event fan-out is the failure mode. Skipped if audit-tail isn't configured. Remediation carries two steps — one for "if accidental, consolidate" pointing at canon, one for "if deliberate, confirm" so the check is operator-resolvable rather than operator-pinging.
- **Remediation substrate.** `CheckOutcome` gains `remediation: Vec<Remediation>` and each `Remediation` carries (title, detail, optional href). Stream.js renders an `sx-remediation` block under Fail/Warn cards (suppressed on Pass/Skipped/NA where remediation makes no sense). Concrete remediations wired into the existing failure modes — schema-body name-mismatch suggests the most-likely correct format declaration based on the uploaded schema's top-level messages, and the schema-upload remediations offer both portal-accepted paths per [`portal-schema-publishing.md`](https://github.com/usareur-af/goatcore/blob/main/docs/design/portal-schema-publishing.md) §5.1: `.proto` source compiled server-side via bufbuild/protocompile (F-158, default easy path for single-file schemas with `google/protobuf/*` imports), or precompiled `.fds` via `protoc --include_imports` (the multi-file fallback). Registration topic-not-registered carries the exact portal POST template + the existing registered key_patterns so the operator can choose registering vs. re-keying; registration vendor-404 carries the vendor POST template; flow-rate FAIL carries two paths (publisher process check, then cert+ACL check — silent ACL drops look identical to a stopped publisher at this layer); the bare-word topic mismatch carries a one-click vendor-switch href. The pattern is verbing title + monospace detail with the exact command, so an operator can act without leaving the diagnose surface.
- **Nav affordances.** STETHOSCOPE brand becomes a clickable home link on every render path. Topbar crumb walks the parent chain — vendor target shows just site, system target adds `/ <vendor-link>`, topic target adds `/ <vendor-link> / <system-link>` — so the operator can walk up the hierarchy from a topic FAIL through its system and vendor. `‹ back` chip in the topbar right falls through to `history.back()` (or `/` if there's no history). Composes with the brand link: one-step back vs. all-the-way-home is operator choice. Landing skips the back button.
- **Dropdown stays quiet on empty input.** The first cut auto-populated the index on focus, which covered the kind chips and Recent list with hundreds of names the operator wasn't looking for. Now it short-circuits when value is empty; suggestions appear once a needle exists.

> **Operational note (2026-05-28, pre-refusal behaviour; an enabled tail with an empty secret is now refused at deploy and at startup).** The `flow-rate` check on nova-a had been universally Skipped because `NOVA_STETHOSCOPE_AUDIT_S3_SECRET_KEY` was missing from the operator's `30-site-services.envrc` — the `minio_extra_users` substrate mints the user from that env var on each apply, and the stethoscope role threads the same secret into the container. The slot was added (alongside `NOVA_STETHOSCOPE_AUDIT_S3_ACCESS_KEY=goat-stethoscope-audit-read`); after the next minio + stethoscope re-apply, flow-rate runs against live audit-tail data and the cardinality check has a populated working set to judge. The fix is documented in the envrc itself; new operators with a fresh envrc should generate a secret with `openssl rand -hex 24` per the comment in `ops/ansible/group_vars/nova_hosts/minio.yml`.

##### Deliberate deviation from Phase A planning

The design's Phase B narration said the `panoscope-core` factor-out could land here OR in flashlight Phase B. **Stethoscope Phase B lands a narrow factor-out — only the shared substrate stethoscope needs (portal + audit-event + audit-tail).** Panoscope itself remains untouched in this PR. Two reasons:

1. **Mixed-hat structural unreviewability.** Bundling a full panoscope refactor with new-feature work in one PR violates the project's two-hat discipline. The reviewer can either audit "new stethoscope behavior" or "panoscope behavior preserved through factor-out" — not both at once without inflating cognitive load past where review quality holds.
2. **Rollback granularity.** Per ADR 0859, each service rolls back independently. A combined PR would couple panoscope's and stethoscope's image-SHA bumps; pulling one back would force pulling the other.

The remaining factor-out (panoscope's `zenoh_client.rs`, `model.rs`, `decoder.rs`, `schema.rs` → `panoscope-core`) lands as a separate behavior-preserving PR. Flashlight Phase B's `panoscope-core` work composes with that PR transparently.

**Closed 2026-05-27.** PR #522 (flashlight Phase B, merged 2026-05-27) absorbed peer-registry + role taxonomy + key-expression helpers + Zenoh TLS config into `panoscope-core`. PR #527 (panoscope-migrate-to-panoscope-core, merged 2026-05-27) finished the consolidation panoscope-side by replacing the inline S3 walker in `minio_replay.rs` with `panoscope_core::AuditTail` and dropping the now-transitive direct deps (rusty-s3, quick-xml, url, chrono). The deliberate-deviation footnote above is preserved as the historical record of why the two waves landed separately, but the gap it described is closed.

### Phase C — Remaining MVP-6 + extension shelf + findings

Adds `decode-roundtrip`, `subscriber-visibility`, `pub-to-audit-latency` + the four extension-shelf checks + the auto-derived-finding engine. Cross-link buttons land in portal + panoscope. Stethoscope is now the answer to "is this target healthy?" for any vendor/topic.

Gate to Phase D: live-fire surfaces at least three real "do my expectations make sense?" findings (declared-but-never-published, schema-doesn't-match-bytes, publisher-without-subscribers) against actual nova-a traffic.

### Phase D — Active probe substrate

Mint `stethoscope` offline-CA cert. Add the `diagnose/<v>/**` reserved prefix to the ACL substrate. Wire the `stethoscope` Zenoh session. Land `synthetic-publish-trace` + `echo-loopback` + `queryable-aliveness`. The per-session blood-dye toggle lights up in the UI.

Gate to canon: live-fire on nova-a; operator toggles blood-dye, three active probes complete with verdicts, audit log shows the probe traffic with `diagnosis_session_id`.

## What's NOT in v1

Explicit non-scope to preserve the "lean by construction" property:

- **Persistent diagnosis history.** v1 surfaces the *current* verdict per check; an operator who wants a history rerun and saves the URL. v2 may add a `diagnosis_runs` table keyed on session UUID for cross-time delta views.
- **Operator-authored expectations / SLOs.** v1's findings come from structural inference only. v2 may accept per-topic / per-vendor SLO YAML at the portal layer and surface SLO breaches as additional findings.
- **Delivery confirmation without active probes.** Without the blood-dye toggle, stethoscope can show subscribers exist but cannot confirm they're receiving the publisher's specific messages. v1 documents this limitation on the Subscriber-visibility card and offers the echo-loopback as the active-probe answer.
- **Derived-topic chain checks.** "Translator publishing on `derived/<topic>` consumes `<source>/<topic>`" — v1 has no way to know that mapping unless the portal grows a `derived_from: [<topic_ids>]` field on the topic row. Recorded as Phase E candidate; not in MVP-6.
- **Mutating actions.** stethoscope is read-only and probe-only. It never PATCHes the portal, never publishes outside `diagnose/**`, never advises a vendor through the wire. Operator-mutating surfaces stay on the portal + id-admin-console.
- **Authentication beyond mesh-only allowlist.** v1 trusts the mesh perimeter like panoscope. If a future deployment context needs OIDC group-gating, the oauth2-proxy substrate is the integration point — but not v1.

## Open questions

Recorded so they're addressed before Phase B lands, not after:

- **Cost budget per diagnosis.** A vendor with 50 topics + active probes enabled could fan out into 300+ check runs. Hard cap per diagnosis (e.g. max 30 checks, prefer Topic over Vendor scope when the target is broad) is plausible but not specified yet.
- **Audit-tail read amplification.** Decode-roundtrip + Flow-rate + Pub-to-audit-latency all read the audit-tail. Sharing one read pass via `CheckCtx` memoization is in the design, but the cache invalidation policy is "per-session, no cross-session" — a busy stethoscope hitting MinIO repeatedly could become a noisy neighbor to the audit-subscriber. Probably fine for trial scope; worth measuring before prod.
- **Stethoscope-on-stethoscope.** "Is the stethoscope service itself healthy?" — recursive diagnosis is meaningful (its data sources can be down, its `stethoscope` cert can be missing). v1 covers this only via the standard isitup card. A `?service=stethoscope` self-target case would let stethoscope diagnose itself; deferred to v2.
- **Finding rot.** Auto-derived findings get stale (e.g. *declared-but-never-published* should clear once a publish lands). v1 re-computes findings on every diagnosis; no caching. If finding storage ever happens (v2), invalidation needs explicit thought.

## Living document

Updates land via PR. The first material revision target is when Phase C ships and an operator has actually used stethoscope to triage a real onboarding-stalled vendor.
