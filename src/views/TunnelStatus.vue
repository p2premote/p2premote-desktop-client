<template>
  <div
    class="tunnel-status-page"
    v-loading="disconnectingIds.size > 0"
    :element-loading-text="$t('tunnel.message.disconnecting')"
  >
    <section class="tunnel-card fluent-card">
      <div class="card-heading">
        <div>
          <h2>{{ $t('tunnel.page.title') }} <span class="tunnel-count">{{ tunnels.length }}</span></h2>
          <p>{{ $t('tunnel.page.subtitle') }}</p>
        </div>
        <el-button class="refresh-button" :icon="Refresh" :loading="loading" @click="refreshStatus">{{ $t('common.refresh') }}</el-button>
      </div>

      <el-empty v-if="!loading && tunnels.length === 0" :description="$t('tunnel.page.empty')" />

      <div v-else class="tunnel-groups">
        <section v-for="group in tunnelGroups" :key="group.role" class="tunnel-group">
          <h3 class="group-heading">
            {{ group.role === 'active' ? $t('tunnel.role.active') : $t('tunnel.role.passive') }}
            <span>{{ group.items.length }}</span>
          </h3>
          <div class="tunnel-list">
            <article v-for="tunnel in group.items" :key="tunnelKey(tunnel)" class="tunnel-item">
              <div class="tunnel-summary">
                <div class="tunnel-main">
                  <div class="device-avatar">
                    <el-icon><Monitor /></el-icon>
                  </div>
                  <div class="tunnel-info">
                    <div class="tunnel-title-row">
                      <strong>{{ deviceLabel(tunnel) }}</strong>
                      <el-tag size="small" :type="lifecycleTagType(tunnel)">
                        {{ lifecycleLabel(tunnel) }}
                      </el-tag>
                    </div>
                    <div class="tunnel-overview">
                      <span v-if="tunnel.role === 'passive'">{{ $t('tunnel.info.from_user', { user: userLabel(tunnel) }) }}</span>
                      <span>{{ connectedDuration(tunnel) }}</span>
                      <span>{{ latencyLabel(tunnel) }}</span>
                      <span class="traffic" :aria-label="$t('tunnel.info.traffic_summary', {
                        download: formatBytes(tunnel.received_bytes),
                        upload: formatBytes(tunnel.transmitted_bytes),
                      })">
                        ↓ {{ formatBytes(tunnel.received_bytes) }}
                        <span aria-hidden="true">·</span>
                        ↑ {{ formatBytes(tunnel.transmitted_bytes) }}
                      </span>
                    </div>
                    <div v-if="shouldShowLifecycleMessage(tunnel)" class="tunnel-alert">
                      <span v-if="tunnel.lifecycle_message">{{ tunnel.lifecycle_message }}</span>
                      <span v-if="tunnel.health_state === 'degraded'">
                        {{ $t('tunnel.info.heartbeat_failed', { count: tunnel.consecutive_failures || 12 }) }}
                      </span>
                      <span v-if="tunnel.health_state === 'degraded' && tunnel.health_grace_deadline">
                        {{ $t('tunnel.info.grace_remaining', { remaining: formatGraceRemaining(tunnel.health_grace_deadline) }) }}
                      </span>
                    </div>
                  </div>
                </div>
                <div class="tunnel-actions">
                  <el-button
                    v-if="tunnel.role === 'active' && tunnel.lifecycle_state === 'recovering'"
                    type="primary"
                    plain
                    :loading="reconnectingIds.has(tunnelKey(tunnel))"
                    @click="retryTunnel(tunnel)"
                  >
                    {{ $t('tunnel.action.retry') }}
                  </el-button>
                  <el-button
                    v-if="tunnel.lifecycle_state === 'connected' || tunnel.lifecycle_state === 'recovering'"
                    type="danger"
                    text
                    :loading="disconnectingIds.has(tunnelKey(tunnel))"
                    @click="disconnectTunnel(tunnel)"
                  >
                    {{ $t('tunnel.action.disconnect') }}
                  </el-button>
                </div>
              </div>
              <button
                type="button"
                class="details-toggle"
                :aria-expanded="expandedIds.has(tunnelKey(tunnel))"
                :aria-controls="detailsId(tunnel)"
                @click="toggleDetails(tunnel)"
              >
                {{ expandedIds.has(tunnelKey(tunnel)) ? $t('tunnel.action.hide_network_details') : $t('tunnel.action.show_network_details') }}
                <el-icon :class="{ expanded: expandedIds.has(tunnelKey(tunnel)) }"><ArrowDown /></el-icon>
              </button>
              <div v-show="expandedIds.has(tunnelKey(tunnel))" :id="detailsId(tunnel)" class="network-details">
                <div><span>{{ $t('tunnel.info.peer_device_label') }}</span><strong>{{ deviceLabel(tunnel) }}</strong></div>
                <div v-if="tunnel.peer_virtual_ip">
                  <span>{{ $t('tunnel.info.peer_virtual_ip_label') }}</span>
                  <strong>{{ tunnel.peer_virtual_ip }}</strong>
                  <button type="button" class="copy-button" :aria-label="$t('tunnel.action.copy_peer_ip')" @click="copyValue(tunnel.peer_virtual_ip)"><el-icon><CopyDocument /></el-icon></button>
                </div>
                <div v-if="tunnel.virtual_ip">
                  <span>{{ $t('tunnel.info.local_virtual_ip_label') }}</span>
                  <strong>{{ tunnel.virtual_ip }}</strong>
                  <button type="button" class="copy-button" :aria-label="$t('tunnel.action.copy_local_ip')" @click="copyValue(tunnel.virtual_ip)"><el-icon><CopyDocument /></el-icon></button>
                </div>
                <div v-if="tunnel.peer_public_ip"><span>{{ $t('tunnel.info.public_ip_label') }}</span><strong>{{ tunnel.peer_public_ip }}</strong></div>
                <div><span>{{ $t('tunnel.info.connected_at_label') }}</span><strong>{{ tunnel.connected_at ? formatTime(tunnel.connected_at) : $t('tunnel.info.current_session') }}</strong></div>
              </div>
            </article>
          </div>
        </section>
      </div>
    </section>
  </div>
