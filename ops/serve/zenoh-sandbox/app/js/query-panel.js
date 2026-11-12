// SPDX-License-Identifier: Apache-2.0
// "Query & Reply" panel: declare queryables on the left, send a get on the
// right and list the replies it collects.
import { h, setText, syncList } from './dom.js'
import { z } from './zenoh.js'

export function initQueryPanel() {
  const qTopic = document.getElementById('q-topic')
  const qReply = document.getElementById('q-reply')
  const selector = document.getElementById('q-selector')
  const chips = document.getElementById('q-chips')
  const none = document.getElementById('q-none')
  const repliesEl = document.getElementById('q-replies')
  const noReplies = document.getElementById('q-noreplies')

  let replies = [] // { id, topic, payload, ok } — the replies to the latest get
  let rid = 0

  function renderReplies() {
    repliesEl.hidden = replies.length === 0
    noReplies.hidden = replies.length > 0
    syncList(repliesEl, replies, r => r.id, r =>
      h('div', { class: 'sample', style: r.ok ? null : 'border-left-color:var(--pink)' },
        h('span', { class: 'topic' }, r.topic),
        h('span', { class: 'payload' }, r.ok ? r.payload : '⚠ ' + r.payload),
        h('span', { class: 'at' }, r.ok ? 'reply' : 'error'),
      ))
  }

  document.getElementById('q-declare').addEventListener('click', () => z.declareQueryable(qTopic.value, qReply.value))
  document.getElementById('q-get').addEventListener('click', () => {
    replies = []
    renderReplies()
    z.sendQuery(selector.value, (topic, payload, ok) => {
      replies.unshift({ id: rid++, topic, payload, ok })
      renderReplies()
    })
  })

  function createChip(q) {
    const meta = h('span', { class: 'meta' })
    const chip = h('span', { class: 'chip' },
      h('span', { class: 'live' }), h('span', { class: 'k' }, q.keyExpr),
      meta,
      h('button', { class: 'ghost danger sm', onclick: () => z.undeclareQueryable(q.id) }, 'stop'),
    )
    chip.__meta = meta
    return chip
  }

  return function render() {
    chips.hidden = z.queryables.length === 0
    none.hidden = z.queryables.length > 0
    syncList(chips, z.queryables, q => q.id, createChip,
      (chip, q) => setText(chip.__meta, `answers “${q.reply}” · ${q.replied} replied`))
  }
}
