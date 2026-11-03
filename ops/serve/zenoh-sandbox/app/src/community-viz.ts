/* ---------------------------------------------------------------------------
   community-viz — the animated centerpiece of the "community of opportunity"
   page. A hand-built Canvas 2D scene (no libraries, no icon assets); the whole
   aesthetic is procedural so it stays cohesive with the sandbox's tacticool/
   warm design tokens and re-themes live (dark + light).

   One story, two states, cross-faded by an eased `t` (0 = today, 1 = fabric):

   TODAY — a transit diagram in the Harry Beck / London Underground style. Each
     vendor system is its own colored LINE; stations are stages of generating,
     receiving, and using data (a kill chain, with the human friction made
     literal). To move data across systems you ride one line, stop, GET OFF, and
     WALK a corridor to another line — re-keying, swivel-chairing, exporting by
     email / PDF / USB — and sometimes it's dropped in the corridor. The whole
     thing is interlocked and clunky: rails, cars, passengers, stops.

   FABRIC — leave the legacy network behind. The same capabilities dock onto one
     shared, glowing, brokerless ring; each becomes a focused node doing its one
     thing; value flows peer-to-peer across the shared medium, contribution
     returns as compensation, and the whole community brightens.
--------------------------------------------------------------------------- */

type ColorKey = 'amber' | 'accent' | 'green' | 'ask'

const TAU = Math.PI * 2
const lerp = (a: number, b: number, t: number) => a + (b - a) * t
const ease = (t: number) => (t < 0.5 ? 2 * t * t : 1 - Math.pow(-2 * t + 2, 2) / 2)
const dist = (a: Pt, b: Pt) => Math.hypot(a.x - b.x, a.y - b.y)

interface Pt { x: number; y: number }

/* ===== TODAY: the transit diagram ========================================= */
// Map-space is fixed (designed once) and fit into the canvas with "contain".
const MAPW = 1000
const MAPH = 580
const MPAD = 92
const COLW = (MAPW - 2 * MPAD) / 10 // 11 columns, 0..10
const ROWH = (MAPH - 2 * MPAD) / 5 //  6 rows,   0..5
const mp = (col: number, row: number): Pt => ({ x: MPAD + col * COLW, y: MPAD + row * ROWH })

// Beck-style line colors — vivid, distinct, readable on both cream and night
// backgrounds (echoing Central / Piccadilly / District / Metropolitan / etc.).
interface LineDef {
  key: string
  name: string
  color: string
  stops: Array<[number, number, string, ('term' | 'inter')?]>
}
const LINES: LineDef[] = [
  {
    key: 'collection', name: 'COLLECTION', color: '#e1251b',
    stops: [[0, 1, 'Detect', 'term'], [2, 1, 'Collect'], [4, 1, 'Ingest', 'inter'], [6, 1, 'Catalog'], [8, 1, 'Store', 'inter'], [10, 1, 'Archive', 'term']],
  },
  {
    key: 'processing', name: 'PROCESSING', color: '#1c5bc4',
    stops: [[0, 3, 'Sensor feed', 'term'], [2, 3, 'Decode'], [4, 3, 'Process', 'inter'], [6, 3, 'Exploit'], [8, 3, 'Enrich', 'inter'], [10, 3, 'Index', 'term']],
  },
  {
    key: 'analysis', name: 'ANALYSIS', color: '#1fa463',
    stops: [[0, 5, 'Triage', 'term'], [2, 5, 'Correlate'], [4, 5, 'Fuse', 'inter'], [6, 5, 'Pattern'], [8, 5, 'Assess', 'inter'], [10, 5, 'Brief', 'term']],
  },
  {
    key: 'command', name: 'COMMAND', color: '#8e44c9',
    stops: [[3, 0, 'Liaison desk', 'term'], [4, 1, 'Ingest', 'inter'], [4, 3, 'Process', 'inter'], [4, 5, 'Fuse', 'inter']],
  },
  {
    key: 'effects', name: 'EFFECTS', color: '#f08521',
    stops: [[7, 0, 'Strike cell', 'term'], [8, 1, 'Store', 'inter'], [8, 3, 'Enrich', 'inter'], [8, 5, 'Assess', 'inter'], [9, 4, 'Effect', 'term']],
  },
]

