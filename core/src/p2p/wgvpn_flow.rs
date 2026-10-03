//! WireGuard-over-P2P session lifecycle and platform integration.

use super::*;
use crate::config::install_data_dir;
use crate::gonc_ffi::{self, UdpTunnelRequest};
use crate::logging::{record_tunnel_audit, TunnelAuditAction, TunnelAuditEvent, TunnelAuditRole};
use crate::subnet_router::{self, LanMode};
use crate::wgvpn::{self, PeerConfig, WgConfigBuilder};
use crate::wgvpn_exchange;
use std::collections::HashSet;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

// ===== wgvpn 虚拟网段定义 =====
// 选用 100.64.0.0/10（RFC 6598 CGNAT 段）内的子网，避开常见的 10.x/172.16.x/192.168.x
// 用户内网段，几乎不会与现场环境冲突。参考 NetBird/Tailscale 同样使用 CGNAT 段。
// .71 沿用早期 PoC 的习惯编号，便于记忆。
//
// 多对端模型下：
//   - 被动端（被连接方）IP 固定 100.99.71.1，所有主动端都要能路由到它
//   - 主动端 IP 由被动端在公钥交换时顺序分配（零碰撞，见 allocate_ip_for_active）
/// wgvpn 虚拟网段（CIDR）。
const WGVPN_CIDR: &str = "100.99.71.0/24";
/// 网段前三个八位组（用于动态构造 IP 范围，如 "100.99.71.2"）。
const WGVPN_OCTET0: u8 = 100;
const WGVPN_OCTET1: u8 = 99;
const WGVPN_OCTET2: u8 = 71;
/// 旧 session 的被动端默认虚拟 IP；新会话按 passive device_id 稳定分配。
const PASSIVE_IP: &str = "100.99.71.1";
/// wg 接口名（多 peer 共用一个接口）。
const WG_TUNNEL_NAME: &str = "wg0";
// 首次启动 WireGuard 服务/接口以及对端 gonc 转发可能需要数秒。
// 15 秒在冷启动时过于激进，容易把尚未完成的握手误判为失败，
// 随后的重试反而会因为接口已就绪而成功。
const WIREGUARD_HANDSHAKE_TIMEOUT: Duration = Duration::from_secs(30);
/// wg0.conf 固定文件名（多 peer 模式下整个接口共用一份配置）。
const WG_CONF_NAME: &str = "wg0.conf";
/// WireGuard 本地监听端口。gonc 为每个 peer 创建独立的本地 UDP 转发端口。
const WGVPN_LISTEN_PORT: u16 = 41118;
const LEGACY_WGVPN_LISTEN_PORT: u16 = 51820;

fn peer_wg_port(advertised: u16) -> u16 {
    if advertised == 0 {
        LEGACY_WGVPN_LISTEN_PORT
    } else {
        advertised
    }
}

fn peer_health_port(advertised: u16) -> u16 {
    if advertised == 0 {
        LEGACY_HEALTH_PORT
    } else {
        advertised
    }
}

const fn uses_userspace_wg() -> bool {
    cfg!(any(windows, target_os = "macos"))
}

#[cfg(windows)]
fn skip_userspace_cleanup_on_legacy_windows() -> bool {
    use std::mem::size_of;
    use windows_sys::Win32::System::SystemInformation::{GetVersionExW, OSVERSIONINFOEXW};

    let mut version = OSVERSIONINFOEXW {
        dwOSVersionInfoSize: size_of::<OSVERSIONINFOEXW>() as u32,
        ..unsafe { std::mem::zeroed() }
    };
    let ok = unsafe { GetVersionExW(&mut version as *mut OSVERSIONINFOEXW as *mut _) } != 0;
    ok && version.dwMajorVersion == 6 && version.dwMinorVersion <= 1
}

/// wgvpn 会话句柄（停止时用）。扩展字段支持多 peer 按 pubkey 精确增删。
#[derive(Debug, Clone, Default)]
pub struct WgVpnSession {
    pub network: String,
    pub target_device_id: i64,
    pub tunnel_name: String, // WireGuard 隧道名（多 peer 模式下永远 "wg0"）
    pub virtual_ip: String,  // 本端虚拟 IP
    // ---- 多 peer 扩展字段 ----
    pub peer_device_id: i64,     // 对端 device_id
    pub peer_pubkey: String,     // 对端公钥（wg set remove 用）
    pub peer_virtual_ip: String, // 对端虚拟 IP
    /// 对端实际健康端口；旧版交换载荷缺失时为 48082。
    pub peer_health_port: u16,
    /// Cross-account passive sessions keep the WG peer alive while their
    /// AllowedIPs are temporarily empty and the UI waits for approval.
    pub approval_pending: bool,
    pub gonc_handle_id: String,  // gonc 库化 UDP tunnel handle
    pub local_forward_port: u16, // gonc 本地 UDP 转发端口
    pub is_active: bool,         // 本端角色（true=主动发起，false=被动等待）
    pub exposed_lan_cidrs: Vec<String>,
    pub advertised_lan_routes: Vec<String>,
    pub lan_mode: LanMode,
    pub subnet_router_handle_id: String,
    pub userspace_wg_peer_handle: String,
    pub userspace_wg: bool,
    pub subnet_router_started: bool,
    pub subnet_tcp_sessions: u32,
    pub subnet_udp_sessions: u32,
    pub subnet_wg_rx_packets: u64,
    pub subnet_wg_tx_packets: u64,
    pub subnet_icmp_success: u64,
    pub subnet_icmp_failed: u64,
    pub subnet_rejected_flows: u64,
    pub subnet_last_error: String,
}

fn wgvpn_audit_event(
    action: TunnelAuditAction,
    local_device_id: Option<i64>,
    session: &WgVpnSession,
) -> TunnelAuditEvent {
    TunnelAuditEvent {
        action,
        role: if session.is_active {
            TunnelAuditRole::Active
        } else {
            TunnelAuditRole::Passive
        },
        local_device_id,
        peer_device_id: session.peer_device_id,
        local_virtual_ip: session.virtual_ip.clone(),
        peer_virtual_ip: session.peer_virtual_ip.clone(),
        exposed_lan_cidrs: session.exposed_lan_cidrs.clone(),
    }
}

fn diagnostic_value(value: &str) -> &str {
    if value.is_empty() {
        "unknown"
    } else {
        value
    }
}

fn log_gonc_udp_punch_established(
    role: &str,
    peer_device_id: i64,
    elapsed: Duration,
    tunnel: &gonc_ffi::UdpTunnelResult,
) {
    tracing::info!(
        "[wgvpn] gonc punch established: role={}, peer_device_id={}, attempts={}, elapsed_ms={}, network={}, selected_traversal={}, transport_mode={}, traversal_role={}, local_forward_addr={}, local_forward_port={}, peer_endpoint={}, local_nat_type={}, remote_nat_type={}, local_lan_addr={}, local_nat_addr={}, remote_lan_addr={}, remote_nat_addr={}",
        role,
        peer_device_id,
        tunnel.attempts,
        elapsed.as_millis(),
        diagnostic_value(&tunnel.network),
        diagnostic_value(&tunnel.selected_traversal),
        diagnostic_value(&tunnel.transport_mode),
        if tunnel.is_client { "client" } else { "server" },
        tunnel.local_forward_addr,
        tunnel.local_forward_port,
        tunnel.peer_endpoint,
        tunnel.local_nat_type,
        tunnel.remote_nat_type,
        diagnostic_value(&tunnel.local_lan_addr),
        diagnostic_value(&tunnel.local_nat_addr),
        diagnostic_value(&tunnel.remote_lan_addr),
        diagnostic_value(&tunnel.remote_nat_addr),
    );
}

