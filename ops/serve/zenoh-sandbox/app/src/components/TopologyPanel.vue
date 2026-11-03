<script setup lang="ts">
// Phase 3 — the machinery, made visible, the BROKERLESS way.
// Zenoh's headline property: there is no broker in the middle. A publisher's
// data goes straight to the subscribers whose key expression matches. So this
// view is peer-to-peer: producers on the left, consumers on the right,
// transforms (which are both) in the middle, queryables below. We draw a
// direct edge for every real match, and animate a packet straight from the
// producer of a message to each peer that actually received it — never through
// a central hub. (Honest caveat in the caption: in THIS browser sandbox the
// bytes do relay through the remote-api router, because browsers can't peer
// directly; native Zenoh peers deliver point-to-point.)
import { ref, computed, onMounted, onBeforeUnmount } from 'vue'
import { z, type FlowEvent, type FlowNode } from '../zenoh'

const W = 900, H = 372

// --- Zenoh key-expression matching (pattern with * / **, concrete key) ------
function keMatches(pattern: string, key: string): boolean {
  return matchSeg(pattern.split('/').filter(Boolean), key.split('/').filter(Boolean))
}
function matchSeg(p: string[], k: string[]): boolean {
  if (p.length === 0) return k.length === 0
  if (p[0] === '**') { for (let i = 0; i <= k.length; i++) if (matchSeg(p.slice(1), k.slice(i))) return true; return false }
  if (k.length === 0) return false
  if (p[0] === '*' || p[0] === k[0]) return matchSeg(p.slice(1), k.slice(1))
  return false
}

interface N { key: string; kind: FlowNode; id: number; label: string; x: number; y: number; color: string
  topic?: string; pattern?: string; inExpr?: string; outTopic?: string; live?: boolean }
function shortLabel(ke: string): string { const t = ke.split('/').filter(Boolean).slice(-2).join('/'); return t.length > 20 ? '…' + t.slice(-19) : t }
function spread(i: number, n: number, a: number, b: number): number { return n <= 1 ? (a + b) / 2 : a + ((b - a) / (n - 1)) * i }
const COLOR: Record<FlowNode, string> = { pub: 'var(--amber)', sub: 'var(--accent)', queryable: 'var(--ask)', xform: 'var(--green)' }

const nodes = computed<N[]>(() => {
  const out: N[] = []
  z.pubs.forEach((p, i) => out.push({ key: `pub:${p.id}`, kind: 'pub', id: p.id, label: shortLabel(p.keyExpr), x: 118, y: spread(i, z.pubs.length, 58, 252), color: COLOR.pub, topic: p.keyExpr, live: !!p.timer }))
  // Transforms are placed below the pub/sub midline so the data path pub→xform→sub
  // forms a visible V rather than a collinear trio on the same horizontal.
  z.xforms.forEach((x, i) => out.push({ key: `xform:${x.id}`, kind: 'xform', id: x.id, label: shortLabel(x.outTopic), x: W / 2, y: spread(i, z.xforms.length, 230, 300), color: COLOR.xform, inExpr: x.inExpr, outTopic: x.outTopic, live: true }))
  z.subs.forEach((s, i) => out.push({ key: `sub:${s.id}`, kind: 'sub', id: s.id, label: shortLabel(s.keyExpr), x: W - 118, y: spread(i, z.subs.length, 58, 252), color: COLOR.sub, pattern: s.keyExpr, live: true }))
  z.queryables.forEach((q, i) => out.push({ key: `queryable:${q.id}`, kind: 'queryable', id: q.id, label: shortLabel(q.keyExpr), x: spread(i, z.queryables.length, 230, 670), y: 326, color: COLOR.queryable, pattern: q.keyExpr }))
  return out
})

// Direct match edges: a producer connects to every consumer whose pattern
// matches what it publishes. THIS is the routing — drawn point-to-point.
const edges = computed(() => {
  const ns = nodes.value
  const producers = ns.filter(n => n.kind === 'pub' || n.kind === 'xform').map(n => ({ n, topic: n.kind === 'pub' ? n.topic! : n.outTopic! }))
  const consumers = ns.filter(n => n.kind === 'sub' || n.kind === 'xform').map(n => ({ n, pat: n.kind === 'sub' ? n.pattern! : n.inExpr! }))
  const es: { x1: number; y1: number; x2: number; y2: number }[] = []
  for (const p of producers) for (const c of consumers) {
    if (p.n.key === c.n.key) continue
    if (keMatches(c.pat, p.topic)) es.push({ x1: p.n.x, y1: p.n.y, x2: c.n.x, y2: c.n.y })
  }
  return es
})

