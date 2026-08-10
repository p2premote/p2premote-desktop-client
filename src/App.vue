<template>
  <el-config-provider :locale="epLocale">
    <div v-if="webAuthGateVisible" class="web-auth-shell">
      <div class="web-auth-card fluent-card">
        <AppLogo :size="48" />
        <h1>p2pRemote</h1>
        <p>{{ $t('app.web_auth.prompt') }}</p>
        <el-input
          v-model="webSecurityCode"
          type="password"
          show-password
          autofocus
          :placeholder="$t('app.web_auth.placeholder')"
          @keyup.enter="handleWebUnlock"
        />
        <el-alert v-if="webAuthError" :title="webAuthError" type="error" :closable="false" />
        <el-button type="primary" size="large" :loading="webAuthLoading" @click="handleWebUnlock">
          {{ $t('app.web_auth.enter') }}
        </el-button>
        <p class="web-auth-cert-note">{{ $t('app.web_auth.certificate_note') }}</p>
      </div>
    </div>
    <div v-else class="app-container">
    <div v-if="forceUpdateVisible" class="force-update-overlay">
      <div class="force-update-card">
        <h2>{{ $t('app.force_update.title') }}</h2>
        <p class="force-update-text">
          {{ $t('app.force_update.body', { current: updateInfo.current, min: updateInfo.minSupported || updateInfo.latest }) }}
        </p>
        <p v-if="updateInfo.releaseNotes" class="force-update-notes">
          {{ updateInfo.releaseNotes }}
        </p>
        <div class="force-update-actions">
          <el-button type="primary" size="large" @click="openUpdatePage">{{ $t('app.force_update.update_now') }}</el-button>
          <el-button v-if="isTauriRuntime()" size="large" @click="handleExitApp">{{ $t('app.force_update.exit') }}</el-button>
        </div>
      </div>
    </div>

    <el-container class="dashboard">
      <el-header
        class="app-titlebar"
        :class="{ 'drag-titlebar-enabled': titlebarDragEnabled }"
        v-bind="titlebarDragAttributes"
        @dblclick="handleTitlebarDoubleClick"
      >
        <div class="header-content" v-bind="titlebarDragAttributes">
          <div class="header-left">
            <AppLogo :size="24" style="vertical-align: middle; margin-right: 6px;" />
            <span class="app-title">p2pRemote</span>
          </div>
          <div class="header-actions no-drag">
            <el-popover placement="bottom-end" :width="300" trigger="click" transition="settings-popover">
              <template #reference>
                <el-button class="header-btn" :icon="Setting" text :aria-label="$t('app.settings.open_aria')" />
              </template>
              <div class="settings-panel">
                <section class="settings-group">
                  <div class="settings-group-title">{{ $t('app.settings.group_preferences') }}</div>
                  <div v-if="isTauriRuntime()" class="settings-item">
                    <span>{{ $t('app.settings.auto_start') }}</span>
                    <el-switch v-model="autoStart" @change="handleAutoStartChange" />
                  </div>
                  <div class="settings-item">
                    <span>{{ $t('app.settings.language') }}</span>
                    <el-select v-model="localeModel" class="language-select" size="small">
                      <el-option v-for="item in SUPPORTED_LOCALES" :key="item.value" :label="item.label" :value="item.value" />
                    </el-select>
                  </div>
                  <div class="settings-item">
                    <span>{{ $t('app.settings.theme') }}</span>
                    <el-select v-model="themeMode" class="theme-select" size="small">
                      <el-option :label="$t('app.settings.theme_system')" value="system" />
                      <el-option :label="$t('app.settings.theme_light')" value="light" />
                      <el-option :label="$t('app.settings.theme_dark')" value="dark" />
                    </el-select>
                  </div>
                </section>
                <section v-if="authStore.isLoggedIn" class="settings-group">
                  <div class="settings-group-title">{{ $t('app.settings.group_account') }}</div>
                  <button type="button" class="settings-item clickable" @click="handleShowInviteDialog">
                    <span>{{ $t('app.settings.invite_friend') }}</span><el-icon><LinkIcon /></el-icon>
                  </button>
                  <button type="button" class="settings-item clickable" @click="handleOpenRecharge">
                    <span>{{ $t('app.settings.subscribe') }}</span><el-icon><LinkIcon /></el-icon>
                  </button>
                </section>
                <section class="settings-group">
                  <div class="settings-group-title">{{ $t('app.settings.group_support') }}</div>
                  <button type="button" class="settings-item clickable" @click="handleCheckUpdate">
                    <span>{{ $t('app.settings.check_update') }}</span><span class="version-info">v{{ appVersion }}</span>
                  </button>
                  <button type="button" class="settings-item clickable" @click="handleOpenWebsite">
                    <span>{{ $t('app.settings.visit_website') }}</span><el-icon><LinkIcon /></el-icon>
                  </button>
                  <button v-if="!isTauriRuntime() && webAuthRequired" type="button" class="settings-item clickable" @click="handleWebAdminLogout">
                    <span>{{ $t('app.web_auth.logout') }}</span>
                  </button>
                </section>
              </div>
            </el-popover>

            <el-dropdown v-if="authStore.isLoggedIn" @command="handleUserAction">
              <span class="user-info">
                <el-icon><UserIcon /></el-icon>
                <span class="user-name">{{ authStore.userInfo?.username || authStore.userInfo?.email || $t('app.user.default_name') }}</span>
                <span class="user-level">{{ memberLevelLabel }}</span>
                <el-icon class="el-icon--right"><ArrowDown /></el-icon>
              </span>
              <template #dropdown>
                <el-dropdown-menu>
                  <el-dropdown-item command="logout">{{ $t('app.user.logout') }}</el-dropdown-item>
                </el-dropdown-menu>
              </template>
            </el-dropdown>
            <div v-if="isTauriRuntime()" class="window-controls no-drag">
              <button class="window-control-btn minimize-btn" type="button" @click="handleMinimizeWindow" :aria-label="$t('app.window.minimize_aria')">
                <span class="window-control-icon window-control-minimize"></span>
              </button>
              <button class="window-control-btn maximize-btn" type="button" @click="handleToggleMaximizeWindow" :aria-label="$t(isMaximized ? 'app.window.restore_aria' : 'app.window.maximize_aria')">
                <span class="window-control-icon" :class="isMaximized ? 'window-control-restore' : 'window-control-maximize'"></span>
              </button>
              <button class="window-control-btn close-btn" type="button" @click="handleCloseWindow" :aria-label="$t('app.window.close_aria')">
                <span class="window-control-icon window-control-close"></span>
              </button>
            </div>
          </div>
        </div>
      </el-header>

      <div v-if="!startupReady" class="startup-preflight">
        <div class="preflight-card fluent-card">
          <AppLogo :size="42" />
          <h2>{{ $t('app.startup.preparing') }}</h2>
          <p class="preflight-subtitle">{{ currentPreflightMessage }}</p>

          <el-progress
            class="preflight-progress"
            :percentage="startupProgress"
            :stroke-width="10"
            :status="startupError ? 'exception' : startupProgress === 100 ? 'success' : undefined"
          />

          <div v-if="startupError" class="preflight-error">
            {{ startupError }}
          </div>
          <el-button
            v-if="startupError"
            type="primary"
            :loading="startupRunning"
            @click="bootstrapApp"
          >
            {{ $t('app.startup.recheck') }}
          </el-button>
        </div>
      </div>

      <div v-else-if="authStore.isLoggedIn" class="app-workspace">
        <aside class="function-sidebar no-drag">
          <button
            type="button"
            class="function-nav-item"
            :class="{ active: activeWorkspace === 'devices' }"
            :aria-current="activeWorkspace === 'devices' ? 'page' : undefined"
            @click="selectWorkspace('devices', $event)"
          >
            <el-icon><Monitor /></el-icon>
            <span>{{ $t('app.nav.devices') }}</span>
          </button>
          <button
            type="button"
            class="function-nav-item"
            :class="{ active: activeWorkspace === 'remote' }"
            :aria-current="activeWorkspace === 'remote' ? 'page' : undefined"
            @click="selectWorkspace('remote', $event)"
          >
            <el-icon><Connection /></el-icon>
            <span>{{ $t('app.nav.remote') }}</span>
          </button>
          <button
            type="button"
            class="function-nav-item"
            :class="{ active: activeWorkspace === 'tunnels' }"
            :aria-current="activeWorkspace === 'tunnels' ? 'page' : undefined"
            @click="selectWorkspace('tunnels', $event)"
          >
            <el-icon><LinkIcon /></el-icon>
            <span>{{ $t('app.nav.tunnel') }}</span>
          </button>
        </aside>

        <el-main class="workspace-main no-drag" :class="{ 'no-workspace-motion': !workspaceMotionEnabled }">
          <Transition name="workspace-view">
            <RemoteConnectionView v-if="activeWorkspace === 'remote'" key="remote" />
            <TunnelStatusView v-else-if="activeWorkspace === 'tunnels'" key="tunnels" />
            <DevicesView v-else key="devices" />
          </Transition>
        </el-main>
      </div>

      <el-main v-else class="login-main no-drag">
        <router-view />
      </el-main>
    </el-container>

    <el-dialog v-model="inviteDialogVisible" :title="$t('app.invite.title')" width="520px">
      <div class="invite-panel">
        <div class="invite-row">
          <span class="invite-label">{{ $t('app.invite.code_label') }}</span>
          <el-input :model-value="inviteInfo?.invite_code || ''" readonly />
        </div>
        <div class="invite-row">
          <span class="invite-label">{{ $t('app.invite.link_label') }}</span>
          <el-input :model-value="inviteInfo?.invite_link || ''" readonly />
        </div>
        <p class="invite-tip">{{ $t('app.invite.tip') }}</p>
      </div>
      <template #footer>
        <el-button @click="inviteDialogVisible = false">{{ $t('common.close') }}</el-button>
        <el-button type="primary" @click="copyInviteCode">{{ $t('app.invite.copy_code') }}</el-button>
        <el-button type="primary" plain @click="copyInviteLink">{{ $t('app.invite.copy_link') }}</el-button>
      </template>
    </el-dialog>

    <div
      v-if="textContextMenu.visible"
      class="text-context-menu no-drag"
      :style="{ left: `${textContextMenu.x}px`, top: `${textContextMenu.y}px` }"
      role="menu"
      @mousedown.stop
    >
      <button type="button" role="menuitem" @click="copyContextSelection">{{ $t('common.copy') }}</button>
    </div>
    </div>
  </el-config-provider>
