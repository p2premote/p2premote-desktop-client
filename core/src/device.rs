use crate::auth_http::{send_authed, AuthedResponse};
use crate::config::{save_machine_config, MachineConfig};
use crate::http::{shared_client, ApiResponse};
use crate::i18n::localized_message;
use anyhow::{anyhow, Result};
use futures_util::{stream::FuturesUnordered, StreamExt};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::net::IpAddr;
use std::sync::OnceLock;
use tracing::{debug, info};

// 平台特定实现按 Go 的文件命名习惯拆分（device_windows.rs / device_macos.rs /
// device_linux.rs）。Rust 不按文件名自动选择，构建门控在这里集中声明：
// 纯函数所在模块附带 `test` 条件，保证单测可在任意平台运行。
#[cfg(any(windows, test))]
mod device_windows;
#[cfg(target_os = "macos")]
mod device_macos;
#[cfg(any(target_os = "linux", test))]
mod device_linux;

#[cfg(windows)]
pub(crate) use device_windows::{get_rdp_port_from_registry, is_rdp_enabled};
#[cfg(windows)]
use device_windows::{current_remote_access, get_system_version};
#[cfg(target_os = "macos")]
pub(crate) use device_macos::get_rdp_port_from_registry;
#[cfg(target_os = "macos")]
use device_macos::{current_remote_access, get_system_version};
#[cfg(target_os = "linux")]
pub(crate) use device_linux::{get_rdp_port_from_registry, is_rdp_enabled};
#[cfg(target_os = "linux")]
use device_linux::{current_remote_access, get_system_version};

fn current_device_type() -> String {
    match std::env::consts::OS {
        "windows" => "Windows",
        "linux" => "Linux",
        "macos" => "macOS",
        other => other,
    }
    .to_string()
}

fn current_client_version() -> String {
    resolve_client_version(
        option_env!("P2PREMOTE_CLIENT_VERSION"),
        std::env::var("P2PREMOTE_CLIENT_VERSION").ok(),
    )
}

