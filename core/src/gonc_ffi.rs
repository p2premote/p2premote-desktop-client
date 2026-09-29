//! gonc FFI client for wgvpn key exchange and UDP data tunnel.

// Punch/Exchange 在 Linux/Windows/macOS 三个平台都通过源码 path 依赖
// （p2premote-punch-rs，原生 Rust API）集成。Go 侧只保留 userspace
// WireGuard 数据面 FFI：Windows 为 p2premote-wg.dll（raw-dylib），
// macOS 为 libp2premote-wg.dylib，均出自 p2premote-wg-ffi（wgonly）。
#[cfg(any(target_os = "linux", windows, target_os = "macos"))]
use p2premote_punch as _;

use anyhow::{anyhow, Context, Result};
use serde::{Deserialize, Serialize};
#[cfg(not(target_os = "linux"))]
use std::ffi::{CStr, CString};
#[cfg(not(target_os = "linux"))]
use std::os::raw::c_char;
use std::path::Path;
use std::time::Duration;

#[derive(Debug, Clone, Serialize)]
pub struct UdpTunnelRequest {
    pub token: String,
    pub role_hint: String,
    pub traversal_mode: String,
    pub network: String,
    pub timeout_secs: u64,
    pub bind_ip: String,
    pub local_listen_ip: String,
    pub local_listen_port: u16,
    pub remote_target_ip: String,
    pub remote_target_port: u16,
    pub allow_relay: bool,
}

impl UdpTunnelRequest {
    pub fn wgvpn(
        token: impl Into<String>,
        role_hint: impl Into<String>,
        wg_listen_port: u16,
    ) -> Self {
        Self {
            token: token.into(),
            role_hint: role_hint.into(),
            traversal_mode: "auto".to_string(),
            network: "udp4".to_string(),
            // NAT mapping behavior can vary by source port. Give gonc enough
            // time to cycle through its candidate ports before declaring the
            // direct UDP path unavailable. The outer active-job deadline is
            // still 120 seconds.
            timeout_secs: 100,
            bind_ip: String::new(),
            local_listen_ip: "127.0.0.1".to_string(),
            local_listen_port: 0,
            remote_target_ip: "127.0.0.1".to_string(),
            remote_target_port: wg_listen_port,
            allow_relay: false,
        }
    }
}

#[derive(Debug, Clone, Deserialize, PartialEq, Eq)]
pub struct UdpTunnelResult {
    pub ok: bool,
    #[serde(default)]
    pub handle_id: String,
    #[serde(default)]
    pub local_forward_addr: String,
    #[serde(default)]
    pub local_forward_port: u16,
    #[serde(default)]
    pub peer_endpoint: String,
    #[serde(default)]
    pub local_nat_type: String,
    #[serde(default)]
    pub remote_nat_type: String,
    #[serde(default)]
    pub network: String,
    #[serde(default)]
    pub selected_traversal: String,
    #[serde(default)]
    pub transport_mode: String,
    #[serde(default)]
    pub local_lan_addr: String,
    #[serde(default)]
    pub local_nat_addr: String,
    #[serde(default)]
    pub remote_lan_addr: String,
    #[serde(default)]
    pub remote_nat_addr: String,
    #[serde(default)]
    pub is_client: bool,
    #[serde(default)]
    pub attempts: u32,
    #[serde(default)]
    pub error: String,
}

#[derive(Debug)]
pub struct UdpTunnelFailure {
    pub result: UdpTunnelResult,
}

impl std::fmt::Display for UdpTunnelFailure {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "udp tunnel failed: {}",
            if self.result.error.is_empty() {
                "unknown error"
            } else {
                self.result.error.as_str()
            }
        )
    }
}

impl std::error::Error for UdpTunnelFailure {}

#[derive(Debug, Clone, Serialize)]
pub struct StartSubnetRouterRequest {
    pub session_id: i64,
    pub peer_device_id: i64,
    pub wg_private_key: String,
    pub peer_public_key: String,
    pub tail_ip: String,
    pub peer_tail_ip: String,
    pub peer_endpoint: String,
    pub listen_ip: String,
    pub listen_port: u16,
    pub exposed_lan_cidrs: Vec<String>,
    pub snat: bool,
    pub allow_tcp: bool,
    pub allow_udp: bool,
    pub allow_icmp_echo: bool,
}

