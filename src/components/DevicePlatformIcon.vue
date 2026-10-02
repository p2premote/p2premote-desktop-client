<template>
  <span class="platform-icon" :class="[`platform-${platform}`, tone ? `tone-${tone}` : '']" role="img" :aria-label="label" :title="label">
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
    <svg v-else-if="brandIcon" viewBox="0 0 24 24" aria-hidden="true">
      <path :d="brandIcon.path" />
    </svg>
    <svg v-else-if="platform === 'kylin'" viewBox="0 0 16.7 16.7" aria-hidden="true">
      <path d="M7.94.07 5.81 2.25l-.54-.06-.44 1.77 2.22-1.17-.49-2.03.89-2.72M4.83 3.96l1.35 1.56 1.36-.69L5.81 2.25 3.89 2.96l.42 3.75L2.14 4.03l-.89 1.82L.03 7.93l1.15 2.84.37 2.24-.77.92-.81.98h2.21l.28-1.99.89-1.36 1.48.25 1.64-.65 1.21-.62 1.18.79-1.42 1.69-1.45 1.37 2.28.03.01-.93 1.73-1.37.63.8-1.82 2.59 2.32.01-.08-1.3 1.66-1.57-.85-1.72-.1-2.12 1.7-1 .96.2 1.31-.49.37-2.63-2.04.6-1.24 1.86-2.09 1.11-2.92-1.18-2.13-1.58 1.39-1.95-.49-2.04-2.71.87-2.72M2.05 6.32v2.13l3.13.63-.87-2.37-2.26-.39m3.13 2.76-.37 1.83 3.48-.73-.01-1.82-3.1.72m3.1 1.1 2.3.94.61-1.57-2.91.63m2.91-.63 1.02.35 1.07-1.46-2.09 1.11m2.09-1.11.63.46.61-2.32-1.24 1.86" />
    </svg>
    <svg v-else-if="platform === 'uos'" viewBox="0 0 24 24" aria-hidden="true">
      <path fill-rule="evenodd" d="M12 2a10 10 0 1 0 10 10h-3.6A6.4 6.4 0 1 1 12 5.6V2Zm0 5.2a4.8 4.8 0 1 0 4.8 4.8h-3.2a1.6 1.6 0 1 1-1.6-1.6V7.2Z" />
    </svg>
    <svg v-else viewBox="0 0 24 24" aria-hidden="true">
      <rect x="3" y="4" width="18" height="13" rx="2" fill="none" stroke="currentColor" stroke-width="2" />
      <path d="M8 21h8M12 17v4" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" />
    </svg>
  </span>
</template>

<script setup lang="ts">
import { computed } from 'vue'
import { siAndroid, siApple, siDeepin, siLinux, siUbuntu, type SimpleIcon } from 'simple-icons'
import { detectDevicePlatform, devicePlatformLabel, type DevicePlatformSource } from '../utils/devicePlatform'

const props = defineProps<{
  device: DevicePlatformSource | null
  tone?: 'offline' | 'online' | 'connecting' | 'connected'
}>()
const platform = computed(() => detectDevicePlatform(props.device))
const label = computed(() => devicePlatformLabel(platform.value))
const brandIcons: Partial<Record<ReturnType<typeof detectDevicePlatform>, SimpleIcon>> = {
  ubuntu: siUbuntu,
  android: siAndroid,
  macos: siApple,
  deepin: siDeepin,
  linux: siLinux,
}
const brandIcon = computed(() => brandIcons[platform.value])
</script>

<style scoped>
.platform-icon { display: inline-flex; width: 1em; height: 1em; align-items: center; justify-content: center; }
.platform-icon svg { width: 1em; height: 1em; overflow: visible; fill: currentColor; }
.platform-ubuntu { color: #e95420; }
.platform-android { color: #3ddc84; }
.platform-kylin { color: #5b68d9; }
.platform-uos { color: #2675ec; }
.platform-deepin { color: #007cff; }
.platform-linux { color: #fcc624; }
.platform-icon.tone-offline { color: var(--fluent-text-tertiary); }
.platform-icon.tone-online,
.platform-icon.tone-connected { color: var(--status-online); }
.platform-icon.tone-connecting {
  color: var(--fluent-warning);
  animation: platform-status-pulse 1.5s var(--fluent-easing) infinite;
}

@keyframes platform-status-pulse {
  0%, 100% { opacity: 1; }
  50% { opacity: 0.52; }
}
</style>
