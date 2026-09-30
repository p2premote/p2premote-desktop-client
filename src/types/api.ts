/**
 * 后端 DTO 单点定义。
 *
 * 字段与 core/src（control.rs / device.rs）的序列化输出一一对应；
 * 修改后端结构时同步更新这里，而不是在各视图复制接口。
 */

// ============ 设备 ============

export interface RemoteAccessInfo {
  protocol: string
  enabled: boolean
  port: number
}

/** core::device::DeviceInfo（get_device_list / RuntimeStatus.current_device） */
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
  remote_access?: RemoteAccessInfo
  public_ip?: string
  public_ip_location?: string
  system_version: string
  client_version?: string
  service_port: number
  connect_code?: string
  created_at?: string
  wake_available?: boolean
  capabilities?: string[]
}

// ============ 主动隧道任务 ============

/** core::p2p::ActiveStartResult */
export interface ActiveTunnelJobResult {
  success: boolean
  rdp_address: string
  remote_address?: string
  remote_protocol?: string
  network?: string
  source_nat_type?: string
  target_nat_type?: string
  message: string
  warning?: string | null
}

/** core::control::TunnelJobStatus（审计 O-1：主动/被动两轨统一元素类型。
 * active_tunnel_jobs 填 peer_device_uuid/result/tcp_retry_recommended；
 * wgvpn_jobs 填 is_active。） */
export interface TunnelJobStatus {
  peer_device_id: number
  peer_device_uuid?: string
  is_active?: boolean
  state: 'running' | 'waiting' | 'succeeded' | 'failed' | 'cancelled'
  attempt: number
  max_attempts: number
  message: string
  result?: ActiveTunnelJobResult | null
  tcp_retry_recommended?: boolean
  updated_at: number
}


// ============ 隧道生命周期 ============

/** core::control::TunnelLifecycleStatus */
export interface TunnelLifecycleStatus {
  peer_device_id: number
  source_user_id?: number
  source_username?: string
  source_email?: string
  peer_device_name?: string
  peer_device_alias?: string
  peer_public_ip?: string
  role: 'active' | 'passive'
  state: 'not_established' | 'connecting' | 'awaiting_approval' | 'connected' | 'recovering'
  attempt: number
  max_attempts: number
  stage?: string | null
  virtual_ip?: string | null
  peer_virtual_ip?: string | null
  last_result:
    | 'none'
    | 'user_disconnected'
    | 'peer_disconnected'
    | 'attempt_failed'
    | 'health_grace_expired'
    | 'cancelled'
  error_code?: string | null
  message?: string | null
  health_failures?: number
  health_grace_deadline?: number | null
  connected_at?: number | null
  updated_at: number
}

// ============ 后台服务状态 ============

/** core::control::PendingInboundApproval */
export interface InboundApprovalStatus {
  attempt_id: string
  source_user_id: number
  source_username?: string
  source_email?: string
  source_device_id: number
  source_device_name?: string
  source_device_alias?: string
  requested_at: number
  expires_at: number
}

/** get_service_status / service-status-changed 的响应载荷 */
export interface BackgroundServiceStatus {
  service: {
    installed: boolean
    running: boolean
    enabled: boolean
    raw_state: string
  }
  runtime?: {
    service_session_id?: string
    logged_in: boolean
    device_id?: number | null
    device_uuid?: string | null
    current_device?: DeviceInfo | null
    ws_connected: boolean
    last_heartbeat_at?: number | null
    last_error?: string | null
    public_ip?: string | null
    invite_temporary_password?: string | null
    wgvpn_sessions?: Array<{
      peer_device_id: number
      is_active: boolean
      approval_pending?: boolean
      virtual_ip: string
      peer_virtual_ip: string
      tunnel_name: string
    }>
    tunnel_lifecycles?: Array<{
      peer_device_id: number
      role: 'active' | 'passive'
      source_username?: string
      peer_device_name?: string
      peer_device_alias?: string
    }>
    pending_inbound_approvals?: InboundApprovalStatus[]
    active_tunnel_jobs?: TunnelJobStatus[]
  } | null
  machine_logged_in: boolean
  config_path: string
  log_dir: string
}
