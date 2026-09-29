//! 公钥 + 虚拟 IP 协商模块。
//!
//! 通过 Rust Punch（进程内链接）的 MQTT 加密通道双向交换载荷：
//! - 主动端：一次 Mutual 模式调用，发出本端载荷并接收对端载荷。
//! - 被动端：两次调用——先 WaitOnly 收主动端载荷，本地分配 IP，再 Reply 回传。
//!
//! 载荷为 JSON 序列化的 ExchangePayload，gonc 侧用 AES-GCM 加密传输。

use crate::wgvpn::validate_public_key;
use anyhow::{anyhow, Context, Result};
use serde::{Deserialize, Serialize};
use std::collections::HashSet;
use std::net::Ipv4Addr;
use std::time::Duration;
// ============ IPv4 <-> u32 转换（大端序，即网络字节序的十进制表示） ============

/// IPv4 地址字符串（如 "100.99.71.2"）转 u32。
///
/// u32 的值 = 网络字节序（大端）整数。例如 100.99.71.2 → 0x64634702 → 1683904258。
/// 这样用十进制文本传输时无字节序歧义，跨平台一致。
pub fn ipv4_to_u32(ip: &str) -> Result<u32> {
    let parts: Vec<&str> = ip.split('.').collect();
    if parts.len() != 4 {
        return Err(anyhow!("invalid ipv4: {}", ip));
    }
    let mut bytes = [0u8; 4];
    for (i, p) in parts.iter().enumerate() {
        bytes[i] = p
            .parse::<u8>()
            .map_err(|_| anyhow!("invalid ipv4 octet: {}", p))?;
    }
    Ok(u32::from_be_bytes(bytes))
}

/// u32 转 IPv4 地址字符串。
pub fn u32_to_ipv4(v: u32) -> String {
    let bytes = v.to_be_bytes();
    format!("{}.{}.{}.{}", bytes[0], bytes[1], bytes[2], bytes[3])
}

/// 解析 IPv4 CIDR（如 "192.168.1.0/24"），返回 (网络地址 u32, 掩码长度)。
///
/// 复用 [`ipv4_to_u32`]，避免各调用点手写 split('.') + parse::<u8>。
/// 掩码长度合法范围 0-32。
pub fn parse_ipv4_cidr(cidr: &str) -> Result<(u32, u8)> {
    let (ip, bits) = cidr
        .split_once('/')
        .ok_or_else(|| anyhow!("invalid ipv4 cidr (缺少 /): {}", cidr))?;
    let ip = ipv4_to_u32(ip)?;
    let bits: u8 = bits
        .parse()
        .map_err(|_| anyhow!("invalid ipv4 cidr (掩码非数字): {}", cidr))?;
    if bits > 32 {
        return Err(anyhow!(
            "invalid ipv4 cidr (掩码必须在 0-32 之间): {}",
            cidr
        ));
    }
    Ok((ip, bits))
}

/// 由掩码长度计算 u32 掩码（bits=0 → 0，bits=24 → 0xFFFFFF00）。
pub fn ipv4_mask_from_prefix(bits: u8) -> u32 {
    if bits == 0 {
        0
    } else {
        u32::MAX << (32 - u32::from(bits))
    }
}

#[cfg(test)]
mod u32_tests {
    use super::*;

    #[test]
    fn ipv4_u32_roundtrip() {
        for ip in &[
            "100.99.71.1",
            "100.99.71.2",
            "100.99.71.254",
            "0.0.0.0",
            "255.255.255.255",
        ] {
            let v = ipv4_to_u32(ip).unwrap();
            assert_eq!(u32_to_ipv4(v), *ip, "roundtrip failed for {}", ip);
        }
    }

    #[test]
    fn known_values() {
        // 100.99.71.1 = 0x64*2^24 + 0x63*2^16 + 0x47*2^8 + 0x01
        assert_eq!(ipv4_to_u32("100.99.71.1").unwrap(), 0x64634701);
        assert_eq!(ipv4_to_u32("100.99.71.2").unwrap(), 0x64634702);
        assert_eq!(ipv4_to_u32("100.99.71.254").unwrap(), 0x646347FE);
    }

    #[test]
    fn parse_ipv4_cidr_valid() {
        let (ip, bits) = parse_ipv4_cidr("192.168.1.0/24").unwrap();
        assert_eq!(ip, 0xC0A80100);
        assert_eq!(bits, 24);

        let (ip, bits) = parse_ipv4_cidr("10.0.0.0/8").unwrap();
        assert_eq!(ip, 0x0A000000);
        assert_eq!(bits, 8);

        // /32 单机
        let (ip, bits) = parse_ipv4_cidr("100.99.71.5/32").unwrap();
        assert_eq!(ip, 0x64634705);
        assert_eq!(bits, 32);
    }