fn log_gonc_udp_punch_failed(
    role: &str,
    peer_device_id: i64,
    elapsed: Duration,
    err: &anyhow::Error,
) {
    let failure = err
        .downcast_ref::<gonc_ffi::UdpTunnelFailure>()
        .map(|failure| &failure.result);
    tracing::warn!(
        "[wgvpn] gonc punch failed: role={}, peer_device_id={}, attempts={}, elapsed_ms={}, network={}, selected_traversal={}, transport_mode={}, traversal_role={}, local_forward_addr={}, local_forward_port={}, peer_endpoint={}, local_nat_type={}, remote_nat_type={}, local_lan_addr={}, local_nat_addr={}, remote_lan_addr={}, remote_nat_addr={}, error={:#}",
        role,
        peer_device_id,
        failure.map_or(0, |result| result.attempts),
        elapsed.as_millis(),
        failure.map_or("unknown", |result| diagnostic_value(&result.network)),
        failure.map_or("unknown", |result| diagnostic_value(&result.selected_traversal)),
        failure.map_or("unknown", |result| diagnostic_value(&result.transport_mode)),
        failure.map_or("unknown", |result| if result.network.is_empty() { "unknown" } else if result.is_client { "client" } else { "server" }),
        failure.map_or("", |result| result.local_forward_addr.as_str()),
        failure.map_or(0, |result| result.local_forward_port),
        failure.map_or("", |result| result.peer_endpoint.as_str()),
        failure.map_or("unknown", |result| diagnostic_value(&result.local_nat_type)),
        failure.map_or("unknown", |result| diagnostic_value(&result.remote_nat_type)),
        failure.map_or("unknown", |result| diagnostic_value(&result.local_lan_addr)),
        failure.map_or("unknown", |result| diagnostic_value(&result.local_nat_addr)),
        failure.map_or("unknown", |result| diagnostic_value(&result.remote_lan_addr)),
        failure.map_or("unknown", |result| diagnostic_value(&result.remote_nat_addr)),
        err,
    );
}

/// 被动端为主动端分配虚拟 IP（被动端是 IP 分配的决策点）。
///
/// 分配策略（按优先级）：
/// 1. 同一 device_id 已有 session → 返回原 IP（重连稳定性）
/// 2. 顺序扫描范围 [ip_start, ip_end]，跳过已占用 IP，返回首个空闲
/// 3. 范围内无空闲 → 报错
///
/// 相比 hash 方案，此方案零碰撞（被动端唯一决策点，信息最全）。
fn allocate_ip_for_active(
    wg_cli: &str,
    active_device_id: i64,
    ip_start: u32,
    ip_end: u32,
    passive_ip: u32,
) -> Result<u32> {
    // 1. 重连稳定性：本进程内同一 device_id 已有 session → 拿回原 IP
    let sessions = WGVPN_SESSIONS.lock();
    if let Some(existing) = sessions
        .values()
        .find(|s| s.peer_device_id == active_device_id)
    {
        if let Ok(ip) = wgvpn_exchange::ipv4_to_u32(&existing.peer_virtual_ip) {
            if ip >= ip_start && ip <= ip_end {
                return Ok(ip);
            }
        }
    }

    // 2. 收集已占用的 IP（两个来源合并）：
    //    a) wg0 接口现有 peer 的 AllowedIPs（跨进程真实状态，最权威）
    //    b) 本进程 session 表（覆盖 wg0 还没生效的待加 peer）
    let mut used: HashSet<u32> = if uses_userspace_wg() {
        HashSet::new()
    } else {
        wgvpn::list_peer_allowed_ips(wg_cli, WG_TUNNEL_NAME)
            .into_iter()
            .filter_map(|cidr| {
                let ip_str = cidr.split('/').next()?;
                wgvpn_exchange::ipv4_to_u32(ip_str).ok()
            })
            .collect()
    };
    used.extend(
        sessions
            .values()
            .filter_map(|s| wgvpn_exchange::ipv4_to_u32(&s.peer_virtual_ip).ok()),
    );
    drop(sessions); // 释放锁
    used.extend(RESERVED_ACTIVE_IPS.lock().iter().copied());

    // 3. 顺序扫描范围内首个空闲 IP
    for ip in ip_start..=ip_end {
        // 跳过被动端自己固定占用的 .1（PASSIVE_IP）
        if ip == passive_ip {
            continue;
        }
        if !used.contains(&ip) {
            return Ok(ip);
        }
    }
    Err(anyhow!(
        "no available virtual IP in range {}..={}",
        wgvpn_exchange::u32_to_ipv4(ip_start),
        wgvpn_exchange::u32_to_ipv4(ip_end)
    ))
}

fn reserve_ip_for_active(
    wg_cli: &str,
    active_device_id: i64,
    ip_start: u32,
    ip_end: u32,
    passive_ip: u32,
) -> Result<u32> {
    let _allocation_guard = IP_ALLOCATION_LOCK.lock();
    let ip = allocate_ip_for_active(wg_cli, active_device_id, ip_start, ip_end, passive_ip)?;
    RESERVED_ACTIVE_IPS.lock().insert(ip);
    Ok(ip)
}

fn release_reserved_ip(ip: u32) {
    RESERVED_ACTIVE_IPS.lock().remove(&ip);
}

fn conflicting_session_ids(
    sessions: &[WgVpnSession],
    target_device_id: i64,
    peer_pubkey: &str,
) -> Vec<(i64, bool)> {
    sessions
        .iter()
        .filter(|session| {
            session.target_device_id == target_device_id || session.peer_pubkey == peer_pubkey
        })
        .map(|session| (session.target_device_id, session.peer_pubkey == peer_pubkey))
        .collect()
}

async fn stop_conflicting_sessions(
    config: &MachineConfig,
    target_device_id: i64,
    peer_pubkey: &str,
) -> Result<()> {
    let conflicting_ids =
        conflicting_session_ids(&snapshot_sessions(), target_device_id, peer_pubkey);
    for (conflicting_id, same_peer_key) in conflicting_ids {
        info!(
            "[wgvpn] replacing conflicting session before active start: old_peer_device_id={}, new_peer_device_id={}, same_peer_key={}",
            conflicting_id,
            target_device_id,
            same_peer_key
        );
        stop_wgvpn_internal(config, conflicting_id, false)
            .await
            .with_context(|| {
                format!(
                    "failed to stop conflicting wgvpn session for peer device {}",
                    conflicting_id
                )
            })?;
    }
    Ok(())
}

/// wgvpn 启动结果。
#[derive(Debug, Clone)]
pub struct WgVpnStartResult {
    pub success: bool,
    pub virtual_ip: String,
    pub peer_virtual_ip: String,
    pub peer_health_port: u16,
    pub network: String,
    pub local_nat_type: String,
    pub remote_nat_type: String,
    pub message: String,
    pub warning: Option<String>,
}