</template>

<script setup lang="ts">
import { computed, onMounted, onUnmounted, ref } from 'vue'
import { useI18n } from 'vue-i18n'
import { invoke, listen, type UnlistenFn } from '../runtime/bridge'
import { ElMessage } from 'element-plus/es/components/message/index.mjs'
import { ElMessageBox } from 'element-plus/es/components/message-box/index.mjs'
import { ArrowDown, CopyDocument, Monitor, Refresh } from '@element-plus/icons-vue'
import { useDeviceStore } from '../stores/device'
const { t } = useI18n()

interface TunnelItem {
  role: 'active' | 'passive'
  peer_device_id: number
  source_user_id?: number
  source_username?: string
  source_email?: string
  source_device_name?: string
  source_device_alias?: string
  peer_device_name?: string
  peer_device_alias?: string
  peer_public_ip?: string
  connected_at?: number
  virtual_ip?: string
  peer_virtual_ip?: string
  health_state?: 'connected' | 'degraded'
  consecutive_failures?: number
  health_grace_deadline?: number | null
  latency_ms?: number | null
  received_bytes: number
  transmitted_bytes: number
  lifecycle_state: 'not_established' | 'connecting' | 'awaiting_approval' | 'connected' | 'recovering'
  lifecycle_message?: string
  attempt?: number
  max_attempts?: number
}

interface WgvpnSessionStatus {
  peer_device_id: number
  /** true=主动发起端，false=被连接的被动端；被动会话可能仍等待本地审批。 */
  is_active: boolean
  approval_pending?: boolean
  virtual_ip: string
  peer_virtual_ip: string
  health_state: 'connected' | 'degraded'
  consecutive_failures: number
  health_grace_deadline?: number | null
  latency_ms?: number | null
  received_bytes?: number
  transmitted_bytes?: number
}

