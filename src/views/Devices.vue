<template>
  <div
    class="devices-page"
    v-loading="selectedDisconnecting"
    :element-loading-text="$t('devices.message.disconnecting')"
  >
    <el-alert
      v-if="registrationError"
      class="registration-error-alert"
      type="error"
      show-icon
      :closable="false"
      :title="$t('devices.registration_failed_title')"
      :description="registrationErrorMessage"
    />
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
            <span class="device-list-platform">
              <DevicePlatformIcon :device="device" :tone="deviceConnectionState(device)" />
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
                  <DevicePlatformIcon :device="selectedDevice" :tone="deviceConnectionState(selectedDevice)" />
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
                  <el-dropdown-item command="delete" divided>{{ $t('devices.menu.delete_device') }}</el-dropdown-item>
                </el-dropdown-menu>
              </template>
            </el-dropdown>
          </div>

          <div class="detail-grid">
            <div v-if="!isCurrentDevice(selectedDevice.device_uuid)" class="detail-section action-section">
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
                  <strong>{{ tunnelLifecycleTitle(selectedDevice) + tunnelLifecycleDots(selectedDevice) }}</strong>
                  <p>{{ tunnelLifecycleDescription(selectedDevice) }}</p>
                  <p v-if="isTunnelConnected(selectedDevice)">{{ $t('devices.lifecycle.desc_connected_latency', { latency: tunnelLatencyText(selectedDevice) }) }}</p>
                  <p v-if="tunnelStatusMap[selectedDevice.device_id]?.network">{{ $t('devices.lifecycle.desc_connected_network', { network: tunnelStatusMap[selectedDevice.device_id]?.network?.toUpperCase() }) }}</p>
                  <p v-if="tunnelLanCidrs(selectedDevice).length">
                    {{ $t('devices.lifecycle.desc_connected_with_lan', { cidrs: tunnelLanCidrs(selectedDevice).join(', ') }) }}
                  </p>
                </div>
              </div>

              <el-alert v-if="activeTunnelJobMap[selectedDevice.device_id]?.tcp_retry_recommended" type="warning" :closable="false" :title="$t('app.settings.tcp_failure_hint')">
                <el-button :loading="preparingTunnelIds.has(selectedDevice.device_id)" @click="disableTcpAndRetry(selectedDevice)">{{ $t('app.settings.disable_tcp_retry') }}</el-button>
              </el-alert>
              <el-alert v-if="activeTunnelJobMap[selectedDevice.device_id]?.symmetric_nat_help_recommended" type="warning" :closable="false" :title="$t('devices.message.symmetric_nat_help')">
                <el-button @click="openSymmetricNatHelp">{{ $t('devices.message.view_network_help') }}</el-button>
              </el-alert>
			  <div class="action-groups">
				<div class="action-group">
				  <div class="action-group-label">{{ $t('devices.detail.connection.tunnel_group') }}</div>
				  <div class="action-grid action-grid-primary">
					<button v-if="selectedDevice.status !== 'online' && selectedDevice.wake_available && !isCurrentDevice(selectedDevice.device_uuid)" type="button" class="action-tile primary" :disabled="wakingIds.has(selectedDevice.device_id)" @click="handleWakeDevice(selectedDevice)">
					<el-icon><SwitchButton /></el-icon>
					<span>{{ wakingIds.has(selectedDevice.device_id) ? $t('devices.wol.sending') : $t('devices.wol.action') }}</span>
				</button>
                <el-tooltip :content="tunnelActionTooltip(selectedDevice)" placement="top">
                  <div class="action-tile-wrap">
                    <button
                      type="button"
                      class="action-tile primary"
                      :disabled="!canUseTunnelAction(selectedDevice) || selectedDisconnecting"
                      :aria-label="`${activeTunnelActionText(selectedDevice)}. ${tunnelActionTooltip(selectedDevice)}`"
                      @click="openTunnelAction(selectedDevice!)"
                    >
                      <el-icon :class="{ 'is-loading': selectedDisconnecting }"><Loading v-if="selectedDisconnecting" /><Link v-else /></el-icon>
                      <span>{{ activeTunnelActionText(selectedDevice) }}</span>
                    </button>
                  </div>
                </el-tooltip>
					</div>
				  </div>

					<div class="action-group">
				  <div class="action-group-label">{{ $t('devices.detail.connection.remote_group') }}</div>
				  <div class="action-grid">
                <el-tooltip
                  v-if="isWindowsDevice(selectedDevice)"
                  :content="$t('devices.detail.connection.remote_desktop_tooltip')"
                  placement="top"
                >
                  <div class="action-tile-wrap">
                    <button
                      type="button"
                      class="action-tile"
                      :disabled="!isTunnelConnected(selectedDevice)"
                      :aria-label="`${$t('devices.detail.connection.remote_desktop')}. ${$t('devices.detail.connection.remote_desktop_tooltip')}`"
                      @click="copyRemoteDesktopAddress(selectedDevice!)"
                    >
                      <el-icon><CopyDocument /></el-icon>
                      <span>{{ $t('devices.detail.connection.remote_desktop') }}</span>
                      <span class="action-help" aria-hidden="true"><el-icon><InfoFilled /></el-icon></span>
                    </button>
                  </div>
                </el-tooltip>

                <el-tooltip
                  v-if="showWindowsRdpAction(selectedDevice)"
                  :content="$t('devices.detail.connection.windows_rdp_tooltip')"
                  placement="top"
                >
                  <div class="action-tile-wrap">
                    <button
                      type="button"
                      class="action-tile"
                      :disabled="!isTunnelConnected(selectedDevice)"
                      :aria-label="`${$t('devices.detail.connection.windows_rdp')}. ${$t('devices.detail.connection.windows_rdp_tooltip')}`"
                      @click="launchWindowsRdp(selectedDevice!)"
                    >
                      <img :src="mstscIcon" class="action-tile-icon" alt="" aria-hidden="true">
                      <span>{{ $t('devices.detail.connection.windows_rdp') }}</span>
                      <span class="action-help" aria-hidden="true"><el-icon><InfoFilled /></el-icon></span>
                    </button>
                  </div>
                </el-tooltip>

                <el-tooltip
                  v-if="showLocalProcessActions"
                  :content="$t('devices.detail.connection.p2premote_desktop_tooltip')"
                  placement="top"
                >
                  <div class="action-tile-wrap">
                    <button
                      type="button"
                      class="action-tile"
                      :disabled="!isTunnelConnected(selectedDevice)"
                      :aria-label="`${$t('devices.detail.connection.p2premote_desktop')}. ${$t('devices.detail.connection.p2premote_desktop_tooltip')}`"
                      @click="launchP2pRemoteDesktop(selectedDevice!)"
                    >
                      <img :src="rustdeskTinyIcon" class="action-tile-icon" alt="" aria-hidden="true">
                      <span>{{ $t('devices.detail.connection.p2premote_desktop') }}</span>
                      <span class="action-help" aria-hidden="true"><el-icon><InfoFilled /></el-icon></span>
                    </button>
                  </div>
                </el-tooltip>
				  </div>
				</div>

				<div v-if="tunnelVirtualIp(selectedDevice)" class="action-group">
				  <div class="action-group-label">{{ $t('devices.detail.connection.tools_group') }}</div>
				  <div class="action-grid">
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
                  <span class="info-label">{{ $t('devices.detail.info.client_version') }}</span>
                  <span class="info-value">{{ selectedDevice.client_version || '-' }}</span>
                </div>
                <div class="info-item">
                  <span class="info-label">{{ $t('devices.detail.info.lan_ip') }}</span>
                  <span class="info-value">{{ selectedDevice.lan_ip || '-' }}</span>
                </div>
                <div class="info-item">
                  <span class="info-label">{{ remoteAccessPortLabel(selectedDevice) }}</span>
                  <span class="info-value">{{ selectedDevice.remote_access?.port || selectedDevice.service_port }}</span>
                </div>
                <div class="info-item">
                  <span class="info-label">{{ $t('devices.detail.info.device_uuid') }}</span>
                  <span class="info-value uuid">{{ selectedDevice.device_uuid }}</span>
                </div>
				<div class="info-item">
				  <span class="info-label">{{ $t('devices.detail.info.capabilities') }}</span>
				  <span v-if="deviceCapabilities(selectedDevice).length" class="capability-list">
					<el-tag v-for="capability in deviceCapabilities(selectedDevice)" :key="capability" size="small" type="info">
					  {{ capabilityLabel(capability) }}
					</el-tag>
				  </span>
				  <span v-else class="info-value">{{ $t('devices.detail.info.no_capabilities') }}</span>
				</div>
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

  </div>
