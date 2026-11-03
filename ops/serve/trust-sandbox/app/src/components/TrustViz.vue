<script setup lang="ts">
// One world of the "wall / no-inside" story, drawn LARGE and full-width. The
// vision page renders two of these in separate sections (world="castle" and
// world="zero") so each illustration has room to breathe and be expressive —
// the ZTA sibling of the Zenoh sandbox's community canvas (a designed story,
// not live data). Each <TrustViz> is an independent component with its own
// clock, so the two worlds tell their own stories at their own pace.
//
//  • castle: a walled keep with a FRONT GATE and a BACK DOOR. Three scenes
//    cycle — (1) a legitimate actor checks in at the front gate and then has
//    the run of every room; (2) an expired pass walks in anyway because the
//    front door was propped open while the watchman looked away; (3) an
//    intruder slips the unwatched back door and floods the entire keep. The
//    one wall is the only line of defence; once you are inside, you roam.
//  • zero: no inside at all. Every interaction is its OWN relationship —
//    established one at a time, mutually proven (two-way), encrypted, used for
//    a single action, then GONE. No standing access, no lateral movement.
import { onMounted, onBeforeUnmount, ref } from 'vue'
import { useVizControls, VIZ_SPEEDS } from '../composables/vizControls'

const props = defineProps<{ world?: 'castle' | 'zero' }>()
const world = props.world ?? 'castle'
const canvas = ref<HTMLCanvasElement | null>(null)
let ctx: CanvasRenderingContext2D | null = null
let W = 900, H = 460
const caption = ref(world === 'castle'
  ? 'One wall, one gate — and once inside, open hallways to every room.'
  : 'No inside to roam: every action is its own verified, two-way, vanishing link.')

// Castle scenes (cycled in the castle world only). `entry` drives where the
// actor comes in and where any flood radiates from.
const CASTLE = [
  { id: 'ok', tag: 'legit', label: 'a legitimate actor', entry: 'front', mode: 'roam', tone: 'ok', scene: 1, title: 'A legitimate actor checks in',
    castle: 'shows proper credentials, the gate opens — then it has the run of every room' },
  { id: 'propped', tag: 'propped door', label: 'an expired pass', entry: 'propped', mode: 'roam', tone: 'stale', scene: 2, title: 'An expired pass, and a propped door',
    castle: 'the front door was propped open while the watchman looked away — so it walks in and roams anyway' },
  { id: 'intruder', tag: 'intruder', label: 'an intruder', entry: 'back', mode: 'flood', tone: 'flood', scene: 3, title: 'An intruder finds the back door',
    castle: 'discovers an unwatched back door — and spreads through the entire keep' },
]

// Zero-trust scenes (cycled in the zero world only). `rightMode` drives the
// per-link outcome; `ttl` drives the credential-lifetime meter.
const ZERO = [
  { id: 'ok', tag: 'legit', label: 'a legitimate actor', rightMode: 'verified', ttl: 'fresh',
    zero: 'each action is its own two-way, encrypted link — proven, used once, then gone' },
  { id: 'stale', tag: 'stale cred', label: 'a stale credential', rightMode: 'refused', ttl: 'expired',
    zero: 'its short-lived credential already expired — no link will form' },
  { id: 'noisy', tag: 'flooder', label: 'a flooding service', rightMode: 'contained', ttl: 'fresh',
    zero: 'every byte is bound to its identity, so the flood is attributable — we watch it cross the line and revoke that credential to shut it off; even unrevoked it reaches only the one node it has a link to' },
  { id: 'forged', tag: 'forged', label: 'a forged identity', rightMode: 'refused', ttl: 'none',
    zero: 'no trusted identity, so the two-way handshake never completes' },
  { id: 'wrong', tag: 'wrong lane', label: 'a misconfigured write (wrong lane)', rightMode: 'refused', ttl: 'fresh',
    zero: 'a valid identity, but outside its lane — refused at that one node' },
]