interface TunnelLifecycleStatus {
  peer_device_id: number
  source_user_id?: number
  source_username?: string
  source_email?: string
  peer_device_name?: string
  peer_device_alias?: string
  peer_public_ip?: string
  role: 'active' | 'passive'
  state: 'not_established' | 'connecting' | 'awaiting_approval' | 'connected' | 'recovering'
  attempt: number
  max_attempts: number
  message?: string | null
  last_result: string
  virtual_ip?: string | null
  peer_virtual_ip?: string | null
  health_failures?: number
  health_grace_deadline?: number | null
  connected_at?: number | null
  updated_at: number
}

interface RuntimeStatus {
  wgvpn_sessions?: WgvpnSessionStatus[]
  tunnel_lifecycles?: TunnelLifecycleStatus[]
}

interface ServiceStatusResponse {
  runtime?: RuntimeStatus | null
}

const runtimeStatus = ref<RuntimeStatus | null>(null)
const deviceStore = useDeviceStore()
const loading = ref(false)
const disconnectingIds = ref(new Set<string>())
const reconnectingIds = ref(new Set<string>())
const expandedIds = ref(new Set<string>())
const currentTime = ref(Math.floor(Date.now() / 1000))
let unlistenServiceStatus: UnlistenFn | null = null
let countdownTimer: ReturnType<typeof window.setInterval> | null = null

const tunnels = computed(() => {
  const merged = new Map<string, TunnelItem>()
  for (const session of runtimeStatus.value?.wgvpn_sessions || []) {
    const device = deviceStore.devices.find(item => item.device_id === session.peer_device_id)
    const role = session.is_active ? 'active' : 'passive'
    merged.set(`${role}-${session.peer_device_id}`, {
      source_device_name: device?.device_name,
      source_device_alias: device?.device_alias,
      role,
      peer_device_id: session.peer_device_id,
      virtual_ip: session.virtual_ip,
      peer_virtual_ip: session.peer_virtual_ip,
      health_state: session.health_state,
      consecutive_failures: session.consecutive_failures,
      health_grace_deadline: session.health_grace_deadline,
      latency_ms: session.latency_ms,
      received_bytes: session.received_bytes || 0,
      transmitted_bytes: session.transmitted_bytes || 0,
      lifecycle_state: session.approval_pending
        ? 'awaiting_approval'
        : session.health_state === 'degraded' ? 'recovering' : 'connected',
    })
  }
  for (const lifecycle of runtimeStatus.value?.tunnel_lifecycles || []) {
    // lifecycle 会保留最近一次任务结果，供设备页展示失败/取消原因；隧道状态页
    // 只展示当前正在建立、已连接或处于恢复宽限期的活跃隧道。
    if (lifecycle.state === 'not_established') continue
    const key = `${lifecycle.role}-${lifecycle.peer_device_id}`
    const existing = merged.get(key)
    const device = deviceStore.devices.find(item => item.device_id === lifecycle.peer_device_id)
    merged.set(key, {
      ...existing,
      role: lifecycle.role,
      peer_device_id: lifecycle.peer_device_id,
      source_device_name: existing?.source_device_name || device?.device_name,
      source_device_alias: existing?.source_device_alias || device?.device_alias,
      peer_device_name: lifecycle.peer_device_name || existing?.peer_device_name,
      peer_device_alias: lifecycle.peer_device_alias || existing?.peer_device_alias,
      peer_public_ip: lifecycle.peer_public_ip || existing?.peer_public_ip,
      source_user_id: existing?.source_user_id || lifecycle.source_user_id,
      source_username: existing?.source_username || lifecycle.source_username,
      source_email: existing?.source_email || lifecycle.source_email,
      virtual_ip: existing?.virtual_ip || lifecycle.virtual_ip || undefined,
      peer_virtual_ip: existing?.peer_virtual_ip || lifecycle.peer_virtual_ip || undefined,
      health_state: lifecycle.state === 'recovering' ? 'degraded' : 'connected',
      consecutive_failures: lifecycle.health_failures || existing?.consecutive_failures,
      health_grace_deadline: lifecycle.health_grace_deadline || existing?.health_grace_deadline,
      received_bytes: existing?.received_bytes || 0,
      transmitted_bytes: existing?.transmitted_bytes || 0,
      lifecycle_state: lifecycle.state,
      lifecycle_message: lifecycle.message || undefined,
      attempt: lifecycle.attempt,
      max_attempts: lifecycle.max_attempts,
      connected_at: existing?.connected_at || lifecycle.connected_at || lifecycle.updated_at,
    })
  }
  return [...merged.values()].sort((a, b) => {
    return (b.connected_at || 0) - (a.connected_at || 0)
  })
})

