// SPDX-License-Identifier: Apache-2.0
// index.html — the playground. The page copy is static HTML; this module owns
// the state, talks to the orchestrator (api.js) and re-renders the parts of the
// page that state drives. Each render function reads `state` and paints one
// region; anything that changes state calls the renders it affects.
import { initTheme, h } from './common.js'
import { initGlossPopup } from './gloss-popup.js'
import { createLiveViz } from './live-viz.js'
import { api, connectWs, startLogin, getToken, logout, decodeJwt } from './api.js'

const $ = (id) => document.getElementById(id)

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

const CONDITIONS = [
  { id: 'fresh', label: 'fresh', warn: false, hint: 'everything in order' },
  { id: 'unenrolled', label: 'not enrolled', warn: true, hint: 'no network route' },
  { id: 'expired', label: 'stale credential', warn: true, hint: 'an expired, not-revoked cert' },
  { id: 'wrong-lane', label: 'wrong lane', warn: true, hint: 'writing outside its anchor' },
  { id: 'noisy', label: 'noisy / flooding', warn: true, hint: 'a misbehaving producer, not malice' },
  { id: 'forged', label: 'forged identity', warn: true, hint: 'rooted in no trusted issuer' },
]

// ---- state ----
const state = {
  roster: [],
  health: { zenoh: false, spire: false, hydra: false, at: 0 },
  svid: null,
  ttl: 0,  // the SVID countdown bar; refreshed by the 3s poll below
  selected: 'acme-sensors',
  condition: 'fresh',
  result: null,
  busy: false,
  token: getToken(),
  mintMsg: '',
  dnsMsg: '',
  dnsAddrs: [],
  log: [],
  showViz: false,
}
let logId = 0

// ---- derived ----
const claims = () => (state.token ? decodeJwt(state.token) : null)
const me = () => claims()?.sub
const myGroups = () => claims()?.groups || []
const isAdmin = () => myGroups().includes('ADMIN')
const selectedId = () => state.roster.find((i) => i.handle === state.selected) || null
const attemptable = () => state.roster.filter((i) => i.kind !== 'Operator' && i.kind !== 'operator')
const isDeny = (r) => r.silently_denied || r.expected.startsWith('deny') || r.expected.startsWith('no') || r.expected === 'handshake refused'

// ---- renders ----
function renderHealth() {
  for (const k of ['zenoh', 'spire', 'hydra']) {
    const el = $('led-' + k)
    el.classList.toggle('up', !!state.health[k]); el.classList.toggle('down', !state.health[k])
  }
}

// Every [data-when="in"|"out"] element shows for a signed-in / signed-out human;
// .js-me and .js-groups carry who that is. The admin call-to-action and the
// veil over mint / rotate / revoke follow the ADMIN role.
function renderSession() {
  const who = me()
  const groups = myGroups().join(', ') || 'no role'
  for (const el of document.querySelectorAll('[data-when]')) el.hidden = (el.dataset.when === 'in') !== !!who
  for (const el of document.querySelectorAll('.js-me')) el.textContent = who || ''
  for (const el of document.querySelectorAll('.js-groups')) el.textContent = groups
  const admin = isAdmin()
  $('admin-cta').hidden = admin
  $('mint-field').classList.toggle('disabled-veil', !admin)
  $('mint-row').classList.toggle('disabled-veil', !admin)
  for (const id of ['mint-btn', 'rotate-btn', 'revoke-btn']) $(id).disabled = !admin
}

function renderGates() {
  const verdicts = state.result?.gates || []
  $('gates').replaceChildren(...GATE_DEFS.map((g) => {
    const v = verdicts.find((x) => x.gate === g.gate) || null
    return h('a', { class: 'gate ' + (v?.state || 'idle'), href: '/concept.html?gate=' + g.gate, title: 'Open the ' + g.layer + ' anatomy →' },
      v ? h('span', { class: 'repbadge ' + (v.live ? 'live' : 'rep') }, v.live ? 'LIVE' : 'REPR') : null,
      h('div', { class: 'n' }, h('span', { class: 'sym sym-' + g.n }, g.n), h('span', { class: 'deeper' }, '↗')),
      h('div', { class: 'big' }),
      h('div', { class: 'layer' }, g.layer),
      h('div', { class: 'rel' }, g.rel),
      h('div', { class: 'q' }, g.q))
  }))
}

