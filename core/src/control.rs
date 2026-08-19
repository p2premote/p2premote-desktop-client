//! IPC 控制协议 — 所有消息统一走 Data enum
//!
//! 跨平台抽象：Windows 用 named pipe，Unix 用 Unix domain socket
//!
//! 协议流程：
//! 1. 客户端连接 IPC 端点
//! 2. 客户端发送 Data::Handshake { secret }
//! 3. 服务端验证后回复 Data::CommandResponse
//! 4. 双方在持久连接上互发 Data 消息（长度前缀帧 + JSON）

use crate::bytes_codec::BytesCodec;
use crate::config::derive_control_secret;
use crate::device::DeviceInfo;
use crate::p2p::ActiveStartResult;
use crate::subnet_router::LanMode;
use anyhow::{anyhow, Context, Result};
use bytes::Bytes;
use futures_util::{SinkExt, StreamExt};
use serde::{Deserialize, Serialize};
use std::time::Duration;
use tokio::time::timeout;
use tokio_util::codec::Framed;

// ---- 平台 IPC 路径 ----

#[cfg(windows)]
pub const IPC_ENDPOINT: &str = r"\\.\pipe\p2premote-service-control";

#[cfg(not(windows))]
pub fn ipc_socket_path() -> std::path::PathBuf {
    crate::config::linux_run_dir().join("p2premote-service.sock")
}

// ---- 消息定义 ----

/// IPC 消息（tagged enum 序列化，双向统一）
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "t", content = "c")]
pub enum Data {
    // --- 握手 ---
    Handshake {
        secret: String,
    },

    // --- 客户端 → 服务端命令 ---
    Ping,
    Status,
    RegisterDevice,
    StartActiveTunnelJob {
        target_device_id: i64,
        target_device_uuid: String,
        #[serde(default)]
        connect_code: Option<String>,
        #[serde(default)]
        temporary_password: Option<String>,
        #[serde(default)]
        lan_cidrs: Vec<String>,
    },
    StartAnonymousActiveTunnelJob {
        connect_code: String,
        temporary_password: String,
    },
    StopActiveTunnelJob {
        target_device_id: i64,
    },
    RefreshTunnelStatus,
    RefreshNetworkInfo,
    ReloadConfig,
    UpdateAuth,
    Logout,
    RebuildDeviceIdentity,
    AcknowledgeDeviceIdentityNotification,
    Reconnect,
    StopTunnel {
        source_device_id: i64,
    },
    StopActiveTunnel {
        target_device_id: i64,
    },
    TestTunnelSpeed {
        peer_device_id: i64,
    },
    ShutdownGracefully,

    // --- 客户端 → 服务端：设备管理命令（统一由 service 持有 token 调服务器） ---
    GetDeviceList,
    UpdateDeviceAlias {
        device_id: i64,
        alias: String,
    },
    DeleteDevice {
        device_id: i64,
    },
    UpdateDeviceInfo {
        device_id: i64,
        lan_ip: String,
        public_ip: String,
        service_port: i64,
    },
    SetDevicePassword {
        device_id: i64,
        password: String,
    },
    GenerateConnectCode {
        device_id: i64,
    },
    MarkCurrentDeviceOffline,

    // --- 客户端 → 服务端：认证命令（统一由 service 持有令牌、负责续期） ---
    /// 登录：service 调 /auth/login 并持久化 token 到 machine config
    Login {
        identifier: String,
        password: String,
    },
    /// 自动登录：service 用已保存 refresh token 恢复会话
    TryAutoLogin,
    /// 用户手动确认使用已保存的 refresh token 恢复会话。
    ResumeSavedSession {
        auto_login: bool,
    },
    /// 获取用户资料：service 调 /auth/profile（走 send_authed 自动刷新）
    GetUserProfile,
    /// 获取邀请信息：service 调 /auth/invite
    GetInviteInfo,