</template>

<script setup lang="ts">
import { computed, defineAsyncComponent, nextTick, onMounted, onUnmounted, ref, watch } from 'vue'
import { useI18n } from 'vue-i18n'
const { t } = useI18n()
import {
  closeWindow,
  getWebAuthStatus,
  invoke,
  isTauriRuntime,
  listen,
  minimizeWindow,
  toggleMaximizeWindow,
  isWindowMaximized,
  openExternal,
  logoutWebAdmin,
  unlockWebAdmin,
} from './runtime/bridge'
import { ElMessage } from 'element-plus/es/components/message/index.mjs'
import { ElMessageBox } from 'element-plus/es/components/message-box/index.mjs'
import { ElNotification } from 'element-plus/es/components/notification/index.mjs'
import elEn from 'element-plus/es/locale/lang/en'
import elZhCn from 'element-plus/es/locale/lang/zh-cn'
import {
  Setting,
  Link as LinkIcon,
  User as UserIcon,
  ArrowDown,
  Connection,
  Monitor,
} from '@element-plus/icons-vue'

import { useRouter } from 'vue-router'
import { useAuthStore } from './stores/auth'
import { useDeviceStore } from './stores/device'
import AppLogo from './components/AppLogo.vue'
import { useLocale } from './composables/useLocale'
import { SUPPORTED_LOCALES } from './i18n'

// 工作区只在登录后按需载入，避免把三个功能页都放进启动包。
const DevicesView = defineAsyncComponent(() => import('./views/Devices.vue'))
const RemoteConnectionView = defineAsyncComponent(() => import('./views/RemoteConnection.vue'))
const TunnelStatusView = defineAsyncComponent(() => import('./views/TunnelStatus.vue'))

// 语言切换：localeModel 双向绑定下拉；epLocale 控制 Element Plus 组件文案
const { locale, setLocale } = useLocale()
const localeModel = computed({
  get: () => locale.value,
  set: (val) => { void setLocale(val) },
})
const epLocale = computed(() => (locale.value === 'en' ? elEn : elZhCn))

type ThemeMode = 'system' | 'light' | 'dark'
const storedTheme = localStorage.getItem('p2premote-theme')
const themeMode = ref<ThemeMode>(storedTheme === 'light' || storedTheme === 'dark' ? storedTheme : 'system')

function applyTheme(mode: ThemeMode) {
  if (mode === 'system') document.documentElement.removeAttribute('data-theme')
  else document.documentElement.dataset.theme = mode
  localStorage.setItem('p2premote-theme', mode)
}

watch(themeMode, applyTheme, { immediate: true })

const webAuthChecked = ref(isTauriRuntime())
const webAuthRequired = ref(false)
const webAuthenticated = ref(isTauriRuntime())
const webSecurityCode = ref('')
const webAuthLoading = ref(false)
const webAuthError = ref('')
let appBootstrapped = false
let wsListenersInitialized = false
const webAuthGateVisible = computed(
  () => !isTauriRuntime() && (!webAuthChecked.value || (webAuthRequired.value && !webAuthenticated.value)),
)

async function refreshWebAuthStatus() {
  if (isTauriRuntime()) return
  try {
    const status = await getWebAuthStatus()
    webAuthRequired.value = status.security_code_required
    webAuthenticated.value = status.authenticated
    webAuthError.value = ''
  } catch (error) {
    webAuthRequired.value = true
    webAuthenticated.value = false
    console.warn('[App] Web authentication status unavailable:', error)
    webAuthError.value = t('app.web_auth.service_unavailable')
  } finally {
    webAuthChecked.value = true
  }
}

async function handleWebUnlock() {
  if (webAuthLoading.value) return
  webAuthLoading.value = true
  webAuthError.value = ''
  try {
    await unlockWebAdmin(webSecurityCode.value)
    webSecurityCode.value = ''
    await refreshWebAuthStatus()
    if (webAuthenticated.value) {
      if (appBootstrapped) startupReady.value = true
      else await bootstrapApp()
    }
  } catch (error) {
    const code = (error as Error & { code?: string }).code
    webAuthError.value = code === 'temporarily_locked'
      ? t('app.web_auth.temporarily_locked')
      : t('app.web_auth.invalid_code')
  } finally {
    webAuthLoading.value = false
  }
}