const tunnelGroups = computed(() => (['active', 'passive'] as const)
  .map(role => ({ role, items: tunnels.value.filter(tunnel => tunnel.role === role) }))
  .filter(group => group.items.length > 0))

onMounted(() => {
  countdownTimer = window.setInterval(() => {
    currentTime.value = Math.floor(Date.now() / 1000)
  }, 1000)
  void refreshStatus()
  listen<RuntimeStatus>('service-status-changed', (event) => {
    runtimeStatus.value = event.payload || null
  }).then(fn => {
    unlistenServiceStatus = fn
  }).catch(error => {
    console.warn('[TunnelStatus] listen service status failed:', error)
  })
})

onUnmounted(() => {
  if (countdownTimer !== null) {
    window.clearInterval(countdownTimer)
    countdownTimer = null
  }
  unlistenServiceStatus?.()
  unlistenServiceStatus = null
})

async function refreshStatus() {
  loading.value = true
  try {
    const status = await invoke<ServiceStatusResponse>('refresh_tunnel_status')
    runtimeStatus.value = status.runtime || null
  } catch (error) {
    const message = typeof error === 'string' ? error : (error as Error)?.message || t('common.unknown_error')
    ElMessage.error(t('tunnel.message.fetch_failed', { error: message }))
  } finally {
    loading.value = false
  }
}

async function disconnectTunnel(tunnel: TunnelItem) {
  const key = tunnelKey(tunnel)
  if (disconnectingIds.value.has(key)) return
  try {
    await ElMessageBox.confirm(
      t('tunnel.message.disconnect_confirm_body', { name: deviceLabel(tunnel) }),
      t('tunnel.message.disconnect_confirm_title'),
      {
        confirmButtonText: t('tunnel.action.disconnect'),
        cancelButtonText: t('common.cancel'),
        type: 'warning',
        confirmButtonClass: 'el-button--danger',
      },
    )
  } catch {
    return
  }
  const next = new Set(disconnectingIds.value)
  next.add(key)
  disconnectingIds.value = next
  try {
    const status = tunnel.role === 'active'
      ? await invoke<ServiceStatusResponse>('stop_service_active_tunnel', {
          targetDeviceId: tunnel.peer_device_id,
        })
      : await invoke<ServiceStatusResponse>('stop_service_tunnel', {
          sourceDeviceId: tunnel.peer_device_id,
        })
    runtimeStatus.value = status.runtime || null
    ElMessage.success(t('tunnel.message.disconnected'))
  } catch (error) {
    const message = typeof error === 'string' ? error : (error as Error)?.message || t('common.unknown_error')
    ElMessage.error(t('tunnel.message.disconnect_failed', { error: message }))
  } finally {
    const latest = new Set(disconnectingIds.value)
    latest.delete(key)
    disconnectingIds.value = latest
  }
}

