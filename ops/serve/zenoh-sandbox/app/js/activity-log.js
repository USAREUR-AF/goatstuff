// SPDX-License-Identifier: Apache-2.0
// "Activity" panel: a plain-English trace of everything the session did,
// newest first.
import { h, syncList, fmtTime } from './dom.js'
import { z } from './zenoh.js'

export function initActivityLog() {
  const lines = document.getElementById('log-lines')
  const empty = document.getElementById('log-empty')
  let shown = -1

  return function render() {
    if (shown === z.logVersion) return
    shown = z.logVersion
    empty.hidden = z.log.length > 0
    syncList(lines, z.log, l => l.id, l =>
      h('div', { class: 'logline' },
        h('span', { class: 't' }, fmtTime(l.at)),
        h('span', { class: 'b ' + l.tone }),
        h('span', { class: 'm' }, l.text),
      ))
  }
}
