<template>
  <div class="devices-page">
    <div class="devices-shell">
      <aside class="devices-sidebar fluent-card">
        <div class="sidebar-header">
          <div>
            <h2>{{ $t('devices.list.title') }} <span class="count-text">({{ onlineDeviceCount }}/{{ sortedDevices.length }})</span></h2>
          </div>
          <el-button
            class="refresh-button"
            :icon="Refresh"
            @click="refreshDevices({ force: true })"
            :loading="deviceStore.loading"
          >{{ $t('common.refresh') }}</el-button>
        </div>

        <div class="device-list" v-loading="deviceStore.loading">
          <button
            v-for="device in sortedDevices"
            :key="device.device_id"
            type="button"
            class="device-list-item"
            :class="{
              active: selectedDevice?.device_id === device.device_id,
              'tunnel-connected': isTunnelConnected(device),
            }"
            @click="selectDevice(device)"
          >
            <span class="device-state-mark" :class="deviceConnectionState(device)" aria-hidden="true">
              <el-icon v-if="isTunnelConnected(device)" class="device-tunnel-icon"><Link /></el-icon>
              <span v-else class="device-status-dot" :class="deviceConnectionState(device)" />
            </span>
            <div class="device-list-main">
              <div class="device-list-title-row">
                <span class="device-list-title" :title="device.device_alias || device.device_name">{{ device.device_alias || device.device_name }}</span>
                <span v-if="isTunnelConnected(device)" class="device-tunnel-badge">
                  <el-icon aria-hidden="true"><Link /></el-icon>
                  <span>{{ tunnelConnectedLabel(device) }}</span>
                </span>
                <el-tag v-if="isCurrentDevice(device.device_uuid)" size="small" type="primary">{{ $t('common.current_device') }}</el-tag>
              </div>
              <div class="device-list-subtitle">
                <span class="device-system-summary" :title="device.system_version || device.device_type || $t('common.unknown_system')">{{ deviceSystemSummary(device) }}</span>
                <span class="device-ip">{{ device.lan_ip || '0.0.0.0' }}</span>
              </div>
            </div>
          </button>

          <div v-if="sortedDevices.length === 0 && !deviceStore.loading" class="empty-list">
            <el-empty :description="$t('devices.list.empty')" />
          </div>
        </div>
      </aside>

      <section class="device-detail-panel fluent-card">
        <template v-if="selectedDevice">
          <div class="detail-header-card">
            <div class="detail-header-main">
              <div class="detail-title-row">
                <div class="device-avatar" :class="deviceConnectionState(selectedDevice)">
                  <el-icon><Monitor /></el-icon>
                </div>
                <div>
                  <div class="detail-title-line">
                    <h3>{{ selectedDevice.device_alias || selectedDevice.device_name }}</h3>
                    <el-tag v-if="isCurrentDevice(selectedDevice.device_uuid)" size="small" type="primary">{{ $t('common.current_device') }}</el-tag>
                    <el-tag
                      type="info"
                      size="small"
                      :class="{ 'device-online-tag': selectedDevice.status === 'online' }"
                    >
                      {{ selectedDevice.status === 'online' ? $t('devices.detail.online') : $t('devices.detail.offline') }}
                    </el-tag>
                    <el-tag v-if="isTunnelConnected(selectedDevice)" size="small" class="device-tunnel-tag">
                      <el-icon aria-hidden="true"><Link /></el-icon>
                      <span>{{ tunnelConnectedLabel(selectedDevice) }}</span>
                    </el-tag>
                    <el-tag v-if="isWindowsDevice(selectedDevice) && !selectedDevice.rdp_enabled" type="danger" size="small">{{ $t('devices.detail.rdp_not_enabled') }}</el-tag>
                  </div>
                  <p class="detail-subtitle">{{ selectedDevice.device_name }} · {{ selectedDevice.device_type }}</p>
                </div>
              </div>
            </div>
            <el-dropdown trigger="click" @command="(cmd: string) => handleDeviceAction(cmd, selectedDevice!)">
              <el-button text :icon="MoreFilled" class="more-btn" />
              <template #dropdown>
                <el-dropdown-menu>
                  <el-dropdown-item command="alias">{{ $t('devices.menu.set_alias') }}</el-dropdown-item>
                  <!--
                  <el-dropdown-item command="connect-code">生成连接码</el-dropdown-item>
                  <el-dropdown-item command="set-password">设置连接密码</el-dropdown-item>
                  -->
                  <el-dropdown-item command="delete" divided>{{ $t('devices.menu.delete_device') }}</el-dropdown-item>
                </el-dropdown-menu>
              </template>
            </el-dropdown>
          </div>

          <div class="detail-grid">
            <div v-if="isCurrentDevice(selectedDevice.device_uuid)" class="detail-section action-section">
              <div class="section-title-row lan-access-heading">
                <div class="section-title">{{ $t('devices.detail.lan_access.section') }}</div>
                <el-switch v-model="lanAccessForm.enabled" />
              </div>
              <div class="lan-access-card">
                <div v-if="lanAccessForm.enabled" class="lan-cidr-editor">
                  <div class="lan-cidr-content">
                    <div class="lan-cidr-input">
                      <div class="lan-cidr-label">{{ $t('devices.detail.lan_access.cidr_label') }}</div>
                      <el-input
                        v-model="lanAccessForm.cidrsText"
                        type="textarea"
                        :rows="4"
                        resize="none"
                        :placeholder="$t('devices.detail.lan_access.cidr_placeholder')"
                      />
                    </div>
                  </div>
                </div>
                <div v-if="lanAccessDirty" class="lan-access-actions">
                  <el-button type="primary" :loading="lanAccessForm.saving" @click="saveLanAccessConfig">{{ $t('common.save') }}</el-button>
                </div>
              </div>
            </div>

            <div v-else class="detail-section action-section">
              <div class="section-title">{{ $t('devices.detail.connection.section') }}</div>
              <div
                class="tunnel-lifecycle-strip"
                :class="[
                  `state-${deviceTunnelLifecycle(selectedDevice).state}`,
                  { compact: deviceTunnelLifecycle(selectedDevice).state === 'not_established' },
                ]"
              >
                <span class="lifecycle-dot"></span>
                <div>
                  <strong>{{ tunnelLifecycleTitle(selectedDevice) }}</strong>
                  <p>{{ tunnelLifecycleDescription(selectedDevice) }}</p>
                  <p v-if="tunnelLanCidrs(selectedDevice).length">
                    {{ $t('devices.lifecycle.desc_connected_with_lan', { cidrs: tunnelLanCidrs(selectedDevice).join(', ') }) }}
                  </p>
                </div>
              </div>
              <div
                class="action-grid"
                :class="{
                  'single-tunnel-action':
                    !(isWindowsDevice(selectedDevice) && selectedDevice.rdp_enabled)
                    && !tunnelVirtualIp(selectedDevice),
                }"
              >
                <button
                  type="button"
                  class="action-tile primary"
                  :disabled="!canUseTunnelAction(selectedDevice)"
                  :aria-label="`${activeTunnelActionText(selectedDevice)}. ${tunnelActionTooltip(selectedDevice)}`"
                  @click="openTunnelAction(selectedDevice!)"
                >
                  <el-icon><Link /></el-icon>
                  <span>{{ activeTunnelActionText(selectedDevice) }}</span>
                  <el-tooltip
                    :content="tunnelActionTooltip(selectedDevice)"
                    placement="top"
                  >
                    <span class="action-help" aria-hidden="true"><el-icon><InfoFilled /></el-icon></span>
                  </el-tooltip>
                </button>

                <button
                  v-if="isWindowsDevice(selectedDevice) && selectedDevice.rdp_enabled"
                  type="button"
                  class="action-tile"
                  :aria-label="`${$t('devices.detail.connection.remote_desktop')}. ${$t('devices.detail.connection.remote_desktop_tooltip')}`"
                  @click="handleRemoteDesktop(selectedDevice!)"
                >
                  <el-icon><Monitor /></el-icon>
                  <span>{{ $t('devices.detail.connection.remote_desktop') }}</span>
                  <el-tooltip :content="$t('devices.detail.connection.remote_desktop_tooltip')" placement="top">
                    <span class="action-help" aria-hidden="true"><el-icon><InfoFilled /></el-icon></span>
                  </el-tooltip>
                </button>

                <button
                  v-if="tunnelVirtualIp(selectedDevice)"
                  type="button"
                  class="action-tile"
                  @click="copyTunnelVirtualIp(selectedDevice!)"
                >
                  <el-icon><CopyDocument /></el-icon>
                  <span class="action-copy-content">
                    <span>{{ $t('devices.detail.connection.copy_virtual_ip') }}</span>
                    <small>{{ tunnelVirtualIp(selectedDevice) }}</small>
                  </span>
                </button>

                <button
                  v-if="tunnelVirtualIp(selectedDevice)"
                  type="button"
                  class="action-tile"
                  :disabled="speedTestingIds.has(selectedDevice.device_id)"
                  @click="testTunnelSpeed(selectedDevice!)"
                >
                  <el-icon><Odometer /></el-icon>
                  <span>{{ speedTestingIds.has(selectedDevice.device_id)
                    ? $t('devices.detail.connection.speed_testing')
                    : $t('devices.detail.connection.speed_test') }}</span>
                </button>
              </div>
            </div>

            <div class="detail-section info-section">
              <div class="section-title-row">
                <div class="section-title">{{ $t('devices.detail.info.section') }}</div>
                <el-button
                  text
                  class="detail-expand-button"
                  :aria-expanded="detailExpanded"
                  @click="detailExpanded = !detailExpanded"
                >
                  <span class="detail-expand-label">
                    {{ detailExpanded ? $t('devices.detail.info.collapse') : $t('devices.detail.info.expand') }}
                  </span>
                  <el-icon :class="{ expanded: detailExpanded }"><ArrowDown /></el-icon>
                </el-button>
              </div>
              <div class="info-grid">
                <div class="info-item">
                  <span class="info-label">{{ $t('devices.detail.info.device_name') }}</span>
                  <span class="info-value">{{ selectedDevice.device_name }}</span>
                </div>
                <div class="info-item">
                  <span class="info-label">{{ $t('devices.detail.info.device_alias') }}</span>
                  <span class="info-value">{{ selectedDevice.device_alias || $t('common.not_set') }}</span>
                </div>
                <div class="info-item">
                  <span class="info-label">{{ $t('devices.detail.info.device_type') }}</span>
                  <span class="info-value">{{ selectedDevice.device_type }}</span>
                </div>
                <div class="info-item">
                  <span class="info-label">{{ $t('devices.detail.info.public_ip') }}</span>
                  <span class="info-value">
                    {{ selectedDevice.public_ip || '-' }}
                    <small v-if="selectedDevice.public_ip_location" class="public-ip-location">{{ selectedDevice.public_ip_location }}</small>
                  </span>
                </div>
              </div>
              <div v-show="detailExpanded" class="info-grid info-grid-expanded">
                <div class="info-item">
                  <span class="info-label">{{ $t('devices.detail.info.system_version') }}</span>
                  <span class="info-value">{{ selectedDevice.system_version || '-' }}</span>
                </div>
                <div class="info-item">
                  <span class="info-label">{{ $t('devices.detail.info.lan_ip') }}</span>
                  <span class="info-value">{{ selectedDevice.lan_ip || '-' }}</span>
                </div>
                <div class="info-item">
                  <span class="info-label">{{ $t('devices.detail.info.rdp_port') }}</span>
                  <span class="info-value">{{ selectedDevice.service_port }}</span>
                </div>
                <div class="info-item">
                  <span class="info-label">{{ $t('devices.detail.info.device_uuid') }}</span>
                  <span class="info-value uuid">{{ selectedDevice.device_uuid }}</span>
                </div>
                <!--
                <div class="info-item wide">
                  <span class="info-label">连接码</span>
                  <div class="connect-code-inline">
                    <span class="info-value code">{{ selectedDevice.connect_code || currentConnectCode || '未生成' }}</span>
                    <el-button size="small" @click="copyInlineConnectCode">复制</el-button>
                  </div>
                </div>
                -->
              </div>
            </div>
          </div>
        </template>

        <div v-else class="empty-detail">
          <el-empty :description="$t('devices.detail.empty_hint')" />
        </div>
      </section>
    </div>

    <el-dialog v-model="aliasDialogVisible" :title="$t('devices.alias_dialog.title')" width="400px">
      <el-form :model="aliasForm" label-width="90px">
        <el-form-item :label="$t('devices.alias_dialog.label')">
          <el-input v-model="aliasForm.alias" :placeholder="$t('devices.alias_dialog.placeholder')" maxlength="20" show-word-limit />
        </el-form-item>
      </el-form>
      <template #footer>
        <el-button @click="aliasDialogVisible = false">{{ $t('common.cancel') }}</el-button>
        <el-button type="primary" @click="confirmSetAlias" :loading="aliasForm.loading">{{ $t('common.confirm') }}</el-button>
      </template>
    </el-dialog>

    <!--
    <el-dialog v-model="passwordDialogVisible" title="设置连接密码" width="420px">
      <el-form :model="passwordForm" label-width="90px">
        <el-form-item label="连接密码">
          <el-input v-model="passwordForm.password" type="password" placeholder="请输入连接密码" show-password />
        </el-form-item>
        <el-form-item label="确认密码">
          <el-input v-model="passwordForm.confirmPassword" type="password" placeholder="请再次输入密码" show-password />
        </el-form-item>
      </el-form>
      <template #footer>
        <el-button @click="passwordDialogVisible = false">取消</el-button>
        <el-button type="primary" @click="confirmSetPassword" :loading="passwordForm.loading">确定</el-button>
      </template>
    </el-dialog>

    <el-dialog v-model="connectCodeDialogVisible" title="设备连接码" width="420px">
      <div class="connect-code-display">
        <p>设备“{{ selectedDevice?.device_alias || selectedDevice?.device_name }}”的连接码：</p>
        <div class="code-box">{{ currentConnectCode }}</div>
        <p class="code-hint">其他人可以通过连接码和密码远程连接此设备</p>
      </div>
      <template #footer>
        <el-button @click="connectCodeDialogVisible = false">关闭</el-button>
        <el-button type="primary" @click="copyConnectCode">复制连接码</el-button>
      </template>
    </el-dialog>
    -->

  </div>