#[derive(Debug, Clone, Serialize)]
pub struct StopSubnetRouterRequest {
    pub handle_id: String,
}

#[derive(Debug, Clone, Serialize)]
pub struct GetSubnetRouterStatusRequest {
    pub handle_id: String,
}

#[derive(Debug, Clone, Serialize)]
pub struct StartWindowsWgPeerRequest {
    pub session_id: i64,
    pub peer_device_id: i64,
    pub role: String,
    pub wg_private_key: String,
    pub peer_public_key: String,
    pub local_tail_ip: String,
    pub peer_tail_ip: String,
    pub peer_endpoint: String,
    pub listen_ip: String,
    pub listen_port: u16,
    pub routes: Vec<String>,
}

#[derive(Debug, Clone, Serialize)]
pub struct WindowsWgPeerHandleRequest {
    pub handle_id: String,
}

#[derive(Debug, Clone, Deserialize, PartialEq, Eq)]
pub struct WgCapabilitiesResult {
    pub ok: bool,
    #[serde(default)]
    pub abi_version: u32,
    #[serde(default)]
    pub platform: String,
    #[serde(default)]
    pub userspace_wg: bool,
    #[serde(default)]
    pub hybrid_tun: bool,
    #[serde(default)]
    pub wintun: bool,
    #[serde(default)]
    pub native_tun: bool,
    #[serde(default)]
    pub netstack_proxy: bool,
    #[serde(default)]
    pub error: String,
}

#[derive(Debug, Clone, Deserialize, PartialEq, Eq)]
pub struct WgKeypairResult {
    pub ok: bool,
    #[serde(default)]
    pub private_key: String,
    #[serde(default)]
    pub public_key: String,
    #[serde(default)]
    pub error: String,
}

#[derive(Debug, Clone, Deserialize, PartialEq, Eq)]
pub struct WindowsWgPeerResult {
    pub ok: bool,
    #[serde(default)]
    pub handle_id: String,
    #[serde(default)]
    pub started: bool,
    #[serde(default)]
    pub role: String,
    #[serde(default)]
    pub local_tail_ip: String,
    #[serde(default)]
    pub peer_tail_ip: String,
    #[serde(default)]
    pub last_handshake_at: i64,
    #[serde(default)]
    pub rx_bytes: i64,
    #[serde(default)]
    pub tx_bytes: i64,
    #[serde(default)]
    pub rx_packets: i64,
    #[serde(default)]
    pub tx_packets: i64,
    #[serde(default)]
    pub rx_batches: i64,
    #[serde(default)]
    pub tx_batches: i64,
    #[serde(default)]
    pub tcp_sessions: u32,
    #[serde(default)]
    pub udp_sessions: u32,
    #[serde(default)]
    pub icmp_success: i64,
    #[serde(default)]
    pub icmp_failed: i64,
    #[serde(default)]
    pub rejected_flows: i64,
    #[serde(default)]
    pub last_error: String,
    #[serde(default)]
    pub error: String,
}

#[derive(Debug, Clone, Deserialize, PartialEq, Eq)]
pub struct StopUdpTunnelResult {
    pub ok: bool,
    #[serde(default)]
    pub error: String,
}

#[derive(Debug, Clone, Deserialize, PartialEq, Eq)]
pub struct SubnetRouterResult {
    pub ok: bool,
    #[serde(default)]
    pub handle_id: String,
    #[serde(default)]
    pub lan_mode: String,
    #[serde(default)]
    pub listen_ip: String,
    #[serde(default)]
    pub listen_port: u16,
    #[serde(default)]
    pub started: bool,
    #[serde(default)]
    pub tcp_sessions: u32,
    #[serde(default)]
    pub udp_sessions: u32,
    #[serde(default)]
    pub wg_rx_packets: u64,
    #[serde(default)]
    pub wg_tx_packets: u64,
    #[serde(default)]
    pub icmp_success: u64,
    #[serde(default)]
    pub icmp_failed: u64,
    #[serde(default)]
    pub rejected_flows: u64,
    #[serde(default)]
    pub last_error: String,
    #[serde(default)]
    pub advertised_routes: Vec<String>,
    #[serde(default)]
    pub error: String,
}