function renderCards() {
  $('cards').replaceChildren(...attemptable().map((i) =>
    h('button', {
      class: 'idcard' + (state.selected === i.handle ? ' on' : '') + (i.revoked ? ' revoked' : ''),
      onclick: () => { state.selected = i.handle; renderCards(); renderGrammar(); renderOverlay() },
    },
    h('div', null,
      h('div', { class: 'name' }, i.handle + ' ', i.revoked ? h('span', { class: 'rev' }, '· revoked') : null),
      h('div', { class: 'blurb' }, i.blurb)),
    h('span', { class: 'kind' }, i.kind))))
}

function renderChips() {
  $('chips').replaceChildren(...CONDITIONS.map((c) =>
    h('button', {
      class: 'chip' + (state.condition === c.id ? ' on' : '') + (c.warn ? ' warn' : ''),
      title: c.hint,
      onclick: () => { state.condition = c.id; renderChips(); renderOverlay() },
    }, c.label)))
}

function renderGrammar() {
  const uri = selectedId()?.uri_san
  $('grammar').hidden = !uri
  $('grammar-v').textContent = uri || ''
}

// The attempt verdict, drawn the same way under the button and in the overlay.
function fillResult(el, r) {
  el.hidden = !r
  if (!r) { el.replaceChildren(); return }
  el.className = 'result ' + (isDeny(r) ? 'deny' : 'allow')
  el.replaceChildren(
    h('div', { class: 'summary' }, r.summary),
    h('div', { class: 'eo' },
      h('span', { class: 'k' }, 'key'), h('span', { class: 'v' }, r.key),
      h('span', { class: 'k' }, 'expected'), h('span', { class: 'v' }, r.expected),
      h('span', { class: 'k' }, 'observed'), h('span', { class: 'v' }, r.observed)),
    r.silently_denied
      ? h('div', { class: 'silent', style: { marginTop: '8px' } }, '⚠ silent denial — the sender got no error. That\'s the lesson, not a bug.')
      : null)
}

function renderAttempt() {
  const btn = $('attempt-btn')
  btn.disabled = state.busy
  btn.textContent = state.busy ? 'running a real attempt…' : '▶ Attempt to write'
  fillResult($('result-main'), state.result)
  renderGates()
  renderOverlay()
}

function renderMint() {
  const el = $('mint-msg')
  el.hidden = !state.mintMsg
  el.querySelector('.summary').textContent = state.mintMsg
}

function renderSvid() {
  const s = state.svid
  $('svid').hidden = !s?.ok
  $('svid-none').hidden = !!s?.ok
  if (s?.ok) {
    $('svid-ttl').textContent = `${s.ttl_seconds}s`
    $('svid-id').textContent = s.spiffe_id || ''
    $('svid-bar').style.width = Math.max(0, Math.min(100, (state.ttl / 120) * 100)) + '%'
  }
}

function renderDns() {
  const a = $('dns-addrs')
  a.hidden = !state.dnsAddrs.length
  a.textContent = 'resolved: ' + 'sensor-gw.trust.local → ' + state.dnsAddrs.join(', ')
  const m = $('dns-msg')
  m.hidden = !state.dnsMsg
  m.className = 'result ' + (/refused|impostor|bad/i.test(state.dnsMsg) ? 'deny' : 'allow')
  m.querySelector('.summary').textContent = state.dnsMsg
}

function renderLog() {
  const wrap = $('log')
  const empty = $('log-empty')
  empty.hidden = state.log.length > 0
  wrap.replaceChildren(...state.log.map((l) =>
    h('div', { class: 'logline' },
      h('span', { class: 't' }, new Date(l.at).toLocaleTimeString()),
      h('span', { class: 'b ' + l.tone }),
      h('span', { class: 'm' }, l.text))), empty)
}