// deterministic pseudo-random (no Math.random — keeps the render stable + replay-safe)
function h1(i: number) { const x = Math.sin(i * 127.1 + 311.7) * 43758.5453; return x - Math.floor(x) }
// THE OLD WAY's interior is an "ocean": a dense field of services + data stores,
// jittered on a grid so it reads rich and organic. The whole point is that once
// you're past the wall, ALL of this is reachable — nothing here protects itself.
const FC = 12, FR = 8
const FIELD = Array.from({ length: FC * FR }, (_, i) => {
  const c = i % FC, r = Math.floor(i / FC)
  const gx = (c + 0.5) / FC + (h1(i) * 2 - 1) * 0.44 / FC
  const gy = (r + 0.5) / FR + (h1(i + 99) * 2 - 1) * 0.44 / FR
  return { gx: 0.04 + gx * 0.92, gy: 0.06 + gy * 0.88, rad: 0.5 + h1(i + 7) * 1.15, depth: 0.38 + h1(i + 33) * 0.62 }
})
const WEB: [number, number][] = []
for (let r = 0; r < FR; r++) for (let c = 0; c < FC; c++) {
  const i = r * FC + c
  if (c < FC - 1) WEB.push([i, i + 1])
  if (r < FR - 1) WEB.push([i, i + FC])
  if (c < FC - 1 && r < FR - 1 && (c + r) % 2 === 0) WEB.push([i, i + FC + 1])
  if (c > 0 && r < FR - 1 && (c + r) % 3 === 0) WEB.push([i, i + FC - 1])
}
// ZERO TRUST reuses the SAME ocean, but with no standing connections — the actor
// only ever reaches the handful it has links to (~15% of the field), one at a time.
const ZERO_TOUCH = FIELD.map((_, i) => i).filter((i) => h1(i + 5) < 0.16)

function color(n: string): string { const c = getComputedStyle(document.documentElement).getPropertyValue(n).trim(); return c || '#888' }
function lerp(a: number, b: number, u: number) { return a + (b - a) * Math.max(0, Math.min(1, u)) }
function ease(u: number) { return u < 0.5 ? 2 * u * u : 1 - Math.pow(-2 * u + 2, 2) / 2 }
function resize() { const c = canvas.value!; W = c.clientWidth; H = c.clientHeight; const d = window.devicePixelRatio || 1; c.width = W * d; c.height = H * d; ctx = c.getContext('2d'); ctx?.scale(d, d) }

const CYCLE = 9000
const SCENES = world === 'castle' ? CASTLE : ZERO
const sceneTags = (SCENES as Array<{ tag: string }>).map((s) => s.tag)
interface Box { x: number; y: number; w: number; h: number }

function render(now: number) {
  if (!ctx) return
  ctx.clearRect(0, 0, W, H)
  const pad = 18, titleH = 26
  const box: Box = { x: pad + (world === 'castle' ? 8 : 6), y: titleH, w: W - pad * 2 - 16, h: H - titleH - 32 }

  ctx.font = '600 12px ui-sans-serif, system-ui, sans-serif'; ctx.textAlign = 'left'
  if (world === 'castle') { ctx.fillStyle = color('--ink-dim'); ctx.fillText('TRUST BY LOCATION — the walled keep', pad, 16) }
  else { ctx.fillStyle = color('--accent'); ctx.fillText('ZERO TRUST — no inside (one link at a time)', pad, 16) }
  ctx.fillStyle = color('--ink-faint'); ctx.font = '10px ui-monospace, monospace'
  ctx.fillText(world === 'castle' ? 'one wall · then free run (lateral movement everywhere)' : 'each action re-proven · two-way · encrypted · ephemeral · blast radius = one node', pad, H - 8)

  const idx = sceneIndex()
  const cphase = (now % CYCLE) / CYCLE
  if (world === 'castle') {
    const p = CASTLE[idx]; drawCastle(box, p, cphase)
    caption.value = cphase < 0.17 ? `Scene ${p.scene} — ${p.title}.` : `${cap(p.label)} — ${p.castle}.`
  } else {
    const p = ZERO[idx]
    const accessU = ease(Math.min(1, Math.max(0, (cphase - 0.04) / 0.56)))
    const outcomeU = ease(Math.min(1, Math.max(0, (cphase - 0.60) / 0.30)))
    drawZero(box, p, accessU, outcomeU)
    caption.value = `${cap(p.label)} — ${p.zero}.`
  }
}
function cap(s: string) { return s.charAt(0).toUpperCase() + s.slice(1) }
function scaleOf(b: Box) { return Math.min(1.9, Math.max(1, b.w / 470)) }

