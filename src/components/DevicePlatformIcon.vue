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
    <svg v-else-if="platform === 'ubuntu'" viewBox="0 0 24 24" aria-hidden="true">
      <circle cx="12" cy="12" r="5.2" fill="none" stroke="currentColor" stroke-width="2.8" />
      <circle cx="4.2" cy="12" r="2.2" /><circle cx="17.6" cy="5.4" r="2.2" /><circle cx="17.6" cy="18.6" r="2.2" />
    </svg>
    <svg v-else-if="platform === 'android'" viewBox="0 0 24 24" aria-hidden="true">
      <path d="m17.6 9.48 1.84-3.18c.16-.31.04-.69-.26-.85a.637.637 0 0 0-.83.22l-1.88 3.24a11.46 11.46 0 0 0-8.94 0L5.65 5.67a.643.643 0 0 0-.87-.2c-.28.18-.37.54-.22.83L6.4 9.48A10.78 10.78 0 0 0 1 18h22a10.78 10.78 0 0 0-5.4-8.52M7 15.25a1.25 1.25 0 1 1 0-2.5 1.25 1.25 0 0 1 0 2.5m10 0a1.25 1.25 0 1 1 0-2.5 1.25 1.25 0 0 1 0 2.5" />
    </svg>
    <svg v-else-if="platform === 'macos'" viewBox="0 0 24 24" aria-hidden="true">
      <path d="M12.152 6.896c-.948 0-2.415-1.078-3.96-1.04-2.04.027-3.91 1.183-4.961 3.014-2.117 3.675-.546 9.103 1.519 12.09 1.013 1.454 2.208 3.09 3.792 3.039 1.52-.065 2.09-.987 3.935-.987 1.831 0 2.35.987 3.96.948 1.637-.026 2.676-1.48 3.676-2.948 1.156-1.688 1.636-3.325 1.662-3.415-.039-.013-3.182-1.221-3.22-4.857-.026-3.04 2.48-4.494 2.597-4.559-1.429-2.09-3.623-2.324-4.39-2.376-2-.156-3.675 1.09-4.61 1.09zM15.53 3.83c.843-1.012 1.4-2.427 1.245-3.83-1.207.052-2.662.805-3.532 1.818-.78.896-1.454 2.338-1.273 3.714 1.338.104 2.715-.688 3.559-1.701" />
    </svg>
    <svg v-else-if="platform === 'kylin'" viewBox="0 0 16.7 16.7" aria-hidden="true">
      <path d="M7.94.07 5.81 2.25l-.54-.06-.44 1.77 2.22-1.17-.49-2.03.89-2.72M4.83 3.96l1.35 1.56 1.36-.69L5.81 2.25 3.89 2.96l.42 3.75L2.14 4.03l-.89 1.82L.03 7.93l1.15 2.84.37 2.24-.77.92-.81.98h2.21l.28-1.99.89-1.36 1.48.25 1.64-.65 1.21-.62 1.18.79-1.42 1.69-1.45 1.37 2.28.03.01-.93 1.73-1.37.63.8-1.82 2.59 2.32.01-.08-1.3 1.66-1.57-.85-1.72-.1-2.12 1.7-1 .96.2 1.31-.49.37-2.63-2.04.6-1.24 1.86-2.09 1.11-2.92-1.18-2.13-1.58 1.39-1.95-.49-2.04-2.71.87-2.72M2.05 6.32v2.13l3.13.63-.87-2.37-2.26-.39m3.13 2.76-.37 1.83 3.48-.73-.01-1.82-3.1.72m3.1 1.1 2.3.94.61-1.57-2.91.63m2.91-.63 1.02.35 1.07-1.46-2.09 1.11m2.09-1.11.63.46.61-2.32-1.24 1.86" />
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

const props = defineProps<{
  device: DevicePlatformSource | null
  tone?: 'offline' | 'online' | 'connecting' | 'connected'
}>()
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
.platform-icon.tone-offline { color: var(--fluent-text-tertiary); }
.platform-icon.tone-online,
.platform-icon.tone-connected { color: var(--status-online); }
.platform-icon.tone-connecting {
  color: #f59e0b;
  animation: platform-status-pulse 1.5s var(--fluent-easing) infinite;
}
.platform-icon[class*='tone-'] .accent { fill: currentColor; }

@keyframes platform-status-pulse {
  0%, 100% { opacity: 1; }
  50% { opacity: 0.52; }
}
</style>
