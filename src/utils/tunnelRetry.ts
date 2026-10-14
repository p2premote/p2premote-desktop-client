import { invoke } from '../runtime/bridge'

// The stop response acknowledges task exit and resource cleanup. No polling or
// pending restart state is needed; the caller chooses how to start the connection.
export async function stopTunnelForRetry(peerDeviceId: number, disableTcp: boolean) {
  if (disableTcp) {
    const preferences = await invoke<{ prefer_ipv6: boolean; prefer_tcp: boolean }>('get_connection_preferences')
    await invoke('save_connection_preferences', {
      preferIpv6: preferences.prefer_ipv6,
      preferTcp: false,
    })
  }
  await invoke('stop_service_active_tunnel', { peerDeviceId })
}