</template>

<script setup lang="ts">
import { computed, onMounted, onUnmounted, reactive, ref, watch } from 'vue'
import { useI18n } from 'vue-i18n'
import { invoke, listen, openExternal, type UnlistenFn } from '../runtime/bridge'
import { ElMessage } from 'element-plus/es/components/message/index.mjs'
import { ElMessageBox } from 'element-plus/es/components/message-box/index.mjs'
import {
  CopyDocument,
  ArrowDown,
  InfoFilled,
  Link,
  Monitor,
  MoreFilled,
  Odometer,
  Refresh,
} from '@element-plus/icons-vue'
import { useDeviceStore, type DeviceInfo } from '../stores/device'
import { useAuthStore } from '../stores/auth'
import { resolveLaunchableRdpAddress } from '../utils/rdpAddress'
const { t } = useI18n()

interface TunnelSpeedTestResult {
  latency_ms: number
  download_mbps: number
  upload_mbps: number
}

interface TunnelInfo {
  local_port: number
  rdp_address: string
  virtual_ip?: string
  exposed_lan_cidrs?: string[]
  health_state?: 'connected' | 'degraded'
  role?: 'active' | 'passive'
}

interface ActiveTunnelJobStatus {
  target_device_id: number
  target_device_uuid: string
  state: 'running' | 'waiting' | 'succeeded' | 'failed' | 'cancelled'
  attempt: number
  max_attempts: number
  message: string
  result?: {
    success: boolean
    local_port: number
    rdp_address: string
    warning?: string | null
  } | null
  updated_at: number
}

interface WgvpnJobStatus {
  peer_device_id: number
  is_active: boolean
  state: 'running' | 'waiting' | 'succeeded' | 'failed' | 'cancelled'
  attempt: number
  max_attempts: number
  message: string
  updated_at: number
}

interface WgvpnLanAccessConfig {
  enabled: boolean
  cidrs: string[]
}