// Out-of-station interchanges — the painful manual hand-offs between systems.
// You exit one line and WALK to another: re-key, swivel-chair, export, carry.
interface WalkDef { a: Pt; b: Pt; label: string; color: string }
const WALKS_RAW: Array<[number, number, number, number, string, string]> = [
  [0, 3, 0, 5, 'PHONE CALL', '#1c5bc4'],
  [2, 1, 2, 3, 'RE-KEY', '#e1251b'],
  [2, 3, 2, 5, 'SNEAKERNET', '#1c5bc4'],
  [6, 1, 6, 3, 'EMAIL EXPORT', '#e1251b'],
  [6, 3, 6, 5, 'SWIVEL CHAIR', '#1c5bc4'],
  [10, 1, 10, 3, 'PDF HANDOFF', '#e1251b'],
]

interface Station { p: Pt; label: string; kind: 'stop' | 'term' | 'inter'; row: number; labelDir: 1 | -1 }
interface Train { line: number; i: number; u: number; speed: number; dwell: number }
interface Passenger { w: number; s: number; speed: number; dropAt: number }

/* ===== FABRIC: the shared ring =========================================== */
interface Product { label: string; color: ColorKey }
const PRODUCTS: Product[] = [
  { label: 'imagery', color: 'amber' }, { label: 'RF sensing', color: 'accent' },
  { label: 'acoustic', color: 'green' }, { label: 'EO/IR tracking', color: 'ask' },
  { label: 'pattern AI', color: 'amber' }, { label: 'track fusion', color: 'accent' },
  { label: 'comms relay', color: 'green' }, { label: 'logistics', color: 'ask' },
  { label: 'weather', color: 'amber' }, { label: 'maritime', color: 'accent' },
  { label: 'cyber sensing', color: 'green' }, { label: 'effects planning', color: 'ask' },
]
interface Flow { a: number; b: number; s: number; speed: number; color: ColorKey; back: boolean }

export class CommunityViz {
  private cv: HTMLCanvasElement
  private ctx: CanvasRenderingContext2D
  private ro: ResizeObserver
  private raf = 0
  private last = 0
  private dpr = 1
  private W = 900
  private H = 520

  private t = 0
  private target = 0
  private targetUntouched = true
  private started = 0
  private pal: Record<string, string> = {}
  private isLight = false

  // subway model
  private stations: Station[] = []
  private lineMpts: Pt[][] = []
  private walks: WalkDef[] = []
  private trains: Train[] = []
  private passengers: Passenger[] = []
  private paxTimer = 0

  // fabric flows
  private flows: Flow[] = []
  private spawnAcc = 0
  private communityPulse = 0

  constructor(canvas: HTMLCanvasElement) {
    this.cv = canvas
    this.ctx = canvas.getContext('2d')!
    this.buildTransit()
    this.refreshTheme()
    this.resize()
    this.ro = new ResizeObserver(() => this.resize())
    this.ro.observe(canvas.parentElement || canvas)
  }