    #[test]
    fn parse_ipv4_cidr_invalid() {
        // 缺少 /
        assert!(parse_ipv4_cidr("192.168.1.0").is_err());
        // 掩码超限
        assert!(parse_ipv4_cidr("192.168.1.0/33").is_err());
        // 掩码非数字
        assert!(parse_ipv4_cidr("192.168.1.0/abc").is_err());
        // IP 非法
        assert!(parse_ipv4_cidr("192.168.1/24").is_err());
        assert!(parse_ipv4_cidr("999.1.1.1/24").is_err());
    }

    #[test]
    fn ipv4_mask_from_prefix_cases() {
        assert_eq!(ipv4_mask_from_prefix(0), 0x00000000);
        assert_eq!(ipv4_mask_from_prefix(8), 0xFF000000);
        assert_eq!(ipv4_mask_from_prefix(24), 0xFFFFFF00);
        assert_eq!(ipv4_mask_from_prefix(32), 0xFFFFFFFF);
    }
}

// ============ 交换载荷 ============

/// 通过 gonc 加密通道交换的载荷（JSON 序列化，serde 自动处理）。
///
/// 主动端 → 被动端：填 `pubkey`/`device_id`/`ip_range_*`，`assigned_ip`/`my_ip` 留 0。
/// 被动端 → 主动端：填 `pubkey`/`device_id`/`assigned_ip`/`my_ip`，`ip_range_*` 留 0。
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct ExchangePayload {
    /// 本端 WireGuard 公钥（base64）。
    pub pubkey: String,
    /// 本端 device_id。
    pub device_id: i64,
    /// 可用 IP 范围起始（含），u32 大端序。仅主动端→被动端方向填。
    pub ip_range_start: u32,
    /// 可用 IP 范围结束（含），u32 大端序。仅主动端→被动端方向填。
    pub ip_range_end: u32,
    /// 被动端为主动端分配的虚拟 IP，u32 大端序。仅被动端→主动端方向填。
    pub assigned_ip: u32,
    /// 被动端自己的虚拟 IP（u32）。主动端用它作为对端 IP。
    pub my_ip: u32,
    /// 本端 WireGuard UDP 监听端口。旧版本缺失时为 0。
    #[serde(default)]
    pub wg_port: u16,
    /// 本端健康/控制 TCP 监听端口。旧版本缺失时为 0。
    #[serde(default)]
    pub health_port: u16,
    /// 被动端显式开放给主动端访问的 LAN 网段（仅被动端→主动端方向使用）。
    #[serde(default)]
    pub exposed_lan_cidrs: Vec<String>,
    /// 被动端无法向本隧道分配 LAN 访问时返回给主动端的提示。
    #[serde(default)]
    pub warning: Option<String>,
}

impl ExchangePayload {
    /// 构造主动端→被动端的载荷。
    pub fn for_active(
        pubkey: impl Into<String>,
        device_id: i64,
        ip_start: u32,
        ip_end: u32,
    ) -> Self {
        Self {
            pubkey: pubkey.into(),
            device_id,
            ip_range_start: ip_start,
            ip_range_end: ip_end,
            exposed_lan_cidrs: Vec::new(),
            warning: None,
            ..Default::default()
        }
    }

    /// 序列化为 JSON 文本（作为 FFI Exchange 的 send_data）。
    pub fn render(&self) -> Result<String> {
        serde_json::to_string(self).context("serialize ExchangePayload")
    }

    /// 从 JSON 文本（FFI Exchange 返回的 recv_data）解析。
    pub fn parse(text: &str) -> Result<Self> {
        let mut payload: Self = serde_json::from_str(text).map_err(|err| {
            // ExchangePayload contains public WireGuard metadata, never the
            // private key or punch token. Keep the preview bounded and escape
            // control characters so malformed/truncated MQTT replies are
            // diagnosable from a single service-log line.
            let preview: String = text.chars().take(256).flat_map(char::escape_default).collect();
            tracing::error!(
                "[wgvpn-exchange] parse ExchangePayload JSON failed: error={}, bytes={}, preview={:?}",
                err,
                text.len(),
                preview
            );
            anyhow!(
                "parse ExchangePayload JSON: {}; bytes={}, preview={:?}",
                err,
                text.len(),
                preview
            )
        })?;
        if payload.pubkey.is_empty() {
            return Err(anyhow!("payload missing pubkey"));
        }
        validate_public_key(&payload.pubkey)?;
        payload.exposed_lan_cidrs = normalize_exposed_lan_cidrs(&payload.exposed_lan_cidrs)?;
        Ok(payload)
    }
}

