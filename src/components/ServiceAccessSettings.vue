<template>
  <section v-loading="loading" class="service-access-settings">
    <h2>{{ t('service_access.title') }}</h2>
    <p class="hint">{{ t('service_access.scope') }}</p>
    <el-alert v-if="loadError" :title="loadError" type="error" :closable="false" />
    <el-button v-if="loadError" @click="load">{{ t('common.retry') }}</el-button>
    <template v-if="loaded">
      <label for="service-access-mode">{{ t('service_access.mode') }}</label>
      <el-select id="service-access-mode" v-model="draft.mode" :disabled="saving" :aria-label="t('service_access.mode')">
        <el-option v-for="mode in ['all', 'allowlist', 'deny']" :key="mode" :value="mode" :label="t(`service_access.${mode}`)" />
      </el-select>
      <template v-if="draft.mode === 'allowlist'">
        <p class="hint">{{ t('service_access.port_hint') }}</p>
        <label for="service-access-tcp">{{ t('service_access.tcp') }}</label>
        <el-input id="service-access-tcp" v-model="draft.tcp" :disabled="saving" placeholder="22, 443, 1000-2000" />
        <label for="service-access-udp">{{ t('service_access.udp') }}</label>
        <el-input id="service-access-udp" v-model="draft.udp" :disabled="saving" placeholder="1000-2000" />
        <div class="ping-row"><label for="service-access-ping">{{ t('service_access.ping') }}</label><el-switch id="service-access-ping" v-model="draft.allow_ping" :disabled="saving" /></div>
        <el-alert v-if="!draft.tcp.trim() && !draft.udp.trim()" :title="t('service_access.empty')" type="info" :closable="false" />
      </template>
      <el-alert v-if="draft.mode === 'deny'" :title="t('service_access.deny_hint')" type="info" :closable="false" />
      <p class="hint">{{ t('service_access.effects') }}</p>
      <p class="hint">{{ t('service_access.control_channel') }}</p>
      <el-alert v-if="saveError" :title="saveError" type="error" :closable="false" />
      <div class="save-bar">
        <span role="status" aria-live="polite">{{ dirty ? t('app.settings.unsaved_changes') : t(applied ? 'service_access.applied' : 'service_access.not_applied') }}</span>
        <el-button type="primary" :loading="saving" :disabled="!dirty && applied" @click="save">{{ t('common.save') }}</el-button>
      </div>
    </template>
  </section>
</template>
<script setup lang="ts">
import { computed, onMounted, reactive, ref } from 'vue'
import { useI18n } from 'vue-i18n'
import { ElMessageBox } from 'element-plus'
import { invoke } from '../runtime/bridge'
import { errorMessage } from '../utils/errorMessage'
type Mode = 'all' | 'allowlist' | 'deny'
type PortRange = { start: number; end: number }
type Policy = { mode: Mode; tcp: PortRange[]; udp: PortRange[]; allow_ping: boolean }
type Status = { policy: Policy; applied: boolean }
const { t } = useI18n()
const loading = ref(false), loaded = ref(false), saving = ref(false), applied = ref(false)
const loadError = ref(''), saveError = ref('')
const draft = reactive({ mode: 'all' as Mode, tcp: '', udp: '', allow_ping: false })
let saved = JSON.stringify(draft)
const revision = ref(0)
const dirty = computed(() => { void revision.value; return JSON.stringify(draft) !== saved })
function portsText(ranges: PortRange[]) { return ranges.map(r => r.start === r.end ? String(r.start) : `${r.start}-${r.end}`).join(', ') }
function accept(status: Status) {
  Object.assign(draft, { mode: status.policy.mode, tcp: portsText(status.policy.tcp), udp: portsText(status.policy.udp), allow_ping: status.policy.allow_ping })
  saved = JSON.stringify(draft); revision.value++; applied.value = status.applied
}
function parsePorts(value: string): PortRange[] {
  if (!value.trim()) return []
  const parts = value.trim().replace(/\s*-\s*/g, '-').split(/[,，\s]+/)
  if (parts.length > 256) throw new Error(t('service_access.too_many'))
  return parts.map(part => {
    const match = /^(\d{1,5})(?:-(\d{1,5}))?$/.exec(part)
    if (!match) throw new Error(t('service_access.invalid_port', { value: part }))
    const start = Number(match[1]), end = Number(match[2] ?? match[1])
    if (start < 1 || end > 65535 || start > end) throw new Error(t('service_access.invalid_port', { value: part }))
    return { start, end }
  })
}
async function load() {
  loading.value = true; loadError.value = ''; loaded.value = false
  try { accept(await invoke<Status>('get_service_access_policy')); loaded.value = true }
  catch (error) { loadError.value = errorMessage(error, t('common.unknown_error')) }
  finally { loading.value = false }
}
async function save() {
  saveError.value = ''
  let policy: Policy
  try { policy = { mode: draft.mode, tcp: parsePorts(draft.tcp), udp: parsePorts(draft.udp), allow_ping: draft.allow_ping } }
  catch (error) { saveError.value = (error as Error).message; return }
  saving.value = true
  try { accept(await invoke<Status>('save_service_access_policy', { policy })) }
  catch (error) { saveError.value = errorMessage(error, t('service_access.save_failed')) }
  finally { saving.value = false }
}
async function confirmClose(): Promise<boolean> {
  if (saving.value) return false
  if (!dirty.value) return true
  try { await ElMessageBox.confirm(t('service_access.discard'), t('app.settings.discard_title'), { confirmButtonText: t('app.settings.discard_confirm'), cancelButtonText: t('app.settings.keep_editing') }); return true }
  catch { return false }
}
defineExpose({ confirmClose })
onMounted(load)
</script>
<style scoped>
.service-access-settings { min-height: 160px; }
h2 { font-size: 16px; font-weight: 600; }
.hint { color: var(--fluent-text-secondary); font-size: 13px; line-height: 1.6; margin: 8px 0 16px; }
label { display: block; font-weight: 500; margin: 14px 0 8px; }
.el-select { width: 100%; }
.ping-row { display: flex; align-items: center; justify-content: space-between; margin-bottom: 16px; }
.save-bar { display: flex; justify-content: space-between; align-items: center; gap: 16px; margin-top: 16px; border-top: 1px solid var(--fluent-divider); padding-top: 16px; font-size: 12px; color: var(--fluent-text-secondary); }
</style>