/// 主动端启动 wgvpn（设备A → 设备B）。
///
/// 完整流程（gonc 库化数据面 + WireGuard 内层 VPN，无中继）：
/// 1. 加载/生成本端 WireGuard 密钥
/// 2. 通过 gonc FFI Exchange 发送 (公钥, device_id, 可用 IP 范围)，接收被动端分配的 IP
/// 3. 通过 gonc FFI StartUdpTunnel 建立加密 UDP 数据面
/// 4. WireGuard peer endpoint 指向 gonc 本地 UDP 转发端口
/// 5. 等待 WireGuard 握手就绪
///
/// 多 Peer 模型：Windows 使用进程内用户态 WG；Linux 首次连接创建 wg0，
/// 后续连接动态追加 Peer。
/// 本端虚拟 IP 由被动端在公钥交换时分配（零碰撞）。
pub async fn start_active_wgvpn(
    config: &MachineConfig,
    target_device_id: i64,
    punch_token: String,
    lan_cidrs: Vec<String>,
) -> Result<WgVpnStartResult> {
    let _registration = crate::traversal::Registration(punch_token.clone());
    if !lan_cidrs.is_empty() {
        return Err(anyhow!("only passive wgvpn may expose LAN CIDRs"));
    }
    let wg_dir = wgvpn_dir();
    std::fs::create_dir_all(&wg_dir).context("failed to create wgvpn dir")?;

    let punch_lib = resolve_p2p_punch_lib(config);
    gonc_ffi::validate_punch_library_available(Path::new(&punch_lib))?;

    let my_device_id = config
        .device_id
        .ok_or_else(|| anyhow!("missing local device_id in MachineConfig"))?;

    // 1. 密钥
    let (priv_key, pub_key) = ensure_keypair(config, &wg_dir)?;
    let pubkey_path = wg_dir.join("pubkey.active");
    std::fs::write(&pubkey_path, &pub_key).context("failed to write pubkey.active")?;

    // 2. 公钥 + IP 协商（FFI Exchange，主动端 Mutual 模式：发范围，收分配结果）
    let ip_start = wgvpn_exchange::ipv4_to_u32(&format!(
        "{}.{}.{}.2",
        WGVPN_OCTET0, WGVPN_OCTET1, WGVPN_OCTET2
    ))?;
    let ip_end = wgvpn_exchange::ipv4_to_u32(&format!(
        "{}.{}.{}.254",
        WGVPN_OCTET0, WGVPN_OCTET1, WGVPN_OCTET2
    ))?;
    let wg_cli = resolve_wg_cli();
    let mut local_payload =
        wgvpn_exchange::ExchangePayload::for_active(&pub_key, my_device_id, ip_start, ip_end);
    local_payload.my_ip = choose_passive_ip_for_peer(&wg_cli, target_device_id, ip_start, ip_end)?;
    local_payload.wg_port = WGVPN_LISTEN_PORT;
    local_payload.health_port = HEALTH_PORT;
    let peer_payload =
        wgvpn_exchange::exchange_as_active(&punch_token, &local_payload, Duration::from_secs(60))
            .await?;
    let peer_exposed_lan_cidrs = peer_payload.exposed_lan_cidrs.clone();
    let warning = peer_payload.warning.clone();
    let peer_pubkey = peer_payload.pubkey;
    let peer_device_id = peer_payload.device_id;
    let peer_wg_port = peer_wg_port(peer_payload.wg_port);
    let peer_health_port = peer_health_port(peer_payload.health_port);
    tracing::info!(
        "[wgvpn-exchange] negotiated ports: role=active, local_wg_port={}, local_health_port={}, peer_wg_port={}, peer_health_port={}, peer_legacy={}",
        WGVPN_LISTEN_PORT,
        HEALTH_PORT,
        peer_wg_port,
        peer_health_port,
        peer_payload.wg_port == 0 || peer_payload.health_port == 0
    );
    let my_ip = wgvpn_exchange::u32_to_ipv4(peer_payload.assigned_ip);
    let peer_ip = wgvpn_exchange::u32_to_ipv4(peer_payload.my_ip);

    // 3. gonc 加密 UDP 数据面。WireGuard 只看到本机 UDP endpoint。
    // Do not log the punch token: it is a connection secret.
    let udp_tunnel_request = UdpTunnelRequest::wgvpn(&punch_token, "active", WGVPN_LISTEN_PORT);
    let traversal_session = crate::traversal::lookup(&punch_token);
    tracing::info!(
            "[wgvpn] gonc punch start: role=active, peer_device_id={}, timeout_secs={}, network={}, local_target={}:{}",
            target_device_id,
            udp_tunnel_request.timeout_secs,
            if traversal_session.is_some() { "negotiated" } else { udp_tunnel_request.network.as_str() },
            udp_tunnel_request.remote_target_ip,
            udp_tunnel_request.remote_target_port,
        );
    let udp_punch_started_at = Instant::now();
    let tunnel_result = match traversal_session {
        Some(session) => session.start(&udp_tunnel_request).await,
        None => gonc_ffi::start_udp_tunnel_native(&udp_tunnel_request).await,
    };
    let udp_tunnel = match tunnel_result {
        Ok(tunnel) => {
            log_gonc_udp_punch_established(
                "active",
                target_device_id,
                udp_punch_started_at.elapsed(),
                &tunnel,
            );
            tunnel
        }
        Err(err) => {
            log_gonc_udp_punch_failed(
                "active",
                target_device_id,
                udp_punch_started_at.elapsed(),
                &err,
            );
            return Err(err);
        }
    };

    // A re-registered/reinstalled peer can have a new device ID while keeping
    // the same WireGuard key. Keep the old session until the replacement UDP
    // data plane is ready, then replace it immediately before adding the peer.
    // Embedded WireGuard permits each public key only once per interface.
    if let Err(err) = stop_conflicting_sessions(config, target_device_id, &peer_pubkey).await {
        let _ = gonc_ffi::stop_udp_tunnel(&udp_tunnel.handle_id);
        return Err(err);
    }
    if let Err(err) = validate_new_lan_routes(&peer_exposed_lan_cidrs) {
        let _ = gonc_ffi::stop_udp_tunnel(&udp_tunnel.handle_id);
        return Err(err);
    }
    if let Err(err) = validate_peer_virtual_ip(target_device_id, &peer_ip) {
        let _ = gonc_ffi::stop_udp_tunnel(&udp_tunnel.handle_id);
        return Err(err);
    }

    // 4. 接口管理
    let conf_path = wg_dir.join(WG_CONF_NAME);
    let tunnel_existed = !uses_userspace_wg() && wgvpn::tunnel_exists(&wg_cli, WG_TUNNEL_NAME);
    if !uses_userspace_wg() && !tunnel_existed {
        if let Err(err) = ensure_wg_listen_port_available(WGVPN_LISTEN_PORT) {
            let _ = gonc_ffi::stop_udp_tunnel(&udp_tunnel.handle_id);
            return Err(err);
        }
    }
    let local_endpoint = format!("127.0.0.1:{}", udp_tunnel.local_forward_port);
    let mut peer = PeerConfig::active(&peer_pubkey, &local_endpoint, &peer_ip);
    peer.allowed_ips
        .extend(peer_exposed_lan_cidrs.iter().cloned());

    let mut userspace_wg_peer_handle = String::new();
    let handle_result = if uses_userspace_wg() {
        match gonc_ffi::start_userspace_wg_peer(
            Path::new(&punch_lib),
            &gonc_ffi::StartWindowsWgPeerRequest {
                session_id: target_device_id,
                peer_device_id,
                role: "active".to_string(),
                wg_private_key: priv_key.clone(),
                peer_public_key: peer_pubkey.clone(),
                local_tail_ip: my_ip.clone(),
                peer_tail_ip: peer_ip.clone(),
                peer_endpoint: local_endpoint.clone(),
                listen_ip: "127.0.0.1".to_string(),
                listen_port: WGVPN_LISTEN_PORT,
                routes: peer_exposed_lan_cidrs.clone(),
            },
        ) {
            Ok(result) => {
                let handle_id = result.handle_id;
                if let Err(err) = wait_for_windows_wg_peer(
                    Path::new(&punch_lib),
                    &handle_id,
                    WIREGUARD_HANDSHAKE_TIMEOUT,
                ) {
                    let _ = gonc_ffi::stop_userspace_wg_peer(Path::new(&punch_lib), &handle_id);
                    Err(err)
                } else {
                    userspace_wg_peer_handle = handle_id;
                    Ok(wgvpn::TunnelHandle {
                        tunnel_name: String::new(),
                        conf_path: conf_path.clone(),
                    })
                }
            }
            Err(err) => Err(err),
        }
    } else if !tunnel_existed {
        let conf = WgConfigBuilder::new()
            .private_key(&priv_key)
            .address(format!("{}/24", my_ip))
            .listen_port(WGVPN_LISTEN_PORT)
            .add_peer(peer)
            .build_active();
        if let Err(err) = conf.write_to(&conf_path) {
            Err(err)
        } else {
            start_tunnel_and_wait(&conf_path)
        }
    } else {
        if let Err(err) = wgvpn::add_peer(&wg_cli, WG_TUNNEL_NAME, &peer) {
            Err(err)
        } else {
            let min_epoch = current_epoch_secs();
            let wait_result = wait_for_handshake_with_peer(
                &wg_cli,
                WG_TUNNEL_NAME,
                &peer_pubkey,
                WIREGUARD_HANDSHAKE_TIMEOUT,
                min_epoch,
            );
            if let Err(err) = wait_result {
                let _ = wgvpn::remove_peer(&wg_cli, WG_TUNNEL_NAME, &peer_pubkey);
                Err(err)
            } else {
                Ok(wgvpn::TunnelHandle {
                    tunnel_name: WG_TUNNEL_NAME.to_string(),
                    conf_path: conf_path.clone(),
                })
            }
        }
    };
    let handle = match handle_result {
        Ok(handle) => handle,
        Err(err) => {
            tracing::error!(
                "[wgvpn] WireGuard setup failed: role=active, peer_device_id={}, userspace_wg={}, tunnel_existed={}, endpoint={}, error={:#}",
                target_device_id,
                uses_userspace_wg(),
                tunnel_existed,
                local_endpoint,
                err
            );
            let _ = gonc_ffi::stop_udp_tunnel(&udp_tunnel.handle_id);
            return Err(err);
        }
    };

    let session = WgVpnSession {
        target_device_id,
        tunnel_name: handle.tunnel_name.clone(),
        virtual_ip: my_ip.clone(),
        peer_device_id,
        peer_pubkey: peer_pubkey.clone(),
        peer_virtual_ip: peer_ip.clone(),
        peer_health_port,
        gonc_handle_id: udp_tunnel.handle_id.clone(),
        network: udp_tunnel.network.clone(),
        local_forward_port: udp_tunnel.local_forward_port,
        is_active: true,
        exposed_lan_cidrs: peer_exposed_lan_cidrs.clone(),
        advertised_lan_routes: peer_exposed_lan_cidrs,
        lan_mode: subnet_router::desired_lan_mode(&peer_payload.exposed_lan_cidrs),
        userspace_wg_peer_handle: userspace_wg_peer_handle.clone(),
        userspace_wg: uses_userspace_wg(),
        ..Default::default()
    };
    let audit_event = wgvpn_audit_event(TunnelAuditAction::Established, config.device_id, &session);
    WGVPN_SESSIONS.lock().insert(target_device_id, session);
    record_tunnel_audit(audit_event);

    Ok(WgVpnStartResult {
        success: true,
        virtual_ip: my_ip,
        peer_virtual_ip: peer_ip,
        peer_health_port,
        network: udp_tunnel.network.clone(),
        local_nat_type: udp_tunnel.local_nat_type,
        remote_nat_type: udp_tunnel.remote_nat_type,
        message: format!(
            "wgvpn established via gonc tunnel {} on {}",
            udp_tunnel.handle_id, udp_tunnel.local_forward_addr
        ),
        warning,
    })
}