</template>

<script setup lang="ts">
import { computed, onMounted, onUnmounted, reactive, ref, watch } from 'vue'
import { useI18n } from 'vue-i18n'
import { invoke, isTauriRuntime, listen, openExternal, type UnlistenFn } from '../runtime/bridge'
import { ElMessage } from 'element-plus/es/components/message/index.mjs'
import { ElMessageBox } from 'element-plus/es/components/message-box/index.mjs'
import {
  CopyDocument,
  ArrowDown,
  InfoFilled,
  Link,
  Loading,
  MoreFilled,
  Odometer,
  Refresh,
	SwitchButton,
} from '@element-plus/icons-vue'
import mstscIcon from '../assets/icons/mstsc.png'
import rustdeskTinyIcon from '../assets/icons/rustdesk-tiny.png'
import { useDeviceStore, type DeviceInfo } from '../stores/device'
import { errorMessage } from '../utils/errorMessage'
import { copyTextToClipboard } from '../utils/clipboard'
import type { TunnelJobStatus, TunnelLifecycleStatus } from '../types/api'
import { useAuthStore } from '../stores/auth'
import DevicePlatformIcon from '../components/DevicePlatformIcon.vue'
import { useConnectingDots } from '../composables/useConnectingDots'
const { t } = useI18n()