// ── THE OLD WAY: a walled keep, one scene at a time ──────────────────────────
// Each vignette: (1) a clear title card orients the viewer; (2) the keep fades
// in; (3) ONLY the entrance in use is revealed — the front gate opens on proper
// credentials, or the back door is hidden until an intruder finds it unguarded.
function drawCastle(b: Box, p: typeof CASTLE[number] | null, cphase: number) {
  const c = ctx!, s = scaleOf(b)
  const line = color('--line'), soft = color('--line-soft'), faint = color('--ink-faint')
  const green = color('--green'), pink = color('--pink'), amber = color('--amber'), ink = color('--ink')
  const entry = p?.entry ?? 'front', mode = p?.mode ?? 'roam', tone = p?.tone ?? 'ok'
  const back = entry === 'back', propped = entry === 'propped'

  // 1) clear screen + orienting title card — centered, on an OPAQUE backing panel
  //    so the lettering never collides with (or matches the color of) the field.
  if (cphase < 0.17) {
    const al = Math.max(0, Math.min(1, cphase / 0.05) * Math.min(1, (0.17 - cphase) / 0.045))
    const cx = b.x + b.w / 2, cy = b.y + b.h * 0.5
    const eyebrow = `SCENE ${p?.scene ?? 1} OF 3`, ttl = p?.title ?? ''
    c.textAlign = 'center'
    c.font = `700 ${Math.min(22, 18 * s)}px ui-sans-serif, system-ui`
    const boxW = Math.max(c.measureText(ttl).width, 140 * s) + 48 * s, boxH = 62 * s
    c.globalAlpha = al
    c.fillStyle = color('--panel-raise'); rr(c, cx - boxW / 2, cy - boxH / 2, boxW, boxH, 11 * s); c.fill()
    c.globalAlpha = al * 0.55; c.strokeStyle = color('--accent'); c.lineWidth = 1.5; rr(c, cx - boxW / 2, cy - boxH / 2, boxW, boxH, 11 * s); c.stroke()
    c.globalAlpha = al
    c.fillStyle = faint; c.font = `700 ${11 * s}px ui-monospace, monospace`; c.fillText(eyebrow, cx, cy - 6 * s)
    c.fillStyle = ink; c.font = `700 ${Math.min(22, 18 * s)}px ui-sans-serif, system-ui`; c.fillText(ttl, cx, cy + 17 * s)
    c.globalAlpha = 1; return
  }

  const revealU = ease(Math.min(1, Math.max(0, (cphase - 0.18) / 0.12)))
  const accessU = ease(Math.min(1, Math.max(0, (cphase - 0.32) / 0.34)))
  const outcomeU = ease(Math.min(1, Math.max(0, (cphase - 0.64) / 0.30)))

  const gateY = b.y + b.h * 0.5, gap = 24 * s
  const leftX = b.x, rightX = b.x + b.w, topY = b.y, botY = b.y + b.h
  const entryX = back ? rightX : leftX
  const roaming = mode === 'roam'
  // back door stays hidden until the intruder reaches it; front gate opens on creds
  const foundU = back ? ease(Math.min(1, Math.max(0, (accessU - 0.05) / 0.40))) : 0
  const openU = propped ? 0.6 : ease(Math.min(1, Math.max(0, (accessU - 0.08) / 0.34)))

  // the interior "ocean" of services + data — all of it reachable once inside
  const fp = FIELD.map((n) => ({ x: b.x + n.gx * b.w, y: b.y + n.gy * b.h, rad: n.rad, depth: n.depth }))
  const dists = fp.map((p) => Math.hypot(p.x - entryX, p.y - gateY))
  const dmin = Math.min(...dists), dmax = Math.max(...dists)
  const distN = dists.map((d) => (d - dmin) / (dmax - dmin || 1))
  const prog = roaming ? accessU : outcomeU
  const reachAt = (i: number) => Math.max(0, Math.min(1, (prog * 1.3 - distN[i]) / 0.26))
  const wash = mode === 'flood' ? pink : (tone === 'stale' ? amber : green)

  c.globalAlpha = revealU  // whole keep fades in after the title card

  // a soft interior wash so the field reads as depth, not a flat grid
  const interiorPad = 8 * s
  const grad = c.createLinearGradient(0, topY, 0, botY)
  grad.addColorStop(0, withA(soft, 0.10)); grad.addColorStop(1, withA(soft, 0.02))
  c.fillStyle = grad; c.globalAlpha = revealU
  rr(c, leftX + interiorPad, topY + interiorPad, rightX - leftX - interiorPad * 2, botY - topY - interiorPad * 2, 10 * s); c.fill()

  // the lateral-pathway web — faint base, washes as the breach/roam spreads
  for (const [i, j] of WEB) {
    const a = fp[i], bn = fp[j]
    c.strokeStyle = soft; c.globalAlpha = revealU * 0.20 * Math.min(a.depth, bn.depth); c.lineWidth = 1
    c.beginPath(); c.moveTo(a.x, a.y); c.lineTo(bn.x, bn.y); c.stroke()
    const rch = Math.min(reachAt(i), reachAt(j))
    if (rch > 0.02) { c.strokeStyle = wash; c.globalAlpha = revealU * 0.5 * rch; c.lineWidth = 1.5 * s; c.beginPath(); c.moveTo(a.x, a.y); c.lineTo(bn.x, bn.y); c.stroke() }
  }
  c.globalAlpha = revealU

  // the ocean of nodes — varied size + depth; the wash sweeps over ALL of them
  fp.forEach((p, idx) => {
    const baseR = (1.5 + p.rad * 2.6) * s, rch = reachAt(idx)
    c.fillStyle = soft; c.globalAlpha = revealU * (0.22 + 0.4 * p.depth)
    c.beginPath(); c.arc(p.x, p.y, baseR, 0, Math.PI * 2); c.fill()
    if (rch > 0.02) {
      c.fillStyle = wash; c.shadowColor = wash; c.shadowBlur = 7 * rch; c.globalAlpha = revealU * (0.3 + 0.6 * rch)
      c.beginPath(); c.arc(p.x, p.y, baseR + 0.7 * s, 0, Math.PI * 2); c.fill(); c.shadowBlur = 0
    }
  })
  c.globalAlpha = revealU

  // perimeter — top + bottom always; the sides depend on which entrance is in use
  c.strokeStyle = line; c.lineWidth = 5 * s; c.lineCap = 'round'; c.lineJoin = 'round'
  c.beginPath(); c.moveTo(leftX, topY); c.lineTo(rightX, topY); c.stroke()
  c.beginPath(); c.moveTo(leftX, botY); c.lineTo(rightX, botY); c.stroke()
  if (back) {
    c.beginPath(); c.moveTo(leftX, topY); c.lineTo(leftX, botY); c.stroke()  // left solid — no front gate this scene
    const oh = gap * foundU  // right wall opens its hidden gap once found
    c.beginPath(); c.moveTo(rightX, topY); c.lineTo(rightX, gateY - oh); c.stroke()
    c.beginPath(); c.moveTo(rightX, gateY + oh); c.lineTo(rightX, botY); c.stroke()
  } else {
    c.beginPath(); c.moveTo(rightX, topY); c.lineTo(rightX, botY); c.stroke()  // right solid — no back door this scene
    c.beginPath(); c.moveTo(leftX, topY); c.lineTo(leftX, gateY - gap); c.stroke()
    c.beginPath(); c.moveTo(leftX, gateY + gap); c.lineTo(leftX, botY); c.stroke()
  }
  c.lineWidth = 1; c.lineCap = 'butt'

  // the front gate: a leaf hinged at the top of the gap, swinging inward
  if (!back) {
    const hx = leftX, hy = gateY - gap, len = 2 * gap, th = openU * 1.15  // 0 = flush/closed, larger = swung in
    const ex = hx + len * Math.sin(th), ey = hy + len * Math.cos(th)
    c.strokeStyle = propped ? amber : green; c.lineWidth = 3 * s; c.lineCap = 'round'
    c.beginPath(); c.moveTo(hx, hy); c.lineTo(ex, ey); c.stroke(); c.lineCap = 'butt'
    c.textAlign = 'left'; c.font = `${8.5 * s}px ui-monospace, monospace`
    if (propped) {
      c.fillStyle = amber; c.beginPath(); c.moveTo(leftX - 2, gateY + gap); c.lineTo(leftX + 9 * s, gateY + gap); c.lineTo(leftX - 2, gateY + gap - 8 * s); c.closePath(); c.fill()  // wedge
      c.fillText('propped open · watchman away', leftX + 12 * s, gateY + gap + 13 * s)
    } else {
      c.fillStyle = openU > 0.15 ? green : faint
      c.fillText(openU > 0.15 ? '✓ credentials verified — gate opens' : 'FRONT GATE', leftX + 8 * s, gateY - gap - 6 * s)
    }
  } else if (foundU > 0.25) {
    c.textAlign = 'right'; c.font = `${8.5 * s}px ui-monospace, monospace`; c.fillStyle = pink
    c.fillText('back door — found · unguarded', rightX - 8 * s, gateY - gap - 6 * s)
  }

  // the actor, entering through whichever opening this scene uses
  const fromX = back ? rightX + 16 * s : leftX - 16 * s
  const toX = back ? rightX - 12 * s : leftX + 12 * s
  const cross = back ? foundU : openU  // don't cross the wall until the way is actually open
  const enterX = lerp(fromX, toX, ease(Math.min(1, accessU * 1.6)) * Math.max(0.18, cross))
  const dot = mode === 'flood' ? pink : (tone === 'stale' ? amber : green)
  c.shadowColor = dot; c.shadowBlur = 12; c.fillStyle = dot; c.beginPath(); c.arc(enterX, gateY, 6 * s, 0, Math.PI * 2); c.fill(); c.shadowBlur = 0

  c.globalAlpha = 1
}