/// 被动端启动 wgvpn（被主动端连接）。
///
/// 多 peer 模型：本端 IP 固定 PASSIVE_IP（被连接方），对端 IP 由本端顺序分配（零碰撞）。
/// 被多个主动端连接时，每个主动端一个 [Peer]（用打洞得到的 endpoint 直连）。
pub async fn start_passive_wgvpn(
    config: &MachineConfig,
    source_device_id: i64,
    punch_token: String,
    lan_cidrs: Vec<String>,
) -> Result<WgVpnStartResult> {
    let _registration = crate::traversal::Registration(punch_token.clone());
    let requested_lan_cidrs = wgvpn_exchange::normalize_exposed_lan_cidrs(&lan_cidrs)?;
    validate_lan_routes_basic(&requested_lan_cidrs)?;
    // Each passive peer has a distinct virtual source IP, so multiple
    // active clients can safely share this device's local LAN routes.
    // Route overlap remains rejected on the active side, where two remote
    // gateways advertising the same CIDR would be ambiguous.
    let passive_exposed_lan_cidrs = requested_lan_cidrs;
    let warning = None;
    let wg_dir = wgvpn_dir();
    std::fs::create_dir_all(&wg_dir).context("failed to create wgvpn dir")?;

    let punch_lib = resolve_p2p_punch_lib(config);
    gonc_ffi::validate_punch_library_available(Path::new(&punch_lib))?;

    let (priv_key, pub_key) = ensure_keypair(config, &wg_dir)?;
    let pubkey_path = wg_dir.join("pubkey.passive");
    std::fs::write(&pubkey_path, &pub_key).context("failed to write pubkey.passive")?;

    let wg_cli = resolve_wg_cli();
    let mut reserved_peer_ip: Option<u32> = None;
    let local_payload_template = wgvpn_exchange::ExchangePayload {
        pubkey: pub_key.clone(),
        device_id: config.device_id.unwrap_or(0),
        wg_port: WGVPN_LISTEN_PORT,
        health_port: HEALTH_PORT,
        exposed_lan_cidrs: passive_exposed_lan_cidrs.clone(),
        warning: warning.clone(),
        ..Default::default()
    };

    // 1. 公钥 + IP 协商（FFI Exchange，被动端：收主动端范围 → 分配 → 回传）
    let active_payload = wgvpn_exchange::exchange_as_passive(
        &punch_token,
        &local_payload_template,
        Duration::from_secs(120),
        |active_device_id, ip_start, ip_end, requested_ip| {
            let my_ip_u32 = passive_ip_for_request(
                config.device_id.unwrap_or(0),
                ip_start,
                ip_end,
                requested_ip,
            )?;
            // 为主动端分配 IP（查 wg0 真实状态 + 本进程 session，零碰撞）
            let assigned =
                reserve_ip_for_active(&wg_cli, active_device_id, ip_start, ip_end, my_ip_u32)?;
            reserved_peer_ip = Some(assigned);
            Ok((assigned, my_ip_u32))
        },
    )
    .await?;
    let peer_pubkey = active_payload.pubkey;
    let peer_device_id = active_payload.device_id;
    let peer_wg_port = peer_wg_port(active_payload.wg_port);
    let peer_health_port = peer_health_port(active_payload.health_port);
    tracing::info!(
        "[wgvpn-exchange] negotiated ports: role=passive, local_wg_port={}, local_health_port={}, peer_wg_port={}, peer_health_port={}, peer_legacy={}",
        WGVPN_LISTEN_PORT,
        HEALTH_PORT,
        peer_wg_port,
        peer_health_port,
        active_payload.wg_port == 0 || active_payload.health_port == 0
    );
    // 被动端固定 PASSIVE_IP；对端（主动端）IP = 本端分配的结果（复算保持一致）
    let my_ip = wgvpn_exchange::u32_to_ipv4(
        active_payload
            .my_ip
            .max(wgvpn_exchange::ipv4_to_u32(PASSIVE_IP).unwrap_or(0)),
    );
    let reserved_peer_ip = reserved_peer_ip
        .ok_or_else(|| anyhow!("passive ip allocation did not reserve a peer ip"))?;
    let peer_ip = wgvpn_exchange::u32_to_ipv4(reserved_peer_ip);

    // 2. gonc 加密 UDP 数据面。Do not log the punch token.
    let udp_tunnel_request = UdpTunnelRequest::wgvpn(&punch_token, "passive", WGVPN_LISTEN_PORT);
    let traversal_session = crate::traversal::lookup(&punch_token);
    tracing::info!(
            "[wgvpn] gonc punch start: role=passive, peer_device_id={}, timeout_secs={}, network={}, local_target={}:{}",
            source_device_id,
            udp_tunnel_request.timeout_secs,
            if traversal_session.is_some() { "negotiated" } else { udp_tunnel_request.network.as_str() },
            udp_tunnel_request.remote_target_ip,
            udp_tunnel_request.remote_target_port,
        );
    let udp_punch_started_at = Instant::now();
    let tunnel_result = match traversal_session {
        Some(session) => session.start(&udp_tunnel_request).await,
        None => gonc_ffi::start_udp_tunnel_native(&udp_tunnel_request).await,
    };
    let udp_tunnel = match tunnel_result {
        Ok(tunnel) => {
            log_gonc_udp_punch_established(
                "passive",
                source_device_id,
                udp_punch_started_at.elapsed(),
                &tunnel,
            );
            tunnel
        }
        Err(err) => {
            log_gonc_udp_punch_failed(
                "passive",
                source_device_id,
                udp_punch_started_at.elapsed(),
                &err,
            );
            release_reserved_ip(reserved_peer_ip);
            return Err(err);
        }
    };

    // 3. Windows/macOS use the single-process userspace WG engine; Linux
    // keeps its existing kernel/wireguard-go backend.
    let conf_path = wg_dir.join(WG_CONF_NAME);
    let use_userspace_router = uses_userspace_wg();
    let tunnel_existed = !use_userspace_router && wgvpn::tunnel_exists(&wg_cli, WG_TUNNEL_NAME);
    if !use_userspace_router && !tunnel_existed {
        if let Err(err) = ensure_wg_listen_port_available(WGVPN_LISTEN_PORT) {
            release_reserved_ip(reserved_peer_ip);
            let _ = gonc_ffi::stop_udp_tunnel(&udp_tunnel.handle_id);
            return Err(err);
        }
    }
    let local_endpoint = format!("127.0.0.1:{}", udp_tunnel.local_forward_port);
    let peer = PeerConfig::active(&peer_pubkey, &local_endpoint, &peer_ip);

    let mut userspace_result = None;
    let handle_result = if use_userspace_router {
        match gonc_ffi::start_userspace_wg_peer(
            Path::new(&punch_lib),
            &gonc_ffi::StartWindowsWgPeerRequest {
                session_id: source_device_id,
                peer_device_id,
                role: "passive".to_string(),
                wg_private_key: priv_key.clone(),
                peer_public_key: peer_pubkey.clone(),
                local_tail_ip: my_ip.clone(),
                peer_tail_ip: peer_ip.clone(),
                peer_endpoint: local_endpoint.clone(),
                listen_ip: "127.0.0.1".to_string(),
                listen_port: WGVPN_LISTEN_PORT,
                routes: passive_exposed_lan_cidrs.clone(),
            },
        ) {
            Ok(result) => {
                if let Err(err) = wait_for_windows_wg_peer(
                    Path::new(&punch_lib),
                    &result.handle_id,
                    WIREGUARD_HANDSHAKE_TIMEOUT,
                ) {
                    let _ =
                        gonc_ffi::stop_userspace_wg_peer(Path::new(&punch_lib), &result.handle_id);
                    Err(err)
                } else {
                    userspace_result = Some(result);
                    Ok(wgvpn::TunnelHandle {
                        tunnel_name: String::new(),
                        conf_path: conf_path.clone(),
                    })
                }
            }
            Err(err) => Err(err),
        }
    } else if !tunnel_existed {
        let conf = WgConfigBuilder::new()
            .private_key(&priv_key)
            .address(format!("{}/24", my_ip))
            .listen_port(WGVPN_LISTEN_PORT)
            .add_peer(peer)
            .build_active();
        if let Err(err) = conf.write_to(&conf_path) {
            Err(err)
        } else {
            start_tunnel_and_wait(&conf_path)
        }
    } else {
        if let Err(err) = wgvpn::add_peer(&wg_cli, WG_TUNNEL_NAME, &peer) {
            Err(err)
        } else {
            let min_epoch = current_epoch_secs();
            let wait_result = wait_for_handshake_with_peer(
                &wg_cli,
                WG_TUNNEL_NAME,
                &peer_pubkey,
                WIREGUARD_HANDSHAKE_TIMEOUT,
                min_epoch,
            );
            if let Err(err) = wait_result {
                let _ = wgvpn::remove_peer(&wg_cli, WG_TUNNEL_NAME, &peer_pubkey);
                Err(err)
            } else {
                Ok(wgvpn::TunnelHandle {
                    tunnel_name: WG_TUNNEL_NAME.to_string(),
                    conf_path: conf_path.clone(),
                })
            }
        }
    };
    let handle = match handle_result {
        Ok(handle) => handle,
        Err(err) => {
            tracing::error!(
                "[wgvpn] WireGuard setup failed: role=passive, peer_device_id={}, userspace_wg={}, tunnel_existed={}, endpoint={}, error={:#}",
                source_device_id,
                use_userspace_router,
                tunnel_existed,
                local_endpoint,
                err
            );
            release_reserved_ip(reserved_peer_ip);
            let _ = gonc_ffi::stop_udp_tunnel(&udp_tunnel.handle_id);
            return Err(err);
        }
    };

    let mut router = None;
    if !use_userspace_router && !passive_exposed_lan_cidrs.is_empty() {
        let started = gonc_ffi::start_subnet_router(&gonc_ffi::StartSubnetRouterRequest {
            session_id: source_device_id,
            peer_device_id,
            wg_private_key: priv_key.clone(),
            peer_public_key: peer_pubkey.clone(),
            tail_ip: my_ip.clone(),
            peer_tail_ip: peer_ip.clone(),
            peer_endpoint: local_endpoint.clone(),
            listen_ip: "127.0.0.1".to_string(),
            listen_port: WGVPN_LISTEN_PORT,
            exposed_lan_cidrs: passive_exposed_lan_cidrs.clone(),
            snat: true,
            allow_tcp: true,
            allow_udp: true,
            allow_icmp_echo: true,
        });
        let started = match started {
            Ok(value) => value,
            Err(err) => {
                if !handle.tunnel_name.is_empty() {
                    let _ = wgvpn::remove_peer(&wg_cli, &handle.tunnel_name, &peer_pubkey);
                }
                if !tunnel_existed && !handle.tunnel_name.is_empty() {
                    let _ = wgvpn::stop_tunnel(&resolve_wireguard_exe(), &handle.tunnel_name);
                }
                release_reserved_ip(reserved_peer_ip);
                let _ = gonc_ffi::stop_udp_tunnel(&udp_tunnel.handle_id);
                return Err(err);
            }
        };
        router = Some(started);
    }

    let session = WgVpnSession {
        target_device_id: source_device_id,
        tunnel_name: handle.tunnel_name.clone(),
        virtual_ip: my_ip.clone(),
        peer_device_id,
        peer_pubkey: peer_pubkey.clone(),
        peer_virtual_ip: peer_ip.clone(),
        peer_health_port,
        gonc_handle_id: udp_tunnel.handle_id.clone(),
        network: udp_tunnel.network.clone(),
        local_forward_port: udp_tunnel.local_forward_port,
        is_active: false,
        exposed_lan_cidrs: passive_exposed_lan_cidrs.clone(),
        advertised_lan_routes: passive_exposed_lan_cidrs.clone(),
        lan_mode: if userspace_result.is_some() && !passive_exposed_lan_cidrs.is_empty() {
            LanMode::UserspaceSnat
        } else {
            router
                .as_ref()
                .and_then(|r| subnet_router::lan_mode_from_backend_label(&r.lan_mode))
                .unwrap_or(LanMode::Disabled)
        },
        userspace_wg_peer_handle: userspace_result
            .as_ref()
            .map(|r| r.handle_id.clone())
            .unwrap_or_default(),
        userspace_wg: userspace_result.is_some(),
        subnet_router_handle_id: router
            .as_ref()
            .map(|r| r.handle_id.clone())
            .unwrap_or_default(),
        subnet_router_started: router.as_ref().is_some_and(|r| r.started),
        subnet_tcp_sessions: userspace_result.as_ref().map_or_else(
            || router.as_ref().map_or(0, |r| r.tcp_sessions),
            |r| r.tcp_sessions,
        ),
        subnet_udp_sessions: userspace_result.as_ref().map_or_else(
            || router.as_ref().map_or(0, |r| r.udp_sessions),
            |r| r.udp_sessions,
        ),
        subnet_wg_rx_packets: userspace_result.as_ref().map_or_else(
            || router.as_ref().map_or(0, |r| r.wg_rx_packets),
            |r| r.rx_packets.max(0) as u64,
        ),
        subnet_wg_tx_packets: userspace_result.as_ref().map_or_else(
            || router.as_ref().map_or(0, |r| r.wg_tx_packets),
            |r| r.tx_packets.max(0) as u64,
        ),
        subnet_icmp_success: userspace_result.as_ref().map_or_else(
            || router.as_ref().map_or(0, |r| r.icmp_success),
            |r| r.icmp_success.max(0) as u64,
        ),
        subnet_icmp_failed: userspace_result.as_ref().map_or_else(
            || router.as_ref().map_or(0, |r| r.icmp_failed),
            |r| r.icmp_failed.max(0) as u64,
        ),
        subnet_rejected_flows: userspace_result.as_ref().map_or_else(
            || router.as_ref().map_or(0, |r| r.rejected_flows),
            |r| r.rejected_flows.max(0) as u64,
        ),
        subnet_last_error: router
            .as_ref()
            .map(|r| r.last_error.clone())
            .unwrap_or_else(|| {
                userspace_result
                    .as_ref()
                    .map(|r| r.last_error.clone())
                    .unwrap_or_default()
            }),
        ..Default::default()
    };
    let audit_event = wgvpn_audit_event(TunnelAuditAction::Established, config.device_id, &session);
    WGVPN_SESSIONS.lock().insert(source_device_id, session);
    release_reserved_ip(reserved_peer_ip);
    record_tunnel_audit(audit_event);

    Ok(WgVpnStartResult {
        success: true,
        virtual_ip: my_ip,
        peer_virtual_ip: peer_ip,
        peer_health_port,
        network: udp_tunnel.network.clone(),
        local_nat_type: udp_tunnel.local_nat_type,
        remote_nat_type: udp_tunnel.remote_nat_type,
        message: format!(
            "wgvpn established via gonc tunnel {} on {}",
            udp_tunnel.handle_id, udp_tunnel.local_forward_addr
        ),
        warning,
    })
}

