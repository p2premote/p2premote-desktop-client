<template>
  <div class="remote-connection-page">
    <section class="remote-card invite-card fluent-card">
      <div class="card-heading">
        <div>
          <h2>{{ $t('remote.invite.title') }}</h2>
        </div>
      </div>

      <div v-if="localDevice" class="invite-content">
        <div class="share-field">
          <span class="share-label">{{ $t('remote.invite.device_code_label') }}</span>
          <div class="share-value-row">
            <el-input :model-value="inviteDeviceCodeDisplay" readonly size="large" />
          </div>
        </div>

        <div class="share-field">
          <span class="share-label">{{ $t('remote.invite.temp_password_label') }}</span>
          <div class="share-value-row password-row">
            <el-input
              :model-value="temporaryPasswordDisplay"
              size="large"
              readonly
            />
          </div>
          <p class="share-hint">{{ $t('remote.invite.temp_password_hint') }}</p>
        </div>

        <el-button
          class="primary-action"
          type="primary"
          :disabled="!inviteDeviceCode || !inviteForm.temporaryPassword || savingPassword"
          @click="copyInviteInfo"
        >
          <span>{{ $t('remote.invite.copy_invite') }}</span>
        </el-button>
      </div>

      <el-empty v-else :description="$t('remote.invite.device_missing')">
        <el-button type="primary" :loading="loadingLocalDevice" @click="refreshLocalDevice">{{ $t('remote.invite.retry') }}</el-button>
      </el-empty>
    </section>

    <section class="remote-card connect-card fluent-card">
      <div class="card-heading">
        <div>
          <h2>{{ $t('remote.connect.title') }}</h2>
        </div>
      </div>

      <el-form
        ref="formRef"
        :model="form"
        :rules="rules"
        label-position="top"
        class="remote-form"
        @submit.prevent
      >
        <el-form-item :label="$t('remote.connect.invite_info_label')" prop="inviteInfo">
          <el-input
            v-model="form.inviteInfo"
            type="textarea"
            :rows="5"
            :placeholder="inviteInfoPlaceholder"
            maxlength="200"
            show-word-limit
          />
        </el-form-item>

        <el-button
          class="primary-action"
          type="primary"
          :loading="connecting"
          @click="handleConnect"
        >
          <span>{{ $t('remote.connect.action') }}</span>
        </el-button>
      </el-form>

      <div v-if="verifiedDevice" class="verified-device">
        <span class="device-dot"></span>
        <div>
          <strong>{{ verifiedDevice.device_alias || verifiedDevice.device_name }}</strong>
          <p>{{ verifiedDevice.device_type }} · {{ verifiedDevice.system_version || $t('common.unknown_system') }}</p>
        </div>
      </div>

      <div v-if="activeJob" class="job-status-card" :class="`job-${activeJob.state}`">
        <div>
          <strong>{{ activeJobTitle }}</strong>
          <p>{{ activeJobMessage }}</p>
        </div>
        <el-button
          v-if="activeJob.state === 'running' || activeJob.state === 'waiting'"
          size="small"
          @click="cancelActiveTunnelJob"
        >
          {{ $t('remote.connect.cancel') }}
        </el-button>
      </div>

      <div v-if="activeJob?.state === 'succeeded'" class="tunnel-result-card">
        <div class="result-header">
          <div>
            <span class="result-label">{{ $t('remote.connect.result_label') }}</span>
            <code v-if="activeTunnelAddress">{{ activeTunnelAddress }}</code>
            <p v-else>{{ $t('remote.connect.no_address') }}</p>
          </div>
          <el-button
            v-if="activeTunnelAddress"
            size="small"
            type="primary"
            plain
            @click="copyActiveTunnelAddress"
          >
            {{ $t('remote.connect.copy_address') }}
          </el-button>
        </div>
        <p class="result-hint">{{ $t('remote.connect.result_hint') }}</p>
        <p class="result-hint">
          {{ $t('remote.connect.other_tool_hint_prefix') }}
          <strong>-3389.gonc.cc</strong>
          {{ $t('remote.connect.other_tool_hint_infix') }}<strong>3389</strong>{{ $t('remote.connect.other_tool_hint_suffix') }}<strong>21118</strong>。
        </p>
      </div>
    </section>

  </div>
</template>