// ── ZERO TRUST: one ephemeral, two-way, encrypted link at a time ─────────────
function drawZero(b: Box, p: typeof ZERO[number] | null, accessU: number, outcomeU: number) {
  const c = ctx!, s = scaleOf(b)
  const line = color('--line'), soft = color('--line-soft'), faint = color('--ink-faint')
  const accent = color('--accent'), green = color('--green'), pink = color('--pink'), amber = color('--amber')
  const originX = b.x + 6 * s, originY = b.y + b.h * 0.5

  // there is no inside: only sealed rooms, no standing connections — nothing to dissolve
  const fp = FIELD.map((n) => ({ x: b.x + n.gx * b.w, y: b.y + n.gy * b.h, rad: n.rad, depth: n.depth }))

  const mode = p?.rightMode ?? 'verified'
  // deterministic targets from the field: nearest-centre for a contained reach, a far node for a wrong-lane write
  const cx = b.x + b.w * 0.55, cy = b.y + b.h * 0.5
  let ZT = 0, far = 0, dmin = 1e9, dmax = -1
  fp.forEach((q, i) => { const d = Math.hypot(q.x - cx, q.y - cy); if (d < dmin) { dmin = d; ZT = i } if (d > dmax) { dmax = d; far = i } })
  const target = (p?.id === 'wrong') ? far : ZT

  // legit actor: ephemeral links to ~15% of the field, ONE at a time — never all
  let activeIdx = -1, local = 0; const served = new Set<number>()
  if (mode === 'verified') {
    const f = accessU * ZERO_TOUCH.length, k = Math.min(ZERO_TOUCH.length - 1, Math.floor(f))
    local = f - k; activeIdx = ZERO_TOUCH[k]
    for (let m = 0; m < k; m++) served.add(ZERO_TOUCH[m])
  }

  // the nodes — sealed by default; only the few the actor links to ever light up
  fp.forEach((q, idx) => {
    const baseR = (1.5 + q.rad * 2.6) * s
    c.fillStyle = soft; c.globalAlpha = 0.22 + 0.4 * q.depth; c.beginPath(); c.arc(q.x, q.y, baseR, 0, Math.PI * 2); c.fill(); c.globalAlpha = 1
    if (mode === 'verified' && idx === activeIdx) {
      const inten = Math.sin(Math.min(1, local) * Math.PI)
      c.fillStyle = green; c.shadowColor = green; c.shadowBlur = 8 * inten; c.globalAlpha = 0.3 + 0.55 * inten
      c.beginPath(); c.arc(q.x, q.y, baseR + 0.8 * s, 0, Math.PI * 2); c.fill(); c.shadowBlur = 0; c.globalAlpha = 1
    } else if (served.has(idx)) {
      c.strokeStyle = green; c.globalAlpha = 0.35; c.lineWidth = 1.3 * s; c.beginPath(); c.arc(q.x, q.y, baseR + 1.7 * s, 0, Math.PI * 2); c.stroke(); c.globalAlpha = 1
    }
    if (mode === 'contained' && idx === target) { const f = Math.max(0, Math.min(1, accessU)); c.fillStyle = pink; c.shadowColor = pink; c.shadowBlur = 8 * f; c.globalAlpha = 0.25 + 0.5 * f; c.beginPath(); c.arc(q.x, q.y, baseR + 1 * s, 0, Math.PI * 2); c.fill(); c.shadowBlur = 0; c.globalAlpha = 1 }
    if (mode === 'refused' && idx === target && outcomeU > 0.05) { c.fillStyle = pink; c.globalAlpha = 0.14 + 0.2 * outcomeU; c.beginPath(); c.arc(q.x, q.y, baseR + 0.8 * s, 0, Math.PI * 2); c.fill(); c.globalAlpha = 1 }
  })

  // the actor (origin) + its credential-lifetime meter
  c.fillStyle = accent; c.shadowColor = accent; c.shadowBlur = 10; c.beginPath(); c.arc(originX, originY, 6 * s, 0, Math.PI * 2); c.fill(); c.shadowBlur = 0
  if (p) {
    const mx = originX - 2, my = originY + 16 * s, mw = 52 * s
    c.fillStyle = faint; c.font = `${8 * s}px ui-monospace, monospace`; c.textAlign = 'left'; c.fillText('credential lifetime (TTL)', mx, my - 4)
    c.strokeStyle = line; c.lineWidth = 1; c.strokeRect(mx, my, mw, 4 * s)
    let frac, col
    if (p.ttl === 'expired') { frac = 0; col = pink } else if (p.ttl === 'none') { frac = 0; col = faint } else { frac = 1 - accessU * 0.7; col = frac < 0.25 ? amber : green }
    c.fillStyle = col; c.fillRect(mx, my, mw * Math.max(0, frac), 4 * s)
  }

  if (mode === 'verified' && activeIdx >= 0) {
    drawEphemeralLink(fp[activeIdx], originX, originY, local, accent, green, faint, s)
  } else if (mode === 'contained') {
    // a NOISY/flooding actor: one authorized link, and the flood CAPS at that one
    // room — expanding rings stop at a containment boundary; neighbours stay dark.
    const r = fp[target], baseR = (1.5 + fp[target].rad * 2.6) * s, cap = baseR + 17 * s
    c.strokeStyle = amber; c.globalAlpha = 0.5 + 0.4 * Math.sin(Math.min(1, accessU) * Math.PI); c.lineWidth = 2.2 * s
    c.setLineDash([4, 4]); c.lineDashOffset = -performance.now() / 30; c.beginPath(); c.moveTo(originX, originY); c.lineTo(r.x, r.y); c.stroke(); c.setLineDash([]); c.globalAlpha = 1
    const grow = (performance.now() / 700) % 1, ring = baseR + grow * (cap - baseR)
    c.strokeStyle = pink; c.globalAlpha = (1 - grow) * 0.85; c.lineWidth = 2 * s; c.beginPath(); c.arc(r.x, r.y, ring, 0, Math.PI * 2); c.stroke(); c.globalAlpha = 1
    c.strokeStyle = green; c.globalAlpha = 0.55; c.setLineDash([3, 3]); c.beginPath(); c.arc(r.x, r.y, cap, 0, Math.PI * 2); c.stroke(); c.setLineDash([]); c.globalAlpha = 1
  } else {
    // refused: the handshake reaches out, never completes, then collapses with an ✗
    const r = fp[target], form = ease(Math.min(1, accessU)), collapse = outcomeU
    const tx = lerp(originX, r.x, form * (1 - collapse)), ty = lerp(originY, r.y, form * (1 - collapse))
    c.strokeStyle = mix(amber, pink, collapse); c.globalAlpha = 0.5 + 0.5 * (1 - collapse * 0.6); c.lineWidth = 2.2 * s
    c.setLineDash([4, 4]); c.lineDashOffset = -performance.now() / 30; c.beginPath(); c.moveTo(originX, originY); c.lineTo(tx, ty); c.stroke(); c.setLineDash([]); c.globalAlpha = 1
    if (collapse > 0.1) { c.strokeStyle = pink; c.lineWidth = 2.2 * s; const k = 6 * s; const ex = r.x - 9 * s; c.beginPath(); c.moveTo(ex - k, r.y - k); c.lineTo(ex + k, r.y + k); c.moveTo(ex + k, r.y - k); c.lineTo(ex - k, r.y + k); c.stroke() }
  }
}

