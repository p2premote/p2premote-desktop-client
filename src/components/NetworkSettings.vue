<template>
  <div v-loading="loading" class="network-settings">
    <el-alert v-if="loadError" :title="loadError" type="error" :closable="false" />
    <el-button v-if="loadError" @click="load">{{ t('common.retry') }}</el-button>
    <template v-if="section === 'connection'">
      <div class="settings-section-header">
        <h2>{{ t('app.settings.group_connection') }}</h2>
        <p>{{ t('app.settings.connection_intro') }}</p>
      </div>
      <div v-for="key in ['prefer_ipv6', 'prefer_tcp'] as const" :key="key" class="preference-row">
        <div><label :for="key">{{ t(`app.settings.${key}`) }}</label><p>{{ t(`app.settings.${key}_hint`) }}</p></div>
        <el-switch :id="key" v-model="preferences[key]" :disabled="!loaded || saving" :loading="savingKey === key" @change="savePreferences(key)" />
      </div>
      <div class="save-status" role="status" aria-live="polite">
        <span v-if="saving">{{ t('app.settings.saving') }}</span>
        <span v-else-if="saveState === 'saved'" class="is-success">{{ t('app.settings.saved') }}</span>
        <span v-else>{{ t('app.settings.connection_scope') }}</span>
      </div>
    </template>
    <template v-else>
      <div class="settings-section-header">
        <h2>{{ t('devices.detail.lan_access.section') }}</h2>
        <p>{{ t('app.settings.lan_intro') }}</p>
      </div>
      <div class="preference-row">
        <div><label for="lan-enabled">{{ t('devices.detail.lan_access.enable') }}</label><p>{{ t('devices.detail.lan_access.enable_hint') }}</p></div>
        <el-switch id="lan-enabled" v-model="lan.enabled" :disabled="!loaded || saving" />
      </div>
      <template v-if="lan.enabled">
        <label class="field-label" for="lan-cidrs">{{ t('devices.detail.lan_access.cidr_label') }}</label>
        <p class="settings-hint">{{ t('devices.detail.lan_access.cidr_hint') }}</p>
        <el-input id="lan-cidrs" v-model="lan.cidrsText" type="textarea" :rows="5" :disabled="!loaded || saving" :placeholder="t('devices.detail.lan_access.cidr_placeholder')" />
        <details class="lan-details">
          <summary>{{ t('devices.detail.lan_access.details_title') }}</summary>
          <p>{{ t('devices.detail.lan_access.details_body') }}</p>
        </details>
      </template>
      <div v-if="dirty" class="save-bar">
        <span>{{ t('app.settings.unsaved_changes') }}</span>
        <el-button type="primary" :loading="saving" @click="saveLan">{{ t('app.settings.save_lan') }}</el-button>
      </div>
    </template>
  </div>
</template>

