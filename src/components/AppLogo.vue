<template>
  <svg xmlns="http://www.w3.org/2000/svg" :width="size" :height="size" viewBox="0 0 200 200">
    <defs>
      <linearGradient id="lg" x1="0%" y1="0%" x2="100%" y2="100%">
        <stop offset="0%" style="stop-color:#409EFF"/>
        <stop offset="100%" style="stop-color:#2B7FD4"/>
      </linearGradient>
    </defs>
    <rect x="10" y="10" width="180" height="180" rx="40" fill="url(#lg)"/>
    <rect x="34" y="60" width="44" height="80" rx="3" fill="none" stroke="#fff" stroke-width="4.5"/>
    <rect x="122" y="60" width="44" height="80" rx="3" fill="none" stroke="#fff" stroke-width="4.5"/>
    <line x1="100" y1="52" x2="100" y2="148" stroke="#fff" stroke-width="4" stroke-linecap="round" opacity="0.5"/>
    <path d="M88 100 L96 100" stroke="#fff" stroke-width="4.5" stroke-linecap="round"/>
    <path d="M91 93 L96 100 L91 107" fill="none" stroke="#fff" stroke-width="4.5" stroke-linecap="round" stroke-linejoin="round"/>
    <path d="M112 100 L104 100" stroke="#fff" stroke-width="4.5" stroke-linecap="round"/>
    <path d="M109 93 L104 100 L109 107" fill="none" stroke="#fff" stroke-width="4.5" stroke-linecap="round" stroke-linejoin="round"/>
  </svg>
</template>
<script setup lang="ts">
defineProps({
  size: { type: [Number, String], default: 32 }
})
</script>
