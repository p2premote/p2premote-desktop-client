export function resolveLaunchableRdpAddress(
  address: string,
  device: { service_port?: number | null; rdp_port?: number | null }
): string | null {
  const port = Number(device.service_port || device.rdp_port || 3389)
  if (!Number.isInteger(port) || port <= 0 || port > 65535) {
    return null
  }

  if (address.includes('{port}')) {
    return address.replace(/\{port\}/g, String(port))
  }

  // WGVPN 成功结果是对端虚拟 IP；mstsc 对纯 IP 虽会使用默认端口，
  // 但统一补端口可兼容设备配置的非默认 RDP 端口。
  if (!address.includes(':')) {
    return `${address}:${port}`
  }

  return address
}