fn resolve_client_version(compiled: Option<&str>, runtime: Option<String>) -> String {
    compiled
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(str::to_string)
        .or_else(|| {
            runtime
                .map(|value| value.trim().to_string())
                .filter(|value| !value.is_empty())
        })
        .unwrap_or_else(|| "unknown".to_string())
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RemoteAccessInfo {
    pub protocol: String,
    pub enabled: bool,
    pub port: u16,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DeviceInfo {
    pub device_id: i64,
    pub device_uuid: String,
    pub device_name: String,
    #[serde(default)]
    pub device_alias: Option<String>,
    pub device_type: String,
    pub status: String,
    #[serde(default)]
    pub lan_ip: Option<String>,
    #[serde(default)]
    pub rdp_enabled: bool,
    #[serde(default)]
    pub rdp_port: u16,
    #[serde(default)]
    pub remote_access: Option<RemoteAccessInfo>,
    #[serde(default)]
    pub public_ip: Option<String>,
    #[serde(default)]
    pub public_ip_location: Option<String>,
    pub system_version: String,
    #[serde(default)]
    pub client_version: String,
    #[serde(default)]
    pub service_port: i64,
    #[serde(default)]
    pub connect_code: Option<String>,
    #[serde(default)]
    pub created_at: Option<String>,
}

#[derive(Serialize)]
struct RegisterRequest {
    device_name: String,
    device_type: String,
    #[serde(rename = "system_version")]
    system_version: String,
    client_version: String,
    #[serde(rename = "device_uuid")]
    device_uuid: String,
    #[serde(rename = "lan_ip")]
    lan_ip: String,
    #[serde(rename = "public_ip")]
    public_ip: String,
    public_ip_location: String,
    #[serde(rename = "service_port")]
    service_port: i64,
    #[serde(rename = "rdp_enabled")]
    rdp_enabled: bool,
    #[serde(rename = "rdp_port")]
    rdp_port: u16,
    #[serde(skip_serializing_if = "Option::is_none")]
    remote_access: Option<RemoteAccessInfo>,
}

/// 业务响应 envelope 复用 http::ApiResponse<T>，按 data 类型别名。
type RegisterResponse = ApiResponse<DeviceInfo>;
type DeviceListResponse = ApiResponse<Vec<DeviceInfo>>;
type ConnectCodeResponse = ApiResponse<Option<ConnectCodeData>>;
type AnonymousConnectResponse = ApiResponse<Option<AnonymousConnectData>>;
/// 仅判定 code 的通用响应（data 不读取，仅反序列化兼容）。
type GenericResponse = ApiResponse<Option<serde_json::Value>>;

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct DeviceStatusReport {
    #[serde(rename = "device_id")]
    device_id: i64,
    #[serde(rename = "device_uuid")]
    device_uuid: String,
    lan_ip: String,
    public_ip: String,
    public_ip_location: String,
    service_port: i64,
    rdp_enabled: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    remote_access: Option<RemoteAccessInfo>,
    client_version: String,
}

#[derive(Serialize)]
struct SetConnectPasswordRequest {
    password: String,
}

#[derive(Serialize)]
struct UpdateAliasRequest {
    alias: String,
}

#[derive(Serialize)]
struct UpdateDeviceInfoRequest {
    lan_ip: String,
    public_ip: String,
    public_ip_location: String,
    service_port: i64,
    client_version: String,
}

#[derive(Serialize)]
struct OfflineRequest {
    device_id: i64,
}

/// 连接码响应的 data 载荷。
#[derive(Deserialize)]
struct ConnectCodeData {
    connect_code: String,
}

#[derive(Serialize)]
struct AnonymousConnectRequest {
    #[serde(rename = "connect_code")]
    connect_code: String,
    password: String,
}

#[derive(Deserialize)]
struct AnonymousConnectData {
    device: DeviceInfo,
}

/// 从 AuthedResponse 解析通用业务响应：code != 0 视为失败。
fn parse_basic(resp: AuthedResponse) -> Result<GenericResponse> {
    let parsed: GenericResponse = serde_json::from_str(&resp.body)
        .map_err(|e| anyhow!("failed to decode response body={}: {}", resp.body, e))?;
    Ok(parsed)
}

/// 构造服务器 URL。
fn server_url(config: &MachineConfig, path: &str) -> String {
    format!("{}{}", config.server_url.trim_end_matches('/'), path)
}

pub async fn register_current_device_auto(config: &mut MachineConfig) -> Result<DeviceInfo> {
    let hostname = hostname::get()
        .map(|h| h.to_string_lossy().to_string())
        .unwrap_or_else(|_| "Unknown".to_string());

    let lan_ip = local_ip_address::local_ip()
        .map(|ip| ip.to_string())
        .unwrap_or_else(|_| "127.0.0.1".to_string());

    if crate::device_identity::ensure_device_uuid(config) {
        // 设备身份必须先落盘再发起网络注册，避免响应丢失后生成另一个 UUID。
        crate::config::save_machine_config(config)?;
    }
    let device_uuid = config
        .device_uuid
        .clone()
        .ok_or_else(|| anyhow!("device uuid missing after initialization"))?;

    let remote_access = current_remote_access();
    let rdp_port = remote_access
        .as_ref()
        .map(|access| access.port)
        .unwrap_or_else(get_rdp_port_from_registry);
    let rdp_enabled = is_rdp_enabled();
    let public_network = refresh_public_network_info(config, false).await;
    let system_version = get_system_version();

    let url = server_url(config, "/api/v1/devices");
    let request = RegisterRequest {
        device_name: hostname,
        device_type: current_device_type(),
        system_version,
        client_version: current_client_version(),
        device_uuid: device_uuid.clone(),
        lan_ip,
        public_ip: public_network.ip,
        public_ip_location: public_network.location,
        service_port: rdp_port as i64,
        rdp_enabled,
        rdp_port,
        remote_access,
    };
    let client = shared_client();
    let resp = send_authed(config, |token| {
        client
            .post(&url)
            .header("Authorization", format!("Bearer {}", token))
            .json(&request)
    })
    .await?;
    let parsed: RegisterResponse = serde_json::from_str(&resp.body)
        .map_err(|e| anyhow!("invalid register response body={}: {}", resp.body, e))?;
    if parsed.code != 0 {
        return Err(anyhow!(
            parsed.localized_error_message(config.locale.as_deref())
        ));
    }

    config.device_id = Some(parsed.data.device_id);
    config.device_uuid = Some(parsed.data.device_uuid.clone());
    save_machine_config(config)?;
    Ok(parsed.data)
}

pub async fn collect_device_status_report(
    config: &mut MachineConfig,
) -> Result<DeviceStatusReport> {
    let device_id = config
        .device_id
        .ok_or_else(|| anyhow!("device not registered"))?;
    let device_uuid = config
        .device_uuid
        .clone()
        .ok_or_else(|| anyhow!("device uuid missing"))?;

    let lan_ip = local_ip_address::local_ip()
        .map(|ip| ip.to_string())
        .unwrap_or_else(|_| "127.0.0.1".to_string());
    let public_network = refresh_public_network_info(config, false).await;
    let remote_access = current_remote_access();
    let service_port = remote_access
        .as_ref()
        .map(|access| access.port as i64)
        .unwrap_or_else(|| get_rdp_port_from_registry() as i64);
    let rdp_enabled = is_rdp_enabled();

    Ok(DeviceStatusReport {
        device_id,
        device_uuid,
        lan_ip,
        public_ip: public_network.ip,
        public_ip_location: public_network.location,
        service_port,
        rdp_enabled,
        remote_access,
        client_version: current_client_version(),
    })
}

pub async fn send_device_status_report(
    config: &mut MachineConfig,
    request: &DeviceStatusReport,
) -> Result<()> {
    let url = server_url(config, "/api/v1/devices/status-report");
    let client = shared_client();
    let resp = send_authed(config, |token| {
        client
            .post(&url)
            .header("Authorization", format!("Bearer {}", token))
            .json(&request)
    })
    .await?;
    let parsed = parse_basic(resp)?;
    if parsed.code != 0 {
        return Err(anyhow!(
            parsed.localized_error_message(config.locale.as_deref())
        ));
    }
    Ok(())
}

pub async fn set_connect_password(config: &mut MachineConfig, password: &str) -> Result<()> {
    let device_id = config
        .device_id
        .ok_or_else(|| anyhow!("device not registered"))?;

    let url = server_url(
        config,
        &format!("/api/v1/devices/{}/connect-password", device_id),
    );
    let request = SetConnectPasswordRequest {
        password: password.to_string(),
    };
    let client = shared_client();
    let resp = send_authed(config, |token| {
        client
            .post(&url)
            .header("Authorization", format!("Bearer {}", token))
            .json(&request)
    })
    .await?;
    let parsed = parse_basic(resp)?;
    if parsed.code != 0 {
        return Err(anyhow!(
            parsed.localized_error_message(config.locale.as_deref())
        ));
    }
    Ok(())
}

/// 获取当前账号下的设备列表。
pub async fn get_device_list(config: &mut MachineConfig) -> Result<Vec<DeviceInfo>> {
    let url = server_url(config, "/api/v1/devices");
    let client = shared_client();
    let resp = send_authed(config, |token| {
        client
            .get(&url)
            .header("Authorization", format!("Bearer {}", token))
    })
    .await?;
    let parsed: DeviceListResponse = serde_json::from_str(&resp.body)
        .map_err(|e| anyhow!("invalid device list response body={}: {}", resp.body, e))?;
    if parsed.code != 0 {
        return Err(anyhow!(
            parsed.localized_error_message(config.locale.as_deref())
        ));
    }
    Ok(parsed.data)
}

/// 匿名连接校验：设备代码 + 临时密码，无 Bearer token。
pub async fn anonymous_connect(
    config: &MachineConfig,
    connect_code: String,
    password: String,
) -> Result<DeviceInfo> {
    let url = server_url(config, "/api/v1/anonymous/connect");
    let client = shared_client();
    let resp = client
        .post(&url)
        .json(&AnonymousConnectRequest {
            connect_code,
            password,
        })
        .send()
        .await?;
    let parsed: AnonymousConnectResponse = resp
        .json()
        .await
        .map_err(|err| anyhow!("invalid anonymous connect response: {}", err))?;
    if parsed.code != 0 {
        return Err(anyhow!(
            parsed.localized_error_message(config.locale.as_deref())
        ));
    }
    parsed
        .data
        .map(|data| data.device)
        .ok_or_else(|| anyhow!("anonymous connect response missing device"))
}

/// 更新设备别名。
pub async fn update_device_alias(
    config: &mut MachineConfig,
    device_id: i64,
    alias: String,
) -> Result<()> {
    let url = server_url(config, &format!("/api/v1/devices/{}/alias", device_id));
    let request = UpdateAliasRequest { alias };
    let client = shared_client();
    let resp = send_authed(config, |token| {
        client
            .put(&url)
            .header("Authorization", format!("Bearer {}", token))
            .json(&request)
    })
    .await?;
    let parsed = parse_basic(resp)?;
    if parsed.code != 0 {
        return Err(anyhow!(
            parsed.localized_error_message(config.locale.as_deref())
        ));
    }
    Ok(())
}

/// 删除设备。
pub async fn delete_device(config: &mut MachineConfig, device_id: i64) -> Result<()> {
    let url = server_url(config, &format!("/api/v1/devices/{}/delete", device_id));
    let client = shared_client();
    let resp = send_authed(config, |token| {
        client
            .post(&url)
            .header("Authorization", format!("Bearer {}", token))
    })
    .await?;
    let parsed = parse_basic(resp)?;
    if parsed.code != 0 {
        return Err(anyhow!(
            parsed.localized_error_message(config.locale.as_deref())
        ));
    }
    Ok(())
}

/// 更新设备的网络信息（lan_ip / public_ip / service_port）。
pub async fn update_device_info(
    config: &mut MachineConfig,
    device_id: i64,
    lan_ip: String,
    public_ip: String,
    service_port: i64,
) -> Result<()> {
    let url = server_url(config, &format!("/api/v1/devices/{}/info", device_id));
    let request = UpdateDeviceInfoRequest {
        lan_ip,
        public_ip,
        public_ip_location: config.cached_public_ip_location.clone().unwrap_or_default(),
        service_port,
        client_version: current_client_version(),
    };
    let client = shared_client();
    let resp = send_authed(config, |token| {
        client
            .put(&url)
            .header("Authorization", format!("Bearer {}", token))
            .json(&request)
    })
    .await?;
    let parsed = parse_basic(resp)?;
    if parsed.code != 0 {
        return Err(anyhow!(
            parsed.localized_error_message(config.locale.as_deref())
        ));
    }
    Ok(())
}

/// 设置指定设备的连接密码（可指定任意 device_id，区别于本机的 set_connect_password）。
pub async fn set_device_password(
    config: &mut MachineConfig,
    device_id: i64,
    password: String,
) -> Result<()> {
    let url = server_url(
        config,
        &format!("/api/v1/devices/{}/connect-password", device_id),
    );
    let request = SetConnectPasswordRequest { password };
    let client = shared_client();
    let resp = send_authed(config, |token| {
        client
            .post(&url)
            .header("Authorization", format!("Bearer {}", token))
            .json(&request)
    })
    .await?;
    let parsed = parse_basic(resp)?;
    if parsed.code != 0 {
        return Err(anyhow!(
            parsed.localized_error_message(config.locale.as_deref())
        ));
    }
    Ok(())
}

/// 生成指定设备的连接码。
pub async fn generate_connect_code(config: &mut MachineConfig, device_id: i64) -> Result<String> {
    let url = server_url(
        config,
        &format!("/api/v1/devices/{}/connect-code", device_id),
    );
    let client = shared_client();
    let resp = send_authed(config, |token| {
        client
            .post(&url)
            .header("Authorization", format!("Bearer {}", token))
    })
    .await?;
    let parsed: ConnectCodeResponse = serde_json::from_str(&resp.body)
        .map_err(|e| anyhow!("invalid connect code response body={}: {}", resp.body, e))?;
    if parsed.code != 0 {
        return Err(anyhow!(
            parsed.localized_error_message(config.locale.as_deref())
        ));
    }
    parsed.data.map(|d| d.connect_code).ok_or_else(|| {
        anyhow!(localized_message(
            config.locale.as_deref(),
            "errors.no_connect_code_data",
            &[],
        ))
    })
}

/// 标记当前设备为离线。
pub async fn mark_current_device_offline(config: &mut MachineConfig) -> Result<()> {
    let device_id = config
        .device_id
        .ok_or_else(|| anyhow!("device not registered"))?;
    let url = server_url(config, "/api/v1/devices/offline");
    let request = OfflineRequest { device_id };
    let client = shared_client();
    let resp = send_authed(config, |token| {
        client
            .post(&url)
            .header("Authorization", format!("Bearer {}", token))
            .json(&request)
    })
    .await?;
    let parsed = parse_basic(resp)?;
    if parsed.code != 0 {
        return Err(anyhow!(
            parsed.localized_error_message(config.locale.as_deref())
        ));
    }
    Ok(())
}

pub fn get_current_device_uuid(config: &MachineConfig) -> String {
    if let Some(uuid) = &config.device_uuid {
        return uuid.clone();
    }
    crate::device_identity::new_device_uuid()
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct PublicNetworkInfo {
    pub ip: String,
    pub location: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum PublicNetworkGroup {
    China,
    Global,
}

static PUBLIC_NETWORK_GROUP: OnceLock<PublicNetworkGroup> = OnceLock::new();

/// 服务启动时固定服务组；运行中不随语言设置变化而切换。
pub fn initialize_public_network_group(locale: Option<&str>) {
    let group = match locale {
        Some(value) if value.starts_with("zh") => PublicNetworkGroup::China,
        _ => PublicNetworkGroup::Global,
    };
    info!(
        "[PublicNetwork] startup provider group selected: group={:?}, locale={:?}",
        group, locale
    );
    let _ = PUBLIC_NETWORK_GROUP.set(group);
}

fn public_network_group() -> PublicNetworkGroup {
    PUBLIC_NETWORK_GROUP
        .get()
        .copied()
        .expect("public network group must be initialized during service startup")
}

trait PublicNetworkProvider: Send + Sync {
    fn name(&self) -> &'static str;
    fn endpoint(&self) -> &'static str;
    fn parse(&self, body: &[u8]) -> Option<PublicNetworkInfo>;
}

struct IpWhoIsProvider;
impl PublicNetworkProvider for IpWhoIsProvider {
    fn name(&self) -> &'static str {
        "ipwho.is"
    }
    fn endpoint(&self) -> &'static str {
        "https://ipwho.is/?lang=en"
    }
    fn parse(&self, body: &[u8]) -> Option<PublicNetworkInfo> {
        let value: Value = serde_json::from_slice(body).ok()?;
        if value.get("success").and_then(Value::as_bool) == Some(false) {
            return None;
        }
        public_network_from_parts(
            value.get("ip")?.as_str()?,
            &[
                value.get("country")?.as_str()?,
                value.get("region").and_then(Value::as_str).unwrap_or(""),
                value.get("city").and_then(Value::as_str).unwrap_or(""),
            ],
            value.pointer("/connection/isp").and_then(Value::as_str),
        )
    }
}

struct CipCcProvider;
impl PublicNetworkProvider for CipCcProvider {
    fn name(&self) -> &'static str {
        "cip.cc"
    }
    fn endpoint(&self) -> &'static str {
        "https://cip.cc/"
    }
    fn parse(&self, body: &[u8]) -> Option<PublicNetworkInfo> {
        let body = std::str::from_utf8(body).ok()?;
        let mut ip = None;
        let mut chinese = None;
        for line in body.lines() {
            let Some((label, value)) = line.split_once(':') else {
                continue;
            };
            let value = value.trim();
            match label.trim() {
                "IP" => ip = Some(value),
                "数据二" => chinese = Some(format_cip_cc_location(value)),
                _ => {}
            }
        }
        let ip = ip?;
        ip.parse::<IpAddr>().ok()?;
        // cip.cc 在中文服务组中仅接受“数据二”；缺失时让其他国内接口继续竞速，
        // 避免用英文“数据三”污染中文设备上报的位置文本。
        let location = chinese?;
        (!location.is_empty()).then(|| PublicNetworkInfo {
            ip: ip.to_string(),
            location: location.to_string(),
        })
    }
}