interface TunnelLifecycleStatus {
  peer_device_id: number
  role: 'active' | 'passive'
  state: 'not_established' | 'connecting' | 'connected' | 'recovering'
  attempt: number
  max_attempts: number
  stage?: string | null
  last_result: 'none' | 'user_disconnected' | 'peer_disconnected' | 'attempt_failed' | 'health_grace_expired' | 'cancelled'
  error_code?: string | null
  message?: string | null
  virtual_ip?: string | null
  peer_virtual_ip?: string | null
}

const deviceStore = useDeviceStore()
const authStore = useAuthStore()

const aliasDialogVisible = ref(false)
/*
const passwordDialogVisible = ref(false)
const connectCodeDialogVisible = ref(false)
*/
const selectedDevice = ref<DeviceInfo | null>(null)
const detailExpanded = ref(false)
/*
const currentConnectCode = ref('')
*/
const currentDeviceUUID = ref('')
const tunnelStatusMap = ref<Record<number, TunnelInfo>>({})
const activeTunnelJobMap = ref<Record<number, ActiveTunnelJobStatus>>({})
const tunnelLifecycleMap = ref<Record<number, TunnelLifecycleStatus>>({})
const preparingTunnelIds = ref(new Set<number>())
const lanAccessForm = reactive({
  enabled: false,
  cidrsText: '',
  saving: false,
})
const savedLanAccessConfig = reactive({
  enabled: false,
  cidrsText: '',
})

const aliasForm = reactive({ alias: '', loading: false })
/*
const passwordForm = reactive({ password: '', confirmPassword: '', loading: false })
*/
const FOREGROUND_REFRESH_THROTTLE_MS = 10_000
let autoRefreshTimer: ReturnType<typeof window.setInterval> | null = null
let lastForegroundRefreshAt = 0
let unlistenServiceStatus: UnlistenFn | null = null

const onlineDeviceCount = computed(() => deviceStore.devices.filter(device => device.status === 'online').length)
const lanAccessDirty = computed(() => (
  lanAccessForm.enabled !== savedLanAccessConfig.enabled
  || lanAccessForm.cidrsText !== savedLanAccessConfig.cidrsText
))
const WINDOWS_10_HOME_HELP_URL = 'https://cloud.tencent.com/developer/article/1445459'
const sortedDevices = computed(() => {
  return [...deviceStore.devices].sort((a, b) => {
    const aCurrent = isCurrentDevice(a.device_uuid) ? 1 : 0
    const bCurrent = isCurrentDevice(b.device_uuid) ? 1 : 0
    if (aCurrent !== bCurrent) {
      return bCurrent - aCurrent
    }

    const aOnline = a.status === 'online' ? 1 : 0
    const bOnline = b.status === 'online' ? 1 : 0
    if (aOnline !== bOnline) {
      return bOnline - aOnline
    }

    const aName = (a.device_alias || a.device_name || '').toLocaleLowerCase()
    const bName = (b.device_alias || b.device_name || '').toLocaleLowerCase()
    return aName.localeCompare(bName, 'zh-Hans-CN')
  })
})

watch(
  sortedDevices,
  (devices) => {
    if (devices.length === 0) {
      selectedDevice.value = null
      return
    }

    const currentSelectedId = selectedDevice.value?.device_id
    const matched = devices.find(device => device.device_id === currentSelectedId)
    selectedDevice.value = matched || devices[0]
  },
  { immediate: true }
)

onMounted(async () => {
  try {
    // 页面首次展示时必须从 service 重新同步设备状态。客户端重启期间，
    // store 里可能仍是 WebSocket 建连前取得的离线快照。
    await refreshDevices({ force: true })
    await loadLanAccessConfig()
  } catch (e) {
    console.error('获取设备列表失败:', e)
  }

  autoRefreshTimer = window.setInterval(() => {
    void autoRefreshDevicesInForeground()
  }, 60_000)

  window.addEventListener('focus', handleForegroundResume)
  document.addEventListener('visibilitychange', handleForegroundResume)
  window.addEventListener('p2p-active-tunnel-job-updated', handleActiveTunnelJobUpdated)
  window.addEventListener('p2p-active-tunnel-job-completed', handleActiveTunnelJobCompleted)
  listen<any>('service-status-changed', (event) => {
    applyCurrentDeviceUUIDFromRuntime(event.payload)
    applyTunnelRuntimeStatus(event.payload)
  }).then(fn => {
    unlistenServiceStatus = fn
  }).catch(error => {
    console.warn('[Devices] listen service status failed:', error)
  })
})

onUnmounted(() => {
  if (autoRefreshTimer !== null) {
    window.clearInterval(autoRefreshTimer)
    autoRefreshTimer = null
  }
  window.removeEventListener('focus', handleForegroundResume)
  document.removeEventListener('visibilitychange', handleForegroundResume)
  window.removeEventListener('p2p-active-tunnel-job-updated', handleActiveTunnelJobUpdated)
  window.removeEventListener('p2p-active-tunnel-job-completed', handleActiveTunnelJobCompleted)
  unlistenServiceStatus?.()
  unlistenServiceStatus = null
})

function isCurrentDevice(deviceUUID: string): boolean {
  return currentDeviceUUID.value !== '' && deviceUUID === currentDeviceUUID.value
}

function deviceSystemSummary(device: DeviceInfo): string {
  const system = (device.system_version || device.device_type || t('common.unknown_system'))
    .replace(/\s*\((?:build\s*)?[^)]*\)\s*/gi, ' ')
    .replace(/\s+/g, ' ')
    .trim()
  return system || t('common.unknown_system')
}

function parseLanCidrs(value: string): string[] {
  const items = value
    .split(/[\n,]/)
    .map(item => item.trim())
    .filter(Boolean)
  return [...new Set(items)]
}

async function loadLanAccessConfig() {
  try {
    const config = await invoke<WgvpnLanAccessConfig>('get_wgvpn_lan_access_config')
    lanAccessForm.enabled = Boolean(config.enabled)
    lanAccessForm.cidrsText = Array.isArray(config.cidrs) ? config.cidrs.join('\n') : ''
    savedLanAccessConfig.enabled = lanAccessForm.enabled
    savedLanAccessConfig.cidrsText = lanAccessForm.cidrsText
  } catch (error) {
    console.warn('[Devices] load LAN access config failed:', error)
  }
}

async function saveLanAccessConfig() {
  const cidrs = parseLanCidrs(lanAccessForm.cidrsText)
  if (lanAccessForm.enabled) {
    if (cidrs.length === 0) {
      ElMessage.warning(t('devices.message.lan_access_required'))
      return
    }
  }

  lanAccessForm.saving = true
  try {
    const saved = await invoke<WgvpnLanAccessConfig>('save_wgvpn_lan_access_config', {
      enabled: lanAccessForm.enabled,
      cidrs,
    })
    lanAccessForm.enabled = Boolean(saved.enabled)
    lanAccessForm.cidrsText = Array.isArray(saved.cidrs) ? saved.cidrs.join('\n') : ''
    savedLanAccessConfig.enabled = lanAccessForm.enabled
    savedLanAccessConfig.cidrsText = lanAccessForm.cidrsText
    ElMessage.success(t('devices.message.lan_saved'))
  } catch (error) {
    const message = typeof error === 'string' ? error : (error as any)?.message || t('devices.message.lan_save_failed_fallback')
    ElMessage.error(t('devices.message.lan_save_failed', { error: message }))
  } finally {
    lanAccessForm.saving = false
  }
}

function hasTunnel(device: DeviceInfo | null): boolean {
  if (!device) return false
  return Boolean(tunnelStatusMap.value[device.device_id])
}

function activeTunnelJob(device: DeviceInfo | null): ActiveTunnelJobStatus | null {
  if (!device) return null
  const job = activeTunnelJobMap.value[device.device_id]
  if (!job || (job.state !== 'running' && job.state !== 'waiting')) return null
  return job
}

