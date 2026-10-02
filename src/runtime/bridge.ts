import i18n from '../i18n'

export type UnlistenFn = () => void

export const SITE_ORIGIN = 'https://www.p2premote.top'
export const CLIENT_DOWNLOAD_URL = `${SITE_ORIGIN}/#download`

export interface WebAuthStatus {
  authenticated: boolean
  security_code_required: boolean
  first_trust_pending?: boolean
  source_ip: string
}

export interface WebAdminSettings {
  mode: 'local' | 'remote'
  allowed_ip: string | null
  listen_addr: string
  source_ip: string
}

export interface WebAccessUpdateResponse {
  ok: boolean
  changed: boolean
  source_allowed: boolean
  mode?: 'local' | 'remote'
  allowed_ip?: string | null
  listen_addr?: string
  config_path?: string
}

export interface UpdateCheckResponse {
  mode: 'none' | 'optional' | 'force'
  has_update: boolean
  force_update: boolean
  current: string
  latest: string
  min_supported: string
  release_notes: string
  error?: string | null
}

type EventHandler<T> = (event: { payload: T }) => void

const webListeners = new Map<string, Set<EventHandler<any>>>()
let webSocket: WebSocket | null = null
let webSocketConnecting = false
let webSocketSuspended = false
let titleResetTimer: ReturnType<typeof window.setTimeout> | null = null
let titleBeforeFlash: string | null = null

export function isTauriRuntime(): boolean {
  return typeof window !== 'undefined' && (
    '__TAURI_IPC__' in window || '__TAURI_METADATA__' in window || '__TAURI__' in window
  )
}

export async function invoke<T = unknown>(command: string, args: Record<string, unknown> = {}): Promise<T> {
  if (isTauriRuntime()) {
    const tauri = await import('@tauri-apps/api/tauri')
    return tauri.invoke<T>(command, args)
  }

  if (command === 'flash_main_window') {
    flashWebPageTitle()
    return undefined as T
  }

  const response = await fetch(`/api/invoke/${encodeURIComponent(command)}`, {
    method: 'POST',
    headers: {
      'Content-Type': 'application/json',
    },
    body: JSON.stringify(args || {}),
  })
  const payload = await response.json().catch(() => null)
  if (!response.ok || !payload?.ok) {
    if (response.status === 401) {
      suspendWebSocket()
      window.dispatchEvent(new CustomEvent('p2premote-web-auth-required'))
    }
    throw new Error(payload?.error || i18n.global.t('errors.bridge_call_failed', { command }))
  }
  return payload.value as T
}

function flashWebPageTitle(message?: string): void {
  if (titleBeforeFlash === null) {
    titleBeforeFlash = document.title
  }
  document.title = message ? `● ${message}` : `● ${titleBeforeFlash}`
  if (titleResetTimer !== null) {
    window.clearTimeout(titleResetTimer)
  }
  titleResetTimer = window.setTimeout(() => {
    document.title = titleBeforeFlash || document.title
    titleBeforeFlash = null
    titleResetTimer = null
  }, 5000)
}

export async function getWebAuthStatus(): Promise<WebAuthStatus> {
  if (isTauriRuntime()) {
    return {
      authenticated: true,
      security_code_required: false,
      source_ip: '127.0.0.1',
    }
  }
  const response = await fetch('/api/web-auth/status', { credentials: 'same-origin' })
  const contentType = response.headers.get('content-type') || ''
  if (!response.ok || !contentType.includes('application/json')) {
    throw new Error('web_service_unavailable')
  }
  return response.json().catch(() => {
    throw new Error('web_service_unavailable')
  })
}

export async function claimWebFirstTrust(): Promise<void> {
  const response = await fetch('/api/web-auth/claim-first-trust', {
    method: 'POST',
    credentials: 'same-origin',
  })
  const payload = await response.json().catch(() => null)
  if (!response.ok || !payload?.ok) {
    const error = new Error(payload?.error || `First-use claim failed: ${response.status}`)
    ;(error as Error & { code?: string }).code = payload?.error
    throw error
  }
}