interface TunnelSpeedTestResult {
  latency_ms: number
  download_mbps: number
  upload_mbps: number
}

interface TunnelInfo {
  network?: string
  virtual_ip?: string
  exposed_lan_cidrs?: string[]
  health_state?: 'connected' | 'degraded'
  role?: 'active' | 'passive'
  latency_ms?: number | null
}

const deviceStore = useDeviceStore()
const authStore = useAuthStore()
const showLocalProcessActions = isTauriRuntime()
const isLocalWindows = /Windows/i.test(navigator.userAgent)

const aliasDialogVisible = ref(false)
const selectedDevice = ref<DeviceInfo | null>(null)
const detailExpanded = ref(false)
const currentDeviceUUID = ref('')
/// 本机注册失败原因（设备数达到账号上限等）。非空时页面顶部显示常驻
/// 告警条：本机不在设备列表且无法被远程连接，后台每分钟自动重试。
const registrationError = ref('')
const tunnelStatusMap = ref<Record<number, TunnelInfo>>({})
const activeTunnelJobMap = ref<Record<number, TunnelJobStatus>>({})
const tunnelLifecycleMap = ref<Record<number, TunnelLifecycleStatus>>({})
const preparingTunnelIds = ref(new Set<number>())
const disconnectingIds = ref(new Set<number>())
const wakingIds = reactive(new Set<number>())

const aliasForm = reactive({ alias: '', loading: false })
let autoRefreshTimer: ReturnType<typeof window.setInterval> | null = null
let unlistenServiceStatus: UnlistenFn | null = null

const onlineDeviceCount = computed(() => deviceStore.devices.filter(device => device.status === 'online').length)
const selectedDisconnecting = computed(() => (
  selectedDevice.value ? disconnectingIds.value.has(selectedDevice.value.device_id) : false
))
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

    // 当前选中优先（列表刷新保持不变）；组件重挂载时回落到记忆中的上次选中
    const currentSelectedId = selectedDevice.value?.device_id ?? deviceStore.lastSelectedDeviceId
    const matched = currentSelectedId != null
      ? devices.find(device => device.device_id === currentSelectedId)
      : undefined
    selectedDevice.value = matched || devices[0]
  },
  { immediate: true }
)