struct MyIpIpipProvider;
impl PublicNetworkProvider for MyIpIpipProvider {
    fn name(&self) -> &'static str {
        "myip.ipip.net"
    }
    fn endpoint(&self) -> &'static str {
        "https://myip.ipip.net"
    }
    fn parse(&self, body: &[u8]) -> Option<PublicNetworkInfo> {
        let body = std::str::from_utf8(body).ok()?;
        let body = body.trim();
        let ip = body.split('：').nth(1)?.split_whitespace().next()?;
        let location = body.split("来自于：").nth(1)?.trim();
        ip.parse::<IpAddr>().ok()?;
        (!location.is_empty()).then(|| PublicNetworkInfo {
            ip: ip.to_string(),
            location: format_whitespace_location(location),
        })
    }
}

struct PConlineProvider;
impl PublicNetworkProvider for PConlineProvider {
    fn name(&self) -> &'static str {
        "whois.pconline.com.cn"
    }
    fn endpoint(&self) -> &'static str {
        "https://whois.pconline.com.cn/ipJson.jsp?json=true"
    }
    fn parse(&self, body: &[u8]) -> Option<PublicNetworkInfo> {
        let (body, _, had_errors) = encoding_rs::GBK.decode(body);
        if had_errors {
            return None;
        }
        let value: Value = serde_json::from_str(&body).ok()?;
        let ip = value.get("ip")?.as_str()?;
        let province = value.get("pro").and_then(Value::as_str).unwrap_or("");
        let city = value.get("city").and_then(Value::as_str).unwrap_or("");
        let address = value.get("addr").and_then(Value::as_str).unwrap_or("");
        pconline_public_network(ip, province, city, address)
    }
}