pub fn normalize_exposed_lan_cidrs(cidrs: &[String]) -> Result<Vec<String>> {
    let mut seen = HashSet::new();
    let mut normalized = Vec::new();
    for raw in cidrs {
        let cidr = raw.trim();
        if cidr.is_empty() {
            continue;
        }
        let (ip_raw, prefix_raw) = cidr
            .split_once('/')
            .ok_or_else(|| anyhow!("invalid ipv4 cidr: {}", cidr))?;
        let ip = ip_raw
            .parse::<Ipv4Addr>()
            .map_err(|_| anyhow!("invalid ipv4 cidr ip: {}", cidr))?;
        let prefix = prefix_raw
            .parse::<u8>()
            .map_err(|_| anyhow!("invalid ipv4 cidr prefix: {}", cidr))?;
        if prefix > 32 {
            return Err(anyhow!("invalid ipv4 cidr prefix: {}", cidr));
        }
        let ip = u32::from(ip);
        let mask = if prefix == 0 {
            0
        } else {
            u32::MAX << (32 - u32::from(prefix))
        };
        let value = format!("{}/{}", Ipv4Addr::from(ip & mask), prefix);
        if seen.insert(value.clone()) {
            normalized.push(value);
        }
    }
    Ok(normalized)
}

#[cfg(test)]
mod payload_tests {
    use super::*;

    #[test]
    fn payload_roundtrip_active() {
        let mut p = ExchangePayload::for_active(
            "MDEyMzQ1Njc4OWFiY2RlZjAxMjM0NTY3ODlhYmNkZWY=",
            12345,
            0x64634702,
            0x646347FE,
        );
        p.wg_port = 41118;
        p.health_port = 41119;
        let text = p.render().unwrap();
        let p2 = ExchangePayload::parse(&text).unwrap();
        assert_eq!(p2.pubkey, p.pubkey);
        assert_eq!(p2.device_id, 12345);
        assert_eq!(p2.ip_range_start, 0x64634702);
        assert_eq!(p2.ip_range_end, 0x646347FE);
        assert_eq!(p2.wg_port, 41118);
        assert_eq!(p2.health_port, 41119);
    }

    #[test]
    fn payload_roundtrip_passive() {
        // 与生产被动端一致：直接用结构体字面量构造（wgvpn_flow.rs 的
        // local_payload_template），assigned_ip/my_ip 在协商回调后填充。
        let mut p = ExchangePayload {
            pubkey: "MDEyMzQ1Njc4OWFiY2RlZjAxMjM0NTY3ODlhYmNkZWY=".to_string(),
            device_id: 67890,
            assigned_ip: 0x64634705,
            my_ip: 0x64634701,
            exposed_lan_cidrs: vec!["192.168.10.0/24".to_string()],
            warning: None,
            ..Default::default()
        };
        p.warning = Some("LAN访问功能已被其他隧道占用，本隧道不支持LAN访问".to_string());
        let text = p.render().unwrap();
        let p2 = ExchangePayload::parse(&text).unwrap();
        assert_eq!(p2.assigned_ip, 0x64634705);
        assert_eq!(p2.my_ip, 0x64634701);
        assert_eq!(p2.exposed_lan_cidrs, vec!["192.168.10.0/24"]);
        assert_eq!(p2.warning, p.warning);
    }

    #[test]
    fn exposed_lan_cidr_is_normalized_to_network_address() {
        let normalized = normalize_exposed_lan_cidrs(&["192.0.2.181/24".to_string()]).unwrap();
        assert_eq!(normalized, vec!["192.0.2.0/24"]);
    }

    #[test]
    fn payload_missing_pubkey_rejected() {
        let json = r#"{"pubkey":"","device_id":1,"ip_range_start":0,"ip_range_end":0,"assigned_ip":0,"my_ip":0,"exposed_lan_cidrs":[]}"#;
        assert!(ExchangePayload::parse(json).is_err());
    }

    #[test]
    fn invalid_exposed_lan_cidr_rejected() {
        let json = r#"{"pubkey":"MDEyMzQ1Njc4OWFiY2RlZjAxMjM0NTY3ODlhYmNkZWY=","device_id":1,"ip_range_start":0,"ip_range_end":0,"assigned_ip":0,"my_ip":0,"exposed_lan_cidrs":["192.168.10.0/99"]}"#;
        assert!(ExchangePayload::parse(json).is_err());
    }