// one mediated relationship: form (I prove to you) → verify (you prove back,
// encrypted) → use once → vanish. Bidirectional pulses + a padlock make it
// clear this is not standing access.
function drawEphemeralLink(r: { x: number; y: number }, ox: number, oy: number, local: number, accent: string, green: string, faint: string, s: number) {
  const c = ctx!
  const env = local < 0.15 ? local / 0.15 : local > 0.72 ? Math.max(0, (1 - local) / 0.28) : 1  // fade in/out → "vanishes"
  c.strokeStyle = mix(accent, green, Math.min(1, local * 1.4)); c.globalAlpha = 0.85 * env; c.lineWidth = 2.2 * s
  c.setLineDash([4, 4]); c.lineDashOffset = -performance.now() / 28
  c.beginPath(); c.moveTo(ox, oy); c.lineTo(r.x, r.y); c.stroke(); c.setLineDash([]); c.globalAlpha = 1
  // outbound pulse (request, I prove to you)
  if (local < 0.5) { const u = ease(local / 0.5); const px = lerp(ox, r.x, u), py = lerp(oy, r.y, u); c.fillStyle = accent; c.globalAlpha = env; c.beginPath(); c.arc(px, py, 3.4 * s, 0, Math.PI * 2); c.fill(); c.globalAlpha = 1 }
  // inbound pulse (response, you prove back — two-way)
  else { const u = ease((local - 0.5) / 0.45); const px = lerp(r.x, ox, u), py = lerp(r.y, oy, u); c.fillStyle = green; c.globalAlpha = env; c.beginPath(); c.arc(px, py, 3.4 * s, 0, Math.PI * 2); c.fill(); c.globalAlpha = 1 }
  // encryption padlock at the midpoint once the link is live
  if (local > 0.28 && local < 0.82) { padlock(c, (ox + r.x) / 2, (oy + r.y) / 2, 5 * s, green) }
  // label travels with the active room
  c.fillStyle = faint; c.font = `${8.5 * s}px ui-monospace, monospace`; c.textAlign = 'center'; c.globalAlpha = env
  c.fillText(local < 0.5 ? 'proving…' : local < 0.78 ? 'verified · encrypted' : 'link gone', r.x, r.y - 16 * s); c.globalAlpha = 1
}