<script setup lang="ts">
import { computed, onMounted, onUnmounted, reactive, ref } from 'vue'
import { useI18n } from 'vue-i18n'
import { invoke, listen, type UnlistenFn } from '../runtime/bridge'
import { ElMessage } from 'element-plus/es/components/message/index.mjs'
import type { FormInstance, FormRules } from 'element-plus/es/components/form/index.mjs'
import { useDeviceStore, type DeviceInfo } from '../stores/device'
const { t } = useI18n()

interface AnonymousConnectResponse {
  success: boolean
  message: string
  device?: DeviceInfo
}

interface BackgroundServiceStatus {
  service?: {
    running: boolean
  }
  runtime?: {
    logged_in: boolean
    ws_connected: boolean
    current_device?: DeviceInfo | null
    invite_temporary_password?: string | null
    active_tunnel_jobs?: ActiveTunnelJobStatus[]
  } | null
}

interface ActiveTunnelJobStatus {
  target_device_id: number
  target_device_uuid: string
  state: 'running' | 'waiting' | 'succeeded' | 'failed' | 'cancelled'
  attempt: number
  max_attempts: number
  message: string
  updated_at: number
  result?: {
    success: boolean
    local_port: number
    rdp_address: string
  }
}

const deviceStore = useDeviceStore()
const formRef = ref<FormInstance>()
const connecting = ref(false)
const verifiedDevice = ref<DeviceInfo | null>(null)
const localDevice = ref<DeviceInfo | null>(null)
const loadingLocalDevice = ref(false)
const generatingCode = ref(false)
const savingPassword = ref(false)
const activeJob = ref<ActiveTunnelJobStatus | null>(null)
let unlistenServiceStatus: UnlistenFn | null = null

const form = reactive({
  inviteInfo: '',
})

const inviteForm = reactive({
  temporaryPassword: '',
})
const passwordError = ref('')

const inviteDeviceCode = computed(() => localDevice.value?.connect_code || '')
const inviteDeviceCodeDisplay = computed(() => {
  if (inviteDeviceCode.value) return inviteDeviceCode.value
  return generatingCode.value ? t('remote.status.generating_code') : t('remote.status.waiting_code')
})
const temporaryPasswordDisplay = computed(() => {
  if (inviteForm.temporaryPassword) {
    return inviteForm.temporaryPassword
  }
  if (passwordError.value) {
    return t('remote.message.password_generate_error', { error: passwordError.value })
  }
  return savingPassword.value ? t('remote.status.generating_password') : t('remote.status.waiting_password')
})
const activeJobTitle = computed(() => {
  if (!activeJob.value) return ''
  return t(`remote.status.${activeJob.value.state}`)
})
const activeJobMessage = computed(() => {
  if (!activeJob.value) return ''
  const job = activeJob.value
  const prefix = job.state === 'succeeded' || job.state === 'failed' || job.state === 'cancelled'
    ? ''
    : t('remote.progress.attempt_prefix', { attempt: job.attempt, max: job.max_attempts })
  return `${prefix}${job.message || t('remote.progress.waiting_service_default')}`
})
const activeTunnelAddress = computed(() => activeJob.value?.result?.rdp_address || '')

const inviteInfoPlaceholder = computed(() =>
  `${t('remote.connect.invite_info_placeholder')}\n${t('remote.connect.invite_info_example')}`
)

async function validateInviteInfo(_rule: unknown, value: unknown, callback: (error?: Error) => void) {
  try {
    const parsed = await invoke('parse_invite_info', { input: String(value || '') })
    callback(parsed ? undefined : new Error(t('remote.message.invite_invalid')))
  } catch {
    callback(new Error(t('remote.message.invite_invalid')))
  }
}

const rules = computed<FormRules>(() => ({
  inviteInfo: [
    { required: true, message: t('remote.connect.invite_info_placeholder'), trigger: 'blur' },
    { validator: validateInviteInfo, trigger: 'blur' },
  ],
}))

