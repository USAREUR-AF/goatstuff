<script setup lang="ts">
import { ref, onMounted, nextTick } from 'vue'
import { GLOSSARY } from './glossary'

const theme = ref(document.documentElement.getAttribute('data-theme') || 'dark')
function toggleTheme() {
  theme.value = theme.value === 'dark' ? 'light' : 'dark'
  document.documentElement.setAttribute('data-theme', theme.value)
  localStorage.setItem('tsbx-theme', theme.value)
}
const entries = [...GLOSSARY].sort((a, b) => a.term.toLowerCase().localeCompare(b.term.toLowerCase()))
const want = new URLSearchParams(location.search).get('term')?.toLowerCase() || ''
const hl = ref(want)
function slug(t: string) { return 'g-' + t.toLowerCase().replace(/[^a-z0-9]+/g, '-') }

onMounted(async () => {
  if (!want) return
  await nextTick()
  const el = document.getElementById(slug(want))
  if (el) { el.scrollIntoView({ block: 'center', behavior: 'smooth' }); setTimeout(() => { hl.value = '' }, 2600) }
})
</script>

<template>
  <div class="wrap">
    <header class="topbar">
      <div class="brand">
        <div class="mark">
          <svg width="18" height="18" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round">
            <path d="M12 3l7 3v5c0 4.5-3 7.5-7 9-4-1.5-7-4.5-7-9V6z" /><path d="M9 12l2 2 4-4" />
          </svg>
        </div>
        <div><h1>GLOSSARY</h1><p class="sub">Every term the sandbox uses, in one line.</p></div>
      </div>
      <span class="spacer" />
      <a class="navlink" href="/">◀ PLAYGROUND</a>
      <a class="navlink" href="/vision.html">✦ BIGGER PICTURE</a>
      <button class="theme-toggle" @click="toggleTheme"><span v-if="theme==='dark'">◐ LIGHT</span><span v-else>◑ DARK</span></button>
    </header>

    <section class="hero">
      <h2>Glossary</h2>
      <p>Plain-language definitions for every acronym and term in the zero-trust sandbox. Dotted terms elsewhere in
        the app open a quick popup; this is the full reference.</p>
    </section>

    <div class="gloss-list">
      <div v-for="e in entries" :key="e.term" :id="slug(e.term)" class="gloss-item no-gloss" :class="{ hot: hl === e.term.toLowerCase() }">
        <div class="gi-term">{{ e.term }}<span v-if="e.full" class="gi-full"> — {{ e.full }}</span></div>
        <div class="gi-def">{{ e.def }}</div>
      </div>
    </div>

    <footer class="foot">A quick reference · <a href="/">back to the playground</a>.</footer>
  </div>
</template>

<style scoped>
.gloss-list { display: flex; flex-direction: column; gap: 10px; }
.gloss-item { background: linear-gradient(180deg, var(--panel), var(--panel-2)); border: var(--seam) solid var(--line);
  border-radius: 10px; padding: 13px 15px; transition: border-color .3s var(--ease), box-shadow .3s var(--ease); }
.gloss-item.hot { border-color: var(--accent); box-shadow: var(--glow); }
.gi-term { font-weight: 700; font-size: 14px; color: var(--accent); }
.gi-full { font-weight: 400; font-size: 12px; color: var(--ink-dim); }
.gi-def { margin-top: 4px; font-size: 13px; color: var(--ink-dim); line-height: 1.55; }
</style>
