import { defineStore } from 'pinia'
import { ref, computed } from 'vue'
import { invoke } from '../runtime/bridge'
import { useDeviceStore } from './device'
import type { DeviceInfo } from '../types/api'
import i18n from '../i18n'
import { businessErrorMessage } from '../utils/errorMessage'

export interface UserInfo {
  user_id: number
  username: string
  email: string
  member_level?: string
  member_expire_time?: string
  trial_start_time?: string
  trial_expire_time?: string
  trial_used?: boolean
  trial_remaining_days?: number
  is_pro?: boolean
}

interface LoginResponse {
  code: number
  msg: string
  data?: {
    // token 由 service 管理，不返回给 UI
    user: UserInfo
  }
}

export const useAuthStore = defineStore('auth', () => {
  const deviceStore = useDeviceStore()
  const token = ref('')
  const userInfo = ref<UserInfo | null>(null)
  const currentDevice = ref<DeviceInfo | null>(null)

  const isLoggedIn = computed(() => !!token.value)
  let authOperationTail: Promise<void> = Promise.resolve()

  function runAuthOperation<T>(operation: () => Promise<T>): Promise<T> {
    const result = authOperationTail.then(operation, operation)
    authOperationTail = result.then(() => undefined, () => undefined)
    return result
  }

  async function syncServiceRuntimeConfig() {
    try {
      const status = await invoke<any>('sync_service_runtime_config')
      if (status?.runtime?.current_device) {
        currentDevice.value = status.runtime.current_device
      }
      return status
    } catch (e) {
      throw e
    }
  }

  async function login(identifier: string, password: string) {
    return runAuthOperation(() => loginInner(identifier, password))
  }

  async function loginInner(identifier: string, password: string) {
    deviceStore.resetDevices()

    const response = await invoke<LoginResponse>('login', {
      identifier,
      password
    })

    // 安全：不打印完整响应（避免 token 泄露到日志）。响应已不含 token，但仍保持习惯。
    if (response.code === 0 && response.data) {
      // service 是令牌唯一管理者，UI 用占位符表示已登录，不持有真实 token
      token.value = '__service_session__'
      userInfo.value = response.data.user

      try {
        await syncServiceRuntimeConfig()
        await deviceStore.fetchDevices({ force: true })
      } catch (e) {
        console.error('[AuthStore] service 配置同步失败:', e)
      }
      return true
    }

    // Tauri/service 已按当前 locale 本地化；旧服务端仍由 core 回退到 msg。
    throw new Error(businessErrorMessage(response.msg, i18n.global.t('errors.login_failed')))
  }

  /// 接管一个已由后台 service 建立的会话（自动登录 token 或 '__service_session__'
  /// 哨兵）：置 token → 拉用户信息 → 同步 service 运行时配置 → 强制刷新设备列表。
  /// App.vue 的启动恢复流程与本 store 的 resumeSavedSession 共用此单一入口
  /// （审计 G-7：原先两份并行流程仅入口命令不同）。
  async function adoptServiceSessionInner(sessionToken: string) {
    token.value = sessionToken
    const info = await invoke<UserInfo | null>('fetch_user_profile')
    if (!info) {
      throw new Error(i18n.global.t('errors.login_failed'))
    }
    userInfo.value = info
    await syncServiceRuntimeConfig()
    await deviceStore.fetchDevices({ force: true })
  }

  async function adoptServiceSession(sessionToken: string) {
    return runAuthOperation(() => adoptServiceSessionInner(sessionToken))
  }

  async function resumeSavedSession(autoLogin: boolean) {
    return runAuthOperation(async () => {
      deviceStore.resetDevices()
      await invoke('resume_saved_session', { autoLogin })
      await adoptServiceSessionInner('__service_session__')
    })
  }

  async function recoverAvailableServiceSession(afterAdopt: () => Promise<void>) {
    return runAuthOperation(async () => {
      // A manual login may have completed while this recovery attempt was
      // waiting in the queue. Never replace or clear that newer session.
      if (isLoggedIn.value) return true

      try {
        const recoveredToken = await invoke<string>('try_auto_login')
        if (recoveredToken) {
          await adoptServiceSessionInner(recoveredToken)
          await afterAdopt()
          return true
        }

        const serviceLoggedIn = await invoke<boolean>('is_logged_in')
        if (serviceLoggedIn) {
          await adoptServiceSessionInner('__service_session__')
          await afterAdopt()
          return true
        }
        return false
      } catch (error) {
        // Keep the reset inside the serialized operation. A manual login queued
        // behind recovery must never be cleared by a late recovery failure.
        resetSession()
        throw error
      }
    })
  }

  async function logout() {
    try {
      if (currentDevice.value) {
        await invoke('mark_current_device_offline')
      }
    } catch (e) {
      console.error('[Auth] mark current device offline failed:', e)
    }
    await invoke('logout')
    token.value = ''
    userInfo.value = null
    currentDevice.value = null
    deviceStore.resetDevices()
  }

  // 启动恢复失败时的本地登出：仅清除 GUI 会话状态，不触发 service 的
  // Data::Logout——后者会连带清除"记住密码/自动登录"与 refresh token，
  // 一次瞬时的网络/服务端故障就会把用户的无人值守登录配置抹掉。
  function resetSession() {
    token.value = ''
    userInfo.value = null
    currentDevice.value = null
    deviceStore.resetDevices()
  }

  return {
    token,
    userInfo,
    currentDevice,
    isLoggedIn,
    login,
    adoptServiceSession,
    resumeSavedSession,
    recoverAvailableServiceSession,
    logout,
    resetSession,
  }
})