function activeTunnelActionText(device: DeviceInfo | null): string {
  if (!device) return t('devices.detail.connection.tunnel_action')
  const lifecycle = deviceTunnelLifecycle(device)
  if (lifecycle.state === 'recovering') {
    return lifecycle.role === 'passive'
      ? t('devices.detail.connection.disconnect_passive')
      : t('devices.detail.connection.retry')
  }
  if (lifecycle.state === 'connected') {
    return lifecycle.role === 'passive'
      ? t('devices.detail.connection.disconnect_passive')
      : t('devices.detail.connection.disconnect')
  }
  if (preparingTunnelIds.value.has(device.device_id)) return t('devices.detail.connection.preparing')
  const job = activeTunnelJob(device)
  if (!job) return t('devices.detail.connection.tunnel_action')
  if (job.state === 'waiting') return t('devices.detail.connection.cancel_waiting', { attempt: job.attempt, max: job.max_attempts })
  return t('devices.detail.connection.cancel_running', { attempt: job.attempt, max: job.max_attempts })
}

function isTunnelDegraded(device: DeviceInfo | null): boolean {
  if (!device) return false
  return tunnelStatusMap.value[device.device_id]?.health_state === 'degraded'
}

function deviceTunnelLifecycle(device: DeviceInfo | null): TunnelLifecycleStatus {
  const existing = device ? tunnelLifecycleMap.value[device.device_id] : undefined
  if (existing) return existing
  const tunnel = device ? tunnelStatusMap.value[device.device_id] : undefined
  if (tunnel) {
    return {
      peer_device_id: device?.device_id || 0,
      role: tunnel.role || 'active',
      state: tunnel.health_state === 'degraded' ? 'recovering' : 'connected',
      attempt: 0,
      max_attempts: 30,
      last_result: 'none',
      peer_virtual_ip: tunnel.virtual_ip,
    }
  }
  const job = activeTunnelJob(device)
  return {
    peer_device_id: device?.device_id || 0,
    role: 'active',
    state: job && (job.state === 'running' || job.state === 'waiting') ? 'connecting' : 'not_established',
    attempt: job?.attempt || 0,
    max_attempts: job?.max_attempts || 30,
    last_result: job?.state === 'failed' ? 'attempt_failed' : job?.state === 'cancelled' ? 'cancelled' : 'none',
    message: job?.message,
  }
}

/// 设备连接状态的视觉呈现：offline/online/connecting/connected。
/// 统一列表状态点与详情头像的颜色语义：
///   - offline 灰：不可达
///   - online 蓝：在线但无隧道活动
///   - connecting 橙(脉冲)：正在建立或重连
///   - connected 绿(光环)：已建立活跃隧道
function deviceConnectionState(device: DeviceInfo | null): 'offline' | 'online' | 'connecting' | 'connected' {
  if (!device || device.status !== 'online') return 'offline'
  const state = deviceTunnelLifecycle(device).state
  if (state === 'connected') return 'connected'
  if (state === 'connecting' || state === 'recovering') return 'connecting'
  return 'online'
}

function isTunnelConnected(device: DeviceInfo | null): boolean {
  return deviceConnectionState(device) === 'connected'
}

function tunnelConnectedLabel(device: DeviceInfo | null): string {
  return deviceTunnelLifecycle(device).role === 'passive'
    ? t('devices.list.passive_tunnel_connected')
    : t('devices.list.active_tunnel_connected')
}

function tunnelLifecycleTitle(device: DeviceInfo | null): string {
  const lifecycle = deviceTunnelLifecycle(device)
  if (lifecycle.state === 'connected') {
    return tunnelConnectedLabel(device)
  }
  const state = lifecycle.state
  return t(`devices.lifecycle.${state}`)
}

function tunnelLifecycleDescription(device: DeviceInfo | null): string {
  const lifecycle = deviceTunnelLifecycle(device)
  if (lifecycle.state === 'connecting') {
    const attempt = lifecycle.attempt || 1
    const suffix = lifecycle.message ? ` · ${lifecycle.message}` : ''
    return t('devices.lifecycle.desc_connecting', { attempt, max: lifecycle.max_attempts || 30, suffix })
  }
  if (lifecycle.state === 'connected') {
    const ip = lifecycle.peer_virtual_ip || tunnelVirtualIp(device)
    return ip ? t('devices.lifecycle.desc_connected_with_ip', { ip }) : t('devices.lifecycle.desc_connected_plain')
  }
  if (lifecycle.state === 'recovering') return lifecycle.message || t('devices.lifecycle.desc_recovering_default')
  if (lifecycle.last_result !== 'none' && lifecycle.message) return lifecycle.message
  return t('devices.lifecycle.desc_none')
}

function tunnelActionTooltip(device: DeviceInfo | null): string {
  const lifecycle = deviceTunnelLifecycle(device)
  if (isTunnelDegraded(device) && lifecycle.role !== 'passive') return t('devices.lifecycle.tooltip_retry')
  return hasTunnel(device) ? t('devices.lifecycle.tooltip_disconnect') : t('devices.lifecycle.tooltip_connect')
}

function canUseTunnelAction(device: DeviceInfo | null): boolean {
  if (!device) return false
  if (isCurrentDevice(device.device_uuid)) return false
  if (preparingTunnelIds.value.has(device.device_id)) return false
  if (deviceTunnelLifecycle(device).state !== 'not_established' || activeTunnelJob(device)) return true
  return device.status === 'online'
}

function isWindowsDevice(device: DeviceInfo | null): boolean {
  if (!device) return false
  const text = `${device.device_type || ''} ${device.system_version || ''}`.toLocaleLowerCase()
  return text.includes('windows') || /(^|\s)win(?:32|64|dows)?(?:\s|$)/.test(text)
}

function tunnelVirtualIp(device: DeviceInfo | null): string {
  if (!device) return ''
  return tunnelStatusMap.value[device.device_id]?.virtual_ip || ''
}

function tunnelLanCidrs(device: DeviceInfo | null): string[] {
  if (!device || deviceTunnelLifecycle(device).state !== 'connected') return []
  return tunnelStatusMap.value[device.device_id]?.exposed_lan_cidrs || []
}

function isWindows10HomeDevice(device: DeviceInfo): boolean {
  const text = `${device.device_type || ''} ${device.system_version || ''}`.toLocaleLowerCase()
  const isWindows = text.includes('windows') || text.includes('win')
  const isWin10 = text.includes('windows 10') || text.includes('win10')
  const isHome = text.includes('home') || text.includes('家庭')
  return isWindows && isWin10 && isHome
}

async function copyTunnelVirtualIp(device: DeviceInfo) {
  const virtualIp = tunnelVirtualIp(device)
  if (!virtualIp) return
  try {
    await navigator.clipboard.writeText(virtualIp)
    ElMessage.success(t('devices.message.virtual_ip_copied', { ip: virtualIp }))
  } catch {
    ElMessage.error(t('devices.message.copy_virtual_ip_failed'))
  }
}

const speedTestingIds = reactive(new Set<number>())

async function testTunnelSpeed(device: DeviceInfo) {
  if (!tunnelVirtualIp(device) || speedTestingIds.has(device.device_id)) return
  speedTestingIds.add(device.device_id)
  try {
    const result = await invoke<TunnelSpeedTestResult>('test_tunnel_speed', {
      peerDeviceId: device.device_id,
    })
    await ElMessageBox.alert(
      t('devices.message.speed_test_result', {
        latency: result.latency_ms.toFixed(1),
        download: result.download_mbps.toFixed(2),
        upload: result.upload_mbps.toFixed(2),
      }),
      t('devices.message.speed_test_title'),
      { confirmButtonText: t('common.confirm') },
    )
  } catch (error) {
    ElMessage.error(t('devices.message.speed_test_failed', { reason: String(error) }))
  } finally {
    speedTestingIds.delete(device.device_id)
  }
}

async function warnWindows10HomeRdpUnsupported() {
  try {
    await ElMessageBox.confirm(
      t('devices.message.win10_home_body'),
      t('devices.message.win10_home_title'),
      {
        confirmButtonText: t('devices.message.view_tutorial'),
        cancelButtonText: t('common.cancel'),
        type: 'warning',
      },
    )
    await openExternal(WINDOWS_10_HOME_HELP_URL)
  } catch {
    // 用户取消查看教程时无需打断当前界面。
  }
}

function selectDevice(device: DeviceInfo) {
  if (selectedDevice.value?.device_id !== device.device_id) {
    detailExpanded.value = false
  }
  selectedDevice.value = device
}

