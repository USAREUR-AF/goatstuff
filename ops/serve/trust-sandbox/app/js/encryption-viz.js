// SPDX-License-Identifier: Apache-2.0
// Encryption, two ways. Every interaction on the fabric is encrypted end-to-end
// TWICE — once by the WireGuard peer-to-peer tunnel (network layer) and again by
// the mTLS tunnel inside Zenoh (application layer; other apps would use their own
// TLS, usually mutual too). Two independent layers, different origins and
// algorithms, each swappable — so it's defence in depth, not one point of failure.
//   • 'nested'   — one tunnel wrapped inside the other (the shape)
//   • 'sequence' — the handshakes + encrypted exchange over time (the order)
import { h, setText } from './common.js'

// mountEncryptionViz(root, 'nested' | 'sequence') builds the canvas + controls inside root.
export function mountEncryptionViz(root, modeArg) {
const mode = modeArg ?? 'nested'
let playing = true
let ctx = null
let raf = 0, W = 900, H = 360, t0 = 0, pausedAt = 0, pausedAcc = 0
const caption = (mode === 'nested'
  ? 'Two independent encrypted tunnels, one inside the other — different algorithms, either alone keeps the bytes secret.'
  : 'Two handshakes, then encrypted traffic: WireGuard brings up the outer tunnel, mTLS the inner one.')

function color(n) { const c = getComputedStyle(document.documentElement).getPropertyValue(n).trim(); return c || '#888' }
function lerp(a, b, u) { return a + (b - a) * Math.max(0, Math.min(1, u)) }
function ease(u) { return u < 0.5 ? 2 * u * u : 1 - Math.pow(-2 * u + 2, 2) / 2 }
function resize() { const c = canvas; W = c.clientWidth; H = c.clientHeight; const d = window.devicePixelRatio || 1; c.width = W * d; c.height = H * d; ctx = c.getContext('2d'); ctx?.scale(d, d) }
function clock() { return (playing ? performance.now() - t0 : pausedAt - t0) - pausedAcc }

function rr(c, x, y, w, h, r) { c.beginPath(); c.moveTo(x + r, y); c.arcTo(x + w, y, x + w, y + h, r); c.arcTo(x + w, y + h, x, y + h, r); c.arcTo(x, y + h, x, y, r); c.arcTo(x, y, x + w, y, r); c.closePath() }
function padlock(c, x, y, sz, col) {
  c.save(); c.strokeStyle = col; c.fillStyle = col; c.lineWidth = 1.5
  c.beginPath(); c.arc(x, y - sz * 0.55, sz * 0.55, Math.PI, 0); c.stroke()
  rr(c, x - sz * 0.72, y - sz * 0.12, sz * 1.44, sz * 1.2, 1.6); c.fill(); c.restore()
}
function peer(c, x, y, label, col, faint) {
  c.fillStyle = col; c.shadowColor = col; c.shadowBlur = 8; c.beginPath(); c.arc(x, y, 8, 0, Math.PI * 2); c.fill(); c.shadowBlur = 0
  c.fillStyle = faint; c.font = '10px ui-monospace, monospace'; c.textAlign = 'center'; c.fillText(label, x, y + 22)
}

const CYCLE = 8400
function draw() {
  raf = requestAnimationFrame(draw)
  if (!ctx) return
  const c = ctx, now = clock()
  c.clearRect(0, 0, W, H)
  if (mode === 'nested') drawNested(c, now); else drawSequence(c, now)
}

// ── nested tunnels ───────────────────────────────────────────────────────────
function drawNested(c, now) {
  const faint = color('--ink-faint'), dim = color('--ink-dim'), line = color('--line')
  const wg = color('--accent'), tls = color('--green'), ink = color('--ink')
  const cy = H * 0.46, Ax = 64, Bx = W - 64
  const span = Bx - Ax
  const OH = Math.min(64, H * 0.20), IH = OH * 0.52

  // outer tunnel (WireGuard) + inner tunnel (mTLS), concentric capsules
  const dash = -now / 24
  for (const [hh, col, lw] of [[OH, wg, 2.4], [IH, tls, 2]]) {
    c.fillStyle = col; c.globalAlpha = 0.06; rr(c, Ax, cy - hh, span, hh * 2, hh); c.fill(); c.globalAlpha = 1
    c.strokeStyle = col; c.lineWidth = lw; c.globalAlpha = 0.85; c.setLineDash([6, 6]); c.lineDashOffset = dash
    rr(c, Ax, cy - hh, span, hh * 2, hh); c.stroke(); c.setLineDash([]); c.globalAlpha = 1
  }

  // a double-wrapped packet travels A→B, wrapping at A and unwrapping at B
  const ph = (now % CYCLE) / CYCLE
  const u = ease(Math.min(1, Math.max(0, (ph - 0.08) / 0.84)))
  const px = lerp(Ax + 14, Bx - 14, u)
  // payload capsule
  c.fillStyle = ink; c.globalAlpha = 0.9; rr(c, px - 16, cy - 8, 32, 16, 4); c.fill(); c.globalAlpha = 1
  c.fillStyle = color('--bg') || '#000'; c.font = '8px ui-monospace, monospace'; c.textAlign = 'center'; c.fillText('data', px, cy + 3)
  // two concentric lock rings riding with it (inner mTLS, outer WG)
  padlock(c, px, cy - IH - 4, 5, tls)
  padlock(c, px, cy - OH - 4, 5, wg)

  // endpoints
  peer(c, Ax, cy, 'Peer A', wg, faint)
  peer(c, Bx, cy, 'Peer B', wg, faint)

  // labels
  c.textAlign = 'left'
  c.fillStyle = wg; c.font = '700 11px ui-sans-serif, system-ui'; c.fillText('① WireGuard tunnel', Ax + 2, cy - OH - 16)
  c.fillStyle = faint; c.font = '10px ui-sans-serif, system-ui'; c.fillText('network layer — encrypts everything on the wire', Ax + 2, cy - OH - 3)
  c.textAlign = 'center'
  c.fillStyle = tls; c.font = '700 11px ui-sans-serif, system-ui'; c.fillText('② mTLS tunnel (inside Zenoh)', W / 2, cy + OH + 22)
  c.fillStyle = faint; c.font = '10px ui-sans-serif, system-ui'; c.fillText('application layer — encrypts AND mutually authenticates', W / 2, cy + OH + 37)

  c.fillStyle = dim; c.font = '10px ui-sans-serif, system-ui'; c.textAlign = 'center'
  c.fillText('two independent layers · different origin & algorithm (swappable, incl. post-quantum) · break one, the other still holds', W / 2, H - 12)
}

// ── sequence over time ───────────────────────────────────────────────────────
function drawSequence(c, now) {
  const faint = color('--ink-faint'), dim = color('--ink-dim'), line = color('--line')
  const wg = color('--accent'), tls = color('--green')
  const Ax = 90, Bx = W - 90, topY = 56, botY = H - 30
  // lifelines
  c.strokeStyle = line; c.lineWidth = 1.4
  for (const x of [Ax, Bx]) { c.beginPath(); c.moveTo(x, topY); c.lineTo(x, botY); c.stroke() }
  c.fillStyle = wg; c.shadowColor = wg; c.shadowBlur = 7
  c.beginPath(); c.arc(Ax, topY - 4, 7, 0, Math.PI * 2); c.fill(); c.beginPath(); c.arc(Bx, topY - 4, 7, 0, Math.PI * 2); c.fill(); c.shadowBlur = 0
  c.fillStyle = faint; c.font = '700 11px ui-monospace, monospace'; c.textAlign = 'center'
  c.fillText('Peer A', Ax, topY - 16); c.fillText('Peer B', Bx, topY - 16)

  const steps = [
    { t: 0.10, dir: 1, label: 'WireGuard handshake (Noise)', sub: 'outer tunnel up — encrypted on the wire', col: () => wg, lock: true },
    { t: 0.34, dir: 2, label: 'mTLS handshake — both present certs', sub: 'inner tunnel up — encrypted + mutually authenticated', col: () => tls, lock: true },
    { t: 0.58, dir: 1, label: 'encrypted request', sub: 'wrapped twice: mTLS inside WireGuard', col: () => tls, lock: true, dub: true },
    { t: 0.80, dir: 2, label: 'encrypted reply', sub: 'ephemeral keys — rotate, then tear down', col: () => wg, lock: true, dub: true },
  ]
  const ph = (now % CYCLE) / CYCLE
  const rowY = (i) => lerp(topY + 34, botY - 26, i / (steps.length - 1))

  steps.forEach((st, i) => {
    const appear = ease(Math.min(1, Math.max(0, (ph - st.t) / 0.12)))
    if (appear <= 0) return
    const y = rowY(i)
    const x0 = st.dir === 1 ? Ax : Bx, x1 = st.dir === 1 ? Bx : Ax
    const tipX = lerp(x0, x1, appear)
    c.strokeStyle = st.col(); c.globalAlpha = 0.5 + 0.5 * appear; c.lineWidth = 2
    c.beginPath(); c.moveTo(x0, y); c.lineTo(tipX, y); c.stroke()
    // arrowhead
    const dir = Math.sign(x1 - x0)
    c.beginPath(); c.moveTo(tipX, y); c.lineTo(tipX - dir * 7, y - 4); c.lineTo(tipX - dir * 7, y + 4); c.closePath(); c.fillStyle = st.col(); c.fill()
    c.globalAlpha = 1
    // lock(s) at midpoint
    const mx = (Ax + Bx) / 2
    if (st.lock) { if (st.dub) { padlock(c, mx - 8, y - 11, 4.5, wg); padlock(c, mx + 8, y - 11, 4.5, tls) } else padlock(c, mx, y - 11, 5, st.col()) }
    // labels
    c.fillStyle = color('--ink'); c.font = '700 11px ui-sans-serif, system-ui'; c.textAlign = 'center'; c.globalAlpha = appear
    c.fillText(st.label, mx, y - 18)
    c.fillStyle = faint; c.font = '9.5px ui-sans-serif, system-ui'; c.fillText(st.sub, mx, y + 16); c.globalAlpha = 1
  })

  c.fillStyle = dim; c.font = '10px ui-sans-serif, system-ui'; c.textAlign = 'center'
  c.fillText('two layers, two key sets, two algorithms — independent and swappable', W / 2, H - 10)
}

function toggle() { if (playing) { pausedAt = performance.now(); playing = false } else { pausedAcc += performance.now() - pausedAt; playing = true } paint() }
function restart() { t0 = performance.now(); pausedAcc = 0; playing = true; paint() }

const canvas = h('canvas', { class: 'viz', style: { height: '360px' } })
const playBtn = h('button', { class: 'sm', onclick: toggle })
root.classList.add('enc-viz')
root.append(canvas, h('div', { class: 'viz-controls' }, playBtn,
  h('button', { class: 'sm ghost', onclick: restart }, '↺'), h('span', { class: 'viz-cap' }, caption)))
function paint() { setText(playBtn, playing ? '❙❙ pause' : '▶ play') }
paint()
resize(); window.addEventListener('resize', resize); t0 = performance.now(); raf = requestAnimationFrame(draw)
}
