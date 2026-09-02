import { defineStore } from 'pinia'
import { ref } from 'vue'
import { invoke, listen } from '../runtime/bridge'

export interface DeviceInfo {
  device_id: number
  device_uuid: string
  device_name: string
  device_alias?: string
  device_type: string
  status: string
  lan_ip?: string
  rdp_enabled: boolean
  service_port: number
  remote_access?: {
    protocol: 'rdp' | 'vnc' | 'custom' | string
    enabled: boolean
    port: number
  }
  public_ip?: string
  public_ip_location?: string
  system_version: string
  client_version?: string
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
  const loading = ref(false)
  const hasFetched = ref(false)

  // WS 事件监听器
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

  function setDevices(newDevices: DeviceInfo[]) {
    devices.value = newDevices.sort((a, b) => a.device_id - b.device_id)
    hasFetched.value = true
  }

  function resetDevices() {
    devices.value = []
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
    hasFetched,
    fetchDevices,
    setDevices,
    resetDevices,
    onDeviceOnline,
    onDeviceOffline
  }
})
