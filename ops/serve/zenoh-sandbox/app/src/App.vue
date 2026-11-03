<script setup lang="ts">
import { ref } from 'vue'
import ConnectBar from './components/ConnectBar.vue'
import TopologyPanel from './components/TopologyPanel.vue'
import PublishPanel from './components/PublishPanel.vue'
import SubscribePanel from './components/SubscribePanel.vue'
import TransformPanel from './components/TransformPanel.vue'
import AskAnswerPanel from './components/AskAnswerPanel.vue'
import ActivityLog from './components/ActivityLog.vue'

const theme = ref(document.documentElement.getAttribute('data-theme') || 'dark')
function toggleTheme() {
  theme.value = theme.value === 'dark' ? 'light' : 'dark'
  document.documentElement.setAttribute('data-theme', theme.value)
  localStorage.setItem('zsbx-theme', theme.value)
}
</script>

<template>
  <div class="wrap">
    <header class="topbar">
      <div class="brand">
        <div class="mark">
          <svg width="18" height="18" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round">
            <circle cx="12" cy="12" r="2.4" /><path d="M12 4v3M12 17v3M4 12h3M17 12h3" />
            <path d="M6.3 6.3l2.1 2.1M15.6 15.6l2.1 2.1M17.7 6.3l-2.1 2.1M8.4 15.6l-2.1 2.1" opacity=".6" />
          </svg>
        </div>
        <div>
          <h1>ZENOH&nbsp;SANDBOX</h1>
          <p class="sub">A safe place to feel how the fabric moves data.</p>
        </div>
      </div>
      <span class="spacer" />
      <a class="navlink" href="/community.html" title="What the shared fabric makes possible">✦ THE BIGGER PICTURE</a>
      <button class="theme-toggle" @click="toggleTheme" :aria-label="'switch to ' + (theme==='dark'?'light':'dark') + ' mode'">
        <span v-if="theme==='dark'">◐ LIGHT</span><span v-else>◑ DARK</span>
      </button>
    </header>

    <section class="hero">
      <h2>Watch messages move across the <span class="accent">fabric</span>.</h2>
      <p>
        Zenoh moves data between things that don't know about each other. Someone
        <b>publishes</b> to a topic; whoever <b>subscribes</b> to a matching topic hears it; and
        a <b>queryable</b> can answer questions on demand. Try it below — then open this page in a
        second tab and watch the two talk.
      </p>
    </section>

    <div class="callout">
      <span>🛟</span>
      <span><b>This is a throwaway sandbox.</b> The router it talks to peers with nothing and stores
        nothing — there's no login and nothing to break. Experiment freely.</span>
    </div>

    <main class="grid">
      <ConnectBar />
      <TopologyPanel />
      <PublishPanel />
      <SubscribePanel />
      <TransformPanel />
      <AskAnswerPanel />
      <ActivityLog />
    </main>

    <footer class="foot">
      Built on <code>@eclipse-zenoh/zenoh-ts</code> · an isolated learning playground, distinct from the trial fabric.<br />
      New to Zenoh? You're in the right place — publish, subscribe, query, reply, and watch it move.
    </footer>
  </div>
</template>
