// SPDX-License-Identifier: Apache-2.0
// concept.html?gate=<id> — one aspect of identity & access, up close. The five
// anatomy diagrams are static HTML (one shown per gate); the text comes from here.
import { initTheme, h } from './common.js'
import { initGlossPopup } from './gloss-popup.js'

// the five aspects of identity & access — each its own focused page.
// Entry shape: { id, n, title, domain, q, tagline, points[], composes, limits }
const ASPECTS = [
  {
    id: 'netbird', n: 'N', title: 'Network admission', domain: 'NetBird · machines · entry',
    q: 'Are you on the mesh at all?',
    tagline: 'An entry point, not a per-action check. Admission is established once at onboarding (a setup key → a WireGuard peer) — independent of the identity you present later.',
    points: [
      'Onboarding mints a WireGuard peer from a one-time setup key. That, not your cert or role, is what lets your packets traverse the mesh.',
      'Enrollment generates a local profile on the device: a WireGuard keypair (the private key never leaves the machine) plus the assigned mesh IP and allowed peers, stored on disk (e.g. ~/.netbird) and used by the netbird client daemon to bring up the tunnel.',
      'Being on the network is not trust: reaching a door is not permission to open it.',
      'This is the one aspect a browser can’t truly exercise (no WireGuard handshake in a browser) — represented in the sandbox, and said to be.',
      'Revocation is mesh-first: cut the peer and the packets stop — no certificate revocation list needed.',
    ],
    composes: 'Independent of the others: a valid identity with no mesh admission can’t reach the fabric; an admitted peer with the wrong identity still fails at the access decision.',
    limits: 'A mesh peer means your packets can arrive — nothing more. It can’t say WHO you are at the application layer, carry a role, scope a write to a namespace, tell a human from a machine, or expire per action — every admitted peer looks alike. It’s an entrance, not an identity or a permission. (This is exactly why being on the network isn’t trust, and why NetBird IDs alone aren’t enough.)',
  },
  {
    id: 'cert', n: 'M', title: 'Machine identity — x509', domain: 'x509 cert · machines · org systems',
    q: 'What signed identity do you present?',
    tagline: 'A provable, signed identity for machines and external-org systems — like a scoped, time-limited, revocable keycard. The connection is mutual TLS: both sides show photo ID.',
    points: [
      'A trusted Certificate Authority (CA) signs a leaf carrying a Common Name (CN, the access anchor) and the identity grammar in its Subject Alternative Name (SAN): urn:goat:__goatnet__:org=…,role=….',
      'Mutual TLS (mTLS) means both the client and the server present a chain-verifiable certificate — neither side is trusted by address.',
      'Short-lived and re-issuable: rotation is a re-key (a fresh leaf for the same anchor), not a revoke; the cut is mesh-first.',
      'Identity is a signed thing you present per connection — not a username, an IP, or a hostname.',
    ],
    composes: 'One of two issuance paths for machine/process identity (the other is SPIRE). Both end at the same access decision.',
    limits: 'A certificate proves WHO a machine is — but it doesn’t authenticate a human (no login, no role), it doesn’t decide WHAT you may do (it isn’t an access policy), and it can’t get your packets onto the network. It’s an identity, not a permission and not a human session.',
  },
  {
    id: 'spire', n: 'W', title: 'Workload identity — SPIRE/SPIFFE', domain: 'SPIRE · processes · workloads',
    q: '…or an attested, auto-rotating workload?',
    tagline: 'SPIFFE is a standard for workload identity, and SPIRE implements it. It answers the same identity question for processes and workloads we run — think a passport for software that expires every hour and re-issues with no human ever touching it.',
    points: [
      'Two-step attestation: the SPIRE server attests the node (agent), then the agent attests the workload (e.g. by uid). Identity is earned, not configured.',
      'The SVID — a SPIFFE Verifiable Identity Document — is short-lived and auto-rotates continuously; a leaked one is useless within minutes (the “window of abuse” is tiny).',
      'No human in the loop: no operator copies a secret; the agent fetches and rotates it.',
      'Parallel to the x509 path, not after it — which one applies is chosen by what kind of principal you are.',
    ],
    composes: 'Issues the cryptographic identity the access decision judges, for workloads. Same destination as x509, a different issuer.',
    limits: 'It only covers workloads we run the agent for — external organizations aren’t attestable, so they get a CA-minted certificate instead. It doesn’t authenticate humans, and it doesn’t decide WHAT a workload may write. It issues identity, not authorization.',
  },
  {
    id: 'ory', n: 'H', title: 'Human identity + role — Ory', domain: 'Ory · humans · authN + role',
    q: 'Which human are you, and what role?',
    tagline: 'Single sign-on for humans to the mesh’s web tools (an OAuth2 / OIDC login that returns a signed JWT carrying who you are + your role). The role decides what you can do — view, operate, or administer. Ory does NOT issue machine/workload identities — the CA and SPIRE do; an admin role can additionally authorize an action like a mint (one scenario).',
    points: [
      'A real OpenID Connect (OIDC) login (a Kratos credential, a Hydra token) yields a signed JSON Web Token (JWT) carrying who you are (sub) and your role (groups).',
      'The role is the delegation mechanism: an ADMIN human is authorized to trigger a mint — the CA then does the issuing.',
      'It is not in every per-write path: a machine or workload data-write involves no human and no Ory.',
      'Authenticate the actor, not the location — and never treat identity itself as the perimeter; it is checked per request, it is not a wall.',
    ],
    composes: 'Upstream in time and authority: a human’s role authorizes who may create the machine/workload identities the access decision later judges.',
    limits: 'It identifies a human and their role — but a machine or process has no human, so Ory can’t speak for it; it doesn’t issue the cryptographic identity a service presents; and it doesn’t itself enforce a per-action decision. It authenticates the actor and carries authority — it doesn’t move data or gate it.',
  },
  {
    id: 'acl', n: 'D', title: 'The access decision — Zenoh ACL', domain: 'Zenoh ACL · per write',
    q: 'Does that identity permit THIS write?',
    tagline: 'The per-action authorization decision, default-DENY, enforced by matching the cryptographic identity above (the cert/SVID Common Name) to a granted write-anchor.',
    points: [
      'Default-deny: nothing is permitted unless a rule grants it — the opposite of “trusted once you’re inside”.',
      'Keyed to the identity: the decision matches the verified cert/SVID Common Name to its anchor namespace; it does not stand alone.',
      'A denied write is silent — the publisher gets no error. Per-resource, per-request (the trap the playground makes visible).',
      'This is the enforcement point on the data path; the policy decision can be re-evaluated on every request.',
    ],
    composes: 'The terminus: admission, identity (x509 or SPIRE), and the human who authorized issuance all converge here, per write.',
    limits: 'The decision is only as good as the identity feeding it — it can’t establish WHO you are, can’t get your packets onto the network, and can’t authenticate a human. With no verified identity above it, it has nothing to decide on. It’s the verdict, not the identity.',
  },
]

