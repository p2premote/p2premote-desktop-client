<template>
  <aside class="function-sidebar no-drag" :class="{ 'is-collapsed': collapsed }" :aria-label="$t('app.nav.label')">
    <nav id="workspace-navigation" :aria-label="$t('app.nav.label')">
      <el-tooltip
        v-for="item in items"
        :key="item.workspace"
        :content="$t(item.label)"
        :disabled="!collapsed"
        :trigger="['hover', 'focus']"
        :trigger-keys="[]"
        placement="right"
        :show-after="300"
        :hide-after="0"
      >
        <button
          type="button"
          class="function-nav-item"
          :class="{ active: activeWorkspace === item.workspace }"
          :aria-label="$t(item.label)"
          :aria-current="activeWorkspace === item.workspace ? 'page' : undefined"
          @click="$emit('select', item.workspace, $event)"
        >
          <el-icon aria-hidden="true"><component :is="item.icon" /></el-icon>
          <span class="sidebar-label" aria-hidden="true">{{ $t(item.label) }}</span>
        </button>
      </el-tooltip>
    </nav>

    <div class="sidebar-footer">
      <el-tooltip :content="toggleLabel" :disabled="!collapsed" :trigger="['hover', 'focus']" :trigger-keys="[]" placement="right" :show-after="300" :hide-after="0">
        <button
          type="button"
          class="sidebar-toggle"
          :aria-label="toggleLabel"
          :aria-expanded="!collapsed"
          aria-controls="workspace-navigation"
          @click="toggleSidebar"
        >
          <svg class="sidebar-toggle-icon" viewBox="0 0 20 20" fill="none" aria-hidden="true">
            <rect x="2.5" y="3.5" width="15" height="13" rx="2.5" />
            <path d="M7 3.5v13" />
            <path v-if="collapsed" d="m10.5 7.5 2.5 2.5-2.5 2.5" />
            <path v-else d="m13 7.5-2.5 2.5 2.5 2.5" />
          </svg>
          <span class="sidebar-label" aria-hidden="true">{{ toggleLabel }}</span>
        </button>
      </el-tooltip>
    </div>
  </aside>
</template>

<script setup lang="ts">
import { computed, ref } from 'vue'
import { useI18n } from 'vue-i18n'
import { Connection, Link, Monitor } from '@element-plus/icons-vue'

type Workspace = 'devices' | 'remote' | 'tunnels'
defineProps<{ activeWorkspace: Workspace }>()
defineEmits<{ select: [workspace: Workspace, event: MouseEvent] }>()

const { t } = useI18n()
const storageKey = 'p2premote-sidebar-collapsed'
const savedPreference = localStorage.getItem(storageKey)
const collapsed = ref(savedPreference === null
  ? window.matchMedia('(max-width: 860px)').matches
  : savedPreference === 'true')
const toggleLabel = computed(() => t(collapsed.value ? 'app.nav.expand' : 'app.nav.collapse'))
const items = [
  { workspace: 'devices', label: 'app.nav.devices', icon: Monitor },
  { workspace: 'remote', label: 'app.nav.remote', icon: Connection },
  { workspace: 'tunnels', label: 'app.nav.tunnel', icon: Link },
] as const

function toggleSidebar() {
  collapsed.value = !collapsed.value
  localStorage.setItem(storageKey, String(collapsed.value))
}
</script>

<style scoped>
.function-sidebar {
  box-sizing: border-box;
  width: 152px;
  min-height: 0;
  padding: 8px;
  display: flex;
  flex-direction: column;
  border-right: 1px solid var(--fluent-stroke);
  background: var(--color-bg-sidebar);
  transition: width 220ms cubic-bezier(0.2, 0, 0, 1);
}

.function-sidebar.is-collapsed { width: 64px; }

.function-nav-item,
.sidebar-toggle {
  position: relative;
  width: 100%;
  height: 40px;
  padding: 0 14px;
  display: flex;
  align-items: center;
  gap: 12px;
  overflow: hidden;
  border: 0;
  border-radius: var(--fluent-radius-md);
  background: transparent;
  color: var(--fluent-text-secondary);
  font: inherit;
  font-size: 14px;
  text-align: left;
  cursor: pointer;
  transition: background-color 120ms ease-out, color 120ms ease-out;
}

.function-nav-item { margin-bottom: 4px; }

.function-nav-item .el-icon,
.sidebar-toggle-icon {
  width: 20px;
  height: 20px;
  flex: 0 0 20px;
  font-size: 18px;
}

.sidebar-toggle-icon {
  stroke: currentColor;
  stroke-width: 1.4;
  stroke-linecap: round;
  stroke-linejoin: round;
}

.sidebar-label {
  white-space: nowrap;
  transition: opacity 120ms ease-out, transform 220ms cubic-bezier(0.2, 0, 0, 1);
}

.is-collapsed .sidebar-label {
  opacity: 0;
  transform: translateX(-4px);
  pointer-events: none;
}

.function-nav-item:hover,
.sidebar-toggle:hover {
  background: var(--fluent-layer-hover);
  color: var(--fluent-text);
}

.function-nav-item:active,
.sidebar-toggle:active { background: var(--fluent-layer-pressed); }

.function-nav-item:focus-visible,
.sidebar-toggle:focus-visible {
  outline: 2px solid var(--color-focus);
  outline-offset: -2px;
}

.function-nav-item.active {
  background: var(--fluent-accent-light);
  color: var(--fluent-accent);
  font-weight: 600;
}

.function-nav-item.active::before {
  content: '';
  position: absolute;
  left: 0;
  width: 3px;
  height: 18px;
  border-radius: var(--fluent-radius-pill);
  background: var(--fluent-accent);
}

.sidebar-footer {
  margin-top: auto;
  padding-top: 8px;
  border-top: 1px solid var(--fluent-stroke);
}

.sidebar-toggle { font-size: 12px; }

@media (prefers-reduced-motion: reduce) {
  .function-sidebar,
  .sidebar-label { transition: none; }
  .is-collapsed .sidebar-label { transform: none; }
}

@media (prefers-contrast: more) {
  .function-sidebar { border-right-color: var(--fluent-text-secondary); }
  .function-nav-item.active { outline: 1px solid currentColor; outline-offset: -1px; }
}
</style>