struct IpInfoProvider;
impl PublicNetworkProvider for IpInfoProvider {
    fn name(&self) -> &'static str {
        "ipinfo.io"
    }
    fn endpoint(&self) -> &'static str {
        "https://ipinfo.io/json"
    }
    fn parse(&self, body: &[u8]) -> Option<PublicNetworkInfo> {
        let value: Value = serde_json::from_slice(body).ok()?;
        public_network_from_parts(
            value.get("ip")?.as_str()?,
            &[
                value.get("country").and_then(Value::as_str).unwrap_or(""),
                value.get("region").and_then(Value::as_str).unwrap_or(""),
                value.get("city").and_then(Value::as_str).unwrap_or(""),
            ],
            value.get("org").and_then(Value::as_str),
        )
    }
}

struct IfconfigCoProvider;
impl PublicNetworkProvider for IfconfigCoProvider {
    fn name(&self) -> &'static str {
        "ifconfig.co"
    }
    fn endpoint(&self) -> &'static str {
        "https://ifconfig.co/json"
    }
    fn parse(&self, body: &[u8]) -> Option<PublicNetworkInfo> {
        let value: Value = serde_json::from_slice(body).ok()?;
        public_network_from_parts(
            value.get("ip")?.as_str()?,
            &[
                value.get("country").and_then(Value::as_str).unwrap_or(""),
                value
                    .get("region_name")
                    .and_then(Value::as_str)
                    .unwrap_or(""),
                value.get("city").and_then(Value::as_str).unwrap_or(""),
            ],
            value.get("asn_org").and_then(Value::as_str),
        )
    }
}

