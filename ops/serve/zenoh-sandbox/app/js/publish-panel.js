// SPDX-License-Identifier: Apache-2.0
// "Publish" panel: a one-shot put, declared publishers (manual or periodic),
// and the pretend temperature sensor.
import { h, setText, syncList } from './dom.js'
import { z } from './zenoh.js'

export function initPublishPanel() {
  const topic = document.getElementById('pub-topic')
  const payload = document.getElementById('pub-payload')
  const period = document.getElementById('pub-period')
  const chips = document.getElementById('pub-chips')

  // An empty or negative period means manual, the same as 0.
  const periodMs = () => Math.max(0, Number(period.value) || 0)

  document.getElementById('pub-once').addEventListener('click', () => z.putOnce(topic.value, payload.value))
  document.getElementById('pub-declare').addEventListener('click', () => z.declarePublisher(topic.value, payload.value, periodMs()))
  document.getElementById('pub-sensor').addEventListener('click', () => z.startSensor('demo/sensor/temp'))

  function createChip(p) {
    const meta = h('span', { class: 'meta' })
    const chip = h('span', { class: 'chip' },
      h('span', { class: 'live' }),
      h('span', { class: 'k' }, p.keyExpr),
      meta,
      p.periodMs === 0 && h('button', { class: 'ghost sm', onclick: () => z.publishNow(p.id) }, 'publish now'),
      h('button', { class: 'ghost danger sm', onclick: () => z.stopPublisher(p.id) }, 'stop'),
    )
    chip.__meta = meta
    return chip
  }

  function updateChip(chip, p) {
    setText(chip.__meta, `${p.periodMs > 0 ? `every ${p.periodMs}ms` : 'manual'} · ${p.sent} put`)
  }

  return function render() {
    chips.hidden = z.pubs.length === 0
    syncList(chips, z.pubs, p => p.id, createChip, updateChip)
  }
}
