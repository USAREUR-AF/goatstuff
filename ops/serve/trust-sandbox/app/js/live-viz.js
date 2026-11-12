// SPDX-License-Identifier: Apache-2.0
// The live fabric viz — fed ENTIRELY by real orchestrator /ws events, so it
// never lies. A packet travels left→right through the five gates on each
// attempt, lighting each gate per the real verdict and stopping (with a puff)
// where a real attempt was denied. Real samples the observer saw blip along the
// fabric line. requestAnimationFrame-driven.
//
//   const v = createLiveViz(canvas)  →  v.pushAttempt(gates), v.ping(), v.destroy()
import { SYM_ORDER, SYM_COLOR } from './aspects.js'

// Spelled-out gate names (the words the N/M/W/H/D letters stand for), colored
// per the latest attempt's verdict in draw(): green pass / red fail / gray idle.
const GATES = ['Network', 'Machine', 'Workload', 'Human', 'Decision']

function color(name) {
  const cs = getComputedStyle(document.documentElement)
  return cs.getPropertyValue(name).trim() || '#888'
}

export function createLiveViz(canvas) {
  let ctx = null
  let raf = 0
  let W = 900, H = 150
  let lastStates = []  // the most recent attempt's per-gate verdict
  let packets = []     // { x, stopAt, failed, states, t }
  let blips = []       // { x, y, t }

  const gateX = (i) => 90 + (i * (W - 160)) / 4

  function pushAttempt(gates) {
    // Map the five gates in order; first 'fail' is where the packet stops.
    const order = ['netbird', 'cert', 'spire', 'ory', 'acl']
    const byGate = new Map(gates.map((g) => [g.gate, g.state]))
    const states = order.map((g) => byGate.get(g) || 'na')
    lastStates = states
    let stop = 4
    let failed = false
    for (let i = 0; i < states.length; i++) {
      if (states[i] === 'fail') { stop = i; failed = true; break }
    }
    packets.push({ x: 0, stopAt: gateX(stop), failed, states, t: performance.now() })
  }
  function ping() {
    blips.push({ x: gateX(4) + 30 + Math.random() * 40, y: H / 2 + (Math.random() - 0.5) * 30, t: performance.now() })
  }

  function resize() {
    W = canvas.clientWidth; H = 150
    const dpr = window.devicePixelRatio || 1
    canvas.width = W * dpr; canvas.height = H * dpr
    ctx = canvas.getContext('2d')
    ctx?.scale(dpr, dpr)
  }

  function draw() {
    if (!ctx) { raf = requestAnimationFrame(draw); return }
    const now = performance.now()
    ctx.clearRect(0, 0, W, H)
    const line = color('--line'); const faint = color('--ink-faint')
    const accent = color('--accent'); const green = color('--green'); const pink = color('--pink'); const amber = color('--amber')

    // fabric baseline
    ctx.strokeStyle = line; ctx.lineWidth = 1
    ctx.beginPath(); ctx.moveTo(20, H / 2); ctx.lineTo(W - 20, H / 2); ctx.stroke()

    // gate posts
    const reveal = packets.length ? packets[packets.length - 1] : null  // newest packet drives progressive reveal
    for (let i = 0; i < 5; i++) {
      const x = gateX(i)
      ctx.strokeStyle = line; ctx.lineWidth = 1.5
      ctx.beginPath(); ctx.moveTo(x, H / 2 - 26); ctx.lineTo(x, H / 2 + 26); ctx.stroke()
      // gate word — spelled out, colored by the latest attempt's verdict:
      // green pass / red fail / gray for not-yet-evaluated or not-applicable.
      let wc = faint
      if (lastStates.length) {
        const reached = !reveal || reveal.x >= x - 2  // colors as the packet arrives; persists after it fades
        if (reached) wc = lastStates[i] === 'pass' ? green : lastStates[i] === 'fail' ? pink : faint
      }
      ctx.fillStyle = wc; ctx.font = '700 11.5px ui-monospace, monospace'; ctx.textAlign = 'center'
      ctx.fillText(GATES[i], x, H / 2 + 44)
      // the aspect letter — large, bold, identity-color-coded
      ctx.fillStyle = SYM_COLOR[SYM_ORDER[i]]; ctx.font = '800 17px ui-monospace, monospace'
      ctx.fillText(SYM_ORDER[i], x, H / 2 - 33)
    }

    // packets
    packets = packets.filter((p) => now - p.t < 4000)
    for (const p of packets) {
      if (p.x < p.stopAt) p.x += 4
      // gate dots colored by state up to current x
      for (let i = 0; i < 5; i++) {
        const gx = gateX(i)
        if (p.x >= gx - 2) {
          const s = p.states[i]
          ctx.fillStyle = s === 'pass' ? green : s === 'fail' ? pink : faint
          ctx.beginPath(); ctx.arc(gx, H / 2 - 26, 4, 0, Math.PI * 2); ctx.fill()
        }
      }
      // the packet
      ctx.fillStyle = p.failed && p.x >= p.stopAt ? pink : accent
      ctx.shadowColor = ctx.fillStyle; ctx.shadowBlur = 12
      ctx.beginPath(); ctx.arc(p.x, H / 2, 6, 0, Math.PI * 2); ctx.fill()
      ctx.shadowBlur = 0
      if (p.failed && p.x >= p.stopAt) {
        ctx.strokeStyle = pink; ctx.lineWidth = 1.5; const r = 10 + ((now - p.t) % 600) / 40
        ctx.globalAlpha = 0.5; ctx.beginPath(); ctx.arc(p.x, H / 2, r, 0, Math.PI * 2); ctx.stroke(); ctx.globalAlpha = 1
      }
    }

    // sample blips (real writes the observer saw)
    blips = blips.filter((b) => now - b.t < 1400)
    for (const b of blips) {
      const a = 1 - (now - b.t) / 1400
      ctx.fillStyle = amber; ctx.globalAlpha = a
      ctx.beginPath(); ctx.arc(b.x, b.y, 3, 0, Math.PI * 2); ctx.fill(); ctx.globalAlpha = 1
    }

    raf = requestAnimationFrame(draw)
  }

  resize()
  window.addEventListener('resize', resize)
  raf = requestAnimationFrame(draw)

  return {
    pushAttempt,
    ping,
    destroy() { cancelAnimationFrame(raf); window.removeEventListener('resize', resize) },
  }
}
