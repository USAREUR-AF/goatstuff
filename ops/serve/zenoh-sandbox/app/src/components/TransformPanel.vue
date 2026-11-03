<script setup lang="ts">
import { ref } from 'vue'
import { z } from '../zenoh'
import Explainer from './Explainer.vue'

const inExpr = ref('demo/sensor/temp')
const outTopic = ref('demo/sensor/fahrenheit')
const label = ref('°C → °F')
const code = ref('msg => {\n  const o = JSON.parse(msg)\n  return JSON.stringify({ fahrenheit: +(o.celsius * 9 / 5 + 32).toFixed(2) })\n}')

const presets: { name: string; code: string; out?: string }[] = [
  { name: '°C → °F', out: 'demo/sensor/fahrenheit',
    code: 'msg => {\n  const o = JSON.parse(msg)\n  return JSON.stringify({ fahrenheit: +(o.celsius * 9 / 5 + 32).toFixed(2) })\n}' },
  { name: 'JSON → CSV', out: 'demo/sensor/csv',
    code: 'msg => Object.values(JSON.parse(msg)).join(\',\')' },
  { name: 'UPPERCASE', out: 'demo/loud',
    code: 'msg => msg.toUpperCase()' },
  { name: 'wrap envelope', out: 'demo/enveloped',
    code: 'msg => JSON.stringify({ value: msg, via: \'sandbox\' })' },
  { name: 'passthrough', out: 'demo/copy',
    code: 'msg => msg' },
]

function usePreset(p: typeof presets[number]) {
  label.value = p.name; code.value = p.code
  if (p.out) outTopic.value = p.out
}
</script>

<template>
  <div class="panel span-2" :class="{ 'disabled-veil': !z.connected }">
    <div class="panel-head">
      <span class="dot amber" />
      <h3>Transform &amp; republish</h3>
      <Explainer>
        Subscribe to one topic, reshape each message, and <b>republish it to another</b> — so a
        downstream consumer can read a format it understands, without the original publisher
        knowing anything about it. This is the fabric as a <b>translator</b>, not a dumb pipe.
        (Your <code>msg =&gt; …</code> runs in your browser; this is an isolated sandbox.)
      </Explainer>
      <span class="hint">one producer's format → another consumer's format</span>
    </div>

    <div class="row" style="align-items:flex-end">
      <div class="field" style="margin:0">
        <label>From topic
          <Explainer>The transform <b>subscribes</b> here. Wildcards work (<code>demo/**</code>).</Explainer>
        </label>
        <input class="mono" v-model="inExpr" spellcheck="false" placeholder="demo/sensor/temp" />
      </div>
      <div style="flex:0 0 auto; padding:0 4px 10px; color:var(--accent); font-size:18px">→</div>
      <div class="field" style="margin:0">
        <label>To topic
          <Explainer>The transformed payload is <b>published</b> here. Keep it different from the input
            topic or you'll make a loop.</Explainer>
        </label>
        <input class="mono" v-model="outTopic" spellcheck="false" placeholder="demo/sensor/fahrenheit" />
      </div>
      <button class="primary" style="flex:0 0 auto; margin-bottom:0"
              :disabled="!z.connected" @click="z.startTransform(inExpr, outTopic, code, label)">
        ▸ Start transform
      </button>
    </div>

    <div class="tryrow" style="margin:12px 0 8px">
      <span class="lead">Transform →</span>
      <button v-for="p in presets" :key="p.name" class="sm" @click="usePreset(p)"
              :class="{ primary: label === p.name }">{{ p.name }}</button>
    </div>
    <div class="field" style="margin:0">
      <label>Transform function <span class="tag" style="text-transform:none">— editable; pick a preset above or write your own</span></label>
      <textarea class="mono" v-model="code" spellcheck="false" style="min-height:78px"
                @input="label = 'custom'" />
    </div>

    <div class="chips" v-if="z.xforms.length">
      <span v-for="x in z.xforms" :key="x.id" class="chip" style="width:100%; justify-content:space-between; border-radius:8px">
        <span style="display:flex; align-items:center; gap:8px; min-width:0">
          <span class="live" />
          <span class="k">{{ x.inExpr }}</span><span class="meta">→ [{{ x.label }}] →</span><span class="k">{{ x.outTopic }}</span>
          <span class="meta">· {{ x.count }} done</span>
        </span>
        <button class="ghost danger sm" @click="z.stopTransform(x.id)">stop</button>
      </span>
    </div>
    <div class="feed" v-if="z.xforms.some(x => x.last || x.error)" style="margin-top:8px">
      <template v-for="x in z.xforms" :key="x.id">
        <div v-if="x.error" class="sample" style="border-left-color:var(--pink)">
          <span class="topic">{{ x.inExpr }}</span>
          <span class="payload">⚠ {{ x.error }}</span><span class="at">error</span>
        </div>
        <div v-else-if="x.last" class="sample">
          <span class="topic mono" style="color:var(--ink-faint)">{{ x.last.in }}</span>
          <span class="payload">→ {{ x.last.out }}</span>
          <span class="at">latest</span>
        </div>
      </template>
    </div>

    <p v-if="z.connected && !z.xforms.length" class="tag" style="margin-top:10px">
      Start the 🌡 sensor (above), run the <span class="mono">°C → °F</span> transform, then subscribe to
      <span class="mono">demo/sensor/fahrenheit</span> — watch one stream become another.
    </p>
  </div>
</template>