// ---- activity log ----
function note(tone, text) {
  state.log.unshift({ id: logId++, tone, at: Date.now(), text })
  if (state.log.length > 200) state.log.length = 200
  renderLog()
}

// ---- the live viz, and its overlay ----
// The aspect viz lives at the top of the page, but the "Attempt to write" trigger
// is far below it. So when an attempt runs we mirror the SAME live gating viz into
// a temporary overlay, right where the viewer is looking, then let it dismiss.
const viz = createLiveViz($('viz-main'))
let vizOverlay = null
let vizHideTimer = null

function renderOverlay() {
  if (!state.showViz) return
  const sel = selectedId()
  $('vm-sub').hidden = !sel
  $('vm-sub').textContent = sel ? `${sel.handle} · ${state.condition}` : ''
  $('vm-status').hidden = !state.busy
  fillResult($('result-overlay'), state.busy ? null : state.result)
}
function openVizOverlay() {
  if (state.showViz) return
  state.showViz = true
  const el = $('viz-overlay')
  el.hidden = false
  el.classList.add('vizfade-enter-active', 'vizfade-enter-from')
  requestAnimationFrame(() => requestAnimationFrame(() => el.classList.remove('vizfade-enter-from')))
  setTimeout(() => el.classList.remove('vizfade-enter-active'), 200)
  vizOverlay = createLiveViz($('viz-overlay-canvas'))
  renderOverlay()
}
function closeVizOverlay() {
  if (vizHideTimer) { clearTimeout(vizHideTimer); vizHideTimer = null }
  if (!state.showViz) return
  state.showViz = false
  vizOverlay?.destroy(); vizOverlay = null
  const el = $('viz-overlay')
  el.classList.add('vizfade-leave-active', 'vizfade-leave-to')
  setTimeout(() => { el.hidden = true; el.classList.remove('vizfade-leave-active', 'vizfade-leave-to') }, 180)
}
function holdVizOverlay() { if (vizHideTimer) { clearTimeout(vizHideTimer); vizHideTimer = null } }
function scheduleVizHide(ms = 9000) { holdVizOverlay(); vizHideTimer = setTimeout(closeVizOverlay, ms) }

// ---- actions ----
async function refresh() {
  try { state.roster = await api.roster() } catch { /* */ }
  try { Object.assign(state.health, await api.health()) } catch { /* */ }
  try { state.svid = await api.svid() } catch { state.svid = null }
  renderCards(); renderGrammar(); renderHealth(); renderSvid()
}

async function doAttempt() {
  if (!selectedId()) return
  state.busy = true; state.result = null
  openVizOverlay(); holdVizOverlay()  // surface the live gating viz where the viewer is
  renderAttempt()
  try {
    const r = await api.attempt({ identity: state.selected, condition: state.condition })
    state.result = r
    viz.pushAttempt(r.gates)
    vizOverlay?.pushAttempt(r.gates)  // feed the overlay the same real verdict
    note(r.silently_denied ? 'deny' : 'attempt', `${r.identity} [${r.condition}] → ${r.summary}`)
    scheduleVizHide()
  } catch (e) { note('deny', 'attempt failed: ' + e); scheduleVizHide(5000) }
  state.busy = false
  renderAttempt()
}

function signOut() {
  logout(); state.token = null; state.result = null
  renderSession(); renderAttempt()
}

// A stale token (e.g. after the sandbox was reset with `down -v` — Hydra's
// signing keys change) still decodes client-side but the server rejects it.
// Detect that and clear it so the buttons revert to the signed-out state with a
// clear prompt, instead of silently 401-ing.
const authStale = (msg) => /not authenticated|no jwk|jwt|kid/i.test(msg || '')
function clearStaleSession() {
  logout(); state.token = null
  state.mintMsg = 'Your session is no longer valid (the sandbox may have been reset). Click “Sign in (Ory)” to start again.'
  renderSession(); renderMint()
}

