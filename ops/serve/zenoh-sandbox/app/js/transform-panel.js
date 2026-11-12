// SPDX-License-Identifier: Apache-2.0
// "Transform & republish" panel: subscribe to one topic, run a
// visitor-written function over each payload, publish the result to another.
import { h, setText, syncList } from './dom.js'
import { z } from './zenoh.js'

const PRESETS = [
  { name: '°C → °F', out: 'demo/sensor/fahrenheit',
    code: 'msg => {\n  const o = JSON.parse(msg)\n  return JSON.stringify({ fahrenheit: +(o.celsius * 9 / 5 + 32).toFixed(2) })\n}' },
  { name: 'JSON → CSV', out: 'demo/sensor/csv',
    code: 'msg => Object.values(JSON.parse(msg)).join(\',\')' },
  { name: 'UPPERCASE', out: 'demo/loud',
    code: 'msg => msg.toUpperCase()' },
  { name: 'wrap envelope', out: 'demo/enveloped',
    code: 'msg => JSON.stringify({ value: msg, via: \'sandbox\' })' },
  { name: 'passthrough', out: 'demo/copy',
    code: 'msg => msg' },
]

export function initTransformPanel() {
  const inExpr = document.getElementById('xf-in')
  const outTopic = document.getElementById('xf-out')
  const code = document.getElementById('xf-code')
  const presetRow = document.getElementById('xf-presets')
  const chips = document.getElementById('xf-chips')
  const feed = document.getElementById('xf-feed')
  const hint = document.getElementById('xf-hint')

  // The name of the transform in the editor: a preset's name, or 'custom'
  // once the visitor edits the function.
  let label = PRESETS[0].name
  code.value = PRESETS[0].code

  const presetButtons = PRESETS.map(p => {
    const b = h('button', { class: 'sm', onclick: () => usePreset(p) }, p.name)
    presetRow.appendChild(b)
    return b
  })
  function paintPresets() {
    PRESETS.forEach((p, i) => presetButtons[i].classList.toggle('primary', label === p.name))
  }
  function usePreset(p) {
    label = p.name; code.value = p.code
    if (p.out) outTopic.value = p.out
    paintPresets()
  }
  code.addEventListener('input', () => { label = 'custom'; paintPresets() })
  paintPresets()

  document.getElementById('xf-start').addEventListener('click',
    () => z.startTransform(inExpr.value, outTopic.value, code.value, label))

  function createChip(x) {
    const meta = h('span', { class: 'meta' })
    const chip = h('span', { class: 'chip', style: 'width:100%; justify-content:space-between; border-radius:8px' },
      h('span', { style: 'display:flex; align-items:center; gap:8px; min-width:0' },
        h('span', { class: 'live' }),
        h('span', { class: 'k' }, x.inExpr), h('span', { class: 'meta' }, `→ [${x.label}] →`), h('span', { class: 'k' }, x.outTopic),
        meta,
      ),
      h('button', { class: 'ghost danger sm', onclick: () => z.stopTransform(x.id) }, 'stop'),
    )
    chip.__meta = meta
    return chip
  }

  // One row per transform that has produced something: its latest
  // input → output pair, or the error its function threw.
  function createRow() {
    const topic = h('span', { class: 'topic' })
    const payload = h('span', { class: 'payload' })
    const at = h('span', { class: 'at' })
    const row = h('div', { class: 'sample' }, topic, payload, at)
    Object.assign(row, { __topic: topic, __payload: payload, __at: at })
    return row
  }
  function updateRow(row, x) {
    const failed = !!x.error
    row.style.borderLeftColor = failed ? 'var(--pink)' : ''
    row.__topic.className = failed ? 'topic' : 'topic mono'
    row.__topic.style.color = failed ? '' : 'var(--ink-faint)'
    setText(row.__topic, failed ? x.inExpr : x.last.in)
    setText(row.__payload, failed ? `⚠ ${x.error}` : `→ ${x.last.out}`)
    setText(row.__at, failed ? 'error' : 'latest')
  }

  return function render() {
    chips.hidden = z.xforms.length === 0
    syncList(chips, z.xforms, x => x.id, createChip, (chip, x) => setText(chip.__meta, `· ${x.count} done`))
    const active = z.xforms.filter(x => x.last || x.error)
    feed.hidden = active.length === 0
    syncList(feed, active, x => x.id, createRow, updateRow)
    hint.hidden = !(z.connected && z.xforms.length === 0)
  }
}