async function handleWebAdminLogout() {
  await logoutWebAdmin()
}

function requireWebAuthentication() {
  if (isTauriRuntime()) return
  webAuthRequired.value = true
  webAuthenticated.value = false
  startupReady.value = false
}

const textContextMenu = ref({
  visible: false,
  x: 0,
  y: 0,
  text: '',
})

function selectedTextAt(target: EventTarget | null): string {
  if (target instanceof HTMLInputElement || target instanceof HTMLTextAreaElement) {
    const start = target.selectionStart
    const end = target.selectionEnd
    if (start !== null && end !== null && end > start) {
      return target.value.slice(start, end)
    }
  }
  return window.getSelection()?.toString() || ''
}

function closeTextContextMenu() {
  textContextMenu.value.visible = false
}

function handleAppContextMenu(event: MouseEvent) {
  event.preventDefault()
  closeTextContextMenu()

  const text = selectedTextAt(event.target)
  if (!text) return

  textContextMenu.value = {
    visible: true,
    x: Math.max(8, Math.min(event.clientX, window.innerWidth - 104)),
    y: Math.max(8, Math.min(event.clientY, window.innerHeight - 48)),
    text,
  }
}

async function copyContextSelection() {
  const text = textContextMenu.value.text
  closeTextContextMenu()
  if (!text) return
  try {
    await navigator.clipboard.writeText(text)
  } catch {
    ElMessage.error(t('common.copy_failed'))
  }
}

interface WsEventPayload {
  msg_type: string
  data: Record<string, any>
}

interface UpdateCheckResponse {
  mode: 'none' | 'optional' | 'force'
  has_update: boolean
  force_update: boolean
  current: string
  latest: string
  min_supported: string
  download_url: string
  release_notes: string
  error?: string | null
}

interface InviteInfo {
  invite_code: string
  invite_link: string
}

interface BackgroundServiceStatus {
  service: {
    installed: boolean
    running: boolean
    enabled: boolean
    raw_state: string
  }
  runtime?: {
    logged_in: boolean
    device_id?: number | null
    device_uuid?: string | null
    ws_connected: boolean
    last_heartbeat_at?: number | null
    last_error?: string | null
    public_ip?: string | null
    wgvpn_sessions?: Array<{
      peer_device_id: number
      is_active: boolean
      virtual_ip: string
      peer_virtual_ip: string
      tunnel_name: string
    }>
    tunnel_lifecycles?: Array<{
      peer_device_id: number
      role: 'active' | 'passive'
      source_username?: string
      peer_device_name?: string
      peer_device_alias?: string
    }>
    active_tunnel_jobs?: ActiveTunnelJobStatus[]
  } | null
  machine_logged_in: boolean
  config_path: string
  log_dir: string
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
    reused: boolean
    local_port: number
    rdp_address: string
    message: string
    warning?: string | null
  } | null
}

interface ActiveTunnelJobNotification {
  key: string
  title: string
  message: string
  type: 'success' | 'error' | 'info'
}

interface RequiredClientFile {
  name: string
  path: string
}

const router = useRouter()
const authStore = useAuthStore()
const deviceStore = useDeviceStore()

const autoStart = ref(false)
const backgroundServiceStatus = ref<BackgroundServiceStatus | null>(null)
const startupReady = ref(false)
const startupRunning = ref(false)
const startupProgress = ref(0)
const startupError = ref('')
const currentPreflightMessage = ref(t('app.startup_status.launching'))
const appVersion = ref('1.0.0')
const forceUpdateVisible = ref(false)
const optionalUpdatePrompted = ref(false)
const inviteDialogVisible = ref(false)
const inviteInfo = ref<InviteInfo | null>(null)
type Workspace = 'remote' | 'devices' | 'tunnels'
const savedWorkspace = localStorage.getItem('p2premote-workspace')
const activeWorkspace = ref<Workspace>(
  savedWorkspace === 'remote' || savedWorkspace === 'tunnels' ? savedWorkspace : 'devices',
)
watch(activeWorkspace, value => localStorage.setItem('p2premote-workspace', value))
const workspaceMotionEnabled = ref(true)

function selectWorkspace(workspace: Workspace, event: MouseEvent) {
  if (workspace === activeWorkspace.value) return
  workspaceMotionEnabled.value = event.detail !== 0
  activeWorkspace.value = workspace
  if (!workspaceMotionEnabled.value) {
    window.requestAnimationFrame(() => { workspaceMotionEnabled.value = true })
  }
}
const isMaximized = ref(false)
const pendingActiveTunnelJobNotifications = ref<ActiveTunnelJobNotification[]>([])
const handledActiveTunnelJobKeys = new Map<number, Set<string>>()
const knownPassiveWgvpnPeers = new Set<number>()
const updateInfo = ref({
  mode: 'none' as 'none' | 'optional' | 'force',
  current: '1.0.0',
  latest: '',
  minSupported: '',
  downloadUrl: '',
  releaseNotes: ''
})
let wsReconnectTimer: ReturnType<typeof window.setInterval> | null = null
const titlebarDragEnabled = isTauriRuntime() && !/Linux/i.test(navigator.userAgent)
const titlebarDragAttributes = titlebarDragEnabled
  ? { 'data-tauri-drag-region': '' }
  : {}
const memberLevelLabel = computed(() => {
  const level = authStore.userInfo?.member_level
  if (level === 'pro') {
    return 'pro'
  }
  if (level === 'free') {
    return 'free'
  }
  return level || 'free'
})

function normalizeError(error: unknown): string {
  if (typeof error === 'string') {
    return error
  }
  if (error instanceof Error) {
    return error.message
  }
  return String(error || t('common.unknown_error'))
}

function isServiceReady(status: BackgroundServiceStatus | null): boolean {
  return Boolean(status?.service?.running || status?.runtime)
}

async function withTimeout<T>(promise: Promise<T>, ms: number, message: string): Promise<T> {
  let timer: ReturnType<typeof window.setTimeout> | null = null
  const timeout = new Promise<never>((_, reject) => {
    timer = window.setTimeout(() => reject(new Error(message)), ms)
  })
  try {
    return await Promise.race([promise, timeout])
  } finally {
    if (timer !== null) {
      window.clearTimeout(timer)
    }
  }
}

function waitForUiPaint(): Promise<void> {
  return new Promise(resolve => {
    window.setTimeout(resolve, 120)
  })
}

async function setStartupStep(message: string, progress?: number) {
  currentPreflightMessage.value = message
  if (progress !== undefined) {
    startupProgress.value = progress
  }
  await nextTick()
  await waitForUiPaint()
}