/// 停止指定会话。
pub async fn stop_wgvpn(config: &MachineConfig, target_device_id: i64) -> Result<()> {
    stop_wgvpn_internal(config, target_device_id, true).await
}

async fn stop_wgvpn_internal(
    config: &MachineConfig,
    target_device_id: i64,
    notify_remote: bool,
) -> Result<()> {
    let Some(session) = WGVPN_SESSIONS.lock().remove(&target_device_id) else {
        return Ok(());
    };

    // 主动端断开时，经已建立的虚拟网卡通知被动端同步清理会话。
    // 被动端执行 stop 时 is_active=false，不会反向通知，避免停止消息循环。
    if notify_remote && session.is_active && !session.peer_virtual_ip.is_empty() {
        if let Some(source_device_id) = config.device_id {
            let health_addr = format!("{}:{}", session.peer_virtual_ip, session.peer_health_port);
            if let Err(err) = notify_remote_tunnel_stop(source_device_id, &health_addr).await {
                warn!(
                        "[wgvpn] remote stop notice failed: peer_device_id={}, health_addr={}, error={:#}",
                        session.peer_device_id, health_addr, err
                    );
            }
        }
    }

    let wg_cli = resolve_wg_cli();
    let wireguard_exe = resolve_wireguard_exe();
    let punch_lib = resolve_p2p_punch_lib(config);
    let mut cleanup_errors = Vec::new();

    // 1. 精确删除当前 Peer，不影响同引擎的其他主动/被动会话。
    if !session.userspace_wg_peer_handle.is_empty() {
        if let Err(e) = gonc_ffi::stop_userspace_wg_peer(
            Path::new(&punch_lib),
            &session.userspace_wg_peer_handle,
        ) {
            warn!("[wgvpn] stop userspace WG peer failed: {:#}", e);
            cleanup_errors.push(format!("stop userspace WG peer: {e:#}"));
        }
    } else if !session.tunnel_name.is_empty() {
        if let Err(e) = wgvpn::remove_peer(&wg_cli, &session.tunnel_name, &session.peer_pubkey) {
            warn!(
                "[wgvpn] remove peer failed (will continue cleanup): {:#}",
                e
            );
            cleanup_errors.push(format!("remove WireGuard peer: {e:#}"));
        }
    }

    if let Err(e) = gonc_ffi::stop_subnet_router(&session.subnet_router_handle_id) {
        warn!("[wgvpn] stop subnet router failed: {:#}", e);
        cleanup_errors.push(format!("stop subnet router: {e:#}"));
    }

    // 2. 停止该 peer 对应的 gonc UDP 数据面
    if let Err(e) = gonc_ffi::stop_udp_tunnel(&session.gonc_handle_id) {
        warn!("[wgvpn] stop gonc tunnel failed: {:#}", e);
        cleanup_errors.push(format!("stop gonc UDP tunnel: {e:#}"));
    }

    // 3. 判断是否还要保留 wg0 接口。系统接口的 peer 数与本 service
    //    内存 session 表共同保护仍在运行的多 peer 会话。
    let real_peer_count = if session.userspace_wg || session.tunnel_name.is_empty() {
        0
    } else {
        wgvpn::peer_count(&wg_cli, &session.tunnel_name)
    };
    let local_remaining = WGVPN_SESSIONS.lock().len();
    if !session.userspace_wg
        && !session.tunnel_name.is_empty()
        && real_peer_count == 0
        && local_remaining == 0
    {
        // 真的无 peer 了，卸载整个 wg0 接口
        if let Err(e) = wgvpn::stop_tunnel(&wireguard_exe, &session.tunnel_name) {
            warn!("[wgvpn] stop tunnel failed: {:#}", e);
            cleanup_errors.push(format!("stop WireGuard tunnel: {e:#}"));
        }
        // 清理 conf 文件
        let conf_path = wgvpn_dir().join(WG_CONF_NAME);
        if let Err(e) = std::fs::remove_file(&conf_path) {
            if e.kind() != std::io::ErrorKind::NotFound {
                warn!("[wgvpn] remove tunnel config failed: {:#}", e);
                cleanup_errors.push(format!("remove tunnel config {}: {e}", conf_path.display()));
            }
        }
        info!(
            "[wgvpn] last peer removed, tunnel {} uninstalled",
            session.tunnel_name
        );
    } else {
        info!(
            "[wgvpn] peer removed, {} peer(s) on {} (local sessions: {})",
            real_peer_count, session.tunnel_name, local_remaining
        );
    }
    ensure_wgvpn_cleanup_complete(&cleanup_errors)?;
    record_tunnel_audit(wgvpn_audit_event(
        TunnelAuditAction::Disconnected,
        config.device_id,
        &session,
    ));
    Ok(())
}