async function refreshDevices(options: { force?: boolean } = {}) {
  try {
    await deviceStore.fetchDevices(options)
    await alignCurrentDeviceUUIDFromService()
    await refreshTunnelStatusMap()
  } catch (e) {
    console.error('Failed to fetch devices:', e)
  }
}

function isDevicesPageInForeground(): boolean {
  return document.visibilityState === 'visible' && document.hasFocus()
}

async function alignCurrentDeviceUUIDFromService() {
  const serviceStatus = await invoke<any>('get_service_status').catch(() => null)
  applyCurrentDeviceUUIDFromRuntime(serviceStatus?.runtime)
}

function applyCurrentDeviceUUIDFromRuntime(runtime: any) {
  const serviceUUID = runtime?.current_device?.device_uuid || runtime?.device_uuid
  currentDeviceUUID.value = typeof serviceUUID === 'string' ? serviceUUID.trim() : ''
}

async function autoRefreshDevicesInForeground() {
  if (!authStore.isLoggedIn) {
    return
  }
  if (!isDevicesPageInForeground()) {
    return
  }
  await refreshDevices()
}

async function handleForegroundResume() {
  if (!isDevicesPageInForeground()) {
    return
  }

  const now = Date.now()
  if (now - lastForegroundRefreshAt < FOREGROUND_REFRESH_THROTTLE_MS) {
    return
  }
  lastForegroundRefreshAt = now

  await autoRefreshDevicesInForeground()
}

async function handleActiveTunnelJobCompleted() {
  await refreshTunnelStatusMap()
}

async function handleActiveTunnelJobUpdated() {
  await refreshTunnelStatusMap()
}

async function refreshTunnelStatusMap() {
  try {
    const serviceStatus = await invoke<any>('get_service_status').catch(() => null)
    applyTunnelRuntimeStatus(serviceStatus?.runtime)
  } catch (e) {
    console.log('[Devices] 刷新隧道状态失败:', e)
  }
}

function applyTunnelRuntimeStatus(runtime: any) {
    const newMap: Record<number, TunnelInfo> = {}
    const newJobMap: Record<number, ActiveTunnelJobStatus> = {}
    const activeJobs = runtime?.active_tunnel_jobs
    if (Array.isArray(activeJobs)) {
      for (const job of activeJobs) {
        newJobMap[job.target_device_id] = job
        if (job.state === 'succeeded' && job.result?.success) {
          newMap[job.target_device_id] = {
            local_port: job.result.local_port || 0,
            rdp_address: job.result.rdp_address || '',
            role: 'active',
          }
        }
      }
    }
    const wgvpnJobs = runtime?.wgvpn_jobs
    if (Array.isArray(wgvpnJobs)) {
      for (const job of wgvpnJobs as WgvpnJobStatus[]) {
        if (!job.is_active) continue
        newJobMap[job.peer_device_id] = {
          target_device_id: job.peer_device_id,
          target_device_uuid: '',
          state: job.state,
          attempt: job.attempt,
          max_attempts: job.max_attempts,
          message: job.message,
          updated_at: job.updated_at,
        }
      }
    }
    activeTunnelJobMap.value = newJobMap
    const wgvpnSessions = runtime?.wgvpn_sessions
    const liveRoleMap = new Map<number, 'active' | 'passive'>()
    if (Array.isArray(wgvpnSessions)) {
      for (const session of wgvpnSessions) {
        const role: 'active' | 'passive' = session.is_active ? 'active' : 'passive'
        liveRoleMap.set(session.peer_device_id, role)
        const existing = newMap[session.peer_device_id]
        newMap[session.peer_device_id] = {
          local_port: existing?.local_port || session.local_forward_port || 0,
          rdp_address: existing?.rdp_address || (session.peer_virtual_ip ? `${session.peer_virtual_ip}:3389` : ''),
          virtual_ip: session.peer_virtual_ip || '',
          exposed_lan_cidrs: Array.isArray(session.exposed_lan_cidrs) ? session.exposed_lan_cidrs : [],
          health_state: session.health_state || 'connected',
          role,
        }
      }
    }
    const newLifecycleMap: Record<number, TunnelLifecycleStatus> = {}
    if (Array.isArray(runtime?.tunnel_lifecycles)) {
      for (const lifecycle of runtime.tunnel_lifecycles as TunnelLifecycleStatus[]) {
        const liveRole = liveRoleMap.get(lifecycle.peer_device_id)
        if (liveRole && lifecycle.role !== liveRole) continue
        const existing = newLifecycleMap[lifecycle.peer_device_id]
        const priority = lifecycle.state === 'connected' ? 3 : lifecycle.state === 'recovering' ? 2 : lifecycle.state === 'connecting' ? 1 : 0
        const existingPriority = existing?.state === 'connected' ? 3 : existing?.state === 'recovering' ? 2 : existing?.state === 'connecting' ? 1 : 0
        if (!existing || priority > existingPriority || lifecycle.role === liveRole) {
          newLifecycleMap[lifecycle.peer_device_id] = lifecycle
        }
      }
    }
    tunnelLifecycleMap.value = newLifecycleMap
    tunnelStatusMap.value = newMap
}

function openAliasDialog(device: DeviceInfo) {
  selectedDevice.value = device
  aliasForm.alias = device.device_alias || ''
  aliasDialogVisible.value = true
}

/*
function openPasswordDialog(device: DeviceInfo) {
  selectedDevice.value = device
  passwordForm.password = ''
  passwordForm.confirmPassword = ''
  passwordDialogVisible.value = true
}
*/

function openTunnelAction(device: DeviceInfo) {
  const lifecycle = deviceTunnelLifecycle(device)
  // 宽限期耗尽后 lifecycle 已回到未建立，但 service 快照中可能短暂保留旧会话。
  // 这时必须一次点击完成旧会话清理和新建，不能先把用户操作误当成“断开”。
  if (lifecycle.state === 'recovering') {
    if (lifecycle.role === 'passive') {
      void handleDisconnectTunnel(device)
    } else {
      void handleRetryTunnel(device)
    }
    return
  }
  if (lifecycle.last_result === 'health_grace_expired') {
    void handleRetryTunnel(device)
    return
  }
  if (lifecycle.state === 'connected' || hasTunnel(device)) {
    void handleDisconnectTunnel(device)
    return
  }
  if (lifecycle.state === 'connecting' || activeTunnelJob(device)) {
    void handleCancelActiveTunnelJob(device)
    return
  }
  void startTunnelSilently(device)
}

async function handleRetryTunnel(device: DeviceInfo) {
  const preparing = new Set(preparingTunnelIds.value)
  preparing.add(device.device_id)
  preparingTunnelIds.value = preparing
  try {
    await invoke('stop_service_active_tunnel', { targetDeviceId: device.device_id })
    await invoke<string>('start_service_active_tunnel', {
      targetDeviceId: device.device_id,
      targetDeviceUuid: device.device_uuid,
      connectCode: null,
      temporaryPassword: null,
      lanCidrs: [],
    })
    ElMessage.success(t('devices.message.reconnect_started'))
    await refreshTunnelStatusMap()
  } catch (error) {
    const message = typeof error === 'string' ? error : (error as any)?.message || t('common.unknown_error')
    ElMessage.error(t('devices.message.reconnect_failed', { error: message }))
    await refreshTunnelStatusMap()
  } finally {
    const latest = new Set(preparingTunnelIds.value)
    latest.delete(device.device_id)
    preparingTunnelIds.value = latest
  }
}

async function startTunnelSilently(device: DeviceInfo) {
  const preparing = new Set(preparingTunnelIds.value)
  preparing.add(device.device_id)
  preparingTunnelIds.value = preparing
  try {
    await invoke<string>('start_service_active_tunnel', {
      targetDeviceId: device.device_id,
      targetDeviceUuid: device.device_uuid,
      connectCode: null,
      temporaryPassword: null,
      lanCidrs: [],
    })
    ElMessage.success(t('devices.message.auto_tunnel_started'))
  } catch (error) {
    const message = typeof error === 'string' ? error : (error as any)?.message || t('common.unknown_error')
    await ElMessageBox.alert(
      t('devices.message.preflight_failed_body', { error: message }),
      t('devices.message.preflight_failed_title'),
      {
        type: 'error',
        confirmButtonText: t('common.got_it'),
      },
    ).catch(() => {})
  } finally {
    const latest = new Set(preparingTunnelIds.value)
    latest.delete(device.device_id)
    preparingTunnelIds.value = latest
  }
}

