<script setup lang="ts">
import { ref } from 'vue'
import TrustViz from './components/TrustViz.vue'
import BlastRadius from './components/BlastRadius.vue'
import EncryptionViz from './components/EncryptionViz.vue'
import GlossaryPopup from './components/GlossaryPopup.vue'

const theme = ref(document.documentElement.getAttribute('data-theme') || 'dark')
function toggleTheme() {
  theme.value = theme.value === 'dark' ? 'light' : 'dark'
  document.documentElement.setAttribute('data-theme', theme.value)
  localStorage.setItem('tsbx-theme', theme.value)
}

const spectrum = [
  ['A noisy or overloaded service', 'flooding, not from malice — a bug or a bad day. It must not be able to drown out everyone else.'],
  ['A broken service', 'that has to be swapped out right now — without making every consumer learn a new address.'],
  ['A stale credential', 'expired, not revoked, just old. It should stop working on its own, with no cleanup crew.'],
  ['A new partner', 'that needs to come online fast and later go offline cleanly — on local authority, not a slow queue.'],
  ['Occasionally, someone hostile', 'who gets contained by the very same machinery, as a special case of "no valid identity for this."'],
]

const aspects = [
  { id: 'netbird', n: 'N', title: 'Network admission', who: 'machines · entry', line: 'Are you on the mesh at all? — admission from onboarding, independent of the identity below.' },
  { id: 'cert', n: 'M', title: 'Machine identity (x509)', who: 'machines', line: 'What signed identity do you present? — a scoped, revocable keycard; both sides show ID (mTLS).' },
  { id: 'spire', n: 'W', title: 'Workload identity (SPIRE)', who: 'processes', line: 'An attested, auto-rotating passport for software — expires hourly, no human touches it.' },
  { id: 'ory', n: 'H', title: 'Human identity + role (Ory)', who: 'humans', line: 'Single sign-on to the mesh’s web tools; your role sets what you can do — view, operate, or administer.' },
  { id: 'acl', n: 'D', title: 'The access decision', who: 'per write', line: 'Does that identity permit this write? — default-deny, matched to the identity above.' },
]
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
        <div><h1>THE BIGGER PICTURE</h1><p class="sub">What zero trust really is, in plain language.</p></div>
      </div>
      <span class="spacer" />
      <a class="navlink" href="/">◀ TO THE PLAYGROUND</a>
      <a class="navlink" href="/glossary.html">📖 GLOSSARY</a>
      <button class="theme-toggle" @click="toggleTheme"><span v-if="theme==='dark'">◐ LIGHT</span><span v-else>◑ DARK</span></button>
    </header>

    <!-- 1. the thesis -->
    <section class="hero v-hero">
      <h2>There is no inside.<br /><span class="accent">Nothing is trusted by location — only by proof, every time.</span></h2>
      <p>"Zero trust" usually lands as a slogan, and most people picture it wrong: that there's a <b>wall</b>, and once
        you're past it you're in — and that the point is <b>catching attackers</b>. Both miss it. There is no inside;
        every action is re-checked by several complementary aspects of identity and access. And most of what they
        handle — a noisy service, a stale credential, a thing that needs replacing — was never an attack at all.</p>
    </section>

    <!-- 2. the two shapes of trust — the big illustrations, each its own room -->
    <section class="story">
      <div class="story-head"><span class="step">THE OLD WAY</span><h3>One wall, then free run</h3></div>
      <p class="story-lead">A castle with a moat: you're checked once at the gate, and after that you roam the inside
        freely. It feels safe — but anything that gets in (a stranger who tailgates, a service gone haywire, a
        credential no one re-checks) can move sideways to everything. The wall is the only line, and there's no second
        one.</p>
      <div class="panel viz-panel"><TrustViz world="castle" /></div>
    </section>

    <section class="story">
      <div class="story-head"><span class="step accent">ZERO TRUST</span><h3>There is no inside</h3></div>
      <p class="story-lead">Now there are no hallways at all — only sealed rooms, each reached by its own single-use
        link that re-proves identity and then vanishes. A breach floods one room and stops; there's nowhere to move
        sideways. Credentials are short-lived, so a stolen one dies on its own. Like a submarine's watertight
        compartments — one floods, the ship stays afloat.</p>
      <div class="panel viz-panel"><TrustViz world="zero" /></div>
    </section>

    <!-- 3. it manages every access risk, not just attackers -->
    <section class="story">
      <div class="story-head"><span class="step amber">THE REAL JOB</span><h3>It manages every access risk — not just attackers</h3></div>
      <p class="story-lead">Good identity and access management isn't a villain-detector. It mediates the whole range of
        things that go wrong around <b>access</b>: a service flooding the channel, a credential gone stale, something
        mid-replacement, an honest misconfiguration — and a bad actor is just one more case the same machinery handles.
        Get the everyday cases right and the hostile one is mostly already covered.</p>
      <p class="story-lead">And because every action is bound to an identity, when something does need cutting off there's
        more than one place to do it — at the layer that owns the actor. Pull its <b>mesh access</b> and it can't reach
        anything at all (immediate, and the lever we actually use); strip its <b>write permission</b> in the fabric's
        policy (a coarser, slower cut); and the <b>credential</b> itself isn't really "revoked" — it's built to
        <b>expire on its own</b> (fast for short-lived workload identities, slower for issued certs). Different layers,
        different speeds, one identity — defense in depth, not a single kill-switch.</p>
      <div class="panel"><table class="matrix">
        <tr v-for="(s,i) in spectrum" :key="i"><td class="lyr" style="white-space:nowrap">{{ s[0] }}</td><td class="d">{{ s[1] }}</td></tr>
      </table></div>
    </section>

    <!-- 3b. the human truth + blast radius -->
    <section class="story">
      <div class="story-head"><span class="step amber">THE HUMAN TRUTH</span><h3>You can't delete human error — so limit the blast</h3></div>
      <p class="story-lead">The overwhelming majority of real-world breaches — commonly put near <b>99%</b> — start with
        people: someone gets phished, or makes a plain configuration blunder (a weak password, a box left open to the
        internet). Because the root cause is human, it can never be fully eliminated — only reduced, watched for, and
        managed. So real security isn't a taller wall. It <b>reduces</b> the chances these happen, <b>watches</b> for
        them, <b>reacts</b> fast — and, above all, <b>limits the blast radius</b> when one slips through. Same incident,
        two worlds — watch where it stops:</p>
      <div class="viz-pair">
        <div class="panel viz-panel"><BlastRadius world="flat" /></div>
        <div class="panel viz-panel"><BlastRadius world="zero" /></div>
      </div>
    </section>

    <!-- 4. the five aspects -->
    <section class="story">
      <div class="story-head"><span class="step">THE PIECES</span><h3>Five aspects of identity &amp; access, used together</h3></div>
      <p class="story-lead">Each answers a question none of the others can — and each fits a kind of principal, so it
        guards a kind of service: <b>x509 for machines</b>, <b>SPIRE for processes</b>, <b>Ory for humans</b>. They're
        complementary and coordinated, <b>not a sequence you pass through</b>: a real action uses only the ones that
        apply. Click any to see its anatomy.</p>
      <div class="aspect-grid">
        <a v-for="a in aspects" :key="a.id" class="aspect-card" :href="'/concept.html?gate=' + a.id">
          <div class="ac-top"><span class="sym" :class="'sym-' + a.n" style="font-size:16px">{{ a.n }}</span><span class="ac-who">{{ a.who }}</span></div>
          <div class="ac-title">{{ a.title }}</div>
          <div class="ac-line">{{ a.line }}</div>
          <div class="ac-go">see its anatomy ↗</div>
        </a>
      </div>
    </section>

    <!-- 4b. encryption -->
    <section class="story">
      <div class="story-head"><span class="step">EVERY BYTE, TWICE</span><h3>Encrypted end-to-end — twice over</h3></div>
      <p class="story-lead">Identity decides <i>who</i> may act; encryption keeps the <i>traffic</i> private and
        untampered on the way. Every interaction on the fabric rides inside <b>two</b> independent encrypted tunnels,
        one wrapped inside the other — so reading the wire reveals nothing, and breaking one layer still leaves the
        other intact.</p>
      <div class="viz-pair">
        <div class="panel viz-panel"><h4 class="ph">One tunnel inside the other</h4><EncryptionViz mode="nested" /></div>
        <div class="panel viz-panel"><h4 class="ph">How a connection comes up</h4><EncryptionViz mode="sequence" /></div>
      </div>
      <div class="grid" style="margin-top:14px">
        <div class="panel"><h4 class="ph">① The outer tunnel — WireGuard</h4><p class="pp">The mesh itself (NetBird on
          WireGuard) encrypts everything between two peers at the network layer. Anything on the wire is already
          ciphertext.</p></div>
        <div class="panel"><h4 class="ph">② The inner tunnel — mTLS</h4><p class="pp">Inside that, the application opens
          its own mutual-TLS connection (here, in Zenoh). It encrypts <i>and</i> proves both identities at once. Other
          applications bring their own TLS — usually mutual too.</p></div>
        <div class="panel"><h4 class="ph">Swappable by design</h4><p class="pp">The two layers differ in origin and
          algorithm, and either can change without the other — including a move to <b>post-quantum</b> ciphers as they
          mature. No single algorithm is load-bearing.</p></div>
        <div class="panel"><h4 class="ph">And at rest</h4><p class="pp">Beyond the wire: disks are encrypted at rest
          (full-disk LUKS), keys are short-lived with <b>forward secrecy</b> (a stolen key can't decrypt past traffic),
          and the crypto can run in FIPS-validated mode where required.</p></div>
      </div>
    </section>

    <!-- 5. the freedom half -->
    <section class="story">
      <div class="story-head"><span class="step green">THE PAYOFF</span><h3>The freedom it buys</h3></div>
      <p class="story-lead">The same machinery that contains misbehavior is what makes identity and access fast and
        local to manage — where the old way was slow and centralized.</p>
      <div class="grid">
        <div class="panel"><h4 class="ph">Sovereign &amp; federated</h4><p class="pp">A group creates, changes, and
          revokes its own identities on its own authority — no funneling every change through one central gatekeeper.</p></div>
        <div class="panel"><h4 class="ph">Dynamic</h4><p class="pp">Granting, scoping, and rescinding access is a live
          change, not a rebuild: bring a participant online now, take them offline cleanly later, narrow what a service
          can touch the moment its role changes.</p></div>
        <div class="panel"><h4 class="ph">Self-healing</h4><p class="pp">Because identity is re-verified per connection
          and credentials expire on their own, much "cleanup" needs no human: stale things lapse, replaced things
          re-prove. And a flooding producer is tied to its identity — it shows up on the health surface and its
          credential can be revoked to cut it off (automatic rate-throttle is on the roadmap).</p></div>
        <div class="panel"><h4 class="ph">Disposable services</h4><p class="pp">A name only says <i>where</i>, never
          <i>whether to trust</i> — so you can tear a service down and stand a fresh one up under the same name in
          seconds, and everyone keeps working. (Try it in the playground.)</p></div>
      </div>
    </section>

    <!-- 6. honesty -->
    <section class="story">
      <div class="story-head"><span class="step">THE FINE PRINT</span><h3>What's actually running</h3></div>
      <p class="story-lead">The playground runs the <b>real</b> components — a real CA, real SPIRE, real OIDC, a real
        access decision — so what you see is the system actually working, not a mock. The one exception is network
        admission (NetBird): a browser can't perform a WireGuard handshake, so that layer is <b>represented</b> rather
        than live, and labeled as such.</p>
    </section>

    <footer class="foot">
      Ready to feel it? <a href="/">Open the playground</a> — pick who you are, change one thing, and watch what holds.
    </footer>
    <GlossaryPopup />
  </div>
</template>

<style scoped>
.v-hero { margin-top: 6px; }
.v-hero h2 { font-size: clamp(24px, 4.2vw, 40px); line-height: 1.12; }
.v-hero p { font-size: clamp(14px, 1.7vw, 16px); max-width: 80ch; }
/* each section is set off by a clear top divider + generous breathing room — NOT
   a nested box (avoids over-framing); the lead carries an accent spine. */
.story { margin: var(--space-break) 0 0; padding-top: var(--space-break); border-top: var(--seam) solid var(--line-soft); }
.story:first-of-type { border-top: none; padding-top: 8px; }
@media (max-width: 768px) { .story { margin-top: 20px; padding-top: 20px; } }
.story-head { display: flex; align-items: baseline; gap: 12px; margin-bottom: 10px; flex-wrap: wrap; }
.story-head h3 { font-size: clamp(18px, 2.6vw, 24px); margin: 0; font-weight: 700; }
.story-head .step { font-family: var(--font-mono); font-size: 12px; letter-spacing: .12em; text-transform: uppercase; color: var(--ink-dim); border: 1.5px solid var(--line-soft); border-radius: 999px; padding: 3px 11px; }
.story-head .step.accent { color: var(--accent); border-color: color-mix(in srgb, var(--accent) 45%, var(--line)); }
.story-head .step.amber { color: var(--amber); border-color: color-mix(in srgb, var(--amber) 45%, var(--line)); }
.story-head .step.green { color: var(--green); border-color: color-mix(in srgb, var(--green) 45%, var(--line)); }
.story-lead { color: var(--ink); font-size: 14.5px; line-height: 1.6; max-width: 82ch; margin: 0 0 16px;
  border-left: 2px solid color-mix(in srgb, var(--accent) 30%, transparent); padding-left: 12px; }
.story-lead b { color: var(--ink); }
.viz-panel { padding: 16px 16px 14px; }
.viz-pair { display: grid; grid-template-columns: repeat(auto-fit, minmax(360px, 1fr)); gap: 16px; }
.viz-pair .ph { margin: 0 0 8px; }
.ph { margin: 0 0 6px; font-size: 14px; }
.pp { margin: 0; color: var(--ink-dim); font-size: 13px; line-height: 1.55; }
.aspect-grid { display: grid; grid-template-columns: repeat(auto-fit, minmax(220px, 1fr)); gap: 18px; }
.aspect-card { display: flex; flex-direction: column; gap: 7px; text-decoration: none; color: inherit;
  background: linear-gradient(180deg, var(--panel), var(--panel-2)); border: var(--seam) solid var(--line); border-top: 3px solid var(--accent);
  border-radius: var(--radius); padding: 15px; box-shadow: var(--shadow); transition: border-color .2s var(--ease), transform .12s var(--ease); }
.aspect-card:hover { border-color: var(--accent); transform: translateY(-2px); }
.ac-top { display: flex; align-items: center; justify-content: space-between; }
.ac-n { font-size: 22px; font-weight: 700; color: var(--accent); }
.ac-who { font-family: var(--font-mono); font-size: 10px; color: var(--ink-faint); border: 1px solid var(--line); border-radius: 999px; padding: 2px 8px; }
.ac-title { font-size: 14px; font-weight: 600; }
.ac-line { font-size: 12.5px; color: var(--ink-dim); line-height: 1.45; flex: 1; }
.ac-go { font-family: var(--font-mono); font-size: 10.5px; color: var(--ink-faint); }
.aspect-card:hover .ac-go { color: var(--accent); }
</style>
