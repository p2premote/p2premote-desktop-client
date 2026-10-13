import i18n from '../i18n'

export interface ClientError {
  code: string
  params?: Record<string, unknown> | null
  traceId?: string
  retryable?: boolean
}

const ERROR_MESSAGE_KEYS: Record<string, string> = {
  NETWORK_OFFLINE: 'errors.network_offline',
  NETWORK_DNS_FAILED: 'errors.network_dns_failed',
  NETWORK_TIMEOUT: 'errors.network_timeout',
  NETWORK_REQUEST_FAILED: 'errors.network_request_failed',
  SERVER_UNAVAILABLE: 'errors.server_unavailable',
  SERVICE_NOT_INSTALLED: 'errors.service_not_installed',
  SERVICE_DISABLED: 'errors.service_disabled',
  SERVICE_START_FAILED: 'errors.service_start_failed',
  SERVICE_NOT_READY: 'errors.service_not_ready',
  SERVICE_CONNECTION_FAILED: 'errors.service_connection_failed',
  SERVICE_PERMISSION_DENIED: 'errors.service_permission_denied',
  AUTH_INVALID_CREDENTIALS: 'errors.auth_invalid_credentials',
  AUTH_SESSION_EXPIRED: 'errors.auth_session_expired',
  AUTH_SAVED_SESSION_EXPIRED: 'errors.auth_saved_session_expired',
  DEVICE_OFFLINE: 'errors.device_offline',
  DEVICE_LIMIT_REACHED: 'errors.device_limit_reached',
  TUNNEL_TIMEOUT: 'errors.tunnel_timeout',
}

function isClientError(value: unknown): value is ClientError {
  return Boolean(
    value
      && typeof value === 'object'
      && 'code' in value
      && typeof (value as { code?: unknown }).code === 'string',
  )
}

function rawMessage(error: unknown): string {
  if (typeof error === 'string') return error.trim()
  if (error instanceof Error) return error.message.trim()
  if (error && typeof error === 'object' && 'message' in error) {
    return String((error as { message?: unknown }).message || '').trim()
  }
  return ''
}

const TECHNICAL_MESSAGE_PATTERN = /(?:https?:\/\/|file:\/\/|[a-z]:\\|\/api\/|\b(?:ipc|stack|panic|backtrace|serde|tauri|socket|channel closed|connection died|runtime status|response missing|unexpected service response|error sending request|failed to (?:send|receive|parse|serialize|deserialize|establish))\b)/i

function safeLegacyBusinessMessage(error: unknown): string {
  const message = rawMessage(error)
  if (!message || message.length > 200 || /[\r\n]/.test(message) || TECHNICAL_MESSAGE_PATTERN.test(message)) {
    return ''
  }
  return message
}

function classifyLegacyError(message: string): string | null {
  const value = message.toLowerCase()
  if (/saved[_ -]session.*expired/.test(value)) return 'AUTH_SAVED_SESSION_EXPIRED'
  if (/invalid (?:token|credential)|unauthorized|session.*expired/.test(value)) return 'AUTH_SESSION_EXPIRED'
  if (/permission denied|access denied|authorization check failed/.test(value)) return 'SERVICE_PERMISSION_DENIED'
  if (/service.*(?:not installed|installation is incomplete)/.test(value)) return 'SERVICE_NOT_INSTALLED'
  if (/service.*disabled/.test(value)) return 'SERVICE_DISABLED'
  if (/service.*(?:not ready|unavailable)|no persistent connection/.test(value)) return 'SERVICE_NOT_READY'
  if (/timed? out|timeout|超时/.test(value)) return 'NETWORK_TIMEOUT'
  if (/dns|name or service not known|failed to lookup address|域名解析/.test(value)) return 'NETWORK_DNS_FAILED'
  if (/offline|network is unreachable|not connected to (?:a )?network|网络不可用|设备不在线/.test(value)) return 'NETWORK_OFFLINE'
  if (/connection refused|connection reset|failed to connect|error sending request|无法连接服务器|网络请求失败/.test(value)) return 'NETWORK_REQUEST_FAILED'
  return null
}

export function normalizeClientError(error: unknown): ClientError {
  if (isClientError(error)) return error

  const message = rawMessage(error)
  const code = classifyLegacyError(message)
  if (code) {
    const retryable = !code.startsWith('AUTH_')
      && !['SERVICE_PERMISSION_DENIED', 'SERVICE_NOT_INSTALLED', 'SERVICE_DISABLED', 'DEVICE_LIMIT_REACHED'].includes(code)
    return { code, retryable }
  }

  return { code: 'UNKNOWN', retryable: true }
}

/** Convert an IPC/service error into text that is safe to display to users. */
export function errorMessage(error: unknown, fallback: string): string {
  const normalized = normalizeClientError(error)
  const key = ERROR_MESSAGE_KEYS[normalized.code]
  if (key) return i18n.global.t(key, normalized.params || {})

  const message = rawMessage(error)
  if (message) console.error('[UserFacingError] hidden technical error:', error)
  return fallback
}

/**
 * Compatibility boundary for legacy commands whose documented failure value is
 * a localized business message. Obvious diagnostics are still never exposed.
 */
export function businessErrorMessage(error: unknown, fallback: string): string {
  const normalized = normalizeClientError(error)
  const key = ERROR_MESSAGE_KEYS[normalized.code]
  if (key) return i18n.global.t(key, normalized.params || {})

  const message = safeLegacyBusinessMessage(error)
  if (message) return message
  return errorMessage(error, fallback)
}