function posOf(kind: FlowNode, id: number) { const n = nodes.value.find(v => v.kind === kind && v.id === id); return n ? { x: n.x, y: n.y } : null }

// Consumers (subs + the input side of transforms) whose pattern matches a
// concrete published topic — this is the real keyexpr routing.
function consumersOf(topic: string, exceptXformId?: number) {
  const ns = nodes.value
  const out: N[] = []
  for (const n of ns) {
    if (n.kind === 'sub' && keMatches(n.pattern!, topic)) out.push(n)
    else if (n.kind === 'xform' && n.id !== exceptXformId && keMatches(n.inExpr!, topic)) out.push(n)
  }
  return out
}

// --- animated packets ------------------------------------------------------
// Driven by the SENDER's own event (we know exactly who sent it — a subscriber
// can't tell which publisher a message came from, so attribution must come from
// the producer side). On a publish we fan a packet out to every matching
// consumer; that fan-out IS the brokerless routing.
interface Pulse { x1: number; y1: number; x2: number; y2: number; color: string; born: number; dur: number }
const pulses: Pulse[] = []
const flashes: Record<string, number> = {}
const frame = ref(0)
let raf = 0
const clock = () => performance.now()

function onFlow(e: FlowEvent) {
  if (e.dir !== 'out') return // packets are driven by sends, not receipts
  if (e.node === 'queryable') { flashes[`queryable:${e.id}`] = clock() + 460; return } // a reply — flash, no fan-out
  // the producer that actually emitted: a declared pub/xform node, else off-stage (ad-hoc put)
  const src = posOf(e.node, e.id) ?? { x: 22, y: H / 2 }
  if (e.id >= 0) flashes[`${e.node}:${e.id}`] = clock() + 420
  const color = COLOR[e.node]
  for (const c of consumersOf(e.topic, e.node === 'xform' ? e.id : undefined)) {
    pulses.push({ x1: src.x, y1: src.y, x2: c.x, y2: c.y, color, born: clock(), dur: 600 })
  }
  if (pulses.length > 300) pulses.splice(0, pulses.length - 300)
}

const livePulses = computed(() => {
  void frame.value
  const now = clock(); const live: { cx: number; cy: number; color: string; o: number }[] = []
  for (const p of pulses) {
    const t = (now - p.born) / p.dur; if (t < 0 || t > 1) continue
    const e = t < 0.5 ? 2 * t * t : 1 - Math.pow(-2 * t + 2, 2) / 2
    live.push({ cx: p.x1 + (p.x2 - p.x1) * e, cy: p.y1 + (p.y2 - p.y1) * e, color: p.color, o: t < 0.15 ? t / 0.15 : t > 0.82 ? (1 - t) / 0.18 : 1 })
  }
  return live
})
const flashState = computed<Record<string, number>>(() => {
  void frame.value; const now = clock(); const m: Record<string, number> = {}
  for (const k in flashes) { const o = (flashes[k] - now) / 420; if (o > 0) m[k] = Math.min(1, o) }
  return m
})

function loop() {
  const now = clock()
  for (let i = pulses.length - 1; i >= 0; i--) if (now - pulses[i].born > pulses[i].dur) pulses.splice(i, 1)
  frame.value = now
  raf = requestAnimationFrame(loop)
}
onMounted(() => { z.onFlow(onFlow); raf = requestAnimationFrame(loop) })
onBeforeUnmount(() => { cancelAnimationFrame(raf); const i = z._flow.indexOf(onFlow); if (i >= 0) z._flow.splice(i, 1) })

const empty = computed(() => nodes.value.length === 0)
const lanes = [
  { x: 118, label: 'publishers', color: COLOR.pub },
  { x: W / 2, label: 'transforms', color: COLOR.xform },
  { x: W - 118, label: 'subscribers', color: COLOR.sub },
]
</script>