async function doMint(rotate) {
  const anchor = $('mint-anchor').value
  state.mintMsg = ''; renderMint()
  if (!state.token) { state.mintMsg = 'Sign in first — issuing identity requires an authenticated admin.'; renderMint(); return }
  try {
    const r = await api.mint({ anchor, rotate, jwt: state.token })
    if (!r.ok && authStale(r.message)) { clearStaleSession(); return }
    state.mintMsg = r.message || (r.ok ? 'done' : 'failed')
    if (r.ok) note('lifecycle', `${rotate ? 'rotated' : 'minted'} ${anchor}: serial ${r.serial?.slice(0, 12) || ''}`)
    refresh()
  } catch (e) { state.mintMsg = String(e) }
  renderMint()
}
async function doRevoke(anchor) {
  if (!state.token) { state.mintMsg = 'Sign in as an admin to revoke.'; renderMint(); return }
  try {
    const r = await api.revoke({ anchor, jwt: state.token })
    if (r.ok === false && authStale(r.message)) { clearStaleSession(); return }
    state.mintMsg = r.message || 'revoked'
    if (r.ok) note('lifecycle', `revoked ${anchor}`)
    refresh()
  } catch (e) { state.mintMsg = String(e) }
  renderMint()
}

async function doResolve() {
  try { const r = await api.resolve(); state.dnsAddrs = r.addresses || []; state.dnsMsg = r.note || '' }
  catch (e) { state.dnsMsg = String(e) }
  renderDns()
}
async function doBurn(bad) {
  state.dnsMsg = 'respawning…'; renderDns()
  try {
    const r = await api.burn(bad)
    state.dnsAddrs = r.addresses || []
    state.dnsMsg = r.summary
    note('lifecycle', `burn & respawn (${bad ? 'bad cert' : 'fresh cert'}) → ${r.landed ? 'write landed' : 'write refused'}`)
  } catch (e) { state.dnsMsg = String(e) }
  renderDns()
}

// ---- wiring ----
initTheme()
for (const b of document.querySelectorAll('[data-act="signin"]')) b.addEventListener('click', () => startLogin())
for (const b of document.querySelectorAll('[data-act="signout"]')) b.addEventListener('click', signOut)
for (const b of document.querySelectorAll('[data-act="switch"]')) b.addEventListener('click', () => { signOut(); startLogin() })
$('attempt-btn').addEventListener('click', doAttempt)
$('mint-btn').addEventListener('click', () => doMint(false))
$('rotate-btn').addEventListener('click', () => doMint(true))
$('revoke-btn').addEventListener('click', () => doRevoke($('mint-anchor').value))
$('dns-resolve').addEventListener('click', doResolve)
$('dns-burn-good').addEventListener('click', () => doBurn(false))
$('dns-burn-bad').addEventListener('click', () => doBurn(true))
$('viz-overlay').addEventListener('click', (e) => { if (e.target === e.currentTarget) closeVizOverlay() })
$('vm-close').addEventListener('click', closeVizOverlay)
document.querySelector('#viz-overlay .viz-modal').addEventListener('mouseenter', holdVizOverlay)
window.addEventListener('keydown', (e) => { if (e.key === 'Escape' && state.showViz) closeVizOverlay() })

// Drop an expired token on load so the buttons reflect a real session.
if (state.token && claims()?.exp && claims().exp * 1000 < Date.now()) { logout(); state.token = null }
renderSession(); renderGates(); renderChips(); renderAttempt(); renderMint(); renderSvid(); renderDns(); renderLog()
initGlossPopup()

await refresh()
connectWs((e) => {
  if (e.type === 'sample') { viz.ping(); if (state.showViz) vizOverlay?.ping() }
  else if (e.type === 'health') { Object.assign(state.health, { zenoh: e.zenoh, spire: e.spire, hydra: e.hydra, at: e.at }); renderHealth() }
  else if (e.type === 'lifecycle') note('lifecycle', `${e.action} · ${e.anchor}`)
})
if (me()) note('lifecycle', `signed in as ${me()} (${myGroups().join(', ') || 'no role'})`)
// poll svid ttl
setInterval(async () => {
  try { const s = await api.svid(); state.svid = s; state.ttl = s.ttl_seconds || 0; renderSvid() } catch { /* */ }
}, 3000)