// Go owns the userspace-WireGuard ABI. Windows links the deliberately thin
// p2premote-wg.dll via raw-dylib; macOS links the equivalent wgonly
// libp2premote-wg.dylib built from the same p2premote-wg-ffi source.
#[cfg(not(target_os = "linux"))]
#[cfg_attr(windows, link(name = "p2premote-wg", kind = "raw-dylib"))]
#[cfg_attr(target_os = "macos", link(name = "p2premote-wg"))]
extern "C" {
    fn StartSubnetRouter(input: *const c_char) -> *mut c_char;
    fn StopSubnetRouter(input: *const c_char) -> *mut c_char;
    fn GetSubnetRouterStatus(input: *const c_char) -> *mut c_char;
    fn GetWgCapabilities(input: *const c_char) -> *mut c_char;
    fn GenerateWgKeypair(input: *const c_char) -> *mut c_char;
    fn StartUserspaceWgPeer(input: *const c_char) -> *mut c_char;
    fn StopUserspaceWgPeer(input: *const c_char) -> *mut c_char;
    fn GetUserspaceWgPeerStatus(input: *const c_char) -> *mut c_char;
    fn SetUserspaceWgPeerAllowed(input: *const c_char) -> *mut c_char;
    fn StopUserspaceWgEngine(input: *const c_char) -> *mut c_char;
    fn CleanupUserspaceWgPlatform(input: *const c_char) -> *mut c_char;
    fn FreeCString(ptr: *mut c_char);
}

#[cfg(not(target_os = "linux"))]
const USERSPACE_WG_ABI_VERSION: u32 = 2;

pub fn get_wg_capabilities(library_path: &Path) -> Result<WgCapabilitiesResult> {
    #[cfg(target_os = "linux")]
    {
        let _ = library_path;
        return Err(anyhow!("userspace WireGuard FFI is not used on Linux"));
    }
    #[cfg(not(target_os = "linux"))]
    {
        validate_punch_library_available(library_path)?;
        tracing::info!(
            "[wgvpn] GetWgCapabilities begin: library={}",
            library_path.display()
        );
        let output = unsafe { ffi_call("{}", |ptr| GetWgCapabilities(ptr), "GetWgCapabilities")? };
        let result: WgCapabilitiesResult =
            serde_json::from_str(&output).context("failed to decode userspace WG capabilities")?;
        tracing::info!(
        "[wgvpn] GetWgCapabilities result: ok={}, abi_version={}, platform={}, userspace_wg={}, native_tun={}, wintun={}, hybrid_tun={}, error={}",
        result.ok,
        result.abi_version,
        result.platform,
        result.userspace_wg,
        result.native_tun,
        result.wintun,
        result.hybrid_tun,
        if result.error.is_empty() { "none" } else { result.error.as_str() }
    );
        if !result.ok {
            return Err(anyhow!(
                "userspace WG capability check failed: {}",
                result.error
            ));
        }
        if result.abi_version != USERSPACE_WG_ABI_VERSION {
            return Err(anyhow!(
                "userspace WG ABI mismatch: expected {}, got {}",
                USERSPACE_WG_ABI_VERSION,
                result.abi_version
            ));
        }
        Ok(result)
    }
}

pub fn generate_wg_keypair(library_path: &Path) -> Result<WgKeypairResult> {
    #[cfg(target_os = "linux")]
    {
        let _ = library_path;
        return Err(anyhow!(
            "userspace WireGuard key generation is not used on Linux"
        ));
    }
    #[cfg(not(target_os = "linux"))]
    {
        get_wg_capabilities(library_path)?;
        let output = unsafe { ffi_call("{}", |ptr| GenerateWgKeypair(ptr), "GenerateWgKeypair")? };
        let result: WgKeypairResult =
            serde_json::from_str(&output).context("failed to decode userspace WG keypair")?;
        if !result.ok || result.private_key.is_empty() || result.public_key.is_empty() {
            return Err(anyhow!(
                "userspace WG key generation failed: {}",
                result.error
            ));
        }
        Ok(result)
    }
}

