<template>
  <div class="remote-connection-page">
    <div class="remote-task-column">
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

    </section>
    </div>

    <section class="device-detail-panel fluent-card" aria-live="polite">
      <template v-if="contextDevice">
        <div class="detail-header-card">
          <div class="detail-header-main">
            <div class="detail-title-row">
              <div class="device-avatar" :class="contextDevice.status === 'online' ? 'online' : 'offline'">
                <el-icon><Monitor /></el-icon>
              </div>
              <div>
                <span class="context-label">{{ contextLabel }}</span>
                <div class="detail-title-line">
                  <h3>{{ contextDevice.device_alias || contextDevice.device_name }}</h3>
                  <el-tag v-if="!verifiedDevice" size="small" type="primary">{{ $t('common.current_device') }}</el-tag>
                  <el-tag type="info" size="small" :class="{ 'device-online-tag': contextDevice.status === 'online' }">
                    {{ contextDevice.status === 'online' ? $t('devices.detail.online') : $t('devices.detail.offline') }}
                  </el-tag>
                </div>
                <p class="detail-subtitle">{{ contextDevice.device_name }} · {{ contextDevice.device_type }}</p>
              </div>
            </div>
          </div>
          <el-button
            v-if="activeJob && (activeJob.state === 'running' || activeJob.state === 'waiting')"
            class="disconnect-button"
            @click="cancelActiveTunnelJob"
          >{{ $t('remote.connect.cancel') }}</el-button>
        </div>

        <div class="detail-grid">
          <div class="detail-section action-section">
            <div class="section-title">{{ $t('devices.detail.connection.section') }}</div>
            <div class="tunnel-lifecycle-strip" :class="`state-${contextConnectionState}`">
              <span class="lifecycle-dot"></span>
              <div>
                <strong>{{ contextStatusTitle }}</strong>
                <p>{{ contextStatusDescription }}</p>
              </div>
            </div>

            <div v-if="verifiedDevice" class="action-groups">
              <div class="action-group">
                <div class="action-group-label">{{ $t('devices.detail.connection.remote_group') }}</div>
                <div class="action-grid">
                  <button
                    type="button"
                    class="action-tile"
                    :disabled="activeJob?.state !== 'succeeded' || !activeTunnelAddress"
                    @click="copyActiveTunnelAddress"
                  >
                    <el-icon><CopyDocument /></el-icon>
                    <span>{{ $t('remote.connect.copy_address') }}</span>
                  </button>
                  <button
                    type="button"
                    class="action-tile"
                    :disabled="activeJob?.state !== 'succeeded'"
                    @click="cancelActiveTunnelJob"
                  >
                    <el-icon><Close /></el-icon>
                    <span>{{ $t('devices.detail.connection.disconnect') }}</span>
                  </button>
                </div>
              </div>
            </div>
          </div>

          <div v-if="activeJob?.state === 'succeeded'" class="detail-section result-section">
            <div class="section-title">{{ $t('remote.connect.result_label') }}</div>
            <code v-if="activeTunnelAddress" class="connection-address">{{ activeTunnelAddress }}</code>
            <p v-else class="result-hint">{{ $t('remote.connect.no_address') }}</p>
            <p class="result-hint">{{ activeTunnelResultHint }}</p>
            <p v-if="activeJob?.result?.remote_protocol === 'rdp'" class="result-hint">
              {{ $t('remote.connect.other_tool_hint_prefix') }}
              <strong>{{ activeTunnelPort }}</strong>
              {{ $t('remote.connect.other_tool_hint_infix') }}
              <strong>21201</strong>{{ $t('remote.connect.other_tool_hint_suffix') }}
            </p>
          </div>

          <div class="detail-section info-section">
            <div class="section-title">{{ $t('devices.detail.info.section') }}</div>
            <div class="info-grid">
              <div class="info-item">
                <span class="info-label">{{ $t('devices.detail.info.device_name') }}</span>
                <span class="info-value">{{ contextDevice.device_name }}</span>
              </div>
              <div class="info-item">
                <span class="info-label">{{ $t('devices.detail.info.device_type') }}</span>
                <span class="info-value">{{ contextDevice.device_type }}</span>
              </div>
              <div class="info-item">
                <span class="info-label">{{ $t('devices.detail.info.system_version') }}</span>
                <span class="info-value">{{ contextDevice.system_version || '-' }}</span>
              </div>
              <div class="info-item">
                <span class="info-label">{{ $t('devices.detail.info.client_version') }}</span>
                <span class="info-value">{{ contextDevice.client_version || '-' }}</span>
              </div>
            </div>
          </div>
        </div>
      </template>

      <div v-else class="empty-detail">
        <el-empty :description="$t('remote.invite.device_missing')" />
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
import { Close, CopyDocument, Monitor } from '@element-plus/icons-vue'
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
    remote_address?: string
    remote_protocol?: string
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
const activeTunnelAddress = computed(() => activeJob.value?.result?.remote_address || activeJob.value?.result?.rdp_address || '')
const activeTunnelResultHint = computed(() => activeJob.value?.result?.remote_protocol === 'vnc'
  ? t('remote.connect.vnc_result_hint')
  : t('remote.connect.result_hint'))