fn public_network_from_parts(
    ip: &str,
    location_parts: &[&str],
    isp: Option<&str>,
) -> Option<PublicNetworkInfo> {
    ip.parse::<IpAddr>().ok()?;
    let mut parts: Vec<&str> = location_parts
        .iter()
        .copied()
        .filter(|part| !part.trim().is_empty())
        .collect();
    if parts.is_empty() {
        return None;
    }
    if let Some(isp) = isp.filter(|value| !value.trim().is_empty()) {
        parts.push(isp);
    }
    Some(PublicNetworkInfo {
        ip: ip.to_string(),
        location: parts.join(" · "),
    })
}

fn format_whitespace_location(value: &str) -> String {
    value
        .split_whitespace()
        .filter(|part| !part.is_empty())
        .collect::<Vec<_>>()
        .join(" · ")
}

fn format_cip_cc_location(value: &str) -> String {
    let mut parts = value
        .split('|')
        .map(str::trim)
        .filter(|part| !part.is_empty());
    let location = parts
        .next()
        .map(format_whitespace_location)
        .unwrap_or_default();
    let isp = parts.next().unwrap_or_default();
    match (location.is_empty(), isp.is_empty()) {
        (false, false) => format!("{location} · {isp}"),
        (false, true) => location,
        (true, false) => isp.to_string(),
        (true, true) => String::new(),
    }
}

