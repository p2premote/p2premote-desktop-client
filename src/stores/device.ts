import { defineStore } from 'pinia'
import { ref } from 'vue'
import { invoke, listen, type UnlistenFn } from '../runtime/bridge'

export interface DeviceInfo {
  device_id: number
  device_uuid: string
  device_name: string
  device_alias?: string
  device_type: string
  status: string
  lan_ip?: string
  rdp_enabled: boolean
  service_port: number  // RDP 端口（服务端字段名）
  public_ip?: string
  public_ip_location?: string
  system_version: string
  connect_code?: string
  created_at?: string
}

/// API 统一响应格式
interface ApiResponse<T> {
  code: number
  msg: string
  data: T
}

/// WS 事件负载
interface WsEventPayload {
  msg_type: string
  data: Record<string, any>
}

/// 设备在线事件
interface DeviceOnlinePayload {
  device_id: number
  device_name: string
}

/// 设备离线事件
interface DeviceOfflinePayload {
  device_id: number
}

export const useDeviceStore = defineStore('device', () => {
  const FETCH_THROTTLE_MS = 10_000
  const devices = ref<DeviceInfo[]>([])
  const currentDevice = ref<DeviceInfo | null>(null)
  const loading = ref(false)
  const hasFetched = ref(false)

  // WS 事件监听器
  let unlistenDeviceOnline: UnlistenFn | null = null
  let unlistenDeviceOffline: UnlistenFn | null = null
  let fetchDevicesPromise: Promise<void> | null = null
  let lastFetchDevicesAt = 0

  async function fetchDevices(options: { force?: boolean } = {}) {
    if (fetchDevicesPromise) {
      console.log('[DeviceStore] fetchDevices already in progress, reusing existing request')
      return fetchDevicesPromise
    }

    const now = Date.now()
    if (!options.force && lastFetchDevicesAt > 0 && now - lastFetchDevicesAt < FETCH_THROTTLE_MS) {
      console.log('[DeviceStore] fetchDevices skipped by throttle')
      return
    }

    lastFetchDevicesAt = now
    loading.value = true
    fetchDevicesPromise = (async () => {
      try {
        console.log('[DeviceStore] start fetching device list')
        const resp = await invoke<ApiResponse<DeviceInfo[]>>('get_device_list')
        console.log('[DeviceStore] get_device_list response:', JSON.stringify(resp))
        if (resp.code === 0) {
          devices.value = (resp.data || []).sort((a, b) => a.device_id - b.device_id)
          hasFetched.value = true
          console.log('[DeviceStore] device list updated, count:', devices.value.length)
          devices.value.forEach(d => {
            console.log(`[DeviceStore] device ${d.device_name}: rdp_enabled=${d.rdp_enabled}, service_port=${d.service_port}, status=${d.status}, system_version=${d.system_version}`)
          })
        } else {
          console.error('[DeviceStore] get_device_list failed:', resp.code, resp.msg)
        }
      } catch (e) {
        console.error('[DeviceStore] invoke get_device_list error:', e)
      } finally {
        loading.value = false
        fetchDevicesPromise = null
      }
    })()

    return fetchDevicesPromise
  }

  async function autoRegisterDevice() {
    const device = await invoke<DeviceInfo>('register_current_device_auto')
    return device
  }

  async function updateAlias(deviceId: number, alias: string) {
    await invoke('update_device_alias', { deviceId, alias })
  }

  async function deleteDevice(deviceId: number) {
    await invoke('delete_device', { deviceId })
  }

  function setDevices(newDevices: DeviceInfo[]) {
    devices.value = newDevices.sort((a, b) => a.device_id - b.device_id)
    hasFetched.value = true
  }

  function setCurrentDevice(device: DeviceInfo) {
    currentDevice.value = device
  }

  function setLoading(state: boolean) {
    loading.value = state
  }

  function resetDevices() {
    devices.value = []
    currentDevice.value = null
    loading.value = false
    hasFetched.value = false
    fetchDevicesPromise = null
    lastFetchDevicesAt = 0
  }

  /// 监听设备上线事件
  function onDeviceOnline(callback: (deviceId: number, deviceName: string) => void) {
    listen<WsEventPayload>('ws-device-online', (event) => {
      console.log('[DeviceStore] received ws-device-online event:', event.payload)
      const data = event.payload.data as DeviceOnlinePayload
      const device = devices.value.find(d => d.device_id === data.device_id)
      if (device) {
        device.status = 'online'
      } else {
        void fetchDevices({ force: true })
      }
      callback(data.device_id, data.device_name)
    }).then(fn => { unlistenDeviceOnline = fn })
  }

  /// 监听设备离线事件
  function onDeviceOffline(callback: (deviceId: number) => void) {
    listen<WsEventPayload>('ws-device-offline', (event) => {
      const data = event.payload.data as DeviceOfflinePayload
      const device = devices.value.find(d => d.device_id === data.device_id)
      if (device) {
        device.status = 'offline'
      }
      callback(data.device_id)
    }).then(fn => { unlistenDeviceOffline = fn })
  }

  /// 清理监听器
  function cleanupListeners() {
    unlistenDeviceOnline?.()
    unlistenDeviceOffline?.()
    unlistenDeviceOnline = null
    unlistenDeviceOffline = null
  }

  return {
    devices,
    currentDevice,
    loading,
    hasFetched,
    fetchDevices,
    autoRegisterDevice,
    updateAlias,
    deleteDevice,
    setDevices,
    setCurrentDevice,
    setLoading,
    resetDevices,
    onDeviceOnline,
    onDeviceOffline,
    cleanupListeners
  }
})
