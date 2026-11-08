<script setup lang="ts">
import { ref, reactive, onMounted, onBeforeUnmount, computed, watch, nextTick } from 'vue'
import LiveViz from './components/LiveViz.vue'
import GlossaryPopup from './components/GlossaryPopup.vue'
import {
  api, connectWs, startLogin, getToken, logout, decodeJwt,
  type Identity, type GateVerdict, type AttemptResp, type Health, type Svid,
} from './api'

// ---- theme ----
const theme = ref(document.documentElement.getAttribute('data-theme') || 'dark')
function toggleTheme() {
  theme.value = theme.value === 'dark' ? 'light' : 'dark'
  document.documentElement.setAttribute('data-theme', theme.value)
  localStorage.setItem('tsbx-theme', theme.value)
}

// ---- state ----
const roster = ref<Identity[]>([])
const health = reactive<Health>({ zenoh: false, spire: false, hydra: false, at: 0 })
const svid = ref<Svid | null>(null)
const selected = ref<string>('acme-sensors')
const condition = ref<string>('fresh')
const result = ref<AttemptResp | null>(null)
const busy = ref(false)
const viz = ref<InstanceType<typeof LiveViz> | null>(null)

// The aspect viz lives at the top of the page, but the "Attempt to write" trigger
// is far below it. So when an attempt runs we mirror the SAME live gating viz into
// a temporary overlay, right where the viewer is looking, then let it dismiss.
const vizOverlay = ref<InstanceType<typeof LiveViz> | null>(null)
const showViz = ref(false)
let vizHideTimer: any = null
function closeVizOverlay() { showViz.value = false; if (vizHideTimer) { clearTimeout(vizHideTimer); vizHideTimer = null } }
function holdVizOverlay() { if (vizHideTimer) { clearTimeout(vizHideTimer); vizHideTimer = null } }
function scheduleVizHide(ms = 9000) { holdVizOverlay(); vizHideTimer = setTimeout(closeVizOverlay, ms) }

// the five gates, default unknown — filled by a real attempt
// Complementary checks — NOT a linear pipeline. ②+③ are two issuance paths for
// the SAME question (chosen by principal type); ④ gates who may ISSUE identity
// (not the per-write path); ⑤ is the per-action decision, keyed to the identity
// in ②/③; ① is an entry point whose admission comes from onboarding. A real
// operation lights only a SUBSET (org write = ①②⑤; workload = ①③⑤; mint = ④).
// Complementary checks — NOT a linear pipeline. Each identity DOMAIN fits a
// kind of principal (and so guards a kind of service): x509 for MACHINES,
// SPIRE for PROCESSES/workloads, Ory for HUMANS — you don't stack all of them
// on every service. ②+③ are two issuance paths for the same machine/process
// identity question. ④ AUTHENTICATES the human + their role (human-facing
// services); a human's role can AUTHORIZE an admin action like a mint, but Ory
// does NOT issue — the CA (machines) and SPIRE (workloads) do. ⑤ is the
// per-write decision keyed to the identity in ②/③; ① is an entry point whose
// admission comes from onboarding. A real action lights a SUBSET (machine write
// = ①②⑤; workload = ①③⑤; a human signing in to act = ①④).
const GATE_DEFS = [
  { gate: 'netbird', n: 'N', word: 'Network', q: 'Are you on the mesh at all?', layer: 'NetBird', rel: 'entry · from onboarding' },
  { gate: 'cert', n: 'M', word: 'Machine', q: 'What signed identity do you present?', layer: 'x509 cert (CA path)', rel: 'machines · org systems' },
  { gate: 'spire', n: 'W', word: 'Workload', q: '…or an attested, auto-rotating workload?', layer: 'SPIRE / SPIFFE', rel: 'processes · workloads' },
  { gate: 'ory', n: 'H', word: 'Human', q: 'Which human are you, and what role?', layer: 'Ory / Hydra', rel: 'humans · authN + role' },
  { gate: 'acl', n: 'D', word: 'Decision', q: 'Does that identity permit THIS action?', layer: 'Zenoh ACL', rel: 'decision · matches M/W' },
]
const gateMap = computed<Record<string, GateVerdict | null>>(() => {
  const m: Record<string, GateVerdict | null> = {}
  for (const g of GATE_DEFS) m[g.gate] = result.value?.gates.find((x) => x.gate === g.gate) || null
  return m
})

