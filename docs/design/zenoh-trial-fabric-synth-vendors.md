# Synthetic vendor publishers for the nova Zenoh trial fabric

| Field | Value |
|---|---|
| Status | **Current** — collapsed to single `notional` vendor / 22 peers (19 publishers + 3 sinks) on 2026-05-19 (PRs #225, #231, and the schema-body uploads in PR-C). The historical 4-vendor / 7-publisher shape described below is retained for reference, but the live deployment + portal vendor registry both run the notional shape now. |
| Scope | Single `notional` vendor identity, 22 peers spanning sensors / effectors / data services / C2 / observer / sinks. One cert (`notional-trial-peer`), one ACL allow rule (`notional/**`), one rocksdb storage prefix. All run as containers on nova-a; trial-scope only; teardown at trial sunset. |
| Composes with | [`zenoh-trial-fabric.md`](https://github.com/usareur-af/goatcore/blob/main/docs/design/zenoh-trial-fabric.md) (the substrate they publish into); `panoscope.md` (the visualization they make non-empty); `nova-x509-identity.md` (the offline-CA-rooted leaf cert) |
| Related | Block 82Q follow-on register at `block-82q-panoscope-followons.md`; F-125 (wire-validates the per-vendor write-scope claim as a side-effect) |

## Current shape (2026-05-19)

22 peers running on nova-a under one cert identity `notional-trial-peer`, publishing under a single `notional/**` namespace. The per-peer animal axis lives in the topic path rather than splitting across multiple vendor prefixes.

| Role class | Peers | Personality | Topic shape | Count |
|---|---|---|---|---|
| Sensor | tarsier-alpha, tarsier-bravo, mantis-alpha, echidna-alpha, kingfisher-bravo, axolotl-charlie | tracks-demo | `notional/<peer>/<fob>/<thing>/v1` | 6 |
| Effector | mongoose-alpha, wolverine-alpha, ibex-alpha, aurochs-charlie, basilisk-alpha | power-demo | `notional/<peer>/<fob>/<thing>/v1` | 5 |
| Data service | octopus-alpha, cuttlefish-theater, chameleon-translator | tracks-demo | `notional/<peer>/<scope>/<thing>/v1` | 3 |
| C2 | condor-theater, peregrine-alpha, falcon-bravo | power-demo | `notional/<peer>/<scope>/orders/v1` | 3 |
| Observer + narwhal | omniwatch (wildcard-subscribes to `**`), narwhal-alpha (events) | beacon-demo / power-demo | `notional/<peer>/theater/<thing>/v1` | 2 |
| Sink (no_publish=true) | glass-display, archive-vault, audit-log | beacon-demo (declared, never publishes) | `notional/<peer>/theater/unused/v1` | 3 |

Three personality schemas drive the data dictionaries: `notional.tracks_demo.v1.Entity`, `notional.power_demo.v1.PowerState`, `notional.beacon_demo.v1.Hello`. Source `.proto` at [`ops/zenoh-trial-peers/synth-vendor-schemas/proto-notional/`](../../ops/zenoh-trial-peers/synth-vendor-schemas/proto-notional/); the protobuf-FDS descriptor sets are uploaded to the portal under each of the 22 topics so the portal `/schema.html` viewer + per-language autogen code surfaces render.

Topic `format` field on every notional topic is `protobuf:<MsgName>` (e.g. `protobuf:notional.tracks_demo.v1.Entity`) so the portal's schema-viewer + per-language autogen-code surfaces render against the uploaded protobuf-FDS. **Wire format on nova-a is now protobuf-encoded** (changed 2026-05-24, track `panoscope-decoded-fix-and-toolkit`) — the synth-vendor binary's three notional render functions (`render_tracks_demo` / `render_power_demo` / `render_beacon_demo`) call `prost::Message::encode_to_vec` against the generated Rust types from `proto-notional/*.proto`, replacing the prior `serde_json::to_vec`. Topic format declarations + uploaded `.fds` bodies now match what's actually on the wire, so panoscope's decoded-stream view + the portal inspector both decode cleanly.

### RCA (2026-05-24): the JSON-on-wire-vs-protobuf-format-declaration gap

The mismatch was a deliberate documentation-only protobuf scaffolding that predated the panoscope decoded-stream view (PR #474). Five layers contributed:

| Layer | What it said | Posture |
|---|---|---|
| Synth-vendor binary render functions | `serde_json::to_vec(&body)` | Original design — JSON-only |
| `.proto` source comments | `Wire format on the fabric: JSON.` + `future protobuf wire alternative` | Deliberate scaffolding |
| `bootstrap-notional-portal.sh` | `format: "protobuf:<MsgName>"` | Light up portal schema-viewer + autogen-code |
| `bootstrap-notional-schemas.sh` | Upload `.fds` to portal | Light up portal schema-viewer |
| Panoscope decoded-stream (yesterday) | Read `format` → fetch `.fds` → decode wire bytes | First surface that read the chain end-to-end |

Not a bug in recent engineering — the gap was called out in CLAUDE.md §Notional from the start as "A future expansion flips the wire format to protobuf to close the gap." The decoded-stream view simply made the gap operator-visible. Fix is at layer 1, leaving layers 2-4 unchanged. Drift between vendored `proto-notional/` (under the synth-vendor crate) and the canonical schemas dir (`synth-vendor-schemas/proto-notional/`) is kept in sync by hand for now; a CI diff lands as a follow-on if drift bites.

Ansible vendor list in `ops/ansible/group_vars/nova_hosts/synth-vendors.yml`. Role template at [`ops/ansible/roles/zenoh-synth-vendors/`](../../ops/ansible/roles/zenoh-synth-vendors/) renders the 22-service compose. Portal seeding scripts: [`ops/zenoh-trial-peers/bootstrap-notional-portal.sh`](../../ops/zenoh-trial-peers/bootstrap-notional-portal.sh) (topic registration) + [`ops/zenoh-trial-peers/synth-vendor-schemas/bootstrap-notional-schemas.sh`](../../ops/zenoh-trial-peers/synth-vendor-schemas/bootstrap-notional-schemas.sh) (schema upload). Both idempotent.

## ZAGROS deployment — per-uuid slot-pool onboarding (F-189)

The nova-a shape above is **flat-vendor**: all 22 peers share one `notional` cert and publish under `notional/**`, and the portal gets one `notional` vendor row from `bootstrap-notional-portal.sh`. **ZAGROS runs per-uuid-slot-pool identity ([ADR 1025](https://github.com/usareur-af/goatcore/blob/main/docs/adr/1025-zenoh-dynamic-tenant-authz-slot-pool.md))**, where a system's write anchor = its cert CN = an allocated `slot_id = sha256(salt:k)[:32]`. The flat path does not fit: each synth **peer** needs its own slot + cert + alias, and topics must be `‹anchor›/‹animal›/…` not `notional/‹animal›/…`.

The first ZAGROS standup reused the nova flat automation, so the fleet published under pool slots it had minted out-of-band but was **never onboarded in the portal** — zero vendor/alias/topic/schema rows. panoscope's decoder (`panoscope-core` `PortalClient::find_topic_by_key`/`fetch_fds`) splits the leading anchor segment as the vendor, 404s, and reports `no_schema` ("No schema uploaded …"); the fleet also rendered as raw UUIDs (no alias). F-189 root-causes it.

The fix is [`ops/ansible/zenoh-synth-vendors-slot-pool-deploy.yml`](../../ops/ansible/zenoh-synth-vendors-slot-pool-deploy.yml), which onboards each peer and deploys in one idempotent run:

1. **Onboard (pre_tasks).** For each `synth_vendors` entry whose handle (`id`) has no anchor yet, `POST /api/vendors {prefix: ‹id›, synthetic: true}` — in slot mode the portal allocates a free slot, mints a `CN=anchor` cert, and writes the vendor + alias rows. The one-time bundle tarball is fetched + extracted into `certs/‹anchor›/` (the tarball's `‹anchor›-cert.pem` lands exactly where the role expects `{cert_dir}/{vendor}/{vendor}-cert.pem` with `vendor=‹anchor›`). Idempotency state = the portal's own `/api/aliases` ⊕ a durable `synth-anchors.json` on the host; an already-anchored handle is skipped (no re-onboard, no churn).
2. **Resolve.** Each base entry is rewritten to `vendor: ‹anchor›` (drives the cert path), `logical_vendor: ‹animal›` (keeps panoscope's vendor grouping human despite the UUID anchor), and `topic: ‹anchor›/‹topic-suffix›`.
3. **Deploy.** The unchanged `zenoh-synth-vendors` role renders + brings up the compose on the resolved fleet.
4. **Schemas.** `ops/zenoh-trial-peers/synth-vendor-schemas/bootstrap-zagros-synth-decode.sh` registers each `‹anchor›/‹animal›/…` topic as `protobuf:‹Msg›` and uploads the matching `.fds` (the wire is already protobuf), flipping the Decoded view from `no_schema` to `decoded`.

Net: each synth peer becomes a first-class onboarded slot-system — decodable in panoscope **and** shown by its alias (`tarsier-alpha`, not `40655337…`). The nova-a flat path is untouched; the deploy asserts `portal_identity_model == per-uuid-slot-pool` and refuses to run otherwise.

## Historical reference (pre-2026-05-19)

The narrative below describes the original 4-vendor / 7-publisher shape (`caracal`, `community`, `homemade-sensors`, `pangolin`) that the trial fabric ran from 2026-05-15 to 2026-05-19. The notional collapse subsumes all four — every animal-named publisher plus the original 7 personalities now live under one `notional` vendor in source, on the wire, and in the portal. The original tables are retained below as a reference for anyone walking back through commits.

## What

A small set of synthetic Zenoh peer publishers + a subscriber-only peer that put real-looking traffic on the nova trial fabric. Each peer is a single Rust binary built from [`ops/zenoh-trial-peers/synth-vendor/`](../../ops/zenoh-trial-peers/synth-vendor/), parameterized by `--personality` to pick one of seven payload shapes (tracks-demo / power-demo / beacon-demo / garage-temp / garage-door / yard-soil / observer). Seven containers run on nova-a, each holding its own Zenoh session (= distinct ZID, = distinct node in panoscope).

All four vendors are registered with the portal's `synthetic: true` flag. The portal UI badges them in the vendors table + topic dropdown so operators can tell at a glance that the traffic is demo data, and panoscope offers a "Show synthetic" checkbox in the legend footer that filters them out of the graph render once real vendors are publishing.

### Multi-key subscribers (2026-06-10)

`SYNTH_VENDOR_SUBSCRIBE_KEY` accepts a **comma-separated list** of key-expressions; the binary declares one subscriber per key and the schema-decl reports each pattern as a separate `subscribes[]` entry (a comma-joined string would never keyexpr-match in panoscope). `parse_subscribe_keys` does the split (trims whitespace, drops empties; a single key is back-compat). This lets one peer cover several patterns — e.g. a **complementary multi-site listener** spanning ~2/3 of the fabric via `**/<site>/**,**/<other-site>/**`. On ZAGROS's per-uuid namespace the topics are `<uuid>/<vendor>/<site>/<suffix>/v1`, so site-scoped subscribes are uuid-agnostic (`**/alpha-fob/**`). Motivation: a couple of overlapping ~2/3 listeners that together cover everything read as a meaningful subscription topology on the COP — distinct from the universal `**` observers (omniwatch/audit/sinks), whose per-topic fan-out panoscope suppresses (see `panoscope.md` §"Universal `**` observers don't fan out edges").

### Per-uuid topic reproducibility (2026-06-10)

On a per-uuid-slot-pool goatnet (ZAGROS, ADR 1025) each synth-vendor publishes under its slot **anchor** `<uuid> = sha256("<salt>:<slot_idx>")[:32]`, not under the literal `notional/` prefix the config carries. Previously the live uuid topics were generated by the portal slot-assignment at onboarding, so the running fabric **diverged from committed config** (`notional/…`) — and re-running the synth deploy from HEAD would push `notional/…` topics that the per-uuid router ACL rejects, breaking every publish. Reconciled 2026-06-10: each vendor entry carries a pinned `slot_idx:`, and the role's compose template rewrites the topic's first segment to the computed anchor using the **same** `((zenoh_slot_pool_salt ~ ':' ~ slot_idx) | hash('sha256'))[:32]` formula the router ACL + storage use (`goat-zenoh/templates/config.json5.j2` parity). Verified: the committed config + formula reproduce all 22 live uuid topics exactly (zero drift), so the synth fabric is now reproducible from committed config and a redeploy is non-breaking. Per-vendor goatnets (no `slot_idx`) publish the topic verbatim — unchanged. The `slot_idx` values are the portal's original allocation (slots 3–23); they live only in deploy config to compute the machine anchor — never a display alias (ADR 1039).

## Why

Three things were true at the same time on 2026-05-15:

1. The trial-fabric substrate landed Healthy on nova-a — Zenoh router, ACL config, REST + audit-logger plugins, vendor portal, panoscope frontend. But there were **no real publishers**, so panoscope showed an empty topology and the portal had no real feeds to reference.
2. The portal's vendor-registration + cert-mint flow needed exercise end-to-end. Until something real used it, the F-125 wire-level claim ("per-vendor leaf cert gates per-vendor write scope at the router ACL") stayed structurally tested only.
3. Operators showing the substrate to new audiences wanted a 30-second walk-through where the visualization animates.

The first synth-vendor wave (caracal + community, three personalities) closed gaps 1-3 narrowly. The expansion to four vendors + seven personalities + a "this is synthetic" flag closes the last UX gap: when the inspector page or topics dropdown showed `homemade-sensors/garage/door-state/v1`, an operator clicking the link saw `(no samples)` because that namespace was inert seed data. Now every catalogued topic is backed by a publisher AND the operator can tell which traffic is demo vs real at every surface.

## Topology

Cross-vendor edges that render in panoscope:

| Source vendor | Sink vendor | Mechanism |
|---|---|---|
| `caracal` → `community` | `community/beacon-demo` subscribes to `caracal/**` | original cross-vendor edge |
| `community` → `caracal` | `caracal/power-demo` subscribes to `community/**` | new cross-vendor edge (this expansion) |
| `homemade-sensors` → `homemade-sensors` | `garage-door` subscribes to `garage/temperature/v1` | intra-vendor (smart-door pattern) |
| `caracal` + `community` + `homemade-sensors` → `pangolin` | `pangolin-observer` subscribes to `**` | observer-flavored read of everything |

## Vendor + topic table

| Vendor (prefix) | Personality | Topic key-expression | Rate | Subscribes to | Format | Schema |
|---|---|---|---|---|---|---|
| `caracal` | tracks-demo | `caracal/tracks-demo/entity/v1` | 1 Hz | — | json | [`tracks-demo-v1.json`](../../ops/zenoh-trial-peers/synth-vendor/schemas/tracks-demo-v1.json) |
| `caracal` | power-demo | `caracal/power-demo/site-nova/v1` | 0.1 Hz | `community/**` | json | [`power-demo-v1.json`](../../ops/zenoh-trial-peers/synth-vendor/schemas/power-demo-v1.json) |
| `community` | beacon-demo | `community/beacon-demo/hello/v1` | 0.5 Hz | `caracal/**` | json | [`beacon-demo-v1.json`](../../ops/zenoh-trial-peers/synth-vendor/schemas/beacon-demo-v1.json) |
| `homemade-sensors` | garage-temp | `homemade-sensors/garage/temperature/v1` | 1/30 Hz | — | json | [`garage-temp-v1.json`](../../ops/zenoh-trial-peers/synth-vendor/schemas/garage-temp-v1.json) |
| `homemade-sensors` | garage-door | `homemade-sensors/garage/door-state/v1` | 0.1 Hz | `homemade-sensors/garage/temperature/v1` | json | [`garage-door-v1.json`](../../ops/zenoh-trial-peers/synth-vendor/schemas/garage-door-v1.json) |
| `homemade-sensors` | yard-soil | `homemade-sensors/yard/soil-moisture/v1` | 1/300 Hz | — | **cbor** | [`yard-soil-v1.json`](../../ops/zenoh-trial-peers/synth-vendor/schemas/yard-soil-v1.json) |
| `pangolin` | observer | `pangolin/observer/heartbeat/v1` | 0.2 Hz | `**` | json | [`observer-v1.json`](../../ops/zenoh-trial-peers/synth-vendor/schemas/observer-v1.json) |

All four vendor namespaces (`caracal/**`, `community/**`, `homemade-sensors/**`, `pangolin/**`) are pre-mapped in panoscope's embedded fallback schema-resolver, so each renders with a distinct edge color out of the box. The portal `/api/schemas` poll (panoscope-followons #4) will eventually override the compiled-in list with portal-served data when that endpoint lands.

The `yard-soil` personality publishes **CBOR-encoded** payloads (ciborium-packed) — the only non-JSON producer in the seven. This exercises the inspector's CBOR decoder path that was otherwise unreached in the trial.

## "Synthetic" flag — portal + panoscope

A new boolean column `vendors.synthetic` lives on the portal's SQLite registry. The bootstrap script ([`ops/zenoh-trial-peers/bootstrap-portal-vendors.sh`](../../ops/zenoh-trial-peers/bootstrap-portal-vendors.sh)) sends `"synthetic": true` when minting each of the four demo vendors. Real partner vendors leave it at the default `false`.

**Portal surface:**
- `GET /api/vendors` returns `synthetic: bool` on each row.
- Vendors table on the home page renders a `synthetic` badge + tinted row background.
- Topics-page vendor-select dropdown suffixes synthetic vendors with `(synthetic)`.
- Operator vendor-registration form has a `Synthetic (demo data)` checkbox.

**Panoscope surface:**
- `SyntheticVendors` set seeded with the embedded fallback `{caracal, community, homemade-sensors, pangolin}` at startup.
- Background poller against the portal's `/api/vendors` updates the set every `PANOSCOPE_PORTAL_REFRESH_SECS` (default 60s). The embedded set is always preserved as a union floor — a vendor row dropped at the portal doesn't un-tag its in-flight data.
- Every `EdgeView` carries `synthetic: bool` derived at view-build time from the topic's vendor prefix.
- Every `NodeView` carries `synthetic: bool` = "all observed publishes AND subscribes are under synthetic-vendor prefixes."
- Legend footer has a `Show synthetic` checkbox (default ON). Choice persists per-browser via localStorage. When OFF, synthetic edges + synthetic-only peers are hidden from the D3 simulation entirely — the force layout reflows.

The first time real vendor traffic lands, the operator can flip the toggle off and see ONLY real traffic. Until then, the demo set is fully visible.

## Identity + ACL

Four registered vendors → four leaf certs minted via the portal's `POST /api/vendors` flow → four ACL allowlist entries on the router (keyed off the leaf `cert_common_name`, per F-124 item 7). The two `caracal`-personality processes share a single `caracal` cert; each of the other three (`community`, `homemade-sensors`, `pangolin`) holds its own. The `pangolin` cert grants write scope on `pangolin/**` for the heartbeat topic; reads are wide-open per the trial fabric's relaxed posture, so its cross-vendor subscribe to `**` works without any per-vendor allowlist entry.

The bootstrap script walks the vendor registration once, downloads the cert tarballs, declares all seven topics via `POST /api/vendors/{v}/topics`, and SCPs the cert tree to nova-a.

## What this validates

- **Vendor registration + cert mint flow end-to-end.** Until now, the portal's vendor-mint path was structurally tested. Synth-vendors are the first non-operator consumers of a minted bundle, so they wire-validate that the issued cert chain-verifies against the nova root and that the leaf CN matches the ACL rule. The expansion adds two more vendors through the same flow.
- **Per-vendor write-scope ACL at the wire.** When run with the `tls/` endpoint, an attempted write to a foreign vendor's prefix gets rejected by the router's `access_control` block. This was the F-125 wire-level claim that the missing-CLI gap blocked from validation; synth-vendors close it as a free side-effect.
- **Cross-vendor read posture.** beacon-demo + power-demo + pangolin-observer subscribe across vendor boundaries — proves the community-of-trust read model end-to-end. `pangolin`'s `**` subscribe is the maximal case.
- **panoscope visualization end-to-end.** Seven distinct ZIDs → seven nodes; seven publish flows → seven animated edges; multiple cross-vendor + intra-vendor subscriber edges. First time the visualization renders a real (non-empty) topology with both same-vendor AND cross-vendor relationships.
- **Inspector decoder coverage.** The `yard-soil` CBOR personality is the only non-JSON producer, so its registered topic exercises the inspector's CBOR decode path.
- **Synthetic-vs-real visibility.** The `synthetic` flag end-to-end means a partner walking up to the portal sees an honest signal: "the rows badged `synthetic` are demo data; real partner traffic shows up without the badge." Panoscope's toggle lets that same partner flip to a real-only view the moment a real vendor publishes.

## Lifecycle

| Phase | Trigger | Action |
|---|---|---|
| **Bring up** | Operator runs `bootstrap-portal-vendors.sh` then `zenoh-synth-vendors-deploy.yml` | Four vendors registered (synthetic=true), certs delivered, seven systemd-managed docker-compose services running on nova-a. |
| **Tend** | Image rebuild via `.github/workflows/synth-vendor-image.yml` | Operator re-runs the playbook; `docker compose pull` + `up -d` refresh the running containers in place. |
| **Tear down at trial sunset** | Trial sunset date | `docker compose down` on nova-a; portal vendor registrations dropped via operator UI or `DELETE` API; cert tree removed from `/opt/synth-vendors/certs/`. Detailed at [`zenoh-synth-vendors-runbook.md`](../operations/zenoh-synth-vendors-runbook.md). |

## Fleet mode — one container, many peers (local panoscope demo)

The nova-a deployment runs **one container per peer** (22 containers), each a single-personality `synth-vendor` process with its own ZID. That's the right shape on a dedicated host, but wasteful on a developer laptop already running other workloads — the motivation behind the local containerized panoscope demo (`panoscope.md` §"Local containerized demo").

Fleet mode collapses the ecosystem into a **single container**. The binary takes `--roster <file>` (env `SYNTH_VENDOR_ROSTER`); when set, one process opens **one Zenoh session per roster row**, each session getting a distinct ZID from `zenoh::open`. Because panoscope keys peers by ZID, N internal sessions render as N distinct nodes — the whole topology (animated edges, role-class shapes, cross-vendor subscriber edges) from one process, one tokio runtime, one allocator. Session opens are staggered ~40 ms so 22 handshakes don't hit the router simultaneously.

- **Roster format.** JSON: a top-level `endpoint` (+ optional `cert`/`key`/`root_ca` for `tls/`) and a `peers[]` array whose row fields mirror the single-peer CLI flags and the `synth_vendors:` entries in `ops/ansible/group_vars/nova_hosts/synth-vendors.yml`. Explicit `--endpoint`/env overrides the baked roster value (so compose can repoint the router without an image rebuild); the roster value is the zero-config fallback.
- **Demo roster.** [`ops/zenoh-trial-peers/synth-vendor/demo-roster.json`](../../ops/zenoh-trial-peers/synth-vendor/demo-roster.json) is generated from the Ansible source of truth (all 22 peers, plaintext endpoint) and baked into the image at `/etc/synth-vendor/demo-roster.json`. Single source of truth: the Ansible vars file remains canonical; the roster is a projection.
- **Single-peer mode is unchanged.** Without `--roster`, the binary requires `--personality`/`--topic`/`--peer-id`/`--endpoint` exactly as before, so the nova-a Ansible role (`ops/ansible/roles/zenoh-synth-vendors/`) is unaffected — one container per peer, distinct ZIDs, per-vendor cert mounts.
- **Pure sinks.** Fleet mode honors `no_publish` (env `SYNTH_VENDOR_NO_PUBLISH`, `--no-publish`): the peer subscribes and emits schema-decls but never declares a data publisher, and its schema-decl omits `publishes` so panoscope renders it as a subscriber-only sink. This flag was referenced by the Ansible role template but had no binary implementation until fleet mode landed — the three sinks (glass-display / archive-vault / audit-log) previously published to their `…/unused/v1` topic. Now they are genuine sinks in both modes.

Fleet mode carries no cert material of its own; for the demo it speaks plaintext `tcp/` to an open-ACL dev router. Pointing fleet mode at a real mTLS router would need a single shared cert whose write-scope covers every roster topic — out of scope for the demo and not how nova-a runs (per-vendor certs, one container each).

## Out of scope for this design

- **Production-class vendors.** Real vendors register themselves via the portal's self-service flow with `synthetic: false`; synth-vendors are operator-managed demo data.
- **Schema family coloring.** Panoscope's `FamilyMapResolver` colors edges using the compiled-in `caracal/**` + `community/**` mappings (the family-map floor); per-peer coloring otherwise rides schema-decl self-registration. `homemade-sensors/**` + `pangolin/**` render as `unknown`-family color unless they self-attest. (The original "schema serving via portal `/api/schemas`" plan was a bulk endpoint the portal never built — the dead fetch path was removed in F-230; see `panoscope.md` §"Schema-color data source".)
- **Audit-logger acceptance evidence.** Audit-logger plugin loading on the new image is panoscope-followons #2. When that resolves, audit-logger JSONL becomes the second observation surface for synth-vendor traffic.
- **`synthetic` on TOPICS, not just vendors.** Today the flag lives on the vendor row only; a partner who legitimately wants to publish a demo topic under their real namespace would have to register a separate synthetic vendor. Acceptable trial-scope simplification.