pub fn start_userspace_wg_peer(
    library_path: &Path,
    request: &StartWindowsWgPeerRequest,
) -> Result<WindowsWgPeerResult> {
    #[cfg(target_os = "linux")]
    {
        let _ = (library_path, request);
        return Err(anyhow!("userspace WireGuard peers are not used on Linux"));
    }
    #[cfg(not(target_os = "linux"))]
    {
        let capabilities = get_wg_capabilities(library_path)?;
        if !capabilities.userspace_wg || !capabilities.native_tun {
            return Err(anyhow!(
                "punch library does not support native userspace WG"
            ));
        }
        let input = serde_json::to_string(request).context("failed to encode userspace WG peer")?;
        tracing::info!(
        "[wgvpn] StartUserspaceWgPeer begin: session_id={}, peer_device_id={}, role={}, local_tail_ip={}, peer_tail_ip={}, peer_endpoint={}, listen={}:{}, routes={}",
        request.session_id,
        request.peer_device_id,
        request.role,
        request.local_tail_ip,
        request.peer_tail_ip,
        request.peer_endpoint,
        request.listen_ip,
        request.listen_port,
        request.routes.len()
    );
        let output = unsafe {
            ffi_call(
                &input,
                |ptr| StartUserspaceWgPeer(ptr),
                "StartUserspaceWgPeer",
            )?
        };
        let result = decode_windows_wg_peer_result(&output, "start");
        match &result {
        Ok(status) => tracing::info!(
            "[wgvpn] StartUserspaceWgPeer result: ok=true, handle_id={}, started={}, last_handshake_at={}, error={}",
            status.handle_id,
            status.started,
            status.last_handshake_at,
            if status.error.is_empty() { "none" } else { status.error.as_str() }
        ),
        Err(err) => tracing::error!("[wgvpn] StartUserspaceWgPeer failed: {:#}", err),
    }
        result
    }
}

pub fn stop_userspace_wg_peer(library_path: &Path, handle_id: &str) -> Result<()> {
    #[cfg(target_os = "linux")]
    {
        let _ = (library_path, handle_id);
        return Ok(());
    }
    #[cfg(not(target_os = "linux"))]
    {
        validate_punch_library_available(library_path)?;
        let input = serde_json::to_string(&WindowsWgPeerHandleRequest {
            handle_id: handle_id.to_string(),
        })?;
        let output = unsafe {
            ffi_call(
                &input,
                |ptr| StopUserspaceWgPeer(ptr),
                "StopUserspaceWgPeer",
            )?
        };
        decode_windows_wg_peer_result(&output, "stop")?;
        Ok(())
    }
}

pub fn get_userspace_wg_peer_status(
    library_path: &Path,
    handle_id: &str,
) -> Result<WindowsWgPeerResult> {
    #[cfg(target_os = "linux")]
    {
        let _ = (library_path, handle_id);
        return Err(anyhow!("userspace WireGuard peers are not used on Linux"));
    }
    #[cfg(not(target_os = "linux"))]
    {
        validate_punch_library_available(library_path)?;
        let input = serde_json::to_string(&WindowsWgPeerHandleRequest {
            handle_id: handle_id.to_string(),
        })?;
        let output = unsafe {
            ffi_call(
                &input,
                |ptr| GetUserspaceWgPeerStatus(ptr),
                "GetUserspaceWgPeerStatus",
            )?
        };
        decode_windows_wg_peer_result(&output, "status")
    }
}