const conditions = [
  { id: 'fresh', label: 'fresh', warn: false, hint: 'everything in order' },
  { id: 'unenrolled', label: 'not enrolled', warn: true, hint: 'no network route' },
  { id: 'expired', label: 'stale credential', warn: true, hint: 'an expired, not-revoked cert' },
  { id: 'wrong-lane', label: 'wrong lane', warn: true, hint: 'writing outside its anchor' },
  { id: 'noisy', label: 'noisy / flooding', warn: true, hint: 'a misbehaving producer, not malice' },
  { id: 'forged', label: 'forged identity', warn: true, hint: 'rooted in no trusted issuer' },
]

// ---- Ory session ----
const token = ref<string | null>(getToken())
const claims = computed(() => (token.value ? decodeJwt(token.value) : null))
const me = computed(() => claims.value?.sub as string | undefined)
const myGroups = computed<string[]>(() => claims.value?.groups || [])
const isAdmin = computed(() => myGroups.value.includes('ADMIN'))

// ---- activity log ----
interface Log { id: number; tone: string; at: number; text: string }
const log = ref<Log[]>([])
let logId = 0
function note(tone: string, text: string) {
  log.value.unshift({ id: logId++, tone, at: Date.now(), text })
  if (log.value.length > 200) log.value.length = 200
}

// ---- lifecycle / mint panel ----
const mintAnchor = ref('acme-sensors')
const mintMsg = ref('')

// ---- dns / burn panel ----
const dnsMsg = ref('')
const dnsAddrs = ref<string[]>([])

const selectedId = computed(() => roster.value.find((i) => i.handle === selected.value) || null)
const attemptable = computed(() => roster.value.filter((i) => i.kind !== 'Operator' && i.kind !== 'operator'))

async function refresh() {
  try { roster.value = await api.roster() } catch { /* */ }
  try { Object.assign(health, await api.health()) } catch { /* */ }
  try { svid.value = await api.svid() } catch { svid.value = null }
}

async function doAttempt() {
  if (!selectedId.value) return
  busy.value = true; result.value = null
  showViz.value = true; holdVizOverlay()  // surface the live gating viz where the viewer is
  try {
    const r = await api.attempt({ identity: selected.value, condition: condition.value })
    result.value = r
    viz.value?.pushAttempt(r.gates as any)
    await nextTick()  // overlay viz has mounted by now; feed it the same real verdict
    vizOverlay.value?.pushAttempt(r.gates as any)
    note(r.silently_denied ? 'deny' : 'attempt', `${r.identity} [${r.condition}] → ${r.summary}`)
    scheduleVizHide()
  } catch (e) { note('deny', 'attempt failed: ' + e); scheduleVizHide(5000) }
  busy.value = false
}

// A stale token (e.g. after the sandbox was reset with `down -v` — Hydra's
// signing keys change) still decodes client-side but the server rejects it.
// Detect that and clear it so the buttons revert to the signed-out state with a
// clear prompt, instead of silently 401-ing.
function authStale(msg: string): boolean { return /not authenticated|no jwk|jwt|kid/i.test(msg || '') }
function clearStaleSession() {
  logout(); token.value = null
  mintMsg.value = 'Your session is no longer valid (the sandbox may have been reset). Click “Sign in (Ory)” to start again.'
}