  // ----- build the static transit model (map-space) -----
  private buildTransit() {
    const byKey = new Map<string, Station>()
    for (const ln of LINES) {
      const pts: Pt[] = []
      for (const [c, r, label, kind] of ln.stops) {
        const k = `${c},${r}`
        let st = byKey.get(k)
        if (!st) {
          st = { p: mp(c, r), label, kind: kind || 'stop', row: r, labelDir: r >= 4 ? 1 : -1 }
          byKey.set(k, st)
        } else if (kind === 'inter') st.kind = 'inter'
        pts.push(st.p)
      }
      this.lineMpts.push(pts)
    }
    this.stations = [...byKey.values()]
    this.walks = WALKS_RAW.map(([ax, ay, bx, by, label, color]) => ({ a: mp(ax, ay), b: mp(bx, by), label, color }))
    // two phase-offset trains per line for a continuously busy (clunky) feel
    for (let l = 0; l < LINES.length; l++) {
      const n = this.lineMpts[l].length - 1
      this.trains.push({ line: l, i: 0, u: 0, speed: 86 + l * 6, dwell: 0 })
      this.trains.push({ line: l, i: Math.max(0, (n / 2) | 0), u: 0.3, speed: 80 + l * 5, dwell: 0 })
    }
  }

  refreshTheme() {
    const cs = getComputedStyle(document.documentElement)
    const v = (n: string) => cs.getPropertyValue(n).trim()
    this.isLight = (document.documentElement.getAttribute('data-theme') || 'dark') === 'light'
    this.pal = {
      bg: v('--bg') || '#0b0f14', panel: v('--panel') || '#11171f',
      line: v('--line') || '#1e2935', ink: v('--ink') || '#e7eef6',
      inkDim: v('--ink-dim') || '#9bb0c2', inkFaint: v('--ink-faint') || '#5f7384',
      amber: v('--amber') || '#ffb84d', accent: v('--accent') || '#38e1c4',
      green: v('--green') || '#5ad17a', ask: v('--ask') || '#8ea6ff', pink: v('--pink') || '#ff6b8b',
      // the "map sheet": warm paper in light, a night map in dark
      sheet: this.isLight ? '#f4eee0' : '#0c1119',
      sheetEdge: this.isLight ? '#e4dcc6' : 'rgba(120,160,190,0.12)',
      tick: this.isLight ? '#2a2f36' : '#cdd9e4',
      ringFill: this.isLight ? '#ffffff' : '#0c1119',
    }
  }

  private col(k: ColorKey) { return this.pal[k] || this.pal.accent }
  private accentRGBA() { return hexToRgb(this.col('accent')) }

  start() {
    this.started = performance.now()
    this.last = this.started
    this.t = 0; this.target = 0
    const loop = (now: number) => { this.frame(now); this.raf = requestAnimationFrame(loop) }
    this.raf = requestAnimationFrame(loop)
  }
  stop() { cancelAnimationFrame(this.raf); this.ro.disconnect() }
  setMode(m: 'today' | 'fabric') { this.target = m === 'fabric' ? 1 : 0; this.targetUntouched = false }
  toggle() { this.target = this.target > 0.5 ? 0 : 1; this.targetUntouched = false }
  get mode(): 'today' | 'fabric' { return this.target > 0.5 ? 'fabric' : 'today' }

  private resize() {
    const w = this.cv.parentElement?.clientWidth || 900
    const h = Math.max(440, Math.min(620, Math.round(w * 0.58)))
    this.dpr = Math.min(2, window.devicePixelRatio || 1)
    this.W = w; this.H = h
    this.cv.width = Math.round(w * this.dpr)
    this.cv.height = Math.round(h * this.dpr)
    this.cv.style.width = w + 'px'; this.cv.style.height = h + 'px'
    this.ctx.setTransform(this.dpr, 0, 0, this.dpr, 0, 0)
  }

  // fit map-space into the canvas ("contain")
  private fit() {
    const s = Math.min(this.W / MAPW, this.H / MAPH) * 0.985
    return { s, ox: (this.W - MAPW * s) / 2, oy: (this.H - MAPH * s) / 2 }
  }