async function handleCancelActiveTunnelJob(device: DeviceInfo) {
  try {
    await invoke('stop_active_tunnel_job', { targetDeviceId: device.device_id })
    delete activeTunnelJobMap.value[device.device_id]
    ElMessage.success(t('devices.message.auto_tunnel_cancelled'))
  } catch (error) {
    ElMessage.error(t('devices.message.auto_tunnel_cancel_failed', { error: (error as any).message }))
  }
}

async function handleDisconnectTunnel(device: DeviceInfo) {
  try {
    const lifecycle = deviceTunnelLifecycle(device)
    const serviceStatus = lifecycle.role === 'passive'
      ? await invoke<any>('stop_service_tunnel', { sourceDeviceId: device.device_id })
      : await invoke<any>('stop_service_active_tunnel', { targetDeviceId: device.device_id })
    applyTunnelRuntimeStatus(serviceStatus?.runtime)
    ElMessage.success(t('devices.message.tunnel_disconnected'))
  } catch (error) {
    ElMessage.error(t('devices.message.disconnect_failed', { error: (error as any).message }))
  }
}

async function handleRemoteDesktop(device: DeviceInfo) {
  selectedDevice.value = device
  try {
    if (isWindows10HomeDevice(device)) {
      await warnWindows10HomeRdpUnsupported()
      return
    }

    const tunnel = tunnelStatusMap.value[device.device_id]
    if (!tunnel) {
      ElMessage.error(t('devices.message.no_active_tunnel'))
      return
    }

    if (!tunnel.rdp_address) {
      ElMessage.error(t('devices.message.no_rdp_address'))
      return
    }

    const launchAddress = resolveLaunchableRdpAddress(tunnel.rdp_address, device)
    if (!launchAddress) {
      await navigator.clipboard.writeText(tunnel.rdp_address)
      ElMessage.warning(t('devices.message.rdp_port_missing'))
      return
    }

    await navigator.clipboard.writeText(launchAddress)
    ElMessage.success(t('devices.message.rdp_address_copied', { address: launchAddress }))
  } catch (e) {
    const msg = typeof e === 'string' ? e : (e as any)?.message || t('common.unknown_error')
    ElMessage.error(t('devices.message.connect_failed', { error: msg }))
  }
}

async function handleDeviceAction(command: string, device: DeviceInfo) {
  selectedDevice.value = device
  switch (command) {
    case 'alias':
      openAliasDialog(device)
      break
    /*
    case 'connect-code':
      await generateConnectCode(device)
      break
    case 'set-password':
      openPasswordDialog(device)
      break
    */
    case 'delete':
      await confirmDeleteDevice(device)
      break
  }
}

async function confirmSetAlias() {
  if (!selectedDevice.value) return
  aliasForm.loading = true
  try {
    await invoke('update_device_alias', { deviceId: selectedDevice.value.device_id, alias: aliasForm.alias })
    ElMessage.success(t('devices.message.alias_set_success'))
    aliasDialogVisible.value = false
    await deviceStore.fetchDevices({ force: true })
  } catch (e) {
    ElMessage.error(t('devices.message.alias_set_failed', { error: e }))
  } finally {
    aliasForm.loading = false
  }
}

/*
async function confirmSetPassword() {
  if (!selectedDevice.value) return
  if (passwordForm.password.length < 4) {
    ElMessage.warning('密码长度不能少于 4 个字符')
    return
  }
  if (passwordForm.password !== passwordForm.confirmPassword) {
    ElMessage.warning('两次输入的密码不一致')
    return
  }

  passwordForm.loading = true
  try {
    await invoke('set_device_password', {
      deviceId: selectedDevice.value.device_id,
      password: passwordForm.password,
    })
    ElMessage.success('密码设置成功')
    passwordDialogVisible.value = false
  } catch (e) {
    ElMessage.error('设置失败: ' + e)
  } finally {
    passwordForm.loading = false
  }
}

async function generateConnectCode(device: DeviceInfo) {
  selectedDevice.value = device
  try {
    const code = await invoke<string>('generate_connect_code', { deviceId: device.device_id })
    currentConnectCode.value = code
    const match = deviceStore.devices.find(item => item.device_id === device.device_id)
    if (match) {
      match.connect_code = code
    }
    connectCodeDialogVisible.value = true
  } catch (e) {
    ElMessage.error('生成连接码失败: ' + e)
  }
}

async function copyConnectCode() {
  try {
    await navigator.clipboard.writeText(currentConnectCode.value)
    ElMessage.success('连接码已复制到剪贴板')
  } catch {
    ElMessage.error('复制失败')
  }
}

async function copyInlineConnectCode() {
  const code = selectedDevice.value?.connect_code || currentConnectCode.value
  if (!code) {
    ElMessage.warning('当前还没有可复制的连接码')
    return
  }

  try {
    await navigator.clipboard.writeText(code)
    ElMessage.success('连接码已复制到剪贴板')
  } catch {
    ElMessage.error('复制失败')
  }
}
*/

async function confirmDeleteDevice(device: DeviceInfo) {
  try {
    await ElMessageBox.confirm(
      t('devices.message.delete_confirm_body', { name: device.device_alias || device.device_name }),
      t('devices.message.delete_confirm_title'),
      { confirmButtonText: t('common.delete'), cancelButtonText: t('common.cancel'), type: 'warning' }
    )
  } catch {
    return
  }

  try {
    await invoke('delete_device', { deviceId: device.device_id })
    ElMessage.success(t('devices.message.delete_success'))
    await deviceStore.fetchDevices({ force: true })
  } catch (e) {
    ElMessage.error(t('devices.message.delete_failed', { error: e }))
  }
}
</script>

<style scoped>
.devices-page {
  height: calc(100% + 40px);
  min-height: 0;
  margin: -20px -32px;
  overflow: hidden;
  background: var(--fluent-bg);
}

.devices-shell {
  display: grid;
  grid-template-columns: 340px minmax(0, 1fr);
  gap: 16px;
  height: 100%;
  min-height: 0;
  padding: 16px 20px 20px;
  box-sizing: border-box;
}

/* 卡片容器视觉由 .fluent-card 工具类提供，此处仅保留 .devices-sidebar 的布局 */
.devices-sidebar {
  display: flex;
  flex-direction: column;
  min-height: 0;
  overflow: hidden;
}

.sidebar-header {
  display: flex;
  justify-content: space-between;
  align-items: flex-start;
  gap: 16px;
  padding: 18px 18px 14px;
  border-bottom: 1px solid var(--fluent-divider);
}

.sidebar-header h2 {
  margin: 0;
  font-size: var(--text-page-title);
  line-height: 32px;
  color: var(--fluent-text);
  font-weight: 600;
}

.count-text {
  color: var(--fluent-accent);
}

.sidebar-header p {
  margin: 6px 0 0;
  font-size: var(--text-body);
  color: var(--fluent-text-secondary);
}

/* 刷新按钮：由全局 EP 主题统一接管，此处仅微调圆角 */
.refresh-button {
  border-radius: var(--fluent-radius-md);
}

.device-list {
  flex: 1;
  overflow: auto;
  padding: 8px 10px 12px;
}

/* ===== 设备列表项：Fluent 选中层（去左侧竖条） ===== */
.device-list-item {
  width: 100%;
  display: grid;
  grid-template-columns: auto 1fr;
  gap: 10px;
  align-items: start;
  text-align: left;
  padding: 12px 12px 11px;
  border: 0;
  background: transparent;
  border-radius: var(--fluent-radius-md);
  cursor: pointer;
  position: relative;
  transition: background-color var(--fluent-duration) var(--fluent-easing);
}

.device-list-item:hover {
  background: var(--fluent-layer-hover);
}

.device-list-item.active {
  background: var(--fluent-accent-light);
}

.device-list-item.tunnel-connected:not(.active) {
  background: color-mix(in srgb, var(--fluent-success) 8%, transparent);
  box-shadow: inset 0 0 0 1px color-mix(in srgb, var(--fluent-success) 22%, transparent);
}