async function retryTunnel(tunnel: TunnelItem) {
  const device = deviceStore.devices.find(item => item.device_id === tunnel.peer_device_id)
  if (!device?.device_uuid) {
    ElMessage.error(t('tunnel.message.peer_identity_missing'))
    return
  }
  const key = tunnelKey(tunnel)
  const next = new Set(reconnectingIds.value)
  next.add(key)
  reconnectingIds.value = next
  try {
    await invoke('stop_service_active_tunnel', {
      targetDeviceId: tunnel.peer_device_id,
    })
    await invoke<string>('start_service_active_tunnel', {
      targetDeviceId: tunnel.peer_device_id,
      targetDeviceUuid: device.device_uuid,
      connectCode: null,
      temporaryPassword: null,
      lanCidrs: [],
    })
    ElMessage.success(t('tunnel.message.reconnect_started'))
    await refreshStatus()
  } catch (error) {
    const message = typeof error === 'string' ? error : (error as Error)?.message || t('common.unknown_error')
    ElMessage.error(t('tunnel.message.reconnect_failed', { error: message }))
    await refreshStatus()
  } finally {
    const latest = new Set(reconnectingIds.value)
    latest.delete(key)
    reconnectingIds.value = latest
  }
}

function tunnelKey(tunnel: TunnelItem): string {
  return `${tunnel.role}-${tunnel.peer_device_id}`
}

function lifecycleLabel(tunnel: TunnelItem): string {
  if (tunnel.lifecycle_state === 'connecting') {
    return t('tunnel.info.building_progress', { attempt: tunnel.attempt || 1, max: tunnel.max_attempts || 30 })
  }
  return t(`tunnel.state.${tunnel.lifecycle_state}`)
}

function lifecycleTagType(tunnel: TunnelItem): 'success' | 'warning' | 'info' | 'primary' {
  if (tunnel.lifecycle_state === 'connected') return 'success'
  if (tunnel.lifecycle_state === 'awaiting_approval') return 'warning'
  if (tunnel.lifecycle_state === 'recovering') return 'warning'
  if (tunnel.lifecycle_state === 'connecting') return 'primary'
  return 'info'
}

function deviceLabel(tunnel: TunnelItem): string {
  return tunnel.peer_device_alias
    || tunnel.peer_device_name
    || tunnel.source_device_alias
    || tunnel.source_device_name
    || t('tunnel.info.unknown_device', { id: tunnel.peer_device_id })
}

function detailsId(tunnel: TunnelItem): string {
  return `tunnel-details-${tunnel.role}-${tunnel.peer_device_id}`
}

function toggleDetails(tunnel: TunnelItem) {
  const next = new Set(expandedIds.value)
  const key = tunnelKey(tunnel)
  next.has(key) ? next.delete(key) : next.add(key)
  expandedIds.value = next
}

function shouldShowLifecycleMessage(tunnel: TunnelItem): boolean {
  return tunnel.lifecycle_state !== 'connected'
}

function latencyLabel(tunnel: TunnelItem): string {
  if (tunnel.health_state === 'degraded') return t('tunnel.info.latency_timeout')
  if (typeof tunnel.latency_ms !== 'number') return t('tunnel.info.latency_unavailable')
  return t('tunnel.info.latency', { latency: tunnel.latency_ms })
}

function connectedDuration(tunnel: TunnelItem): string {
  if (!tunnel.connected_at) return t('tunnel.info.current_session')
  const seconds = Math.max(0, currentTime.value - tunnel.connected_at)
  const days = Math.floor(seconds / 86400)
  const hours = Math.floor((seconds % 86400) / 3600)
  const minutes = Math.floor((seconds % 3600) / 60)
  if (days > 0) return t('tunnel.info.connected_duration_days', { days, hours })
  if (hours > 0) return t('tunnel.info.connected_duration_hours', { hours, minutes })
  return t('tunnel.info.connected_duration_minutes', { minutes })
}

