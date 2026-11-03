# Secure messaging substrate — design

> Canonical design doc for the per-site Matrix-protocol secure
> messaging service for goat-net peers. Anchors:
> [ADR 0835](https://github.com/usareur-af/goatcore/blob/main/docs/adr/0835-secure-messaging-substrate-tuwunel.md)
> (server selection — tuwunel) and
> [ADR 0836](https://github.com/usareur-af/goatcore/blob/main/docs/adr/0836-matrix-push-gateway-hedwig.md) (push
> gateway — hedwig + named iOS waiver). Implementation
> tracking: `../project/implementation-plan.md`
> Block 70.

## 1. Purpose

A sovereign per-site secure messaging service that scales to
**1000 users per site**, **~6000 fleet-wide**, with the
following properties:

- **End-to-end encrypted by default** (Matrix Megolm)
- **Sovereign per-site authority** — each site runs its own
  homeserver; users belong to a site
- **Federated across sites** over the wg-cp0 silent control
  plane — no public-internet listeners
- **OIDC SSO via Ory Kratos** — no manual user provisioning
- **Mobile push that respects the no-public-internet-on-secure-
  plane invariant where structurally possible** — UnifiedPush +
  ntfy on Android default; FCM-via-hedwig for stock-Element +
  iOS with a named waiver
- **Audit-by-construction** via Block 27 OTLP wide-event surface
  + Block 17D audit-integrity tier
- **Phoenix-native peer** per [`phoenix-architecture.md`](https://github.com/usareur-af/goatcore/blob/main/docs/design/phoenix-architecture.md) —
  rebuildable from supply-chain artifacts; identity bound;
  configuration in source-controlled repos

Out of scope for v1: external federation to partner Matrix
homeservers; voice/video calling; bridging to non-Matrix chat
networks (Slack/Discord/IRC bridges).

## 2. Position relative to other goat-net layers

```
┌────────────────────────────────────────────────────────────┐
│ Operators on issued devices (Android + iOS + laptops)      │
│   ↓ TLS 1.3 from Element / FluffyChat / Element X          │
├────────────────────────────────────────────────────────────┤
│ Per-site Caddy at chat.<site>.netbird.legacy.example:443  │
│   ↓ HTTP                                                   │
├────────────────────────────────────────────────────────────┤
│ tuwunel homeserver                                         │
│   - RocksDB at /opt/docker-data/tuwunel/db/                │
│   - Media → MinIO chat-media bucket                        │
│   - OIDC client → Ory Hydra                                │
│   - Push → hedwig (Tier 2 + 3) or per-user UnifiedPush URL │
│   - Federation → wg-cp0 to peer-site tuwunel:8448          │
└────────────────────────────────────────────────────────────┘
```

Composes with — does not replace:

- **Block 48J SRE escalation pager** (per-cluster Matrix rooms
  for agent-escalation severity-tier push profiles per ADR 0010;
  becomes one-of-many room types alongside operator chat)
- **Block 17A.5 policy-engine notification adapter** (Matrix is
  a notification adapter, not a load-bearing dependency; sites
  without a homeserver gracefully degrade per ADR 0805)
- **Block 38B chaos probe failures** that page via Matrix
- **Block 56 site-loss-under-duress operator coordination**

The chat substrate is **not** the policy engine; the chat
substrate is **not** the SRE pager. Both ride on top of it.

## 3. Capacity envelope

Per ADR 0215:

| Tier | Per-site users | Fleet-wide | Posture |
|---|---|---|---|
| Floor | 100 | 600 | Always supported |
| Target | 500 | 3000 | Soak-validated v2 |
| Stretch | 1000 | 6000 | Soak-validated v3 with growth headroom |

Active concurrency and daily-active fractions:

| Metric | Floor | Target | Stretch |
|---|---|---|---|
| Registered users / site | 100 | 500 | 1000 |
| Daily-active / site (assume 30%) | 30 | 150 | 300 |
| Concurrent peak / site (assume 10%) | 10 | 50 | 100 |
| Federation events / site / day (rough) | 10K | 100K | 250K |

These shape the soak harness in §11.

## 4. Per-site topology

### 4.1 Co-location

The tuwunel homeserver lands on **Orin-A** (active mgmt-host)
per ADR 0826 —
same host as netbird-mgmt + signal + Kratos + Hydra. Resource
sizing on a 64 GB Orin:

| Component | RAM | Disk | Notes |
|---|---|---|---|
| netbird-mgmt | ~500 MB | ~5 GB | Existing |
| signal-server | ~300 MB | ~1 GB | Existing |
| Kratos + Hydra | ~1 GB | ~2 GB | Existing per ADR 0411 |
| postgres-spock | ~4 GB | ~100 GB | Existing |
| **tuwunel** | **~2-4 GB** | **~50-200 GB** | New (RocksDB sized for ~1 GB/active-user) |
| **hedwig** | **~100 MB** | **<100 MB** | New (stateless) |
| **Element web (Caddy file_server)** | static | <500 MB | New |
| Caddy / Traefik | ~200 MB | <100 MB | Existing |

Headroom comfortable on a 64 GB Orin even at stretch capacity.
Disk pressure becomes the binding constraint — see §10.

### 4.2 Failover

Active-passive matches the rest of the mgmt stack: Orin-A
runs tuwunel; Orin-B is a hot standby. **The chat substrate
does not ride postgres-spock** (RocksDB backend); failover is
handled at the federation-protocol layer:

- Steady state: clients connect to `chat.<site>...` resolving
  to Orin-A's mesh IP.
- A-fails event: keepalived moves the chat VIP to Orin-B (same
  pattern as netbird-mgmt). Orin-B brings up tuwunel against
  a **periodically rsynced RocksDB snapshot** from Orin-A
  (cadence: hourly). Acceptable RPO: ~1 hour of chat history.
- After A-recovery: operator-fired drain + cutback procedure
  (mirrors the netbird-mgmt one; documented in operations
  runbook).

**Chat history loss of up to ~1 hour on A-fail is the
deliberate trade-off** — the alternative (Spock-replicated
chat events at high write volume) would be the dominant load
on the cluster and is engineered against by the RocksDB
choice. E2EE clients tolerate transient gaps; users see brief
"reconnecting" then re-sync.

A v3 enhancement could add streaming RocksDB replication via
WAL shipping; not in v1 or v2 scope.

## 5. Identity

### 5.1 OIDC SSO via Ory

User flow:

1. User visits `chat.<site>.netbird.legacy.example` in browser
   or opens Element X mobile.
2. Client redirects to Ory Hydra OAuth authorize endpoint.
3. Hydra delegates to Kratos for the actual authentication
   (password / TOTP / FIDO2).
4. On success, Hydra issues an access token + ID token; client
   redirects back to tuwunel with the code.
5. Tuwunel exchanges the code, validates the ID token signature
   against Hydra's JWKS, extracts the `sub` claim.
6. Tuwunel auto-provisions the Matrix user
   `@<sub>:<site>.netbird.legacy.example` on first login;
   subsequent logins reuse the row.

Tuwunel's OIDC config:

```toml
[global.well_known.openid_configuration]
issuer = "https://auth.<site>.netbird.legacy.example"
# ... Hydra discovery + JWKS endpoints
client_id = "tuwunel-<site>"
client_secret = "{{ ory_oidc_client_secret }}"  # from group_vars + env
```

### 5.2 First-admin import

The first administrator user is imported via tuwunel admin CLI
during bootstrap, not via OIDC (chicken-and-egg). After import,
the admin authenticates the same way as any user. Pattern
mirrors the Block 21C-production first-admin Kratos import.

### 5.3 IdP-down recovery

When Ory is unavailable (per Block 21F runbook), no new
authentications can occur but **existing tuwunel sessions
continue working** — Matrix access tokens have configurable
TTL (default 7 days; we set to 24 h to balance security vs
IdP-outage tolerance). Browser / app refresh during outage
fails; existing connections persist.

### 5.4 Cross-site users

A user belongs to one home site (their `:domain` part of the
MXID). To talk to a user at another site, the homeservers
federate over wg-cp0. **No user-account sync between sites** —
each site is the authority for its own users.

Cross-site Ory Kratos identity assertion is out of scope here;
each site runs its own Ory instance per ADR 0411 / Block 21C.
Future federation to a single-IdP-fronts-multiple-sites model
is a Phase 3+ design question and would simplify cross-site
addressing if needed; not in v1.

## 6. Federation

### 6.1 Topology

Six sites per ADR 0827.
**Full-mesh** federation between all pairs over wg-cp0.

Each site's tuwunel federates with peer sites' tuwunel on the
peer's wg-cp0 mesh IP, port 8448:

```
tuwunel @ vega-A (198.18.0.1)
   ├─ federates with tuwunel @ prov-site3 (198.18.0.6:8448)
   └─ federates with tuwunel @ <peer site> (198.18.0.X:8448)
   ...
```

### 6.2 Discovery

Matrix federation uses `.well-known/matrix/server` for server
delegation. Per-site DNS (`mesh-hosts` zone, see
`../design/dns-service-discovery.md`) returns the wg-cp0 IP for
`chat.<site>.netbird.legacy.example`. Federation between sites
goes wg-cp0-only — chat traffic between sites rides the
silent control plane.

### 6.3 Allow-list at the network layer

Caddy / nftables allow port 8448 only from the wg-cp0 subnet
(`198.18.0.0/24`). Federation cannot be initiated from outside
wg-cp0. Composes with [ADR 0313](https://github.com/usareur-af/goatcore/blob/main/docs/adr/0313-silent-control-plane-outer-wg-wrap.md)
(silent control plane) — the chat substrate inherits the same
no-public-internet-on-secure-plane invariant.

### 6.4 External / partner federation

**Out of scope for v1.** Queued for v3 under the Block 60
moon-pod federation pattern, and gated on:

- Block 28 WASM conditioning sandbox availability for inbound
  message classification (we'd treat partner-Matrix events
  the same way we treat partner-Zenoh events per ADR 0821 —
  release/<partner>/... namespace, default-deny, WASM-classified)
- Per-partner charter signed
- Partner Matrix homeserver demonstrating IAL2/AAL2 identity
  assurance per `partner-templates/data-sharing-charter.md`

### 6.5 Federation soak gate (the v2 cutover gate)

See §11. The v2 cutover (single-site → multi-site federation)
does not happen until the soak passes.

## 7. TLS + the offline CA

Caddy terminates TLS 1.3 in front of tuwunel for both client-
server and server-server traffic. Leaf cert from the Block 22J /
[ADR 0407](https://github.com/usareur-af/goatcore/blob/main/docs/adr/0407-internal-x509-ca-for-service-tls.md)
internal X.509 CA via `cert-sign --eku serverAuth`:

```
cert-sign \
  --site-id vega \
  --cn chat.vega.netbird.legacy.example \
  --san chat.vega.netbird.legacy.example \
  --san 198.18.0.1 \
  --san 192.168.1.X \
  --eku serverAuth \
  --validity 8760h
```

Tuwunel federation client trusts the offline-CA root delivered
via the Block 22K bundle's `tls_trust_anchor` field. Same
posture as netbird-mgmt, signal-server, Ory.

**No Let's Encrypt** for the chat substrate — the homeserver
hostname is mesh-only, no public DNS, no HTTP-01 / DNS-01 path
applicable. This is the standard pattern for every mesh-internal
service in our stack.

## 8. Push gateway

Per [ADR 0836](https://github.com/usareur-af/goatcore/blob/main/docs/adr/0836-matrix-push-gateway-hedwig.md). Three
tiers, decided by client population per device:

### Tier 1 — UnifiedPush + self-hosted ntfy (Android default)

```
tuwunel → user's UnifiedPush URL → ntfy.<site>.netbird.legacy.example
       → distributor app on phone (ntfy / FluffyChat distributor)
       → app
```

ntfy is per-site and already named in Block 48J
for SRE escalation. Same server, additional topic per
operator. No new substrate.

### Tier 2 — hedwig + FCM (Android stock-Element fallback)

```
tuwunel → hedwig (per-site, on Orin-A) → FCM → phone
```

Opaque body. Google in path. Acceptable tradeoff named in
ADR 0836 — operator who uses stock Play Store is voluntarily
already in the Google ecosystem on that device.

### Tier 3 — hedwig + direct APNs (iOS)

> **Corrected 2026-05-07 post-Block-70.A spike** (`block-70-verification-spike.md` §3.1): hedwig is **not** FCM-only. The original "FCM-proxied APNs (Google + Apple in path)" framing was based on a misreading of the hedwig README. Source review confirms direct APNs via the [`a2`](https://crates.io/crates/a2) crate ([`src/apns.rs`](https://github.com/famedly/hedwig/blob/main/src/apns.rs)).

```
tuwunel → hedwig → APNs → phone
```

**Apple is in the path. Google is NOT in the path** — assuming the
operator configures hedwig with an Apple `.p8` private key +
`team_id` + `key_id` to enable the direct-APNs branch. Opaque body.
**Named waiver** at `docs/compliance/waivers/ios-push-apns.md`
covers the structural reality that Apple's APNs is unavoidable for
any iOS push (iOS platform constraint, not a hedwig decision); same
baseline a Sygnal deployment would carry.

**Operator config requirement.** `hedwig` config must populate the
`[apns]` section with the .p8 key path, team_id, key_id, and
sandbox/production flag. Absent that config, hedwig's `apns_sender`
is `None` and iOS push attempts return an `APNSNotConfigured` error
— do NOT fall back silently to FCM-proxied-APNs (which would put
Google in the path).

**Routing decision is per-device.** Hedwig routes by the
`NotificationMethod` field on each registered device pusher
(`Apns` or `Fcm`). This is set when the Matrix client registers
its push pusher with the homeserver. Element X / FluffyChat
clients on iOS register `Apns`; clients on Android typically
register `Fcm` (or use UnifiedPush, bypassing hedwig per Tier 1).
The operator does not "choose" the iOS path — the client app's
pusher registration sets it. Our deployment configures
`apns_sender` so the `Apns` route works; UnifiedPush + ntfy is
the preferred Android path (Tier 1) regardless.

### v3+ APNs-direct upstream contribution — RETIRED 2026-05-07

The Block 70.D.5 entry that queued an upstream PR to add direct-APNs
to hedwig is **redundant** — the feature already exists in upstream
(`src/apns.rs` + `pusher::push_notification_apns` per spike report
§3). Drop the queued item.

## 9. Mobile client distribution

| Platform | Recommended | Source | Push tier |
|---|---|---|---|
| Web | Element web | Caddy file_server at `chat.<site>...` | Browser-tab notifications |
| Android (preferred) | FluffyChat or Element X | F-Droid or MDM | Tier 1 (UnifiedPush) |
| Android (fallback) | Stock Element | Play Store | Tier 2 (FCM) |
| iOS | Element X | App Store | Tier 3 (direct APNs via hedwig — see correction above) |
| Linux desktop | Element Desktop | Apt or Flatpak | Same as web |
| macOS desktop | Element Desktop | DMG | Same as web |
| Windows desktop | Element Desktop | MSIX | Same as web |

Operator onboarding runbook (Block 70.5) covers per-platform
install + first-login + UnifiedPush distributor configuration.

## 10. Storage

### 10.1 Per-site disk allocation

| Bucket | Where | Size estimate (1000 users) | Retention |
|---|---|---|---|
| RocksDB events | Orin-A NVMe `/opt/docker-data/tuwunel/db/` | ~500 GB | Indefinite (compaction handles tombstones) |
| Media (images, files) | MinIO `chat-media` bucket on Orin-C | ~200 GB | 90 days hot, then archive |
| Backup snapshots | MinIO `chat-archive` bucket on Orin-C | ~50 GB rolling | 30 days |
| Federation cache | Orin-A `/opt/docker-data/tuwunel/cache/` | ~10 GB | tuwunel-managed |

Disk pressure is the binding capacity constraint at stretch
(~750 GB on a 1 TB Orin-A NVMe leaves ~25% headroom — tight).
Block 27 OTLP alert thresholds:

- Warn at 70% disk usage
- Critical at 85% disk usage (page via the chat substrate
  itself plus ntfy)

If stretch is realized at one site, disk capacity is the first
thing that needs upsize — 2 TB NVMe procurement.

### 10.2 MinIO buckets

Two new MinIO buckets on the Orin-C MinIO substrate (per
[Block 27 / ADR 0822](https://github.com/usareur-af/goatcore/blob/main/docs/adr/0822-wide-event-observability-otlp-openobserve.md)):

- **`chat-media`** — TTL 90 days. Object Lock not required
  (E2EE content is opaque to the server; no compliance
  retention mandate).
- **`chat-archive`** — backup snapshots; TTL 30 days.

Each bucket gets its own IAM credential pair; tuwunel only
has access to those two buckets.

### 10.3 Backup discipline

Nightly rsync of RocksDB snapshot + recent media to the
`chat-archive` bucket. Restore is tested annually as part of
the Block 56 site-loss-under-duress drill. Backups are encrypted-
at-rest (MinIO server-side encryption with offline-CA-rooted KEK).

## 11. Verification + soak

### 11.1 Verification spike (Block 70.1, before any code lands)

Half-day research-only spike to convert the tuwunel selection
from "README claim" to "verified evidence":

1. WebFetch the tuwunel.chat homepage + docs site — confirm
   architecture details + production references.
2. Lurk in `#tuwunel:matrix.org` for ~24 h to read recent
   ops chatter — known issues, recent incidents, federation
   bug class.
3. Scan tuwunel's open issues for federation-bug-class
   items + recent CHANGELOG for federation-related fixes.
4. Confirm hedwig FCM-only + APNs-direct-PR feasibility by
   reading hedwig source at `famedly/hedwig`.

Output: signed-off `docs/project/block-70-verification-spike.md`
with PASS or pivot decision before any v1 code commits.

### 11.2 v1 single-site soak (Block 70.4 — vega only)

5-day soak on vega only, no inter-site federation:

- 100 synthetic users register via Ory Kratos test endpoints
- Each posts ~50 messages/day in a small set of rooms
- 5 with E2EE keys (cross-signing exercised)
- 5 with media uploads (~10 MB each, exercise MinIO path)
- Restart tuwunel mid-soak to validate session persistence

PASS criteria: zero data loss, zero unplanned restarts,
p99 message-send latency < 1 s, RocksDB grows linearly (no
runaway).

### 11.3 v2 multi-site federation soak (Block 70.7 — the load-bearing gate)

30 days. Three sites. Synthetic 1000-user
load split across the three sites. Pass criteria from
[ADR 0835](https://github.com/usareur-af/goatcore/blob/main/docs/adr/0835-secure-messaging-substrate-tuwunel.md):

- Zero room-state-divergence events across the three
  homeservers as observed by a synthetic auditor peer
- Federation lag p99 < 5 s for room events
- Zero unplanned tuwunel restarts attributable to federation
  bugs
- Zero data-corruption RocksDB incidents
- Memory + CPU within 70% of provisioned headroom on Orin-A
  under sustained synthetic load

**FAIL paths named in advance:**

- Mode A — federation bugs at scale: pivot to Dendrite
- Mode B — both Matrix implementations fail: pivot to ejabberd
  (XMPP, protocol pivot, loses Element ecosystem)
- Mode C — fixable upstream bug: file issue + carry small fork

Evidence path: `docs/reports/2026-XX-XX-secure-messaging-federation-soak.jsonl`
+ `docs/project/block-70-decision-review.md` (Ory-pattern
decision review).

## 12. Phoenix-native posture

Per [`phoenix-architecture.md`](https://github.com/usareur-af/goatcore/blob/main/docs/design/phoenix-architecture.md). The
chat substrate is engineered to satisfy:

- **Identity-bound** — every user authenticates via Ory Kratos
  OIDC; tuwunel doesn't issue independent credentials.
- **Capability-discovered** — per-site DNS at
  `chat.<site>.netbird.legacy.example`; Matrix `.well-known`
  for federation.
- **Supply-chain-delivered** — tuwunel container image from
  ghcr.io/matrix-construct/tuwunel:latest verified against
  pinned tag; hedwig from famedly/hedwig pinned tag; both
  targeted for Trivy + grype scanning under Block 18H. The current
  advisory workflow scans only images named in its curated list and does
  not gate individual image builds; grype is not yet wired into CI.
- **Self-attesting** — at boot, tuwunel emits a startup event
  to the Block 27 wide-event surface with version + config
  hash + offline-CA cert fingerprint.
- **Rebuildable from source-controlled config** — entire
  per-site config under `ops/ansible/roles/tuwunel/` +
  `group_vars/<site>_hosts/vars.yml`. RocksDB data is
  recoverable from `chat-archive` MinIO bucket. **Full chat-
  substrate rebuild from a wiped Orin: ≤ 30 minutes**
  (target; soaked in Block 70.10 drill).
- **Block 68 rapid-bringup integration** — first-boot for a
  new site provisions Caddy + tuwunel + hedwig as part of the
  standard mgmt-stack profile.

## 13. Audit + observability integration

### 13.1 Block 27 wide events

tuwunel emits OTLP traces + structured events for:

- User register / login / logout
- Room create / invite / kick / ban
- Federation event send / receive (with peer-site attribution)
- Message send / redact (event metadata only — bodies are
  E2EE and opaque)
- Admin actions (server-admin commands)
- Errors at the federation, OIDC, push paths

All events carry `correlation_id` for cross-plane joining.
Configuration: tuwunel's OTLP exporter points at the per-site
OTel Collector on Orin-C.

### 13.2 Block 17D audit-integrity

Admin actions, account creation/deletion, and federation
peer-add events route through the Block 17D audit-integrity
tier with Object-Lock retention per [ADR 0811](https://github.com/usareur-af/goatcore/blob/main/docs/adr/0811-audit-log-integrity.md).
This is a subset of Block 27 events specifically retained for
compliance and incident investigation.

### 13.3 Prometheus metrics

tuwunel exposes Prometheus metrics at `/metrics` on a
loopback admin port; scraped by the per-site Prometheus on
Orin-C. Standard Matrix homeserver dashboards apply (Element
publishes Grafana JSON for Synapse; tuwunel emits a similar
metric set).

Per-substrate alert rules:

- `tuwunel_federation_lag_p99 > 5s` for 5m → page
- `tuwunel_db_size_bytes > 850e9` (85% of 1 TB NVMe) → page
- `tuwunel_unplanned_restarts_total > 0` in 1h → page
- `hedwig_push_failure_rate > 5%` for 10m → warn (graceful
  degradation; iOS push delivery)

## 14. Failure modes + degradation

| Failure | User-visible behavior | Recovery |
|---|---|---|
| Tuwunel process crash | Brief "reconnecting" in client; session resumes | systemd restart; ≤30s downtime |
| Orin-A power loss | Failover to Orin-B with up-to-1h chat history loss | keepalived VIP move + RocksDB snapshot replay |
| Federation peer-site unreachable | Cross-site rooms go quiet; same-site rooms unaffected | Auto-recover when wg-cp0 reachable |
| Ory IdP down | New logins fail; existing sessions persist for token TTL | Block 21F runbook; sessions extend up to 24h |
| MinIO (chat-media) down | Media uploads/downloads fail; text messages unaffected | MinIO restart; queued uploads drain |
| ntfy down (Tier 1) | Android UnifiedPush users stop getting wake-ups; messages still queue | ntfy restart; clients pull-on-foreground |
| FCM/APNs unreachable | Tier 2/3 mobile users no push; Tier 1 + web unaffected | External; users must check actively |

Per the Block 63B failure-mode matrix in `threat-model.md` §3a,
the secure messaging substrate's declared dependency modes are:
**fail-stale on Ory** (existing tokens valid until expiry),
**fail-stale on MinIO** (text works, media slow), **fail-open
on push gateways** (no security implication, just convenience),
**fail-rebuild on RocksDB corruption** (Phoenix path; restore
from `chat-archive`).

## 15. Cost + sizing summary

| Resource | Per-site | Fleet (6 sites) |
|---|---|---|
| Orin-A RAM impact | +2-4 GB | +12-24 GB (no procurement — fits in 64 GB Orin) |
| Orin-A NVMe at stretch | +500-750 GB | +3-4.5 TB across sites; per-site upsize on demand |
| MinIO buckets on Orin-C | +250 GB | +1.5 TB — fits in 1 TB Orin-C NVMe at floor/target; stretch needs upsize |
| Network (federation) | ~5 GB/day at stretch | wg-cp0 internal; no public-internet egress cost |
| Operator time (steady state) | ~1 h/week | ~6 h/week |

No new procurement at floor + target. Stretch capacity at one
site triggers Orin-A 2 TB NVMe upgrade + Orin-C 2 TB NVMe
upgrade.

## 16. Out of scope

Named explicitly so a future reviewer doesn't mistake gap for
oversight:

- **External federation to non-goat-net Matrix homeservers**
  (matrix.org, partner orgs). v3+ under Block 60 moon-pod.
- **Voice / video calling** (Matrix supports it via Element
  Call / WebRTC SFU). Future add-on; would compose with the
  Block 35 video fabric MediaMTX/LiveKit substrate.
- **Bridging to non-Matrix networks** (Slack / Discord / IRC).
  No use case identified; queued.
- **Cross-site user account merge / single-sign-everywhere**.
  Each site is the authority for its own users; cross-site
  identity is a Phase 3+ design question.
- **Fully offline / island-mode chat** beyond same-site rooms.
  Same-site rooms work without internet; cross-site rooms
  require wg-cp0 reachability between sites; no DTN-style
  store-and-forward across partition.
- **Mandatory mobile client (FluffyChat, custom Element fork).**
  We support stock Element with the Tier 2/3 + waiver; we
  *recommend* FluffyChat for sovereignty.
- **APNs-direct push for iOS at v1/v2.** Hedwig FCM-only path
  ships first. **APNs-direct upstream contribution is RETIRED
  2026-05-07** post-Block-70.A spike — hedwig already implements
  direct APNs in upstream; no PR needed.

## 17. Open questions

- **Element web vs Element X mobile UX consistency** — Element X
  uses the new sliding-sync API; tuwunel claims sliding-sync
  support but this is exactly the kind of feature that shows
  rough edges in non-Synapse implementations. Soak it.
- **Push key rotation + device verification UX** — Matrix
  cross-signing is a mature feature but the user-facing
  ceremony (verify another device) is sometimes confusing for
  non-technical operators. Operator onboarding runbook needs
  a clear walkthrough.
- **Does federation between different tuwunel versions
  cause issues?** Same-version federation is the well-tested
  case; rolling upgrades across 6 sites will exercise mixed-
  version federation. Soak harness exercises one peer-site at
  N-1 vs the rest at N.
- **Audit-log volume at stretch** — 1000 users × 6 sites
  generating Block 27 OTLP wide events at message-send rate
  may push the OTel Collector's ingest envelope. Sizing TBD
  in Block 70.6 instrumentation work.

## 18. Document drift discipline

Per CLAUDE.md documentation discipline: changes to the chat
substrate (new push tier, federation topology change, identity
provider change) update **this design doc + the implementation
block + CLAUDE.md pointer** in the same change. Stale references
to "Dendrite" anywhere in the tree are a finding (F-NNN) per
[ADR 1011](https://github.com/usareur-af/goatcore/blob/main/docs/adr/1011-findings-register.md).

---

*Anchor doc for [ADR 0835](https://github.com/usareur-af/goatcore/blob/main/docs/adr/0835-secure-messaging-substrate-tuwunel.md)
+ [ADR 0836](https://github.com/usareur-af/goatcore/blob/main/docs/adr/0836-matrix-push-gateway-hedwig.md). Supersedes
the chat-related portions of [ADR 0812](https://github.com/usareur-af/goatcore/blob/main/docs/adr/0812-per-site-matrix.md)
(Dendrite-pager) which is now Superseded.*