fn pconline_public_network(
    ip: &str,
    province: &str,
    city: &str,
    address: &str,
) -> Option<PublicNetworkInfo> {
    ip.parse::<IpAddr>().ok()?;
    let prefix = format!("{}{}", province.trim(), city.trim());
    let isp = address
        .trim()
        .strip_prefix(&prefix)
        .unwrap_or(address)
        .trim();
    let location =
        public_network_from_parts(ip, &[province, city], (!isp.is_empty()).then_some(isp))?;
    Some(location)
}

/// 并发访问异构公共接口；每个 provider 自己负责格式适配，首个完整有效结果胜出。
pub async fn get_public_network_info() -> Option<PublicNetworkInfo> {
    get_public_network_info_for_group(public_network_group()).await
}

async fn get_public_network_info_for_group(group: PublicNetworkGroup) -> Option<PublicNetworkInfo> {
    static IPWHOIS: IpWhoIsProvider = IpWhoIsProvider;
    static CIPCC: CipCcProvider = CipCcProvider;
    static IPIP: MyIpIpipProvider = MyIpIpipProvider;
    static PCONLINE: PConlineProvider = PConlineProvider;
    static IPINFO: IpInfoProvider = IpInfoProvider;
    static IFCONFIG: IfconfigCoProvider = IfconfigCoProvider;
    let providers: Vec<&dyn PublicNetworkProvider> = match group {
        PublicNetworkGroup::China => vec![&IPIP, &CIPCC, &PCONLINE],
        PublicNetworkGroup::Global => vec![&IPWHOIS, &IPINFO, &IFCONFIG],
    };
    query_first_public_network(&providers).await
}

async fn query_first_public_network(
    providers: &[&dyn PublicNetworkProvider],
) -> Option<PublicNetworkInfo> {
    let client = shared_client();
    let mut requests = FuturesUnordered::new();
    for provider in providers.iter().copied() {
        let client = client.clone();
        requests.push(async move {
            let response = tokio::time::timeout(
                std::time::Duration::from_secs(3),
                client.get(provider.endpoint()).send(),
            )
            .await
            .ok()?
            .ok()?;
            let status = response.status();
            let content_type = response
                .headers()
                .get(reqwest::header::CONTENT_TYPE)
                .and_then(|value| value.to_str().ok())
                .unwrap_or("<missing>")
                .to_string();
            if !response.status().is_success() {
                debug!(
                    "[PublicNetwork] provider response rejected: provider={}, status={}, content_type={}",
                    provider.name(), status, content_type
                );
                return None;
            }
            let body = response.bytes().await.ok()?;
            match provider.parse(&body) {
                Some(info) => {
                    info!(
                        "[PublicNetwork] provider selected: provider={}, status={}, content_type={}, bytes={}, ip={}, location={}",
                        provider.name(), status, content_type, body.len(), info.ip, info.location
                    );
                    Some(info)
                }
                None => {
                    debug!(
                        "[PublicNetwork] provider response parse failed: provider={}, status={}, content_type={}, bytes={}",
                        provider.name(), status, content_type, body.len()
                    );
                    None
                }
            }
        });
    }
    tokio::time::timeout(std::time::Duration::from_secs(5), async {
        while let Some(result) = requests.next().await {
            if result.is_some() {
                return result;
            }
        }
        None
    })
    .await
    .ok()
    .flatten()
}