async function runStartupPreflight(): Promise<boolean> {
  startupReady.value = false
  startupRunning.value = true
  startupProgress.value = 0
  startupError.value = ''
  await setStartupStep(t('app.startup_status.verifying_files'), 0)

  try {
    const files = await invoke<RequiredClientFile[]>('check_required_client_files')
    void files
    await setStartupStep(t('app.startup_status.checking_service'), 25)

    const status = await withTimeout(
      invoke<BackgroundServiceStatus>('ensure_background_service_session'),
      20_000,
      t('app.startup_status.service_timeout')
    )
    backgroundServiceStatus.value = status
    if (!isServiceReady(status)) {
      throw new Error(t('app.startup_errors.service_not_ready'))
    }
    await setStartupStep(t('app.startup_status.checking_network'), 50)

    const refreshed = await invoke<BackgroundServiceStatus>('refresh_service_network_info')
    backgroundServiceStatus.value = refreshed
    if (!isServiceReady(refreshed)) {
      throw new Error(t('app.startup_errors.service_not_ready_network'))
    }
    await setStartupStep(t('app.startup_status.checking_update'), 75)

    await checkForUpdates()

    await setStartupStep(t('app.startup_status.ready'), 90)
    return true
  } catch (error) {
    const message = normalizeError(error)
    startupError.value = message
    await setStartupStep(t('app.startup_status.failed'))
    return false
  } finally {
    startupRunning.value = false
  }
}

function clearWsReconnectTimer() {
  if (wsReconnectTimer !== null) {
    window.clearInterval(wsReconnectTimer)
    wsReconnectTimer = null
  }
}

function releaseUiRealtimeOwnership() {
  clearWsReconnectTimer()
}

async function ensureServiceRealtimeOwnership() {
  if (!authStore.isLoggedIn) {
    return
  }
  // 确保持久 IPC 连接存在（ensure_background_service_session 已在 onMounted 中调用过）
  try {
    await invoke('listen_service_events')
  } catch (_e) {
    // service 可能还没准备好，忽略
  }
  releaseUiRealtimeOwnership()
}

/// 收到 service-status-changed 推送时，直接更新本地状态，避免再调 get_service_status
function updateServiceStatusFromEvent(runtime: any) {
  if (backgroundServiceStatus.value) {
    backgroundServiceStatus.value = {
      ...backgroundServiceStatus.value,
      runtime,
      service: {
        ...backgroundServiceStatus.value.service,
        running: true,
      },
    }
  }
  void handleActiveTunnelJobStatuses(runtime?.active_tunnel_jobs)
  void handlePassiveWgvpnSessions(runtime)
}

function primePassiveWgvpnPeers(runtime: BackgroundServiceStatus['runtime'] | null | undefined) {
  knownPassiveWgvpnPeers.clear()
  for (const session of runtime?.wgvpn_sessions || []) {
    if (!session.is_active) knownPassiveWgvpnPeers.add(session.peer_device_id)
  }
}

function primeActiveTunnelJobs(runtime: BackgroundServiceStatus['runtime'] | null | undefined) {
  handledActiveTunnelJobKeys.clear()
  for (const job of runtime?.active_tunnel_jobs || []) {
    if (job.state !== 'succeeded' && job.state !== 'failed' && job.state !== 'cancelled') continue
    handledActiveTunnelJobKeys.set(
      job.target_device_id,
      new Set([`${job.target_device_id}:${job.state}:${job.updated_at}`]),
    )
  }
}

async function handlePassiveWgvpnSessions(
  runtime?: BackgroundServiceStatus['runtime'] | null,
) {
  const sessions = runtime?.wgvpn_sessions
  if (!Array.isArray(sessions)) return

  const passiveSessions = sessions.filter(session => !session.is_active)
  const currentPeers = new Set(passiveSessions.map(session => session.peer_device_id))
  const connected = passiveSessions.filter(session => !knownPassiveWgvpnPeers.has(session.peer_device_id))

  knownPassiveWgvpnPeers.clear()
  for (const peerId of currentPeers) knownPassiveWgvpnPeers.add(peerId)

  for (const session of connected) {
    const device = deviceStore.devices.find(item => item.device_id === session.peer_device_id)
    const lifecycle = runtime?.tunnel_lifecycles?.find(item => (
      item.peer_device_id === session.peer_device_id && item.role === 'passive'
    ))
    const deviceName = lifecycle?.peer_device_alias?.trim()
      || lifecycle?.peer_device_name?.trim()
      || device?.device_alias
      || device?.device_name
      || t('app.notification.peer_device_id', { deviceId: session.peer_device_id })
    const username = lifecycle?.source_username?.trim()
    const message = username
      ? t('app.notification.tunnel_connected_by_user', { username, deviceName })
      : t('app.notification.tunnel_connected_unknown_user', { deviceName })

    await invoke('show_system_notification', {
      title: t('app.notification.tunnel_connected_title'),
      body: message,
    }).catch(error => {
      console.warn('[App] passive tunnel system notification failed:', error)
    })

    if (isAppForeground()) {
      ElNotification({
        title: t('app.notification.tunnel_connected_title'),
        message,
        type: 'success',
        duration: 0,
        position: 'bottom-right',
        showClose: true,
      })
    } else {
      await invoke('flash_main_window').catch(error => {
        console.warn('[App] flash main window failed:', error)
      })
    }
  }
}

function isAppForeground(): boolean {
  return document.visibilityState === 'visible' && document.hasFocus()
}

async function showActiveTunnelJobNotification(notification: ActiveTunnelJobNotification) {
  if (notification.type === 'info') {
    ElMessage.info(notification.message)
    return
  }

  if (notification.type === 'success') {
    await ElMessageBox.alert(notification.message, notification.title, {
      type: 'success',
      confirmButtonText: t('common.got_it'),
    }).catch(() => {})
    return
  }

  await ElMessageBox.alert(notification.message, notification.title, {
    type: 'error',
    confirmButtonText: t('common.got_it'),
  }).catch(() => {})
}

async function notifyActiveTunnelJobResult(notification: ActiveTunnelJobNotification) {
  await invoke('show_system_notification', {
    title: notification.title,
    body: notification.message,
  }).catch((e) => {
    console.warn('[App] show system notification failed:', e)
  })

  if (isAppForeground()) {
    await showActiveTunnelJobNotification(notification)
    return
  }

  pendingActiveTunnelJobNotifications.value.push(notification)
  await invoke('flash_main_window').catch((e) => {
    console.warn('[App] flash main window failed:', e)
  })
}

async function flushPendingActiveTunnelJobNotifications() {
  if (!isAppForeground() || pendingActiveTunnelJobNotifications.value.length === 0) {
    return
  }

  const pending = [...pendingActiveTunnelJobNotifications.value]
  pendingActiveTunnelJobNotifications.value = []
  for (const notification of pending) {
    await showActiveTunnelJobNotification(notification)
  }
}

