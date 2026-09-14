export type DevicePlatform =
  | 'windows-7'
  | 'windows-10'
  | 'windows-11'
  | 'windows'
  | 'ubuntu'
  | 'kylin'
  | 'uos'
  | 'android'
  | 'linux'
  | 'unknown'

export interface DevicePlatformSource {
  device_type?: string | null
  system_version?: string | null
}

export function detectDevicePlatform(device: DevicePlatformSource | null): DevicePlatform {
  if (!device) return 'unknown'

  const value = `${device.device_type || ''} ${device.system_version || ''}`.toLocaleLowerCase()

  if (/android|安卓/.test(value)) return 'android'
  if (/kylin|麒麟/.test(value)) return 'kylin'
  if (/\buos\b|统信/.test(value)) return 'uos'
  if (/ubuntu/.test(value)) return 'ubuntu'

  if (/windows|\bwin(?:dows)?\b/.test(value)) {
    if (/windows\s*11|\bwin\s*11\b/.test(value)) return 'windows-11'
    if (/windows\s*10|\bwin\s*10\b/.test(value)) return 'windows-10'
    if (/windows\s*7|\bwin\s*7\b/.test(value)) return 'windows-7'
    return 'windows'
  }

  if (/linux|debian|fedora|centos|red\s*hat|arch\b|opensuse/.test(value)) return 'linux'
  return 'unknown'
}

export function devicePlatformLabel(platform: DevicePlatform): string {
  const labels: Record<DevicePlatform, string> = {
    'windows-7': 'Windows 7',
    'windows-10': 'Windows 10',
    'windows-11': 'Windows 11',
    windows: 'Windows',
    ubuntu: 'Ubuntu',
    kylin: 'Kylin',
    uos: 'UOS',
    android: 'Android',
    linux: 'Linux',
    unknown: 'Unknown',
  }
  return labels[platform]
}
