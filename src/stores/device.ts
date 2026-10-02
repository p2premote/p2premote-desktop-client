import { defineStore } from 'pinia'
import { ref } from 'vue'
import { invoke } from '../runtime/bridge'
import type { DeviceInfo } from '../types/api'

// 视图沿用 from stores/device 的 DeviceInfo 导入路径
export type { DeviceInfo } from '../types/api'

/// API 统一响应格式
interface ApiResponse<T> {
  code: number
  msg: string
  data: T
}


export const useDeviceStore = defineStore('device', () => {
  const FETCH_THROTTLE_MS = 10_000
  const devices = ref<DeviceInfo[]>([])
  const loading = ref(false)

  // 上次选中的设备只存活在内存里：页面切换、窗口隐藏到托盘后重新打开都保留，
  // GUI 进程重启即失，不写任何存储。
  const lastSelectedDeviceId = ref<number | null>(null)

  // WS 事件监听器
  let fetchDevicesPromise: Promise<void> | null = null
  let lastFetchDevicesAt = 0

  function rememberSelectedDevice(deviceId: number) {
    lastSelectedDeviceId.value = deviceId
  }

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
    lastSelectedDeviceId.value = null
  }

  return {
    devices,
    loading,
    lastSelectedDeviceId,
    fetchDevices,
    setDevices,
    resetDevices,
    rememberSelectedDevice
  }
})
