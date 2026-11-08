<script setup lang="ts">
import { ref, computed } from 'vue'
import ConceptViz from './components/ConceptViz.vue'
import GlossaryPopup from './components/GlossaryPopup.vue'

const theme = ref(document.documentElement.getAttribute('data-theme') || 'dark')
function toggleTheme() {
  theme.value = theme.value === 'dark' ? 'light' : 'dark'
  document.documentElement.setAttribute('data-theme', theme.value)
  localStorage.setItem('tsbx-theme', theme.value)
}

// the five aspects of identity & access — each its own focused page.
interface Aspect {
  id: string; n: string; title: string; domain: string; q: string; tagline: string
  points: string[]; composes: string; limits: string
}
const ASPECTS: Aspect[] = [
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
const idx = computed(() => Math.max(0, ASPECTS.findIndex((a) => a.id === id)))
const a = computed(() => ASPECTS[idx.value])
const prev = computed(() => ASPECTS[(idx.value + ASPECTS.length - 1) % ASPECTS.length])
const next = computed(() => ASPECTS[(idx.value + 1) % ASPECTS.length])

// Each aspect has a third-layer drill-down (anatomy + algorithms) in deep.html.
const DEEP_TOPIC: Record<string, string> = { netbird: 'netbird', cert: 'cert', spire: 'svid', ory: 'jwt', acl: 'acl' }
const deepTopic = computed(() => DEEP_TOPIC[a.value.id])
</script>

<template>
  <div class="wrap">
    <header class="topbar">
      <div class="brand">
        <div class="mark">
          <svg width="18" height="18" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round">
            <path d="M12 3l7 3v5c0 4.5-3 7.5-7 9-4-1.5-7-4.5-7-9V6z" /><path d="M9 12l2 2 4-4" />
          </svg>
        </div>
        <div><h1><span class="sym" :class="'sym-' + a.n" style="font-size:18px">{{ a.n }}</span> {{ a.title }}</h1><p class="sub">One layer of identity &amp; access, up close.</p></div>
      </div>
      <span class="spacer" />
      <a class="navlink" href="/">◀ PLAYGROUND</a>
      <a class="navlink" href="/glossary.html">📖 GLOSSARY</a>
      <a class="navlink" href="/vision.html">✦ BIGGER PICTURE</a>
      <button class="theme-toggle" @click="toggleTheme"><span v-if="theme==='dark'">◐ LIGHT</span><span v-else>◑ DARK</span></button>
    </header>

    <section class="chero">
      <div class="cn">{{ a.n }}</div>
      <div>
        <div class="tag">{{ a.domain }}</div>
        <h2>{{ a.title }}</h2>
        <p class="q">“{{ a.q }}”</p>
      </div>
    </section>

    <p class="tagline">{{ a.tagline }}</p>

    <div class="panel span-2">
      <div class="panel-head"><span class="dot accent" /><h3>Anatomy &amp; mechanics</h3>
        <span class="hint">how this aspect actually works</span></div>
      <ConceptViz :gate="a.id" />
      <a v-if="deepTopic" class="go-deeper" :href="'/deep.html?topic=' + deepTopic">
        Go deeper — the anatomy &amp; the actual algorithms <span class="gd-arrow">↗</span></a>
    </div>

    <div class="grid" style="margin-top:16px">
      <div class="panel">
        <div class="panel-head"><span class="dot amber" /><h3>The salient points</h3></div>
        <ul class="points">
          <li v-for="(pt, i) in a.points" :key="i"><span class="bullet" /><span>{{ pt }}</span></li>
        </ul>
      </div>
      <div class="panel">
        <div class="panel-head"><span class="dot green" /><h3>How it composes with the rest</h3></div>
        <p class="composes">{{ a.composes }}</p>
        <p class="composes-note">These five aspects are <b>complementary and coordinated</b>, not a sequence you pass
          through. A real action uses only the ones that apply — a machine write uses <span class="sym sym-N">N</span><span class="sym sym-M">M</span><span class="sym sym-D">D</span>, a workload <span class="sym sym-N">N</span><span class="sym sym-W">W</span><span class="sym sym-D">D</span>, a human signing in to act <span class="sym sym-N">N</span><span class="sym sym-H">H</span>.</p>
      </div>
    </div>

    <div class="panel span-2 limits-panel" style="margin-top:16px">
      <div class="panel-head"><span class="dot" style="background:var(--amber);box-shadow:0 0 10px var(--amber)" />
        <h3>Why this isn't enough on its own</h3>
        <span class="hint">the gap the other aspects fill</span></div>
      <p class="limits">{{ a.limits }}</p>
      <p class="limits-foot">That gap is the whole reason there are five complementary aspects rather than one. Keep
        only this one and a specific protection disappears — which is exactly what the
        <a href="/">playground</a> lets you prove by changing a single thing and watching what breaks.</p>
    </div>

    <nav class="aspect-nav">
      <a :href="'/concept.html?gate=' + prev.id" class="navlink">◀ {{ prev.n }} {{ prev.title }}</a>
      <a :href="'/concept.html?gate=' + next.id" class="navlink">{{ next.n }} {{ next.title }} ▶</a>
    </nav>

    <footer class="foot">
      A focused look at one aspect · <a href="/">back to the playground</a> to exercise it live.
    </footer>
    <GlossaryPopup />
  </div>
</template>

<style scoped>
.chero { display: flex; gap: 22px; align-items: center; margin: 6px 2px 14px; padding-bottom: 14px; border-bottom: var(--seam) solid var(--line-soft); }
.chero .cn { font-size: clamp(40px, 7vw, 72px); font-weight: 700; color: var(--accent); line-height: 1; opacity: .9; }
.chero h2 { font-size: clamp(20px, 3.4vw, 30px); margin: 6px 0 8px; }
.chero .q { margin: 0; color: var(--ink); font-size: 16px; font-weight: 500; }
.tagline { color: var(--ink); font-size: 15px; line-height: 1.6; max-width: 78ch; margin: 0 0 16px;
  border-left: 2px solid color-mix(in srgb, var(--accent) 30%, transparent); padding-left: 12px; }
.points { list-style: none; margin: 0; padding: 0; display: flex; flex-direction: column; gap: 12px; }
.points li { display: flex; gap: 11px; align-items: flex-start; font-size: 13.5px; color: var(--ink-dim); line-height: 1.5; }
.points .bullet { flex: none; margin-top: 6px; width: 7px; height: 7px; border-radius: 50%; background: var(--accent); box-shadow: 0 0 8px var(--accent); }
.composes { color: var(--ink-dim); font-size: 13.5px; line-height: 1.55; margin: 0 0 12px; }
.composes-note { color: var(--ink-faint); font-size: 12.5px; line-height: 1.5; margin: 0; }
/* the "why this alone isn't enough" panel is the capstone — warm caution accent */
.limits-panel { border-color: color-mix(in srgb, var(--amber) 28%, var(--line)); border-left: 4px solid var(--amber);
  background: linear-gradient(180deg, color-mix(in srgb, var(--amber) 8%, var(--panel)), var(--panel-2)); }
.limits { color: var(--ink); font-size: 14px; line-height: 1.6; margin: 0 0 10px; }
.limits-foot { color: var(--ink-dim); font-size: 12.5px; line-height: 1.5; margin: 0; }
.aspect-nav { display: flex; justify-content: space-between; gap: 12px; margin-top: 22px; padding-top: 16px;
  border-top: var(--seam) solid var(--line-soft); flex-wrap: wrap; }
.go-deeper { display: inline-flex; align-items: center; gap: 7px; margin-top: 14px; padding: 9px 14px;
  border: 1px solid color-mix(in srgb, var(--accent) 40%, var(--line)); border-radius: 9px;
  background: color-mix(in srgb, var(--accent) 9%, var(--panel-2)); color: var(--accent);
  font-size: 13px; font-weight: 600; text-decoration: none; transition: border-color .15s, background .15s; }
.go-deeper:hover { border-color: var(--accent); background: color-mix(in srgb, var(--accent) 16%, var(--panel-2)); }
.go-deeper .gd-arrow { font-size: 14px; }
</style>
