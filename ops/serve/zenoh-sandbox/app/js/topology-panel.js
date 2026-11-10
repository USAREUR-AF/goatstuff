// SPDX-License-Identifier: Apache-2.0
// "The fabric, live" panel — the machinery made visible, the BROKERLESS way.
// Zenoh's headline property: there is no broker in the middle. A publisher's
// data goes straight to the subscribers whose key expression matches. So this
// view is peer-to-peer: producers on the left, consumers on the right,
// transforms (which are both) in the middle, queryables below. It draws a
// direct edge for every real match, and animates a packet straight from the
// producer of a message to each peer that actually received it — never through
// a central hub. (The panel's caption says so: in THIS browser sandbox the
// bytes do relay through the remote-api router, because browsers can't peer
// directly; native Zenoh peers deliver point-to-point.)
import { s, setText, syncList } from './dom.js'
import { z } from './zenoh.js'

const W = 900, H = 372
const COLOR = { pub: 'var(--amber)', sub: 'var(--accent)', queryable: 'var(--ask)', xform: 'var(--green)' }
const LANES = [
  { x: 118, label: 'publishers', color: COLOR.pub },
  { x: W / 2, label: 'transforms', color: COLOR.xform },
  { x: W - 118, label: 'subscribers', color: COLOR.sub },
]

// --- Zenoh key-expression matching (pattern with * / **, concrete key) ------
function keMatches(pattern, key) {
  return matchSeg(pattern.split('/').filter(Boolean), key.split('/').filter(Boolean))
}
function matchSeg(p, k) {
  if (p.length === 0) return k.length === 0
  if (p[0] === '**') { for (let i = 0; i <= k.length; i++) if (matchSeg(p.slice(1), k.slice(i))) return true; return false }
  if (k.length === 0) return false
  if (p[0] === '*' || p[0] === k[0]) return matchSeg(p.slice(1), k.slice(1))
  return false
}

function shortLabel(ke) { const t = ke.split('/').filter(Boolean).slice(-2).join('/'); return t.length > 20 ? '…' + t.slice(-19) : t }
function spread(i, n, a, b) { return n <= 1 ? (a + b) / 2 : a + ((b - a) / (n - 1)) * i }

// The peers to draw, laid out from the current state.
function layoutNodes() {
  const out = []
  z.pubs.forEach((p, i) => out.push({ key: `pub:${p.id}`, kind: 'pub', id: p.id, label: shortLabel(p.keyExpr), x: 118, y: spread(i, z.pubs.length, 58, 252), color: COLOR.pub, topic: p.keyExpr, live: !!p.timer }))
  // Transforms sit below the pub/sub midline so the data path pub→xform→sub
  // forms a visible V rather than three points on one horizontal.
  z.xforms.forEach((x, i) => out.push({ key: `xform:${x.id}`, kind: 'xform', id: x.id, label: shortLabel(x.outTopic), x: W / 2, y: spread(i, z.xforms.length, 230, 300), color: COLOR.xform, inExpr: x.inExpr, outTopic: x.outTopic, live: true }))
  z.subs.forEach((sub, i) => out.push({ key: `sub:${sub.id}`, kind: 'sub', id: sub.id, label: shortLabel(sub.keyExpr), x: W - 118, y: spread(i, z.subs.length, 58, 252), color: COLOR.sub, pattern: sub.keyExpr, live: true }))
  z.queryables.forEach((q, i) => out.push({ key: `queryable:${q.id}`, kind: 'queryable', id: q.id, label: shortLabel(q.keyExpr), x: spread(i, z.queryables.length, 230, 670), y: 326, color: COLOR.queryable, pattern: q.keyExpr }))
  return out
}

// Direct match edges: a producer connects to every consumer whose pattern
// matches what it publishes. THIS is the routing — drawn point-to-point.
function matchEdges(nodes) {
  const producers = nodes.filter(n => n.kind === 'pub' || n.kind === 'xform').map(n => ({ n, topic: n.kind === 'pub' ? n.topic : n.outTopic }))
  const consumers = nodes.filter(n => n.kind === 'sub' || n.kind === 'xform').map(n => ({ n, pat: n.kind === 'sub' ? n.pattern : n.inExpr }))
  const edges = []
  for (const p of producers) for (const c of consumers) {
    if (p.n.key === c.n.key) continue
    if (keMatches(c.pat, p.topic)) edges.push({ x1: p.n.x, y1: p.n.y, x2: c.n.x, y2: c.n.y })
  }
  return edges
}