function padlock(c: CanvasRenderingContext2D, x: number, y: number, sz: number, col: string) {
  c.save(); c.strokeStyle = col; c.fillStyle = col; c.lineWidth = 1.4
  c.beginPath(); c.arc(x, y - sz * 0.5, sz * 0.55, Math.PI, 0); c.stroke()  // shackle
  c.globalAlpha = 0.85; rr(c, x - sz * 0.7, y - sz * 0.1, sz * 1.4, sz * 1.2, 1.5); c.fill(); c.globalAlpha = 1; c.restore()
}

function mix(a: string, b: string, u: number): string { const pa = parse(a), pb = parse(b); if (!pa || !pb) return a; return `rgb(${Math.round(lerp(pa[0], pb[0], u))},${Math.round(lerp(pa[1], pb[1], u))},${Math.round(lerp(pa[2], pb[2], u))})` }
function withA(col: string, a: number): string { const p = parse(col); return p ? `rgba(${p[0]},${p[1]},${p[2]},${a})` : col }
function parse(s: string): [number, number, number] | null { s = s.trim(); if (s.startsWith('#')) { const h = s.slice(1); const x = h.length === 3 ? h.split('').map((c) => c + c).join('') : h; return [parseInt(x.slice(0, 2), 16), parseInt(x.slice(2, 4), 16), parseInt(x.slice(4, 6), 16)] } const m = s.match(/rgba?\(([^)]+)\)/); if (m) { const p = m[1].split(',').map((v) => parseFloat(v)); return [p[0], p[1], p[2]] } return null }
function rr(c: CanvasRenderingContext2D, x: number, y: number, w: number, h: number, r: number) { c.beginPath(); c.moveTo(x + r, y); c.arcTo(x + w, y, x + w, y + h, r); c.arcTo(x + w, y + h, x, y + h, r); c.arcTo(x, y + h, x, y, r); c.arcTo(x, y, x + w, y, r); c.closePath() }