async function handleActiveTunnelJobStatuses(jobs?: ActiveTunnelJobStatus[]) {
  if (!Array.isArray(jobs)) {
    return
  }

  for (const job of jobs) {
    window.dispatchEvent(new CustomEvent('p2p-active-tunnel-job-updated', { detail: job }))
    if (job.state !== 'succeeded' && job.state !== 'failed' && job.state !== 'cancelled') {
      continue
    }

    const key = `${job.target_device_id}:${job.state}:${job.updated_at}`
    let deviceKeys = handledActiveTunnelJobKeys.get(job.target_device_id)
    if (!deviceKeys) {
      deviceKeys = new Set<string>()
      handledActiveTunnelJobKeys.set(job.target_device_id, deviceKeys)
    }
    if (deviceKeys.has(key)) {
      continue
    }
    deviceKeys.add(key)
    if (deviceKeys.size > 20) {
      deviceKeys.clear()
      deviceKeys.add(key)
    }

    if (job.state === 'succeeded') {
      // 通知文案只显示对端虚拟 IP，端口属于后续 RDP 连接细节，由远程协助页单独处理。
      const address = (job.result?.rdp_address || '').replace(/:\d+$/, '')
      window.dispatchEvent(new CustomEvent('p2p-active-tunnel-job-completed', { detail: job }))
      await notifyActiveTunnelJobResult({
        key,
        title: t('app.notification.tunnel_built_title'),
        message: [
          address ? t('app.notification.tunnel_built_with_address', { address }) : t('app.notification.tunnel_built_plain'),
          job.result?.warning || '',
        ].filter(Boolean).join('\n'),
        type: 'success',
      })
    } else if (job.state === 'cancelled') {
      await notifyActiveTunnelJobResult({
        key,
        title: t('app.notification.tunnel_cancelled_title'),
        message: job.message || t('app.notification.tunnel_cancelled_body'),
        type: 'info',
      })
    } else {
      await notifyActiveTunnelJobResult({
        key,
        title: t('app.notification.tunnel_failed_title'),
        message: job.message || t('app.notification.tunnel_failed_body'),
        type: 'error',
      })
    }
  }
}

const handleActiveTunnelJobForeground = () => {
  void flushPendingActiveTunnelJobNotifications()
}

async function handleAutoStartChange(val: boolean) {
  if (val) {
    // 开机自启动仅依赖两个登录偏好开关；登录 token 可能尚未生成，
    // 因此不能将其作为开启自启动的前置条件。
    let rememberMe = false
    let autoLogin = false
    try {
      const settings = await invoke<{ remember_me: boolean; auto_login: boolean }>('get_settings')
      rememberMe = settings.remember_me
      autoLogin = settings.auto_login
    } catch (e) {
      console.warn('[App] 开启自启前查询登录偏好失败:', e)
      autoStart.value = false
      ElMessage.error(t('app.auto_login.settings_query_failed', { error: normalizeError(e) }))
      return
    }

    if (!rememberMe || !autoLogin) {
      try {
        await ElMessageBox.confirm(
          t('app.auto_login.confirm_body'),
          t('app.auto_login.confirm_title'),
          {
            confirmButtonText: t('app.auto_login.confirm_ok'),
            cancelButtonText: t('common.cancel'),
            type: 'warning'
          }
        )
      } catch {
        autoStart.value = false
        return
      }
    }
  }

  try {
    await invoke('set_auto_start', { enabled: val })
    ElMessage.success(val ? t('app.actions.auto_start_enabled') : t('app.actions.auto_start_disabled'))
  } catch (e) {
    autoStart.value = !val
    ElMessage.error(t('app.actions.set_failed', { error: e }))
  }
}

async function handleCheckUpdate() {
  await checkForUpdates(true)
}

async function openUpdatePage() {
  if (!updateInfo.value.downloadUrl) {
    ElMessage.warning(t('app.actions.no_download_url'))
    return
  }

  try {
    await openExternal(updateInfo.value.downloadUrl)
  } catch (e) {
    ElMessage.error(t('app.actions.open_update_failed', { error: e }))
  }
}

async function handleExitApp() {
  await invoke('exit_application')
}

async function handleMinimizeWindow() {
  try {
    await minimizeWindow()
  } catch (e) {
    ElMessage.error(t('app.actions.minimize_failed', { error: e }))
  }
}

async function handleToggleMaximizeWindow() {
  try {
    isMaximized.value = await toggleMaximizeWindow()
  } catch {
    ElMessage.error(t('app.actions.maximize_failed'))
  }
}

async function handleTitlebarDoubleClick(event: MouseEvent) {
  if ((event.target as HTMLElement).closest('.no-drag')) return
  await handleToggleMaximizeWindow()
}

async function syncMaximizedWindowState() {
  isMaximized.value = await isWindowMaximized().catch(() => false)
}

let windowStateSyncTimer: ReturnType<typeof window.setTimeout> | null = null
const handleWindowResize = () => {
  if (windowStateSyncTimer !== null) window.clearTimeout(windowStateSyncTimer)
  windowStateSyncTimer = window.setTimeout(() => {
    windowStateSyncTimer = null
    void syncMaximizedWindowState()
  }, 80)
}

async function handleCloseWindow() {
  try {
    await closeWindow()
  } catch (e) {
    ElMessage.error(t('app.actions.close_failed', { error: e }))
  }
}

async function handleOpenWebsite() {
  try {
    await openExternal('https://www.p2premote.top')
  } catch (e) {
    ElMessage.error(t('app.actions.open_website_failed', { error: e }))
  }
}

async function handleOpenRecharge() {
  try {
    await openExternal('https://www.p2premote.top/console/recharge')
  } catch (e) {
    ElMessage.error(t('app.actions.open_subscribe_failed', { error: e }))
  }
}

async function loadInviteInfo() {
  inviteInfo.value = await invoke<InviteInfo | null>('get_invite_info')
}

async function handleShowInviteDialog() {
  try {
    await loadInviteInfo()
    inviteDialogVisible.value = true
  } catch (e) {
    ElMessage.error(t('app.actions.get_invite_failed', { error: e }))
  }
}

async function copyText(value: string, successMessage: string) {
  if (!value) {
    ElMessage.warning(t('common.nothing_to_copy'))
    return
  }
  await navigator.clipboard.writeText(value)
  ElMessage.success(successMessage)
}

async function copyInviteCode() {
  try {
    await copyText(inviteInfo.value?.invite_code || '', t('app.actions.invite_code_copied'))
  } catch (e) {
    ElMessage.error(t('app.actions.copy_invite_code_failed', { error: e }))
  }
}

async function copyInviteLink() {
  try {
    await copyText(inviteInfo.value?.invite_link || '', t('app.actions.invite_link_copied'))
  } catch (e) {
    ElMessage.error(t('app.actions.copy_invite_link_failed', { error: e }))
  }
}

async function handleUserAction(command: string) {
  if (forceUpdateVisible.value) {
    return
  }
  if (command !== 'logout') {
    return
  }

  try {
    await ElMessageBox.confirm(t('app.actions.logout_confirm_body'), t('app.actions.logout_confirm_title'), {
      confirmButtonText: t('common.confirm'),
      cancelButtonText: t('common.cancel'),
      type: 'warning'
    })
  } catch {
    return
  }

  await authStore.logout()
  router.push('/login')
}

async function refreshUserInfo() {
  await authStore.fetchUserInfo()
}

async function restoreServiceSession(token: string, source: string) {
  authStore.setToken(token)
  await refreshUserInfo()
  await ensureServiceRealtimeOwnership()
  await deviceStore.fetchDevices({ force: true })
  console.log(`[App] 已从 ${source} 恢复 service 会话`)
}

