import { defineStore } from 'pinia'
import { ref } from 'vue'
import { invoke, listen } from '../runtime/bridge'
import type { DeviceInfo, WsEventPayload } from '../types/api'

// 视图沿用 from stores/device 的 DeviceInfo 导入路径
export type { DeviceInfo } from '../types/api'

/// API 统一响应格式
interface ApiResponse<T> {
  code: number
  msg: string
  data: T
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
  const loading = ref(false)

  // WS 事件监听器
  let fetchDevicesPromise: Promise<void> | null = null
  let lastFetchDevicesAt = 0

  async function fetchDevices(options: { force?: boolean } = {}) {
    if (fetchDevicesPromise) {
      return fetchDevicesPromise
    }

    const now = Date.now()
    if (!options.force && lastFetchDevicesAt > 0 && now - lastFetchDevicesAt < FETCH_THROTTLE_MS) {
      return
    }

    lastFetchDevicesAt = now
    loading.value = true
    fetchDevicesPromise = (async () => {
      try {
        const resp = await invoke<ApiResponse<DeviceInfo[]>>('get_device_list')
        if (resp.code === 0) {
          devices.value = (resp.data || []).sort((a, b) => a.device_id - b.device_id)
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

  function setDevices(newDevices: DeviceInfo[]) {
    devices.value = newDevices.sort((a, b) => a.device_id - b.device_id)
  }

  function resetDevices() {
    devices.value = []
    loading.value = false
    fetchDevicesPromise = null
    lastFetchDevicesAt = 0
  }

  /// 监听设备上线事件
  function onDeviceOnline(callback: (deviceId: number, deviceName: string) => void) {
    listen<WsEventPayload>('ws-device-online', (event) => {
      const data = event.payload.data as DeviceOnlinePayload
      const device = devices.value.find(d => d.device_id === data.device_id)
      if (device) {
        device.status = 'online'
      } else {
        void fetchDevices({ force: true })
      }
      callback(data.device_id, data.device_name)
    })
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
    })
  }

  return {
    devices,
    loading,
    fetchDevices,
    setDevices,
    resetDevices,
    onDeviceOnline,
    onDeviceOffline
  }
})