/// Toggle a passive userspace WireGuard peer's AllowedIPs without rebuilding
/// the peer or touching the packet hot path.
pub fn set_userspace_wg_peer_allowed(
    library_path: &Path,
    handle_id: &str,
    allowed: bool,
) -> Result<WindowsWgPeerResult> {
    #[cfg(target_os = "linux")]
    {
        let _ = (library_path, handle_id, allowed);
        return Err(anyhow!("userspace WireGuard peers are not used on Linux"));
    }
    #[cfg(not(target_os = "linux"))]
    {
        validate_punch_library_available(library_path)?;
        let input = serde_json::json!({ "handle_id": handle_id, "allowed": allowed }).to_string();
        let output = unsafe {
            ffi_call(
                &input,
                |ptr| SetUserspaceWgPeerAllowed(ptr),
                "SetUserspaceWgPeerAllowed",
            )?
        };
        decode_windows_wg_peer_result(&output, "set allowed")
    }
}

pub fn stop_userspace_wg_engine(library_path: &Path) -> Result<()> {
    #[cfg(target_os = "linux")]
    {
        let _ = library_path;
        return Ok(());
    }
    #[cfg(not(target_os = "linux"))]
    {
        validate_punch_library_available(library_path)?;
        let output = unsafe {
            ffi_call(
                "{}",
                |ptr| StopUserspaceWgEngine(ptr),
                "StopUserspaceWgEngine",
            )?
        };
        decode_windows_wg_peer_result(&output, "engine stop")?;
        Ok(())
    }
}

pub fn cleanup_userspace_wg_platform(library_path: &Path) -> Result<()> {
    #[cfg(target_os = "linux")]
    {
        let _ = library_path;
        return Ok(());
    }
    #[cfg(not(target_os = "linux"))]
    {
        validate_punch_library_available(library_path)?;
        let output = unsafe {
            ffi_call(
                "{}",
                |ptr| CleanupUserspaceWgPlatform(ptr),
                "CleanupUserspaceWgPlatform",
            )?
        };
        decode_windows_wg_peer_result(&output, "platform cleanup")?;
        Ok(())
    }
}

#[cfg(not(target_os = "linux"))]
fn decode_windows_wg_peer_result(output: &str, action: &str) -> Result<WindowsWgPeerResult> {
    let result: WindowsWgPeerResult = serde_json::from_str(output)
        .with_context(|| format!("failed to decode userspace WG {} result", action))?;
    if !result.ok {
        return Err(anyhow!("userspace WG {} failed: {}", action, result.error));
    }
    Ok(result)
}

pub fn validate_punch_library_available(library_path: &Path) -> Result<()> {
    #[cfg(target_os = "linux")]
    {
        let _ = library_path;
        return Ok(());
    }

    #[cfg(not(target_os = "linux"))]
    {
        if !library_path.exists() {
            return Err(anyhow!(
                "gonc ffi library path does not exist: {}",
                library_path.display()
            ));
        }
        Ok(())
    }
}

#[cfg(not(target_os = "linux"))]
fn ffi_call<F>(input: &str, call: F, null_name: &str) -> Result<String>
where
    F: FnOnce(*const c_char) -> *mut c_char,
{
    let input = CString::new(input).context("ffi request contains nul byte")?;
    let output_ptr = call(input.as_ptr());
    if output_ptr.is_null() {
        return Err(anyhow!("{} returned null", null_name));
    }
    let output = unsafe { CStr::from_ptr(output_ptr) }
        .to_string_lossy()
        .to_string();
    unsafe { FreeCString(output_ptr) };
    Ok(output)
}

/// Native Rust Punch path used by Linux/Windows/macOS; the punch crate is
/// linked in-process on all three platforms.
#[cfg(any(target_os = "linux", windows, target_os = "macos"))]
pub async fn start_udp_tunnel_native(request: &UdpTunnelRequest) -> Result<UdpTunnelResult> {
    let result = p2premote_punch::api::start_udp_tunnel(
        request.into(),
        Duration::from_secs(request.timeout_secs.saturating_add(10)),
    )
    .await
    .map_err(|error| anyhow!(error))?;
    native_udp_tunnel_result(result)
}