const { playing, progress, speedIdx, scene, toggle, setSpeed, pickScene, scrubTo, onScrubStart, onScrubEnd, sceneIndex } =
  useVizControls(CYCLE, SCENES.length, render)
onMounted(() => { resize(); window.addEventListener('resize', resize) })
onBeforeUnmount(() => { window.removeEventListener('resize', resize) })
</script>

<template>
  <div>
    <canvas ref="canvas" class="viz" style="height:460px"></canvas>
    <div class="viz-bar">
      <button class="vb-play" @click="toggle" :title="playing ? 'pause' : 'play'">{{ playing ? '❙❙' : '▶' }}</button>
      <input class="scrub" type="range" min="0" max="1" step="0.001" :value="progress"
             @pointerdown="onScrubStart" @pointerup="onScrubEnd"
             @input="scrubTo(+($event.target as HTMLInputElement).value)" aria-label="scrub timeline" />
      <div class="speeds">
        <button v-for="(sp, i) in VIZ_SPEEDS" :key="sp" class="spd" :class="{ on: speedIdx === i }" @click="setSpeed(i)">{{ sp }}×</button>
      </div>
    </div>
    <div class="scenes">
      <span class="sc-lbl">scenario</span>
      <button class="sc" :class="{ on: scene < 0 }" @click="pickScene(-1)">⟳ auto</button>
      <button v-for="(t, i) in sceneTags" :key="t" class="sc" :class="{ on: scene === i }" @click="pickScene(i)">{{ t }}</button>
    </div>
    <div class="viz-cap">{{ caption }}</div>
  </div>