    // --- 客户端 → 服务端：登录设置管理（machine config 受保护，UI 无权直接读写） ---
    /// 保存登录设置：service 仅保存 token 会话偏好，不持久化密码。
    SaveLoginSettings {
        identifier: String,
        remember_me: bool,
        auto_login: bool,
    },
    /// 读取保存的登录标识和自动登录偏好。
    GetSavedLogin,
    /// 读取登录偏好（remember_me / auto_login，用于设置页展示）
    GetLoginPreferences,
    /// 保存开机自启偏好。
    SetAutoStartConfig {
        enabled: bool,
    },
    /// 设置 UI 语言（"zh-CN" | "en"）。service 持久化到 MachineConfig.locale
    /// 并更新内存缓存 RuntimeStatus.locale，供后端 message 选词使用。
    SetLocale {
        locale: String,
    },
    /// 读取当前 UI 语言（从 RuntimeStatus.locale 返回，未设置时为 None）。
    GetLocale,
    /// 读取本机 wgvpn LAN 访问配置。
    GetWgvpnLanAccessConfig,
    /// 保存本机 wgvpn LAN 访问配置。
    SaveWgvpnLanAccessConfig {
        enabled: bool,
        #[serde(default)]
        cidrs: Vec<String>,
    },
    // --- 服务端 → 客户端：命令响应 ---
    CommandResponse {
        ok: bool,
        message: String,
        #[serde(default)]
        status: Option<RuntimeStatus>,
        /// 持久连接上匹配请求-响应的 ID（短连接可忽略）
        #[serde(default)]
        request_id: Option<u64>,
        /// 结构化响应数据（如设备列表 JSON）。
        /// #[serde(default)] 保证旧版本互通不受影响。
        #[serde(default)]
        data: Option<serde_json::Value>,
    },

    // --- 服务端 → 客户端：主动推送 ---
    StatusChanged(RuntimeStatus),
}

