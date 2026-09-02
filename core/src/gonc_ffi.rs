//! gonc FFI client for wgvpn key exchange and UDP data tunnel.

use anyhow::{anyhow, Context, Result};
use serde::{Deserialize, Serialize};
use std::ffi::{CStr, CString};
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
pub struct StopUdpTunnelRequest {
    pub handle_id: String,
}

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

#[cfg_attr(windows, link(name = "p2premote-punch", kind = "raw-dylib"))]
#[cfg_attr(target_os = "macos", link(name = "p2premote-punch"))]
unsafe extern "C" {
    fn StartUdpTunnel(input: *const c_char) -> *mut c_char;
    fn StopUdpTunnel(input: *const c_char) -> *mut c_char;
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
    fn Exchange(input: *const c_char) -> *mut c_char;
    fn FreeCString(ptr: *mut c_char);
}

const USERSPACE_WG_ABI_VERSION: u32 = 2;

pub fn get_wg_capabilities(library_path: &Path) -> Result<WgCapabilitiesResult> {
    validate_punch_library_available(library_path)?;
    let output = unsafe { ffi_call("{}", |ptr| GetWgCapabilities(ptr), "GetWgCapabilities")? };
    let result: WgCapabilitiesResult =
        serde_json::from_str(&output).context("failed to decode userspace WG capabilities")?;
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

pub fn generate_wg_keypair(library_path: &Path) -> Result<WgKeypairResult> {
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

pub fn start_userspace_wg_peer(
    library_path: &Path,
    request: &StartWindowsWgPeerRequest,
) -> Result<WindowsWgPeerResult> {
    let capabilities = get_wg_capabilities(library_path)?;
    if !capabilities.userspace_wg || !capabilities.native_tun {
        return Err(anyhow!(
            "punch library does not support native userspace WG"
        ));
    }
    let input = serde_json::to_string(request).context("failed to encode userspace WG peer")?;
    let output = unsafe {
        ffi_call(
            &input,
            |ptr| StartUserspaceWgPeer(ptr),
            "StartUserspaceWgPeer",
        )?
    };
    decode_windows_wg_peer_result(&output, "start")
}

pub fn stop_userspace_wg_peer(library_path: &Path, handle_id: &str) -> Result<()> {
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

pub fn get_userspace_wg_peer_status(
    library_path: &Path,
    handle_id: &str,
) -> Result<WindowsWgPeerResult> {
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

/// Toggle a passive userspace WireGuard peer's AllowedIPs without rebuilding
/// the peer or touching the packet hot path.
pub fn set_userspace_wg_peer_allowed(
    library_path: &Path,
    handle_id: &str,
    allowed: bool,
) -> Result<WindowsWgPeerResult> {
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

pub fn stop_userspace_wg_engine(library_path: &Path) -> Result<()> {
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

pub fn cleanup_userspace_wg_platform(library_path: &Path) -> Result<()> {
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

pub fn start_udp_tunnel(
    library_path: &Path,
    request: &UdpTunnelRequest,
) -> Result<UdpTunnelResult> {
    validate_punch_library_available(library_path)?;
    let input = serde_json::to_string(request).context("failed to encode udp tunnel request")?;
    let output = ffi_call(
        &input,
        |ptr| unsafe { StartUdpTunnel(ptr) },
        "StartUdpTunnel",
    )?;
    parse_udp_tunnel_result(&output)
}

pub fn stop_udp_tunnel(library_path: &Path, handle_id: &str) -> Result<()> {
    if handle_id.is_empty() {
        return Ok(());
    }
    validate_punch_library_available(library_path)?;
    let input = serde_json::to_string(&StopUdpTunnelRequest {
        handle_id: handle_id.to_string(),
    })
    .context("failed to encode stop udp tunnel request")?;
    let output = ffi_call(&input, |ptr| unsafe { StopUdpTunnel(ptr) }, "StopUdpTunnel")?;
    let result: StopUdpTunnelResult = serde_json::from_str(&output)
        .with_context(|| format!("invalid stop tunnel result: {}", output))?;
    if !result.ok {
        return Err(anyhow!(
            "stop udp tunnel failed: {}",
            if result.error.is_empty() {
                "unknown error"
            } else {
                result.error.as_str()
            }
        ));
    }
    Ok(())
}

pub fn start_subnet_router(
    library_path: &Path,
    request: &StartSubnetRouterRequest,
) -> Result<SubnetRouterResult> {
    validate_punch_library_available(library_path)?;
    let input = serde_json::to_string(request).context("failed to encode subnet router request")?;
    let output = ffi_call(
        &input,
        |ptr| unsafe { StartSubnetRouter(ptr) },
        "StartSubnetRouter",
    )?;
    parse_subnet_router_result(&output, "start subnet router")
}

pub fn stop_subnet_router(library_path: &Path, handle_id: &str) -> Result<()> {
    if handle_id.is_empty() {
        return Ok(());
    }
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

pub fn get_subnet_router_status(
    library_path: &Path,
    handle_id: &str,
) -> Result<SubnetRouterResult> {
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

pub fn parse_udp_tunnel_result(raw: &str) -> Result<UdpTunnelResult> {
    let result: UdpTunnelResult = serde_json::from_str(raw)
        .with_context(|| format!("invalid udp tunnel result json: {}", raw))?;
    if !result.ok {
        return Err(anyhow::Error::new(UdpTunnelFailure { result }));
    }
    if result.handle_id.is_empty() {
        return Err(anyhow!("udp tunnel result missing handle_id"));
    }
    if result.local_forward_port == 0 {
        return Err(anyhow!("udp tunnel result missing local_forward_port"));
    }
    Ok(result)
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

#[derive(Debug, Serialize)]
struct ExchangeRequest {
    token: String,
    exmode: i32,
    send_data: String,
    role_hint: String,
    timeout_secs: u64,
}

#[derive(Debug, Deserialize)]
struct ExchangeResponse {
    ok: bool,
    #[serde(default)]
    recv_data: String,
    #[serde(default)]
    error: String,
}

pub fn exchange_payload(
    library_path: &Path,
    token: &str,
    mode: ExchangeMode,
    send_data: &str,
    timeout: Duration,
) -> Result<String> {
    if timeout.is_zero() {
        return Err(anyhow!("exchange timeout must be greater than zero"));
    }
    let timeout_secs = timeout
        .as_secs()
        .saturating_add(u64::from(timeout.subsec_nanos() != 0));
    validate_punch_library_available(library_path)?;
    let req = ExchangeRequest {
        token: token.to_string(),
        exmode: mode.as_int(),
        send_data: send_data.to_string(),
        role_hint: if mode == ExchangeMode::Mutual {
            "active"
        } else {
            "passive"
        }
        .to_string(),
        timeout_secs,
    };
    let input = serde_json::to_string(&req).context("encode exchange request")?;
    let output = ffi_call(&input, |ptr| unsafe { Exchange(ptr) }, "Exchange")?;
    let resp: ExchangeResponse = serde_json::from_str(&output)
        .with_context(|| format!("invalid exchange result: {}", output))?;
    if !resp.ok {
        return Err(anyhow!(
            "exchange failed: {}",
            if resp.error.is_empty() {
                "unknown error"
            } else {
                resp.error.as_str()
            }
        ));
    }
    Ok(resp.recv_data)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_udp_tunnel_success_result() {
        let raw = r#"{
            "ok": true,
            "handle_id": "udp-1",
            "local_forward_addr": "127.0.0.1:32000",
            "local_forward_port": 32000,
            "peer_endpoint": "203.0.113.10:40000",
            "local_nat_type": "easy",
            "remote_nat_type": "hard",
            "network": "udp4",
            "selected_traversal": "lan",
            "transport_mode": "plain",
            "local_lan_addr": "192.168.1.10:32001",
            "local_nat_addr": "198.51.100.20:41000",
            "remote_lan_addr": "192.168.2.10:32002",
            "remote_nat_addr": "203.0.113.10:40000",
            "is_client": true
        }"#;
        let parsed = parse_udp_tunnel_result(raw).unwrap();
        assert_eq!(parsed.handle_id, "udp-1");
        assert_eq!(parsed.local_forward_port, 32000);
        assert_eq!(parsed.selected_traversal, "lan");
        assert_eq!(parsed.transport_mode, "plain");
    }

    #[test]
    fn rejects_failure_result() {
        let err = parse_udp_tunnel_result(
            r#"{
                "ok": false,
                "attempts": 5,
                "network": "udp4",
                "local_nat_type": "hard",
                "remote_nat_type": "symm",
                "local_nat_addr": "198.51.100.20:41000",
                "remote_nat_addr": "203.0.113.10:40000",
                "error": "timeout"
            }"#,
        )
        .unwrap_err();
        assert!(err.to_string().contains("timeout"));
        let failure = err.downcast_ref::<UdpTunnelFailure>().unwrap();
        assert_eq!(failure.result.attempts, 5);
        assert_eq!(failure.result.local_nat_type, "hard");
        assert_eq!(failure.result.remote_nat_type, "symm");
        assert_eq!(failure.result.local_nat_addr, "198.51.100.20:41000");
    }

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

    #[test]
    fn exchange_rejects_zero_timeout_before_ffi() {
        let err = exchange_payload(
            Path::new("unused"),
            "token",
            ExchangeMode::Mutual,
            "{}",
            Duration::ZERO,
        )
        .unwrap_err();
        assert!(err.to_string().contains("greater than zero"));
    }
}