export async function completeWebFirstTrust(
  newSecurityCode: string,
  allowedIp: string,
): Promise<{ source_allowed: boolean; allowed_ip: string }> {
  const response = await fetch('/api/web-auth/complete-first-trust', {
    method: 'POST',
    credentials: 'same-origin',
    headers: { 'Content-Type': 'application/json' },
    body: JSON.stringify({ new_security_code: newSecurityCode, allowed_ip: allowedIp }),
  })
  const payload = await response.json().catch(() => null)
  if (!response.ok || !payload?.ok) {
    const error = new Error(payload?.error || `First-use setup failed: ${response.status}`)
    ;(error as Error & { code?: string }).code = payload?.error
    throw error
  }
  return payload
}

export async function unlockWebAdmin(securityCode: string): Promise<void> {
  const response = await fetch('/api/web-auth/unlock', {
    method: 'POST',
    credentials: 'same-origin',
    headers: { 'Content-Type': 'application/json' },
    body: JSON.stringify({ security_code: securityCode }),
  })
  const payload = await response.json().catch(() => null)
  if (!response.ok || !payload?.ok) {
    const error = new Error(payload?.error || `Web authentication failed: ${response.status}`)
    ;(error as Error & { code?: string }).code = payload?.error
    throw error
  }
}

export async function changeWebAdminSecurityCode(
  currentSecurityCode: string | null,
  newSecurityCode: string,
): Promise<void> {
  const response = await fetch('/api/web-auth/change-security-code', {
    method: 'POST',
    credentials: 'same-origin',
    headers: { 'Content-Type': 'application/json' },
    body: JSON.stringify({
      current_security_code: currentSecurityCode,
      new_security_code: newSecurityCode,
    }),
  })
  const payload = await response.json().catch(() => null)
  if (!response.ok || !payload?.ok) {
    const error = new Error(payload?.error || `Changing Web security code failed: ${response.status}`)
    ;(error as Error & { code?: string }).code = payload?.error
    throw error
  }
  suspendWebSocket()
  window.dispatchEvent(new CustomEvent('p2premote-web-auth-required'))
}

export async function getWebAdminSettings(): Promise<WebAdminSettings> {
  const response = await fetch('/api/web-admin/settings', { credentials: 'same-origin' })
  if (!response.ok) throw new Error(`Loading Web settings failed: ${response.status}`)
  return response.json()
}

export async function updateWebAdminAccess(
  mode: 'local' | 'remote',
  allowedIp: string | null,
): Promise<WebAccessUpdateResponse> {
  const response = await fetch('/api/web-admin/access', {
    method: 'POST',
    credentials: 'same-origin',
    headers: { 'Content-Type': 'application/json' },
    body: JSON.stringify({ mode, allowed_ip: allowedIp }),
  })
  const payload = await response.json().catch(() => null)
  if (!response.ok || !payload?.ok) {
    const error = new Error(payload?.error || `Updating Web access failed: ${response.status}`)
    ;(error as Error & { code?: string }).code = payload?.error
    throw error
  }
  if (payload.changed) suspendWebSocket()
  return payload as WebAccessUpdateResponse
}

export async function logoutWebAdmin(): Promise<void> {
  if (isTauriRuntime()) return
  suspendWebSocket()
  const response = await fetch('/api/web-auth/logout', {
    method: 'POST',
    credentials: 'same-origin',
    headers: { 'Content-Type': 'application/json' },
    body: '{}',
  })
  if (!response.ok) throw new Error(`Web logout failed: ${response.status}`)
  window.dispatchEvent(new CustomEvent('p2premote-web-auth-required'))
}

