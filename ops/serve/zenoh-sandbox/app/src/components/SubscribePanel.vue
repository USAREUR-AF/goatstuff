<script setup lang="ts">
import { ref } from 'vue'
import { z } from '../zenoh'
import Explainer from './Explainer.vue'

const topic = ref('demo/**')
function fmtTime(t: number) { return new Date(t).toLocaleTimeString([], { hour12: false }) }
</script>

<template>
  <div class="panel" :class="{ 'disabled-veil': !z.connected }">
    <div class="panel-head">
      <span class="dot accent" />
      <h3>Subscribe</h3>
      <Explainer>
        To subscribe is to say <b>"I care about this topic"</b>. From then on, anything
        published to a matching topic lands here in real time. Wildcards let you listen wide:
        <code>demo/**</code> hears everything under <code>demo</code>.
      </Explainer>
      <span class="hint">hear what you care about</span>
    </div>

    <div class="row">
      <input class="mono" v-model="topic" spellcheck="false" placeholder="demo/**" />
      <button class="primary" style="flex:0 0 auto" :disabled="!z.connected" @click="z.subscribe(topic)">
        ▸ Listen
      </button>
    </div>

    <div v-for="s in z.subs" :key="s.id" style="margin-top:14px">
      <div class="chip" style="width:100%; justify-content:space-between; border-radius:8px">
        <span><span class="live" /> <span class="k">{{ s.keyExpr }}</span>
          <span class="meta"> · {{ s.count }} heard</span></span>
        <button class="ghost danger sm" @click="z.unsubscribe(s.id)">stop</button>
      </div>
      <div class="feed">
        <div v-for="m in s.feed" :key="m.seq" class="sample" :class="{ fresh: m.fresh }">
          <span class="topic">{{ m.topic }}</span>
          <span class="payload">{{ m.payload }}</span>
          <span class="at">{{ fmtTime(m.at) }}</span>
        </div>
      </div>
    </div>

    <p v-if="z.connected && !z.subs.length" class="tag" style="margin-top:12px">
      Listen on <span class="mono">demo/**</span>, then publish — or open this page in a second tab and watch them talk.
    </p>
  </div>
</template>