  private frame(now: number) {
    const dt = Math.min(0.05, (now - this.last) / 1000)
    this.last = now
    // auto-reveal: dwell on the clunky legacy network, then arrive at the fabric
    if (now - this.started > 5400 && this.targetUntouched) this.target = 1
    this.t += (this.target - this.t) * Math.min(1, dt * 3.0)
    const te = ease(Math.max(0, Math.min(1, this.t)))

    this.updateTransit(dt)
    if (te > 0.015) this.advanceFlows(dt)

    const ctx = this.ctx
    ctx.setTransform(this.dpr, 0, 0, this.dpr, 0, 0)
    ctx.clearRect(0, 0, this.W, this.H)
    this.drawFabricBackdrop(te)
    if (te < 0.985) this.drawTransit(1 - te)
    if (te > 0.015) this.drawFabric(te, now)
  }

  /* ===== TODAY ===== */
  private updateTransit(dt: number) {
    for (const t of this.trains) {
      if (t.dwell > 0) { t.dwell -= dt; continue }
      const pts = this.lineMpts[t.line]
      const a = pts[t.i], b = pts[t.i + 1]
      const len = dist(a, b) || 1
      t.u += (t.speed * dt) / len
      if (t.u >= 1) {
        t.u = 0; t.i++
        if (t.i >= pts.length - 1) { t.i = 0 }
        t.dwell = 0.45 + Math.random() * 0.6 // stop at every station — clunky
      }
    }
    this.paxTimer -= dt
    if (this.paxTimer <= 0 && this.passengers.length < 9) {
      this.paxTimer = 0.5 + Math.random() * 0.8
      const w = (Math.random() * this.walks.length) | 0
      const drop = Math.random() < 0.26 ? 0.32 + Math.random() * 0.46 : 2 // >1 = never drops
      this.passengers.push({ w, s: 0, speed: 1 / (2.4 + Math.random() * 1.4), dropAt: drop })
    }
    for (let k = this.passengers.length - 1; k >= 0; k--) {
      const p = this.passengers[k]
      p.s += p.speed * dt
      if (p.s >= 1 || (p.dropAt <= 1 && p.s > p.dropAt + 0.22)) this.passengers.splice(k, 1)
    }
  }