.device-list-item.tunnel-connected.active {
  background: color-mix(in srgb, var(--fluent-accent-light) 86%, var(--fluent-success) 14%);
  box-shadow: inset 0 0 0 1px color-mix(in srgb, var(--fluent-success) 32%, transparent);
}

.device-state-mark {
  width: 20px;
  height: 20px;
  margin-top: 2px;
  display: inline-flex;
  align-items: center;
  justify-content: center;
  flex: 0 0 20px;
}

.device-status-dot {
  width: 8px;
  height: 8px;
  border-radius: 50%;
  box-shadow: 0 0 0 3px rgba(0, 0, 0, 0.035);
}

/* 离线：灰 */
.device-status-dot.offline {
  background: var(--fluent-text-tertiary);
}

/* 在线空闲：蓝（设备可达，但尚未建立隧道） */
.device-status-dot.online {
  background: var(--status-online);
}

/* 连接中/重连中：橙 + 脉冲扩散 */
.device-status-dot.connecting {
  background: #f59e0b;
  animation: device-status-pulse 1.5s var(--fluent-easing) infinite;
}

/* 已连接：图标、文字标签与容器层级共同表达，不依赖相近的状态色。 */
.device-tunnel-icon {
  width: 20px;
  height: 20px;
  display: inline-flex;
  align-items: center;
  justify-content: center;
  border-radius: 7px;
  color: #fff;
  background: var(--fluent-success);
  box-shadow: 0 2px 5px color-mix(in srgb, var(--fluent-success) 26%, transparent);
}

.device-tunnel-badge {
  display: inline-flex;
  align-items: center;
  gap: 4px;
  min-height: 20px;
  padding: 0 6px;
  border: 1px solid color-mix(in srgb, var(--fluent-success) 34%, transparent);
  border-radius: 999px;
  color: color-mix(in srgb, var(--fluent-success) 88%, var(--fluent-text));
  background: color-mix(in srgb, var(--fluent-success) 12%, transparent);
  font-size: 11px;
  font-weight: 650;
  line-height: 1;
  white-space: nowrap;
  transition: background-color 160ms var(--fluent-easing), border-color 160ms var(--fluent-easing), color 160ms var(--fluent-easing);
}

.device-tunnel-badge .el-icon {
  font-size: 12px;
}

@keyframes device-status-pulse {
  0%,
  100% {
    box-shadow: 0 0 0 0 rgba(245, 158, 11, 0.45);
  }
  50% {
    box-shadow: 0 0 0 4px rgba(245, 158, 11, 0);
  }
}

@media (prefers-reduced-motion: reduce) {
  .device-status-dot.connecting {
    animation: none;
  }
}

.device-list-title-row {
  display: flex;
  align-items: center;
  gap: 8px;
  min-width: 0;
  flex-wrap: nowrap;
}

.device-list-main {
  min-width: 0;
}

.device-list-title {
  min-width: 0;
  overflow: hidden;
  text-overflow: ellipsis;
  white-space: nowrap;
  font-size: 15px;
  color: var(--fluent-text);
  font-weight: 600;
}

.device-list-subtitle {
  margin-top: 6px;
  display: flex;
  align-items: center;
  min-width: 0;
  flex-wrap: nowrap;
  gap: 8px;
  font-size: 12px;
  color: var(--fluent-text-secondary);
  overflow: hidden;
}

.device-system-summary {
  flex: 0 1 150px;
  min-width: 40px;
  max-width: 150px;
  overflow: hidden;
  text-overflow: ellipsis;
  white-space: nowrap;
}

.device-ip {
  max-width: 112px;
  overflow: hidden;
  flex-shrink: 0;
  text-overflow: ellipsis;
  white-space: nowrap;
}

.device-list-subtitle span::after {
  content: '•';
  margin-left: 8px;
  color: var(--fluent-text-tertiary);
}

.device-list-subtitle span:last-child::after {
  display: none;
}

.empty-list,
.empty-detail {
  display: flex;
  align-items: center;
  justify-content: center;
  min-height: 240px;
}

.device-detail-panel {
  min-height: 0;
  padding: 0 24px 24px;
  overflow-y: auto;
  overscroll-behavior: contain;
}

.detail-header-card {
  display: flex;
  justify-content: space-between;
  gap: 16px;
  padding: 22px 0 20px;
  border-radius: 0;
  background: transparent;
  border: 0;
  border-bottom: 1px solid var(--fluent-divider);
}

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
  font-size: 24px;
}

/* 离线：灰 */
.device-avatar.offline {
  color: var(--fluent-text-tertiary);
  background: var(--fluent-layer-hover);
}

/* 在线·空闲：青绿，蓝色保留给选择与主要操作 */
.device-avatar.online {
  color: var(--status-online);
  background: var(--status-online-bg);
}

/* 连接中/重连中：列表状态点承担运动反馈，详情头像保持稳定 */
.device-avatar.connecting {
  color: #b45309;
  background: rgba(245, 158, 11, 0.12);
  border: 1px solid #f59e0b;
}

/* 已连接：绿填充（活跃隧道） */
.device-avatar.connected {
  color: #fff;
  background: var(--fluent-success);
}

.detail-title-line {
  display: flex;
  align-items: center;
  gap: 8px;
  flex-wrap: wrap;
}

.device-online-tag {
  --el-tag-text-color: var(--status-online);
  --el-tag-bg-color: var(--status-online-bg);
  --el-tag-border-color: color-mix(in srgb, var(--status-online) 35%, transparent);
}

.device-tunnel-tag {
  --el-tag-text-color: color-mix(in srgb, var(--fluent-success) 88%, var(--fluent-text));
  --el-tag-bg-color: color-mix(in srgb, var(--fluent-success) 12%, transparent);
  --el-tag-border-color: color-mix(in srgb, var(--fluent-success) 38%, transparent);
  display: inline-flex;
  align-items: center;
  gap: 4px;
  font-weight: 650;
}

.detail-title-line h3 {
  margin: 0;
  font-size: 22px;
  color: var(--fluent-text);
  font-weight: 600;
}

.detail-subtitle {
  margin: 5px 0 0;
  color: var(--fluent-text-secondary);
  font-size: 14px;
}

.detail-expand-button .el-icon {
  transition: transform var(--fluent-duration) var(--fluent-easing);
}

.detail-expand-button .el-icon.expanded {
  transform: rotate(180deg);
}

.more-btn {
  font-size: 20px;
  color: var(--fluent-text-secondary);
}

.detail-grid {
  margin-top: 0;
  display: grid;
  grid-template-columns: 1fr;
  gap: 0;
}

.detail-section {
  border-radius: 0;
  padding: 22px 0;
  background: transparent;
  border: 0;
  border-bottom: 1px solid var(--fluent-divider);
}

.detail-section:last-child { border-bottom: 0; }

.section-title {
  margin-bottom: 14px;
  font-size: 16px;
  font-weight: 600;
  color: var(--fluent-text);
}

.section-title-row {
  display: flex;
  align-items: center;
  justify-content: space-between;
}

.section-title-row .section-title {
  margin-bottom: 14px;
}

.section-title-row .detail-expand-button {
  margin-top: -8px;
  margin-bottom: 6px;
}

.info-grid {
  display: grid;
  grid-template-columns: repeat(2, minmax(0, 1fr));
  column-gap: 32px;
  row-gap: 0;
  border-top: 1px solid var(--fluent-divider);
}

.info-grid-expanded {
  border-top: 0;
}

.info-item {
  display: flex;
  flex-direction: column;
  gap: 5px;
  min-height: 62px;
  padding: 12px 2px 10px;
  border-radius: 0;
  background: transparent;
  border: 0;
  border-bottom: 1px solid var(--fluent-divider);
}

.info-item.wide {
  grid-column: 1 / -1;
}

.info-label {
  font-size: 12px;
  color: var(--fluent-text-secondary);
  letter-spacing: 0.04em;
}

.info-value {
  font-size: 15px;
  color: var(--fluent-text);
  word-break: break-word;
}

.public-ip-location {
  margin-left: 8px;
  color: var(--fluent-text-secondary);
  font-size: 13px;
  font-weight: 400;
}

.info-value.uuid {
  font-family: 'Cascadia Code', 'Consolas', 'Courier New', monospace;
  font-size: 13px;
}