export function suspendWebSocket(): void {
  if (isTauriRuntime()) return
  webSocketSuspended = true
  const socket = webSocket
  webSocket = null
  webSocketConnecting = false
  socket?.close(1000, 'web session suspended')
}

export function resumeWebSocket(): void {
  if (isTauriRuntime()) return
  webSocketSuspended = false
  if (webListeners.size > 0) ensureWebSocket()
}

export async function listen<T = unknown>(event: string, handler: EventHandler<T>): Promise<UnlistenFn> {
  if (isTauriRuntime()) {
    const tauri = await import('@tauri-apps/api/event')
    return tauri.listen<T>(event, handler)
  }

  let handlers = webListeners.get(event)
  if (!handlers) {
    handlers = new Set()
    webListeners.set(event, handlers)
  }
  handlers.add(handler as EventHandler<any>)
  ensureWebSocket()

  return () => {
    handlers?.delete(handler as EventHandler<any>)
  }
}

export async function openExternal(url: string): Promise<void> {
  if (isTauriRuntime()) {
    const shell = await import('@tauri-apps/api/shell')
    await shell.open(url)
    return
  }

  window.open(url, '_blank', 'noopener,noreferrer')
}

export async function openClientDownloadPage(): Promise<void> {
  return openExternal(CLIENT_DOWNLOAD_URL)
}

export async function minimizeWindow(): Promise<void> {
  if (!isTauriRuntime()) {
    return
  }
  const tauriWindow = await import('@tauri-apps/api/window')
  await tauriWindow.appWindow.minimize()
}

export async function startWindowDragging(): Promise<void> {
  if (!isTauriRuntime()) return
  const tauriWindow = await import('@tauri-apps/api/window')
  await tauriWindow.appWindow.startDragging()
}

export async function toggleMaximizeWindow(): Promise<boolean> {
  if (!isTauriRuntime()) return false
  const tauriWindow = await import('@tauri-apps/api/window')
  const window = tauriWindow.appWindow
  await window.toggleMaximize()
  return window.isMaximized()
}

export async function isWindowMaximized(): Promise<boolean> {
  if (!isTauriRuntime()) return false
  const tauriWindow = await import('@tauri-apps/api/window')
  return tauriWindow.appWindow.isMaximized()
}

export async function closeWindow(): Promise<void> {
  if (!isTauriRuntime()) {
    window.close()
    return
  }
  const tauriWindow = await import('@tauri-apps/api/window')
  // The title-bar close button means "keep running in the tray". Hiding the
  // window directly avoids depending on the native close-event interception.
  await tauriWindow.appWindow.hide()
}

function ensureWebSocket() {
  if (webSocketSuspended || webSocket || webSocketConnecting) {
    return
  }
  webSocketConnecting = true
  const protocol = window.location.protocol === 'https:' ? 'wss:' : 'ws:'
  const socket = new WebSocket(`${protocol}//${window.location.host}/api/events`)
  webSocket = socket

  socket.addEventListener('open', () => {
    if (webSocket !== socket) return
    webSocketConnecting = false
  })
  socket.addEventListener('message', event => {
    if (webSocket !== socket) return
    const payload = JSON.parse(event.data)
    if (!payload?.event) {
      return
    }
    const handlers = webListeners.get(payload.event)
    handlers?.forEach(handler => handler({ payload: payload.payload }))
  })
  socket.addEventListener('close', event => {
    if (webSocket !== socket) return
    webSocket = null
    webSocketConnecting = false
    if (event.code === 4001) {
      webSocketSuspended = true
      window.dispatchEvent(new CustomEvent('p2premote-web-auth-required'))
      return
    }
    const handlers = webListeners.get('service-connection-lost')
    handlers?.forEach(handler => handler({ payload: undefined }))
    window.setTimeout(() => {
      if (!webSocketSuspended && webListeners.size > 0) {
        ensureWebSocket()
      }
    }, 1500)
  })
  socket.addEventListener('error', () => {
    socket.close()
  })
}