onMounted(() => {
  void refreshLocalDevice()
  listen<BackgroundServiceStatus['runtime']>('service-status-changed', (event) => {
    // 状态广播每次心跳都会携带该字段，只在密码实际变化时才更新；
    // 首次展示（挂载恢复或本地生成）不弹提示，仅被消费后轮换时提示
    const nextPassword = event.payload?.invite_temporary_password
    if (nextPassword && nextPassword !== inviteForm.temporaryPassword) {
      const rotated = Boolean(inviteForm.temporaryPassword)
      inviteForm.temporaryPassword = nextPassword
      passwordError.value = ''
      if (rotated) {
        ElMessage.success(t('remote.message.password_changed'))
      }
    }
    updateActiveJobFromList(event.payload?.active_tunnel_jobs)
  }).then(fn => {
    unlistenServiceStatus = fn
  }).catch(error => {
    console.warn('[RemoteConnection] listen service status failed:', error)
  })
  window.addEventListener('p2p-active-tunnel-job-updated', handleActiveTunnelJobEvent)
})

onUnmounted(() => {
  unlistenServiceStatus?.()
  unlistenServiceStatus = null
  window.removeEventListener('p2p-active-tunnel-job-updated', handleActiveTunnelJobEvent)
})

async function refreshLocalDevice() {
  loadingLocalDevice.value = true
  try {
    const serviceStatus = await invoke<BackgroundServiceStatus>('get_service_status')
    const serviceDevice = serviceStatus.runtime?.current_device || null
    if (!serviceDevice) {
      localDevice.value = null
      return
    }
    localDevice.value = serviceDevice
    deviceStore.setDevices([
      ...deviceStore.devices.filter(device => device.device_id !== serviceDevice.device_id),
      serviceDevice,
    ])

    // 服务端已生效的临时密码优先复用，避免每次进入页面重新生成、使已分享未使用的邀请失效
    if (!inviteForm.temporaryPassword && serviceStatus.runtime?.invite_temporary_password) {
      inviteForm.temporaryPassword = serviceStatus.runtime.invite_temporary_password
    }

    if (localDevice.value) {
      await ensureInviteCode(false)
      if (!inviteForm.temporaryPassword) {
        await generateAndSaveTemporaryPassword(false)
      }
    }
  } catch (error) {
    console.error('[RemoteConnection] refresh local device failed:', error)
    ElMessage.error(t('remote.message.device_info_failed'))
  } finally {
    loadingLocalDevice.value = false
  }
}

async function ensureInviteCode(force: boolean) {
  if (!localDevice.value) {
    ElMessage.warning(t('remote.message.device_missing'))
    return
  }
  if (!force && localDevice.value.connect_code) {
    return
  }

  generatingCode.value = true
  try {
    const code = await invoke<string>('generate_connect_code', { deviceId: localDevice.value.device_id })
    localDevice.value.connect_code = code
    const match = deviceStore.devices.find(device => device.device_id === localDevice.value?.device_id)
    if (match) {
      match.connect_code = code
    }
    if (force) {
      ElMessage.success(t('remote.message.code_generated'))
    }
  } catch (error) {
    ElMessage.error(t('remote.message.code_generate_failed', { error }))
  } finally {
    generatingCode.value = false
  }
}

function generateTemporaryPassword(): string {
  const values = new Uint32Array(6)
  crypto.getRandomValues(values)
  return Array.from(values, value => String(value % 10)).join('')
}

async function generateAndSaveTemporaryPassword(showSuccess: boolean) {
  if (!localDevice.value) {
    ElMessage.warning(t('remote.message.device_missing'))
    return
  }

  const password = generateTemporaryPassword()
  savingPassword.value = true
  passwordError.value = ''
  try {
    await invoke('set_device_password', {
      deviceId: localDevice.value.device_id,
      password,
    })
    inviteForm.temporaryPassword = password
    if (showSuccess) {
      ElMessage.success(t('remote.message.password_generated'))
    }
  } catch (error) {
    passwordError.value = typeof error === 'string' ? error : (error as Error)?.message || t('common.unknown_error')
    ElMessage.error(t('remote.message.password_generate_failed', { error: passwordError.value }))
  } finally {
    savingPassword.value = false
  }
}

async function copyText(text: string, successMessage: string) {
  if (!text) {
    ElMessage.warning(t('common.nothing_to_copy'))
    return
  }
  try {
    await navigator.clipboard.writeText(text)
    ElMessage.success(successMessage)
  } catch {
    ElMessage.error(t('common.copy_failed'))
  }
}