pub async fn refresh_public_network_info(
    config: &mut MachineConfig,
    force: bool,
) -> PublicNetworkInfo {
    const REFRESH_INTERVAL_SECS: i64 = 300;
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|duration| duration.as_secs() as i64)
        .unwrap_or_default();
    if !force && now - config.cached_public_network_checked_at < REFRESH_INTERVAL_SECS {
        if let Some(ip) = &config.cached_public_ip {
            return PublicNetworkInfo {
                ip: ip.clone(),
                location: config.cached_public_ip_location.clone().unwrap_or_default(),
            };
        }
    }
    let result = get_public_network_info_for_group(public_network_group())
        .await
        .unwrap_or_default();
    if config.cached_public_ip.as_deref() != Some(result.ip.as_str())
        || config.cached_public_ip_location.as_deref() != Some(result.location.as_str())
        || config.cached_public_network_checked_at != now
    {
        config.cached_public_ip = (!result.ip.is_empty()).then(|| result.ip.clone());
        config.cached_public_ip_location =
            (!result.location.is_empty()).then(|| result.location.clone());
        config.cached_public_network_checked_at = now;
        if let Err(error) = save_machine_config(config) {
            debug!(
                "[PublicNetwork] failed to persist current network info: {}",
                error
            );
        }
    }
    result
}

#[cfg(any(target_os = "macos", target_os = "linux"))]
use std::net::{SocketAddr, TcpStream};
#[cfg(any(target_os = "macos", target_os = "linux"))]
use std::time::Duration;

// macOS 与 Linux（TigerVNC/wayvnc 等）共用本地端口探测辅助。
#[cfg(any(target_os = "macos", target_os = "linux"))]
fn is_local_port_open(port: u16) -> bool {
    [
        SocketAddr::from(([127, 0, 0, 1], port)),
        SocketAddr::from(([0, 0, 0, 0, 0, 0, 0, 1], port)),
    ]
    .iter()
    .any(|address| TcpStream::connect_timeout(address, Duration::from_millis(250)).is_ok())
}

// 未支持平台的兜底实现（与拆分前的 not(any(...)) 分支一致）。
#[cfg(not(any(windows, target_os = "macos", target_os = "linux")))]
fn get_system_version() -> String {
    std::env::consts::OS.to_string()
}

#[cfg(not(any(windows, target_os = "macos", target_os = "linux")))]
pub(crate) fn get_rdp_port_from_registry() -> u16 {
    3389
}

#[cfg(not(any(windows, target_os = "linux")))]
pub(crate) fn is_rdp_enabled() -> bool {
    false
}

#[cfg(not(any(windows, target_os = "macos", target_os = "linux")))]
fn current_remote_access() -> Option<RemoteAccessInfo> {
    None
}

#[cfg(test)]
mod tests {
    use super::*;
    use super::device_linux::parse_linux_system_version;
    use super::device_windows::normalize_windows_product_name;
    #[cfg(windows)]
    use super::device_windows::get_windows_version_from_wmi;

    #[test]
    fn formats_pconline_without_repeating_province_and_city() {
        let info =
            pconline_public_network("112.19.146.175", "四川省", "达州市", "四川省达州市 移通")
                .expect("PConline response should parse");
        assert_eq!(info.location, "四川省 · 达州市 · 移通");
    }

    #[test]
    fn formats_cip_cc_location_and_isp() {
        assert_eq!(
            format_cip_cc_location("中国 四川 成都 | 中国移动"),
            "中国 · 四川 · 成都 · 中国移动"
        );
    }

    #[test]
    fn device_info_deserialize_full() {
        let json = r#"{
            "device_id": 123,
            "device_uuid": "abc-123",
            "device_name": "测试设备",
            "device_alias": "我的电脑",
            "device_type": "Windows",
            "status": "online",
            "lan_ip": "192.168.1.100",
            "rdp_enabled": true,
            "rdp_port": 3389,
            "public_ip": "1.2.3.4",
            "system_version": "Windows 11",
            "client_version": "1.7.3-3becb9",
            "service_port": 8080,
            "connect_code": "ABC123"
        }"#;

