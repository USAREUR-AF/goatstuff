// SPDX-License-Identifier: Apache-2.0
// "Subscribe" panel: declare subscribers (wildcards welcome) and show each
// one's live feed of samples.
import { h, setText, syncList, fmtTime } from './dom.js'
import { z } from './zenoh.js'

export function initSubscribePanel() {
  const topic = document.getElementById('sub-topic')
  const list = document.getElementById('sub-list')
  const hint = document.getElementById('sub-hint')

  document.getElementById('sub-listen').addEventListener('click', () => z.subscribe(topic.value))

  function createSub(sub) {
    const meta = h('span', { class: 'meta' })
    const feed = h('div', { class: 'feed' })
    const block = h('div', { style: 'margin-top:14px' },
      h('div', { class: 'chip', style: 'width:100%; justify-content:space-between; border-radius:8px' },
        h('span', null, h('span', { class: 'live' }), ' ', h('span', { class: 'k' }, sub.keyExpr), meta),
        h('button', { class: 'ghost danger sm', onclick: () => z.unsubscribe(sub.id) }, 'stop'),
      ),
      feed,
    )
    block.__meta = meta
    block.__feed = feed
    return block
  }

  function createSample(m) {
    return h('div', { class: 'sample' },
      h('span', { class: 'topic' }, m.topic),
      h('span', { class: 'payload' }, m.payload),
      h('span', { class: 'at' }, fmtTime(m.at)),
    )
  }

  function updateSub(block, sub) {
    setText(block.__meta, ` · ${sub.count} heard`)
    syncList(block.__feed, sub.feed, m => m.seq, createSample, (el, m) => el.classList.toggle('fresh', m.fresh))
  }

  return function render() {
    syncList(list, z.subs, sub => sub.id, createSub, updateSub)
    hint.hidden = !(z.connected && z.subs.length === 0)
  }
}