async function copyInviteInfo() {
  if (!inviteDeviceCode.value || !inviteForm.temporaryPassword) {
    ElMessage.warning(passwordError.value ? t('remote.message.password_retry') : t('remote.message.password_wait'))
    return
  }
  await copyText(
    `${t('remote.invite.device_code_label')}: ${inviteDeviceCode.value}\n${t('remote.invite.temp_password_label')}: ${inviteForm.temporaryPassword}`,
    t('remote.message.invite_copied')
  )
}

async function copyActiveTunnelAddress() {
  await copyText(activeTunnelAddress.value, t('remote.message.address_copied'))
}

async function handleConnect() {
  verifiedDevice.value = null
  activeJob.value = null

  const valid = await formRef.value?.validate().catch(() => false)
  if (!valid) {
    return
  }

  const parsed = await invoke<{ deviceCode: string; temporaryPassword: string } | null>(
    'parse_invite_info',
    { input: form.inviteInfo },
  )
  if (!parsed) {
    ElMessage.error(t('remote.message.invite_invalid'))
    return
  }

  connecting.value = true
  try {
    const result = await invoke<AnonymousConnectResponse>('start_service_anonymous_active_tunnel', {
      connectCode: parsed.deviceCode,
      temporaryPassword: parsed.temporaryPassword,
    })
    if (!result.success || !result.device) {
      throw new Error(result.message || t('remote.message.device_verify_failed'))
    }

    verifiedDevice.value = result.device
    activeJob.value = null
    const serviceStatus = await invoke<BackgroundServiceStatus>('get_service_status').catch(() => null)
    updateActiveJobFromList(serviceStatus?.runtime?.active_tunnel_jobs)
    ElMessage.success(t('remote.message.auto_tunnel_started'))
  } catch (error) {
    const message = typeof error === 'string' ? error : (error as Error)?.message || t('remote.message.remote_failed')
    ElMessage.error(message)
  } finally {
    connecting.value = false
  }
}

function updateActiveJobFromList(jobs?: ActiveTunnelJobStatus[]) {
  if (!verifiedDevice.value || !Array.isArray(jobs)) return
  const job = jobs.find(item => item.target_device_id === verifiedDevice.value?.device_id)
  if (job) {
    activeJob.value = job
  }
}

function handleActiveTunnelJobEvent(event: Event) {
  if (!verifiedDevice.value) return
  const job = (event as CustomEvent<ActiveTunnelJobStatus>).detail
  if (job?.target_device_id === verifiedDevice.value.device_id) {
    activeJob.value = job
  }
}

async function cancelActiveTunnelJob() {
  if (!verifiedDevice.value) return
  try {
    await invoke('stop_active_tunnel_job', { targetDeviceId: verifiedDevice.value.device_id })
    ElMessage.success(t('remote.message.auto_tunnel_cancelled'))
  } catch (error) {
    const message = typeof error === 'string' ? error : (error as Error)?.message || t('remote.message.cancel_failed')
    ElMessage.error(t('remote.message.auto_tunnel_cancel_failed', { error: message }))
  }
}
</script>

<style scoped>
.remote-connection-page {
  min-height: calc(100% + 40px);
  margin: -20px -32px;
  padding: 16px 20px 20px;
  background: var(--fluent-bg);
  display: grid;
  grid-template-columns: minmax(360px, 1fr) minmax(360px, 1fr);
  gap: 16px;
  align-items: start;
}

/* 卡片视觉由 .fluent-card 工具类提供，此处仅保留尺寸 */
.remote-card {
  min-width: 0;
  padding: 24px;
}

.card-heading {
  display: flex;
  align-items: flex-start;
  justify-content: space-between;
  gap: 20px;
  margin-bottom: 20px;
}

.eyebrow {
  margin: 0 0 8px;
  font-size: 13px;
  color: var(--fluent-text-secondary);
}

.card-heading h2 {
  margin: 0;
  font-size: var(--text-card-title);
  line-height: 1.4;
  color: var(--fluent-text);
  font-weight: 600;
}

.heading-desc {
  margin: 10px 0 0;
  color: var(--fluent-text-secondary);
  font-size: var(--text-body);
  line-height: 1.6;
}

.remote-form,
.invite-content {
  width: 520px;
  max-width: 100%;
}

