<template>
  <div v-loading="loading" class="network-settings">
    <el-alert v-if="loadError" :title="loadError" type="error" :closable="false" />
    <el-button v-if="loadError" @click="load">{{ t('common.retry') }}</el-button>
    <template v-if="section === 'connection'">
      <div v-for="key in ['prefer_ipv6', 'prefer_tcp'] as const" :key="key" class="preference-row">
        <div><label :for="key">{{ t(`app.settings.${key}`) }}</label><p>{{ t(`app.settings.${key}_hint`) }}</p></div>
        <el-switch :id="key" v-model="preferences[key]" :disabled="!loaded || saving" :loading="saving" @change="savePreferences" />
      </div>
      <p class="settings-hint">{{ t('app.settings.connection_scope') }}</p>
    </template>
    <template v-else>
      <div class="preference-row">
        <label for="lan-enabled">{{ t('devices.detail.lan_access.section') }}</label>
        <el-switch id="lan-enabled" v-model="lan.enabled" :disabled="!loaded || saving" />
      </div>
      <template v-if="lan.enabled">
        <label for="lan-cidrs">{{ t('devices.detail.lan_access.cidr_label') }}</label>
        <p class="settings-hint">{{ t('devices.detail.lan_access.cidr_hint') }}</p>
        <el-input id="lan-cidrs" v-model="lan.cidrsText" type="textarea" :rows="5" :disabled="!loaded || saving" :placeholder="t('devices.detail.lan_access.cidr_placeholder')" />
      </template>
      <el-button v-if="dirty" class="save-lan" type="primary" :loading="saving" @click="saveLan">{{ t('common.save') }}</el-button>
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
const preferences = reactive({ prefer_ipv6: false, prefer_tcp: false })
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
async function savePreferences() {
  saving.value = true
  try {
    Object.assign(preferences, await invoke<typeof preferences>('save_connection_preferences', { preferIpv6: preferences.prefer_ipv6, preferTcp: preferences.prefer_tcp }))
    savedPreferences = { ...preferences }
  } catch (error) { Object.assign(preferences, savedPreferences); ElMessage.error(String(error)) }
  finally { saving.value = false }
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
  try { await ElMessageBox.confirm(t('app.settings.discard_lan'), t('app.settings.title'), { confirmButtonText: t('common.confirm'), cancelButtonText: t('common.cancel') }); return true }
  catch { return false }
}
defineExpose({ confirmClose })
onMounted(load)
</script>

<style scoped>
.network-settings { min-height: 120px; }
.preference-row { display: flex; align-items: center; justify-content: space-between; gap: 24px; padding: 12px 0; }
.preference-row + .preference-row { border-top: 1px solid var(--fluent-divider); }
.preference-row p, .settings-hint { color: var(--fluent-text-secondary); font-size: 13px; line-height: 1.6; white-space: pre-line; margin: 8px 0; }
.preference-row .el-switch { flex-shrink: 0; }
.save-lan { margin-top: 16px; }
</style>
