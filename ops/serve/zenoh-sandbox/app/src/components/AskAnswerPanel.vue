<script setup lang="ts">
import { ref } from 'vue'
import { z } from '../zenoh'
import Explainer from './Explainer.vue'

// Kept on the SAME through-line as the rest of the page (the temperature):
// pub/sub above *pushes* a stream of readings; query/reply *pulls* an answer
// on demand — "what's the temperature right now?".
const qTopic = ref('demo/temperature')
const qReply = ref('21.5 °C')
const selector = ref('demo/temperature')
const replies = ref<{ topic: string; payload: string; ok: boolean; id: number }[]>([])
let rid = 0

function runQuery() {
  replies.value = []
  z.sendQuery(selector.value, (topic, payload, ok) => {
    replies.value.unshift({ topic, payload, ok, id: rid++ })
  })
}
</script>

<template>
  <div class="panel span-2" :class="{ 'disabled-veil': !z.connected }">
    <div class="panel-head">
      <span class="dot ask" />
      <h3>Query &amp; Reply</h3>
      <Explainer>
        Publish/subscribe (above) <b>pushes</b> a stream — a subscriber hears every reading as
        it happens. Query/reply <b>pulls</b>: a <b>queryable</b> stands ready to answer a topic, and a
        <b>get</b> sends a <b>query</b> that reaches every matching queryable and collects their
        <b>replies</b>. Same fabric, opposite direction.
      </Explainer>
      <span class="hint">get · query · queryable · reply</span>
    </div>

    <p class="callout" style="margin-bottom:14px">
      <span>🌡️</span>
      <span>The sensor above <b>streams</b> the temperature (subscribe to hear every reading). Here you
        do the other half: stand up a <b>queryable</b> that answers <span class="mono">demo/temperature</span>,
        then <b>ask</b> it — <i>"what's the temperature right now?"</i> — and get a reply back.</span>
    </p>

    <div class="grid">
      <div>
        <p class="tag" style="margin:0 0 8px">① Declare a queryable — stand ready to answer</p>
        <div class="row">
          <input class="mono" v-model="qTopic" spellcheck="false" placeholder="demo/temperature" aria-label="queryable key expression" />
          <input class="mono" v-model="qReply" spellcheck="false" placeholder="reply payload" aria-label="reply" />
          <button class="primary" style="flex:0 0 auto" :disabled="!z.connected" @click="z.declareQueryable(qTopic, qReply)">
            ＋ Queryable
          </button>
        </div>
        <div class="chips" v-if="z.queryables.length">
          <span v-for="q in z.queryables" :key="q.id" class="chip">
            <span class="live" /><span class="k">{{ q.keyExpr }}</span>
            <span class="meta">answers “{{ q.reply }}” · {{ q.replied }} replied</span>
            <button class="ghost danger sm" @click="z.undeclareQueryable(q.id)">stop</button>
          </span>
        </div>
        <p v-else class="tag" style="margin-top:10px">Nothing is answering yet. Declare a queryable, then ask for it on the right →</p>
      </div>

      <div>
        <p class="tag" style="margin:0 0 8px">② Send a query (get) — ask for it</p>
        <div class="row">
          <input class="mono" v-model="selector" spellcheck="false" placeholder="demo/temperature" aria-label="query selector" />
          <button class="primary" style="flex:0 0 auto" :disabled="!z.connected" @click="runQuery">
            ▸ get
          </button>
        </div>
        <div class="feed" v-if="replies.length">
          <div v-for="r in replies" :key="r.id" class="sample" :style="!r.ok ? 'border-left-color:var(--pink)' : ''">
            <span class="topic">{{ r.topic }}</span>
            <span class="payload">{{ r.ok ? r.payload : '⚠ ' + r.payload }}</span>
            <span class="at">{{ r.ok ? 'reply' : 'error' }}</span>
          </div>
        </div>
        <p v-else class="tag" style="margin-top:10px">
          No replies yet. Declare a queryable on the left for this topic, then hit <span class="mono">get</span> —
          its reply lands here. (If nothing is answering, a get simply comes back empty.)
        </p>
      </div>
    </div>
  </div>
</template>
