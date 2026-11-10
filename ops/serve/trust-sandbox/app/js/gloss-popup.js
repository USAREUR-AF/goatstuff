// SPDX-License-Identifier: Apache-2.0
// Call initGlossPopup() once on a page and it (1) auto-badges known glossary
// terms in the page's static copy with a subtle dotted underline, and (2) shows
// an ephemeral, dismissible popup with the definition when a badge is clicked —
// a quick reference that never navigates you away (Esc / X / click-outside to
// close). A "full glossary" link opens the glossary page teed to that term.
import { TERMS_BY_LEN, lookup } from './glossary.js'
import { h } from './common.js'

const SKIP = new Set(['SCRIPT', 'STYLE', 'A', 'BUTTON', 'INPUT', 'TEXTAREA', 'SELECT', 'CODE', 'CANVAS', 'OPTION'])
const esc = (s) => s.replace(/[.*+?^${}()|[\]\\]/g, '\\$&')
// token-boundary match, case-sensitive for ALL-CAPS acronyms, case-insensitive otherwise.
// single-letter terms (the N/M/W/H/D chips) are NOT auto-badged in prose — they're
// investigated by clicking the chip itself (handled in onClick below).
const RX = new RegExp('(?<![\\w-])(' + TERMS_BY_LEN.filter((t) => t.length >= 2).map(esc).join('|') + ')(?![\\w-])', 'g')

function glossify(root) {
  if (root.dataset.glossed) return
  root.dataset.glossed = '1'
  const walker = document.createTreeWalker(root, NodeFilter.SHOW_TEXT, {
    acceptNode(n) {
      const p = n.parentElement
      if (!p) return NodeFilter.FILTER_REJECT
      if (p.closest('.gloss, .no-gloss, .gloss-pop')) return NodeFilter.FILTER_REJECT
      let e = p
      while (e) { if (SKIP.has(e.tagName)) return NodeFilter.FILTER_REJECT; e = e.parentElement }
      return n.nodeValue && n.nodeValue.trim() ? NodeFilter.FILTER_ACCEPT : NodeFilter.FILTER_REJECT
    },
  })
  const nodes = []
  let cur
  while ((cur = walker.nextNode())) nodes.push(cur)

  for (const node of nodes) {
    const text = node.nodeValue || ''
    RX.lastIndex = 0
    let m
    const pieces = []
    let last = 0
    while ((m = RX.exec(text))) {
      const matched = m[1]
      // match must equal a known term; badge EVERY occurrence (consistent, not first-only)
      const key = TERMS_BY_LEN.find((t) => t.toLowerCase() === matched.toLowerCase())
      if (!key) continue
      // ALL-CAPS acronyms must match case exactly to avoid badging stray words
      if (key === key.toUpperCase() && matched !== key) continue
      pieces.push({ t: text.slice(last, m.index) })
      pieces.push({ t: matched, term: key })
      last = m.index + matched.length
    }
    if (!pieces.length) continue
    pieces.push({ t: text.slice(last) })
    const frag = document.createDocumentFragment()
    for (const p of pieces) {
      if (p.term) {
        const span = document.createElement('span')
        span.className = 'gloss'; span.dataset.term = p.term; span.textContent = p.t
        frag.appendChild(span)
      } else if (p.t) {
        frag.appendChild(document.createTextNode(p.t))
      }
    }
    node.parentNode?.replaceChild(frag, node)
  }
}

let pop = null

function closeGloss() {
  if (pop) { pop.remove(); pop = null }
}

function openGloss(term, x, y) {
  const e = lookup(term)
  if (!e) return
  closeGloss()
  const w = 300, pad = 12
  let left = Math.min(x, window.innerWidth - w - pad)
  left = Math.max(pad, left)
  let top = y + 14
  if (top > window.innerHeight - 160) top = Math.max(pad, y - 160)
  pop = h('div', { class: 'gloss-pop', style: { left: left + 'px', top: top + 'px', width: w + 'px' } },
    h('div', { class: 'gp-head' },
      h('span', { class: 'gp-term' }, e.term, e.full ? h('span', { class: 'gp-full' }, ' — ' + e.full) : null),
      h('button', { class: 'gp-x', 'aria-label': 'close', onclick: closeGloss }, '×')),
    h('div', { class: 'gp-def' }, e.def),
    h('a', { class: 'gp-more', href: '/glossary.html?term=' + encodeURIComponent(e.term) }, 'full glossary →'))
  document.body.appendChild(pop)
}

function onClick(ev) {
  const el = ev.target
  // a badged term, or a symbology chip (.sym N/M/W/H/D) — both are investigatable
  const t = el?.closest?.('.gloss, .sym')
  const term = t?.dataset.term || (t?.classList.contains('sym') ? t.textContent?.trim() : undefined)
  if (t && term && lookup(term)) {
    ev.preventDefault(); ev.stopPropagation()
    openGloss(term, ev.clientX, ev.clientY)
  } else if (pop && !el?.closest?.('.gloss-pop')) {
    closeGloss()
  }
}

export function initGlossPopup() {
  setTimeout(() => glossify(document.body), 60)
  document.addEventListener('click', onClick, true)
  document.addEventListener('keydown', (ev) => { if (ev.key === 'Escape') closeGloss() })
}
