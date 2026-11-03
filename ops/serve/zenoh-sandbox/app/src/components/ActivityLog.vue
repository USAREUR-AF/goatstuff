<script setup lang="ts">
import { z } from '../zenoh'
function fmtTime(t: number) { return new Date(t).toLocaleTimeString([], { hour12: false }) }
</script>

<template>
  <div class="panel span-2">
    <div class="panel-head">
      <span class="dot green" />
      <h3>Activity</h3>
      <span class="hint">a plain-English trace of everything happening on your link</span>
    </div>
    <div class="logwrap">
      <div v-for="l in z.log" :key="l.id" class="logline">
        <span class="t">{{ fmtTime(l.at) }}</span>
        <span class="b" :class="l.tone" />
        <span class="m">{{ l.text }}</span>
      </div>
      <p v-if="!z.log.length" class="tag" style="padding:8px 2px">Nothing yet. Connect, then publish or subscribe.</p>
    </div>
  </div>
</template>
