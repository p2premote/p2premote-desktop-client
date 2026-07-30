import i18n from '../i18n'

export type UnlistenFn = () => void

export interface WebAuthStatus {
  authenticated: boolean
  security_code_required: boolean
}

type EventHandler<T> = (event: { payload: T }) => void

const webListeners = new Map<string, Set<EventHandler<any>>>()
let webSocket: WebSocket | null = null
let webSocketConnecting = false
let webSocketSuspended = false
let titleResetTimer: ReturnType<typeof window.setTimeout> | null = null
let titleBeforeFlash: string | null = null

export function isTauriRuntime(): boolean {
  return typeof window !== 'undefined' && '__TAURI_INTERNALS__' in window
}

export async function invoke<T = unknown>(command: string, args: Record<string, unknown> = {}): Promise<T> {
  if (isTauriRuntime()) {
    const tauri = await import('@tauri-apps/api/core')
    return tauri.invoke<T>(command, args)
  }

  if (command === 'show_system_notification') {
    showWebNotification(String(args.title || ''), String(args.body || ''))
    return undefined as T
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

function showWebNotification(title: string, body: string): void {
  if (typeof Notification !== 'undefined' && Notification.permission === 'granted') {
    new Notification(title, { body })
  }
  if (document.visibilityState !== 'visible' || !document.hasFocus()) {
    flashWebPageTitle(title)
  }
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
    return { authenticated: true, security_code_required: false }
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
  resumeWebSocket()
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
    const opener = await import('@tauri-apps/plugin-opener')
    await opener.openUrl(url)
    return
  }

  window.open(url, '_blank', 'noopener,noreferrer')
}

export async function minimizeWindow(): Promise<void> {
  if (!isTauriRuntime()) {
    return
  }
  const tauriWindow = await import('@tauri-apps/api/window')
  await tauriWindow.getCurrentWindow().minimize()
}

export async function toggleMaximizeWindow(): Promise<boolean> {
  if (!isTauriRuntime()) return false
  const tauriWindow = await import('@tauri-apps/api/window')
  const window = tauriWindow.getCurrentWindow()
  await window.toggleMaximize()
  return window.isMaximized()
}

export async function isWindowMaximized(): Promise<boolean> {
  if (!isTauriRuntime()) return false
  const tauriWindow = await import('@tauri-apps/api/window')
  return tauriWindow.getCurrentWindow().isMaximized()
}

export async function closeWindow(): Promise<void> {
  if (!isTauriRuntime()) {
    window.close()
    return
  }
  const tauriWindow = await import('@tauri-apps/api/window')
  await tauriWindow.getCurrentWindow().close()
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