<script setup lang="ts">
import { computed, onMounted, reactive, ref } from 'vue'
import { useI18n } from 'vue-i18n'
import { ElMessage, ElMessageBox } from 'element-plus'
import { invoke } from '../runtime/bridge'
const props = defineProps<{ section: 'connection' | 'lan' }>()
const { t } = useI18n()
const loading = ref(false), loaded = ref(false), saving = ref(false), loadError = ref('')
const saveState = ref<'idle' | 'saved'>('idle')
let saveStateTimer: ReturnType<typeof window.setTimeout> | undefined
const preferences = reactive({ prefer_ipv6: false, prefer_tcp: false })
const savingKey = ref<keyof typeof preferences | null>(null)
let savedPreferences = { ...preferences }
const lan = reactive({ enabled: false, cidrsText: '' })
let savedLan = { ...lan }
const revision = ref(0)
const dirty = computed(() => { void revision.value; return props.section === 'lan' && (lan.enabled !== savedLan.enabled || lan.cidrsText !== savedLan.cidrsText) })
async function load() {
  loading.value = true; loadError.value = ''; loaded.value = false
  try {
    if (props.section === 'connection') {
      Object.assign(preferences, await invoke<typeof preferences>('get_connection_preferences'))
      savedPreferences = { ...preferences }
    } else {
      const value = await invoke<{ enabled: boolean; cidrs: string[] }>('get_wgvpn_lan_access_config')
      Object.assign(lan, { enabled: value.enabled, cidrsText: value.cidrs.join('\n') })
      savedLan = { ...lan }; revision.value++
    }
    loaded.value = true
  } catch (error) { loadError.value = String(error) }
  finally { loading.value = false }
}
async function savePreferences(key: keyof typeof preferences) {
  saving.value = true
  savingKey.value = key
  saveState.value = 'idle'
  if (saveStateTimer) window.clearTimeout(saveStateTimer)
  try {
    Object.assign(preferences, await invoke<typeof preferences>('save_connection_preferences', { preferIpv6: preferences.prefer_ipv6, preferTcp: preferences.prefer_tcp }))
    savedPreferences = { ...preferences }
    saveState.value = 'saved'
    saveStateTimer = window.setTimeout(() => { saveState.value = 'idle' }, 2000)
  } catch (error) { Object.assign(preferences, savedPreferences); ElMessage.error(String(error)) }
  finally { saving.value = false; savingKey.value = null }
}
async function saveLan() {
  const cidrs = [...new Set(lan.cidrsText.split(/[\n,]/).map(v => v.trim()).filter(Boolean))]
  if (lan.enabled && !cidrs.length) { ElMessage.warning(t('devices.message.lan_access_required')); return }
  saving.value = true
  try {
    const value = await invoke<{ enabled: boolean; cidrs: string[] }>('save_wgvpn_lan_access_config', { enabled: lan.enabled, cidrs })
    Object.assign(lan, { enabled: value.enabled, cidrsText: value.cidrs.join('\n') })
    savedLan = { ...lan }; revision.value++
    ElMessage.success(t('devices.message.lan_saved'))
  } catch (error) { ElMessage.error(t('devices.message.lan_save_failed', { error: String(error) })) }
  finally { saving.value = false }
}
async function confirmClose(): Promise<boolean> {
  if (saving.value) return false
  if (!dirty.value) return true
  try { await ElMessageBox.confirm(t('app.settings.discard_lan'), t('app.settings.discard_title'), { confirmButtonText: t('app.settings.discard_confirm'), cancelButtonText: t('app.settings.keep_editing') }); return true }
  catch { return false }
}
defineExpose({ confirmClose })
onMounted(load)
</script>

<style scoped>
.network-settings { min-height: 120px; }
.settings-section-header { padding-bottom: 12px; }
.settings-section-header h2 { font-size: 16px; font-weight: 600; line-height: 1.35; }
.settings-section-header p { color: var(--fluent-text-secondary); font-size: 13px; line-height: 1.5; margin-top: 4px; }
.preference-row { display: flex; align-items: center; justify-content: space-between; gap: 24px; min-height: 56px; padding: 8px 0; }
.preference-row + .preference-row { border-top: 1px solid var(--fluent-divider); }
.preference-row label, .field-label { font-weight: 500; }
.preference-row p { color: var(--fluent-text-secondary); font-size: 12px; line-height: 1.45; margin-top: 2px; }
.settings-hint { color: var(--fluent-text-secondary); font-size: 13px; line-height: 1.5; margin: 4px 0 10px; }
.preference-row .el-switch { flex-shrink: 0; }
.save-status { min-height: 20px; margin-top: 8px; color: var(--fluent-text-tertiary); font-size: 12px; }
.save-status .is-success { color: var(--fluent-success); }
.field-label { display: block; margin-top: 14px; }
.lan-details { margin-top: 10px; color: var(--fluent-text-secondary); font-size: 12px; line-height: 1.55; }
.lan-details summary { width: max-content; color: var(--fluent-accent-text); cursor: pointer; }
.lan-details p { margin-top: 6px; white-space: pre-line; }
.save-bar { display: flex; align-items: center; justify-content: space-between; gap: 16px; margin-top: 16px; padding-top: 12px; border-top: 1px solid var(--fluent-divider); color: var(--fluent-text-secondary); font-size: 12px; }
</style>