async function checkForUpdates(manual = false) {
  try {
    const result = await invoke<UpdateCheckResponse>('check_update')
    appVersion.value = result.current || appVersion.value
    updateInfo.value = {
      mode: result.mode || 'none',
      current: result.current || appVersion.value,
      latest: result.latest || '',
      minSupported: result.min_supported || '',
      downloadUrl: result.download_url || '',
      releaseNotes: result.release_notes || ''
    }

    if (result.mode === 'force') {
      forceUpdateVisible.value = true
      return
    }

    forceUpdateVisible.value = false

    if (result.mode === 'optional') {
      if (manual) {
        await ElMessageBox.confirm(
          t('app.actions.update_available_force', { current: result.current, latest: result.latest }),
          t('app.actions.update_available_title'),
          {
            confirmButtonText: t('app.actions.update_now'),
            cancelButtonText: t('app.actions.later'),
            type: 'info'
          }
        ).then(openUpdatePage).catch(() => {})
        return
      }

      if (!optionalUpdatePrompted.value) {
        optionalUpdatePrompted.value = true
        await ElMessageBox.confirm(
          t('app.actions.update_available_optional', { current: result.current, latest: result.latest }),
          t('app.actions.update_available_title'),
          {
            confirmButtonText: t('app.actions.update_now'),
            cancelButtonText: t('app.actions.later'),
            type: 'info'
          }
        ).then(openUpdatePage).catch(() => {})
      }
      return
    }

    if (manual) {
      ElMessage.success(t('app.actions.already_latest'))
    }
  } catch (e) {
    if (manual) {
      ElMessage.error(t('app.actions.check_update_failed', { error: e }))
    }
  }
}

async function setupWsListeners() {
  if (wsListenersInitialized) return
  wsListenersInitialized = true
  await listen<WsEventPayload>('ws-connected', async () => {
    console.log('[App] [WS] connected')
    clearWsReconnectTimer()
    // 建连前拉取的列表可能把本机标记为离线；连接事件必须绕过普通刷新节流。
    await deviceStore.fetchDevices({ force: true })
  })

  await listen<WsEventPayload>('ws-disconnected', () => {
    console.log('[App] [WS] UI websocket disconnected, realtime is owned by background service')
    clearWsReconnectTimer()
  })

  await listen<WsEventPayload>('ws-p2p-start', async () => {
    console.log('[App] [WS] ignoring p2p-start because background service owns realtime')
  })

  deviceStore.onDeviceOnline((deviceId, deviceName) => {
    console.log('[App] 设备上线:', deviceId, deviceName)
    ElMessage.success(t('app.notification.device_online', { deviceName }))
  })

  deviceStore.onDeviceOffline((deviceId) => {
    const device = deviceStore.devices.find(d => d.device_id === deviceId)
    ElMessage.info(t('app.notification.device_offline', { deviceName: device?.device_name || String(deviceId) }))
  })
}

async function bootstrapApp() {
  try {
    const preflightOk = await runStartupPreflight()
    if (!preflightOk) {
      return
    }
    // 初次启动也要把 localStorage/系统探测得到的语言同步给 service。
    // 否则英文系统在用户手动切换语言之前，后端状态消息仍会使用中文。
    await setLocale(locale.value)
    // The initial service snapshot is baseline state, not a newly completed
    // event. Historical terminal jobs must not produce startup notifications.
    primeActiveTunnelJobs(backgroundServiceStatus.value?.runtime)
    primePassiveWgvpnPeers(backgroundServiceStatus.value?.runtime)
    if (forceUpdateVisible.value) {
      await setupWsListeners()
      return
    }

    // 加载设置（开机自启状态、日志级别、版本号）
    await setStartupStep(t('app.startup_status.loading_settings'), 92)
    try {
      const settings = await invoke<{ auto_start: boolean; version: string }>('get_settings')
      autoStart.value = settings.auto_start
      appVersion.value = settings.version
    } catch (e) {
      console.warn('[App] 加载设置失败:', e)
    }

    // 建立到 service 的持久 IPC 连接，监听推送事件
    await setStartupStep(t('app.startup_status.connecting_events'), 94)
    try {
      await invoke('listen_service_events')
    } catch (e) {
      console.warn('[App] listen service events failed:', e)
    }
    let identityRebuildNotified = false
    await listen<{
      logged_in: boolean
      ws_connected: boolean
      device_identity_rebuilt?: boolean
      device_identity_message?: string | null
    }>('service-status-changed', (event) => {
      console.log('[App] service status changed:', event.payload)
      // StatusChanged 推送已携带完整状态，直接更新，无需再调 get_service_status
      if (event.payload) {
        updateServiceStatusFromEvent(event.payload)
        if (event.payload.device_identity_rebuilt && !identityRebuildNotified) {
          identityRebuildNotified = true
          ElNotification({
            title: t('app.notification.device_identity_updated'),
            message: event.payload.device_identity_message || t('app.notification.device_clone_default'),
            type: 'warning',
            duration: 8000,
          })
          void invoke('acknowledge_device_identity_notification').catch(error => {
            console.warn('[App] acknowledge device identity notification failed:', error)
          })
        }
      }
    })

    // IPC 持久连接断开时持续退避重连（1s → 2s → 4s → ... → 30s）
    // 仅在已登录且未强制更新时重连，登出/退出时停止
    await listen('service-connection-lost', async () => {
      let delay = 1000
      const maxDelay = 30_000
      let attempt = 0
      while (authStore.isLoggedIn && !forceUpdateVisible.value) {
        attempt++
        console.log(`[App] service IPC connection lost, reconnecting in ${delay / 1000}s (attempt ${attempt})...`)
        await new Promise(resolve => setTimeout(resolve, delay))
        if (!authStore.isLoggedIn) break
        try {
          await invoke('listen_service_events')
          console.log('[App] service IPC reconnected')
          return
        } catch (e) {
          console.warn(`[App] service IPC reconnect attempt ${attempt} failed:`, e)
          delay = Math.min(delay * 2, maxDelay)
        }
      }
      console.log('[App] service IPC reconnect stopped (logged out or app exiting)')
    })

    releaseUiRealtimeOwnership()

    await setStartupStep(t('app.startup_status.checking_login'), 96)
    const token = await withTimeout(
      invoke<string>('try_auto_login'),
      10_000,
      t('app.startup_errors.login_check_timeout')
    )
    if (token) {
      try {
        await restoreServiceSession(token, '自动登录')
      } catch (e) {
        console.warn('[App] 自动登录后刷新用户信息失败，token 可能已失效:', e)
        await authStore.logout()
        router.push('/login')
      }
    } else {
      const serviceLoggedIn = await invoke<boolean>('is_logged_in').catch(() => {
        return Boolean(backgroundServiceStatus.value?.runtime?.logged_in)
      })
      // GUI 关闭窗口或进程重开时允许接管仍在运行的 service 会话。
      // 保存但未启用自动登录的 refresh token 只能在登录页由用户手动恢复。
      if (serviceLoggedIn) {
        try {
          await restoreServiceSession('__service_session__', '后台 service 已登录状态')
        } catch (e) {
          console.warn('[App] 后台 service 已登录，但恢复前端会话失败:', e)
          await authStore.logout()
          router.push('/login')
        }
      }
    }
    await setStartupStep(t('app.startup_status.startup_complete'), 100)
    appBootstrapped = true
    startupReady.value = true
  } catch (e) {
    console.error('[App] 启动检查失败:', e)
    startupError.value = normalizeError(e)
    await setStartupStep(t('app.startup_status.failed'))
  }

  await setupWsListeners()
}

