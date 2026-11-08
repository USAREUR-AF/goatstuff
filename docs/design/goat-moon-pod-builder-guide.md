# goat-moon-pod builder guide — for vendors and partners building their own

> **Scope: a moon pod is a learning tool.** Partners use it in an exercise environment to learn how goat works. It is not the operational path for sharing data between nations. Operationally, each sovereign runs its own environment and manages its own pods, and data passes between sovereigns only across boundary nodes that federate their environments ([ADR 1157](https://github.com/usareur-af/goatcore/blob/main/docs/adr/1157-cross-sovereign-federation-edge-terminated.md), [ADR 1208](https://github.com/usareur-af/goatcore/blob/main/docs/adr/1208-sovereign-environments-release-namespaces-and-grant-encryption.md)). Where this document describes a pod joining another nation's environment, read it as the learning setup.

| Field | Value |
|---|---|
| Status | Proposed (Phase 2D-late / Phase 3 commitment) |
| ADR anchor | [`../adr/0223-goat-moon-pod-repo-containment.md`](https://github.com/usareur-af/goatcore/blob/main/docs/adr/0223-goat-moon-pod-repo-containment.md) |
| Companions | [`moon-pod-conformance.md`](moon-pod-conformance.md), [`goat-moon-pod.md`](goat-moon-pod.md) |
| Audience | Vendors with a data platform or sensor product; partners with engineering capacity who want to build their own moon-pod |
| Reference implementation | [`USAREUR-AF/zagros-moon-pod`](https://github.com/USAREUR-AF/zagros-moon-pod) (worked example, Apache 2.0; private during MVP, public at GA) |

## Why this guide exists

Goat does not own every implementation of the partner-receiver pattern. We publish the requirements contract, ship one reference implementation as a worked example, and explicitly invite the larger technological community to build their own conformant moon-pods. This guide is for vendors and partners who want to build.

We commit to maintaining the contract, the reference implementation, and the conformance smoke-test suite. We do not commit to evaluating every possible alternative, but we will engage with any candidate that asserts conformance and offers itself for evaluation.

This guide is intentionally lightweight. Per "don't overdo preoptimization": we leave just enough scaffolding for a vendor to build, not a full SDK. If vendor build volume justifies more scaffolding later, we add it then.

## Three things you need to read first

1. **The contract**: [`moon-pod-conformance.md`](moon-pod-conformance.md) — the seven requirements your moon-pod must meet at the boundary. Includes the moon-metaphor definition, the sovereignty-floor edge cases, and the conformance criteria.
2. **The architectural shape**: [`goat-moon-pod.md`](goat-moon-pod.md) — the abstract spec for what a moon-pod looks like. Six pieces, multiple acceptable implementations of each.
3. **The repo-containment decision**: [ADR 0223](https://github.com/usareur-af/goatcore/blob/main/docs/adr/0223-goat-moon-pod-repo-containment.md) — naming pattern (`goat-moon-<provider>-pod`), where each kind of moon-pod's code lives, why the contract is in trunk and the implementation in a separate repo.

**New to Zenoh?** A moon-pod's piece #2 is a Zenoh router. If pub/sub / queryable / get are unfamiliar, spend ten minutes in the browser sandbox first: `zenoh-sandbox.md` (`sandbox.nova.netbird.goat.example`, mesh-internal) lets you open two tabs and watch the four operations work live against an isolated throwaway router — no cert, no setup, no real data. It is a learning aid, not part of any moon-pod bundle.

## The naming convention

Per [ADR 0223](https://github.com/usareur-af/goatcore/blob/main/docs/adr/0223-goat-moon-pod-repo-containment.md), the pattern is `goat-<plane>-<provider>-<service>`:

- Our reference implementation: `goat-moon-open-pod`
- Vendor implementations: `goat-moon-<product>-pod` — e.g., `goat-moon-acme-pod`, `goat-moon-widget-pod`
- Partner-built (no vendor product): the partner picks an identifier. We do not arbitrate naming.

Use this convention so partners and operators can find your moon-pod through the goat discoverability surface (`../integrations/INDEX.md`).

## What you have to provide

A conformant goat-moon-pod must:

1. **Meet all seven requirements** at the boundary (see [`moon-pod-conformance.md`](moon-pod-conformance.md)).
2. **Pass the conformance smoke-test suite** at [`../../tests/moon-pod-conformance/`](https://github.com/usareur-af/goatcore/tree/main/tests/moon-pod-conformance) when run against a fresh deployment.
3. **Document the partner-facing 24-hour deployment path** in your README.
4. **Document your moon-pod's mapping** of the six required architectural pieces (per [`goat-moon-pod.md`](goat-moon-pod.md) §"Required pieces") to your specific implementation choices. Partners need to know what they're getting.
5. **Use the naming convention** above so your moon-pod is discoverable.
6. **Cite the contract by version**. Pin the trunk commit / git tag of [`moon-pod-conformance.md`](moon-pod-conformance.md) you target. When trunk advances the contract, your moon-pod tracks the bump on its own cadence.

## What you don't have to provide

- **A specific container runtime, image registry, or hardware SKU.** Implementation choice.
- **A specific IdP, audit system, or translation engine.** Multiple acceptable implementations exist; pick what works for your product.
- **An open-source license.** The contract does not require open-sourcing your moon-pod. We strongly recommend it for adoption reasons, but it is not a conformance criterion.
- **Pre-approval from goat.** You do not need our blessing to start building. Conformance evaluation happens at partnership-onboarding time, not before.

## How to use the reference implementation

[`USAREUR-AF/zagros-moon-pod`](https://github.com/USAREUR-AF/zagros-moon-pod) is a worked example, not a template you must follow. Useful uses:

- **Read it to understand a conformant shape.** Our pod implements all six pieces; reading the source shows one way each piece can be satisfied.
- **Copy specific overlays.** The IdP overlays (Kanidm / Dex / Authelia / Zitadel) are reusable across moon-pods if they fit your product shape.
- **Crib the first-boot script.** Sovereignty-transition mechanics (rotating every shipped credential before declaring operational) are the same for every moon-pod. Don't reinvent this; use ours or copy it.
- **Run our conformance smoke-test against your moon-pod.** Same suite, same expectations, regardless of what you're building.

You should not feel obligated to reuse anything. A vendor moon-pod that is structurally different (e.g., a moon-pod hosted inside a vendor's data platform, where the "container" is an application running inside that platform) will look very different from `goat-moon-open-pod` and that is fine.

## How to handle partner-side identity

The trickiest piece for vendors is requirement 1 (identity assurance + IdP standard) and requirement 6(b) (partner controls operator credentials). Three patterns work:

- **Pod ships a local IdP, partner administers it.** What `goat-moon-open-pod` does. Partner has full custody.
- **Pod federates to a partner-existing IdP at install time.** If the partner already operates an OIDC-compliant IdP (Keycloak, Ory, vendor IdP), the moon-pod's web UI just consumes that. No local IdP shipped. Partner has full custody by definition.
- **Pod federates to a vendor-managed IdP that the partner has tenant control over.** Acceptable *if* the partner controls the credential database for their tenant — they can add/remove operators, the vendor cannot read or modify operator credentials, and the partner can export the credential database on demand. If the vendor holds the partner's credentials in an unrecoverable form, this fails requirement 6(b).

When in doubt, default to pattern 1 or 2. Pattern 3 requires careful vendor architecture and we'll need to review it explicitly during conformance evaluation.

## How to handle partner-side data

Requirement 6(a) says partner data lands at rest only on partner-controlled infrastructure. Patterns:

- **Pod runs on partner hardware, data lands on local disk.** Default for `goat-moon-open-pod`.
- **Pod runs in a partner-controlled cloud account, data lands in partner-controlled storage.** Acceptable. The partner pays the cloud bill on a partner-controlled account.
- **Pod runs in vendor-hosted SaaS, data lands in vendor storage.** **Fails the sovereignty floor.** Not a conformant goat-moon-pod. The vendor would need to publish a self-hosted variant for partner-controlled deployment.

If your product is fundamentally a vendor-hosted SaaS, you can still publish a goat-moon-pod that integrates with your SaaS — but the *data* must persist on partner infrastructure, not in your SaaS. The moon-pod is the partner's data plane; your SaaS, if involved, is a control-plane consumer that fetches data from the moon-pod under partnership terms.

## How to publish a conformance claim

When your moon-pod is ready:

1. Run [`../../tests/moon-pod-conformance/`](https://github.com/usareur-af/goatcore/tree/main/tests/moon-pod-conformance) against a fresh deployment of your moon-pod. Report all results in your README.
2. Open a PR against this repo adding a row in `../integrations/INDEX.md` pointing at your repo + linking to your conformance-test results.
3. (Optional, encouraged) Open a PR adding a cookbook at `../integrations/moon-pods/<your-product>.md` sketching how your product's API surface maps to `release/<partner>/...` keyexprs. We accept PRs from anyone.
4. When a partner shows up wanting to use your moon-pod, we evaluate against the contract at partnership-onboarding time. If conformance holds, we work with that partner.

We do not issue conformance certifications today. Your conformance claim is what you publish; goat's evaluation is what we do at onboarding time. If volume of vendor moon-pods grows past ~3 distinct products in production, we revisit whether a lightweight conformance-attestation process is warranted.

## Where to ask questions

- **Contract clarifications**: open an issue against this repo referencing [`moon-pod-conformance.md`](moon-pod-conformance.md). We treat contract-clarification issues as priority because they affect every conformant implementation.
- **Cookbook contributions**: PR against `../integrations/moon-pods/`. We merge cookbooks for any vendor product with a public API surface; we do not require the vendor to be a goat partner to merge a cookbook.
- **Reference-implementation issues**: open against [`USAREUR-AF/zagros-moon-pod`](https://github.com/USAREUR-AF/zagros-moon-pod). Our pod is the worked example; bugs there are bugs in goat.
- **Partnership inquiries**: contact channels in the [Partner Data Sharing Charter](https://github.com/usareur-af/goatdocs/blob/main/docs/partner-templates/data-sharing-charter.md). The charter is a precondition for actual data flow regardless of which moon-pod the partner uses.

## Out of scope for this guide

- **A `goat-moon init` scaffolder / SDK.** Deferred until at least one external vendor actually starts building.
- **A formal vendor-conformance-certification process.** Deferred until at least one external vendor actually ships.
- **Cross-moon-pod federation or peering.** Each moon-pod is sovereign-orbital; no inter-moon-pod relationships at this layer.

If you build something here you would like to upstream, open an issue first to discuss scope; we want to keep the contract surface narrow per the no-overdo-preoptimization guidance.

## References

- [`moon-pod-conformance.md`](moon-pod-conformance.md) — the seven-requirement contract
- [`goat-moon-pod.md`](goat-moon-pod.md) — abstract spec
- [ADR 0223](https://github.com/usareur-af/goatcore/blob/main/docs/adr/0223-goat-moon-pod-repo-containment.md) — repo-containment + naming-pattern decision
- [ADR 0821](https://github.com/usareur-af/goatcore/blob/main/docs/adr/0821-mission-partner-sharing-pattern.md) — mission-partner-sharing anchor
- [`../partner-templates/data-sharing-charter.md`](https://github.com/usareur-af/goatdocs/blob/main/docs/partner-templates/data-sharing-charter.md) — partnership prerequisite
- `../integrations/INDEX.md` — discoverability surface
- `../integrations/moon-pods/` — per-vendor cookbooks
- [`../../tests/moon-pod-conformance/`](https://github.com/usareur-af/goatcore/tree/main/tests/moon-pod-conformance) — conformance smoke-test suite
- [`USAREUR-AF/zagros-moon-pod`](https://github.com/USAREUR-AF/zagros-moon-pod) — reference implementation