<template>
  <div class="panel span-2 topo" :class="{ 'disabled-veil': !z.connected }">
    <div class="panel-head">
      <span class="dot accent" />
      <h3>The fabric, live</h3>
      <span class="hint">brokerless — a publisher's data goes straight to the subscribers that match</span>
    </div>

    <p class="callout" style="margin-bottom:10px">
      <span>🛰️</span>
      <span><b>No broker in the middle.</b> When a publisher puts to a topic, Zenoh delivers it
        <b>directly</b> to every subscriber whose key expression matches — peer to peer, no hub
        relaying everything. Each line below is a real match; each dot is a real delivery.
        <span style="color:var(--ink-faint)">(In this browser sandbox the bytes do relay through the
        remote-api router, since browsers can't peer directly; native Zenoh peers go point-to-point.)</span></span>
    </p>

    <svg class="topo-svg" :viewBox="`0 0 ${W} ${H}`" preserveAspectRatio="xMidYMid meet">
      <!-- lane captions -->
      <g class="lanes">
        <text v-for="l in lanes" :key="l.label" :x="l.x" y="22" :style="{ fill: l.color }">{{ l.label }}</text>
        <text v-if="z.queryables.length" :x="W/2" y="300" class="lane-q">queryables · a get reaches the matching ones</text>
      </g>

      <!-- direct producer → consumer match edges (the routing, drawn P2P) -->
      <g class="edges">
        <line v-for="(e, i) in edges" :key="'e' + i" :x1="e.x1" :y1="e.y1" :x2="e.x2" :y2="e.y2" />
      </g>

      <!-- peers -->
      <g v-for="n in nodes" :key="n.key" :transform="`translate(${n.x},${n.y})`">
        <circle v-if="flashState[`${n.kind}:${n.id}`]" class="flash" r="11"
                :style="{ stroke: n.color, opacity: flashState[`${n.kind}:${n.id}`] }" />
        <circle class="nodefill" r="11" :style="{ fill: n.color }" />
        <circle v-if="n.live" class="nodepulse" r="11" :style="{ stroke: n.color }" />
        <text class="nodelabel"
              :x="n.kind==='pub' ? 18 : n.kind==='sub' ? -18 : 0"
              :y="n.kind==='queryable' ? 26 : n.kind==='xform' ? -18 : 4"
              :text-anchor="n.kind==='pub' ? 'start' : n.kind==='sub' ? 'end' : 'middle'">{{ n.label }}</text>
      </g>

      <!-- packets: straight from producer to the peer that received -->
      <circle v-for="(p, i) in livePulses" :key="'p' + i" :cx="p.cx" :cy="p.cy" r="4.5"
              class="packet" :style="{ fill: p.color, opacity: p.o }" />
    </svg>

    <p v-if="z.connected && empty" class="topo-empty tag">
      Declare a publisher or subscriber below — start the 🌡 sensor and listen on
      <span class="mono">demo/**</span> — and watch the data go straight across.
    </p>
  </div>
</template>

<style scoped>
.topo-svg { width: 100%; height: auto; display: block; margin-top: 4px; }
.lanes text { font-family: var(--font-mono); font-size: 11px; letter-spacing: .06em; text-anchor: middle; opacity: .9; }
.lane-q { fill: var(--ink-faint); font-family: var(--font-mono); font-size: 10px; text-anchor: middle; letter-spacing: .08em; }
.edges line { stroke: var(--line); stroke-width: 1.1; }
.nodefill { stroke: var(--panel); stroke-width: 2; }
.nodepulse { fill: none; stroke-width: 1.4; opacity: .5; transform-origin: center; animation: nodepulse 1.9s var(--ease) infinite; }
@keyframes nodepulse { 0% { r: 11px; opacity: .5; } 100% { r: 20px; opacity: 0; } }
.flash { fill: none; stroke-width: 2.4; }
.nodelabel { fill: var(--ink-dim); font-family: var(--font-mono); font-size: 11px; }
.packet { filter: drop-shadow(0 0 5px currentColor); }
.topo-empty { text-align: center; margin-top: 6px; }
</style>