/// 运行时状态
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct RuntimeStatus {
    /// Identifies the current background-service process lifetime. A new value
    /// is generated on every service start so UIs can distinguish a page reload
    /// from a service restart without persisting credentials.
    #[serde(default)]
    pub service_session_id: String,
    pub logged_in: bool,
    pub device_id: Option<i64>,
    pub device_uuid: Option<String>,
    pub ws_connected: bool,
    pub last_heartbeat_at: Option<i64>,
    pub last_error: Option<String>,
    #[serde(default)]
    pub device_identity_rebuilt: bool,
    #[serde(default)]
    pub device_identity_message: Option<String>,
    #[serde(default)]
    pub public_ip: Option<String>,
    #[serde(default)]
    pub public_ip_location: Option<String>,
    #[serde(default)]
    pub current_device: Option<DeviceInfo>,
    #[serde(default)]
    pub active_tunnel_jobs: Vec<ActiveTunnelJobStatus>,
    #[serde(default)]
    pub invite_temporary_password: Option<String>,
    /// wgvpn 已建立的会话列表（从 WGVPN_SESSIONS 映射）。
    #[serde(default)]
    pub wgvpn_sessions: Vec<WgvpnSessionStatus>,
    /// wgvpn 异步 job 状态（重试中的连接任务）。
    #[serde(default)]
    pub wgvpn_jobs: Vec<WgvpnJobStatus>,
    #[serde(default)]
    pub tunnel_lifecycles: Vec<TunnelLifecycleStatus>,
    /// 当前 UI 语言（"zh-CN" | "en"）。None 表示尚未设置，前端兜底探测系统语言。
    #[serde(default)]
    pub locale: Option<String>,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum TunnelLifecycleState {
    NotEstablished,
    Connecting,
    Connected,
    Recovering,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum TunnelLifecycleRole {
    Active,
    Passive,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum TunnelLastResult {
    None,
    UserDisconnected,
    PeerDisconnected,
    AttemptFailed,
    HealthGraceExpired,
    Cancelled,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TunnelLifecycleStatus {
    pub peer_device_id: i64,
    #[serde(default)]
    pub source_user_id: i64,
    #[serde(default)]
    pub source_username: String,
    #[serde(default)]
    pub source_email: String,
    #[serde(default)]
    pub peer_device_name: String,
    #[serde(default)]
    pub peer_device_alias: String,
    #[serde(default)]
    pub peer_public_ip: String,
    pub role: TunnelLifecycleRole,
    pub state: TunnelLifecycleState,
    pub attempt: u8,
    pub max_attempts: u8,
    #[serde(default)]
    pub stage: Option<String>,
    #[serde(default)]
    pub virtual_ip: Option<String>,
    #[serde(default)]
    pub peer_virtual_ip: Option<String>,
    pub last_result: TunnelLastResult,
    #[serde(default)]
    pub error_code: Option<String>,
    #[serde(default)]
    pub message: Option<String>,
    #[serde(default)]
    pub health_failures: u8,
    #[serde(default)]
    pub health_grace_deadline: Option<i64>,
    /// 会话首次进入 Connected 状态的 Unix 时间戳。后续心跳/重连保留，不清零。
    /// 缺失时由前端回退到 updated_at。
    #[serde(default)]
    pub connected_at: Option<i64>,
    pub updated_at: i64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ActiveTunnelJobStatus {
    pub target_device_id: i64,
    pub target_device_uuid: String,
    pub state: ActiveTunnelJobState,
    pub attempt: u8,
    pub max_attempts: u8,
    pub message: String,
    #[serde(default)]
    pub result: Option<ActiveStartResult>,
    pub updated_at: i64,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum ActiveTunnelJobState {
    Running,
    Waiting,
    Succeeded,
    Failed,
    Cancelled,
}

// ---- wgvpn 状态结构 ----

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq, Default)]
#[serde(rename_all = "snake_case")]
pub enum WgvpnHealthState {
    #[default]
    Connected,
    Degraded,
}

/// wgvpn 已建立会话的状态快照（推送给 UI / CLI status）。
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct WgvpnSessionStatus {
    pub peer_device_id: i64,
    pub is_active: bool,
    pub virtual_ip: String,
    pub peer_virtual_ip: String,
    pub peer_pubkey: String,
    pub tunnel_name: String,
    #[serde(default)]
    pub health_state: WgvpnHealthState,
    #[serde(default)]
    pub consecutive_failures: u8,
    #[serde(default)]
    pub health_grace_deadline: Option<i64>,
    /// 健康控制连接最近一次测得的往返延迟（毫秒）。首个样本前为空。
    #[serde(default)]
    pub latency_ms: Option<u32>,
    /// WireGuard peer 累计接收/发送字节数。接口重建后从零开始。
    #[serde(default)]
    pub received_bytes: u64,
    #[serde(default)]
    pub transmitted_bytes: u64,
    #[serde(default)]
    pub local_forward_port: u16,
    #[serde(default)]
    pub exposed_lan_cidrs: Vec<String>,
    #[serde(default)]
    pub advertised_lan_routes: Vec<String>,
    #[serde(default)]
    pub lan_mode: LanMode,
    #[serde(default)]
    pub subnet_router_handle_id: String,
    #[serde(default)]
    pub userspace_wg_peer_handle: String,
    #[serde(default)]
    pub userspace_wg: bool,
    #[serde(default)]
    pub subnet_router_started: bool,
    #[serde(default)]
    pub subnet_tcp_sessions: u32,
    #[serde(default)]
    pub subnet_udp_sessions: u32,
    #[serde(default)]
    pub subnet_wg_rx_packets: u64,
    #[serde(default)]
    pub subnet_wg_tx_packets: u64,
    /// 用户态 WireGuard Bind 实际收发批次数，用于计算每批包数和定位逐包退化。
    #[serde(default)]
    pub userspace_wg_rx_batches: u64,
    #[serde(default)]
    pub userspace_wg_tx_batches: u64,
    #[serde(default)]
    pub subnet_icmp_success: u64,
    #[serde(default)]
    pub subnet_icmp_failed: u64,
    #[serde(default)]
    pub subnet_rejected_flows: u64,
    #[serde(default)]
    pub subnet_last_error: String,
}

/// wgvpn 异步 job 的状态（重试中的连接任务，仿 ActiveTunnelJobStatus）。
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct WgvpnJobStatus {
    pub peer_device_id: i64,
    pub is_active: bool,
    pub state: WgvpnJobState,
    pub attempt: u8,
    pub max_attempts: u8,
    pub message: String,
    pub updated_at: i64,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum WgvpnJobState {
    Running,
    Waiting,
    Succeeded,
    Failed,
    Cancelled,
}

// ---- 连接封装 ----

/// IPC 连接 — 包装 Framed<stream, BytesCodec>，提供 send/next
pub struct Connection<T> {
    inner: Framed<T, BytesCodec>,
}

impl<T> Connection<T>
where
    T: tokio::io::AsyncRead + tokio::io::AsyncWrite + Unpin,
{
    /// 从任意流创建连接
    pub fn new(stream: T) -> Self {
        Self {
            inner: Framed::new(stream, BytesCodec::new()),
        }
    }

    /// 发送 Data
    pub async fn send(&mut self, data: &Data) -> Result<()> {
        let json = serde_json::to_vec(data).context("failed to serialize ipc data")?;
        self.inner
            .send(Bytes::from(json))
            .await
            .map_err(|e| anyhow!("ipc send failed: {}", e))
    }

    /// 读取下一帧并反序列化为 Data
    pub async fn next(&mut self) -> Result<Option<Data>> {
        match self.inner.next().await {
            Some(Ok(bytes)) => {
                let data: Data =
                    serde_json::from_slice(&bytes).context("failed to decode ipc data")?;
                Ok(Some(data))
            }
            Some(Err(e)) => Err(anyhow!("ipc read error: {}", e)),
            None => Ok(None),
        }
    }
}

// ---- 跨平台 IPC 流类型 ----

/// 拆分读写的组合类型，用于 trait object
pub struct ReadWrite<R, W> {
    pub read: R,
    pub write: W,
}

impl<R: tokio::io::AsyncRead + Unpin, W: tokio::io::AsyncWrite + Unpin> tokio::io::AsyncRead
    for ReadWrite<R, W>
{
    fn poll_read(
        self: std::pin::Pin<&mut Self>,
        cx: &mut std::task::Context<'_>,
        buf: &mut tokio::io::ReadBuf<'_>,
    ) -> std::task::Poll<std::io::Result<()>> {
        std::pin::Pin::new(&mut self.get_mut().read).poll_read(cx, buf)
    }
}

impl<R: tokio::io::AsyncRead + Unpin, W: tokio::io::AsyncWrite + Unpin> tokio::io::AsyncWrite
    for ReadWrite<R, W>
{
    fn poll_write(
        self: std::pin::Pin<&mut Self>,
        cx: &mut std::task::Context<'_>,
        buf: &[u8],
    ) -> std::task::Poll<std::io::Result<usize>> {
        std::pin::Pin::new(&mut self.get_mut().write).poll_write(cx, buf)
    }

    fn poll_flush(
        self: std::pin::Pin<&mut Self>,
        cx: &mut std::task::Context<'_>,
    ) -> std::task::Poll<std::io::Result<()>> {
        std::pin::Pin::new(&mut self.get_mut().write).poll_flush(cx)
    }

    fn poll_shutdown(
        self: std::pin::Pin<&mut Self>,
        cx: &mut std::task::Context<'_>,
    ) -> std::task::Poll<std::io::Result<()>> {
        std::pin::Pin::new(&mut self.get_mut().write).poll_shutdown(cx)
    }
}

/// 统一的 IPC 流类型
pub type IpcStream = ReadWrite<
    Box<dyn tokio::io::AsyncRead + Unpin + Send>,
    Box<dyn tokio::io::AsyncWrite + Unpin + Send>,
>;

fn to_ipc_stream<S>(stream: S) -> IpcStream
where
    S: tokio::io::AsyncRead + tokio::io::AsyncWrite + Unpin + Send + 'static,
{
    let (read, write) = tokio::io::split(stream);
    ReadWrite {
        read: Box::new(read),
        write: Box::new(write),
    }
}

// ---- 客户端连接（跨平台） ----

/// 连接到 service IPC 端点并完成握手
pub async fn connect_with_handshake() -> Result<Connection<IpcStream>> {
    let secret = derive_control_secret();

    let stream = timeout(Duration::from_secs(10), connect_ipc_stream())
        .await
        .context("ipc connect timeout")??;

    let mut conn = Connection::new(stream);

    // 握手
    timeout(
        Duration::from_secs(10),
        conn.send(&Data::Handshake { secret }),
    )
    .await
    .context("ipc handshake send timeout")??;

    let ack = timeout(Duration::from_secs(10), conn.next())
        .await
        .context("ipc handshake response timeout")??
        .ok_or_else(|| anyhow!("no handshake response"))?;
    match ack {
        Data::CommandResponse { ok, message, .. } if !ok => {
            return Err(anyhow!("handshake rejected: {}", message));
        }
        Data::CommandResponse { .. } => {}
        _ => return Err(anyhow!("unexpected handshake response")),
    }

    Ok(conn)
}

#[cfg(windows)]
async fn connect_ipc_stream() -> Result<IpcStream> {
    use tokio::net::windows::named_pipe::ClientOptions;
    let client = ClientOptions::new()
        .open(IPC_ENDPOINT)
        .with_context(|| format!("failed to open control pipe {}", IPC_ENDPOINT))?;
    Ok(to_ipc_stream(client))
}

#[cfg(not(windows))]
async fn connect_ipc_stream() -> Result<IpcStream> {
    use tokio::net::UnixStream;
    let stream = UnixStream::connect(ipc_socket_path())
        .await
        .with_context(|| format!("failed to connect to unix socket {:?}", ipc_socket_path()))?;
    Ok(to_ipc_stream(stream))
}

// ---- 服务端 accept（跨平台） ----

#[cfg(windows)]
struct LocalSecurityDescriptor(windows_sys::Win32::Security::PSECURITY_DESCRIPTOR);

#[cfg(windows)]
impl Drop for LocalSecurityDescriptor {
    fn drop(&mut self) {
        if !self.0.is_null() {
            unsafe {
                windows_sys::Win32::Foundation::LocalFree(self.0 as _);
            }
        }
    }
}

#[cfg(windows)]
const CONTROL_PIPE_SDDL: &str = "D:P(A;;GA;;;SY)(A;;GA;;;BA)(A;;GRGW;;;IU)(A;;GRGW;;;RU)";

#[cfg(windows)]
fn pipe_security_descriptor() -> Result<LocalSecurityDescriptor> {
    use windows_sys::Win32::Security::Authorization::{
        ConvertStringSecurityDescriptorToSecurityDescriptorW, SDDL_REVISION_1,
    };

    let sddl: Vec<u16> = CONTROL_PIPE_SDDL
        .encode_utf16()
        .chain(std::iter::once(0))
        .collect();
    let mut descriptor = std::ptr::null_mut();
    if unsafe {
        ConvertStringSecurityDescriptorToSecurityDescriptorW(
            sddl.as_ptr(),
            SDDL_REVISION_1,
            &mut descriptor,
            std::ptr::null_mut(),
        )
    } == 0
    {
        return Err(std::io::Error::last_os_error())
            .context("failed to build control pipe security descriptor");
    }
    Ok(LocalSecurityDescriptor(descriptor))
}

/// accept 一个 IPC 客户端连接
#[cfg(windows)]
pub async fn accept_ipc_client() -> Result<IpcStream> {
    use std::sync::atomic::{AtomicBool, Ordering};
    use tokio::net::windows::named_pipe::ServerOptions;
    use windows_sys::Win32::Security::SECURITY_ATTRIBUTES;

    static FIRST_PIPE_INSTANCE: AtomicBool = AtomicBool::new(true);

    let first_pipe_instance = FIRST_PIPE_INSTANCE.swap(false, Ordering::SeqCst);
    let server = {
        let descriptor = pipe_security_descriptor()?;
        let mut security_attributes = SECURITY_ATTRIBUTES {
            nLength: std::mem::size_of::<SECURITY_ATTRIBUTES>() as u32,
            lpSecurityDescriptor: descriptor.0,
            bInheritHandle: 0,
        };
        unsafe {
            ServerOptions::new()
                .first_pipe_instance(first_pipe_instance)
                .reject_remote_clients(true)
                .create_with_security_attributes_raw(
                    IPC_ENDPOINT,
                    (&mut security_attributes as *mut SECURITY_ATTRIBUTES).cast(),
                )
        }
        .with_context(|| format!("failed to create secured control pipe {}", IPC_ENDPOINT))?
    };

    server
        .connect()
        .await
        .context("failed to accept control client")?;
    Ok(to_ipc_stream(server))
}

#[cfg(not(windows))]
pub async fn accept_ipc_client() -> Result<IpcStream> {
    use std::os::unix::fs::PermissionsExt;
    use tokio::net::UnixListener;
    let path = ipc_socket_path();
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)
            .with_context(|| format!("failed to create unix socket dir {:?}", parent))?;
        // 0o755：去掉全局可写位，防止任意用户篡改/删除 socket 目录。
        // （旧值 0o777 允许全局可写，存在 DoS 风险）
        let _ = std::fs::set_permissions(parent, std::fs::Permissions::from_mode(0o755));
    }
    let _ = std::fs::remove_file(&path);
    let listener = UnixListener::bind(&path)
        .with_context(|| format!("failed to bind unix socket {:?}", path))?;
    // 0o660：仅 owner+group 可读写，去掉全局可读写位。
    // 配合握手 secret 鉴权（derive_control_secret）。
    // 注：Linux 下 secret 基于 /etc/machine-id（通常全局可读），同机任意用户可推导，
    // 因此本机仍有提权面。生产环境建议使用 Windows（Named Pipe ACL 兜底）。
    let _ = std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o660));
    let (stream, _) = listener
        .accept()
        .await
        .context("failed to accept unix socket client")?;
    Ok(to_ipc_stream(stream))
}

// ---- CLI 便捷封装 ----

/// 一次性连接：connect → send command → read response → disconnect
pub async fn send_command(data: Data) -> Result<Data> {
    let mut conn = connect_with_handshake().await?;
    timeout(Duration::from_secs(10), conn.send(&data))
        .await
        .context("ipc command send timeout")??;
    // HTTP 请求自身最多等待 10 秒；IPC 必须留出 service 处理和持久化余量，
    // 避免网络请求刚结束时 GUI 先报后台服务超时。
    let resp = timeout(Duration::from_secs(20), conn.next())
        .await
        .context("ipc command response timeout")??
        .ok_or_else(|| anyhow!("no response from service"))?;
    Ok(resp)
}

#[cfg(test)]
mod tests {
    use super::*;
    use tokio::io::duplex;

    fn roundtrip_serialize(data: &Data) {
        let json = serde_json::to_string(data).unwrap();
        let parsed: Data = serde_json::from_str(&json).unwrap();
        let original = serde_json::to_string(data).unwrap();
        let roundtrip = serde_json::to_string(&parsed).unwrap();
        assert_eq!(original, roundtrip);
    }

    #[test]
    fn data_ping_roundtrip() {
        roundtrip_serialize(&Data::Ping);
    }

    #[test]
    fn data_status_roundtrip() {
        roundtrip_serialize(&Data::Status);
    }

    #[test]
    fn data_handshake_roundtrip() {
        roundtrip_serialize(&Data::Handshake {
            secret: "test-secret".to_string(),
        });
    }

    #[cfg(windows)]
    #[test]
    fn control_pipe_dacl_allows_only_local_interactive_principals() {
        assert_eq!(
            CONTROL_PIPE_SDDL,
            "D:P(A;;GA;;;SY)(A;;GA;;;BA)(A;;GRGW;;;IU)(A;;GRGW;;;RU)"
        );
        assert!(!CONTROL_PIPE_SDDL.contains(";;;WD)"));
        assert!(!CONTROL_PIPE_SDDL.contains(";;;AU)"));
    }

    #[cfg(windows)]
    #[tokio::test]
    async fn local_interactive_user_can_connect_to_secured_control_pipe() {
        use tokio::net::windows::named_pipe::{ClientOptions, ServerOptions};
        use windows_sys::Win32::Security::SECURITY_ATTRIBUTES;

        let endpoint = format!(r"\\.\pipe\p2premote-control-test-{}", uuid::Uuid::new_v4());
        let server = {
            let descriptor = pipe_security_descriptor().expect("security descriptor");
            let mut attributes = SECURITY_ATTRIBUTES {
                nLength: std::mem::size_of::<SECURITY_ATTRIBUTES>() as u32,
                lpSecurityDescriptor: descriptor.0,
                bInheritHandle: 0,
            };
            unsafe {
                ServerOptions::new().create_with_security_attributes_raw(
                    &endpoint,
                    (&mut attributes as *mut SECURITY_ATTRIBUTES).cast(),
                )
            }
            .expect("secured pipe")
        };
        let _client = ClientOptions::new().open(&endpoint).expect("pipe client");
        server.connect().await.expect("pipe server connection");
    }

    #[test]
    fn data_command_response_roundtrip() {
        roundtrip_serialize(&Data::CommandResponse {
            ok: true,
            message: "ok".to_string(),
            status: Some(RuntimeStatus {
                service_session_id: "service-session-1".to_string(),
                logged_in: true,
                device_id: Some(42),
                device_uuid: Some("abc-123".to_string()),
                ws_connected: true,
                last_heartbeat_at: Some(1234567890),
                last_error: None,
                device_identity_rebuilt: false,
                device_identity_message: None,
                public_ip: None,
                public_ip_location: None,
                current_device: None,
                active_tunnel_jobs: vec![],
                invite_temporary_password: None,
                wgvpn_sessions: vec![],
                wgvpn_jobs: vec![],
                tunnel_lifecycles: vec![],
                locale: Some("zh-CN".to_string()),
            }),
            request_id: None,
            data: None,
        });
    }

    /// RuntimeStatus.locale 默认为 None（向后兼容：旧 config 文件无此字段时反序列化得到 None）
    #[test]
    fn runtime_status_locale_defaults_none_on_deserialize() {
        // 模拟旧版本序列化数据（无 locale 字段）
        let legacy_json = serde_json::json!({
            "logged_in": false,
            "device_id": null,
            "device_uuid": null,
            "ws_connected": false,
            "last_heartbeat_at": null,
            "last_error": null
        });
        let status: RuntimeStatus = serde_json::from_value(legacy_json).unwrap();
        assert_eq!(status.locale, None, "locale 缺失时应反序列化为 None");
    }

    #[test]
    fn data_status_changed_roundtrip() {
        roundtrip_serialize(&Data::StatusChanged(RuntimeStatus::default()));
    }

    #[tokio::test]
    async fn connection_send_receive() {
        let (client, server) = duplex(4096);
        let mut client_conn = Connection::new(client);
        let mut server_conn = Connection::new(server);

        client_conn
            .send(&Data::Ping)
            .await
            .expect("client send failed");

        let received = server_conn.next().await.expect("server read failed");
        match received {
            Some(Data::Ping) => {}
            other => panic!("expected Ping, got {:?}", other),
        }

        server_conn
            .send(&Data::CommandResponse {
                ok: true,
                message: "pong".to_string(),
                status: None,
                request_id: None,
                data: None,
            })
            .await
            .expect("server send failed");

        let response = client_conn.next().await.expect("client read failed");
        match response {
            Some(Data::CommandResponse { ok, message, .. }) => {
                assert!(ok);
                assert_eq!(message, "pong");
            }
            other => panic!("expected CommandResponse, got {:?}", other),
        }
    }
}