async function doMint(rotate: boolean) {
  mintMsg.value = ''
  if (!token.value) { mintMsg.value = 'Sign in first — issuing identity requires an authenticated admin.'; return }
  try {
    const r = await api.mint({ anchor: mintAnchor.value, rotate, jwt: token.value })
    if (!r.ok && authStale(r.message)) { clearStaleSession(); return }
    mintMsg.value = r.message || (r.ok ? 'done' : 'failed')
    if (r.ok) note('lifecycle', `${rotate ? 'rotated' : 'minted'} ${mintAnchor.value}: serial ${r.serial?.slice(0, 12) || ''}`)
    refresh()
  } catch (e) { mintMsg.value = String(e) }
}
async function doRevoke(anchor: string) {
  if (!token.value) { mintMsg.value = 'Sign in as an admin to revoke.'; return }
  try {
    const r = await api.revoke({ anchor, jwt: token.value })
    if (r.ok === false && authStale(r.message)) { clearStaleSession(); return }
    mintMsg.value = r.message || 'revoked'
    if (r.ok) note('lifecycle', `revoked ${anchor}`)
    refresh()
  } catch (e) { mintMsg.value = String(e) }
}

async function doResolve() {
  try { const r = await api.resolve(); dnsAddrs.value = r.addresses || []; dnsMsg.value = r.note || '' }
  catch (e) { dnsMsg.value = String(e) }
}
async function doBurn(bad: boolean) {
  dnsMsg.value = 'respawning…'
  try {
    const r = await api.burn(bad)
    dnsAddrs.value = r.addresses || []
    dnsMsg.value = r.summary
    note('lifecycle', `burn & respawn (${bad ? 'bad cert' : 'fresh cert'}) → ${r.landed ? 'write landed' : 'write refused'}`)
  } catch (e) { dnsMsg.value = String(e) }
}

// ---- svid countdown ----
const ttl = ref(0)
let ttlTimer: any
const svidPct = computed(() => Math.max(0, Math.min(100, (ttl.value / 120) * 100)))

let stopWs: (() => void) | null = null
onMounted(async () => {
  // Drop an expired token on load so the buttons reflect a real session.
  if (token.value && claims.value?.exp && claims.value.exp * 1000 < Date.now()) { logout(); token.value = null }
  await refresh()
  window.addEventListener('keydown', onKey)
  stopWs = connectWs((e) => {
    if (e.type === 'sample') { viz.value?.ping(); if (showViz.value) vizOverlay.value?.ping() }
    else if (e.type === 'health') Object.assign(health, { zenoh: e.zenoh, spire: e.spire, hydra: e.hydra, at: e.at })
    else if (e.type === 'lifecycle') note('lifecycle', `${e.action} · ${e.anchor}`)
  })
  if (me.value) note('lifecycle', `signed in as ${me.value} (${myGroups.value.join(', ') || 'no role'})`)
  // poll svid ttl
  ttlTimer = setInterval(async () => {
    try { const s = await api.svid(); svid.value = s; ttl.value = s.ttl_seconds || 0 } catch { /* */ }
  }, 3000)
})
onBeforeUnmount(() => { stopWs?.(); clearInterval(ttlTimer); window.removeEventListener('keydown', onKey); holdVizOverlay() })

