// SPDX-License-Identifier: Apache-2.0
// glossary.html — the full glossary, alphabetical. ?term=<t> scrolls to and
// briefly highlights that entry (the popup's "full glossary →" link).
import { initTheme, h } from './common.js'
import { GLOSSARY } from './glossary.js'

initTheme()

const entries = [...GLOSSARY].sort((a, b) => a.term.toLowerCase().localeCompare(b.term.toLowerCase()))
const want = new URLSearchParams(location.search).get('term')?.toLowerCase() || ''
const slug = (t) => 'g-' + t.toLowerCase().replace(/[^a-z0-9]+/g, '-')

const list = document.getElementById('gloss-list')
for (const e of entries) {
  list.appendChild(h('div', { id: slug(e.term), class: 'gloss-item no-gloss' + (want === e.term.toLowerCase() ? ' hot' : '') },
    h('div', { class: 'gi-term' }, e.term, e.full ? h('span', { class: 'gi-full' }, ' — ' + e.full) : null),
    h('div', { class: 'gi-def' }, e.def)))
}

if (want) {
  const el = document.getElementById(slug(want))
  if (el) {
    el.scrollIntoView({ block: 'center', behavior: 'smooth' })
    setTimeout(() => { for (const x of list.querySelectorAll('.hot')) x.classList.remove('hot') }, 2600)
  }
}
