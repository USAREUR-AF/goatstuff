// SPDX-License-Identifier: Apache-2.0
// "Limit the blast radius." Two side-by-side illustrations of the SAME incident
// (synced to a shared clock) — a flat network where one slip spreads to
// everything, and a zero-trust fabric where the very same slip is contained to
// one room. Almost every incident here is human-rooted (a phished login, a
// config blunder); the point isn't "we stop every mistake" — it's "when one
// happens, it goes nowhere." Rendered one world per mountBlastRadius() call so
// each has room to breathe (the two-in-one version was too cramped).
import { h, setText } from './common.js'

// mountBlastRadius(root, 'flat' | 'zero') builds the canvas + controls inside root.
export function mountBlastRadius(root, worldArg) {
const world = worldArg ?? 'zero'
const isFlat = world === 'flat'
let playing = true
let ctx = null
let raf = 0, W = 760, H = 380

const INCIDENTS = [
  { id: 'phish', label: 'A phished operator login', src: 'human', cause: 'social engineering — someone clicked a convincing fake', catch: 'short-lived credential + every action re-proven', radius: 'one session — and it expires on its own', origin: 9 },
  { id: 'open', label: 'A service left open by mistake', src: 'human', cause: 'a config blunder — exposed with no auth', catch: 'default-deny: no identity, no access', radius: 'nothing — the door opens onto a wall', origin: 3 },
  { id: 'leak', label: 'A leaked machine certificate', src: 'human', cause: 'a key copied to the wrong place', catch: 'narrowly scoped cert, cut at the mesh', radius: "one machine's lane — nothing else", origin: 14 },
  { id: 'buggy', label: 'A buggy service flooding', src: 'system', cause: 'a bad deploy, not malice', catch: 'attributable to its identity — monitored, then its credential revoked to cut it off; and it only ever reaches the one room it has a link to', radius: 'one room — everyone else keeps working', origin: 7 },
  { id: 'proc', label: 'A compromised workload', src: 'system', cause: 'a dependency went bad', catch: 'its passport (SVID) expires in ~60s, scoped tight', radius: 'one workload, briefly — then it lapses', origin: 18 },
]

const COLS = 6, ROWS = 4, N = COLS * ROWS
const NODES = Array.from({ length: N }, (_, i) => {
  const cx = i % COLS, cy = Math.floor(i / COLS)
  return { gx: 0.10 + (cx / (COLS - 1)) * 0.80, gy: 0.16 + (cy / (ROWS - 1)) * 0.68 }
})

function color(n) { const c = getComputedStyle(document.documentElement).getPropertyValue(n).trim(); return c || '#888' }
function lerp(a, b, u) { return a + (b - a) * Math.max(0, Math.min(1, u)) }
function ease(u) { return u < 0.5 ? 2 * u * u : 1 - Math.pow(-2 * u + 2, 2) / 2 }
function resize() { const c = canvas; W = c.clientWidth; H = c.clientHeight; const d = window.devicePixelRatio || 1; c.width = W * d; c.height = H * d; ctx = c.getContext('2d'); ctx?.scale(d, d) }

const CYCLE = 8200
function draw() {
  if (!playing) return
  raf = requestAnimationFrame(draw)
  if (!ctx) return
  const c = ctx, now = performance.now()  // shared wall clock → both worlds show the same incident
  c.clearRect(0, 0, W, H)
  const pad = 16
  const line = color('--line'), soft = color('--line-soft'), faint = color('--ink-faint')
  const ink = color('--ink'), dim = color('--ink-dim')
  const green = color('--green'), pink = color('--pink'), amber = color('--amber'), accent = color('--accent')

  const incident = INCIDENTS[Math.floor(now / CYCLE) % INCIDENTS.length]
  const ph = (now % CYCLE) / CYCLE
  const spark = ease(Math.min(1, ph / 0.18))
  const spread = ease(Math.min(1, Math.max(0, (ph - 0.18) / 0.28)))
  const contain = ease(Math.min(1, Math.max(0, (ph - 0.46) / 0.26)))

  // header — incident + where it came from
  c.textAlign = 'left'
  c.fillStyle = isFlat ? dim : accent; c.font = '600 11px ui-sans-serif, system-ui'
  c.fillText(isFlat ? 'FLAT NETWORK — one wall, then free run' : 'ZERO TRUST — sealed rooms, single-use links', pad, 16)
  c.textAlign = 'right'
  c.fillStyle = incident.src === 'human' ? amber : accent; c.font = '700 9.5px ui-monospace, monospace'
  c.fillText(incident.src === 'human' ? '◆ HUMAN-ROOTED' : '◆ SYSTEM FAULT', W - pad, 16)
  c.textAlign = 'left'; c.fillStyle = ink; c.font = '700 14px ui-sans-serif, system-ui'
  c.fillText(incident.label, pad, 34)

  // arena
  const arena = { x: pad, y: 46, w: W - pad * 2, h: H - 46 - 52 }
  const pts = NODES.map((n) => ({ x: arena.x + n.gx * arena.w, y: arena.y + n.gy * arena.h }))
  const ox = pts[incident.origin].x, oy = pts[incident.origin].y
  const r = Math.min(15, arena.w / 22)
  const maxD = Math.hypot(arena.w, arena.h)

  // flat arena keeps its lateral mesh lines; zero trust has none
  if (isFlat) {
    c.strokeStyle = soft; c.lineWidth = 1
    for (let i = 0; i < N; i++) {
      const cx = i % COLS, cy = Math.floor(i / COLS)
      if (cx < COLS - 1) { c.beginPath(); c.moveTo(pts[i].x, pts[i].y); c.lineTo(pts[i + 1].x, pts[i + 1].y); c.stroke() }
      if (cy < ROWS - 1) { c.beginPath(); c.moveTo(pts[i].x, pts[i].y); c.lineTo(pts[i + COLS].x, pts[i + COLS].y); c.stroke() }
    }
  }

  pts.forEach((pt, i) => {
    const d = Math.hypot(pt.x - ox, pt.y - oy)
    let stroke = line, fill = soft, a = 0.5
    if (isFlat) {
      const reach = Math.max(0, Math.min(1, (spread * 1.15 - d / maxD) / 0.22))
      if (i === incident.origin) { stroke = pink; fill = pink; a = 0.28 + 0.45 * spark }
      else if (reach > 0.02) { stroke = pink; fill = pink; a = 0.14 + 0.34 * reach }
    } else {
      if (i === incident.origin) { const hot = Math.max(spark, 1 - contain); stroke = mix(pink, amber, contain); fill = mix(pink, amber, contain); a = 0.22 + 0.4 * hot }
      else { stroke = green; fill = green; a = 0.10 + 0.12 * contain }
    }
    c.fillStyle = fill; c.globalAlpha = a; rr(c, pt.x - r, pt.y - r * 0.8, r * 2, r * 1.6, 4); c.fill(); c.globalAlpha = 1
    c.strokeStyle = stroke; c.lineWidth = 1.4; rr(c, pt.x - r, pt.y - r * 0.8, r * 2, r * 1.6, 4); c.stroke()
  })

  // origin marker + (zero) containment ring
  c.fillStyle = pink; c.shadowColor = pink; c.shadowBlur = 12 * spark; c.beginPath(); c.arc(ox, oy, 5, 0, Math.PI * 2); c.fill(); c.shadowBlur = 0
  if (!isFlat && contain > 0.02) {
    c.strokeStyle = green; c.globalAlpha = Math.min(1, contain) * (0.6 + 0.4 * Math.sin(performance.now() / 240))
    c.lineWidth = 2.6; c.setLineDash([5, 4]); c.lineDashOffset = -performance.now() / 26
    c.beginPath(); c.arc(ox, oy, r * 2.3, 0, Math.PI * 2); c.stroke(); c.setLineDash([]); c.globalAlpha = 1
  }

  // verdict + the one line that matters for this world
  const vy = H - 40
  if (isFlat) {
    c.globalAlpha = spread; c.fillStyle = pink; c.font = '700 13px ui-sans-serif, system-ui'; c.textAlign = 'center'
    c.fillText('spreads to everything', W / 2, vy); c.globalAlpha = 1
    c.fillStyle = faint; c.font = '11px ui-sans-serif, system-ui'
    c.fillText('one flat trust zone — nothing stops it moving sideways', W / 2, vy + 18)
  } else {
    c.globalAlpha = Math.max(0.25, contain); c.fillStyle = contain > 0.3 ? green : dim; c.font = '700 13px ui-sans-serif, system-ui'; c.textAlign = 'center'
    c.fillText('contained — it goes nowhere', W / 2, vy); c.globalAlpha = 1
    c.fillStyle = faint; c.font = '11px ui-sans-serif, system-ui'
    c.fillText(`contained by ${incident.catch}`, W / 2, vy + 18)
  }
}

function mix(a, b, u) { const pa = parse(a), pb = parse(b); if (!pa || !pb) return a; return `rgb(${Math.round(lerp(pa[0], pb[0], u))},${Math.round(lerp(pa[1], pb[1], u))},${Math.round(lerp(pa[2], pb[2], u))})` }
function parse(s) { s = s.trim(); if (s.startsWith('#')) { const h = s.slice(1); const x = h.length === 3 ? h.split('').map((c) => c + c).join('') : h; return [parseInt(x.slice(0, 2), 16), parseInt(x.slice(2, 4), 16), parseInt(x.slice(4, 6), 16)] } const m = s.match(/rgba?\(([^)]+)\)/); if (m) { const pp = m[1].split(',').map((v) => parseFloat(v)); return [pp[0], pp[1], pp[2]] } return null }
function rr(c, x, y, w, h, r) { c.beginPath(); c.moveTo(x + r, y); c.arcTo(x + w, y, x + w, y + h, r); c.arcTo(x + w, y + h, x, y + h, r); c.arcTo(x, y + h, x, y, r); c.arcTo(x, y, x + w, y, r); c.closePath() }

function toggle() { playing = !playing; if (playing) raf = requestAnimationFrame(draw); paint() }
function restart() { if (!playing) { playing = true; raf = requestAnimationFrame(draw) } paint() }

const canvas = h('canvas', { class: 'viz', style: { height: '380px' } })
const playBtn = h('button', { class: 'sm', onclick: toggle })
root.classList.add('blast-radius')
root.append(canvas, h('div', { class: 'viz-controls' }, playBtn, h('button', { class: 'sm ghost', onclick: restart }, '↺')))
function paint() { setText(playBtn, playing ? '❙❙ pause' : '▶ play') }
paint()
resize(); window.addEventListener('resize', resize); raf = requestAnimationFrame(draw)
}
