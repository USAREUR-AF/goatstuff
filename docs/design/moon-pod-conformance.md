# Moon-pod conformance — exercise environment

> **Scope: exercise environment only.** This is the list a moon pod meets to join an exercise environment, where partners learn how goat works. It is not how nations share data in operation. Operationally, each nation runs its own environment: its own pods, NetBird coordination plane, routers, republisher and conditioning. Data passes between nations only across boundary nodes that federate their environments ([ADR 1157](https://github.com/usareur-af/goatcore/blob/main/docs/adr/1157-cross-sovereign-federation-edge-terminated.md), [ADR 1208](https://github.com/usareur-af/goatcore/blob/main/docs/adr/1208-sovereign-environments-release-namespaces-and-grant-encryption.md)). Nothing in this list places one nation's data, identity, audit or infrastructure in another nation's environment. There is no NetBird-to-NetBird bridge.
>
> Moved from goatcore `docs/design/partner-receiver-requirements.md`.

| Field | Value |
|---|---|
| Status | Proposed (Phase 2D-late / Phase 3 commitment) |
| ADR anchor | [ADR 0223](https://github.com/usareur-af/goatcore/blob/main/docs/adr/0223-goat-moon-pod-repo-containment.md) |
| Companions | [`goat-moon-pod.md`](goat-moon-pod.md), [`goat-moon-pod-builder-guide.md`](goat-moon-pod-builder-guide.md), [`partner-reference-architecture.md`](https://github.com/usareur-af/goatdocs/blob/main/docs/design/partner-reference-architecture.md), [`mission-partner-sharing.md`](https://github.com/usareur-af/goatcore/blob/main/docs/design/mission-partner-sharing.md) |

## What this is

The **conformance list** for any pod that wants to send and receive data in a goat exercise environment on behalf of a partner without engineering staff on the partner side.

This document defines the **requirements** for the exercise, not an implementation. There are several conformant paths:

- **`moon-pod`** — our reference implementation. Apache 2.0 (private during MVP, public at GA), lives at [`USAREUR-AF/zagros-moon-pod`](https://github.com/USAREUR-AF/zagros-moon-pod) (separate repo per [ADR 0223](https://github.com/usareur-af/goatcore/blob/main/docs/adr/0223-goat-moon-pod-repo-containment.md)). Designed to deploy in 24 hours by a non-engineer.
- **Vendor goat-moon-pods** — third-party products that conform to this list. Naming convention: `goat-moon-<product>-pod`. Per-vendor cookbooks at goatcore `docs/integrations/moon-pods/`.
- **Partner roll-your-own** — a partner with engineering capacity that prefers to build their own. We hold them to this list; we do not provide implementation help.

The list is the same for all three. We commit to maintaining the list and `goat-moon-open-pod`. We do not commit to evaluating every possible alternative, but we will engage with any candidate that asserts conformance and offers itself for evaluation against this document.

## The "moon" metaphor (defined once, used everywhere)

In the exercise, partner pods orbit the exercise environment's fabric like moons orbit a planet. Each moon is its own sovereign body — its own hardware, its own identity database, its own audit log, its own administrative custody — held in a gravitational (data-flow) relationship with the goat fabric, not contained within it. Multiple moons coexist; each can be ours (`goat-moon-open-pod`) or a vendor's (`goat-moon-<product>-pod`); each is independently sovereign.

A **goat-moon-pod** (or just **moon-pod**) is a deployable instance of this orbital pattern — the artifact a partner runs to participate. The naming convention is `goat-<plane>-<provider>-<service>` where:

- `goat` — project
- `moon` — relationship plane (sovereign-orbital partner)
- `<provider>` — `open` for our reference implementation, vendor-product-name otherwise
- `pod` — service type (a deployable unit)

We use the term `goat-moon-pod` throughout this document and its companions. Future relationship planes (if any emerge) follow the same pattern with a different middle term.

## Posture

- We are **not trying to own the world** or boil the ocean. We are unblocking the gnarly problem that most plausible mission partners cannot stand up the full PRA in a reasonable time, and we are unleashing the larger technological community to build conformant moon-pods of their own.
- The list is **prescriptive on the boundary, not on the implementation**. Any path that meets the seven requirements is acceptable.
- The reference implementation `goat-moon-open-pod` is **an easy button, not the only way**. Partners with their own engineering capacity, or partners using a conformant vendor product, do not need to use ours.
- Vendors are explicitly invited to build conformant moon-pods.

## The seven requirements

### 1. Identity assurance + IdP standard

Every partner-side operator who can read or publish `release/<partner>/...` carries an identity issued by an IdP that emits standards-compliant assertions and meets a defined assurance level.

- **Floor:** NIST SP 800-63 IAL2 / AAL2 (verified identity proofing + phishing-resistant MFA). OIDC discovery endpoint reachable from the goat-side bridge to the partner side. SAML 2.0 acceptable as an alternative with a documented integration note explaining the assertion shape.
- **Below the floor:** assurance levels lower than IAL2/AAL2; identity systems without an OIDC discovery endpoint or equivalent SAML metadata; bearer-token-only systems with no operator-identity binding.
- **Why this line:** below IAL2/AAL2, "an unknown partner-side principal accessed releasable data" is the audit record we'd be left with. That is not a defensible record.

### 2. Network / transport sovereignty

Bytes between the partner edge and the goat edge are end-to-end encrypted with keys neither side delegates to a third party, and the partner-side network handshake is silent at the public boundary.

- **Floor:** WireGuard preferred (silent handshake, sovereign key custody, by-design property). IPsec acceptable. mTLS over public HTTPS acceptable as a fallback with documented loss of the silent-control-plane property at the partner boundary.
- **Below the floor:** SaaS VPN providers where the provider holds one side of the keys; networks terminating in cloud-vendor-managed PKI without partner key custody; plaintext transports.
- **Why this line:** this is the same invariant we hold for our own substrate (see [ADR 0313](https://github.com/usareur-af/goatcore/blob/main/docs/adr/0313-silent-control-plane-outer-wg-wrap.md), [ADR 0314](https://github.com/usareur-af/goatcore/blob/main/docs/adr/0314-no-public-dns-for-control-plane.md)) — "no part of the secure plane on the public internet." Relaxing it at the partner boundary effectively publishes the boundary.

### 3. Landing interface contract

Data flows in and out at well-known Zenoh keyexpr(s) under the `release/<partner>/...` namespace, OR at a documented adapter the partner runs on their side that maps cleanly to those keyexprs.

- **Floor:** a single named landing point with a documented schema for what flows through it. Acceptable adapter shapes: REST, gRPC, MQTT, Kafka, file-drop, webhook, syslog, TAK CoT, MISP/STIX-TAXII, S3-compatible object drop. Native Zenoh peer is the preferred shape (zero translation cost, full keyexpr discipline).
- **Below the floor:** "send me data however you want" — no contract, no per-topic auditability, no schema discipline. Bespoke per-message protocols negotiated out of band.
- **Why this line:** without a contract, every partner becomes bespoke integration work, per-topic release decisions cannot be cleanly audited, and `polygloat` cannot do format fanout because there is no canonical pivot.

### 4. Audit retention

The partner retains 90+ days of access logs covering reads and writes to `release/<partner>/...` data on their side, queryable on demand under partnership terms.

- **Floor:** 90 days minimum; format flexible (Loki, OpenSearch, journald with retention configured, Splunk, OpenObserve, partner SIEM all qualify). Queryable answer to "who accessed topic X at time T from identity Y?" must be reachable.
- **Below the floor:** retention shorter than 90 days; logs that cannot tie an access event to a specific operator identity; logs not covering the `release/<partner>/...` namespace.
- **Why this line:** NIST SP 800-53 AU-11 floor; below 90 days, post-incident forensics is structurally impossible. We do not require partners to ship audit logs to us — symmetric audit federation is a separate negotiation. We do require retention plus query.

### 5. Schema declaration discipline

Any topic the partner publishes under `release/<partner>/...` carries a schema declaration at `attest/schema/<topic>` (registered) or `attest/schema-declaration/<topic>` (binary-declared with documented external schema). Partner consumption of `release/goat/...` topics relies on schemas we publish on the same convention.

- **Floor:** undeclared partner publishes are rejected at the exercise environment's release-bridge by the schema-enforcer interceptor. No "we'll declare it later" exemptions. The data-dictionary discipline ([`data-dictionary.md`](https://github.com/usareur-af/goatcore/blob/main/docs/design/data-dictionary.md)) applies symmetrically at the partner boundary.
- **Below the floor:** opaque payloads with no schema; ad-hoc schema descriptions that change without versioning; topics that exist on the wire but not in the data dictionary.
- **Why this line:** every downstream consumer needs schema for citation, lineage, and downstream conditioning to function. An opaque partner topic forces every downstream task to degrade to "trust the bytes, don't ask what they mean."

### 6. Sovereignty floor

Three nested invariants, all of which must hold:

- **(a) Data:** all releasable partner data lands at rest only on partner-controlled hardware/infrastructure. Partner-controlled means the partner owns the host, or pays the cloud bill on a partner-controlled account, *and* the partner controls administrative access.
- **(b) Identity:** partner operator credentials live in the partner's controlled IdP. We may ship the IdP binary (Kanidm, Dex, Authelia, Zitadel, or other); once it is running on the partner's host and the partner has enrolled their first admin, the credential database is theirs. We do not host their user database.
- **(c) Audit:** partner audit logs live in partner storage. We do not aggregate partner audit by default; symmetric audit federation is a separate negotiation per partnership.

- **Floor:** if any of the three flips — partner data persisting on goat hosts, partner credentials living in goat-managed storage, partner audit aggregated into goat infrastructure by default — it is not sovereign.
- **Why this line:** this is the difference between "we publish a substrate the partner operates" (our posture, [ADR 0512 §"Posture"](https://github.com/usareur-af/goatcore/blob/main/docs/adr/0512-rapid-adaptation-with-ai.md)) and "we host the partner's platform" (the vendor-platform-as-tenant shape, which we explicitly reject). Without this line the easy button collapses into a SaaS offering wearing a sovereignty costume.

#### Sovereignty-floor edge cases (explicit resolution)

The sovereignty floor needs four edge cases nailed down because they are exactly where a hostile reviewer would push:

**E1. The exercise environment's coordination plane.** In the exercise environment, goat runs a separate NetBird management plane for moon pods, distinct from the main fabric management plane. It holds WG public keys, setup keys, peer addressing, peer-name and group metadata and last-seen timestamps. It does **not** hold partner data, operator credentials or audit logs.
- **Resolution:** in the exercise, this coordination plane is a connection-metadata service, not in the data path, and not a sovereignty violation. Requirement 6 covers data at rest, identity custody and audit storage; connection metadata is excluded. Operationally there is no shared coordination plane: each nation runs its own, and there is no NetBird-to-NetBird bridge.

**E2. Release-bridge in the exercise data path.** In the exercise environment, a goat-run release-bridge (see [`mission-partner-sharing.md`](https://github.com/usareur-af/goatcore/blob/main/docs/design/mission-partner-sharing.md) §"Bridge data-conditioning checkpoint") processes partner data in flight: schema enforcement, allowlist, conditioning and audit emission.
- **Resolution:** in the exercise, partner data may transit that bridge in flight and may not persist at rest in goat infrastructure. Requirement 6 is about *data at rest*, not *data in motion*. Operationally, each nation runs the republisher and conditioning for what it releases, on its own hosts ([ADR 1157](https://github.com/usareur-af/goatcore/blob/main/docs/adr/1157-cross-sovereign-federation-edge-terminated.md)).

**E3. Goat-shipped binaries (IdP, pod bundle, polygloat) running on the partner's host.** Sovereignty is about administrative custody, not code provenance. The partner runs the binary on their host, the first-boot script rotates every shipped credential and key, the source is open and replaceable.
- **Resolution:** sovereignty begins at the partner's first-boot completion, after default keys and credentials are rotated. The bundle in transit is not yet a sovereign asset; it becomes sovereign at takeover.

**E4. Pre-imaged hardware in transit.** From the moment we image a hardware unit to the moment the partner takes physical custody and runs first-boot, the device is in supply-chain status (covered by bundle-signing, tamper-evident packaging, and Block 40 reproducible-build discipline).
- **Resolution:** same line as E3 — sovereignty timer starts at first-boot completion. Supply-chain integrity covers the transit segment.

If a fifth edge case emerges, append it here in the same shape (situation, resolution, requirement-clause it ties to). Do not relax requirement 6 in response to an edge case; instead, add an explicit exclusion with rationale.

### 7. Leaving the exercise cleanly

A partner that learned on a moon pod builds its own sovereign environment ([ADR 1208](https://github.com/usareur-af/goatcore/blob/main/docs/adr/1208-sovereign-environments-release-namespaces-and-grant-encryption.md)) and federates with other nations through boundary nodes. Its own environment has its own certificate authority, identities and management. Nothing on the pod locks the partner in.

- **Floor:** WG keys, IdP, hardware and bundle version can all be rotated or replaced; the pod holds nothing the partner's own environment depends on.
- **Below the floor:** identity systems that cannot be replaced; allowlist entries tied to specific bundle internals rather than the partner identity.
- **Why this line:** the pod lowers the cost of learning. Without a clean exit it becomes a dependency.

## Conformance

A candidate goat-moon-pod (ours, vendor's, or partner-built) is **conformant** when all seven requirements hold at the boundary. Conformance is asserted by the implementation, evaluated by goat against the list, and validated by the conformance smoke-test suite at [goatcore `tests/moon-pod-conformance/`](https://github.com/usareur-af/goatcore/blob/main/tests/moon-pod-conformance/).

The smoke-test suite walks the 24-hour deployment path against a candidate pod and asserts each of the seven requirements. The suite runs on `goat-moon-open-pod` continuously; vendors and partners may run it locally to self-check before requesting evaluation.

We do not issue conformance certifications today. Conformance is a partnership-by-partnership evaluation against this document. If volume of vendor-built moon-pods grows past ~3 distinct products in production, we revisit whether a lightweight conformance-attestation process is warranted.

## Vendor candidates

### Others

Any vendor or partner with a standards-compliant identity system and the ability to terminate WireGuard / IPsec / mTLS at their edge is a candidate. We accept conformance evaluations against the list above; we do not require pre-approval to attempt one.

## Out of scope

- **Cross-domain (CDS) conditioning.** The exercise is unclassified-only. Cross-classification release is governed by [ADR 0504](https://github.com/usareur-af/goatcore/blob/main/docs/adr/0504-cds-conditioning.md) and is a separate boundary discipline.
- **Real-time push-based partner revocation.** Current revocation is allowlist-removal + trust-revocation; push-based revocation deferred until needed.
- **Conformance certification process.** Today's conformance is partnership-by-partnership evaluation. A formal certification process is deferred until vendor moon-pod volume justifies it.
- **Per-vendor SDK / scaffolder.** Until at least one external vendor actually starts building, we do not invest in a `goat-moon init` scaffolder.

## Trigger conditions to revise this list

- **A requirement consistently disqualifies plausible partners.** Revisit the floor placement.
- **A new edge case for the sovereignty floor surfaces.** Append to §"Sovereignty-floor edge cases" with explicit resolution.
- **A vendor moon-pod ships and our cookbook misrepresents the integration.** Update the cookbook; if the list itself was wrong, update the list.
- **Vendor moon-pod count crosses ~3 in production.** Revisit whether a lightweight conformance-attestation process is warranted.
- **Cross-domain release becomes a partner requirement.** Cross-link the CDS conditioning pattern; do not extend this list to cover it.

## References

- [ADR 0223](https://github.com/usareur-af/goatcore/blob/main/docs/adr/0223-goat-moon-pod-repo-containment.md) — repo-containment + naming-pattern decision
- [ADR 0821](https://github.com/usareur-af/goatcore/blob/main/docs/adr/0821-mission-partner-sharing-pattern.md) — mission-partner-sharing anchor
- [`goat-moon-pod.md`](goat-moon-pod.md) — abstract spec for moon-pods + reference-implementation pointer
- [`goat-moon-pod-builder-guide.md`](goat-moon-pod-builder-guide.md) — vendor-extensibility guide
- [`partner-reference-architecture.md`](https://github.com/usareur-af/goatdocs/blob/main/docs/design/partner-reference-architecture.md) — partner reference architecture
- [`mission-partner-sharing.md`](https://github.com/usareur-af/goatcore/blob/main/docs/design/mission-partner-sharing.md) — design companion
- [`../partner-templates/data-sharing-charter.md`](https://github.com/usareur-af/goatdocs/blob/main/docs/partner-templates/data-sharing-charter.md) — formal charter
- [`data-dictionary.md`](https://github.com/usareur-af/goatcore/blob/main/docs/design/data-dictionary.md) — schema discipline at boundary
- goatcore `docs/design/zenoh-data-fabric.md` — substrate + keyexpr conventions
- goatcore `docs/integrations/moon-pods/` — per-vendor cookbooks
- [goatcore `tests/moon-pod-conformance/`](https://github.com/usareur-af/goatcore/blob/main/tests/moon-pod-conformance/) — conformance smoke-test suite
- [ADR 1157](https://github.com/usareur-af/goatcore/blob/main/docs/adr/1157-cross-sovereign-federation-edge-terminated.md) and [ADR 1208](https://github.com/usareur-af/goatcore/blob/main/docs/adr/1208-sovereign-environments-release-namespaces-and-grant-encryption.md) — how nations share in operation
