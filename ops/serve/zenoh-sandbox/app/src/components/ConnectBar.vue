<script setup lang="ts">
import { z } from '../zenoh'
import Explainer from './Explainer.vue'
const labels: Record<string, string> = { idle: 'not connected', connecting: 'connecting…', connected: 'on the fabric', error: 'connection trouble' }
</script>

<template>
  <div class="panel span-2">
    <div class="panel-head">
      <span class="dot accent" />
      <h3>The fabric link</h3>
      <Explainer>
        Your browser talks to a throwaway <b>Zenoh router</b> over a WebSocket. This sandbox
        router peers with nothing and keeps nothing — it's safe to play. Same URL in another
        tab = two peers on the same fabric.
      </Explainer>
      <span class="hint">isolated · disposable · no login</span>
    </div>

    <div class="connbar">
      <input class="url mono" v-model="z.url" :disabled="z.connected || z.status==='connecting'"
             spellcheck="false" aria-label="server URL" />
      <button v-if="!z.connected" class="primary" :disabled="z.status==='connecting'" @click="z.connect()">
        {{ z.status==='connecting' ? 'Linking…' : 'Connect' }}
      </button>
      <button v-else class="ghost danger" @click="z.disconnect()">Disconnect</button>
      <span class="status" :class="z.status">
        <span class="led" /> {{ labels[z.status] }}
      </span>
    </div>
    <p v-if="z.status==='error'" class="mono" style="color:var(--pink);font-size:12px;margin:10px 0 0">{{ z.error }}</p>
  </div>
</template>
