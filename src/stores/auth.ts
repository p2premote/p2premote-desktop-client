import { defineStore } from 'pinia'
import { ref, computed } from 'vue'
import { invoke } from '../runtime/bridge'
import { useDeviceStore } from './device'
import i18n from '../i18n'

export interface UserInfo {
  user_id: number
  username: string
  email: string
  member_level?: string
  member_expire_time?: string
  trial_start_time?: string
  trial_used?: boolean
  trial_remaining_days?: number
}

export interface DeviceInfo {
  device_id: number
  device_uuid: string
  device_name: string
  device_alias?: string
  device_type: string
  status: string
  lan_ip?: string
  rdp_enabled: boolean
  rdp_port: number
  public_ip?: string
  system_version: string
  service_port: number
  connect_code?: string
  created_at?: string
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

  async function syncServiceRuntimeConfig() {
    try {
      const status = await invoke<any>('sync_service_runtime_config')
      if (status?.runtime?.current_device) {
        currentDevice.value = status.runtime.current_device
      }
      console.log('[Service] synced background service runtime config')
      return status
    } catch (e) {
      console.error('[Service] sync runtime config failed:', e)
      throw e
    }
  }

  async function login(identifier: string, password: string) {
    console.log('[AuthStore] 开始登录:', identifier)
    deviceStore.resetDevices()

    const response = await invoke<LoginResponse>('login', {
      identifier,
      password
    })

    // 安全：不打印完整响应（避免 token 泄露到日志）。响应已不含 token，但仍保持习惯。
    if (response.code === 0 && response.data) {
      // service 是令牌唯一管理者，UI 用占位符表示已登录，不持有真实 token
      token.value = '__service_session__'
      userInfo.value = {
        user_id: response.data.user.user_id,
        username: response.data.user.username,
        email: response.data.user.email,
        member_level: response.data.user.member_level,
        member_expire_time: response.data.user.member_expire_time,
        trial_start_time: response.data.user.trial_start_time,
        trial_used: response.data.user.trial_used,
        trial_remaining_days: response.data.user.trial_remaining_days
      }
      console.log('[AuthStore] 登录成功, user:', userInfo.value.username)

      try {
        const status = await syncServiceRuntimeConfig()
        if (status?.runtime?.current_device) {
          console.log('[AuthStore] service 设备配置已同步:', JSON.stringify(status.runtime.current_device))
        }
        await deviceStore.fetchDevices({ force: true })
      } catch (e) {
        console.error('[AuthStore] service 配置同步失败:', e)
      }
      return true
    }

    console.error('[AuthStore] 登录失败:', response.msg)
    // Tauri/service 已按当前 locale 本地化；旧服务端仍由 core 回退到 msg。
    throw new Error(response.msg || i18n.global.t('errors.login_failed'))
  }

  async function resumeSavedSession(autoLogin: boolean) {
    deviceStore.resetDevices()
    await invoke('resume_saved_session', { autoLogin })
    const info = await invoke<UserInfo | null>('fetch_user_profile')
    if (!info) {
      throw new Error(i18n.global.t('errors.login_failed'))
    }
    token.value = '__service_session__'
    userInfo.value = info
    const status = await syncServiceRuntimeConfig()
    if (status?.runtime?.current_device) {
      currentDevice.value = status.runtime.current_device
    }
    await deviceStore.fetchDevices({ force: true })
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

  function setToken(newToken: string) {
    token.value = newToken
  }

  async function fetchUserInfo() {
    const info = await invoke<UserInfo | null>('fetch_user_profile')
    if (info) {
      userInfo.value = info
    }
    return info
  }

  return {
    token,
    user: userInfo,
    userInfo,
    currentDevice,
    isLoggedIn,
    login,
    resumeSavedSession,
    logout,
    setToken,
    fetchUserInfo,
  }
})