// 其他工具提示引用地址中的实际端口。
const activeTunnelPort = computed(() => activeTunnelAddress.value.split(':').pop() || '3389')
const contextDevice = computed(() => verifiedDevice.value || localDevice.value)
const contextLabel = computed(() => verifiedDevice.value
  ? t('remote.context.remote_device')
  : t('remote.context.local_device'))
const contextConnectionState = computed(() => {
  if (!verifiedDevice.value) return 'ready'
  return activeJob.value?.state || 'running'
})
const contextStatusTitle = computed(() => {
  if (!verifiedDevice.value) return t('remote.context.ready_title')
  return activeJobTitle.value || t('remote.context.verified_title')
})
const contextStatusDescription = computed(() => {
  if (!verifiedDevice.value) return t('remote.context.ready_description')
  return activeJobMessage.value || t('remote.context.verified_description')
})

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
    [
      t('remote.invite.share_title'),
      '',
      `${t('remote.invite.device_code_label')}: ${inviteDeviceCode.value}`,
      `${t('remote.invite.temp_password_label')}: ${inviteForm.temporaryPassword}`,
      '',
      `${t('remote.invite.share_guide_title')}:`,
      t('remote.invite.share_guide_open'),
      t('remote.invite.share_guide_connect'),
      '',
      t('remote.invite.share_security_tip'),
    ].join('\n'),
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
  grid-template-columns: minmax(340px, 410px) minmax(460px, 1fr);
  gap: 16px;
  align-items: stretch;
  overflow: hidden;
}

.remote-task-column {
  min-width: 0;
  overflow-y: auto;
  overscroll-behavior: contain;
  display: grid;
  align-content: start;
  gap: 16px;
}

/* 卡片视觉由 .fluent-card 工具类提供，此处仅保留尺寸 */
.remote-card {
  min-width: 0;
  padding: 24px;
}

.device-detail-panel {
  min-width: 0;
  min-height: 0;
  padding: 0 24px 24px;
  overflow-y: auto;
  overscroll-behavior: contain;
}

.detail-header-card {
  position: sticky;
  z-index: 2;
  top: 0;
  display: flex;
  align-items: center;
  justify-content: space-between;
  gap: 16px;
  padding: 22px 0 20px;
  border-bottom: 1px solid var(--fluent-divider);
  background: color-mix(in srgb, var(--fluent-layer) 94%, transparent);
  backdrop-filter: blur(18px) saturate(150%);
}

.detail-header-main,
.detail-title-row > div:last-child { min-width: 0; }

.detail-title-row {
  display: flex;
  align-items: center;
  gap: 16px;
}

.device-avatar {
  width: 48px;
  height: 48px;
  border-radius: var(--fluent-radius-lg);
  display: flex;
  align-items: center;
  justify-content: center;
  flex-shrink: 0;
  font-size: 24px;
}

.device-avatar.offline {
  color: var(--fluent-text-tertiary);
  background: var(--fluent-layer-hover);
}

.device-avatar.online {
  color: var(--status-online);
  background: var(--status-online-bg);
}

.context-label {
  display: block;
  margin-bottom: 4px;
  color: var(--fluent-text-secondary);
  font-size: 12px;
  font-weight: 600;
  letter-spacing: 0.03em;
}

.detail-title-line {
  display: flex;
  align-items: center;
  gap: 8px;
  flex-wrap: wrap;
}

.detail-title-line h3 {
  margin: 0;
  color: var(--fluent-text);
  font-size: 22px;
  font-weight: 600;
  letter-spacing: -0.015em;
}

.detail-subtitle {
  margin: 5px 0 0;
  overflow: hidden;
  color: var(--fluent-text-secondary);
  font-size: 14px;
  text-overflow: ellipsis;
  white-space: nowrap;
}

.device-online-tag {
  --el-tag-text-color: var(--status-online);
  --el-tag-bg-color: var(--status-online-bg);
  --el-tag-border-color: color-mix(in srgb, var(--status-online) 36%, transparent);
}

.disconnect-button:active { transform: scale(0.97); }

.detail-grid {
  display: grid;
  grid-template-columns: 1fr;
}

.detail-section {
  padding: 22px 0;
  border-bottom: 1px solid var(--fluent-divider);
}

.detail-section:last-child { border-bottom: 0; }

.section-title {
  margin-bottom: 14px;
  color: var(--fluent-text);
  font-size: 16px;
  font-weight: 600;
}