/// Toggle the control-plane AllowedIPs gate for one passive session. This
/// deliberately performs no per-packet work: WireGuard keeps the peer and
/// handshake alive while an empty AllowedIPs list prevents normal IP traffic.
pub async fn set_wgvpn_allowed(
    config: &MachineConfig,
    target_device_id: i64,
    allowed: bool,
) -> Result<()> {
    let session = WGVPN_SESSIONS
        .lock()
        .get(&target_device_id)
        .cloned()
        .ok_or_else(|| anyhow!("wgvpn session not found"))?;
    if session.is_active {
        return Err(anyhow!(
            "AllowedIPs approval gate is only valid for passive sessions"
        ));
    }
    let mut allowed_ips = if allowed {
        vec![format!("{}/32", session.peer_virtual_ip)]
    } else {
        Vec::new()
    };
    if allowed && !session.userspace_wg {
        // Kernel-WG LAN forwarding is owned by the subnet-router backend;
        // only the peer tail address belongs in the WG peer's AllowedIPs.
        allowed_ips.truncate(1);
    }
    if !session.userspace_wg_peer_handle.is_empty() {
        let punch_lib = resolve_p2p_punch_lib(config);
        gonc_ffi::set_userspace_wg_peer_allowed(
            Path::new(&punch_lib),
            &session.userspace_wg_peer_handle,
            allowed,
        )?;
    } else {
        let wg_cli = resolve_wg_cli();
        wgvpn::set_peer_allowed_ips(
            &wg_cli,
            &session.tunnel_name,
            &session.peer_pubkey,
            &allowed_ips,
        )?;
    }
    WGVPN_SESSIONS
        .lock()
        .get_mut(&target_device_id)
        .map(|current| current.approval_pending = !allowed);
    Ok(())
}

fn ensure_wgvpn_cleanup_complete(cleanup_errors: &[String]) -> Result<()> {
    if cleanup_errors.is_empty() {
        return Ok(());
    }
    Err(anyhow!(
        "wgvpn cleanup incomplete: {}",
        cleanup_errors.join("; ")
    ))
}

// ============ 内部辅助 ============

static WGVPN_SESSIONS: once_cell::sync::Lazy<Mutex<HashMap<i64, WgVpnSession>>> =
    once_cell::sync::Lazy::new(|| Mutex::new(HashMap::new()));
static RESERVED_ACTIVE_IPS: once_cell::sync::Lazy<Mutex<HashSet<u32>>> =
    once_cell::sync::Lazy::new(|| Mutex::new(HashSet::new()));
static IP_ALLOCATION_LOCK: once_cell::sync::Lazy<Mutex<()>> =
    once_cell::sync::Lazy::new(|| Mutex::new(()));
fn wgvpn_dir() -> PathBuf {
    install_data_dir().join("wgvpn")
}

/// 返回所有 wgvpn 会话的快照（供 service runtime 推送给 UI）。
pub fn snapshot_sessions() -> Vec<WgVpnSession> {
    WGVPN_SESSIONS.lock().values().cloned().collect()
}

/// service 启动时清理上次进程异常退出留下的 WGVPN 资源。
///
/// gonc UDP 数据面和 Windows userspace WireGuard 都属于进程内状态，重启后
/// 无法恢复；新 service 生命周期只从空状态开始。
pub fn cleanup_stale_sessions(config: &MachineConfig) -> Result<()> {
    // run_service_foreground may be started again in the same process (tests,
    // embedded/service lifecycle changes). A service start is a hard tunnel
    // boundary: process-local sessions and IP reservations must never survive it.
    WGVPN_SESSIONS.lock().clear();
    RESERVED_ACTIVE_IPS.lock().clear();

    if uses_userspace_wg() {
        let punch_lib = resolve_p2p_punch_lib(config);
        #[cfg(windows)]
        if skip_userspace_cleanup_on_legacy_windows() {
            // The legacy Go/GVisor DLL currently terminates the Win7 process
            // inside CleanupUserspaceWgPlatform before returning across FFI.
            // This cleanup is optional because all state is process-local; skip
            // it on Win7 so the service can start while DLL compatibility work
            // remains isolated and explicitly unverified.
        } else {
            gonc_ffi::cleanup_userspace_wg_platform(Path::new(&punch_lib))
                .context("failed to clear stale userspace WireGuard state")?;
        }
        #[cfg(not(windows))]
        gonc_ffi::cleanup_userspace_wg_platform(Path::new(&punch_lib))
            .context("failed to clear stale userspace WireGuard state")?;
    }
    let wg_cli = resolve_wg_cli();
    let wireguard_exe = resolve_wireguard_exe();
    if !uses_userspace_wg() {
        if !Path::new(&wg_cli).exists() || !Path::new(&wireguard_exe).exists() {
            return Err(anyhow!(
                "cannot verify stale wgvpn cleanup: required tools are missing"
            ));
        }
        // wg0 is owned by this service. A new service lifecycle never adopts
        // an existing interface, even if it contains untracked peers.
        if wgvpn::tunnel_exists(&wg_cli, WG_TUNNEL_NAME) {
            wgvpn::stop_tunnel(&wireguard_exe, WG_TUNNEL_NAME)
                .context("failed to stop stale wg0 interface")?;
        }
    }
    let conf_path = wgvpn_dir().join(WG_CONF_NAME);
    if let Err(err) = std::fs::remove_file(&conf_path) {
        if err.kind() != std::io::ErrorKind::NotFound {
            return Err(err).with_context(|| {
                format!(
                    "failed to remove stale wgvpn config: {}",
                    conf_path.display()
                )
            });
        }
    }
    Ok(())
}
fn resolve_wireguard_exe() -> String {
    crate::config::default_wireguard_path()
        .to_string_lossy()
        .to_string()
}

