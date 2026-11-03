# trust-sandbox — a live, local zero-trust identity playground

A disposable, **fully local** playground that lets a visitor *feel* how the five
identity layers of goat net compose into zero trust — **NetBird** (network admission),
**x509 leaf certs** (cryptographic identity + the URI-SAN grammar), **SPIRE/SPIFFE**
(attested, short-lived workload identity), **Ory** (the authenticated human/role behind
an action), and **Zenoh ACLs** (the per-write authorization decision) — plus **private
mesh DNS** as the *addressing* layer that, by design, carries **no** trust at all. The
thesis the playground proves by experiment: **you cannot keep only some of these and
still have zero trust** — knock any one layer out and a specific trust property visibly
breaks; and because the name confers no trust, the service behind it can be **burned down
and respawned** freely, with identity re-verified cryptographically every time.

It is the identity-and-permission sibling of the Zenoh learning sandbox
(`zenoh-sandbox.md` / [`zenoh-sandbox-ux.md`](zenoh-sandbox-ux.md)):
same warm/tacticool Vite+Vue3 experience, same active+passive shape, same
"it's *real*, not a cartoon" bar. Where the Zenoh sandbox teaches *what pub/sub is*,
this teaches *why every layer of identity is load-bearing*.

| Field | Value |
|---|---|
| Status | **Built + verified + deployed live on ZAGROS** (track `trust-sandbox-scaffold`, [ADR 1049](https://github.com/usareur-af/goatcore/blob/main/docs/adr/1049-zero-trust-identity-sandbox.md)). All seven phases run end-to-end via `docker compose up`: real CA-cert mTLS write (allow + silent-deny + handshake-refusal), real SPIRE SVID (CN-bearing, ~60s rotation) authorizing a workload write, real Ory/Hydra browser OIDC → `groups`-gated mint/rotate/revoke, real CoreDNS resolve + burn-&-respawn. Hosted live (one mesh-only, Ory-gated instance) at `https://trust-sandbox.zagros.netbird.zagros.example/` — see §Delivery. Per-session multi-tenancy = v2. |
| Surface | **Local only** — `docker compose up` → `http://localhost:8100`. No mesh, no Thor, no Traefik. |
| Lifecycle | Ephemeral: `docker compose down -v` is a full wipe. Holds nothing real, ever. |
| Audience | Engineers *and* stakeholders — "what does zero trust actually look like, and why all five?" |
| Lexicon | ZAGROS words (org, not vendor) via the same serve-time substitution as the portal ([`portal-lexicon.md`](https://github.com/usareur-af/goatcore/blob/main/docs/design/portal-lexicon.md)) |

## In plain language — what it illustrates (the opening narrative)

"Zero trust" usually lands as a slogan, and the picture people walk away with is
wrong in two ways: that there's a **wall** — get past it and you're in — and that
the point is **catching attackers**. Both miss it. There is no inside: every
action is re-checked by several independent things, each asking a question the
others can't. And catching attackers is only a fraction of the value.

**Most of what goes wrong isn't an attack.** The honest frame is not "good user
vs. bad actor" — it's a spectrum of ordinary situations the system must keep
coherent: a producer that's **noisy or overloaded** (a bug, not malice) that
mustn't starve everyone else; a service that's **broken** and must be **swapped
out now** without re-addressing every consumer; a credential that's simply gone
**stale** and should stop working on its own; a new participant that needs to come
online **fast** and later go offline cleanly on local authority; and, occasionally,
**someone hostile** — who gets contained by the very same checks, as a special case
of "doesn't have the right identity for this." The layers aren't there to spot
villains; they're there so the system stays coherent under all of this at once.

**Five questions, complementary roles** (each fills a job none of the others can):
① *Can your packets even get here?* (network) — reaching the door isn't permission,
and it's where a flood gets capped. ② *Who are you, provably?* (a short-lived cert,
not a name or IP) — so identity can be issued, scoped, and withdrawn precisely.
③ *Still valid right now?* (SPIRE) — credentials rotate and lapse on their own, so
stale resolves itself and a replaced service re-proves itself. ④ *Who is the actor
and role?* (Ory) — authenticate the actor, which is what lets authority be
**delegated** (an admin issues/revokes for others). ⑤ *May you do this specific
thing?* (the ACL, default-no) — fine-grained and changeable per identity, so access
is granted and rescinded **live**, without a rebuild. Keep only some and the
strategy stops being coherent — a gap opens that one of the others was quietly
covering.

**The freedom half.** The same machinery that contains misbehavior is what makes
identity + access fast and local to manage, where traditional setups are slow and
centralized: **sovereign/federated** (a group manages its own identities on its own
authority), **dynamic** (grant/scope/rescind is a live change, not a rebuild),
**self-healing** (per-connection re-verification + auto-expiry means stale things
lapse and replaced things re-prove with no human in the loop; a noisy/flooding
producer is *attributable* to its identity — it shows up on the health/throughput
surface and its credential can be revoked to cut it off, with automatic
rate-throttle on the roadmap).
And the **name carries no trust** by design (private mesh DNS): resolving a name
yields an address and nothing else, which is exactly what makes services
**disposable** — burn one down, respawn a fresh one under the same name, trust
re-earned per connection, nobody re-addressed.

The playground makes all of this something a visitor *does*, not is told. The
stakeholder-facing version of this narrative is the in-UI **vision page**
(`vision.html`).

## Why local, and why that is *more* honest

Running the whole stack on a laptop (not the ZAGROS Thor) is a deliberate fit, not a
downgrade:

- **The ephemeral contract is the whole point.** Like the Zenoh playground's standalone
  router, this holds nothing real and peers with nothing. Local `down -v` makes "wiped
  clean" structural — there is no shared live env to corrupt.
- **It is the only layer we can't make literally live anyway.** A browser cannot perform
  a WireGuard handshake, so the NetBird layer is *represented* regardless of where the
  rest runs. Locally there is simply no mesh, so representing it is honest by
  construction — no "you reached this page, therefore you're enrolled" sleight of hand.
- **Self-contained demo.** One `docker compose up` on any engineer's machine (or a
  conference laptop) brings up Ory, SPIRE, a throwaway CA, a real `zenohd` with a real
  ACL, the orchestrator, and the web app. No credentials, no network, no mesh enrollment.

## The honesty matrix — live vs represented (load-bearing)

Every claim the UI makes is graded, mirroring the project's honesty discipline (fabric
health colors, the explainer docs' built-vs-designed maps). The UI labels each layer:

| Layer | What runs in the sandbox | Genuinely live? |
|---|---|---|
| **Ory — Hydra** (OIDC + role) | Hydra dev-mode, memory DSN; the orchestrator is the login/consent app over a demo-persona picker (`alice-admin`/`omar-operator`/`bea-observer`); `groups` lands in the ID token at consent | **Live** — real browser authorization-code flow, real JWT (JWKS-verified) you decode on screen; the `groups` claim gates issuance |
| **Ory — Kratos** (credential store) | the demo-persona picker stands in for the Kratos identity/credential store in v1 | **Represented** — the *authentication act* (Hydra OIDC) is live; the password/MFA credential store is shown as a picker, stated plainly (v1 implementation choice, not the design ceiling) |
| **x509 leaf cert** (org path) | the orchestrator owns a throwaway CA and mints a real leaf with the real URI-SAN grammar (`urn:goat:zagros:org=<anchor>,role=…`) | **Live** — real cert → real TLS handshake to `zenohd` |
| **SPIRE/SPIFFE** (workload path) | a real SPIRE server + agent issuing short-TTL SVIDs to the orchestrator-as-workload | **Live** — real attestation + a visible rotation countdown |
| **Zenoh ACL** | an isolated `zenohd` with a real client-cert-gated ACL keyed on CN / URI-SAN (mirrors the slot-pool ACL of [ADR 1025](https://github.com/usareur-af/goatcore/blob/main/docs/adr/1025-zenoh-dynamic-tenant-authz-slot-pool.md)) | **Live** — real PUT either lands (seen on a live subscriber) or is *silently* denied (F-177), made visible via expected-vs-actual ([ADR 1022](https://github.com/usareur-af/goatcore/blob/main/docs/adr/1022-zenoh-denial-observability.md)) |
| **Private mesh DNS** (CoreDNS) | a real CoreDNS serving a tiny mesh-style zone; services register a stable name (e.g. `sensor-gw.trust.local`) that the orchestrator resolves before connecting | **Live** — real resolution; and a real "burn & respawn" re-points the name to a fresh endpoint/cert while every gate re-verifies (see below) |
| **NetBird / WireGuard** | a toggle + real artifacts (a setup key, an ACL group); when "not enrolled" the orchestrator refuses the transport, modeling no L3 route | **Represented** — WG can't run in a browser; this is the one layer shown, not literally re-exercised. The UI says so plainly. |

## The teaching model — five complementary checks (NOT a series)

Each check answers a question none of the others can. But they are **not a linear
pipeline** where each depends on the one before, and **not all of them guard every
service** — a common misread the UI used to imply. Two structural facts:

- **Each identity domain fits a kind of principal**, and so guards a kind of
  service: **x509 certs → machines** (machine-level services), **SPIRE/SPIFFE →
  processes/workloads**, **Ory → humans** (human-facing/UX services). A service
  is guarded by the domain its consumers use — you don't stack all of them on
  every service.
- **The checks relate to identity differently.** ① is an *entry point* whose
  admission comes from onboarding (a setup key → a mesh peer), independent of the
  data-plane identity. ② and ③ are **two issuance paths for the same
  machine/process identity question**, chosen by principal type (parallel, not
  sequential). ④ **authenticates humans + their role** (human-facing services); a
  role can *authorize* a human to trigger an admin action (e.g. a mint), but
  **Ory does NOT issue** anything — the CA (machines) and SPIRE (workloads) do.
  It is *not* in every per-write path. ⑤ is the per-action decision, **enforced by
  matching the cryptographic identity in ②/③** — so it traces back to them; it
  doesn't stand alone.

A real operation therefore lights a **subset**, not all five: a machine/org write
= ①②⑤; a workload write = ①③⑤; a human minting an identity = ①④ (+ the CA path).

| Check | Layer · principal | The question it answers | Relationship |
|---|---|---|---|
| ① On the mesh? | NetBird · network admission | Are you an enrolled mesh peer at all? | entry point; from onboarding, independent of the identity below |
| ② Machine identity | x509 leaf (URI-SAN grammar) · machines | What signed identity do you present? | one of two issuance paths (orgs/machines) |
| ③ Workload identity | SPIRE / SPIFFE · processes | …or are you an attested, auto-rotating workload? | the *other* issuance path (parallel to ②, not after it) |
| ④ Human + role | Ory (Kratos + Hydra) · humans | Which authenticated human is this, and what role? | authenticates humans + role; a role may *authorize* an admin action (e.g. a mint) — Ory does **not** issue, the CA/SPIRE do; not a per-write check |
| ⑤ The decision | Zenoh ACL | Does that identity permit *this* write? | per-action, default-deny, **keyed to the identity in ②/③** |

### The explainer layers — playground → concept → mechanism

The UI unpacks each idea at three depths so a congested diagram becomes a deeper page
rather than a wall of text:

1. **Playground** (`index.html`) — the interactive surface: the five-aspect strip + the
   live `LiveViz` gating animation (gate words spelled out — Network/Machine/Workload/
   Human/Decision — and colored green=pass / red=fail / gray=not-evaluated per the real
   verdict), the attempt/knockout panel (with the live viz mirrored into an overlay at the
   trigger so the verdict is visible without scrolling), the mint/rotate/revoke panel, and
   private-DNS burn-&-respawn.
2. **Concept pages** (`concept.html?gate=…`) — one aspect up close: anatomy diagram +
   salient points + what it composes with + its limits, with a "Go deeper" link.
3. **Mechanism drill-downs** (`deep.html?topic=…`) — the third layer, given its own room,
   with the *actual algorithms* and a standard-vs-ours panel: `jwt` (RS256, OIDC/OAuth2),
   `netbird` (Curve25519/WireGuard), `cert` (ECDSA P-256/SHA-256, mTLS TLS 1.3), `svid`
   (X509-SVID + where SPIRE's CA authority comes from — its own independent root, both
   roots trusted), `acl` (the zenohd router enforces, against the verified CN × put ×
   key-expression, default-deny + silent), and the lifecycle pair `rotate` (re-key same
   identity; no CRL so compose with a mesh-first cut; SPIRE auto-rotates) + `revoke`
   (mesh-first because there is no CRL/OCSP; enumerate-and-cut; short TTLs substitute for a
   revocation list).

Each explainer carries a **"Where this is today"** honesty annotation (LIVE / PARTLY BUILT
/ DESIGNED), mirroring the live-vs-represented matrix. In particular the SPIRE/lifecycle
explainers state plainly that ZAGROS today runs **one internal SPIRE trust domain** plus the
mesh-first cut, while a **federated partner pod** — one that runs its *own* SPIRE and
issues/rotates/revokes its own workloads locally (only public trust bundles cross the core,
so each side's SPIRE is a SPOF for its own domain only) — is **designed-not-built** and
lands with the moon-pod SPIRE overlay ([`goat-moon-pod.md`](goat-moon-pod.md),
`USAREUR-AF/zagros-moon-pod`).

### Two issuance paths, one decision point — the accurate picture

A subtlety the sandbox teaches honestly (it is true of the real fabric): there are **two
ways to get the cryptographic identity** that gate ⑤ judges, and which one you use depends
on *what kind of principal you are* —

- **External org → portal/offline-CA mint.** External orgs aren't SPIRE-attestable, so
  they get a portal-minted leaf carrying the URI-SAN grammar. In the sandbox the
  orchestrator-as-CA plays this role.
- **A workload we run the agent for → SPIRE attestation.** Infra and pod-resident
  workloads get a short-TTL, auto-rotated SVID. In the sandbox the SPIRE server+agent
  plays this role.

Both paths terminate at the **same** `zenohd` ACL. Showing both — same destination, two
issuers, chosen by principal type — is the clearest way to explain why SPIRE and the
portal mint coexist rather than compete (see `identity-architecture.md`
§3.4 and the CLAUDE.md identity pointer).

### The synergy story, end to end

A **human** authenticates via Ory and, *because of their `ADMIN` role*, is permitted to
mint a **cert/slot** for an **org system** (this is the `goat admin` chokepoint, modeled).
That system rides the **NetBird** mesh, presents its **cert** (or, if it's a workload we
run, an SVID that **SPIRE** keeps rotating), and the **Zenoh ACL** makes the final
per-write decision under the system's anchor. Every layer is doing a job none of the
others can do.

## The addressing layer — private DNS carries no trust (and that's the point)

The five gates above are the *verification* chain. Private mesh DNS is a **distinct,
complementary** layer: it is how a consumer *finds* a service, and in a zero-trust system
it is the thing you deliberately **do not** trust. Resolving `sensor-gw.trust.local` gives
you an address — and *nothing else*. The endpoint behind that name still has to pass every
gate; the friendly name launders no trust.

This is exactly what makes the goat net mesh DNS model
(`dns-service-discovery.md`) a security and adaptation win, and
the sandbox makes it visible with a **burn & respawn** interaction:

1. A backing service is registered under a stable name and is reachable + authorized
   (green end-to-end). A consumer connects *by name*, re-verifying identity on the way.
2. Hit **Burn it down** — the backing container is killed and respawned with a **fresh
   cert/SVID** and possibly a **new address**. CoreDNS re-points the name; nothing else
   about the consumer changes.
3. The consumer reconnects *to the same name*, the orchestrator re-resolves, and — because
   the new endpoint still presents a valid, authorized identity — it **just works**. The
   name stayed constant; trust was re-earned cryptographically.
4. Now respawn it with a **bad/unauthorized cert** (wrong anchor, or an `OBSERVER`-role
   leaf). The name resolves identically, but the connection is **denied** — proving the
   name never conferred trust, and a stale/cached name cannot be used to impersonate the
   service.

The lesson: because identity is verified per-connection and never inherited from a name or
address, services become **phoenix-like** — you can tear one down and stand a new one up in
seconds (faster patching, smaller blast radius, no long-lived endpoint to compromise) and
the fabric adapts safely without any trust ever resting on "it's at the usual address."
This is the rapid-adaptation property of [`phoenix-architecture.md`](https://github.com/usareur-af/goatcore/blob/main/docs/design/phoenix-architecture.md),
made tangible.

## The payoff interaction — the knockout experiment

A persistent **Zero-Trust Integrity** panel shows the five gate LEDs. The visitor:

1. Picks or composes a **persona** (e.g. *alice-admin*, *omar-operator*, an *org system*,
   a *workload*, or *mallory* — unenrolled / expired / wrong-anchor).
2. Hits **Attempt** — the orchestrator runs a *real* end-to-end attempt and returns a
   per-gate verdict trace plus the observed outcome (did the PUT land on the live
   subscriber, or was it silently denied?).
3. **Knocks a layer out** — flip *not enrolled* / *expired cert* / *expired SVID* /
   *no JWT* / *write outside your anchor* — and watches that gate go red and the
   end-to-end write fail (often *silently*, which is itself the lesson). The panel names
   the exact trust property just lost.
4. Restores it and watches green return.

This turns "you need all five" from an assertion into something the visitor *did*.

## Active + passive (mirroring the Zenoh sandbox)

- **Active (you drive).** Compose a persona, toggle layers, attempt operations, and
  *attack* — present an expired SVID, write outside your anchor, act as an `OBSERVER`
  and try to publish. Each independent check catches you. (The analog of "open two tabs
  and watch them talk.")
- **Passive (you watch).** On the playground: a `requestAnimationFrame` canvas (the
  live `LiveViz`) fed entirely by **real orchestrator events** (observer samples + gate
  verdicts) — a packet lights only the gates that *apply* to the chosen identity (the
  subset, not a forced five-in-a-row) and stops where a real check refused it; amber
  blips are real writes the mTLS observer saw. On the vision page: the designed metaphor
  story **"The wall, and the world with no inside"** (`TrustViz`) — a perimeter castle
  (one wall, then open hallways = lateral freedom) vs. a world of sealed rooms with **no
  hallways** (single-use, per-link verified corridors; no lateral movement). The opening
  beat *retracts the hallways* — you watch zero trust delete the thing that let you roam
  — then it cycles the failure spectrum (stale / flooding / forged), the left flooding
  while the right dead-ends. This is the designed story (the Zenoh-community-viz sibling),
  clearly distinct from the live `LiveViz`.

  *Metaphor choices are research-backed.* A 2026-06-17 sweep of how the field teaches ZTA
  found the strongest before/after pairing is **castle-and-moat → submarine watertight
  compartments** (Illumio/Akamai) with a **single toggle running the same breach against
  both** — exactly this viz. It also names the traps we deliberately avoid: the **"smuggled
  boundary"** (castle / airport / bouncer metaphors that secretly re-teach "verify once,
  trust inside" — our right side has *no inside*), **"always-verify theater"** (claiming
  continuous verification while issuing long-lived bearer tokens — we show real per-link
  mTLS + TTL rotation), and the SANS-refuted **"identity is the new perimeter"** category
  error (we frame identity as authenticated per-request, never as the boundary). Borrowed
  one-liners: SPIFFE = "a passport for software that expires hourly, no human touches it";
  mTLS = "both sides show photo ID"; a cert/token = "a scoped, time-limited, revocable
  keycard." Roadmap interactions the sweep surfaced: flood-fill blast radius from the
  breach point, a visible TTL countdown killing a stolen credential ("window of abuse"),
  and a PDP/PEP control-plane query drawn on the live gate viz.

## Architecture

One isolated Docker Compose project (`trust-sandbox`) on its own bridge network. No
host-published ports except the web app; nothing peers with any real fabric.

> **Deliberate v1 refinements (more honest than the original sketch).** Two
> implementation choices diverge from the first sketch above and are *more* honest:
> (1) the **live fabric viz is fed by the orchestrator's real mTLS observer over
> `/ws`**, not a browser→zenohd `remote_api` connection — a browser can't do mTLS,
> and a non-mTLS path would just hit the default-deny ACL, so feeding the viz from
> the real observer is the only non-misleading option (it shows only what an
> ACL-gated subscriber actually saw). zenohd therefore runs the **stock** pinned
> image with no `remote_api` plugin. (2) **Ory is split** (see the honesty matrix):
> Hydra (JWT + role) is live; the Kratos credential store is represented by the
> demo-persona picker in v1.

```
browser (localhost:8100)
   │  http  ┌────────────────────────────────────────────────────────────┐
   ├────────┤  GET /            → trust-sandbox-web (nginx, static SPA)     │
   │        │  /api/**, /ws     → trust-sandbox-orchestrator (Rust/axum)    │  ← /ws viz fed by the real observer
   │        │  /hydra/**        → Ory Hydra (OIDC discovery / JWKS / token)  │
   │        └────────────────────────────────────────────────────────────┘
   ▼
 orchestrator (the privileged broker — does what a browser can't):
   • owns a throwaway CA → mints org leaf certs (URI-SAN grammar)
   • is a SPIRE workload → fetches/rotates real SVIDs
   • resolves service names via CoreDNS, then opens REAL native-zenoh mTLS
     sessions with a chosen cert and attempts PUTs (re-verifying per connect)
   • calls Ory admin/JWKS → gates issuance authority on the operator's role claim
   • returns a per-gate verdict trace + expected-vs-actual outcome
        │ resolve (DNS)   │ mTLS (real client cert / SVID)   │ OIDC / admin API
        ▼                 ▼                                  ▼
 trust-sandbox-coredns   trust-sandbox-zenohd        trust-sandbox-kratos + -hydra
  (mesh-style zone;       (eclipse/zenoh:1.9.0         (Ory dev-mode, in-memory DSN,
   stable names re-       + remote_api + real           pre-seeded demo identities
   pointed on respawn)     client-cert ACL, no store)   + groups)
                                 ▲
                                 │ trust bundle = CA root ⊕ SPIRE bundle
                       trust-sandbox-spire-server + -agent  (short-TTL SVIDs, rotation)
```

### Containers

1. **`trust-sandbox-web`** — nginx serving the Vite+Vue3 SPA (source under
   `ops/serve/trust-sandbox/app/`). Talks to the orchestrator over same-host `/api`,
   and to `zenohd` over `/ws` (`@eclipse-zenoh/zenoh-ts@1.9.0`, the proven Zenoh-sandbox
   pattern) to render the live fabric viz — so the visitor *sees* a permitted write land
   on a real subscriber and a denied one never arrive.
2. **`trust-sandbox-orchestrator`** — **Rust** (axum + the repo's native Zenoh client,
   reusing panoscope-core/goat-cli patterns). Rust because exercising gate ⑤ requires a
   real native mTLS Zenoh session holding a real client cert — which the browser cannot
   do. The orchestrator is the analog of the remote_api plugin: it is what makes the demo
   real. It owns the throwaway CA, is registered as a SPIRE workload, and brokers every
   privileged move on behalf of the browser session.
3. **`trust-sandbox-zenohd`** — stock `eclipse/zenoh:1.9.0@sha256:…` (the fleet-pinned
   image; passes `check-zenoh-pinning.sh` with no allowlist) + the remote_api plugin
   (bind-mounted, SHA-verified, exactly the Zenoh-sandbox mechanism) + a **real**
   TLS-client-cert-gated ACL keyed on CN / URI-SAN. No storage backend (statelessness =
   trivial wipe).
4. **`trust-sandbox-kratos` + `trust-sandbox-hydra`** — Ory dev-mode, in-memory/SQLite
   DSN (maximally disposable), pre-seeded demo identities with `groups` claims
   (`ADMIN` / `OPERATOR` / `OBSERVER`). Groups-claim forwarding via the same approach as
   the patched consent UI ([ADR 0857](https://github.com/usareur-af/goatcore/blob/main/docs/adr/0857-patched-selfservice-ui-for-groups-claim.md)).
5. **`trust-sandbox-spire-server` + `trust-sandbox-spire-agent`** — SPIRE issuing
   short-TTL SVIDs to the orchestrator workload; a visible rotation countdown is a
   teaching surface.
6. **`trust-sandbox-coredns`** — a real CoreDNS serving a tiny mesh-style zone, the local
   analog of the mesh-internal service-discovery tier (`dns-service-discovery.md`).
   Stable service names resolve to the backing endpoints; on **burn & respawn** the zone
   is re-pointed to the fresh endpoint (the orchestrator rewrites the record and reloads),
   so the name stays constant while identity changes underneath. Honors the F-039 gotcha
   (zone file mode `0644` so the non-root CoreDNS UID loads records).
7. **`trust-sandbox-target`** — a small disposable backing service registered under a mesh
   name, used as the burn-&-respawn subject; respawns with a fresh (or deliberately bad)
   cert/SVID to demonstrate that the name never launders trust.

### Orchestrator API (sketch)

- `POST /api/session` → new ephemeral playground session (no real state seeded).
- `POST /api/identity` `{persona | custom toggles}` → mint the org leaf (CA path) **or**
  fetch the SVID (SPIRE path) for the chosen principal; record the represented NetBird +
  Ory state. Returns the issued identity's decoded fields (URI-SAN grammar, SVID ID, TTL).
- `POST /api/attempt` `{key, op}` → open a real mTLS Zenoh session with the session's
  current identity + layer toggles, attempt the PUT/GET, and return:
  `{ gates: {netbird, cert, spire, ory, acl}, expected, observed, silently_denied }`.
- Layer toggles compose a *real* attempt: `enrolled` (represented), `cert`
  (valid/expired/wrong-anchor/absent), `svid` (fresh/expired), `jwt`
  (admin/operator/observer/none).

## Ephemerality + isolation

- In-memory Ory DSN; SPIRE data dir on tmpfs; `zenohd` no storage; orchestrator CA +
  session state in-memory. `docker compose down -v` is a full wipe; recreate resets.
- **Demo identities only** — `alice-admin`, `omar-operator`, `bea-observer`, etc. Never a
  real roster, never a real anchor that exists on a live goatnet.
- **No `connect:` endpoints anywhere** — `zenohd` federates with nothing. If a future
  change adds one, it stops being a sandbox (same load-bearing invariant as
  `zenoh-sandbox.md`).

## Stack + build

Mirrors the Zenoh sandbox: Vite + Vue 3 + the shared warm/tacticool design tokens
(dark+light), static build → nginx. The live fabric viz is a canvas fed by the
orchestrator's `/ws` event stream (real observer samples + gate verdicts) — not
`zenoh-ts`, see the architecture refinement note. The orchestrator is a small
Rust/axum service built to a SHA-pinned image. All four images the hosted
instance runs are built by CI (`.github/workflows/trust-sandbox-image.yml` →
`ghcr.io/usareur-af/{trust-sandbox-orchestrator,trust-sandbox-web,trust-sandbox-spire,caddy-cloudflare}`,
`:latest` + `:sha-<short>` multi-arch). Primary delivery is the in-repo
`ops/serve/trust-sandbox/docker-compose.yml` (`docker compose up`), which *builds*
the images locally and tags them `:dev` — `:dev` is the local-build tag, never a
deploy pin. The **hosted** overlay pins each image to an immutable
`:sha-<short>@sha256:<index-digest>` (the fleet convention — git-tracked, PR-bumped,
multi-arch index digest per the goatnet-turnkey "multi-arch-complete pins" gotcha),
NOT a floating tag.

**SPIRE durability (built):** the server + agent use **disk** KeyManagers (keys
persist across a restart) and the node SVID TTL is long (1h) while the workload
SVID rotates fast (60s, per-entry) — so a container recreate doesn't trigger
re-attestation with the single-use join token or a fresh CA that breaks
the trust bundle. `docker compose down -v` is the documented full reset
(rolls a fresh CA + fresh SPIRE identities). The CA mint is idempotent so
re-running the `certs-init` one-shot on a partial `up` never churns the CA out
from under a running zenohd.

## Delivery + multi-user (how a visitor test-drives it)

The browser is the UI; the real components run server-side, because the browser
sandbox forbids exactly what makes the demo real (raw TCP/UDP sockets,
transport-layer mTLS to an arbitrary endpoint). That rules out a
**WASM-in-the-browser** build of the stack: zenohd's TLS listener, the
orchestrator's mTLS client dial, SPIRE's gRPC, CoreDNS's UDP, all need real
sockets, so compiling them to in-page WASM would force every gate to be
*simulated* — the same wall that makes NetBird "represented," applied to all
five. ("Containers in the browser" tech like container2wasm/WebVM only tunnels
networking out to an external gateway via WebSocket — neither local nor genuine
peer mTLS, and a heavy emulated VM.) So the real question is only *where the real
components run*:

- **Local (v1, shipped): `docker compose up`.** Each user gets a truly isolated,
  truly real, truly ephemeral stack on their own machine; `down -v` wipes it.
  Zero hosting cost, maximum honesty. Cost: needs Docker + a clone.
- **Hosted per-session (v2 follow-on, "click a link"):** a thin session-broker
  hands each visitor an isolated, idle-reaped environment, server-side-real,
  browser-UI — the same shape the Zenoh sandbox is already hosted in. Two cost
  tiers: (a) **shared backend + session-scoped orchestrator state** (make the
  orchestrator's CA/roster/DNS view per-session — a contained change since it
  already brokers everything — so many visitors share one zenohd/SPIRE/Hydra
  cheaply); (b) **full per-session stacks** (a namespaced compose project / pod
  per visitor) for stronger isolation, heavier. Start with (a), escalate only if
  isolation demands. Needs resource caps + idle teardown; tracked separately.

### Footprint + where to host it (v2)

The stack is light: steady-state ≈ **300–450 MB RAM, ~0 idle CPU** (zenohd
~30–80 MB · orchestrator ~20–50 MB · Hydra ~40–80 MB, the heaviest · SPIRE
server+agent ~60–100 MB · CoreDNS ~15–30 MB · nginx web+target ~20 MB). CPU only
blips on a TLS handshake (an attempt), a JWT signing (a login), or an SVID
rotation — all short. Session-scoping (tier *a*) adds **zero containers** per
user — just a few KB of orchestrator state per session — so the shared
zenohd/SPIRE/Hydra carry tens of intermittent learners on the same box; the only
real ceiling is simultaneous bursts of handshakes/JWT-signs, which a 2-vCPU box
absorbs for teaching-demo concurrency.

The deploy automation lives at
[`ops/serve/trust-sandbox/deploy/`](../../ops/serve/trust-sandbox/deploy/) and is
**deployed live on ZAGROS (2026-06-18)**: one mesh-only, Ory-gated instance at
`https://trust-sandbox.zagros.netbird.zagros.example/` on a small mesh-joined
cloud VM (Graviton t4g) behind a self-hosted Caddy TLS
edge (LE DNS-01 over the ZAGROS DNS zone) + an oauth2-proxy front gate →
ZAGROS Ory (any authenticated mesh peer, not admins-only), registered on the ZAGROS
isitup page (Browsing / Public). Images: the four CI-built packages
`ghcr.io/usareur-af/{trust-sandbox-web,trust-sandbox-orchestrator,trust-sandbox-spire,caddy-cloudflare}`,
pinned in the hosted overlay by `:sha-<short>@sha256:<index-digest>` (caddy-cloudflare lacks the
DNS-provider module in the stock image, so it's a built package too). ZAGROS was
touched **additively only** — DNS (`dns_extra_hosts`), an
isitup row, and one Ory client; no Thor service edits. The Thor-local path stays
wired-but-not-built (and may be wiped), per the runbook.

That Ory client is first-party and **skips the consent page**, like every other
door on the goatnet. It is the one client made by hand (the gate runs on the VM, so
no role writes it). The
runbook's `hydra create client` passes `--skip-consent`, and
`ops/ansible/trust-sandbox-oidc-client.yml` sets it on an existing client — found
by its callback URL, one field by JSON Patch, so the secret held on the VM is
untouched.

The hosted bring-up surfaced four issues, all fixed durably + generalized (local
dev unchanged): the ephemeral playground's **teaching Ory/Hydra was localhost-pinned**
(issuer/login/consent + SPA `redirect_uri`) → now parameterized by a single
`TS_PUBLIC_ORIGIN` (default `http://localhost:8100`); the compose **network had a
fixed `name:`** so two instances on one host shared it and cross-wired their flows
→ now project-scoped (the shared edge bridges per-instance networks); **oauth2-proxy
keys the session on `sub`** (ZAGROS/Kratos tokens carry no email claim); and the
**group gate is configurable** (`TS_GATE_ALLOWED_GROUPS`, default empty = any
authenticated mesh identity). A second "scratch" instance was stood up then
retired — one shared sandbox is the shipped shape; the deploy assets remain
multi-instance-capable for future envs.

**Host it in the cloud, not on a goatnet Thor.** The load-bearing isolation
invariant (peers with nothing) means the sandbox has *no reason* to be adjacent
to the real fabric — running it on the Thor would only contend for the Thor's
CPU/RAM for no benefit. The right shape is a **small dedicated cloud VM,
mesh-joined** (a NetBird peer) so it's reachable
mesh-internal at `trust-sandbox.<site>.<zone>` behind the usual Ory gate — no
public exposure, no Thor leech — exactly the pattern collab-services uses
(a mesh-joined cloud EC2). Images are multi-arch, so **Graviton/ARM (t4g.small,
2 vCPU/2 GB, ~$12/mo)** is the cheap fit with headroom; ephemeral/scheduled
(up for a demo, down after) drops cost near zero.

## Build phases (PR per phase, like the Zenoh sandbox)

1. **Scaffold + compose-up green.** The compose project boots; orchestrator
   `/api/session` answers; SPA shell with the design system renders; live fabric viz
   connects over `/ws`. (No teaching content yet.)
2. **The real attempt.** `/api/identity` + `/api/attempt` exercise the org-cert path
   end-to-end: mint a real leaf, open a real mTLS Zenoh session, observe allow vs silent
   deny on a live subscriber.
3. **SPIRE path + rotation.** Workload personas via real SVIDs; visible rotation;
   expired-SVID knockout.
4. **Ory live login + role-gated issuance.** Real browser OIDC; the `groups` claim gates
   whether the operator may mint (the `goat admin` model).
5. **Private DNS + burn & respawn.** CoreDNS resolves a stable service name; the
   orchestrator resolves-then-verifies; the burn-&-respawn interaction re-points the name
   to a fresh (or deliberately bad) endpoint and shows trust re-earned per connection.
6. **The knockout experiment + Zero-Trust Integrity panel.** The persistent gate-LED
   payoff interaction with the "property you just lost" explanations.
7. **Passive "day in the life" + the stakeholder vision page** (cousin of the Zenoh
   sandbox's community page). Honesty matrix surfaced in-UI.

## Non-goals

- Not the trial/real fabric, not on any live goatnet, not a place for real identities or
  real data.
- Not an IdP, not a CA, not a SPIRE deployment you operate — it is a *teaching* miniature
  that happens to run the real components.
- Not gamified. Wonder and clarity, not points (same discipline as the Zenoh sandbox).

## References

- `zenoh-sandbox.md` + [`zenoh-sandbox-ux.md`](zenoh-sandbox-ux.md) — the sibling whose shape + quality bar this matches
- `identity-architecture.md` — the four-domain model + identity grammar this dramatizes
- `identity-grammar.md` — role / affiliation / echelon / node-class / site (the URI-SAN grammar shown)
- `mesh-acl.md` — the NetBird group model the "on the network?" gate represents
- `dns-service-discovery.md` — the mesh-internal CoreDNS tier the private-DNS layer mirrors (incl. the F-039 zone-file-mode gotcha)
- [`phoenix-architecture.md`](https://github.com/usareur-af/goatcore/blob/main/docs/design/phoenix-architecture.md) — the burn-&-respawn / rapid-adaptation property the DNS layer makes tangible
- [ADR 1025](https://github.com/usareur-af/goatcore/blob/main/docs/adr/1025-zenoh-dynamic-tenant-authz-slot-pool.md) — the slot-pool ACL the sandbox `zenohd` mirrors
- F-177 + [ADR 1022](https://github.com/usareur-af/goatcore/blob/main/docs/adr/1022-zenoh-denial-observability.md) — silent denial, made visible here
- [ADR 1049](https://github.com/usareur-af/goatcore/blob/main/docs/adr/1049-zero-trust-identity-sandbox.md) — the decision record