</template>

<style scoped>
.viz-bar { display: flex; align-items: center; gap: 10px; margin-top: 10px; }
.vb-play { flex: none; width: 30px; height: 26px; border: 1px solid var(--line); border-radius: 7px; background: var(--panel-2); color: var(--ink); font-size: 11px; cursor: pointer; }
.vb-play:hover { border-color: var(--accent); color: var(--accent); }
.scrub { flex: 1; height: 5px; -webkit-appearance: none; appearance: none; background: var(--line); border-radius: 999px; cursor: pointer; }
.scrub::-webkit-slider-thumb { -webkit-appearance: none; width: 14px; height: 14px; border-radius: 50%; background: var(--accent); border: 2px solid var(--panel); cursor: grab; }
.scrub::-moz-range-thumb { width: 14px; height: 14px; border: 2px solid var(--panel); border-radius: 50%; background: var(--accent); cursor: grab; }
.speeds { display: inline-flex; flex: none; border: 1px solid var(--line); border-radius: 7px; overflow: hidden; }
.spd { background: var(--panel-2); color: var(--ink-dim); border: none; border-left: 1px solid var(--line); padding: 4px 7px; font-family: var(--font-mono); font-size: 10.5px; cursor: pointer; }
.spd:first-child { border-left: none; }
.spd.on { background: color-mix(in srgb, var(--accent) 22%, transparent); color: var(--accent); }
.scenes { display: flex; align-items: center; gap: 6px; margin-top: 9px; flex-wrap: wrap; }
.sc-lbl { font-family: var(--font-mono); font-size: 9.5px; letter-spacing: .12em; text-transform: uppercase; color: var(--ink-faint); margin-right: 2px; }
.sc { background: var(--panel-2); color: var(--ink-dim); border: 1px solid var(--line); border-radius: 999px; padding: 3px 10px; font-size: 11px; cursor: pointer; transition: border-color .15s, color .15s; }
.sc:hover { border-color: var(--accent); color: var(--ink); }
.sc.on { background: color-mix(in srgb, var(--accent) 16%, transparent); border-color: var(--accent); color: var(--accent); }
.viz-cap { color: var(--ink-dim); font-size: 13px; margin-top: 9px; min-height: 18px; }
</style>
