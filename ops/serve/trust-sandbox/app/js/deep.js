// SPDX-License-Identifier: Apache-2.0
// deep.html?topic=<t> — the THIRD layer: a deeper drill-down into one mechanism's
// anatomy, given its own room so the concept page's inline diagram can stay
// clean. Each topic is a static block in deep.html; this shows the one asked for
// and points the back links at the page it came from.
import { initTheme } from './common.js'
import { initGlossPopup } from './gloss-popup.js'

const topic = new URLSearchParams(location.search).get('topic') || 'jwt'
const BACK = {
  jwt: { href: '/concept.html?gate=ory', label: 'Human identity (Ory)' },
  netbird: { href: '/concept.html?gate=netbird', label: 'Network admission (NetBird)' },
  cert: { href: '/concept.html?gate=cert', label: 'Machine identity (x509)' },
  svid: { href: '/concept.html?gate=spire', label: 'Workload identity (SPIRE)' },
  acl: { href: '/concept.html?gate=acl', label: 'The access decision (Zenoh ACL)' },
  rotate: { href: '/', label: 'the playground' },
  revoke: { href: '/', label: 'the playground' },
}
const back = BACK[topic] || BACK.jwt

initTheme()
for (const el of document.querySelectorAll('[data-topic]')) el.hidden = el.dataset.topic !== topic
const top = document.getElementById('d-back'); top.href = back.href; top.textContent = `◀ ${back.label}`
const foot = document.getElementById('d-back-foot'); foot.href = back.href; foot.textContent = `◀ back to ${back.label}`
initGlossPopup()
