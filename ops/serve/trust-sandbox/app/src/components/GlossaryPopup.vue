<script setup lang="ts">
// Drop one <GlossaryPopup/> on a page and it (1) auto-badges known glossary
// terms in the page's static copy with a subtle dotted underline, and (2) shows
// an ephemeral, dismissible popup with the definition when a badge is clicked —
// a quick reference that never navigates you away (Esc / X / click-outside to
// close). A "full glossary" link opens the glossary page teed to that term.
import { onMounted, onBeforeUnmount, nextTick } from 'vue'
import { GLOSSARY, TERMS_BY_LEN, glossState, openGloss, closeGloss, lookup } from '../glossary'

const SKIP = new Set(['SCRIPT', 'STYLE', 'A', 'BUTTON', 'INPUT', 'TEXTAREA', 'SELECT', 'CODE', 'CANVAS', 'OPTION'])
const esc = (s: string) => s.replace(/[.*+?^${}()|[\]\\]/g, '\\$&')
// token-boundary match, case-sensitive for ALL-CAPS acronyms, case-insensitive otherwise.
// single-letter terms (the N/M/W/H/D chips) are NOT auto-badged in prose — they're
// investigated by clicking the chip itself (handled in onClick below).
const RX = new RegExp('(?<![\\w-])(' + TERMS_BY_LEN.filter((t) => t.length >= 2).map(esc).join('|') + ')(?![\\w-])', 'g')

function glossify(root: HTMLElement) {
  if (root.dataset.glossed) return
  root.dataset.glossed = '1'
  const walker = document.createTreeWalker(root, NodeFilter.SHOW_TEXT, {
    acceptNode(n) {
      const p = n.parentElement
      if (!p) return NodeFilter.FILTER_REJECT
      if (p.closest('.gloss, .no-gloss')) return NodeFilter.FILTER_REJECT
      let e: HTMLElement | null = p
      while (e) { if (SKIP.has(e.tagName)) return NodeFilter.FILTER_REJECT; e = e.parentElement }
      return n.nodeValue && n.nodeValue.trim() ? NodeFilter.FILTER_ACCEPT : NodeFilter.FILTER_REJECT
    },
  })
  const nodes: Text[] = []
  let cur: Node | null
  while ((cur = walker.nextNode())) nodes.push(cur as Text)

  for (const node of nodes) {
    const text = node.nodeValue || ''
    RX.lastIndex = 0
    let m: RegExpExecArray | null
    const pieces: { t: string; term?: string }[] = []
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

function onClick(ev: MouseEvent) {
  const el = ev.target as HTMLElement
  // a badged term, or a symbology chip (.sym N/M/W/H/D) — both are investigatable
  const t = el?.closest?.('.gloss, .sym') as HTMLElement | null
  const term = t?.dataset.term || (t?.classList.contains('sym') ? t.textContent?.trim() : undefined)
  if (t && term && lookup(term)) {
    ev.preventDefault(); ev.stopPropagation()
    openGloss(term, ev.clientX, ev.clientY)
  } else if (glossState.open && !(el)?.closest?.('.gloss-pop')) {
    closeGloss()
  }
}
function onKey(ev: KeyboardEvent) { if (ev.key === 'Escape') closeGloss() }

onMounted(async () => {
  await nextTick()
  setTimeout(() => glossify(document.body), 60)
  document.addEventListener('click', onClick, true)
  document.addEventListener('keydown', onKey)
})
onBeforeUnmount(() => { document.removeEventListener('click', onClick, true); document.removeEventListener('keydown', onKey) })

function popStyle() {
  const w = 300, pad = 12
  let x = Math.min(glossState.x, window.innerWidth - w - pad)
  x = Math.max(pad, x)
  let y = glossState.y + 14
  if (y > window.innerHeight - 160) y = Math.max(pad, glossState.y - 160)
  return { left: x + 'px', top: y + 'px', width: w + 'px' }
}
</script>

<template>
  <div v-if="glossState.open && glossState.entry" class="gloss-pop" :style="popStyle()">
    <div class="gp-head">
      <span class="gp-term">{{ glossState.entry.term }}<span v-if="glossState.entry.full" class="gp-full"> — {{ glossState.entry.full }}</span></span>
      <button class="gp-x" @click="closeGloss" aria-label="close">×</button>
    </div>
    <div class="gp-def">{{ glossState.entry.def }}</div>
    <a class="gp-more" :href="'/glossary.html?term=' + encodeURIComponent(glossState.entry.term)">full glossary →</a>
  </div>
</template>

<style>
/* global (not scoped) so the auto-injected .gloss spans are styled everywhere */
.gloss { border-bottom: 1px dotted color-mix(in srgb, var(--accent) 70%, transparent); cursor: help; }
.gloss:hover { border-bottom-color: var(--accent); background: color-mix(in srgb, var(--accent) 10%, transparent); }
.gloss-pop {
  position: fixed; z-index: 1000; background: var(--panel-raise); color: var(--ink);
  border: 1px solid var(--accent); border-radius: 10px; padding: 12px 13px; box-shadow: var(--shadow);
  animation: gpfade .14s var(--ease);
}
@keyframes gpfade { from { opacity: 0; transform: translateY(-4px); } to { opacity: 1; transform: translateY(0); } }
.gloss-pop .gp-head { display: flex; align-items: baseline; gap: 8px; margin-bottom: 6px; }
.gloss-pop .gp-term { font-weight: 700; font-size: 13px; color: var(--accent); }
.gloss-pop .gp-full { font-weight: 400; font-size: 11.5px; color: var(--ink-dim); }
.gloss-pop .gp-x { margin-left: auto; background: none; border: none; color: var(--ink-faint); font-size: 18px; line-height: 1; cursor: pointer; padding: 0 2px; }
.gloss-pop .gp-x:hover { color: var(--ink); transform: none; }
.gloss-pop .gp-def { font-size: 12.5px; color: var(--ink-dim); line-height: 1.5; }
.gloss-pop .gp-more { display: inline-block; margin-top: 9px; font-family: var(--font-mono); font-size: 10.5px; color: var(--ink-faint); }
.gloss-pop .gp-more:hover { color: var(--accent); }
</style>
