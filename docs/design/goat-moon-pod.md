# goat-moon-pod — abstract spec + reference-implementation pointer

| Field | Value |
|---|---|
| Status | Proposed (Phase 2D-late / Phase 3 commitment) |
| ADR anchor | [`../adr/0223-goat-moon-pod-repo-containment.md`](https://github.com/usareur-af/goatcore/blob/main/docs/adr/0223-goat-moon-pod-repo-containment.md) |
| Companions | [`partner-receiver-requirements.md`](https://github.com/usareur-af/goatcore/blob/main/docs/design/partner-receiver-requirements.md), [`goat-moon-pod-builder-guide.md`](goat-moon-pod-builder-guide.md), [`partner-reference-architecture.md`](https://github.com/usareur-af/goatdocs/blob/main/docs/design/partner-reference-architecture.md), [`mission-partner-sharing.md`](https://github.com/usareur-af/goatcore/blob/main/docs/design/mission-partner-sharing.md) |
| Reference implementation | [`USAREUR-AF/zagros-moon-pod`](https://github.com/USAREUR-AF/zagros-moon-pod) (separate repo; Apache 2.0; private during MVP, public at GA) |
| Implementation tracker | `../project/implementation-plan.md` Block 60 |

## What this is

The **abstract spec** for a goat-moon-pod — a deployable instance of the sovereign-orbital partner-relationship pattern. This document defines the architectural shape; the requirements contract lives at [`partner-receiver-requirements.md`](https://github.com/usareur-af/goatcore/blob/main/docs/design/partner-receiver-requirements.md); the reference implementation lives at [`USAREUR-AF/zagros-moon-pod`](https://github.com/USAREUR-AF/zagros-moon-pod); a guide for vendors who want to build their own lives at [`goat-moon-pod-builder-guide.md`](goat-moon-pod-builder-guide.md).

## The shape in one paragraph

A goat-moon-pod is a single-host containerized appliance a mission partner runs on their own hardware to participate in the goat fabric without engineering staff. It bundles a network client (joining a goat-hosted partner-net coordination plane), a Zenoh router (joining the data fabric over WireGuard), a translation engine (`polygloat`) that fans data out into whatever format the partner's downstream systems consume, an identity database (one of Kanidm / Dex / Authelia / Zitadel — partner picks), an audit sink with 90-day local retention, and a small operator-facing web UI. The partner runs it on their hardware, in their facility, on their power, with their identity database holding their operators' credentials. They could fork the bundle and run the coordination plane themselves any day; they just don't have to.

## Architecture sketch

```
┌──────────────────────────────────────────────────────────────────────┐
│  Partner site — goat-moon-pod (single host)                          │
│                                                                      │
│  Operator ─▶ Web UI ─▶ Local IdP                                     │
│              (auth)    (Kanidm / Dex / Authelia / Zitadel)           │
│                 │                                                    │
│                 ▼                                                    │
│         polygloat ─▶ partner downstream systems                     │
│              ▲              (REST / MQTT / Kafka / file-drop /       │
│              │              syslog / TAK CoT / MISP / S3 / webhook)  │
│              │                                                       │
│         Zenoh router ─── WireGuard ───▶ goat partner-net             │
│              │           (commercial      (separate netbird-mgmt     │
│              ▼           internet)        instance, goat-hosted)     │
│         Audit sink (Loki / journald, 90d local retention)            │
│                                                                      │
└──────────────────────────────────────────────────────────────────────┘
                                │
                                │ release/<partner>/...
                                │ release/goat/...
                                ▼
                       goat fabric (release-bridge,
                       schema-enforcer, allowlist,
                       conditioning, audit emission)
```

Single host. Single bundle. Two networks (partner's local network + goat partner-net). The pod is the entirety of what the partner runs.

**Mesh classification ([ADR 1092](https://github.com/usareur-af/goatcore/blob/main/docs/adr/1092-moon-pod-peer-classes.md)):** when the attachment enrolls a peer directly in one of *our* goatnets (the ZAGROS sandbox pattern) rather than the partner-net, the peer carries a canonical node-class group — `<PREFIX>-POD` for a pod appliance inside our identity planes (our NetBird/Ory/CA), `<PREFIX>-MOON` for the uplink peer of a foreign-stack goatnet that runs its own complete stack (the foreign-goatnet pattern). Neither is a person role; posture details in mesh-and-ory-roles.md.

## Required pieces

A conformant goat-moon-pod has six pieces. Each piece has multiple acceptable implementations; the partner (or vendor building a non-`open` moon-pod) picks. The contract is that the *seven requirements* of [`partner-receiver-requirements.md`](https://github.com/usareur-af/goatcore/blob/main/docs/design/partner-receiver-requirements.md) are met at the boundary, not that any specific implementation is used.

### 1. Network client (partner-net peer)

Joins the goat-hosted **partner-net** — a separate netbird mgmt-plane instance distinct from the main fabric mgmt-plane — over WireGuard. Setup key embedded in the signed bundle at first deploy; rotated thereafter. Partner-net is goat-hosted *coordination metadata only*; partner data does not transit or persist there. Not a sovereignty violation per [`partner-receiver-requirements.md`](https://github.com/usareur-af/goatcore/blob/main/docs/design/partner-receiver-requirements.md) §"Sovereignty-floor edge cases" E1.

Acceptable implementations: native netbird client (default), wireguard-tools with manual config, equivalent ZTOL client.

### 2. Zenoh router

Single-instance `zenohd` joining the goat fabric over the WG tunnel established in piece 1. Publishes/subscribes under `release/<partner>/...` for outbound and `release/goat/...` for inbound (when bilateral).

Acceptable implementations: stock `eclipse/zenoh` upstream image, build-from-source per [ADR 0708](https://github.com/usareur-af/goatcore/blob/main/docs/adr/0708-build-zenoh-from-source.md), other Zenoh-protocol-compatible runtimes if any emerge.

### 3. Translation engine

Maps Zenoh-keyexpr-shaped data flows to/from whatever the partner's downstream systems can consume. The reference implementation uses `polygloat` (Apache 2.0) — canonical-as-pivot translation. Format adapters: REST API, gRPC, MQTT, Kafka, file-drop (JSONL/CBOR), syslog, TAK CoT, MISP/STIX-TAXII, S3-compatible object drop, webhook.

Acceptable implementations: polygloat (default), partner-built translator, vendor-translation-layer (if a vendor moon-pod has its own).

### 4. Local identity database

Holds the partner's operator credentials. Standards-compliant OIDC or SAML. Partner administers the database; goat does not host it. Per [`partner-receiver-requirements.md`](https://github.com/usareur-af/goatcore/blob/main/docs/design/partner-receiver-requirements.md) §"Sovereignty floor" requirement 6(b).

Acceptable implementations (the reference pod ships overlays for all four):

- **Kanidm** — Rust, single binary, built-in admin web UI, fits up to tens of operators; a strong default for partners new to OIDC.
- **Dex** — minimal CNCF-graduated, OIDC-only, no native UI; lightest if the partner's UI burden lands elsewhere.
- **Authelia** — what we already pilot on goat-side; familiar, ForwardAuth-shaped.
- **Zitadel** — heaviest, most full-featured, multi-tenant; for partners with larger operator populations.
- **External IdP** — partner already operates one (Keycloak, Ory, vendor IdP, ...); the pod's local IdP is bypassed and the partner's existing OIDC discovery URL is configured at install time.

Partner picks at install time. The pod's web UI auth federates to whatever they pick.

### 5. Audit sink

Retains 90+ days of access logs covering reads and writes to `release/<partner>/...` data. Per [`partner-receiver-requirements.md`](https://github.com/usareur-af/goatcore/blob/main/docs/design/partner-receiver-requirements.md) requirement 4. Format flexible.

Acceptable implementations: Loki (default in reference pod), journald with retention configured, OpenSearch, partner SIEM integration, OpenObserve, partner-existing-Splunk via syslog-forwarder.

### 6. Operator-facing web UI

Single-page application served from the pod, authenticated against the local IdP. Lets a non-engineer operator:

- Enroll the pod's first admin (one-time, via QR code or local-console flow).
- View incoming `release/goat/...` topics (when bilateral).
- Configure outbound translator adapters ("send these topics as JSONL to `/var/data/goat/`; send these as MQTT to `mqtt://my-broker/...`").
- View the pod's local audit log.
- View pod health (last-emission timestamp, last-handshake timestamp, partner-net reachability, fabric reachability).
- Rotate credentials, regenerate WG keys, replace the IdP overlay.

The UI is part of the pod bundle and ships in the same repo.

## Optional overlay: workload identity (SPIRE/SPIFFE) — Phase 3+

A conformant pod meets the seven-requirement contract **without** workload identity, so SPIFFE/SPIRE is **not** a required piece. It is offered as a **first-class optional overlay**, layered exactly like the local-IdP choice (piece 4): the partner picks their altitude.

- **What it adds:** the pod runs its own SPIRE trust domain (`spiffe://<partner>.pod`) and attests its own containers (zenoh router, polygloat, audit, UI) with short-lived X509-SVIDs. Because the pod is a *controlled bundle*, its workloads are attestable — unlike a bare external publisher.
- **Why the pod is the right home:** SPIFFE **federation** (pod trust domain ↔ goatnet trust domain, exchanging only public trust *bundles*) lets the pod's router carry a cryptographically-attested identity the goat-side ingress gateway (the [ADR 1020](https://github.com/usareur-af/goatcore/blob/main/docs/adr/1020-zenoh-federated-peering-trust-model.md) Tier-2 receiver) can verify — an upgrade over trusting the bundle-embedded setup key. Sovereignty-clean: SPIRE runs *inside* the pod on partner hardware; goat hosts no partner SVIDs.
- **Progressive disclosure, not a filter.** The 24-hour layman path stays an honest easy default — the base pod works end-to-end with no SPIRE. The SPIRE overlay is fully available to any partner who wants (or is required to have) attested workload identity. Neither path is second-class; offering the simple default does not withhold the capability.
- **Status:** Phase 3+, partner-gated, off the 24-hour path. The goat-side analysis + the ZAGROS sandbox proving-ground for the same pattern live at `../project/spiffe-spire-spike-2026-06-09.md` (§6 moon-pod fit) + `../project/zagros-spire-implementation-plan.md`. Until a Zenoh URI-SAN matcher lands upstream (backlog `zenoh-uri-san-acl-matcher-upstream`), an SVID admitted at the wire uses the CN-bearing-SVID bridge or is verified off the core router by the ingress gateway. The reference-implementation overlay (a `profiles/<…>-spire` overlay alongside the IdP overlays) lives in the standalone [`USAREUR-AF/zagros-moon-pod`](https://github.com/USAREUR-AF/zagros-moon-pod) repo.

## What lives where

Per [ADR 0223](https://github.com/usareur-af/goatcore/blob/main/docs/adr/0223-goat-moon-pod-repo-containment.md):

| Concern | Location |
|---|---|
| Requirements contract (the seven requirements) | This repo: [`partner-receiver-requirements.md`](https://github.com/usareur-af/goatcore/blob/main/docs/design/partner-receiver-requirements.md) |
| Boundary enforcement (release-bridge, schema-enforcer, allowlist, conditioning) | This repo: `zenoh-plugins/release-bridge/` + Ansible roles |
| Coordination plane (partner-net netbird-mgmt instance) | This repo: `terraform/` + `ops/ansible/` |
| Reference implementation (bundle, IdP overlays, hardware imaging, web UI, first-boot scripts, signing pipeline) | **Canonical:** separate repo [`USAREUR-AF/zagros-moon-pod`](https://github.com/USAREUR-AF/zagros-moon-pod). In-monorepo dev mirror at `incubator/goat-moon-open-pod/` (kept in sync; the standalone repo wins on divergence). |
| ZAGROS-partner collaboration surface | Separate repo [`USAREUR-AF/zagros-moon-pod`](https://github.com/USAREUR-AF/zagros-moon-pod) — the reference pod ZAGROS-defaulted (per-uuid slot profile) and **scrubbed for open partner co-dev**: no links into this private monorepo, no `ghcr.io/<org>` registry coupling (→ `ghcr.io/usareur-af`), no internal hostnames/mesh-IPs/operator-provisioning runbooks; the boundary contract is vendored self-contained. Where we and ZAGROS partners develop the pod together without us-internal content leaking. See "Bundle is the source of truth" below. |
| Vendor implementations | Vendor-controlled repos: `goat-moon-<product>-pod` |
| Per-vendor cookbooks | This repo: `../integrations/moon-pods/` |
| Conformance smoke-test suite | This repo: [`../../tests/moon-pod-conformance/`](https://github.com/usareur-af/goatcore/tree/main/tests/moon-pod-conformance) |
| Builder guide for vendors | This repo: [`goat-moon-pod-builder-guide.md`](goat-moon-pod-builder-guide.md) |

## Bundle is the source of truth (no stack over-specification)

The pod runs against multiple goatnets — nova, the ZAGROS sandbox, production — so the compose
stack, host scripts, and config templates **hardcode nothing stack-specific**. Every
environment-specific value arrives at first-boot from the **signed join bundle** (minted by the
operator onboarding flow, `goat admin onboard <handle> [--env <goatnet>]`); the handful the bundle
does not carry are plain **profile** vars under `profiles/<env>/`. Switching stacks is a different
bundle + profile, never a code change.

What the bundle carries (read by the pod, never hardcoded):

| Value | Bundle field | How the pod reads it |
|---|---|---|
| Mesh enrollment | `inner_mesh_setup.{management_url, setup_key}` | `goat-clientd --import-bundle` (or stock netbird on the sandbox) |
| Fabric router endpoint | `goat_cli_extras.router_endpoint` | `goat profile init` writes it to `profile.toml`; `first-boot.sh` reads it from there — the profile `ZENOH_FABRIC_ENDPOINT` is a fallback only |
| Operator portal | `goat_cli_extras.{portal_endpoint, portal_token}` | `goat profile init` |
| mTLS identity (the pod's slot CN) | `goat_cli_extras.{mtls_cert, mtls_key}` | extracted into the Zenoh router's mTLS material |
| CA roots | `goat_cli_extras.ca_roots` | verification trust; never compiled in |

The only values that legitimately live in a profile are the ones the bundle does not carry — the
bilateral inbound prefix (`INBOUND_NAMESPACE`, parameterized — not a hardcoded `release/...` in
the router template), audit retention, mesh mode, and the ACL subject axis. This
over-specification guard applies to all moon-pod repos (canonical + ZAGROS mirror + incubator);
the deltas live in each repo's `*-vs-production.md`.

## Two delivery shapes (same product)

The reference pod ships in two delivery shapes, both produced from the same source tree at [`USAREUR-AF/zagros-moon-pod`](https://github.com/USAREUR-AF/zagros-moon-pod):

### Shape A — software bundle

Partner runs `curl https://goat.../install.sh | sudo bash` on their existing Linux box (or downloads the install tarball + signature, verifies, runs offline). Bundle is a docker-compose / Quadlet stack with the six pieces above. First-boot script generates keys, prints QR code on console, partner scans to enroll first admin.

This is the default shape for any partner with a Linux host and the ability to run a containerized appliance.

### Shape B — pre-imaged hardware

For partners who genuinely have no Linux box. We hand them a small SFF computer (BOM candidates: OnLogic ML100, Lenovo ThinkCentre Tiny, Intel NUC, Raspberry Pi 5 8GB) pre-flashed with Ubuntu + the bundle + the first-boot script. They plug in power and ethernet; the device prints a QR code on a small attached display (or via local web UI on a discovered IP). They scan, enroll, done.

This shape is *available*, not *committed*. Whether we actually image and ship a device is a per-partnership decision — could equally be "here's the BOM and the imaging script, your local IT puts it together in 30 minutes."

The hardware-side `hardware/` subtree of the pod repo holds: BOM (with multiple candidate SKUs, not a single SKU), Packer templates, cloud-init files, first-boot key-rotation script, tamper-evident-packaging spec. Per [ADR 0223](https://github.com/usareur-af/goatcore/blob/main/docs/adr/0223-goat-moon-pod-repo-containment.md), software + hardware live in one repo because pre-imaged hardware is a delivery shape of the same product, not a different product.

## The 24-hour layman path

Calibrated for a non-engineer who can read a printed instruction sheet and follow steps. Times are upper bounds.

| T+ | Step | What's happening |
|---|---|---|
| 0 | Partner runs `install.sh` on their Linux host (Shape A) OR plugs in the pre-imaged device (Shape B) | Bundle expands; containers pull; first-boot script runs |
| 5 min | Pod prints a QR code on console (Shape A) or console display (Shape B), and a localhost URL | First-boot complete; default keys generated; sovereignty timer starts |
| 15 min | Partner scans QR with their phone → enrolls themselves as the pod's first admin in the local IdP → logs into the pod web UI | Partner now has IAL2/AAL2 identity in their own IdP, on their hardware |
| 30 min | Partner picks output adapters from the web UI dropdown ("I want JSONL files in `/var/data/goat/` and a webhook to `https://my-system.local/ingest`") | Translator configured; pod ready to receive |
| 1 hour | Goat issues their first per-topic allowlist entry through the policy engine ([ADR 0805](https://github.com/usareur-af/goatcore/blob/main/docs/adr/0805-policy-engine-active-passive.md)) | This is the only step that requires goat-side action; deliberate gating |
| 2 hours | First test message flows; partner sees it in their downstream system; audit log entry appears in pod web UI | Smoke test passed |
| 24 hours | Operational use begins; audit log accumulating; partner-side downstream consuming | The pod is now a sovereign endpoint |

This path is what the conformance smoke-test at [`../../tests/moon-pod-conformance/`](https://github.com/usareur-af/goatcore/tree/main/tests/moon-pod-conformance) validates against any candidate pod (`goat-moon-open-pod`, vendor moon-pods, partner-built moon-pods).

## Sovereignty preservation

The pod preserves partner sovereignty because:

- **Data lands on partner hardware** in partner's facility, on partner's power. Per [`partner-receiver-requirements.md`](https://github.com/usareur-af/goatcore/blob/main/docs/design/partner-receiver-requirements.md) requirement 6(a).
- **Partner's operators authenticate through partner's IdP** — even though we shipped the IdP binary, the partner administers it, the credential database is theirs. Per requirement 6(b).
- **Partner audit logs live in partner storage**; we do not aggregate. Per requirement 6(c).
- **Partner can fork the bundle** at any time and run the partner-net coordination plane themselves. The bundle is open source (Apache 2.0); the partner-net mgmt-plane Ansible roles + Terraform live in this repo as documented; there is no architectural lock-in. Per requirement 7.
- **Goat hosts only coordination metadata** (WG metadata, setup keys, peer addressing) on the partner-net mgmt-plane. Not in the data path; not a sovereignty violation. Per requirement 6 §E1.

What goat *does* host (and the partner depends on us for at Tier 0):

- The partner-net netbird-mgmt instance.
- The release-bridge that conditions data crossing into / out of the main fabric. For data
  arriving over a **Zenoh router peering**, the concrete receiver is the **ingress gateway** of
  [ADR 1020](https://github.com/usareur-af/goatcore/blob/main/docs/adr/1020-zenoh-federated-peering-trust-model.md) / [`zenoh-federated-peering-trust.md`](https://github.com/usareur-af/goatcore/blob/main/docs/design/zenoh-federated-peering-trust.md):
  a moon-pod is a Tier-2 (non-goat↔goat) peer, so its egress is authenticated, admitted
  origin-scoped, and provenance-recorded by a goat-operated gateway rather than federating
  directly into a core router. The v0 reference implementation incubates at
  `incubator/goat-moon-open-pod/`.
- The signed-bundle distribution + signing keys.
- The per-vendor cookbooks and conformance smoke-test suite.

The partner can take any of these in-house by transitioning to the [PRA](https://github.com/usareur-af/goatdocs/blob/main/docs/design/partner-reference-architecture.md), without re-onboarding under a different namespace or identity. That is the reversibility promise of requirement 7.

## What this design does not specify

- **Specific container runtime**: the bundle ships in docker-compose + Quadlet shapes; partner picks. A vendor moon-pod may use a different runtime entirely.
- **Specific bundle image registry**: the reference pod uses `ghcr.io/<org>/...`. Vendor moon-pods host their own.
- **Specific hardware SKU**: BOM lists multiple candidates; partner picks based on availability and budget.
- **Specific operator-UI styling**: the reference pod ships a clean default; vendor moon-pods style as they choose.
- **Specific audit format**: any 90-day-retention, query-capable system qualifies (per requirement 4).

These are all implementation choices, not contract concerns.

## Trigger conditions to revise this design

- **A required piece can't be cleanly satisfied by any acceptable implementation.** Revisit acceptable-implementation list; if no implementation works, revisit the piece.
- **A vendor moon-pod ships and surfaces a structural shape we didn't anticipate.** Update the design with the new shape; do not break vendor moon-pod by tightening the spec.
- **A partner deploys the pod and the 24-hour path takes substantially longer.** Investigate; either improve the bundle (most likely) or recalibrate the 24-hour claim (only if structurally impossible).
- **The two-delivery-shape model proves wrong** (e.g., one shape is never used, or a third shape emerges). Revisit.

## References

- [ADR 0223](https://github.com/usareur-af/goatcore/blob/main/docs/adr/0223-goat-moon-pod-repo-containment.md) — repo-containment + naming-pattern + vendor-extensibility-bones decision
- [ADR 0821](https://github.com/usareur-af/goatcore/blob/main/docs/adr/0821-mission-partner-sharing-pattern.md) — mission-partner-sharing anchor
- [`partner-receiver-requirements.md`](https://github.com/usareur-af/goatcore/blob/main/docs/design/partner-receiver-requirements.md) — the seven-requirement contract
- [`goat-moon-pod-builder-guide.md`](goat-moon-pod-builder-guide.md) — for vendors building their own
- [`partner-reference-architecture.md`](https://github.com/usareur-af/goatdocs/blob/main/docs/design/partner-reference-architecture.md) — Tier-0-aligned reference (when partner has more engineering capacity)
- [`mission-partner-sharing.md`](https://github.com/usareur-af/goatcore/blob/main/docs/design/mission-partner-sharing.md) — design companion to ADR 0821
- [`../partner-templates/data-sharing-charter.md`](https://github.com/usareur-af/goatdocs/blob/main/docs/partner-templates/data-sharing-charter.md) — formal charter
- `zenoh-data-fabric.md` — substrate
- `../integrations/moon-pods/` — per-vendor cookbooks
- [`../../tests/moon-pod-conformance/`](https://github.com/usareur-af/goatcore/tree/main/tests/moon-pod-conformance) — conformance smoke-test suite
- [`USAREUR-AF/zagros-moon-pod`](https://github.com/USAREUR-AF/zagros-moon-pod) — reference implementation
- `polygloat` — translation engine
