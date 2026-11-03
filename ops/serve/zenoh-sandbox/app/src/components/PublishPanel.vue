<script setup lang="ts">
import { ref } from 'vue'
import { z } from '../zenoh'
import Explainer from './Explainer.vue'

const topic = ref('demo/hello')
const payload = ref('Hello, fabric!')
const periodMs = ref(0)
</script>

<template>
  <div class="panel" :class="{ 'disabled-veil': !z.connected }">
    <div class="panel-head">
      <span class="dot amber" />
      <h3>Publish</h3>
      <Explainer>
        To publish is to <b>put a value onto a topic</b>. You don't address anyone — you just
        announce it. Whoever is subscribed to a matching topic hears it; if nobody is, it
        simply isn't heard. No queues, no broker mailbox.
      </Explainer>
      <span class="hint">put a value onto the fabric</span>
    </div>

    <div class="field">
      <label>Topic
        <Explainer>Zenoh calls this a <b>key expression</b> — a <code>/</code>-separated path like
          <code>demo/sensor/temp</code>. Subscribers can use <code>*</code> (one segment) and
          <code>**</code> (any depth) to listen broadly.</Explainer>
      </label>
      <input class="mono" v-model="topic" spellcheck="false" placeholder="demo/hello" />
    </div>
    <div class="field">
      <label>Payload</label>
      <textarea v-model="payload" spellcheck="false" placeholder="anything — text, JSON, …" />
    </div>
    <div class="field" style="max-width:240px">
      <label>Period ms <span class="tag" style="text-transform:none">(0 = manual only)</span>
        <Explainer>A <b>declared publisher</b> with a period &gt; 0 <b>auto-publishes</b> the payload
          on that interval. <code>0</code> means manual — declare it, then hit <b>publish now</b> whenever
          you like. (One-shot <code>put</code> below needs no declared publisher at all.)</Explainer>
      </label>
      <input class="mono" type="number" min="0" step="100" v-model.number="periodMs" />
    </div>

    <div class="row">
      <button class="primary" :disabled="!z.connected" @click="z.putOnce(topic, payload)">▸ Publish once</button>
      <button class="ghost" :disabled="!z.connected" @click="z.declarePublisher(topic, payload, periodMs)">＋ Declare publisher</button>
    </div>

    <div class="tryrow" style="margin-top:14px">
      <span class="lead">Try this →</span>
      <button class="sm" :disabled="!z.connected" @click="z.startSensor('demo/sensor/temp')">
        🌡 Start a pretend temperature sensor
      </button>
    </div>

    <div class="chips" v-if="z.pubs.length">
      <span v-for="p in z.pubs" :key="p.id" class="chip">
        <span class="live" />
        <span class="k">{{ p.keyExpr }}</span>
        <span class="meta">{{ p.periodMs > 0 ? `every ${p.periodMs}ms` : 'manual' }} · {{ p.sent }} put</span>
        <button v-if="p.periodMs === 0" class="ghost sm" @click="z.publishNow(p.id)">publish now</button>
        <button class="ghost danger sm" @click="z.stopPublisher(p.id)">stop</button>
      </span>
    </div>
  </div>
</template>