    #[test]
    fn legacy_payload_without_ports_defaults_to_zero() {
        let json = r#"{"pubkey":"MDEyMzQ1Njc4OWFiY2RlZjAxMjM0NTY3ODlhYmNkZWY=","device_id":1,"ip_range_start":1684227842,"ip_range_end":1684228094,"assigned_ip":0,"my_ip":0,"exposed_lan_cidrs":[]}"#;
        let payload = ExchangePayload::parse(json).unwrap();
        assert_eq!(payload.wg_port, 0);
        assert_eq!(payload.health_port, 0);
    }
}

// ============ 双向交换（主动端 / 被动端） ============

/// 派生协商用 token（避免与 UDP 数据面 token 完全相同，隔离 topic）。
///
/// gonc 的 MQTT_ExchangePayload 内部用 salt 前缀 "wgvpn-kx/" 已与打洞 topic 隔离，
/// 这里再加一个 token 后缀，作为双保险。
pub fn derive_kx_token(base_token: &str) -> String {
    format!("{}-kx", base_token)
}

/// 主动端执行交换：Mutual 模式一次调用，发出本端载荷并接收对端载荷，
/// 返回被动端回传的载荷（含 assigned_ip / my_ip）。
#[cfg(any(target_os = "linux", windows, target_os = "macos"))]
pub async fn exchange_as_active(
    base_token: &str,
    local_payload: &ExchangePayload,
    timeout: Duration,
) -> Result<ExchangePayload> {
    let kx_token = derive_kx_token(base_token);
    let send_data = local_payload.render()?;
    let recv = crate::gonc_ffi::exchange_payload_native(
        &kx_token,
        crate::gonc_ffi::ExchangeMode::Mutual,
        &send_data,
        timeout,
    )
    .await?;
    ExchangePayload::parse(&recv)
}

/// 被动端执行交换：先 WaitOnly 收主动端载荷，由调用方分配 IP，再 Reply 回传。
///
/// `allocate` 闭包接收（对端 device_id, IP 范围），返回分配的 (assigned_ip, my_ip)。
/// 这样被动端的 IP 分配策略由 p2p.rs 决定，本模块只负责传输。
#[cfg(any(target_os = "linux", windows, target_os = "macos"))]
pub async fn exchange_as_passive<F>(
    base_token: &str,
    local_payload_template: &ExchangePayload,
    timeout: Duration,
    allocate: F,
) -> Result<ExchangePayload>
where
    F: FnOnce(i64, u32, u32, u32) -> Result<(u32, u32)>,
{
    let kx_token = derive_kx_token(base_token);
    let recv = crate::gonc_ffi::exchange_payload_native(
        &kx_token,
        crate::gonc_ffi::ExchangeMode::WaitOnly,
        "",
        timeout,
    )
    .await?;
    let active_payload = ExchangePayload::parse(&recv)?;
    let (assigned_ip, my_ip) = allocate(
        active_payload.device_id,
        active_payload.ip_range_start,
        active_payload.ip_range_end,
        active_payload.my_ip,
    )?;
    let passive_payload = ExchangePayload {
        pubkey: local_payload_template.pubkey.clone(),
        device_id: local_payload_template.device_id,
        ip_range_start: 0,
        ip_range_end: 0,
        assigned_ip,
        my_ip,
        wg_port: local_payload_template.wg_port,
        health_port: local_payload_template.health_port,
        exposed_lan_cidrs: local_payload_template.exposed_lan_cidrs.clone(),
        warning: local_payload_template.warning.clone(),
    };
    let send_data = passive_payload.render()?;
    let _ = crate::gonc_ffi::exchange_payload_native(
        &kx_token,
        crate::gonc_ffi::ExchangeMode::Mutual,
        &send_data,
        timeout,
    )
    .await?;
    Ok(active_payload)
}

#[cfg(test)]
mod exchange_tests {
    use super::*;

    #[test]
    fn kx_token_appends_suffix() {
        assert_eq!(derive_kx_token("abc"), "abc-kx");
    }

    #[test]
    fn parse_error_includes_serde_cause_and_bounded_preview() {
        let malformed = format!(r#"{{"pubkey": "{}""#, "x".repeat(300));
        let error = ExchangePayload::parse(&malformed).unwrap_err().to_string();
        assert!(error.contains("parse ExchangePayload JSON:"));
        assert!(error.contains("bytes="));
        assert!(error.contains("preview="));
        assert!(!error.contains(&"x".repeat(257)));
    }
}