async function copyValue(value: string) {
  try {
    await navigator.clipboard.writeText(value)
    ElMessage.success(t('tunnel.message.copied'))
  } catch {
    ElMessage.error(t('tunnel.message.copy_failed'))
  }
}

function userLabel(tunnel: TunnelItem): string {
  if (tunnel.source_username) return tunnel.source_username
  if (tunnel.source_email) return tunnel.source_email
  if (tunnel.source_user_id) return t('tunnel.info.user_id', { id: tunnel.source_user_id })
  return t('common.unknown_user')
}

function formatTime(value?: number): string {
  if (!value) return t('common.unknown')
  const date = new Date(value * 1000)
  if (Number.isNaN(date.getTime())) return t('common.unknown')
  return date.toLocaleString('zh-CN', { hour12: false })
}

function formatGraceRemaining(deadline: number): string {
  const remaining = Math.max(0, deadline - currentTime.value)
  const minutes = Math.floor(remaining / 60)
  const seconds = remaining % 60
  return `${String(minutes).padStart(2, '0')}:${String(seconds).padStart(2, '0')}`
}

function formatBytes(bytes: number): string {
  if (!Number.isFinite(bytes) || bytes <= 0) return '0 B'
  const units = ['B', 'KB', 'MB', 'GB', 'TB']
  const unitIndex = Math.min(Math.floor(Math.log(bytes) / Math.log(1024)), units.length - 1)
  const value = bytes / Math.pow(1024, unitIndex)
  const precision = unitIndex === 0 || value >= 100 ? 0 : value >= 10 ? 1 : 2
  return `${value.toFixed(precision)} ${units[unitIndex]}`
}
</script>

<style scoped>
.tunnel-status-page {
  min-height: calc(100% + 40px);
  margin: -20px -32px;
  padding: 16px 20px 20px;
  box-sizing: border-box;
  background: var(--fluent-bg);
}

/* 卡片视觉由 .fluent-card 工具类提供，此处仅保留尺寸 */
.tunnel-card {
  min-height: 280px;
  padding: 24px;
}

.tunnel-actions {
  display: flex;
  flex-shrink: 0;
  align-items: center;
  gap: 8px;
}

.tunnel-actions .el-button + .el-button {
  margin-left: 0;
}

.card-heading {
  display: flex;
  align-items: flex-start;
  justify-content: space-between;
  gap: 16px;
  margin-bottom: 22px;
}

.card-heading p {
  margin: 5px 0 0;
  color: var(--fluent-text-secondary);
  font-size: 13px;
}

.card-heading h2 {
  margin: 0;
  color: var(--fluent-text);
  font-size: var(--text-page-title);
  line-height: 1.4;
  font-weight: 600;
}

.tunnel-count {
  display: inline-flex;
  align-items: center;
  justify-content: center;
  min-width: 22px;
  height: 22px;
  margin-left: 6px;
  padding: 0 6px;
  border-radius: var(--fluent-radius-pill);
  color: var(--fluent-accent);
  background: var(--fluent-accent-light);
  font-size: 12px;
  vertical-align: 2px;
}

/* 刷新按钮由全局 EP 主题统一接管，此处仅微调圆角 */
.refresh-button {
  border-radius: var(--fluent-radius-md);
}

.tunnel-groups,
.tunnel-group,
.tunnel-list {
  display: flex;
  flex-direction: column;
}

.tunnel-groups {
  gap: 24px;
}

.tunnel-group {
  gap: 8px;
}

.group-heading {
  display: flex;
  align-items: center;
  gap: 7px;
  margin: 0;
  color: var(--fluent-text-secondary);
  font-size: 13px;
  font-weight: 600;
}

.group-heading span {
  display: inline-grid;
  min-width: 20px;
  height: 20px;
  padding: 0 5px;
  place-items: center;
  border-radius: var(--fluent-radius-pill);
  color: var(--fluent-text-secondary);
  background: var(--fluent-bg-subtle);
  font-size: 11px;
}

.tunnel-list {
  border-top: 1px solid var(--fluent-divider);
}