.info-value.code {
  font-family: 'Cascadia Code', 'Consolas', 'Courier New', monospace;
  font-size: 17px;
  font-weight: 600;
  color: var(--fluent-accent);
  letter-spacing: 0.08em;
}

.connect-code-inline {
  display: flex;
  align-items: center;
  justify-content: space-between;
  gap: 12px;
}

/* ===== 隧道生命周期条：Fluent InfoBar ===== */
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

.tunnel-lifecycle-strip .lifecycle-dot {
  width: 9px;
  height: 9px;
  margin-top: 5px;
  border-radius: 50%;
  background: var(--fluent-text-tertiary);
}

.tunnel-lifecycle-strip strong { display: block; color: var(--fluent-text); }
.tunnel-lifecycle-strip p { margin: 3px 0 0; color: var(--fluent-text-secondary); font-size: 13px; }
.tunnel-lifecycle-strip.compact {
  padding-block: 9px;
  background: transparent;
  border-top: 1px solid var(--fluent-stroke);
  border-right: 1px solid var(--fluent-stroke);
  border-bottom: 1px solid var(--fluent-stroke);
}
.tunnel-lifecycle-strip.compact > div {
  min-width: 0;
  display: flex;
  align-items: baseline;
  gap: 8px;
}
.tunnel-lifecycle-strip.compact p {
  margin: 0;
  overflow: hidden;
  text-overflow: ellipsis;
  white-space: nowrap;
}
.tunnel-lifecycle-strip.state-connecting {
  border-left-color: var(--fluent-accent);
  background: var(--fluent-info-bg);
}
.tunnel-lifecycle-strip.state-connecting .lifecycle-dot { background: var(--fluent-accent); }
.tunnel-lifecycle-strip.state-connected {
  border-left-color: var(--fluent-success);
  background: var(--fluent-success-bg);
}
.tunnel-lifecycle-strip.state-connected .lifecycle-dot { background: var(--fluent-success); }
.tunnel-lifecycle-strip.state-recovering {
  border-left-color: var(--fluent-warning);
  background: var(--fluent-warning-bg);
}
.tunnel-lifecycle-strip.state-recovering .lifecycle-dot { background: var(--fluent-warning); }

.action-grid {
  display: grid;
  grid-template-columns: repeat(auto-fit, minmax(180px, 1fr));
  gap: 10px;
}

/* 只有隧道操作时保持紧凑，适用于 Linux、未开启 RDP 的 Windows 等设备。 */
.action-grid.single-tunnel-action {
  grid-template-columns: minmax(180px, 240px);
}

/* ===== 操作瓦片：Fluent 按钮（去位移反馈，改背景层变化） ===== */
.action-tile {
  border: 1px solid var(--fluent-stroke);
  border-radius: var(--fluent-radius-md);
  padding: 12px 14px;
  background: var(--fluent-layer);
  display: flex;
  align-items: center;
  justify-content: flex-start;
  gap: 10px;
  cursor: pointer;
  transition: background-color var(--fluent-duration) var(--fluent-easing),
    border-color var(--fluent-duration) var(--fluent-easing),
    transform var(--motion-fast) var(--ease-out);
  color: var(--fluent-text);
  text-align: left;
}

.action-tile:hover:not(:disabled) {
  background: var(--fluent-layer-hover);
  border-color: var(--fluent-stroke-strong);
}

.action-tile:active:not(:disabled) {
  background: var(--fluent-layer-pressed);
}

.action-tile:disabled {
  cursor: not-allowed;
  opacity: 0.45;
}

.action-tile .el-icon {
  font-size: 20px;
  flex-shrink: 0;
}

.action-tile span {
  font-size: 14px;
  font-weight: 600;
  white-space: normal;
  line-height: 1.3;
  flex: 1;
}

.action-copy-content {
  display: flex;
  min-width: 0;
  flex-direction: column;
  gap: 3px;
}

.action-copy-content > span {
  font-size: 14px;
}

.action-copy-content small {
  overflow: hidden;
  color: var(--fluent-text-secondary);
  font-family: 'Cascadia Code', 'Consolas', 'Courier New', monospace;
  font-size: 12px;
  font-weight: 400;
  text-overflow: ellipsis;
  white-space: nowrap;
}

/* 主操作瓦片：accent 实底 */
.action-tile.primary {
  background: var(--fluent-accent);
  border-color: var(--fluent-accent);
  color: var(--fluent-text-on-accent);
}

.action-tile.primary:hover:not(:disabled) {
  background: var(--fluent-accent-hover);
  border-color: var(--fluent-accent-hover);
}

.action-tile.primary:active:not(:disabled) {
  background: var(--fluent-accent-pressed);
  border-color: var(--fluent-accent-pressed);
}

.action-tile.danger {
  border-color: var(--fluent-danger-stroke);
}

.action-help {
  margin-left: auto;
  width: auto;
  height: auto;
  display: inline-flex;
  align-items: center;
  justify-content: center;
  font-size: 14px;
  font-weight: 700;
  background: transparent;
  border: none;
  color: var(--fluent-text-tertiary);
  flex-shrink: 0;
  transition: color var(--fluent-duration) var(--fluent-easing);
}

.detail-expand-label {
  font-size: 13px;
  font-weight: 500;
}

.action-help:hover {
  color: var(--fluent-accent);
}

.action-tile.primary .action-help {
  background: transparent;
  border: none;
  color: rgba(255, 255, 255, 0.85);
}

.action-tile.primary .action-help:hover {
  color: #fff;
}

/* ===== LAN 访问配置 ===== */
.lan-access-card {
  display: flex;
  flex-direction: column;
  gap: 0;
}

.lan-access-heading .section-title {
  margin-bottom: 14px;
}

.lan-cidr-content {
  display: flex;
  align-items: flex-start;
  gap: 24px;
}

.lan-cidr-input {
  flex: 1;
  min-width: 0;
}

.lan-cidr-editor {
  padding: 16px 2px;
  border-radius: 0;
  background: transparent;
  border: 0;
}

.lan-cidr-label {
  margin-bottom: 8px;
  font-size: 13px;
  font-weight: 600;
  color: var(--fluent-text);
}

.lan-access-actions {
  display: flex;
  justify-content: flex-end;
  padding-top: 14px;
}

.connect-code-display {
  text-align: center;
  padding: 20px 0;
}

.connect-code-display p {
  margin: 0 0 16px;
  color: var(--fluent-text-secondary);
}

.code-box {
  margin: 16px 0;
  padding: 20px;
  border-radius: var(--fluent-radius-md);
  background: var(--fluent-accent-light);
  color: var(--fluent-accent);
  font-size: 32px;
  font-weight: 700;
  letter-spacing: 0.2em;
  font-family: 'Cascadia Code', 'Consolas', 'Courier New', monospace;
}

.code-hint,
.address-hint {
  font-size: 12px;
  color: var(--fluent-text-tertiary);
}

.service-summary h3 {
  margin: 0 0 6px;
  font-size: 20px;
  color: var(--fluent-text);
}

.service-summary p {
  margin: 0 0 18px;
  color: var(--fluent-text-secondary);
}

.service-result {
  display: flex;
  align-items: center;
  gap: 10px;
  width: 100%;
}

.service-result .el-input {
  flex: 1;
}

@media (max-width: 1200px) {
  .detail-grid {
    grid-template-columns: 1fr;
  }
}

@media (max-width: 900px) {
  .devices-page {
    height: auto;
    min-height: calc(100% + 40px);
    margin: -20px -32px;
    overflow: visible;
  }

  .devices-shell {
    grid-template-columns: 1fr;
    height: auto;
    min-height: auto;
    padding: 14px;
  }

  .devices-sidebar {
    min-height: 320px;
    max-height: 45vh;
  }

  .device-detail-panel {
    overflow: visible;
  }

  .lan-cidr-content {
    flex-direction: column;
    gap: 4px;
  }

  .info-grid,
  .action-grid {
    grid-template-columns: 1fr;
  }

  .action-grid.single-tunnel-action {
    grid-template-columns: minmax(180px, 240px);
  }

  .connect-code-inline {
    flex-direction: column;
    align-items: flex-start;
  }
}
</style>