export function initTopologyPanel() {
  const lanesEl = document.getElementById('topo-lanes')
  const edgesEl = document.getElementById('topo-edges')
  const nodesEl = document.getElementById('topo-nodes')
  const packetsEl = document.getElementById('topo-packets')
  const emptyEl = document.getElementById('topo-empty')

  for (const l of LANES) lanesEl.appendChild(s('text', { x: l.x, y: 22, style: { fill: l.color } }, l.label))
  const queryableLane = s('text', { x: W / 2, y: 300, class: 'lane-q' }, 'queryables · a get reaches the matching ones')
  queryableLane.style.display = 'none'
  lanesEl.appendChild(queryableLane)

  let nodes = []

  function createNode(n) {
    const flash = s('circle', { class: 'flash', r: 11, style: { stroke: n.color, display: 'none' } })
    const label = s('text', {
      class: 'nodelabel',
      x: n.kind === 'pub' ? 18 : n.kind === 'sub' ? -18 : 0,
      y: n.kind === 'queryable' ? 26 : n.kind === 'xform' ? -18 : 4,
      'text-anchor': n.kind === 'pub' ? 'start' : n.kind === 'sub' ? 'end' : 'middle',
    })
    const g = s('g', null,
      flash,
      s('circle', { class: 'nodefill', r: 11, style: { fill: n.color } }),
      n.live && s('circle', { class: 'nodepulse', r: 11, style: { stroke: n.color } }),
      label,
    )
    g.__flash = flash
    g.__label = label
    return g
  }
  function updateNode(g, n) {
    g.setAttribute('transform', `translate(${n.x},${n.y})`)
    setText(g.__label, n.label)
  }

  function render() {
    nodes = layoutNodes()
    syncList(nodesEl, nodes, n => n.key, createNode, updateNode)
    edgesEl.replaceChildren(...matchEdges(nodes).map(e => s('line', e)))
    queryableLane.style.display = z.queryables.length ? '' : 'none'
    emptyEl.hidden = !(z.connected && nodes.length === 0)
  }

  // Consumers (subs + the input side of transforms) whose pattern matches a
  // concrete published topic — this is the real key-expression routing.
  function consumersOf(topic, exceptXformId) {
    const out = []
    for (const n of nodes) {
      if (n.kind === 'sub' && keMatches(n.pattern, topic)) out.push(n)
      else if (n.kind === 'xform' && n.id !== exceptXformId && keMatches(n.inExpr, topic)) out.push(n)
    }
    return out
  }

  // --- animated packets ----------------------------------------------------
  // Driven by the SENDER's own event (a subscriber can't tell which publisher
  // a message came from, so attribution must come from the producer side). On
  // a publish a packet fans out to every matching consumer; that fan-out IS
  // the brokerless routing.
  const pulses = []    // { x1, y1, x2, y2, color, born, dur }
  const flashes = {}   // node key → time the flash ends
  const packetPool = []
  const clock = () => performance.now()

  z.onFlow((e) => {
    if (e.dir !== 'out') return // packets are driven by sends, not receipts
    if (e.node === 'queryable') { flashes[`queryable:${e.id}`] = clock() + 460; return } // a reply — flash, no fan-out
    // the producer that actually emitted: a declared pub/xform node, else off-stage (ad-hoc put)
    const from = nodes.find(n => n.kind === e.node && n.id === e.id) ?? { x: 22, y: H / 2 }
    if (e.id >= 0) flashes[`${e.node}:${e.id}`] = clock() + 420
    const color = COLOR[e.node]
    for (const c of consumersOf(e.topic, e.node === 'xform' ? e.id : undefined)) {
      pulses.push({ x1: from.x, y1: from.y, x2: c.x, y2: c.y, color, born: clock(), dur: 600 })
    }
    if (pulses.length > 300) pulses.splice(0, pulses.length - 300)
  })

  function frame() {
    const now = clock()
    for (let i = pulses.length - 1; i >= 0; i--) if (now - pulses[i].born > pulses[i].dur) pulses.splice(i, 1)

    // packets: straight from the producer to the peer that received
    let used = 0
    for (const p of pulses) {
      const t = (now - p.born) / p.dur
      if (t < 0 || t > 1) continue
      const e = t < 0.5 ? 2 * t * t : 1 - Math.pow(-2 * t + 2, 2) / 2
      let dot = packetPool[used]
      if (!dot) { dot = s('circle', { class: 'packet', r: 4.5 }); packetPool.push(dot); packetsEl.appendChild(dot) }
      used++
      dot.setAttribute('cx', p.x1 + (p.x2 - p.x1) * e)
      dot.setAttribute('cy', p.y1 + (p.y2 - p.y1) * e)
      dot.style.fill = p.color
      dot.style.opacity = t < 0.15 ? t / 0.15 : t > 0.82 ? (1 - t) / 0.18 : 1
      dot.style.display = ''
    }
    for (let i = used; i < packetPool.length; i++) packetPool[i].style.display = 'none'

    // a ring around a node that just sent, fading out
    for (const g of nodesEl.children) {
      const o = ((flashes[g.__key] ?? 0) - now) / 420
      if (o > 0) { g.__flash.style.display = ''; g.__flash.style.opacity = Math.min(1, o) }
      else if (g.__flash.style.display !== 'none') g.__flash.style.display = 'none'
    }
    for (const k in flashes) if (flashes[k] <= now) delete flashes[k]

    requestAnimationFrame(frame)
  }
  requestAnimationFrame(frame)

  return render
}