  private drawTransit(alpha: number) {
    if (alpha <= 0.01) return
    const ctx = this.ctx
    const { s, ox, oy } = this.fit()
    const TX = (m: Pt): Pt => ({ x: ox + m.x * s, y: oy + m.y * s })
    ctx.save()
    ctx.globalAlpha = alpha

    // map sheet
    const sx = ox + MPAD * 0.35 * s, sy = oy + MPAD * 0.2 * s
    roundRect(ctx, sx, sy, MAPW * s - (MPAD * 0.7 * s), MAPH * s - (MPAD * 0.4 * s), 14 * s)
    ctx.fillStyle = this.pal.sheet
    ctx.fill()
    ctx.strokeStyle = this.pal.sheetEdge
    ctx.lineWidth = 1
    ctx.stroke()

    // title roundel + legend (top-left of the sheet)
    this.drawTransitHeader(TX(mp(0, 0)), s)

    // walk corridors (dashed) — drawn under the lines
    for (const w of this.walks) {
      const a = TX(w.a), b = TX(w.b)
      ctx.strokeStyle = withAlpha(this.pal.tick, this.isLight ? 0.5 : 0.55)
      ctx.lineWidth = 1.4
      ctx.setLineDash([2, 4])
      ctx.beginPath(); ctx.moveTo(a.x, a.y); ctx.lineTo(b.x, b.y); ctx.stroke()
      ctx.setLineDash([])
      // friction label
      const mxp = (a.x + b.x) / 2, myp = (a.y + b.y) / 2
      ctx.font = `${Math.round(8.5 * s)}px ui-monospace, monospace`
      ctx.textAlign = 'left'; ctx.textBaseline = 'middle'
      ctx.fillStyle = withAlpha(this.pal.pink, 0.92)
      ctx.fillText(w.label, mxp + 7 * s, myp)
    }

    // line routes — night map gets an additive underglow
    for (let l = 0; l < LINES.length; l++) {
      const pts = this.lineMpts[l].map(TX)
      if (!this.isLight) {
        ctx.save()
        ctx.globalCompositeOperation = 'lighter'
        ctx.strokeStyle = withAlpha(LINES[l].color, 0.22)
        ctx.lineWidth = 12 * s; ctx.lineJoin = 'round'; ctx.lineCap = 'round'
        strokeRoute(ctx, pts, 10 * s)
        ctx.restore()
      }
      ctx.strokeStyle = LINES[l].color
      ctx.lineWidth = 6.5 * s; ctx.lineJoin = 'round'; ctx.lineCap = 'round'
      strokeRoute(ctx, pts, 10 * s)
    }

    // stations: ticks / termini / interchange rings + labels
    ctx.lineCap = 'butt'
    for (const st of this.stations) {
      const c = TX(st.p)
      if (st.kind === 'inter') {
        ctx.beginPath(); ctx.arc(c.x, c.y, 6.5 * s, 0, TAU)
        ctx.fillStyle = this.pal.ringFill; ctx.fill()
        ctx.lineWidth = 2 * s; ctx.strokeStyle = this.pal.tick; ctx.stroke()
      } else {
        // small tick perpendicular to the (mostly orthogonal) line
        const horiz = st.row === 1 || st.row === 3 || st.row === 5
        ctx.strokeStyle = this.pal.tick; ctx.lineWidth = 2 * s
        ctx.beginPath()
        if (horiz) { ctx.moveTo(c.x, c.y - 5 * s); ctx.lineTo(c.x, c.y + 5 * s) }
        else { ctx.moveTo(c.x - 5 * s, c.y); ctx.lineTo(c.x + 5 * s, c.y) }
        ctx.stroke()
      }
      // label
      ctx.font = `${Math.round(9.5 * s)}px ui-sans-serif, system-ui, sans-serif`
      ctx.fillStyle = this.pal.tick
      ctx.textAlign = 'center'
      ctx.textBaseline = st.labelDir < 0 ? 'bottom' : 'top'
      ctx.fillText(st.label, c.x, c.y + st.labelDir * 11 * s)
    }

    // trains (data riding within a system)
    for (const t of this.trains) {
      const pts = this.lineMpts[t.line]
      const a = pts[t.i], b = pts[t.i + 1]
      const m = { x: lerp(a.x, b.x, t.u), y: lerp(a.y, b.y, t.u) }
      const c = TX(m)
      this.glowDot(c, LINES[t.line].color, 5 * s, alpha)
    }

    // passengers walking the corridors (data carried by hand between systems)
    for (const p of this.passengers) {
      const w = this.walks[p.w]
      const a = TX(w.a), b = TX(w.b)
      const pt = { x: lerp(a.x, b.x, p.s), y: lerp(a.y, b.y, p.s) }
      const dropped = p.dropAt <= 1 && p.s >= p.dropAt
      if (dropped) {
        const fade = Math.max(0, 1 - (p.s - p.dropAt) / 0.22)
        ctx.save(); ctx.globalAlpha = alpha * fade
        ctx.strokeStyle = this.pal.pink; ctx.lineWidth = 2 * s
        const z = 4 * s
        ctx.beginPath()
        ctx.moveTo(pt.x - z, pt.y - z); ctx.lineTo(pt.x + z, pt.y + z)
        ctx.moveTo(pt.x + z, pt.y - z); ctx.lineTo(pt.x - z, pt.y + z)
        ctx.stroke(); ctx.restore()
        continue
      }
      // a little carrier: a data packet + a person tick
      ctx.save(); ctx.globalAlpha = alpha
      ctx.fillStyle = w.color
      ctx.fillRect(pt.x - 3 * s, pt.y - 3 * s, 6 * s, 6 * s)
      ctx.strokeStyle = this.pal.tick; ctx.lineWidth = 1.3 * s
      ctx.beginPath(); ctx.arc(pt.x + 6 * s, pt.y - 4 * s, 1.8 * s, 0, TAU); ctx.stroke()
      ctx.beginPath(); ctx.moveTo(pt.x + 6 * s, pt.y - 2 * s); ctx.lineTo(pt.x + 6 * s, pt.y + 3 * s); ctx.stroke()
      ctx.restore()
    }
    ctx.restore()
  }