onMounted(async () => {
  await syncMaximizedWindowState()
  window.addEventListener('resize', handleWindowResize)
  window.addEventListener('focus', handleActiveTunnelJobForeground)
  document.addEventListener('visibilitychange', handleActiveTunnelJobForeground)
  document.addEventListener('contextmenu', handleAppContextMenu)
  document.addEventListener('mousedown', closeTextContextMenu)
  window.addEventListener('blur', closeTextContextMenu)
  window.addEventListener('resize', closeTextContextMenu)
  window.addEventListener('scroll', closeTextContextMenu, true)
  window.addEventListener('p2premote-web-auth-required', requireWebAuthentication)

  await refreshWebAuthStatus()
  if (!webAuthGateVisible.value) await bootstrapApp()
})

onUnmounted(() => {
  window.removeEventListener('resize', handleWindowResize)
  if (windowStateSyncTimer !== null) window.clearTimeout(windowStateSyncTimer)
  window.removeEventListener('focus', handleActiveTunnelJobForeground)
  document.removeEventListener('visibilitychange', handleActiveTunnelJobForeground)
  document.removeEventListener('contextmenu', handleAppContextMenu)
  window.removeEventListener('p2premote-web-auth-required', requireWebAuthentication)
  document.removeEventListener('mousedown', closeTextContextMenu)
  window.removeEventListener('blur', closeTextContextMenu)
  window.removeEventListener('resize', closeTextContextMenu)
  window.removeEventListener('scroll', closeTextContextMenu, true)
})

// 登录页显式登录后只同步配置到已就绪的 service，不再重复做完整 service 检查。
watch(
  () => authStore.isLoggedIn,
  async (loggedIn) => {
    if (!loggedIn) {
      handledActiveTunnelJobKeys.clear()
      knownPassiveWgvpnPeers.clear()
      pendingActiveTunnelJobNotifications.value = []
      return
    }
    releaseUiRealtimeOwnership()
  }
)
</script>

<style>
/* 全局基础样式（html/body/#app 背景、字体、滚动条、::selection）
   已迁移到 src/assets/styles/fluent-theme.css，此处保留覆盖层专用样式 */

.app-container {
  width: 100%;
  height: 100vh;
  position: relative;
}

.force-update-overlay {
  position: fixed;
  inset: 0;
  z-index: 5000;
  display: flex;
  align-items: center;
  justify-content: center;
  padding: 24px;
  background: var(--fluent-bg);
}

.force-update-card {
  width: min(560px, 100%);
  padding: 32px;
  border-radius: var(--fluent-radius-lg);
  background: var(--fluent-layer);
  border: 1px solid var(--fluent-stroke);
  box-shadow: var(--fluent-shadow-dialog);
}

.force-update-card h2 {
  margin-bottom: 16px;
  color: var(--fluent-text);
}

.force-update-text {
  margin-bottom: 12px;
  line-height: 1.7;
  color: var(--fluent-text-secondary);
}

.force-update-notes {
  margin-bottom: 24px;
  padding: 12px 14px;
  border-radius: var(--fluent-radius-md);
  background: var(--fluent-warning-bg);
  border: 1px solid var(--fluent-warning-stroke);
  color: var(--fluent-warning);
  line-height: 1.6;
}

.force-update-actions {
  display: flex;
  gap: 12px;
  justify-content: flex-end;
}
</style>

<style scoped>
.web-auth-shell {
  min-height: 100vh;
  display: grid;
  place-items: center;
  padding: 24px;
  background: var(--fluent-bg);
}

.web-auth-card {
  width: min(420px, 100%);
  display: flex;
  flex-direction: column;
  align-items: center;
  gap: 18px;
  padding: 36px;
}

.web-auth-card h1,
.web-auth-card p {
  margin: 0;
}

.web-auth-card .el-button,
.web-auth-card .el-input {
  width: 100%;
}

.web-auth-cert-note {
  color: var(--fluent-text-tertiary);
  font-size: 12px;
  line-height: 1.5;
  text-align: center;
}

.text-context-menu {
  position: fixed;
  z-index: 6000;
  min-width: 96px;
  padding: 5px;
  border: 1px solid var(--fluent-stroke);
  border-radius: var(--fluent-radius-md);
  background: var(--fluent-layer);
  box-shadow: var(--fluent-shadow-flyout);
}

.text-context-menu button {
  width: 100%;
  height: 32px;
  padding: 0 12px;
  border: 0;
  border-radius: var(--fluent-radius-sm);
  background: transparent;
  color: var(--fluent-text);
  cursor: pointer;
  font-size: 14px;
  text-align: left;
  transition: background-color var(--fluent-duration) var(--fluent-easing);
}

.text-context-menu button:hover {
  background: var(--fluent-layer-hover);
  color: var(--fluent-accent);
}

/* ===== 标题栏：实色 ===== */
.app-titlebar {
  height: 48px;
  padding: 0;
  border-bottom: 1px solid var(--fluent-stroke);
  background: var(--fluent-layer);
}

.drag-titlebar-enabled,
.drag-titlebar-enabled .header-content,
.drag-titlebar-enabled .header-left,
.drag-titlebar-enabled .app-title {
  -webkit-app-region: drag;
  cursor: move;
  user-select: none;
}

.dashboard {
  height: 100vh;
  display: flex;
  flex-direction: column;
}

/* ===== 启动检测卡片 ===== */
.startup-preflight {
  flex: 1;
  min-height: 0;
  display: flex;
  align-items: center;
  justify-content: center;
  padding: 32px;
  background: var(--fluent-bg);
}

/* 卡片视觉由 .fluent-card 工具类提供，此处仅保留尺寸 */
.preflight-card {
  width: min(560px, 100%);
  padding: 34px 38px;
  text-align: center;
}

.preflight-card h2 {
  margin-top: 16px;
  font-size: 24px;
  color: var(--fluent-text);
}

.preflight-subtitle {
  margin-top: 8px;
  color: var(--fluent-text-secondary);
  font-size: 14px;
}

.preflight-progress {
  margin: 28px 0 18px;
}

.preflight-error {
  margin: 18px 0 12px;
  color: var(--fluent-danger);
  font-size: 14px;
  line-height: 1.5;
  word-break: break-all;
}

/* ===== 工作区布局 ===== */
.app-workspace {
  flex: 1;
  min-height: 0;
  display: grid;
  grid-template-columns: 152px minmax(0, 1fr);
  background: var(--fluent-bg);
}

/* ===== 侧边栏：Fluent 导航 ===== */
.function-sidebar {
  min-height: 0;
  padding: 8px;
  border-right: 1px solid var(--fluent-stroke);
  background: var(--fluent-layer);
}

.function-nav-item {
  width: 100%;
  height: 40px;
  margin-bottom: 4px;
  padding: 0 12px;
  border: 0;
  border-radius: var(--fluent-radius-md);
  background: transparent;
  color: var(--fluent-text-secondary);
  display: flex;
  align-items: center;
  gap: 12px;
  cursor: pointer;
  font-size: 14px;
  font-weight: 400;
  text-align: left;
  position: relative;
  transition: background-color var(--fluent-duration) var(--fluent-easing),
    color var(--fluent-duration) var(--fluent-easing),
    transform var(--motion-fast) var(--ease-out);
}

.function-nav-item::before {
  content: '';
  position: absolute;
  left: 0;
  width: 3px;
  height: 0;
  border-radius: var(--fluent-radius-pill);
  background: var(--fluent-accent);
  transition: height var(--motion-standard) var(--ease-standard);
}

.function-nav-item .el-icon {
  font-size: 18px;
}