const id = new URLSearchParams(location.search).get('gate') || 'netbird'
const idx = Math.max(0, ASPECTS.findIndex((x) => x.id === id))
const a = ASPECTS[idx]
const prev = ASPECTS[(idx + ASPECTS.length - 1) % ASPECTS.length]
const next = ASPECTS[(idx + 1) % ASPECTS.length]

// Each aspect has a third-layer drill-down (anatomy + algorithms) in deep.html.
const DEEP_TOPIC = { netbird: 'netbird', cert: 'cert', spire: 'svid', ory: 'jwt', acl: 'acl' }
const deepTopic = DEEP_TOPIC[a.id]

const $ = (sel) => document.getElementById(sel)
initTheme()
const sym = $('c-sym'); sym.classList.add('sym-' + a.n); sym.textContent = a.n
$('c-title').textContent = a.title
$('c-n').textContent = a.n
$('c-domain').textContent = a.domain
$('c-title2').textContent = a.title
$('c-q').textContent = `“${a.q}”`
$('c-tagline').textContent = a.tagline
for (const el of document.querySelectorAll('.anat [data-gate]')) el.hidden = el.dataset.gate !== a.id
if (deepTopic) { const d = $('c-deeper'); d.href = '/deep.html?topic=' + deepTopic; d.hidden = false }
$('c-points').append(...a.points.map((pt) => h('li', null, h('span', { class: 'bullet' }), h('span', null, pt))))
$('c-composes').textContent = a.composes
$('c-limits').textContent = a.limits
const p = $('c-prev'); p.href = '/concept.html?gate=' + prev.id; p.textContent = `◀ ${prev.n} ${prev.title}`
const n = $('c-next'); n.href = '/concept.html?gate=' + next.id; n.textContent = `${next.n} ${next.title} ▶`
initGlossPopup()
