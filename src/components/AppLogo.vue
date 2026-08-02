<template>
  <img
    src="/favicon.png"
    alt=""
    :width="size"
    :height="size"
    class="app-logo"
    draggable="false"
  />
</template>
<script setup lang="ts">
defineProps({
  size: { type: [Number, String], default: 32 }
})
</script>

<style scoped>
.app-logo {
  display: inline-block;
  object-fit: contain;
  flex: 0 0 auto;
  user-select: none;
}
</style>