function onKey(e: KeyboardEvent) { if (e.key === 'Escape' && showViz.value) closeVizOverlay() }
function fmtTime(t: number) { return new Date(t).toLocaleTimeString() }
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
        <div>
          <h1>ZERO-TRUST&nbsp;SANDBOX</h1>
          <p class="sub">The zero-trust foundation of the __GOATNET__ Backbone — made hands-on.</p>
        </div>
      </div>
      <span class="spacer" />
      <a class="navlink" href="/vision.html" title="The bigger picture">✦ THE BIGGER PICTURE</a>
      <a class="navlink" href="/glossary.html" title="Plain-language definitions">📖 GLOSSARY</a>
      <button class="theme-toggle" @click="toggleTheme">
        <span v-if="theme==='dark'">◐ LIGHT</span><span v-else>◑ DARK</span>
      </button>
    </header>

    <section class="hero">
      <h2>There's no <span class="accent">"inside"</span> to get into.</h2>
      <p class="lede">The zero-trust foundation of the <b>__GOATNET__ Backbone</b> — notional and ephemeral here, but every layer mirrors the real design. Don't take our word for it:</p>
      <ul class="hero-do">
        <li><span class="hd-k">Be someone, act for real</span> — pick a principal, run a true end-to-end action, watch which aspects apply.</li>
        <li><span class="hd-k">Break something</span> — go stale, flood, write out of your lane, drop enrollment — and watch the right aspect catch it and name what it protected.</li>
        <li><span class="hd-k">Run identity live</span> — as an admin, mint, rotate, and revoke on the fly.</li>
      </ul>
      <p class="hero-take">Most of what goes wrong isn't an attack — it's ordinary and human. The same checks that would
        stop an attacker contain it just as well.</p>
    </section>

    <div class="callout">
      <span>🧪</span>
      <span><b>Throwaway + fully local.</b> Real CA, real SPIRE, real OIDC, a real access decision — running on this
        machine, peering with nothing. <code>docker compose down -v</code> wipes it. Nothing here is real, ever.</span>
    </div>

    <!-- status bar — the real components running locally -->
    <div class="panel span-2" style="margin-bottom:16px">
      <div class="statusbar">
        <span class="tag" style="margin-right:4px">LIVE COMPONENTS</span>
        <span class="layerled" :class="health.zenoh ? 'up':'down'"><span class="led" /> FABRIC</span>
        <span class="layerled" :class="health.spire ? 'up':'down'"><span class="led" /> SPIRE</span>
        <span class="layerled" :class="health.hydra ? 'up':'down'"><span class="led" /> ORY</span>
        <span class="layerled represented"><span class="led" /> NETBIRD (represented)</span>
        <span class="spacer" />
        <span v-if="me" class="who">signed in: <b>{{ me }}</b> · {{ myGroups.join(', ') || 'no role' }}
          <button class="sm ghost" @click="logout(); token=null; result=null">sign out</button></span>
        <span v-else class="who notsignedin">Not signed in ·
          <button class="linklike" @click="startLogin()">Sign in (Ory)</button> to unlock admin actions below</span>
      </div>
    </div>

    <!-- the five-gate integrity panel -->
    <div class="panel span-2 gates-panel" style="margin-bottom:16px">
      <div class="panel-head"><span class="dot accent" /><h3>Five aspects of identity &amp; access</h3>
        <span class="hint">complementary &amp; coordinated — not a sequence; an action uses only the ones that apply · click any for its anatomy</span></div>
      <div class="gates">
        <a v-for="g in GATE_DEFS" :key="g.gate" class="gate" :class="gateMap[g.gate]?.state || 'idle'"
           :href="'/concept.html?gate=' + g.gate" :title="'Open the ' + g.layer + ' anatomy →'">
          <span v-if="gateMap[g.gate]" class="repbadge" :class="gateMap[g.gate]!.live ? 'live':'rep'">
            {{ gateMap[g.gate]!.live ? 'LIVE' : 'REPR' }}</span>
          <div class="n"><span class="sym" :class="'sym-' + g.n">{{ g.n }}</span><span class="deeper">↗</span></div>
          <div class="big" />
          <div class="layer">{{ g.layer }}</div>
          <div class="rel">{{ g.rel }}</div>
          <div class="q">{{ g.q }}</div>
        </a>
      </div>
      <p class="gates-note">They coordinate; they aren't a sequence you pass through. An action uses only the aspects that
        apply — a machine write uses <span class="sym sym-N">N</span><span class="sym sym-M">M</span><span class="sym sym-D">D</span>,
        a workload <span class="sym sym-N">N</span><span class="sym sym-W">W</span><span class="sym sym-D">D</span>, a human
        signing in to act <span class="sym sym-N">N</span><span class="sym sym-H">H</span>. Run an attempt below to see it live.</p>
      <LiveViz ref="viz" style="margin-top:6px" />
      <div class="viz-cap">On each attempt the aspects that apply light up with a real verdict; the marker stops where one refused it. Amber blips are real writes the observer saw land.</div>
    </div>

    <main class="grid">
      <!-- the unified identity model: who you are + what you do -->
      <div class="panel">
        <div class="panel-head"><span class="dot amber" /><h3>Who are you?</h3>
          <span class="hint">two kinds of principal act here</span></div>
        <p class="whoami-intro">A <b>human</b> signs in with <b>Ory</b> — the single sign-on that gates the mesh's
          browser tools (dashboards, viewers, consoles). Most access is just <b>viewing and operating</b>; your
          <b>role</b> decides the rest, and an <b>admin</b> role additionally lets you issue, rotate, and revoke
          identities (what you can try below). A <b>machine</b> or <b>process</b> is a different thing — an identity you
          <b>act as</b> to write to the fabric.</p>

        <!-- human -->
        <div class="whoami-group">
          <div class="wg-label">Human <span class="wg-sub">Ory · single sign-on + your role</span></div>
          <div v-if="me" class="signed">Signed in as <b>{{ me }}</b> · {{ myGroups.join(', ') || 'no role' }}
            <button class="sm ghost" @click="logout(); token=null; result=null">sign out</button></div>
          <button v-else class="sm primary" @click="startLogin()">Sign in (Ory)</button>
          <div class="wg-hint">Then “Identity, on the fly” (right) unlocks mint / rotate / revoke — ADMIN only.</div>
        </div>

        <!-- machine / process -->
        <div class="whoami-group">
          <div class="wg-label">Machine or process <span class="wg-sub">an identity you act as · to write</span></div>
          <div class="cards">
            <button v-for="i in attemptable" :key="i.handle" class="idcard"
                    :class="{ on: selected===i.handle, revoked: i.revoked }" @click="selected=i.handle">
              <div>
                <div class="name">{{ i.handle }} <span v-if="i.revoked" class="rev">· revoked</span></div>
                <div class="blurb">{{ i.blurb }}</div>
              </div>
              <span class="kind">{{ i.kind }}</span>
            </button>
          </div>
          <div class="field" style="margin-top:12px"><label>Change one thing (the knockout)</label>
            <div class="chips">
              <button v-for="c in conditions" :key="c.id" class="chip" :class="{ on: condition===c.id, warn: c.warn }"
                      :title="c.hint" @click="condition=c.id">{{ c.label }}</button>
            </div>
          </div>
          <div class="row" style="margin-top:10px">
            <button class="primary" :disabled="busy" @click="doAttempt">
              {{ busy ? 'running a real attempt…' : '▶ Attempt to write' }}</button>
          </div>
          <div v-if="result" class="result" :class="result.silently_denied || result.expected.startsWith('deny') || result.expected.startsWith('no') || result.expected==='handshake refused' ? 'deny':'allow'">
            <div class="summary">{{ result.summary }}</div>
            <div class="eo">
              <span class="k">key</span><span class="v">{{ result.key }}</span>
              <span class="k">expected</span><span class="v">{{ result.expected }}</span>
              <span class="k">observed</span><span class="v">{{ result.observed }}</span>
            </div>
            <div v-if="result.silently_denied" class="silent" style="margin-top:8px">
              ⚠ silent denial — the sender got no error. That's the lesson, not a bug.
            </div>
          </div>
          <div v-if="selectedId?.uri_san" class="who" style="margin-top:10px">identity grammar:
            <b class="mono">{{ selectedId!.uri_san }}</b></div>
        </div>
      </div>

      <!-- the two issuance paths -->
      <div class="panel">
        <div class="panel-head"><span class="dot green" /><h3>Identity, on the fly</h3>
          <span class="hint">autonomy + dynamism · ADMIN only</span></div>
        <p class="blurb" style="color:var(--ink-dim);font-size:12.5px;margin:0 0 12px">
          As an <b>admin human</b>, issuing, rotating, and revoking identity is fast and local — not a slow central queue.
        </p>

        <!-- access-state call to action: this is itself aspect ④ at work -->
        <div v-if="!isAdmin" class="admin-cta">
          <template v-if="!me">
            <button class="primary sm" @click="startLogin()">🔑 Sign in to unlock</button>
            <span class="cta-hint">these are locked until a human signs in (Ory). Pick <b>alice-admin</b> — she has the ADMIN role.</span>
          </template>
          <template v-else>
            <span class="cta-hint">Signed in as <b>{{ me }}</b> ({{ myGroups.join(', ') || 'no role' }}) — but only an
              <b>ADMIN</b> may issue. That refusal <i>is</i> the <span class="sym sym-H">H</span> aspect (human + role) doing its job.</span>
            <button class="sm ghost" @click="logout(); token=null; result=null; startLogin()">switch to alice-admin</button>
          </template>
        </div>

        <div class="field" :class="{ 'disabled-veil': !isAdmin }"><label>anchor</label>
          <select v-model="mintAnchor">
            <option value="acme-sensors">acme-sensors (org system)</option>
            <option value="sensor-gw">sensor-gw (backing service)</option>
          </select>
        </div>
        <div class="row" :class="{ 'disabled-veil': !isAdmin }">
          <button class="primary" :disabled="!isAdmin" @click="doMint(false)" title="first issuance">Mint</button>
          <button :disabled="!isAdmin" @click="doMint(true)" title="re-key, same anchor">Rotate</button>
          <button class="danger" :disabled="!isAdmin" @click="doRevoke(mintAnchor)">Revoke (mesh-first)</button>
        </div>
        <div v-if="mintMsg" class="result" style="margin-top:12px"><div class="summary">{{ mintMsg }}</div></div>
        <div class="lifecycle-deep">
          <a href="/deep.html?topic=rotate">↻ How rotation works</a>
          <a href="/deep.html?topic=revoke">⊘ How revocation works</a>
        </div>

        <div class="panel-head" style="margin-top:20px"><span class="dot ask" /><h3 style="font-size:13px">SPIRE — still valid right now?</h3></div>
        <div v-if="svid?.ok" class="svid">
          <div><div class="ttl">{{ svid.ttl_seconds }}s</div><div class="tag">until SVID rotates</div></div>
          <div style="flex:1">
            <div class="mono" style="font-size:11px;color:var(--ink-dim)">{{ svid.spiffe_id }}</div>
            <div class="bar"><span :style="{ width: svidPct + '%' }" /></div>
          </div>
        </div>
        <div v-else class="who">SPIRE not issuing yet — the workload path lights up once the agent attests.</div>
      </div>

      <!-- private DNS + burn-respawn -->
      <div class="panel span-2">
        <div class="panel-head"><span class="dot accent" /><h3>A name tells you <i>where</i> — not whether to trust</h3>
          <span class="hint">private mesh DNS · burn &amp; respawn</span></div>
        <p class="dns-intro"><b>A name tells you where to go — not whether to trust what's there.</b> Every connection
          re-checks identity for itself, so <code>sensor-gw.trust.local</code> only has to point to an address.</p>
        <ul class="dns-notes">
          <li><span class="dn-k">The name never vouches for the endpoint</span> — even a correct address just points you
            somewhere; the service there still has to prove who it is on connect. (The lookup isn't tamperable either:
            inside the private mesh it rides the encrypted, authenticated tunnel. The only DNS that lives outside the
            mesh is the one-time network enrollment, and that adds DNSSEC on top of the other checks.)</li>
          <li><span class="dn-k">The payoff</span> — tear a service down and stand a fresh one up under the <b>same
            name</b> any time; nobody changes the address they use, because the new one re-proves its own identity.</li>
        </ul>
        <p class="dns-go">Try it in order:</p>
        <ol class="dns-steps">
          <li>
            <button class="sm" @click="doResolve">① Look up the name</button>
            <span class="hint2">A name gives you an address — and nothing more.</span>
          </li>
          <li>
            <button class="sm primary" @click="doBurn(false)">② Replace it with a fresh, real service</button>
            <span class="hint2">New service, new credential, same name — watch it <b>still work</b>, because the new one re-proved itself.</span>
          </li>
          <li>
            <button class="sm danger" @click="doBurn(true)">③ Replace it with an impostor</button>
            <span class="hint2">Same name, same address — but no valid identity. Watch the connection get <b>refused</b>; the name didn't help it.</span>
          </li>
        </ol>
        <div v-if="dnsAddrs.length" class="who" style="margin-top:4px">resolved: {{ 'sensor-gw.trust.local → ' + dnsAddrs.join(', ') }}</div>
        <div v-if="dnsMsg" class="result" :class="/refused|impostor|bad/i.test(dnsMsg) ? 'deny':'allow'" style="margin-top:8px">
          <div class="summary">{{ dnsMsg }}</div></div>
        <p class="dns-take"><b>The takeaway:</b> the name is just an address. Trust comes from identity, re-checked
          every time — which is exactly why a service can be swapped, patched, or moved in seconds without anyone
          ever trusting the wrong thing.</p>
      </div>

      <!-- activity log -->
      <div class="panel">
        <div class="panel-head"><span class="dot amber" /><h3>Activity</h3>
          <span class="hint">real events from the orchestrator</span></div>
        <div class="logwrap">
          <div v-for="l in log" :key="l.id" class="logline">
            <span class="t">{{ fmtTime(l.at) }}</span><span class="b" :class="l.tone" /><span class="m">{{ l.text }}</span>
          </div>
          <div v-if="!log.length" class="who">Run an attempt or mint an identity — real events land here.</div>
        </div>
      </div>

      <!-- honesty matrix -->
      <div class="panel">
        <div class="panel-head"><span class="dot green" /><h3>Live vs represented</h3>
          <span class="hint">honest by construction</span></div>
        <table class="matrix">
          <tr><th>Layer</th><th>State</th><th></th></tr>
          <tr><td class="lyr">x509 cert (CA)</td><td><span class="pill live">LIVE</span></td><td class="d">real CA, real mTLS handshake</td></tr>
          <tr><td class="lyr">SPIRE / SVID</td><td><span class="pill live">LIVE</span></td><td class="d">real attestation + rotation</td></tr>
          <tr><td class="lyr">Zenoh ACL</td><td><span class="pill live">LIVE</span></td><td class="d">real allow / silent-deny</td></tr>
          <tr><td class="lyr">Ory JWT + role</td><td><span class="pill live">LIVE</span></td><td class="d">real OIDC; role gates issuance</td></tr>
          <tr><td class="lyr">Ory credential store</td><td><span class="pill rep">REPR</span></td><td class="d">demo-persona picker stands in for Kratos passwords/MFA</td></tr>
          <tr><td class="lyr">Private DNS</td><td><span class="pill live">LIVE</span></td><td class="d">real CoreDNS resolve + re-point</td></tr>
          <tr><td class="lyr">NetBird / WireGuard</td><td><span class="pill rep">REPR</span></td><td class="d">a browser can't do the WG handshake — shown, never faked</td></tr>
        </table>
      </div>
    </main>

    <footer class="foot">
      A teaching miniature that runs the real components · isolated, disposable, peers with nothing.<br />
      New here? Start with <a href="/vision.html">the bigger picture</a>, then come back and knock a layer out.
    </footer>
    <!-- live gating viz, surfaced at the trigger so you don't have to scroll up -->
    <transition name="vizfade">
      <div v-if="showViz" class="viz-overlay" @click.self="closeVizOverlay">
        <div class="viz-modal" @mouseenter="holdVizOverlay">
          <div class="vm-head">
            <span class="dot accent" /><h3>Your attempt, live</h3>
            <span class="vm-sub" v-if="selectedId">{{ selectedId.handle }} · {{ condition }}</span>
            <button class="vm-close" @click="closeVizOverlay" aria-label="close">✕</button>
          </div>
          <LiveViz ref="vizOverlay" />
          <div v-if="busy" class="vm-status">running a real attempt…</div>
          <div v-else-if="result" class="result"
               :class="result.silently_denied || result.expected.startsWith('deny') || result.expected.startsWith('no') || result.expected==='handshake refused' ? 'deny':'allow'">
            <div class="summary">{{ result.summary }}</div>
            <div class="eo">
              <span class="k">key</span><span class="v">{{ result.key }}</span>
              <span class="k">expected</span><span class="v">{{ result.expected }}</span>
              <span class="k">observed</span><span class="v">{{ result.observed }}</span>
            </div>
            <div v-if="result.silently_denied" class="silent" style="margin-top:8px">
              ⚠ silent denial — the sender got no error. That's the lesson, not a bug.
            </div>
          </div>
          <div class="vm-foot">The aspects that apply light up with a real verdict; the marker stops where one
            refused it. <span class="vm-dismiss">Esc / click outside to dismiss</span></div>
        </div>
      </div>
    </transition>
    <GlossaryPopup />
  </div>
</template>