/// 字段级转换（与原 JSON round-trip 等价：两侧均 snake_case 无 rename，
/// UdpTunnelRequest 恒序列化全部字段，punch 侧的 serde default 不会触发）。
#[cfg(any(target_os = "linux", windows, target_os = "macos"))]
impl From<&UdpTunnelRequest> for p2premote_punch::UdpTunnelInput {
    fn from(request: &UdpTunnelRequest) -> Self {
        Self {
            token: request.token.clone(),
            role_hint: request.role_hint.clone(),
            traversal_mode: request.traversal_mode.clone(),
            network: request.network.clone(),
            timeout_secs: i32::try_from(request.timeout_secs).unwrap_or(i32::MAX),
            bind_ip: request.bind_ip.clone(),
            local_listen_ip: request.local_listen_ip.clone(),
            local_listen_port: i32::from(request.local_listen_port),
            remote_target_ip: request.remote_target_ip.clone(),
            remote_target_port: i32::from(request.remote_target_port),
            allow_relay: request.allow_relay,
        }
    }
}

/// punch 结果 → core 结果。端口号/尝试次数由 i32 收窄为 u16/u32，
/// 溢出按错误处理（原 JSON 反序列化遇到越界同样失败）。
#[cfg(any(target_os = "linux", windows, target_os = "macos"))]
fn native_udp_tunnel_result(result: p2premote_punch::UdpTunnelResult) -> Result<UdpTunnelResult> {
    Ok(UdpTunnelResult {
        ok: result.ok,
        handle_id: result.handle_id,
        local_forward_addr: result.local_forward_addr,
        local_forward_port: u16::try_from(result.local_forward_port)
            .context("udp tunnel local_forward_port out of range")?,
        peer_endpoint: result.peer_endpoint,
        local_nat_type: result.local_nat_type,
        remote_nat_type: result.remote_nat_type,
        network: result.network,
        selected_traversal: result.selected_traversal,
        transport_mode: result.transport_mode,
        local_lan_addr: result.local_lan_addr,
        local_nat_addr: result.local_nat_addr,
        remote_lan_addr: result.remote_lan_addr,
        remote_nat_addr: result.remote_nat_addr,
        is_client: result.is_client,
        attempts: u32::try_from(result.attempts).context("udp tunnel attempts out of range")?,
        error: result.error,
    })
}

#[cfg(not(any(target_os = "linux", windows, target_os = "macos")))]
pub async fn start_udp_tunnel_native(_request: &UdpTunnelRequest) -> Result<UdpTunnelResult> {
    Err(anyhow!("native Rust Punch is not enabled on this platform"))
}

#[cfg(any(target_os = "linux", windows, target_os = "macos"))]
pub fn stop_udp_tunnel_native(handle_id: &str) {
    p2premote_punch::api::stop_udp_tunnel(handle_id);
}

pub fn stop_udp_tunnel(handle_id: &str) -> Result<()> {
    // All supported platforms stop tunnels through the linked Rust Punch crate.
    if handle_id.is_empty() {
        return Ok(());
    }
    stop_udp_tunnel_native(handle_id);
    Ok(())
}

pub fn start_subnet_router(
    library_path: &Path,
    request: &StartSubnetRouterRequest,
) -> Result<SubnetRouterResult> {
    #[cfg(target_os = "linux")]
    {
        let _ = library_path;
        let native =
            p2premote_punch::api::start_subnet_router(p2premote_punch::StartSubnetRouterInput {
                session_id: request.session_id,
                peer_device_id: request.peer_device_id,
                wg_private_key: request.wg_private_key.clone(),
                peer_public_key: request.peer_public_key.clone(),
                tail_ip: request.tail_ip.clone(),
                peer_tail_ip: request.peer_tail_ip.clone(),
                peer_endpoint: request.peer_endpoint.clone(),
                listen_ip: request.listen_ip.clone(),
                listen_port: i32::from(request.listen_port),
                exposed_lan_cidrs: request.exposed_lan_cidrs.clone(),
                snat: request.snat,
                allow_tcp: request.allow_tcp,
                allow_udp: request.allow_udp,
                allow_icmp_echo: request.allow_icmp_echo,
            });
        return native_subnet_router_result(native, "start subnet router");
    }
    #[cfg(not(target_os = "linux"))]
    {
        validate_punch_library_available(library_path)?;
        let input =
            serde_json::to_string(request).context("failed to encode subnet router request")?;
        let output = ffi_call(
            &input,
            |ptr| unsafe { StartSubnetRouter(ptr) },
            "StartSubnetRouter",
        )?;
        parse_subnet_router_result(&output, "start subnet router")
    }
}