.tunnel-item {
  display: flex;
  flex-direction: column;
  padding: 16px 2px 12px;
  border: 0;
  border-bottom: 1px solid var(--fluent-divider);
  background: transparent;
}

.tunnel-summary {
  width: 100%;
  display: flex;
  align-items: center;
  justify-content: space-between;
  gap: 16px;
}

.tunnel-main {
  min-width: 0;
  display: flex;
  align-items: center;
  gap: 14px;
}

.device-avatar {
  width: 42px;
  height: 42px;
  display: grid;
  place-items: center;
  flex: 0 0 auto;
  color: var(--fluent-accent);
  background: var(--fluent-accent-light);
  border-radius: var(--fluent-radius-md);
}

.device-avatar .el-icon {
  font-size: 20px;
}

.tunnel-info {
  min-width: 0;
}

.tunnel-title-row {
  display: flex;
  align-items: center;
  gap: 8px;
  margin-bottom: 5px;
}

.tunnel-title-row strong {
  color: var(--fluent-text);
  font-size: 16px;
  font-weight: 600;
}

.tunnel-overview,
.tunnel-alert {
  display: flex;
  flex-wrap: wrap;
  gap: 5px 14px;
  color: var(--fluent-text-secondary);
  font-size: 13px;
}

.traffic {
  font-variant-numeric: tabular-nums;
}

.tunnel-alert {
  margin-top: 7px;
  color: var(--fluent-warning, #9a6700);
}

.details-toggle {
  align-self: flex-start;
  display: inline-flex;
  align-items: center;
  gap: 4px;
  margin: 8px 0 0 56px;
  padding: 3px 5px;
  border: 0;
  border-radius: var(--fluent-radius-sm);
  color: var(--fluent-text-secondary);
  background: transparent;
  font: inherit;
  font-size: 12px;
  cursor: pointer;
}

.details-toggle:hover {
  color: var(--fluent-text);
  background: var(--fluent-bg-subtle);
}

.details-toggle:focus-visible,
.copy-button:focus-visible {
  outline: 2px solid var(--fluent-accent);
  outline-offset: 2px;
}

.details-toggle .el-icon {
  transition: transform 160ms cubic-bezier(0.23, 1, 0.32, 1);
}

.details-toggle .el-icon.expanded {
  transform: rotate(180deg);
}

.network-details {
  width: calc(100% - 56px);
  display: grid;
  grid-template-columns: repeat(2, minmax(200px, 1fr));
  gap: 8px 24px;
  margin: 10px 0 0 56px;
  padding: 12px 14px;
  box-sizing: border-box;
  border-radius: var(--fluent-radius-md);
  background: var(--fluent-bg-subtle);
}

.network-details > div {
  min-width: 0;
  display: grid;
  grid-template-columns: minmax(90px, auto) minmax(0, 1fr) auto;
  align-items: center;
  gap: 8px;
  font-size: 12px;
}

.network-details span {
  color: var(--fluent-text-secondary);
}

.network-details strong {
  min-width: 0;
  overflow: hidden;
  color: var(--fluent-text);
  font-weight: 500;
  font-variant-numeric: tabular-nums;
  text-overflow: ellipsis;
  white-space: nowrap;
}

.copy-button {
  display: inline-grid;
  width: 24px;
  height: 24px;
  padding: 0;
  place-items: center;
  border: 0;
  border-radius: var(--fluent-radius-sm);
  color: var(--fluent-text-secondary);
  background: transparent;
  cursor: pointer;
}

.copy-button:hover {
  color: var(--fluent-accent);
  background: var(--fluent-accent-light);
}

@media (max-width: 900px) {
  .tunnel-summary {
    align-items: stretch;
    flex-direction: column;
  }

  .network-details {
    grid-template-columns: 1fr;
  }
}

@media (prefers-reduced-motion: reduce) {
  .details-toggle .el-icon {
    transition: none;
  }
}
</style>