  private drawTransitHeader(origin: Pt, s: number) {
    const ctx = this.ctx
    const x = origin.x - 14 * s, y = origin.y - 30 * s
    // roundel
    ctx.beginPath(); ctx.arc(x, y, 9 * s, 0, TAU)
    ctx.lineWidth = 3 * s; ctx.strokeStyle = '#e1251b'; ctx.stroke()
    ctx.strokeStyle = '#1c5bc4'; ctx.lineWidth = 3 * s
    ctx.beginPath(); ctx.moveTo(x - 14 * s, y); ctx.lineTo(x + 14 * s, y); ctx.stroke()
    ctx.font = `700 ${Math.round(12 * s)}px ui-sans-serif, system-ui, sans-serif`
    ctx.fillStyle = this.pal.tick; ctx.textAlign = 'left'; ctx.textBaseline = 'middle'
    ctx.fillText('THE LEGACY NETWORK', x + 22 * s, y - 4 * s)
    ctx.font = `${Math.round(9 * s)}px ui-monospace, monospace`
    ctx.fillStyle = withAlpha(this.pal.tick, 0.7)
    ctx.fillText('every system its own line · change here to carry data across', x + 22 * s, y + 8 * s)
  }

  private glowDot(c: Pt, color: string, r: number, alpha: number) {
    const ctx = this.ctx
    ctx.save()
    if (!this.isLight) ctx.globalCompositeOperation = 'lighter'
    ctx.globalAlpha = alpha
    const g = ctx.createRadialGradient(c.x, c.y, 0, c.x, c.y, r * 2)
    g.addColorStop(0, withAlpha(color, 0.95))
    g.addColorStop(1, withAlpha(color, 0))
    ctx.fillStyle = g
    ctx.beginPath(); ctx.arc(c.x, c.y, r * 2, 0, TAU); ctx.fill()
    ctx.globalCompositeOperation = 'source-over'
    ctx.fillStyle = color
    ctx.beginPath(); ctx.arc(c.x, c.y, r * 0.7, 0, TAU); ctx.fill()
    ctx.restore()
  }

  /* ===== FABRIC ===== */
  private center(): Pt { return { x: this.W / 2, y: this.H / 2 + 6 } }
  private ringR(): number { return Math.min(this.W, this.H) * 0.36 }
  private angle(i: number): number { return (i / PRODUCTS.length) * TAU - Math.PI / 2 }
  // bloom outward from the center as te 0->1
  private fabPos(i: number, te: number): Pt {
    const c = this.center(), r = this.ringR(), a = this.angle(i)
    return { x: lerp(c.x, c.x + Math.cos(a) * r, te), y: lerp(c.y, c.y + Math.sin(a) * r, te) }
  }
  private drawFabricBackdrop(te: number) {
    if (te <= 0.01) return
    const ctx = this.ctx, c = this.center()
    const g = ctx.createRadialGradient(c.x, c.y, 10, c.x, c.y, Math.max(this.W, this.H) * 0.7)
    const a = this.accentRGBA()
    g.addColorStop(0, `rgba(${a},${0.1 * te})`)
    g.addColorStop(0.5, `rgba(${a},${0.04 * te})`)
    g.addColorStop(1, 'rgba(0,0,0,0)')
    ctx.fillStyle = g
    ctx.fillRect(0, 0, this.W, this.H)
  }