.tunnel-lifecycle-strip {
  display: flex;
  align-items: flex-start;
  gap: 10px;
  margin-bottom: 14px;
  padding: 12px 14px;
  border: 1px solid var(--fluent-stroke);
  border-left: 4px solid var(--fluent-text-tertiary);
  border-radius: var(--fluent-radius-md);
  background: var(--fluent-layer-hover);
}

.lifecycle-dot {
  width: 9px;
  height: 9px;
  margin-top: 5px;
  border-radius: 50%;
  flex-shrink: 0;
  background: var(--fluent-text-tertiary);
}

.tunnel-lifecycle-strip strong { display: block; color: var(--fluent-text); }
.tunnel-lifecycle-strip p { margin: 3px 0 0; color: var(--fluent-text-secondary); font-size: 13px; }
.tunnel-lifecycle-strip.state-ready,
.tunnel-lifecycle-strip.state-succeeded {
  border-left-color: var(--fluent-success);
  background: var(--fluent-success-bg);
}
.tunnel-lifecycle-strip.state-ready .lifecycle-dot,
.tunnel-lifecycle-strip.state-succeeded .lifecycle-dot { background: var(--fluent-success); }
.tunnel-lifecycle-strip.state-running {
  border-left-color: var(--fluent-accent);
  background: var(--fluent-info-bg);
}
.tunnel-lifecycle-strip.state-running .lifecycle-dot { background: var(--fluent-accent); }
.tunnel-lifecycle-strip.state-waiting {
  border-left-color: var(--fluent-warning);
  background: var(--fluent-warning-bg);
}
.tunnel-lifecycle-strip.state-waiting .lifecycle-dot { background: var(--fluent-warning); }
.tunnel-lifecycle-strip.state-failed,
.tunnel-lifecycle-strip.state-cancelled { border-left-color: var(--fluent-danger); }

.action-groups,
.action-group { display: grid; gap: 8px; }

.action-group-label {
  color: var(--fluent-text-secondary);
  font-size: 12px;
  font-weight: 600;
  letter-spacing: 0.03em;
}

.action-grid {
  display: grid;
  grid-template-columns: repeat(2, minmax(0, 1fr));
  gap: 10px;
}

.action-tile {
  min-width: 0;
  padding: 12px 14px;
  border: 1px solid var(--fluent-stroke);
  border-radius: var(--fluent-radius-md);
  background: var(--fluent-layer);
  color: var(--fluent-text);
  display: flex;
  align-items: center;
  justify-content: flex-start;
  gap: 10px;
  cursor: pointer;
  transition: transform 100ms ease-out, background-color 160ms ease-out, border-color 160ms ease-out;
}

@media (hover: hover) and (pointer: fine) {
  .action-tile:hover:not(:disabled) {
    border-color: var(--fluent-stroke-strong);
    background: var(--fluent-layer-hover);
  }
}

.action-tile:active:not(:disabled) { transform: scale(0.97); }
.action-tile:disabled { cursor: not-allowed; opacity: 0.45; }
.action-tile .el-icon { flex-shrink: 0; font-size: 20px; }
.action-tile span { min-width: 0; font-size: 14px; font-weight: 600; }

.connection-address {
  display: block;
  width: fit-content;
  max-width: 100%;
  margin-bottom: 10px;
  padding: 8px 10px;
  overflow: hidden;
  border-radius: var(--fluent-radius-sm);
  background: var(--fluent-layer-hover);
  color: var(--fluent-text);
  text-overflow: ellipsis;
}

.info-grid {
  display: grid;
  grid-template-columns: repeat(2, minmax(0, 1fr));
  column-gap: 32px;
  border-top: 1px solid var(--fluent-divider);
}

.info-item {
  min-width: 0;
  min-height: 62px;
  padding: 12px 2px 10px;
  border-bottom: 1px solid var(--fluent-divider);
  display: flex;
  flex-direction: column;
  gap: 5px;
}

.info-label { color: var(--fluent-text-secondary); font-size: 12px; letter-spacing: 0.04em; }
.info-value { color: var(--fluent-text); font-size: 15px; overflow-wrap: anywhere; }
.empty-detail { min-height: 320px; display: flex; align-items: center; justify-content: center; }

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
    overflow: visible;
  }

  .remote-task-column { overflow: visible; }

  .device-detail-panel {
    min-height: 440px;
    overflow: visible;
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

  .detail-header-card {
    position: static;
    align-items: flex-start;
  }

  .info-grid,
  .action-grid { grid-template-columns: 1fr; }
}

@media (prefers-reduced-motion: reduce) {
  .action-tile,
  .disconnect-button { transition: none; }
}

@media (prefers-reduced-transparency: reduce) {
  .detail-header-card {
    background: var(--fluent-layer);
    backdrop-filter: none;
  }
}
</style>