/// 解析 Linux 内核后端使用的 WireGuard CLI 路径。
fn resolve_wg_cli() -> String {
    crate::config::default_wg_path()
        .to_string_lossy()
        .to_string()
}

fn resolve_p2p_punch_lib(config: &MachineConfig) -> String {
    if config.p2p_punch_path.is_empty() {
        crate::config::default_p2p_punch_path()
            .to_string_lossy()
            .to_string()
    } else {
        config.p2p_punch_path.clone()
    }
}

fn ensure_wg_listen_port_available(port: u16) -> Result<()> {
    // Best-effort preflight only. The real WireGuard/subnet bind remains authoritative.
    let addr = format!("127.0.0.1:{}", port);
    let socket = std::net::UdpSocket::bind(&addr)
        .with_context(|| format!("wireguard listen port is unavailable: {}", addr))?;
    drop(socket);
    Ok(())
}

fn validate_new_lan_routes(routes: &[String]) -> Result<()> {
    validate_lan_routes_basic(routes)?;
    let existing: Vec<String> = WGVPN_SESSIONS
        .lock()
        .values()
        .flat_map(|session| session.exposed_lan_cidrs.clone())
        .collect();
    for route in routes {
        let candidate = ipv4_cidr_range(route)?;
        for active in &existing {
            let current = ipv4_cidr_range(active)?;
            if candidate.0 <= current.1 && current.0 <= candidate.1 {
                return Err(anyhow!(
                    "LAN route {} overlaps active route {}",
                    route,
                    active
                ));
            }
        }
    }
    Ok(())
}

fn validate_lan_routes_basic(routes: &[String]) -> Result<()> {
    for route in routes {
        let candidate = ipv4_cidr_range(route)?;
        if candidate.0 <= 0x7fff_ffff && candidate.1 >= 0x7f00_0000 {
            return Err(anyhow!("LAN route overlaps loopback: {}", route));
        }
        let wgvpn = ipv4_cidr_range(WGVPN_CIDR)?;
        if candidate.0 <= wgvpn.1 && wgvpn.0 <= candidate.1 {
            return Err(anyhow!("LAN route overlaps wgvpn network: {}", route));
        }
    }
    Ok(())
}

fn validate_peer_virtual_ip(target_device_id: i64, peer_ip: &str) -> Result<()> {
    if WGVPN_SESSIONS
        .lock()
        .iter()
        .any(|(id, session)| *id != target_device_id && session.peer_virtual_ip == peer_ip)
    {
        return Err(anyhow!(
            "peer virtual IP {} is already used by another session",
            peer_ip
        ));
    }
    Ok(())
}

fn ipv4_cidr_range(cidr: &str) -> Result<(u32, u32)> {
    let (ip, bits) = crate::wgvpn_exchange::parse_ipv4_cidr(cidr)?;
    let mask = crate::wgvpn_exchange::ipv4_mask_from_prefix(bits);
    Ok((ip & mask, (ip & mask) | !mask))
}

fn passive_ip_for_device(device_id: i64, ip_start: u32, ip_end: u32) -> Result<u32> {
    if ip_start > ip_end {
        return Err(anyhow!("invalid wgvpn ip range"));
    }
    let first = ip_start.saturating_sub(1);
    let count = u64::from(ip_end - first);
    if count == 0 {
        return Err(anyhow!("empty wgvpn passive ip range"));
    }
    Ok(first + 1 + (device_id.unsigned_abs() % count) as u32)
}

fn passive_ip_for_request(
    device_id: i64,
    ip_start: u32,
    ip_end: u32,
    requested_ip: u32,
) -> Result<u32> {
    if requested_ip >= ip_start && requested_ip <= ip_end {
        return Ok(requested_ip);
    }
    passive_ip_for_device(device_id, ip_start, ip_end)
}

fn choose_passive_ip_for_peer(
    wg_cli: &str,
    target_device_id: i64,
    ip_start: u32,
    ip_end: u32,
) -> Result<u32> {
    let preferred = passive_ip_for_device(target_device_id, ip_start, ip_end)?;
    let used = used_peer_virtual_ips(wg_cli);
    if !used.contains(&preferred) {
        return Ok(preferred);
    }
    for ip in ip_start..=ip_end {
        if !used.contains(&ip) {
            return Ok(ip);
        }
    }
    Err(anyhow!(
        "no available passive virtual IP in range {}..={}",
        wgvpn_exchange::u32_to_ipv4(ip_start),
        wgvpn_exchange::u32_to_ipv4(ip_end)
    ))
}

fn used_peer_virtual_ips(wg_cli: &str) -> HashSet<u32> {
    let mut used: HashSet<u32> = if uses_userspace_wg() {
        HashSet::new()
    } else {
        wgvpn::list_peer_allowed_ips(wg_cli, WG_TUNNEL_NAME)
            .into_iter()
            .filter_map(|cidr| {
                let (ip, bits) = cidr.split_once('/')?;
                if bits != "32" {
                    return None;
                }
                let value = wgvpn_exchange::ipv4_to_u32(ip).ok()?;
                let (start, end) = ipv4_cidr_range(WGVPN_CIDR).ok()?;
                (value >= start && value <= end).then_some(value)
            })
            .collect()
    };
    used.extend(
        WGVPN_SESSIONS
            .lock()
            .values()
            .filter_map(|session| wgvpn_exchange::ipv4_to_u32(&session.peer_virtual_ip).ok()),
    );
    used
}

fn wait_for_windows_wg_peer(library: &Path, handle_id: &str, timeout: Duration) -> Result<()> {
    let started = std::time::Instant::now();
    while started.elapsed() <= timeout {
        let status = match gonc_ffi::get_userspace_wg_peer_status(library, handle_id) {
            Ok(status) => status,
            Err(err) => {
                tracing::error!(
                    "[wgvpn] GetUserspaceWgPeerStatus failed: handle_id={}, error={:#}",
                    handle_id,
                    err
                );
                return Err(err);
            }
        };
        if !status.error.is_empty() || !status.last_error.is_empty() {
            tracing::warn!(
                "[wgvpn] userspace WireGuard status reports error: handle_id={}, error={}, last_error={}",
                handle_id,
                status.error,
                status.last_error
            );
        }
        if status.started && status.last_handshake_at > 0 {
            info!(
                "[wgvpn] userspace WireGuard handshake established for {} after {:?}",
                handle_id,
                started.elapsed()
            );
            return Ok(());
        }
        std::thread::sleep(Duration::from_millis(250));
    }
    Err(anyhow!(
        "userspace WireGuard handshake timeout after {:?} for {}",
        timeout,
        handle_id
    ))
}

/// 加载或生成本端密钥对（长期复用，不每次连接重新生成）。
fn ensure_keypair(config: &MachineConfig, wg_dir: &Path) -> Result<(String, String)> {
    let priv_path = wg_dir.join("private.key");
    let pub_path = wg_dir.join("public.key");

    // 已存在则复用
    if priv_path.exists() && pub_path.exists() {
        let priv_key = wgvpn::read_private_key(&priv_path)?;
        let pub_key = std::fs::read_to_string(&pub_path)?.trim().to_string();
        if wgvpn::validate_public_key(&pub_key).is_ok() {
            return Ok((priv_key, pub_key));
        }
        // 公钥无效，重新生成
        warn!("[wgvpn] cached pubkey invalid, regenerating");
    }

    let (priv_key, pub_key) = if uses_userspace_wg() {
        let punch_lib = resolve_p2p_punch_lib(config);
        let pair = gonc_ffi::generate_wg_keypair(Path::new(&punch_lib))?;
        (pair.private_key, pair.public_key)
    } else {
        let wg_cli = resolve_wg_cli();
        wgvpn::generate_keypair(&wg_cli)?
    };
    wgvpn::write_private_key(&priv_path, &priv_key)?;
    std::fs::write(&pub_path, &pub_key).context("failed to write public.key")?;
    Ok((priv_key, pub_key))
}