pub fn stop_subnet_router(library_path: &Path, handle_id: &str) -> Result<()> {
    if handle_id.is_empty() {
        return Ok(());
    }
    #[cfg(target_os = "linux")]
    {
        let _ = library_path;
        let result = native_subnet_router_result(
            p2premote_punch::api::stop_subnet_router(handle_id),
            "stop subnet router",
        )?;
        let _ = result;
        return Ok(());
    }
    #[cfg(not(target_os = "linux"))]
    {
        validate_punch_library_available(library_path)?;
        let input = serde_json::to_string(&StopSubnetRouterRequest {
            handle_id: handle_id.to_string(),
        })
        .context("failed to encode stop subnet router request")?;
        let output = ffi_call(
            &input,
            |ptr| unsafe { StopSubnetRouter(ptr) },
            "StopSubnetRouter",
        )?;
        let _ = parse_subnet_router_result(&output, "stop subnet router")?;
        Ok(())
    }
}

pub fn get_subnet_router_status(
    library_path: &Path,
    handle_id: &str,
) -> Result<SubnetRouterResult> {
    #[cfg(target_os = "linux")]
    {
        let _ = library_path;
        return native_subnet_router_result(
            p2premote_punch::api::get_subnet_router_status(handle_id),
            "get subnet router status",
        );
    }
    #[cfg(not(target_os = "linux"))]
    {
        validate_punch_library_available(library_path)?;
        let input = serde_json::to_string(&GetSubnetRouterStatusRequest {
            handle_id: handle_id.to_string(),
        })
        .context("failed to encode subnet router status request")?;
        let output = ffi_call(
            &input,
            |ptr| unsafe { GetSubnetRouterStatus(ptr) },
            "GetSubnetRouterStatus",
        )?;
        parse_subnet_router_result(&output, "get subnet router status")
    }
}

#[cfg(target_os = "linux")]
fn native_subnet_router_result(
    result: p2premote_punch::SubnetRouterResult,
    op: &str,
) -> Result<SubnetRouterResult> {
    let mapped = SubnetRouterResult {
        ok: result.ok,
        handle_id: result.handle_id,
        lan_mode: result.lan_mode,
        listen_ip: result.listen_ip,
        listen_port: u16::try_from(result.listen_port).unwrap_or_default(),
        started: result.started,
        tcp_sessions: u32::try_from(result.tcp_sessions).unwrap_or_default(),
        udp_sessions: u32::try_from(result.udp_sessions).unwrap_or_default(),
        wg_rx_packets: u64::try_from(result.wg_rx_packets).unwrap_or_default(),
        wg_tx_packets: u64::try_from(result.wg_tx_packets).unwrap_or_default(),
        icmp_success: u64::try_from(result.icmp_success).unwrap_or_default(),
        icmp_failed: u64::try_from(result.icmp_failed).unwrap_or_default(),
        rejected_flows: u64::try_from(result.rejected_flows).unwrap_or_default(),
        last_error: result.last_error,
        advertised_routes: result.advertised_routes,
        error: result.error,
    };
    if !mapped.ok {
        return Err(anyhow!(
            "{} failed: {}",
            op,
            if mapped.error.is_empty() {
                "unknown error"
            } else {
                mapped.error.as_str()
            }
        ));
    }
    if op.starts_with("start") && (mapped.handle_id.is_empty() || !mapped.started) {
        return Err(anyhow!("{} result is incomplete", op));
    }
    Ok(mapped)
}