  private advanceFlows(dt: number) {
    this.spawnAcc += dt * (3 + 18 * Math.min(1, this.t))
    while (this.spawnAcc >= 1) { this.spawnAcc -= 1; this.spawnFlow() }
    for (let k = this.flows.length - 1; k >= 0; k--) {
      this.flows[k].s += this.flows[k].speed * dt
      if (this.flows[k].s >= 1) this.flows.splice(k, 1)
    }
  }
  private spawnFlow() {
    const n = PRODUCTS.length
    const a = (Math.random() * n) | 0
    let b = (Math.random() * n) | 0
    if (b === a) b = (b + 1 + ((Math.random() * (n - 1)) | 0)) % n
    const back = Math.random() < 0.22
    this.flows.push({ a: back ? b : a, b: back ? a : b, s: 0, speed: 0.35 + Math.random() * 0.4, color: PRODUCTS[a].color, back })
    if (this.flows.length > 110) this.flows.splice(0, this.flows.length - 110)
  }
  private chordAt(a: Pt, b: Pt, c: Pt, s: number): Pt {
    const cx = lerp((a.x + b.x) / 2, c.x, 0.55), cy = lerp((a.y + b.y) / 2, c.y, 0.55)
    const u = 1 - s
    return { x: u * u * a.x + 2 * u * s * cx + s * s * b.x, y: u * u * a.y + 2 * u * s * cy + s * s * b.y }
  }

  private drawFabric(te: number, now: number) {
    const ctx = this.ctx, c = this.center(), r = this.ringR()
    const breathe = 0.5 + 0.5 * Math.sin(now / 1300)

    ctx.save(); ctx.globalCompositeOperation = 'lighter'
    const ig = ctx.createRadialGradient(c.x, c.y, r * 0.2, c.x, c.y, r * 1.04)
    const ar = this.accentRGBA()
    ig.addColorStop(0, `rgba(${ar},${0.05 * te})`)
    ig.addColorStop(0.82, `rgba(${ar},${(0.1 + 0.05 * breathe) * te})`)
    ig.addColorStop(1, 'rgba(0,0,0,0)')
    ctx.fillStyle = ig; ctx.beginPath(); ctx.arc(c.x, c.y, r * 1.04, 0, TAU); ctx.fill()
    ctx.restore()

    ctx.save(); ctx.globalAlpha = te
    ctx.strokeStyle = withAlpha(this.col('accent'), 0.35 + 0.25 * breathe)
    ctx.lineWidth = 1.5; ctx.beginPath(); ctx.arc(c.x, c.y, r * te, 0, TAU); ctx.stroke()
    ctx.restore()

    this.communityPulse += 0.0048
    if (this.communityPulse > 1) this.communityPulse -= 1
    const cp = this.communityPulse
    ctx.save(); ctx.globalCompositeOperation = 'lighter'; ctx.globalAlpha = te
    ctx.strokeStyle = withAlpha(this.col('accent'), 0.22 * (1 - cp)); ctx.lineWidth = 2
    ctx.beginPath(); ctx.arc(c.x, c.y, r * (0.6 + 0.7 * cp), 0, TAU); ctx.stroke()
    ctx.restore()

    // value flows
    ctx.save(); ctx.globalCompositeOperation = 'lighter'
    for (const f of this.flows) {
      const A = this.fabPos(f.a, te), B = this.fabPos(f.b, te)
      const head = this.chordAt(A, B, c, f.s), tail = this.chordAt(A, B, c, Math.max(0, f.s - 0.06))
      const col = f.back ? this.col('amber') : this.col(f.color)
      ctx.strokeStyle = withAlpha(col, 0.5 * te); ctx.lineWidth = f.back ? 1 : 1.6
      ctx.beginPath(); ctx.moveTo(tail.x, tail.y); ctx.lineTo(head.x, head.y); ctx.stroke()
      const gr = ctx.createRadialGradient(head.x, head.y, 0, head.x, head.y, f.back ? 5 : 7)
      gr.addColorStop(0, withAlpha(col, 0.95 * te)); gr.addColorStop(1, withAlpha(col, 0))
      ctx.fillStyle = gr; ctx.beginPath(); ctx.arc(head.x, head.y, f.back ? 5 : 7, 0, TAU); ctx.fill()
    }
    ctx.restore()

    for (let i = 0; i < PRODUCTS.length; i++) this.drawNode(this.fabPos(i, te), PRODUCTS[i], te, now, i)

    if (te > 0.4) {
      ctx.save(); ctx.globalAlpha = (te - 0.4) / 0.6; ctx.textAlign = 'center'
      ctx.fillStyle = this.pal.ink; ctx.font = '600 13px ui-sans-serif, system-ui, sans-serif'
      ctx.fillText('one shared fabric', c.x, c.y - 4)
      ctx.fillStyle = this.pal.inkFaint; ctx.font = '11px ui-monospace, monospace'
      ctx.fillText('every product fluent · no gatekeeper', c.x, c.y + 14)
      ctx.restore()
    }
  }