onMounted(async () => {
  try {
    // 页面首次展示时必须从 service 重新同步设备状态。客户端重启期间，
    // store 里可能仍是 WebSocket 建连前取得的离线快照。
    await refreshDevices({ force: true })
  } catch (e) {
    console.error('获取设备列表失败:', e)
  }

  autoRefreshTimer = window.setInterval(() => {
    void autoRefreshDevicesInForeground()
  }, 60_000)

  window.addEventListener('focus', handleForegroundResume)
  document.addEventListener('visibilitychange', handleForegroundResume)
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

function hasTunnel(device: DeviceInfo | null): boolean {
  if (!device) return false
  return Boolean(tunnelStatusMap.value[device.device_id])
}

function activeTunnelJob(device: DeviceInfo | null): TunnelJobStatus | null {
  if (!device) return null
  const job = activeTunnelJobMap.value[device.device_id]
  if (!job || (job.state !== 'running' && job.state !== 'waiting')) return null
  return job
}

function activeTunnelActionText(device: DeviceInfo | null): string {
  if (!device) return t('devices.detail.connection.tunnel_action')
  const lifecycle = deviceTunnelLifecycle(device)
  if (lifecycle.state === 'connected') {
    return lifecycle.role === 'passive'
      ? t('devices.detail.connection.disconnect_passive')
      : t('devices.detail.connection.disconnect')
  }
  if (preparingTunnelIds.value.has(device.device_id)) return t('devices.detail.connection.preparing')
  if (lifecycle.state === 'connecting'
    || lifecycle.state === 'awaiting_approval'
    || lifecycle.state === 'recovering'
    || activeTunnelJob(device)) {
    return t('devices.detail.connection.cancel')
  }
  return t('devices.detail.connection.tunnel_action')
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
      updated_at: 0,
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
    updated_at: job?.updated_at || 0,
  }
}

const connectingDots = useConnectingDots()

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
  if (state === 'connecting' || state === 'awaiting_approval' || state === 'recovering') return 'connecting'
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

/// 打洞进行中在标题后追加 '.' → '..' → '...' 滚动省略号，
/// 提示过程仍在推进（每秒一帧）。
function tunnelLifecycleDots(device: DeviceInfo | null): string {
  return deviceTunnelLifecycle(device).state === 'connecting' ? connectingDots.value : ''
}

function tunnelLifecycleDescription(device: DeviceInfo | null): string {
  const lifecycle = deviceTunnelLifecycle(device)
  if (lifecycle.state === 'connecting') {
    const attempt = lifecycle.attempt
    const suffix = lifecycle.message ? ` · ${lifecycle.message}` : ''
    return t('devices.lifecycle.desc_connecting', { attempt, max: lifecycle.max_attempts, suffix })
  }
  if (lifecycle.state === 'connected') {
    const ip = lifecycle.peer_virtual_ip || tunnelVirtualIp(device)
    return ip ? t('devices.lifecycle.desc_connected_with_ip', { ip }) : t('devices.lifecycle.desc_connected_plain')
  }
  if (lifecycle.state === 'awaiting_approval') return lifecycle.message || t('devices.lifecycle.desc_awaiting_approval')
  if (lifecycle.state === 'recovering') return lifecycle.message || t('devices.lifecycle.desc_recovering_default')
  if (lifecycle.last_result !== 'none' && lifecycle.message) return lifecycle.message
  return t('devices.lifecycle.desc_none')
}

function tunnelActionTooltip(device: DeviceInfo | null): string {
  const lifecycle = deviceTunnelLifecycle(device)
  if (lifecycle.state === 'connecting' || lifecycle.state === 'awaiting_approval' || lifecycle.state === 'recovering') {
    return t('devices.lifecycle.tooltip_cancel')
  }
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

function isWindowsHomeEdition(device: DeviceInfo | null): boolean {
  if (!device) return false
  const version = (device.system_version || '').toLocaleLowerCase()
  return /(?:^|\s)home(?:\s|$)/.test(version) || version.includes('家庭版')
}

function isRdpEnabled(device: DeviceInfo): boolean {
  return device.remote_access?.enabled ?? device.rdp_enabled
}

function warnIfWindowsRdpUnavailable(device: DeviceInfo): boolean {
  if (!isWindowsDevice(device) || isRdpEnabled(device)) return false
  ElMessage.warning(t(isWindowsHomeEdition(device)
    ? 'devices.message.windows_home_rdp_disabled'
    : 'devices.message.windows_rdp_disabled'))
  return true
}

function deviceCapabilities(device: DeviceInfo | null): string[] {
  return Array.isArray(device?.capabilities) ? device.capabilities : []
}

function hasCapability(device: DeviceInfo | null, capability: string): boolean {
  return deviceCapabilities(device).includes(capability)
}

function capabilityLabel(capability: string): string {
  return capability === 'rustdesk_tiny'
    ? t('devices.detail.info.capability_rustdesk_tiny')
    : capability
}

/// 远程访问协议只认后端显式上报的 remote_access.protocol；
/// 缺失时不按 device_type 猜测，相关操作按钮保持隐藏以暴露契约变化。
function remoteAccessProtocol(device: DeviceInfo | null): string {
  if (!device) return ''
  return device.remote_access?.protocol?.trim().toLowerCase() || ''
}

function remoteAccessPortLabel(device: DeviceInfo | null): string {
  return remoteAccessProtocol(device) === 'vnc'
    ? t('devices.detail.info.vnc_port')
    : t('devices.detail.info.rdp_port')
}

function remoteDesktopAddress(device: DeviceInfo): string {
  const virtualIp = tunnelVirtualIp(device)
  const port = Number(device.remote_access?.port || device.service_port)
  if (!virtualIp || !Number.isInteger(port) || port <= 0 || port > 65535) return ''
  return `${virtualIp}:${port}`
}

function showWindowsRdpAction(device: DeviceInfo | null): boolean {
  return showLocalProcessActions && isLocalWindows && remoteAccessProtocol(device) === 'rdp'
}

function isAndroidDevice(device: DeviceInfo | null): boolean {
  if (!device) return false
  const text = `${device.device_type || ''} ${device.system_version || ''}`.toLocaleLowerCase()
  return text.includes('android')
}

function tunnelVirtualIp(device: DeviceInfo | null): string {
  if (!device) return ''
  return tunnelStatusMap.value[device.device_id]?.virtual_ip || ''
}

/// 已连接时的心跳 RTT；心跳尚未回报时显示 —，与隧道状态页语义一致。
function tunnelLatencyText(device: DeviceInfo | null): string {
  if (!device) return '—'
  const latency = tunnelStatusMap.value[device.device_id]?.latency_ms
  return typeof latency === 'number' ? String(latency) : '—'
}

const RUSTDESK_TINY_DEFAULT_PORT = 21121

function rustdeskTinyAddress(device: DeviceInfo | null): string {
  const virtualIp = tunnelVirtualIp(device)
  return virtualIp ? `${virtualIp}:${RUSTDESK_TINY_DEFAULT_PORT}` : ''
}

function tunnelLanCidrs(device: DeviceInfo | null): string[] {
  if (!device || deviceTunnelLifecycle(device).state !== 'connected') return []
  return tunnelStatusMap.value[device.device_id]?.exposed_lan_cidrs || []
}

async function copyTunnelVirtualIp(device: DeviceInfo) {
  const virtualIp = tunnelVirtualIp(device)
  if (!virtualIp) return
  try {
    await copyTextToClipboard(virtualIp)
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

/// 用户显式选中设备（点击列表项、菜单操作等）的唯一入口：
/// 记录到 store 内存，页面切换或窗口隐藏到托盘后重开时恢复上次选中。
function selectDevice(device: DeviceInfo) {
  if (selectedDevice.value?.device_id !== device.device_id) {
    detailExpanded.value = false
  }
  selectedDevice.value = device
  deviceStore.rememberSelectedDevice(device.device_id)
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
  const serviceUUID = runtime?.current_device?.device_uuid
  currentDeviceUUID.value = typeof serviceUUID === 'string' ? serviceUUID.trim() : ''
  const registrationFailure = runtime?.logged_in
    ? (typeof runtime?.device_registration_error === 'string' ? runtime.device_registration_error.trim() : '')
    : ''
  registrationError.value = registrationFailure
}

const registrationErrorMessage = computed(() => {
  if (!registrationError.value) return ''
  return t('devices.registration_failed_body', { error: registrationError.value })
})

async function autoRefreshDevicesInForeground() {
  if (!authStore.isLoggedIn) {
    return
  }
  if (!isDevicesPageInForeground()) {
    return
  }
  await refreshDevices()
}

/// focus/visibilitychange 直接走 autoRefresh：刷新频率由 device store 的
/// 10s 节流统一兜底（审计 G-5：原先页面层再叠一层同参数节流，语义重复）。
async function handleForegroundResume() {
  if (!isDevicesPageInForeground()) {
    return
  }
  await autoRefreshDevicesInForeground()
}

async function refreshTunnelStatusMap() {
  try {
    const serviceStatus = await invoke<any>('get_service_status').catch(() => null)
    applyTunnelRuntimeStatus(serviceStatus?.runtime)
  } catch (e) {
    console.error('[Devices] 刷新隧道状态失败:', e)
  }
}

function applyTunnelRuntimeStatus(runtime: any) {
    const newMap: Record<number, TunnelInfo> = {}
    const newJobMap: Record<number, TunnelJobStatus> = {}
    const activeJobs = runtime?.active_tunnel_jobs
    if (Array.isArray(activeJobs)) {
      for (const job of activeJobs) {
        newJobMap[job.peer_device_id] = job
        if (job.state === 'succeeded' && job.result?.success) {
          newMap[job.peer_device_id] = {
            role: 'active',
          }
        }
      }
    }
    const wgvpnJobs = runtime?.wgvpn_jobs
    if (Array.isArray(wgvpnJobs)) {
      for (const job of wgvpnJobs as TunnelJobStatus[]) {
        if (!job.is_active) continue
        newJobMap[job.peer_device_id] = {
          peer_device_id: job.peer_device_id,
          peer_device_uuid: '',
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
        newMap[session.peer_device_id] = {
          network: session.network || '',
          virtual_ip: session.peer_virtual_ip || '',
          exposed_lan_cidrs: Array.isArray(session.exposed_lan_cidrs) ? session.exposed_lan_cidrs : [],
          health_state: session.health_state || 'connected',
          role,
          latency_ms: typeof session.latency_ms === 'number' ? session.latency_ms : null,
        }
      }
    }
    const newLifecycleMap: Record<number, TunnelLifecycleStatus> = {}
    if (Array.isArray(runtime?.tunnel_lifecycles)) {
      for (const lifecycle of runtime.tunnel_lifecycles as TunnelLifecycleStatus[]) {
        const liveRole = liveRoleMap.get(lifecycle.peer_device_id)
        if (liveRole && lifecycle.role !== liveRole) continue
        const existing = newLifecycleMap[lifecycle.peer_device_id]
        const priority = lifecycle.state === 'connected' ? 3 : lifecycle.state === 'recovering' ? 2 : lifecycle.state === 'awaiting_approval' ? 1 : lifecycle.state === 'connecting' ? 1 : 0
        const existingPriority = existing?.state === 'connected' ? 3 : existing?.state === 'recovering' ? 2 : existing?.state === 'awaiting_approval' ? 1 : existing?.state === 'connecting' ? 1 : 0
        if (!existing || priority > existingPriority || lifecycle.role === liveRole) {
          newLifecycleMap[lifecycle.peer_device_id] = lifecycle
        }
      }
    }
    tunnelLifecycleMap.value = newLifecycleMap
    tunnelStatusMap.value = newMap
}

function openAliasDialog(device: DeviceInfo) {
  selectDevice(device)
  aliasForm.alias = device.device_alias || ''
  aliasDialogVisible.value = true
}

function openTunnelAction(device: DeviceInfo) {
  const lifecycle = deviceTunnelLifecycle(device)
  // 宽限期耗尽后 lifecycle 已回到未建立，但 service 快照中可能短暂保留旧会话。
  // 这时必须一次点击完成旧会话清理和新建，不能先把用户操作误当成“断开”。
  if (lifecycle.state === 'recovering') {
    void handleCancelRecoveringTunnel(device)
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
  if (lifecycle.state === 'connecting' || lifecycle.state === 'awaiting_approval' || activeTunnelJob(device)) {
    void handleCancelActiveTunnelJob(device)
    return
  }
  // Android 当前仅支持主动发起隧道，不能作为被动接收端。
  // 仅在真正启动后台建链任务前拦截，保留已有任务的取消能力。
  if (isAndroidDevice(device)) {
    ElMessage.error(t('devices.message.android_passive_unsupported'))
    return
  }
  if (warnIfWindowsRdpUnavailable(device)) return
  void startTunnelSilently(device)
}

async function handleCancelRecoveringTunnel(device: DeviceInfo) {
  if (disconnectingIds.value.has(device.device_id)) return
  const next = new Set(disconnectingIds.value)
  next.add(device.device_id)
  disconnectingIds.value = next
  try {
    const lifecycle = deviceTunnelLifecycle(device)
    const serviceStatus = lifecycle.role === 'passive'
      ? await invoke<any>('stop_service_tunnel', { sourceDeviceId: device.device_id })
      : await invoke<any>('stop_service_active_tunnel', { peerDeviceId: device.device_id })
    applyTunnelRuntimeStatus(serviceStatus?.runtime)
    ElMessage.success(t('devices.message.auto_tunnel_cancelled'))
  } catch (error) {
    const message = errorMessage(error, t('common.unknown_error'))
    ElMessage.error(t('devices.message.auto_tunnel_cancel_failed', { error: message }))
  } finally {
    const latest = new Set(disconnectingIds.value)
    latest.delete(device.device_id)
    disconnectingIds.value = latest
  }
}

async function handleRetryTunnel(device: DeviceInfo) {
  const preparing = new Set(preparingTunnelIds.value)
  preparing.add(device.device_id)
  preparingTunnelIds.value = preparing
  try {
    await invoke('stop_service_active_tunnel', { peerDeviceId: device.device_id })
    // A native punch call may still be draining. Starting early would reuse
    // the old job rather than the newly saved machine preferences.
    const deadline = Date.now() + 150_000
    while (true) {
      const status = await invoke<any>('get_service_status')
      const job = status.runtime?.active_tunnel_jobs?.find((item: TunnelJobStatus) => item.peer_device_id === device.device_id)
      if (!job || !['running', 'waiting'].includes(job.state)) break
      if (Date.now() >= deadline) throw new Error(t('app.settings.draining'))
      await new Promise(resolve => window.setTimeout(resolve, 500))
    }
    await invoke<string>('start_service_active_tunnel', {
      peerDeviceId: device.device_id,
      peerDeviceUuid: device.device_uuid,
      connectCode: null,
      temporaryPassword: null,
      lanCidrs: [],
    })
    ElMessage.success(t('devices.message.reconnect_started'))
    await refreshTunnelStatusMap()
  } catch (error) {
    const message = errorMessage(error, t('common.unknown_error'))
    ElMessage.error(t('devices.message.reconnect_failed', { error: message }))
    await refreshTunnelStatusMap()
  } finally {
    const latest = new Set(preparingTunnelIds.value)
    latest.delete(device.device_id)
    preparingTunnelIds.value = latest
  }
}

async function disableTcpAndRetry(device: DeviceInfo) {
  try {
    const preferences = await invoke<{ prefer_ipv6: boolean; prefer_tcp: boolean }>('get_connection_preferences')
    await invoke('save_connection_preferences', { preferIpv6: preferences.prefer_ipv6, preferTcp: false })
    await handleRetryTunnel(device)
  } catch (error) { ElMessage.error(String(error)) }
}

const SYMMETRIC_NAT_HELP_URL = 'https://www.p2premote.top/zh/docs/improve-p2p-success'
function openSymmetricNatHelp() {
  void openExternal(SYMMETRIC_NAT_HELP_URL)
}

async function startTunnelSilently(device: DeviceInfo) {
  const preparing = new Set(preparingTunnelIds.value)
  preparing.add(device.device_id)
  preparingTunnelIds.value = preparing
  try {
    await invoke<string>('start_service_active_tunnel', {
      peerDeviceId: device.device_id,
      peerDeviceUuid: device.device_uuid,
      connectCode: null,
      temporaryPassword: null,
      lanCidrs: [],
    })
    ElMessage.success(t('devices.message.auto_tunnel_started'))
  } catch (error) {
    const message = errorMessage(error, t('common.unknown_error'))
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
    await invoke('stop_active_tunnel_job', { peerDeviceId: device.device_id })
    delete activeTunnelJobMap.value[device.device_id]
    ElMessage.success(t('devices.message.auto_tunnel_cancelled'))
  } catch (error) {
    ElMessage.error(t('devices.message.auto_tunnel_cancel_failed', { error: (error as any).message }))
  }
}

async function handleDisconnectTunnel(device: DeviceInfo) {
  if (disconnectingIds.value.has(device.device_id)) return
  try {
    await ElMessageBox.confirm(
      t('devices.message.disconnect_confirm_body', { name: device.device_alias || device.device_name }),
      t('devices.message.disconnect_confirm_title'),
      {
        confirmButtonText: t('devices.detail.connection.disconnect'),
        cancelButtonText: t('common.cancel'),
        type: 'warning',
        confirmButtonClass: 'el-button--danger',
      },
    )
  } catch {
    return
  }
  const next = new Set(disconnectingIds.value)
  next.add(device.device_id)
  disconnectingIds.value = next
  try {
    const lifecycle = deviceTunnelLifecycle(device)
    const serviceStatus = lifecycle.role === 'passive'
      ? await invoke<any>('stop_service_tunnel', { sourceDeviceId: device.device_id })
      : await invoke<any>('stop_service_active_tunnel', { peerDeviceId: device.device_id })
    applyTunnelRuntimeStatus(serviceStatus?.runtime)
    ElMessage.success(t('devices.message.tunnel_disconnected'))
  } catch (error) {
    const message = errorMessage(error, t('common.unknown_error'))
    ElMessage.error(t('devices.message.disconnect_failed', { error: message }))
  } finally {
    const latest = new Set(disconnectingIds.value)
    latest.delete(device.device_id)
    disconnectingIds.value = latest
  }
}

async function copyRemoteDesktopAddress(device: DeviceInfo) {
  selectDevice(device)
  try {
    const address = remoteDesktopAddress(device)
    if (!address) {
      ElMessage.error(t('devices.message.no_rdp_address'))
      return
    }
    await copyTextToClipboard(address)
    ElMessage.success(t('devices.message.rdp_address_copied', { address }))
  } catch {
    ElMessage.error(t('devices.message.copy_rdp_address_failed'))
  }
}

async function launchWindowsRdp(device: DeviceInfo) {
  const address = remoteDesktopAddress(device)
  if (!address) {
    ElMessage.error(t('devices.message.no_rdp_address'))
    return
  }
  try {
    await invoke('launch_windows_rdp', { address })
    ElMessage.success(t('devices.message.windows_rdp_started'))
  } catch (e) {
    ElMessage.error(t('devices.message.connect_failed', { error: String(e) }))
  }
}

async function launchP2pRemoteDesktop(device: DeviceInfo) {
  if (!hasCapability(device, 'rustdesk_tiny')) {
    ElMessage.error(t('devices.message.desktop_capability_missing'))
    return
  }
  const address = rustdeskTinyAddress(device)
  if (!address) {
    ElMessage.error(t('devices.message.no_rustdesk_tiny_address'))
    return
  }

  let addressCopied = false
  try {
    await copyTextToClipboard(address)
    addressCopied = true
  } catch {
    // Launch can still use --connect; the copy failure is reported only if
    // launching also fails so the user gets the most useful next step.
  }

  try {
    await invoke('launch_rustdesk_tiny', { address })
    ElMessage.success(t('devices.message.desktop_window_started', { address }))
  } catch (e) {
    if (addressCopied) {
      ElMessage.warning(t('devices.message.desktop_address_copied', { address }))
    } else {
      const msg = errorMessage(e, t('common.unknown_error'))
      ElMessage.error(t('devices.message.connect_failed', { error: msg }))
    }
  }
}

async function handleDeviceAction(command: string, device: DeviceInfo) {
  selectDevice(device)
  switch (command) {
    case 'alias':
      openAliasDialog(device)
      break
    case 'delete':
      await confirmDeleteDevice(device)
      break
  }
}

async function handleWakeDevice(device: DeviceInfo) {
	if (wakingIds.has(device.device_id)) return
	wakingIds.add(device.device_id)
	try {
		const status = await invoke<string>('wake_device', { deviceId: device.device_id })
		if (status === 'sent') ElMessage.success(t('devices.wol.sent'))
		else ElMessage.error(t(`devices.wol.${status}`))
	} catch (e) { ElMessage.error(t('devices.wol.failed', { error: String(e) })) }
	finally { wakingIds.delete(device.device_id) }
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
  /* 顶部注册失败告警条与设备面板纵向排布 */
  display: flex;
  flex-direction: column;
}

.registration-error-alert {
  margin: 16px 20px 0;
}

.devices-shell {
  display: grid;
  grid-template-columns: 340px minmax(0, 1fr);
  gap: 16px;
  flex: 1;
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
  grid-template-columns: 24px minmax(0, 1fr);
  column-gap: 10px;
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

.device-list-platform {
  width: 24px;
  height: 24px;
  margin-top: 1px;
  display: inline-flex;
  align-items: center;
  justify-content: center;
  font-size: 21px;
  line-height: 1;
  --platform-icon-cutout: var(--fluent-layer);
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

@media (prefers-reduced-motion: reduce) {
  .device-list-platform :deep(.tone-connecting) {
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
  --platform-icon-cutout: var(--fluent-layer);
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
  color: var(--fluent-warning);
  background: var(--fluent-warning-bg);
  border: 1px solid var(--fluent-warning-stroke);
}

/* 已连接：仅用绿色图标表达状态，避免色块降低图标辨识度 */
.device-avatar.connected {
  color: var(--fluent-success);
  background: transparent;
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

.action-groups {
  display: grid;
  gap: 18px;
}

.action-group {
  display: grid;
  gap: 8px;
}

.action-group-label {
  color: var(--fluent-text-secondary);
  font-size: 12px;
  font-weight: 600;
  letter-spacing: 0.03em;
}

.action-grid {
  display: grid;
  /* 列宽封顶 280px：宽窗口下自动增加列数，而不是把按钮拉长 */
  grid-template-columns: repeat(auto-fill, minmax(200px, 280px));
  gap: 10px;
  justify-content: start;
}

.action-grid-primary {
  grid-template-columns: repeat(auto-fill, minmax(220px, 320px));
}

/* ===== 操作瓦片：Fluent 按钮（去位移反馈，改背景层变化） ===== */
.action-tile-wrap {
  display: block;
  min-width: 0;
}

.action-tile-wrap .action-tile {
  width: 100%;
}

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

@media (hover: hover) and (pointer: fine) {
  .action-tile:hover:not(:disabled) {
    background: var(--fluent-layer-hover);
    border-color: var(--fluent-stroke-strong);
  }
}

.action-tile:active:not(:disabled) {
  background: var(--fluent-layer-pressed);
  transform: scale(0.97);
}

.action-tile:disabled {
  cursor: not-allowed;
  opacity: 0.45;
}

/* 品牌图标（mstsc / RustDeskTiny）：与 el-icon 同尺寸对齐 */
.action-tile-icon {
  display: block;
  width: 20px;
  height: 20px;
  flex-shrink: 0;
  object-fit: contain;
}

.action-tile .el-icon {
  font-size: 20px;
  flex-shrink: 0;
}

.action-tile span {
  font-size: 14px;
  font-weight: 500;
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

@media (hover: hover) and (pointer: fine) {
  .action-tile.primary:hover:not(:disabled) {
    background: var(--fluent-accent-hover);
    border-color: var(--fluent-accent-hover);
  }
}

.capability-list {
  display: flex;
  flex-wrap: wrap;
  gap: 6px;
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
  color: color-mix(in srgb, var(--fluent-text-on-accent) 85%, transparent);
}

.action-tile.primary .action-help:hover {
  color: var(--fluent-text-on-accent);
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

  .registration-error-alert {
    margin: 14px 14px 0;
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

  .info-grid,
  .action-grid {
    grid-template-columns: 1fr;
  }

}
</style>
