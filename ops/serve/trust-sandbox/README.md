# trust-sandbox — a live, local zero-trust identity playground

A disposable, **fully local** playground that lets a visitor *feel* how the
identity-and-access layers of goat net compose into zero trust — and why each
one is load-bearing. The teaching frame is **complementary roles in one coherent
strategy**, not "good user vs. attacker": most of what goes wrong is ordinary
(a noisy producer, a stale credential, a service mid-replacement), and the same
machinery that contains a hostile identity is what keeps the ordinary cases
coherent — and what lets identity + access be created, rotated, and revoked
quickly and on local authority.

> Design: [`docs/design/trust-sandbox.md`](../../../docs/design/trust-sandbox.md) ·
> Decision: [`docs/adr/1049-zero-trust-identity-sandbox.md`](https://github.com/usareur-af/goatcore/blob/main/docs/adr/1049-zero-trust-identity-sandbox.md)

## Run it

```bash
docker compose -f ops/serve/trust-sandbox/docker-compose.yml up --build
# then open http://localhost:8100
#   /            the playground
#   /vision.html the bigger picture (plain-language narrative)
```

Full wipe (it holds nothing real, ever):

```bash
docker compose -f ops/serve/trust-sandbox/docker-compose.yml down -v
```

## The five gates (complementary checks)

| # | Question | Layer | Live? |
|---|---|---|---|
| ① | Can your packets even get here? | NetBird / WireGuard | **represented** (a browser can't do a WG handshake — shown, never faked) |
| ② | Who are you, provably? | x509 leaf (CA path, URI-SAN grammar) | **live** — real CA, real mTLS |
| ③ | Still valid right now? | SPIRE / SPIFFE | **live** — real attestation + rotation countdown |
| ④ | Who is the actor & role? | Ory / Hydra (OIDC) | **live** JWT + role; the Kratos credential store is **represented** by a demo-persona picker in v1 |
| ⑤ | May you do *this* thing? | Zenoh ACL | **live** — real allow / silent-deny on a real mTLS subscriber |

Plus **private mesh DNS** (CoreDNS) — the addressing layer that carries no trust
by design, which is what makes services disposable (burn & respawn).

## What you can do

- **Attempt** — pick an identity (org system / workload / backing service) + a
  *condition* (`fresh` / `not enrolled` / `stale credential` / `wrong lane` /
  `noisy` / `forged`) and run a real end-to-end write; watch the gates light with
  a real verdict and see whether the write landed or was *silently* denied.
- **Identity on the fly** — sign in via real OIDC; as an **ADMIN** mint / rotate /
  revoke identities (mint + rotate are instant CA operations; revoke is a
  mesh-first cut — there is no CRL, matching the real fleet).
- **Burn & respawn** — re-point a stable mesh name to a fresh (or deliberately
  bad) identity and watch trust be re-earned per connection, with no re-addressing.

## Architecture

One isolated Docker Compose project on its own bridge network; only the web port
is published; **no `connect:` anywhere** (zenohd peers with nothing).

```
browser :8100 ──nginx──┬─ /            → web (Vue3 SPA)
                       ├─ /api, /ws    → orchestrator (Rust/axum — the broker)
                       └─ /hydra       → Ory Hydra (OIDC)
orchestrator ── mTLS ── zenohd (real client-cert ACL, gate ⑤)
            ── fetch ── spire-agent ← spire-server (short-TTL SVIDs, gate ③)
            ── OIDC  ── hydra (JWT + groups, gate ④)
            ── DNS   ── coredns (resolve + re-point, the untrusted name)
```

The orchestrator is the privileged broker: it owns a throwaway CA, is a SPIRE
workload, validates Hydra JWTs, resolves + rewrites the private zone, and opens
**real** mTLS Zenoh sessions holding a chosen identity's cert — the thing a
browser cannot do. The live viz is fed entirely by the orchestrator's real mTLS
observer, so it only ever shows what an ACL-gated subscriber actually saw.

## Components

- `orchestrator/` — Rust/axum broker (`init-ca` one-shot + `serve`).
- `app/` — Vite + Vue 3 SPA + the OIDC login/consent/callback pages.
- `zenohd/config.json5` — stock fleet-pinned zenohd + a real client-cert ACL.
- `spire/` — alpine + SPIRE binaries; server entrypoint mints the join token,
  registers the orchestrator workload (CN-bearing SVID via `-dns`), and builds
  the `sandbox-CA ⊕ SPIRE` client trust bundle.
- `ory/hydra.yml` — Hydra dev config. `coredns/` — the `trust.local` zone.
