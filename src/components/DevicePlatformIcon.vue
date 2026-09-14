<template>
  <span class="platform-icon" :class="`platform-${platform}`" role="img" :aria-label="label" :title="label">
    <svg v-if="platform === 'windows-11'" viewBox="0 0 24 24" aria-hidden="true">
      <path d="M3 3h8v8H3V3Zm10 0h8v8h-8V3ZM3 13h8v8H3v-8Zm10 0h8v8h-8v-8Z" />
    </svg>
    <svg v-else-if="platform === 'windows-10'" viewBox="0 0 24 24" aria-hidden="true">
      <path d="M3 5.2 10.6 4v7.1H3V5.2Zm9-1.4L21 2.5v8.6h-9V3.8ZM3 12.5h7.6v7.2L3 18.5v-6Zm9 0h9V21l-9-1.3v-7.2Z" />
    </svg>
    <svg v-else-if="platform === 'windows-7'" viewBox="0 0 24 24" aria-hidden="true">
      <path d="M2.8 5.8c2.6.5 5.2-.8 7.8-1.3v6.6c-2.6.5-5.2 1.8-7.8 1.3V5.8Zm9.1-1.6c3-.5 6-2 9.1-1.2v6.5c-3-.8-6 .7-9.1 1.2V4.2ZM2.8 13.7c2.6.5 5.2-.8 7.8-1.3V19c-2.6.5-5.2 1.8-7.8 1.3v-6.6Zm9.1-1.6c3-.5 6-2 9.1-1.2v6.5c-3-.8-6 .7-9.1 1.2v-6.5Z" />
    </svg>
    <svg v-else-if="platform === 'windows'" viewBox="0 0 24 24" aria-hidden="true">
      <path d="M3 4.5 10.7 3v8H3V4.5ZM12.2 2.8 21 1.5V11h-8.8V2.8ZM3 12.5h7.7v8L3 19v-6.5Zm9.2 0H21v9.4l-8.8-1.2v-8.2Z" />
    </svg>
    <svg v-else-if="platform === 'ubuntu'" viewBox="0 0 24 24" aria-hidden="true">
      <circle cx="12" cy="12" r="5.2" fill="none" stroke="currentColor" stroke-width="2.8" />
      <circle cx="4.2" cy="12" r="2.2" /><circle cx="17.6" cy="5.4" r="2.2" /><circle cx="17.6" cy="18.6" r="2.2" />
    </svg>
    <svg v-else-if="platform === 'android'" viewBox="0 0 24 24" aria-hidden="true">
      <path d="m7.2 7.2-1.5-2.6.9-.5 1.6 2.7A8 8 0 0 1 12 6c1.4 0 2.7.3 3.8.8l1.6-2.7.9.5-1.5 2.6A6.1 6.1 0 0 1 19 12H5a6.1 6.1 0 0 1 2.2-4.8ZM5 13h14v6.5c0 .8-.7 1.5-1.5 1.5h-11c-.8 0-1.5-.7-1.5-1.5V13Zm3.3-3.2a1 1 0 1 0 0-2 1 1 0 0 0 0 2Zm7.4 0a1 1 0 1 0 0-2 1 1 0 0 0 0 2Z" />
    </svg>
    <svg v-else-if="platform === 'kylin'" viewBox="0 0 24 24" aria-hidden="true">
      <path d="M5 3h3v7.2L15.8 3H20l-8.6 8.2L20.5 21h-4.3L8 12.3V21H5V3Z" />
    </svg>
    <svg v-else-if="platform === 'uos'" viewBox="0 0 24 24" aria-hidden="true">
      <path d="M4 4h3.3v9.7c0 2.8 1.7 4.3 4.7 4.3s4.7-1.5 4.7-4.3V4H20v9.9c0 4.8-3 7.5-8 7.5s-8-2.7-8-7.5V4Z" />
    </svg>
    <svg v-else-if="platform === 'linux'" viewBox="0 0 24 24" aria-hidden="true">
      <ellipse cx="12" cy="14.2" rx="6.2" ry="7.2" /><ellipse cx="12" cy="6.4" rx="3.8" ry="4.4" />
      <circle cx="10.6" cy="5.8" r=".7" class="cutout" /><circle cx="13.4" cy="5.8" r=".7" class="cutout" />
      <path d="m12 7.1-2 1.5 2 1 2-1-2-1.5Z" class="accent" />
      <ellipse cx="9" cy="21" rx="3.4" ry="1.3" class="accent" /><ellipse cx="15" cy="21" rx="3.4" ry="1.3" class="accent" />
    </svg>
    <svg v-else viewBox="0 0 24 24" aria-hidden="true">
      <rect x="3" y="4" width="18" height="13" rx="2" fill="none" stroke="currentColor" stroke-width="2" />
      <path d="M8 21h8M12 17v4" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" />
    </svg>
  </span>
</template>

<script setup lang="ts">
import { computed } from 'vue'
import { detectDevicePlatform, devicePlatformLabel, type DevicePlatformSource } from '../utils/devicePlatform'

const props = defineProps<{ device: DevicePlatformSource | null }>()
const platform = computed(() => detectDevicePlatform(props.device))
const label = computed(() => devicePlatformLabel(platform.value))
</script>

<style scoped>
.platform-icon { display: inline-flex; width: 1em; height: 1em; align-items: center; justify-content: center; }
.platform-icon svg { width: 1em; height: 1em; overflow: visible; fill: currentColor; }
.platform-ubuntu { color: #e95420; }
.platform-android { color: #3ddc84; }
.platform-kylin { color: #5b68d9; }
.platform-uos { color: #2675ec; }
.platform-linux { color: currentColor; }
.platform-linux .cutout { fill: var(--platform-icon-cutout, #fff); }
.platform-linux .accent { fill: #f4b400; }
</style>