  private drawNode(p: Pt, prod: Product, te: number, now: number, i: number) {
    const ctx = this.ctx, col = this.col(prod.color)
    const pulse = 0.5 + 0.5 * Math.sin(now / 600 + i * 0.9)
    ctx.save(); ctx.globalCompositeOperation = 'lighter'
    const gr = ctx.createRadialGradient(p.x, p.y, 0, p.x, p.y, 16)
    gr.addColorStop(0, withAlpha(col, (0.5 + 0.3 * pulse) * te)); gr.addColorStop(1, withAlpha(col, 0))
    ctx.fillStyle = gr; ctx.beginPath(); ctx.arc(p.x, p.y, 16, 0, TAU); ctx.fill()
    ctx.restore()
    ctx.save(); ctx.globalAlpha = te
    ctx.beginPath(); ctx.fillStyle = col; ctx.arc(p.x, p.y, 5.5, 0, TAU); ctx.fill()
    ctx.strokeStyle = withAlpha(this.pal.bg, 0.9); ctx.lineWidth = 1.5; ctx.stroke()
    const a = this.angle(i), out = Math.cos(a) >= 0
    ctx.fillStyle = withAlpha(this.pal.ink, te)
    ctx.font = '11px ui-sans-serif, system-ui, sans-serif'
    ctx.textAlign = out ? 'left' : 'right'; ctx.textBaseline = 'middle'
    ctx.fillText(prod.label, p.x + Math.cos(a) * 16, p.y + Math.sin(a) * 16)
    ctx.restore()
  }
}

/* ----- helpers ----- */
function strokeRoute(ctx: CanvasRenderingContext2D, pts: Pt[], r: number) {
  if (pts.length < 2) return
  ctx.beginPath()
  ctx.moveTo(pts[0].x, pts[0].y)
  for (let i = 1; i < pts.length - 1; i++) ctx.arcTo(pts[i].x, pts[i].y, pts[i + 1].x, pts[i + 1].y, r)
  ctx.lineTo(pts[pts.length - 1].x, pts[pts.length - 1].y)
  ctx.stroke()
}
function roundRect(ctx: CanvasRenderingContext2D, x: number, y: number, w: number, h: number, r: number) {
  ctx.beginPath(); ctx.moveTo(x + r, y)
  ctx.arcTo(x + w, y, x + w, y + h, r); ctx.arcTo(x + w, y + h, x, y + h, r)
  ctx.arcTo(x, y + h, x, y, r); ctx.arcTo(x, y, x + w, y, r); ctx.closePath()
}
function hexToRgb(hex: string): string {
  const m = hex.replace('#', '')
  const full = m.length === 3 ? m.split('').map((c) => c + c).join('') : m
  const n = parseInt(full, 16)
  if (isNaN(n)) return '56,225,196'
  return `${(n >> 16) & 255},${(n >> 8) & 255},${n & 255}`
}
function withAlpha(hex: string, a: number): string {
  if (!hex || hex[0] !== '#') return hex
  return `rgba(${hexToRgb(hex)},${Math.max(0, Math.min(1, a))})`
}
