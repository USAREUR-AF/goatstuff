<script setup lang="ts">
// A tiny 🛈 that reveals a plain-language note on hover/focus. The whole point
// of the sandbox: never punish a newcomer with vocabulary before meaning.
defineProps<{ label?: string }>()
</script>

<template>
  <span class="explain" tabindex="0">
    <span class="i" aria-label="explain">i</span>
    <span class="pop"><slot /></span>
  </span>
</template>