/* 主操作按钮：对齐 Fluent 主按钮 */
.primary-action {
  min-width: 148px;
  height: 38px;
  margin-top: 10px;
  border-radius: var(--fluent-radius-md);
  font-size: 14px;
  font-weight: 600;
}

.primary-action :deep(span) {
  display: inline-flex;
  align-items: center;
  justify-content: center;
}

/* ===== Fluent InfoBar 风格状态条（左色条 + 淡底 + 细描边） ===== */
.verified-device {
  margin-top: 18px;
  padding: 12px 14px;
  border: 1px solid var(--fluent-stroke);
  border-left: 4px solid var(--fluent-accent);
  border-radius: var(--fluent-radius-md);
  background: var(--fluent-info-bg);
  display: flex;
  align-items: center;
  gap: 12px;
}

.device-dot {
  width: 8px;
  height: 8px;
  border-radius: 50%;
  background: var(--fluent-success);
  box-shadow: 0 0 0 3px var(--fluent-success-bg);
}

.verified-device strong {
  display: block;
  color: var(--fluent-text);
  font-size: 15px;
  font-weight: 600;
}

.verified-device p {
  margin: 4px 0 0;
  color: var(--fluent-text-secondary);
  font-size: 12px;
}

.job-status-card {
  margin-top: 14px;
  padding: 14px 16px;
  border: 1px solid var(--fluent-stroke);
  border-left: 4px solid var(--fluent-accent);
  border-radius: var(--fluent-radius-md);
  background: var(--fluent-info-bg);
  display: flex;
  align-items: center;
  justify-content: space-between;
  gap: 12px;
}

.job-status-card strong {
  display: block;
  color: var(--fluent-text);
  font-size: 15px;
  font-weight: 600;
}

.job-status-card p {
  margin: 4px 0 0;
  color: var(--fluent-text-secondary);
  font-size: 12px;
  line-height: 1.5;
}

.job-succeeded {
  border-left-color: var(--fluent-success);
  background: var(--fluent-success-bg);
}

.job-failed {
  border-left-color: var(--fluent-danger);
  background: var(--fluent-danger-bg);
}

.job-cancelled {
  border-left-color: var(--fluent-text-tertiary);
  background: var(--fluent-layer-hover);
}

.tunnel-result-card {
  margin-top: 14px;
  padding: 16px;
  border: 1px solid var(--fluent-stroke);
  border-left: 4px solid var(--fluent-success);
  border-radius: var(--fluent-radius-md);
  background: var(--fluent-success-bg);
}

.result-header {
  display: flex;
  align-items: flex-start;
  justify-content: space-between;
  gap: 12px;
}

.result-label {
  display: block;
  margin-bottom: 8px;
  color: var(--fluent-success);
  font-size: 13px;
  font-weight: 700;
}

.result-header code {
  display: block;
  max-width: 100%;
  padding: 10px 12px;
  overflow-wrap: anywhere;
  color: var(--fluent-text);
  background: var(--fluent-layer);
  border: 1px solid var(--fluent-stroke);
  border-radius: var(--fluent-radius-md);
  font-size: 13px;
}

.result-header p,
.result-hint {
  margin: 0;
  color: var(--fluent-text-secondary);
  font-size: 12px;
  line-height: 1.6;
}

.result-hint {
  margin-top: 10px;
}

.result-hint strong {
  color: var(--fluent-text);
  font-weight: 700;
}

.share-field {
  display: flex;
  flex-direction: column;
  gap: 8px;
}

.share-label {
  color: var(--fluent-text);
  font-size: 14px;
  font-weight: 600;
}

.share-value-row {
  display: grid;
  grid-template-columns: minmax(0, 1fr);
  gap: 8px;
  align-items: center;
}

.password-row {
  grid-template-columns: minmax(0, 1fr);
}

.share-hint {
  margin: 0;
  color: var(--fluent-text-tertiary);
  font-size: 13px;
  line-height: 1.5;
}

@media (max-width: 1100px) {
  .remote-connection-page {
    grid-template-columns: 1fr;
  }
}

@media (max-width: 900px) {
  .remote-connection-page {
    margin: -20px -32px;
    padding: 14px;
  }

  .share-value-row {
    grid-template-columns: 1fr;
  }

  .result-header {
    flex-direction: column;
  }
}
</style>