pub fn parse_subnet_router_result(raw: &str, op: &str) -> Result<SubnetRouterResult> {
    let result: SubnetRouterResult = serde_json::from_str(raw)
        .with_context(|| format!("invalid subnet router result json: {}", raw))?;
    if !result.ok {
        return Err(anyhow!(
            "{} failed: {}",
            op,
            if result.error.is_empty() {
                "unknown error"
            } else {
                result.error.as_str()
            }
        ));
    }
    if op.starts_with("start") {
        if result.handle_id.is_empty() {
            return Err(anyhow!("{} result missing handle_id", op));
        }
        if !result.started {
            return Err(anyhow!("{} result is not started", op));
        }
    }
    Ok(result)
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ExchangeMode {
    Mutual,
    WaitOnly,
    Reply,
}

impl ExchangeMode {
    fn as_int(self) -> i32 {
        match self {
            ExchangeMode::Mutual => 0,
            ExchangeMode::WaitOnly => 1,
            ExchangeMode::Reply => 2,
        }
    }
}

/// Native Rust Punch exchange used by all platforms; the punch crate is
/// linked in-process on Linux/Windows/macOS.
#[cfg(any(target_os = "linux", windows, target_os = "macos"))]
pub async fn exchange_payload_native(
    token: &str,
    mode: ExchangeMode,
    send_data: &str,
    timeout: Duration,
) -> Result<String> {
    if timeout.is_zero() {
        return Err(anyhow!("exchange timeout must be greater than zero"));
    }
    let request = p2premote_punch::ExchangeInput {
        token: token.to_string(),
        exmode: mode.as_int(),
        send_data: send_data.to_string(),
        role_hint: String::new(),
        timeout_secs: timeout.as_secs().min(i32::MAX as u64) as i32,
    };
    let result = p2premote_punch::api::exchange(request, timeout)
        .await
        .map_err(|error| anyhow!(error))?;
    if !result.ok {
        let error = if result.error.is_empty() {
            "exchange failed".to_string()
        } else {
            result.error
        };
        return Err(anyhow!(error));
    }
    Ok(result.recv_data)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn request_defaults_to_wgvpn_udp4_without_relay() {
        let req = UdpTunnelRequest::wgvpn("tok", "active", 51820);
        assert_eq!(req.network, "udp4");
        assert_eq!(req.traversal_mode, "auto");
        assert_eq!(req.timeout_secs, 100);
        assert_eq!(req.remote_target_ip, "127.0.0.1");
        assert_eq!(req.remote_target_port, 51820);
        assert!(!req.allow_relay);
    }

    #[cfg(not(target_os = "linux"))]
    #[test]
    fn preload_missing_library_fails() {
        let err =
            validate_punch_library_available(Path::new("missing-p2premote-punch.dll")).unwrap_err();
        assert!(err.to_string().contains("gonc ffi library"));
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn statically_linked_punch_library_needs_no_runtime_file() {
        validate_punch_library_available(Path::new("missing-libp2premote-punch.a")).unwrap();
    }

    #[test]
    fn parses_subnet_router_success_result() {
        let raw = r#"{
            "ok": true,
            "handle_id": "sr-1",
            "lan_mode": "userspace_snat",
            "listen_ip": "127.0.0.1",
            "listen_port": 51820,
            "started": true,
            "tcp_sessions": 1,
            "udp_sessions": 2,
            "icmp_success": 3,
            "icmp_failed": 4,
            "rejected_flows": 5,
            "last_error": "dial timeout",
            "advertised_routes": ["192.168.10.0/24"]
        }"#;
        let parsed = parse_subnet_router_result(raw, "start subnet router").unwrap();
        assert_eq!(parsed.handle_id, "sr-1");
        assert_eq!(parsed.listen_port, 51820);
        assert_eq!(parsed.advertised_routes, vec!["192.168.10.0/24"]);
        assert_eq!(parsed.icmp_success, 3);
        assert_eq!(parsed.icmp_failed, 4);
        assert_eq!(parsed.rejected_flows, 5);
        assert_eq!(parsed.last_error, "dial timeout");
    }

    #[test]
    fn rejects_subnet_router_failure_result() {
        let err = parse_subnet_router_result(
            r#"{"ok":false,"error":"subnet router backend is not implemented on windows in this build"}"#,
            "start subnet router",
        )
        .unwrap_err();
        assert!(err.to_string().contains("not implemented"));
    }

    #[test]
    fn rejects_started_subnet_router_without_handle() {
        let err =
            parse_subnet_router_result(r#"{"ok":true,"started":true}"#, "start subnet router")
                .unwrap_err();
        assert!(err.to_string().contains("handle_id"));
    }
}