.function-nav-item:hover {
  background: var(--fluent-layer-hover);
  color: var(--fluent-text);
}

.function-nav-item.active {
  background: var(--fluent-accent-light);
  color: var(--fluent-accent);
  font-weight: 600;
}

.function-nav-item.active::before {
  height: 18px;
}

/* ===== Header 内容 ===== */
.el-header {
  background-color: transparent;
  color: var(--fluent-text);
  display: flex;
  align-items: center;
  padding: 0 16px;
  height: 48px;
  flex-shrink: 0;
}

.header-content {
  width: 100%;
  display: flex;
  justify-content: space-between;
  align-items: center;
  gap: 12px;
}

.header-left {
  display: flex;
  align-items: center;
  gap: 0;
}

.app-title {
  font-size: 14px;
  font-weight: 600;
  color: var(--fluent-text);
}

.header-actions {
  display: flex;
  align-items: center;
  gap: 4px;
  margin-left: auto;
}

.header-btn {
  color: var(--fluent-text-secondary) !important;
  font-size: 18px;
  border-radius: var(--fluent-radius-md);
  transition: background-color var(--fluent-duration) var(--fluent-easing);
}

.header-btn:hover {
  color: var(--fluent-accent) !important;
  background: var(--fluent-layer-hover);
}

.no-drag {
  -webkit-app-region: no-drag;
}

.workspace-main,
.workspace-main *,
.login-main,
.login-main * {
  -webkit-app-region: no-drag;
}

.workspace-main {
  position: relative;
}

.workspace-view-enter-active,
.workspace-view-leave-active {
  transition: opacity var(--motion-fast) var(--ease-out);
}

.workspace-view-enter-from,
.workspace-view-leave-to {
  opacity: 0;
}

.workspace-view-leave-to {
  position: absolute;
  inset: 0;
}

.no-workspace-motion .workspace-view-enter-active,
.no-workspace-motion .workspace-view-leave-active {
  transition: none;
}

/* ===== 窗口控制按钮（Fluent 风格） ===== */
.window-controls {
  display: flex;
  align-items: stretch;
  align-self: stretch;
  margin-right: -16px;
}

.window-control-btn {
  width: 46px;
  height: 100%;
  border: none;
  background: transparent;
  color: var(--fluent-text);
  display: inline-flex;
  align-items: center;
  justify-content: center;
  cursor: pointer;
  transition: background-color var(--fluent-duration) var(--fluent-easing);
}

.window-control-btn:hover {
  background: var(--fluent-layer-hover);
}

.window-control-btn:active {
  background: var(--fluent-layer-pressed);
}

.close-btn:hover {
  background: var(--fluent-danger);
  color: #fff;
}

.close-btn:active {
  background: #b12419;
}

.window-control-icon {
  position: relative;
  display: inline-block;
}

.window-control-minimize {
  width: 12px;
  height: 2px;
  border-radius: var(--fluent-radius-pill);
  background: currentColor;
}

.window-control-close {
  width: 12px;
  height: 12px;
}

.window-control-maximize {
  width: 11px;
  height: 11px;
  border: 1px solid currentColor;
}

.window-control-restore {
  width: 11px;
  height: 11px;
  border: 1px solid currentColor;
}

.window-control-restore::before {
  content: '';
  position: absolute;
  width: 8px;
  height: 8px;
  left: 2px;
  top: -3px;
  border: 1px solid currentColor;
  background: var(--color-bg-surface, var(--fluent-layer));
  z-index: -1;
}

.window-control-close::before,
.window-control-close::after {
  content: '';
  position: absolute;
  left: 5px;
  top: 0;
  width: 2px;
  height: 12px;
  border-radius: var(--fluent-radius-pill);
  background: currentColor;
}

.window-control-close::before {
  transform: rotate(45deg);
}

.window-control-close::after {
  transform: rotate(-45deg);
}

/* ===== 用户信息 ===== */
.user-info {
  display: flex;
  align-items: center;
  gap: 8px;
  cursor: pointer;
  color: var(--fluent-text-secondary);
  font-size: 13px;
  padding: 4px 8px;
  border-radius: var(--fluent-radius-md);
  transition: background-color var(--fluent-duration) var(--fluent-easing);
}

.user-name {
  font-size: 13px;
  color: var(--fluent-text);
  max-width: 160px;
  overflow: hidden;
  text-overflow: ellipsis;
  white-space: nowrap;
}

.user-level {
  padding: 1px 5px;
  border-radius: var(--fluent-radius-pill);
  background: var(--fluent-accent-light);
  font-size: 11px;
  color: var(--fluent-accent);
  font-weight: 600;
  line-height: 16px;
}

.user-info:hover {
  background: var(--fluent-layer-hover);
}

.user-info:hover .user-name {
  color: var(--fluent-accent);
}

/* ===== 设置面板 ===== */
.settings-panel {
  padding: 2px;
}

.settings-group + .settings-group {
  margin-top: 10px;
  padding-top: 10px;
  border-top: 1px solid var(--fluent-divider);
}

.settings-group-title {
  padding: 2px 8px 5px;
  color: var(--fluent-text-tertiary);
  font-size: 11px;
  font-weight: 600;
  letter-spacing: 0.04em;
  text-transform: uppercase;
}

.settings-item {
  display: flex;
  justify-content: space-between;
  align-items: center;
  min-height: 36px;
  padding: 6px 8px;
  font-size: 14px;
  color: var(--fluent-text);
}

.settings-item .el-select {
  width: 132px;
}

.settings-item.clickable {
  width: 100%;
  border: 0;
  background: transparent;
  font: inherit;
  text-align: left;
  cursor: pointer;
  border-radius: var(--fluent-radius-sm);
  transition: background-color var(--fluent-duration) var(--fluent-easing);
}

.settings-item.clickable:hover {
  background: var(--fluent-layer-hover);
}

.settings-tip {
  margin-top: 6px;
  color: var(--fluent-text-tertiary);
  font-size: 12px;
  line-height: 1.5;
}

.version-info {
  color: var(--fluent-text-tertiary);
  font-size: 12px;
}

/* ===== 主内容区：透明背景，沿用工作区底色 ===== */
.el-main {
  flex: 1;
  padding: 20px 32px;
  background-color: transparent;
  overflow-y: auto;
  min-width: 0;
}

.login-main {
  padding: 0;
  overflow: hidden;
  background: var(--fluent-bg);
  min-height: 0;
}

.invite-panel {
  display: flex;
  flex-direction: column;
  gap: 14px;
}

.invite-row {
  display: flex;
  flex-direction: column;
  gap: 8px;
}

.invite-label {
  font-size: 13px;
  color: var(--fluent-text-secondary);
}

.invite-tip {
  margin: 0;
  color: var(--fluent-text-tertiary);
  font-size: 13px;
}

@media (max-width: 860px) {
  .app-workspace { grid-template-columns: 64px minmax(0, 1fr); }
  .function-sidebar { padding: var(--space-2); }
  .function-nav-item { justify-content: center; padding: 0; }
  .function-nav-item span {
    position: absolute;
    width: 1px;
    height: 1px;
    overflow: hidden;
    clip-path: inset(50%);
    white-space: nowrap;
  }
  .el-main { padding: var(--space-4); }
  .user-name { max-width: 92px; }
  .user-level { display: none; }
}
</style>