fn start_tunnel_and_wait(conf_path: &Path) -> Result<wgvpn::TunnelHandle> {
    let wireguard_exe = resolve_wireguard_exe();
    let wg_cli = resolve_wg_cli();
    let min_handshake_epoch = current_epoch_secs();
    let handle = wgvpn::start_tunnel(&wireguard_exe, &wg_cli, conf_path)?;
    if let Err(err) = wait_for_handshake(
        &wg_cli,
        &handle.tunnel_name,
        WIREGUARD_HANDSHAKE_TIMEOUT,
        min_handshake_epoch,
    ) {
        if let Err(stop_err) = wgvpn::stop_tunnel(&wireguard_exe, &handle.tunnel_name) {
            warn!(
                "[wgvpn] failed to uninstall tunnel after handshake error: {:#}",
                stop_err
            );
        }
        return Err(err);
    }
    Ok(handle)
}

/// 等待 WireGuard 握手就绪。
///
/// 轮询 `wg show <tunnel_name> latest-handshakes` 输出，直到看到本次启动后的握手时间戳或超时。
/// PoC 实测：握手通常在隧道启动后 3-5 秒内完成。
///
/// 用同步 std::process + std::thread::sleep，因为：
/// 1. 调用方已在 tokio runtime，但握手等待是短期阻塞（<30s）
/// 2. wg show 是快命令（<100ms），不会长时间占用线程
/// 3. 改用 tokio::process 会引入 async 复杂度，收益不大
fn wait_for_handshake(
    wg_cli: &str,
    tunnel_name: &str,
    timeout: Duration,
    min_epoch_secs: u64,
) -> Result<()> {
    let start = std::time::Instant::now();
    let poll_interval = Duration::from_millis(500);
    loop {
        if let Ok(stdout) = crate::wgvpn::peer_latest_handshakes(wg_cli, tunnel_name) {
            if has_established_handshake(&stdout, min_epoch_secs) {
                info!(
                    "[wgvpn] handshake established on {} after {:?}",
                    tunnel_name,
                    start.elapsed()
                );
                return Ok(());
            }
        }
        if start.elapsed() > timeout {
            return Err(anyhow!(
                "wireguard handshake timeout after {:?} on {}",
                timeout,
                tunnel_name
            ));
        }
        std::thread::sleep(poll_interval);
    }
}

fn current_epoch_secs() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs()
}

/// 等待指定 peer（按 pubkey 过滤）握手成功。
///
/// 多 peer 场景下必须按 pubkey 过滤，否则其他 peer 的旧握手会让本判断误为成功。
fn wait_for_handshake_with_peer(
    wg_cli: &str,
    tunnel_name: &str,
    peer_pubkey: &str,
    timeout: Duration,
    min_epoch_secs: u64,
) -> Result<()> {
    let start = std::time::Instant::now();
    let poll_interval = Duration::from_millis(500);
    loop {
        if let Ok(stdout) = crate::wgvpn::peer_latest_handshakes(wg_cli, tunnel_name) {
            if has_peer_handshake(&stdout, peer_pubkey, min_epoch_secs) {
                info!(
                    "[wgvpn] handshake established with peer {}.. on {} after {:?}",
                    peer_pubkey.get(..8).unwrap_or(peer_pubkey),
                    tunnel_name,
                    start.elapsed()
                );
                return Ok(());
            }
        }
        if start.elapsed() > timeout {
            return Err(anyhow!(
                "wireguard handshake timeout after {:?} on {} for peer {}..",
                timeout,
                tunnel_name,
                peer_pubkey.get(..8).unwrap_or(peer_pubkey)
            ));
        }
        std::thread::sleep(poll_interval);
    }
}

/// 判断指定 peer 是否已握手（按 pubkey 匹配，时间戳 >= min_epoch_secs 且 > 0）。
fn has_peer_handshake(stdout: &str, peer_pubkey: &str, min_epoch_secs: u64) -> bool {
    stdout.lines().any(|line| {
        let mut fields = line.split_whitespace();
        let Some(peer) = fields.next() else {
            return false;
        };
        if peer != peer_pubkey {
            return false;
        }
        let Some(ts_raw) = fields.next() else {
            return false;
        };
        if let Ok(ts) = ts_raw.parse::<u64>() {
            return ts >= min_epoch_secs && ts > 0;
        }
        false
    })
}

pub(super) fn has_established_handshake(stdout: &str, min_epoch_secs: u64) -> bool {
    stdout.lines().any(|line| {
        let mut fields = line.split_whitespace();
        let _peer = fields.next();
        let Some(ts_raw) = fields.next() else {
            return false;
        };
        let Ok(ts) = ts_raw.parse::<u64>() else {
            return false;
        };
        ts >= min_epoch_secs && ts > 0
    })
}
#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn conflicting_sessions_match_target_id_or_peer_public_key() {
        let sessions = vec![
            WgVpnSession {
                target_device_id: 65,
                peer_pubkey: "same-machine-key".to_string(),
                ..Default::default()
            },
            WgVpnSession {
                target_device_id: 77,
                peer_pubkey: "other-key".to_string(),
                ..Default::default()
            },
        ];

        assert_eq!(
            conflicting_session_ids(&sessions, 66, "same-machine-key"),
            vec![(65, true)]
        );
        assert_eq!(
            conflicting_session_ids(&sessions, 77, "new-key"),
            vec![(77, false)]
        );
        assert!(conflicting_session_ids(&sessions, 88, "new-key").is_empty());
    }

    #[test]
    fn latest_handshakes_accepts_recent_timestamp() {
        let output = "abc123\t200\n";
        assert!(has_established_handshake(output, 100));
    }

    #[test]
    fn latest_handshakes_rejects_none_zero_and_stale() {
        assert!(!has_established_handshake("abc123\t0\n", 100));
        assert!(!has_established_handshake(
            "latest handshake: (none)\n",
            100
        ));
        assert!(!has_established_handshake("abc123\t99\n", 100));
    }

    #[test]
    fn cleanup_errors_prevent_successful_disconnect_completion() {
        let errors = vec!["stop WireGuard tunnel: access denied".to_string()];
        let err = ensure_wgvpn_cleanup_complete(&errors).unwrap_err();
        assert!(err.to_string().contains("cleanup incomplete"));
        assert!(err.to_string().contains("access denied"));
    }

    #[test]
    fn passive_device_ip_is_stable_and_device_specific() {
        let start = wgvpn_exchange::ipv4_to_u32("100.99.71.2").unwrap();
        let end = wgvpn_exchange::ipv4_to_u32("100.99.71.254").unwrap();
        assert_eq!(
            passive_ip_for_device(59, start, end).unwrap(),
            passive_ip_for_device(59, start, end).unwrap()
        );
        assert_ne!(
            passive_ip_for_device(59, start, end).unwrap(),
            passive_ip_for_device(130, start, end).unwrap()
        );
    }

    #[test]
    fn passive_honors_valid_requested_ip() {
        let start = wgvpn_exchange::ipv4_to_u32("100.99.71.2").unwrap();
        let end = wgvpn_exchange::ipv4_to_u32("100.99.71.254").unwrap();
        let requested = wgvpn_exchange::ipv4_to_u32("100.99.71.88").unwrap();
        assert_eq!(
            passive_ip_for_request(59, start, end, requested).unwrap(),
            requested
        );
        assert_eq!(
            passive_ip_for_request(59, start, end, 0).unwrap(),
            passive_ip_for_device(59, start, end).unwrap()
        );
    }

    #[test]
    fn active_chooses_next_passive_ip_when_preferred_is_used() {
        let start = wgvpn_exchange::ipv4_to_u32("100.99.71.2").unwrap();
        let end = wgvpn_exchange::ipv4_to_u32("100.99.71.254").unwrap();
        let preferred = passive_ip_for_device(59, start, end).unwrap();
        {
            let mut sessions = WGVPN_SESSIONS.lock();
            sessions.clear();
            sessions.insert(
                1,
                WgVpnSession {
                    peer_virtual_ip: wgvpn_exchange::u32_to_ipv4(preferred),
                    ..Default::default()
                },
            );
        }
        let chosen = choose_passive_ip_for_peer("", 59, start, end).unwrap();
        WGVPN_SESSIONS.lock().clear();
        assert_ne!(chosen, preferred);
        assert!(chosen >= start && chosen <= end);
    }

    #[test]
    fn lan_routes_reject_wgvpn_and_overlaps() {
        assert!(
            ipv4_cidr_range("100.99.71.0/24").unwrap().0 <= ipv4_cidr_range(WGVPN_CIDR).unwrap().1
        );
        assert!(validate_new_lan_routes(&["100.99.71.8/32".to_string()]).is_err());
    }
}