        let device: DeviceInfo = serde_json::from_str(json).unwrap();
        assert_eq!(device.device_id, 123);
        assert_eq!(device.device_name, "测试设备");
        assert_eq!(device.status, "online");
        assert_eq!(device.rdp_port, 3389);
        assert_eq!(device.client_version, "1.7.3-3becb9");
    }

    #[test]
    fn device_info_deserialize_minimal() {
        let json = r#"{
            "device_id": 456,
            "device_uuid": "def-456",
            "device_name": "最小设备",
            "device_type": "Linux",
            "status": "offline",
            "system_version": "Ubuntu 22.04"
        }"#;

        let device: DeviceInfo = serde_json::from_str(json).unwrap();
        assert_eq!(device.device_id, 456);
        assert!(device.device_alias.is_none());
        assert!(device.lan_ip.is_none());
        assert_eq!(device.rdp_port, 0);
        assert!(!device.rdp_enabled);
        assert!(device.client_version.is_empty());
    }

    #[test]
    fn status_report_serializes_client_version() {
        let report = DeviceStatusReport {
            device_id: 123,
            device_uuid: "abc-123".to_string(),
            lan_ip: "192.168.1.100".to_string(),
            public_ip: "1.2.3.4".to_string(),
            public_ip_location: "test".to_string(),
            service_port: 3389,
            rdp_enabled: true,
            remote_access: Some(RemoteAccessInfo {
                protocol: "rdp".to_string(),
                enabled: true,
                port: 3389,
            }),
            client_version: "1.7.3-3becb9".to_string(),
        };

        let value = serde_json::to_value(report).unwrap();
        assert_eq!(value["client_version"], "1.7.3-3becb9");
        assert_eq!(value["remote_access"]["protocol"], "rdp");
    }

    #[test]
    fn compiled_client_version_wins_over_runtime_override() {
        assert_eq!(
            resolve_client_version(Some(" 1.7.3-3becb9 "), Some("9.9.9-stale".to_string())),
            "1.7.3-3becb9"
        );
        assert_eq!(
            resolve_client_version(None, Some(" ".to_string())),
            "unknown"
        );
    }

    #[test]
    fn windows_11_build_corrects_legacy_registry_product_name() {
        assert_eq!(
            normalize_windows_product_name("Windows 10 Home", "26200"),
            "Windows 11 Home"
        );
        assert_eq!(
            normalize_windows_product_name("Windows 10 Pro", "22000"),
            "Windows 11 Pro"
        );
    }

    #[test]
    fn windows_product_name_keeps_windows_10_and_server_names() {
        assert_eq!(
            normalize_windows_product_name("Windows 10 Home", "19045"),
            "Windows 10 Home"
        );
        assert_eq!(
            normalize_windows_product_name("Windows Server 2025 Datacenter", "26100"),
            "Windows Server 2025 Datacenter"
        );
    }

    #[test]
    fn windows_product_name_does_not_assume_windows_10_without_a_build() {
        assert_eq!(
            normalize_windows_product_name("Windows 10 Home", ""),
            "Windows 10/11 Home"
        );
        assert_eq!(
            normalize_windows_product_name("Windows 10 Pro", "unknown"),
            "Windows 10/11 Pro"
        );
    }

    #[cfg(windows)]
    #[test]
    fn windows_wmi_returns_caption_and_numeric_build() {
        let (caption, build_number) =
            get_windows_version_from_wmi().expect("Win32_OperatingSystem should be available");
        assert!(caption.to_ascii_lowercase().contains("windows"));
        assert!(build_number.parse::<u32>().is_ok());
    }

    #[test]
    fn linux_system_version_preserves_full_distribution_version() {
        let cases = [
            ("NAME=\"Ubuntu\"\nVERSION_ID=\"20.04\"", "Ubuntu 20.04"),
            (
                "NAME=\"Debian GNU/Linux\"\nVERSION_ID=\"12\"",
                "Debian GNU/Linux 12",
            ),
            (
                "NAME=\"CentOS Linux\"\nVERSION_ID=\"7.9.2009\"",
                "CentOS Linux 7.9.2009",
            ),
            ("NAME=\"Deepin\"\nVERSION_ID=\"20.9\"", "Deepin 20.9"),
        ];

        for (os_release, expected) in cases {
            assert_eq!(
                parse_linux_system_version(os_release).as_deref(),
                Some(expected)
            );
        }
    }

    #[test]
    fn linux_system_version_falls_back_when_version_is_missing() {
        assert_eq!(
            parse_linux_system_version("NAME=Ubuntu\n"),
            Some("Ubuntu".to_string())
        );
        assert_eq!(parse_linux_system_version("VERSION_ID=20.04\n"), None);
    }
}
