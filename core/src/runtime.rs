//! 服务端运行时主循环 + IPC 控制服务器
//!
//! 对齐 RustDesk 模式：
//! - accept + tokio::spawn 每连接独立 task
//! - 握手验证后进入消息循环
//! - 状态变化时通过持久连接推送 StatusChanged

use crate::auth::{
    fetch_invite_info, fetch_profile, login_and_persist, refresh_with_config,
    register_by_email_code, reset_password_by_email_code, send_verification_code,
};
use crate::config::{
    clear_machine_credentials, derive_control_secret, ensure_machine_config, load_machine_config,
    save_machine_config,
};
use crate::control::{
    accept_ipc_client, ActiveTunnelJobState, ActiveTunnelJobStatus, Connection, Data, IpcStream,
    RuntimeStatus, TunnelLastResult, TunnelLifecycleRole, TunnelLifecycleState,
    TunnelLifecycleStatus, WgvpnHealthState, WgvpnJobState, WgvpnJobStatus, WgvpnSessionStatus,
};
use crate::device::{
    anonymous_connect, collect_device_status_report, delete_device, generate_connect_code,
    get_device_list, get_public_network_info, get_rdp_port_from_registry, is_rdp_enabled,
    mark_current_device_offline, register_current_device_auto, send_device_status_report,
    set_connect_password, set_device_password, update_device_alias, DeviceStatusReport,
};
use crate::device_identity::{detect_clone, ensure_device_uuid, CloneDetectionResult};
use crate::gonc_ffi;
use crate::health::{
    spawn_health_server, HealthDisconnectHandler, HealthServerHandle, PassiveHealthEvent,
};
use crate::i18n::localized_message;
use crate::p2p::wgvpn_flow;
use crate::p2p::{
    build_punch_token, close_active_p2p_job, open_active_p2p_job, wgvpn_health_monitor_loop,
    ActiveP2POpenResult, ActiveStartResult, PassivePeerInfo, TunnelHealthEvent,
};
use crate::speed_test::TunnelSpeedTestCommand;
use crate::subnet_router;
use crate::ws::{ServiceWsClient, WsEvent};
use anyhow::{anyhow, Context, Result};
use parking_lot::Mutex;
use rand::Rng;
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, HashMap};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::Arc;
use std::time::{SystemTime, UNIX_EPOCH};
use tokio::sync::{mpsc, oneshot, watch, Notify};
use tokio::time::Instant;
use tokio::time::{interval, Duration};
use tracing::{debug, error, info, warn};

mod active_jobs;
mod desktop_engine;
mod ipc;
mod p2p_signal;
mod status;
mod tunnel_health;
mod tunnel_jobs;
mod web_admin;

use active_jobs::*;
use ipc::*;
use p2p_signal::*;
use status::{
    clear_active_tunnel_job_status, current_locale, relocalize_runtime_status,
    update_active_tunnel_job_status, update_status, upsert_tunnel_lifecycle,
};
use tunnel_health::*;
use tunnel_jobs::*;

/// 状态变化广播通道容量
const STATUS_BROADCAST_CAPACITY: usize = 16;
const ACTIVE_TUNNEL_JOB_MAX_ATTEMPTS: u8 = 30;
const ACTIVE_TUNNEL_JOB_ATTEMPT_TOTAL_SECS: u64 = 120;
const ACTIVE_TUNNEL_JOB_BACKOFF_SECS: u64 = 60;
/// wgvpn job 最大重试次数（打洞/握手失败时重试）。
const WGVPN_JOB_MAX_ATTEMPTS: u8 = 30;
/// wgvpn job 单次尝试超时（含公钥交换 + 打洞 + 握手）。
const WGVPN_JOB_ATTEMPT_SECS: u64 = 120;
/// wgvpn job 失败后的退避间隔。
const WGVPN_JOB_BACKOFF_SECS: u64 = 30;
static NEXT_WGVPN_JOB_GENERATION: AtomicU64 = AtomicU64::new(1);
static NEXT_WGVPN_HEALTH_GENERATION: AtomicU64 = AtomicU64::new(1);
static NEXT_PASSIVE_HEALTH_GRACE_GENERATION: AtomicU64 = AtomicU64::new(1);
static NEXT_PASSIVE_HEALTH_WATCHDOG_GENERATION: AtomicU64 = AtomicU64::new(1);
/// 主动端每 5 秒评估一次健康状态，连续 12 次失败约 60 秒后进入网络异常。
const WGVPN_HEALTH_FAILURE_THRESHOLD: u8 = 12;
const WGVPN_HEALTH_GRACE_SECS: u64 = 300;
/// 被动会话建立或恢复后，等待主动端首次健康连接的时间。
const PASSIVE_HEALTH_CONNECT_WAIT_SECS: u64 = 60;
const DEVICE_STATUS_POLL_SECS: u64 = 30;
const DEVICE_STATUS_REPORT_FALLBACK_SECS: u64 = 300;

struct WgvpnJobControl {
    generation: u64,
    cancel_tx: watch::Sender<bool>,
}

struct WgvpnHealthControl {
    generation: u64,
    stop_tx: watch::Sender<bool>,
    speed_tx: mpsc::UnboundedSender<TunnelSpeedTestCommand>,
    /// 测速是否正在进行（true 时拒绝新请求，避免在 health loop 里堆积串行）。
    speed_test_busy: Arc<std::sync::atomic::AtomicBool>,
}

struct PassiveHealthGraceControl {
    generation: u64,
    cancel_tx: watch::Sender<bool>,
}

#[derive(Clone, Copy)]
enum CleanupGuard {
    None,
    Monitor(u64),
    Grace(u64),
}

#[derive(Debug, Clone)]
struct WgvpnHealthRuntime {
    state: WgvpnHealthState,
    consecutive_failures: u8,
    grace_deadline: Option<i64>,
    latency_ms: Option<u32>,
}

impl Default for WgvpnHealthRuntime {
    fn default() -> Self {
        Self {
            state: WgvpnHealthState::Connected,
            consecutive_failures: 0,
            grace_deadline: None,
            latency_ms: None,
        }
    }
}

#[derive(Default)]
pub(super) struct SharedRuntimeState {
    status: RuntimeStatus,
    /// 当前 service 生命周期是否已获准使用持久化令牌登录。
    /// service 启动时仅由记住会话 + 自动登录 + token 开启；显式登录后保持到进程退出。
    login_session_enabled: bool,
    reconnect_requested: bool,
    shutdown_requested: bool,
    active_tunnel_job_cancels: HashMap<i64, watch::Sender<bool>>,
    /// wgvpn job 的取消句柄，key 为 peer_device_id。
    wgvpn_job_cancels: HashMap<i64, WgvpnJobControl>,
    /// 主动端基于 WGVPN 虚拟 IP 建立的持久 TCP 健康监测任务。
    wgvpn_health_controls: HashMap<i64, WgvpnHealthControl>,
    /// 被动端健康连接断开后的宽限清理任务；重连或显式 Stop 时取消。
    passive_health_grace_controls: HashMap<i64, PassiveHealthGraceControl>,
    /// 被动端每个设备当前有效的健康 TCP 连接代次。
    passive_health_connection_generations: HashMap<i64, u64>,
    /// 被动会话等待首次健康连接的看门狗代次。
    passive_health_watchdog_generations: HashMap<i64, u64>,
    /// 健康状态只保存在本次 service 进程内，不写入 session 文件。
    wgvpn_health_runtime: HashMap<i64, WgvpnHealthRuntime>,
    /// health server 句柄，用于被动端主动关闭指定 peer 的 health TCP 连接
    /// （方案 A：触发 TCP FIN 立即通知主动端断开，避免长时间无感知）。
    health_server_handle: Option<Arc<HealthServerHandle>>,
    ws_client: Option<ServiceWsClient>,
    p2p_attempt_waiters: HashMap<String, mpsc::UnboundedSender<P2PAttemptEvent>>,
    passive_p2p_attempts: HashMap<i64, PassiveP2PAttempt>,
    /// Current in-memory account identity used to classify inbound attempts.
    current_user_id: Option<i64>,
    /// At most one temporary inbound approval can exist on a passive endpoint.
    /// This is intentionally a single slot rather than a queue: remote assistance
    /// is a one-shot interaction and concurrent inbound approvals are rejected.
    pending_inbound_approval: Option<PendingInboundApprovalRuntime>,
    /// 被动连接请求附带的来源设备展示信息，仅用于本次进程内通知。
    passive_peer_infos: HashMap<i64, PassivePeerInfo>,
    p2p_notify_locks: HashMap<i64, Arc<tokio::sync::Mutex<()>>>,
    status_tx: Option<tokio::sync::broadcast::Sender<RuntimeStatus>>,
}

#[derive(Debug, Clone)]
struct PassiveP2PAttempt {
    connection_id: String,
    attempt_id: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum PassiveApprovalDecision {
    Pending,
    Granted,
    Denied,
}

struct PendingInboundApprovalRuntime {
    status: crate::control::InboundApprovalStatus,
    decision_tx: watch::Sender<PassiveApprovalDecision>,
    connection_id: String,
    source_device_id: i64,
    access_grant: String,
    ws_client: ServiceWsClient,
}

#[derive(Debug, Clone)]
enum P2PAttemptEvent {
    Ready {
        rdp_port: u16,
        approval_required: bool,
        traversal_negotiation: Option<crate::traversal_policy::Negotiation>,
    },
    ApprovalRequired,
    ApprovalGranted,
    ApprovalDenied,
    ApprovalTimeout,
    Failed {
        error_code: String,
        message: String,
    },
    Cancelled,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
enum P2PAttemptMessage {
    AttemptStart {
        protocol_version: u8,
        client_job_id: String,
        attempt_id: String,
        attempt: u8,
        max_attempts: u8,
        punch_token: String,
        #[serde(default)]
        source_user_id: i64,
        #[serde(default)]
        source_username: String,
        #[serde(default)]
        source_email: String,
        #[serde(default)]
        source_device_name: String,
        #[serde(default)]
        source_device_alias: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        traversal_negotiation: Option<crate::traversal_policy::Negotiation>,
    },
    AttemptReady {
        protocol_version: u8,
        attempt_id: String,
        stage: String,
        rdp_enabled: bool,
        rdp_port: u16,
        #[serde(default)]
        approval_required: bool,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        traversal_negotiation: Option<crate::traversal_policy::Negotiation>,
    },
    Traversal {
        attempt_id: String,
        frame: crate::traversal::Frame,
    },
    ApprovalRequired {
        protocol_version: u8,
        attempt_id: String,
        expires_at: i64,
    },
    ApprovalGranted {
        protocol_version: u8,
        attempt_id: String,
    },
    ApprovalDenied {
        protocol_version: u8,
        attempt_id: String,
    },
    ApprovalTimeout {
        protocol_version: u8,
        attempt_id: String,
    },
    AttemptFailed {
        protocol_version: u8,
        attempt_id: String,
        stage: String,
        error: P2PAttemptErrorPayload,
    },
    AttemptCancel {
        protocol_version: u8,
        attempt_id: String,
        reason: String,
    },
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct P2PAttemptErrorPayload {
    code: String,
    message: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    message_key: Option<String>,
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    message_params: BTreeMap<String, serde_json::Value>,
}

impl P2PAttemptErrorPayload {
    fn new(code: impl Into<String>, message: impl Into<String>) -> Self {
        let code = code.into();
        Self {
            message_key: Some(format!("p2p.attempt.{code}")),
            code,
            message: message.into(),
            message_params: BTreeMap::new(),
        }
    }

    fn localized_message(&self, locale: Option<&str>) -> String {
        let Some(key) = self.message_key.as_deref() else {
            return self.message.clone();
        };
        let owned_params: Vec<(String, String)> = self
            .message_params
            .iter()
            .map(|(name, value)| {
                let value = value
                    .as_str()
                    .map(ToOwned::to_owned)
                    .unwrap_or_else(|| value.to_string());
                (name.clone(), value)
            })
            .collect();
        let params: Vec<(&str, &str)> = owned_params
            .iter()
            .map(|(name, value)| (name.as_str(), value.as_str()))
            .collect();
        let localized = localized_message(locale, key, &params);
        if localized == key {
            self.message.clone()
        } else if self.message.trim().is_empty()
            || self.message == self.code
            || self.message == localized
        {
            localized
        } else {
            // Keep the diagnostic supplied by the peer. Replacing it with the
            // localized category used to turn errors such as access denied or
            // an invalid WireGuard key into only "WireGuard configuration
            // failed", leaving neither the UI nor the active-side log with an
            // actionable cause.
            format!("{}: {}", localized, self.message)
        }
    }
}

impl SharedRuntimeState {
    fn status_tx(&self) -> Option<&tokio::sync::broadcast::Sender<RuntimeStatus>> {
        self.status_tx.as_ref()
    }
}

pub(super) fn resolve_inbound_approval(
    shared: &Arc<Mutex<SharedRuntimeState>>,
    attempt_id: &str,
    allow: bool,
) -> Option<Data> {
    let approval_context = {
        shared
            .lock()
            .pending_inbound_approval
            .as_ref()
            .filter(|pending| pending.status.attempt_id == attempt_id)
            .map(|pending| (pending.decision_tx.clone(), pending.status.expires_at))
    };
    let Some((decision_tx, expires_at)) = approval_context else {
        return Some(cmd_response(
            false,
            "inbound approval request not found or expired",
            Some(shared.lock().status.clone()),
        ));
    };
    if now_ts() >= expires_at {
        let _ = decision_tx.send(PassiveApprovalDecision::Denied);
        return Some(cmd_response(
            false,
            "inbound approval request expired",
            Some(shared.lock().status.clone()),
        ));
    }
    let decision = if allow {
        PassiveApprovalDecision::Granted
    } else {
        PassiveApprovalDecision::Denied
    };
    let _ = decision_tx.send(decision);
    Some(cmd_response(
        true,
        if allow {
            "inbound tunnel approval granted"
        } else {
            "inbound tunnel approval denied"
        },
        Some(shared.lock().status.clone()),
    ))
}

fn generate_temporary_password() -> String {
    let mut rng = rand::thread_rng();
    (0..6)
        .map(|_| char::from(b'0' + rng.gen_range(0..10) as u8))
        .collect()
}

async fn rotate_invite_temporary_password(shared: &Arc<Mutex<SharedRuntimeState>>) -> Result<()> {
    let mut config = load_machine_config()?;
    let password = generate_temporary_password();
    set_connect_password(&mut config, &password).await?;
    config.invite_temporary_password = Some(password.clone());
    // 密码已在服务器端生效；持久化失败只影响重启后复用，不应视为轮换失败。
    if let Err(err) = save_machine_config(&config) {
        warn!(
            "[ServiceRuntime] persist rotated invite password failed: {}",
            err
        );
    }
    update_status(shared, |status| {
        status.invite_temporary_password = Some(password);
    });
    Ok(())
}

static EXTERNAL_SHUTDOWN_REQUESTED: AtomicBool = AtomicBool::new(false);

pub async fn run_service_foreground() -> Result<()> {
    // Web UI 使用 HTTP 时，TLS provider 仍会被 API/P2P HTTPS 客户端使用。
    // 必须在任何 Rustls 客户端初始化前设置进程级默认 provider。ring
    // 对 Windows 7 的系统 API 依赖更低；reqwest 的 rustls-tls feature
    // 已启用同一 provider，因此不再强制加载 aws-lc native backend。
    let provider = rustls::crypto::ring::default_provider();
    let _ = provider.install_default();
    info!("[ServiceRuntime] starting, pid={}", std::process::id());
    EXTERNAL_SHUTDOWN_REQUESTED.store(false, Ordering::SeqCst);
    if let Err(err) = crate::config::clear_cached_public_network_info() {
        info!(
            "[ServiceRuntime] failed to clear stale public network cache: {}",
            err
        );
    }
    // 启动路径只加载一次 machine config：locale 恢复、wgvpn 残留清理、
    // 登录会话判定与 Web Admin 启动/日志共用，避免同一文件重复读 4-6 次。
    // 读失败时各消费点沿用原先的默认值语义（default / false / 跳过恢复）。
    let startup_config = match crate::config::load_machine_config() {
        Ok(config) => Some(config),
        Err(err) => {
            info!(
                "[ServiceRuntime] config load failed during startup; using defaults: {}",
                err
            );
            None
        }
    };
    let startup_locale = startup_config
        .as_ref()
        .and_then(|config| config.locale.clone());
    crate::device::initialize_public_network_group(startup_locale.as_deref());

    // WGVPN 数据面属于 service 进程，重启后不可恢复；这里只清理崩溃残留。
    let cleanup_config = startup_config.clone().unwrap_or_default();
    wgvpn_flow::cleanup_stale_sessions(&cleanup_config)
        .context("failed to clear stale wgvpn state before service startup")?;

    let startup_login_enabled = startup_config
        .as_ref()
        .map(persistent_login_enabled)
        .unwrap_or(false);
    let (status_tx, _) = tokio::sync::broadcast::channel(STATUS_BROADCAST_CAPACITY);
    let shared = Arc::new(Mutex::new(SharedRuntimeState {
        status: RuntimeStatus {
            service_session_id: uuid::Uuid::new_v4().to_string(),
            ..Default::default()
        },
        status_tx: Some(status_tx),
        login_session_enabled: startup_login_enabled,
        ..Default::default()
    }));
    // 从持久化配置恢复 locale 到内存缓存，保证 service 重启后
    // 第一批 message（批次4）能按正确语言选词，避免短暂显示默认语言。
    if let Some(config) = startup_config.as_ref() {
        if let Some(locale) = config.locale.clone() {
            shared.lock().status.locale = Some(locale);
        }
        // 恢复邀请临时密码，邀请页挂载时优先复用而不是重新生成覆盖。
        if let Some(password) = config
            .invite_temporary_password
            .clone()
            .filter(|value| !value.is_empty())
        {
            shared.lock().status.invite_temporary_password = Some(password);
        }
    }
    let wake = Arc::new(Notify::new());
    spawn_control_server(shared.clone(), wake.clone());
    info!("[ServiceRuntime] IPC control server started");
    web_admin::spawn_web_admin_server(shared.clone(), wake.clone(), startup_config.clone());
    info!(
        "[ServiceRuntime] web admin server scheduled on {}",
        web_admin::web_admin_addr_for_log(startup_config.as_ref())
    );

    let health_shared = shared.clone();
    let health_handler: HealthDisconnectHandler = Arc::new(
        move |peer_device_id, generation, event| {
            if matches!(&event, PassiveHealthEvent::HeartbeatSucceeded { .. }) {
                debug!(
                    "[ServiceRuntime] tunnel health event: peer_device_id={}, event={:?}",
                    peer_device_id, event
                );
            } else {
                info!(
                    "[ServiceRuntime] tunnel health event: peer_device_id={}, event={:?}",
                    peer_device_id, event
                );
            }
            if event == PassiveHealthEvent::Connected {
                {
                    let mut state = health_shared.lock();
                    state
                        .passive_health_connection_generations
                        .insert(peer_device_id, generation);
                    state
                        .passive_health_watchdog_generations
                        .remove(&peer_device_id);
                }
                record_wgvpn_health_success(&health_shared, peer_device_id, None, None);
                return;
            }
            let is_current = health_shared
                .lock()
                .passive_health_connection_generations
                .get(&peer_device_id)
                .copied()
                == Some(generation);
            if !is_current {
                debug!(
                "[ServiceRuntime] ignored stale health connection event: peer_device_id={}, generation={}, event={:?}",
                peer_device_id, generation, event
            );
                return;
            }
            match event {
                PassiveHealthEvent::Connected => {}
                PassiveHealthEvent::HeartbeatSucceeded { latency_ms } => {
                    record_wgvpn_health_success(&health_shared, peer_device_id, None, latency_ms)
                }
                PassiveHealthEvent::Disconnected {
                    reason: reason @ ("remote_stop" | "peer_disconnected"),
                } => {
                    cancel_passive_health_grace(&health_shared, peer_device_id);
                    cleanup_wgvpn_session_async(
                        health_shared.clone(),
                        peer_device_id,
                        reason,
                        CleanupGuard::None,
                    );
                }
                PassiveHealthEvent::Disconnected { reason } => {
                    mark_wgvpn_health_degraded(health_shared.clone(), peer_device_id, reason)
                }
            }
        },
    );
    let peer_validator = Arc::new(move |source_device_id: i64, peer_ip: std::net::IpAddr| {
        let peer_ip = peer_ip.to_string();
        let session = wgvpn_flow::snapshot_sessions()
            .into_iter()
            .find(|session| session.peer_device_id == source_device_id)
            .ok_or_else(|| "wgvpn session not found".to_string())?;
        if session.approval_pending {
            return Err("wgvpn session is still awaiting approval".to_string());
        }
        if session.peer_virtual_ip != peer_ip {
            return Err(format!(
                "expected peer virtual ip {}, received {}",
                session.peer_virtual_ip, peer_ip
            ));
        }
        Ok(())
    });
    let health_server = match spawn_health_server(health_handler, peer_validator).await {
        Ok(handle) => {
            let handle = Arc::new(handle);
            shared.lock().health_server_handle = Some(handle.clone());
            Some(handle)
        }
        Err(err) => {
            warn!("[ServiceRuntime] health server start failed: {}", err);
            None
        }
    };
    let ws_client = ServiceWsClient::new();
    shared.lock().ws_client = Some(ws_client.clone());
    let (event_tx, mut event_rx) = mpsc::unbounded_channel::<WsEvent>();
    let mut device_status_tick = interval(Duration::from_secs(DEVICE_STATUS_POLL_SECS));
    let device_report_state = Arc::new(tokio::sync::Mutex::new((
        Option::<DeviceStatusReport>::None,
        Option::<Instant>::None,
    )));
    // 配置刷新、设备注册和状态上报可能同时读写 machine config；后台执行但仍串行化，
    // 既不阻塞 WS 事件分发，也避免多个维护任务互相覆盖配置。
    let maintenance_lock = Arc::new(tokio::sync::Mutex::new(()));
    let mut refresh_tick = interval(Duration::from_secs(60));
    let mut bootstrap_tick = interval(Duration::from_secs(60));

    if let Err(err) = bootstrap_service(&shared, &ws_client, event_tx.clone()).await {
        info!("[ServiceRuntime] initial bootstrap failed: {}", err);
        handle_bootstrap_error(&shared, &ws_client, err).await;
    } else {
        info!("[ServiceRuntime] initial bootstrap succeeded");
    }

    loop {
        if shared.lock().shutdown_requested || EXTERNAL_SHUTDOWN_REQUESTED.load(Ordering::SeqCst) {
            info!("[ServiceRuntime] shutdown requested");
            break;
        }

        tokio::select! {
            _ = bootstrap_tick.tick() => {
                spawn_bootstrap_maintenance(
                    shared.clone(), ws_client.clone(), event_tx.clone(), maintenance_lock.clone(), "tick"
                );
            }
            _ = wake.notified() => {
                debug!("[ServiceRuntime] wake received, running bootstrap");
                spawn_bootstrap_maintenance(
                    shared.clone(), ws_client.clone(), event_tx.clone(), maintenance_lock.clone(), "wake"
                );
            }
            _ = device_status_tick.tick() => {
                let report_shared = shared.clone();
                let report_state = device_report_state.clone();
                let report_lock = maintenance_lock.clone();
                tokio::spawn(async move {
                    let _maintenance_guard = report_lock.lock().await;
                    let mut report_state = report_state.lock().await;
                    let (last_status, last_report_at) = &mut *report_state;
                    if let Err(err) = maybe_report_device_status(
                        &report_shared,
                        last_status,
                        last_report_at,
                    ).await {
                        warn!("[ServiceRuntime] device status report failed: {}", err);
                        set_last_error(&report_shared, err.to_string());
                    }
                });
            }
            _ = refresh_tick.tick() => {
                let refresh_shared = shared.clone();
                let refresh_wake = wake.clone();
                let refresh_lock = maintenance_lock.clone();
                tokio::spawn(async move {
                    let _maintenance_guard = refresh_lock.lock().await;
                    if let Err(err) = maybe_refresh_auth(&refresh_shared, &refresh_wake).await {
                        warn!("[ServiceRuntime] auth refresh failed: {}", err);
                        set_last_error(&refresh_shared, err.to_string());
                    }
                });
            }
            maybe_event = event_rx.recv() => {
                if let Some(event) = maybe_event {
                    match event {
                        WsEvent::Connected => {
                            info!("[ServiceRuntime] WebSocket connected");
                            update_status(&shared, |s| {
                                s.ws_connected = true;
                                s.last_error = None;
                            });
                        }
                        WsEvent::Disconnected => {
                            info!("[ServiceRuntime] WebSocket disconnected");
                            update_status(&shared, |s| {
                                s.ws_connected = false;
                            });
                            shared.lock().reconnect_requested = true;
                            wake.notify_one();
                        }
                        WsEvent::P2PStart {
                            source_user_id,
                            source_username,
                            source_email,
                            source_device_id,
                            source_device_name,
                            source_device_alias,
                            source_public_ip,
                            punch_token,
                        } => {
                            info!(
                                "[ServiceRuntime] P2P start request: source_device_id={}",
                                source_device_id
                            );
                            if let Err(err) = handle_p2p_start(
                                &shared,
                                &ws_client,
                                source_device_id,
                                punch_token,
                                PassivePeerInfo {
                                    source_user_id,
                                    source_username,
                                    source_email,
                                    source_device_name,
                                    source_device_alias,
                                    source_public_ip,
                                },
                            )
                            .await
                            {
                                info!("[ServiceRuntime] P2P start failed: {}", err);
                                set_last_error(&shared, err.to_string());
                            }
                        }
                        WsEvent::P2PNotify {
                            connection_id,
                            source_device_id,
                            message_id: _,
                            access_grant,
                            data,
                        } => {
                            let peer_lock = shared
                                .lock()
                                .p2p_notify_locks
                                .entry(source_device_id)
                                .or_insert_with(|| Arc::new(tokio::sync::Mutex::new(())))
                                .clone();
                            let notify_shared = shared.clone();
                            let notify_ws = ws_client.clone();
                            tokio::spawn(async move {
                                let _peer_guard = peer_lock.lock().await;
                                if let Err(err) = handle_p2p_notify(
                                    &notify_shared,
                                    &notify_ws,
                                    connection_id,
                                    source_device_id,
                                    access_grant,
                                    data,
                                )
                                .await
                                {
                                    info!("[ServiceRuntime] p2p_notify handling failed: {}", err);
                                    set_last_error(&notify_shared, err.to_string());
                                }
                            });
                        }
                        WsEvent::TempPasswordConsumed { device_id } => {
                            info!("[ServiceRuntime] temporary password consumed for device_id={}", device_id);
                            if let Err(err) = rotate_invite_temporary_password(&shared).await {
                                info!("[ServiceRuntime] rotate temporary password failed: {}", err);
                                set_last_error(&shared, err.to_string());
                            }
                        }
                        WsEvent::WOLRequest {
                            request_id,
                            macs,
                            target_ipv4,
                            prefix_len,
                        } => {
                            let client = ws_client.clone();
                            tokio::spawn(async move {
                                let result =
                                    crate::wol::send_magic_packets(&macs, &target_ipv4, prefix_len)
                                        .await;
                                let success = result.is_ok();
                                let code = if success {
                                    "sent".to_string()
                                } else {
                                    "send_failed".to_string()
                                };
                                let _ = client.send_wol_result(request_id, success, code).await;
                            });
                        }
                    }
                }
            }
        }
    }

    cancel_all_wgvpn_jobs(&shared).await;
    if cfg!(any(windows, target_os = "macos")) {
        let punch_lib = load_machine_config()
            .ok()
            .filter(|cfg| !cfg.p2p_punch_path.is_empty())
            .map(|cfg| std::path::PathBuf::from(cfg.p2p_punch_path))
            .unwrap_or_else(crate::config::default_p2p_punch_path);
        if let Err(err) = gonc_ffi::stop_userspace_wg_engine(&punch_lib) {
            warn!(
                "[ServiceRuntime] userspace WG engine shutdown failed: {:#}",
                err
            );
        }
    }
    ws_client.disconnect().await;
    // 先释放 SharedRuntimeState 对 health_server_handle 的 Arc 引用，
    // 再尝试拿回唯一所有权调用 stop()。若 try_unwrap 失败（理论上不会，
    // 因为 shared 已不再被引用），则放弃显式停止——进程退出时 task 自然终止。
    shared.lock().health_server_handle = None;
    if let Some(handle) = health_server {
        match Arc::try_unwrap(handle) {
            Ok(handle) => handle.stop(),
            Err(_) => {
                debug!("[ServiceRuntime] health server handle still shared, skip explicit stop")
            }
        }
    }
    info!("[ServiceRuntime] stopped");
    Ok(())
}

fn spawn_bootstrap_maintenance(
    shared: Arc<Mutex<SharedRuntimeState>>,
    ws_client: ServiceWsClient,
    event_tx: mpsc::UnboundedSender<WsEvent>,
    maintenance_lock: Arc<tokio::sync::Mutex<()>>,
    reason: &'static str,
) {
    tokio::spawn(async move {
        let _maintenance_guard = maintenance_lock.lock().await;
        if let Err(err) = bootstrap_service(&shared, &ws_client, event_tx).await {
            info!("[ServiceRuntime] bootstrap {} failed: {}", reason, err);
            handle_bootstrap_error(&shared, &ws_client, err).await;
        }
    });
}

pub fn request_shutdown() {
    EXTERNAL_SHUTDOWN_REQUESTED.store(true, Ordering::SeqCst);
}

fn persistent_login_enabled(config: &crate::config::MachineConfig) -> bool {
    config.remember_me
        && config.auto_login
        && config.auth_token.is_some()
        && config.refresh_token.is_some()
}

async fn bootstrap_service(
    shared: &Arc<Mutex<SharedRuntimeState>>,
    ws_client: &ServiceWsClient,
    event_tx: mpsc::UnboundedSender<WsEvent>,
) -> Result<()> {
    let mut config = load_machine_config()?;
    let reconnect_requested = shared.lock().reconnect_requested;

    let uuid_created = ensure_device_uuid(&mut config);
    let clone_result = detect_clone(&mut config);
    let config_changed = uuid_created
        || matches!(
            clone_result,
            CloneDetectionResult::BaselineCreated | CloneDetectionResult::CloneDetected
        );
    if config_changed {
        save_machine_config(&config)?;
    }
    match &clone_result {
        CloneDetectionResult::CloneDetected => {
            info!("[device-identity] desktop clone detected; device identity rebuilt");
            ws_client.disconnect().await;
            cancel_all_wgvpn_jobs(shared).await;
            update_status(shared, |status| {
                status.device_identity_rebuilt = true;
                status.device_identity_message = Some(localized_message(
                    status.locale.as_deref(),
                    "device_identity.cloned_rebuilt",
                    &[],
                ));
            });
        }
        CloneDetectionResult::BaselineCreated => {
            info!("[device-identity] desktop fingerprint baseline created");
        }
        CloneDetectionResult::Unavailable(error) => {
            info!("[device-identity] clone fingerprint unavailable: {}", error);
        }
        CloneDetectionResult::Unchanged | CloneDetectionResult::Unsupported => {}
    }

    // Token 始终需要持久化供当前 service 生命周期调用 API，但只有用户明确开启
    // “记住会话 + 自动登录”时，新的 service 进程才可据此恢复登录。
    if !shared.lock().login_session_enabled {
        ws_client.disconnect().await;
        update_status(shared, |status| {
            status.logged_in = false;
            status.ws_connected = false;
            status.device_id = config.device_id;
            status.device_uuid = config.device_uuid.clone();
        });
        shared.lock().reconnect_requested = false;
        return Ok(());
    }

    if config.auth_token.is_none() || config.refresh_token.is_none() {
        ws_client.disconnect().await;
        update_status(shared, |s| {
            s.logged_in = false;
            s.ws_connected = false;
            s.device_id = config.device_id;
            s.device_uuid = config.device_uuid.clone();
        });
        shared.lock().reconnect_requested = false;
        return Ok(());
    }

    if token_expires_soon(config.access_token_expires_at, 300) {
        let _ = refresh_with_config(&mut config).await?;
    }

    // Keep the account identity in process memory only. This is needed before
    // the authenticated WebSocket can deliver an inbound attempt so the
    // passive side can classify same-account versus cross-account requests.
    if shared.lock().current_user_id.is_none() {
        let profile = fetch_profile(&mut config).await?;
        shared.lock().current_user_id = Some(profile.user_id);
    }

    let mut current_device = None;
    if config.device_id.is_none() || config.device_uuid.is_none() {
        current_device = Some(register_current_device_auto(&mut config).await?);
    }

    if reconnect_requested || !ws_client.is_connected().await {
        ws_client.disconnect().await;
        ws_client
            .connect(
                &config.server_url,
                config.auth_token.as_deref().unwrap_or_default(),
                config.device_id.unwrap_or_default(),
                config.device_uuid.as_deref().unwrap_or_default(),
                event_tx,
            )
            .await
            .map_err(|err| anyhow!("failed to connect service websocket: {}", err))?;
        shared.lock().reconnect_requested = false;
    }

    update_status(shared, |s| {
        s.logged_in = true;
        s.device_id = config.device_id;
        s.device_uuid = config.device_uuid.clone();
        if current_device.is_some() {
            s.public_ip = current_device
                .as_ref()
                .and_then(|device| device.public_ip.clone());
            s.public_ip_location = current_device
                .as_ref()
                .and_then(|device| device.public_ip_location.clone());
            s.current_device = current_device;
        }
    });
    Ok(())
}

async fn maybe_report_device_status(
    shared: &Arc<Mutex<SharedRuntimeState>>,
    last_status: &mut Option<DeviceStatusReport>,
    last_report_at: &mut Option<Instant>,
) -> Result<()> {
    let mut config = load_machine_config()?;
    if config.auth_token.is_none() || config.device_id.is_none() {
        return Ok(());
    }
    let status = collect_device_status_report(&mut config).await?;
    let status_changed = last_status.as_ref() != Some(&status);
    let fallback_due = last_report_at
        .map(|at| at.elapsed() >= Duration::from_secs(DEVICE_STATUS_REPORT_FALLBACK_SECS))
        .unwrap_or(true);
    if !status_changed && !fallback_due {
        return Ok(());
    }
    send_device_status_report(&mut config, &status).await?;
    *last_status = Some(status);
    *last_report_at = Some(Instant::now());
    update_status(shared, |s| {
        s.last_heartbeat_at = Some(now_ts());
    });
    Ok(())
}

async fn refresh_network_info(shared: &Arc<Mutex<SharedRuntimeState>>) -> Result<Option<String>> {
    let mut config = load_machine_config()?;
    let public_network = crate::device::refresh_public_network_info(&mut config, true).await;
    let public_ip = (!public_network.ip.is_empty()).then(|| public_network.ip.clone());
    let public_ip_location =
        (!public_network.location.is_empty()).then(|| public_network.location.clone());
    if config.auth_token.is_some() && config.device_id.is_some() {
        // 状态上报是尽力而为：本地保存的凭据已失效（如自动登录的账号密码被改）
        // 时上报必然失败，不应阻断网络信息刷新——启动预检继续走完后由
        // try_auto_login 自然回落到登录页，周期心跳（maybe_report_device_status）
        // 也会自行重试上报。
        let report = async {
            let status = collect_device_status_report(&mut config).await?;
            send_device_status_report(&mut config, &status).await
        }
        .await;
        if let Err(err) = report {
            warn!("[ServiceRuntime] device status report failed: {}", err);
        }
    }
    update_status(shared, |s| {
        s.public_ip = public_ip.clone();
        s.public_ip_location = public_ip_location.clone();
        if let Some(device) = &mut s.current_device {
            device.public_ip = public_ip.clone();
            device.public_ip_location = public_ip_location.clone();
        }
    });
    Ok(public_ip)
}

async fn maybe_refresh_auth(
    shared: &Arc<Mutex<SharedRuntimeState>>,
    wake: &Arc<Notify>,
) -> Result<()> {
    let mut config = load_machine_config()?;
    if config.refresh_token.is_none() {
        return Ok(());
    }
    if token_expires_soon(config.access_token_expires_at, 600) {
        let _ = refresh_with_config(&mut config).await?;
        shared.lock().reconnect_requested = true;
        wake.notify_one();
    }
    Ok(())
}

fn set_last_error(shared: &Arc<Mutex<SharedRuntimeState>>, error: String) {
    error!("[ServiceRuntime] {}", error);
    update_status(shared, |s| {
        s.last_error = Some(error);
    });
}

fn is_auth_session_error(error: &str) -> bool {
    let error = error.to_ascii_lowercase();
    [
        "401",
        "403",
        "unauthorized",
        "forbidden",
        "invalid token",
        "expired token",
        "token expired",
        "无效或已过期的令牌",
        "令牌已过期",
    ]
    .iter()
    .any(|needle| error.contains(needle))
}

async fn handle_bootstrap_error(
    shared: &Arc<Mutex<SharedRuntimeState>>,
    ws_client: &ServiceWsClient,
    err: anyhow::Error,
) {
    let error = err.to_string();
    if !is_auth_session_error(&error) {
        set_last_error(shared, error);
        return;
    }

    warn!("[ServiceRuntime] saved session is no longer valid; signing out");
    ws_client.disconnect().await;
    match load_machine_config().and_then(|mut config| {
        clear_machine_credentials(&mut config);
        save_machine_config(&config)
    }) {
        Ok(()) => {}
        Err(clear_err) => error!(
            "[ServiceRuntime] failed to clear invalid saved credentials: {}",
            clear_err
        ),
    }
    {
        let mut state = shared.lock();
        state.login_session_enabled = false;
        state.reconnect_requested = false;
        state.current_user_id = None;
    }
    update_status(shared, |status| {
        status.logged_in = false;
        status.ws_connected = false;
        status.device_id = None;
        status.last_error = Some(localized_message(
            status.locale.as_deref(),
            "auth.token_expired",
            &[],
        ));
    });
}

fn token_expires_soon(expires_at: Option<i64>, threshold_secs: i64) -> bool {
    match expires_at {
        Some(expires_at) => expires_at <= now_ts() + threshold_secs,
        None => true,
    }
}

fn now_ts() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|duration| duration.as_secs() as i64)
        .unwrap_or_default()
}

pub(super) fn refresh_pending_inbound_approvals(shared: &Arc<Mutex<SharedRuntimeState>>) {
    let pending = {
        let state = shared.lock();
        state
            .pending_inbound_approval
            .as_ref()
            .map(|item| vec![item.status.clone()])
            .unwrap_or_default()
    };
    update_status(shared, |status| {
        status.pending_inbound_approvals = pending;
    });
}

// ---- IPC 控制服务器 ----

fn classify_tunnel_error_code(message: &str) -> &'static str {
    let message = message.to_lowercase();
    let structured_code = message
        .split_once(':')
        .map(|(code, _)| code.trim())
        .unwrap_or(message.trim());
    if let Some(code) = [
        "hole_punch_wait_timeout",
        "peer_prepare_timeout",
        "peer_offline",
        "peer_cancelled",
        "user_cancelled",
        "invalid_lan_cidr",
        "config_load_failed",
        "passive_start_failed",
        "wireguard_config_failed",
        "wireguard_handshake_failed",
        "approval_denied",
        "approval_timeout",
        "approval_request_missing",
        "approval_busy",
        "internal_error",
    ]
    .into_iter()
    .find(|code| *code == structured_code)
    {
        return code;
    }
    if matches!(structured_code, "punch_exhausted" | "traversal_signal_timeout")
        || message.starts_with("hole_punch_wait_timeout:")
        || (message.contains("exchange failed:") && message.contains("timeout"))
        || (message.contains("udp tunnel failed:")
            && (message.contains("timeout")
                || message.contains("direct p2p connection failed")
                || message.contains("failed to establish gonc p2p tunnel")))
    {
        "hole_punch_wait_timeout"
    } else if message.contains("invalid") && message.contains("cidr") {
        "invalid_lan_cidr"
    } else if message.contains("offline") {
        "peer_offline"
    } else if message.contains("cancel") {
        "user_cancelled"
    } else if message.contains("wireguard") && message.contains("handshake") {
        "wireguard_handshake_failed"
    } else if message.contains("wireguard") {
        "wireguard_config_failed"
    } else {
        "internal_error"
    }
}

fn is_retryable_tunnel_error(message: &str) -> bool {
    const RETRYABLE_ERROR_CODES: &[&str] = &["hole_punch_wait_timeout"];
    let code = classify_tunnel_error_code(message);
    RETRYABLE_ERROR_CODES.contains(&code)
}

fn is_non_retryable_wgvpn_error(message: &str) -> bool {
    let message = message.to_lowercase();
    [
        "not logged in",
        "device not registered",
        "unauthorized",
        "forbidden",
        "401",
        "403",
        "invalid token",
        "expired token",
        "无效或已过期的令牌",
        "未登录",
        "设备未注册",
        "only passive wgvpn may expose lan cidrs",
        "invalid ipv4 cidr",
        "invalid exposed lan cidr",
        "exposed lan cidr overlaps",
        "subnet router backend is not implemented",
        "approval_denied",
        "approval_timeout",
        "approval_request_missing",
        "approval_busy",
    ]
    .iter()
    .any(|needle| message.contains(needle))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn passive_tunnel_popup_uses_runtime_username_or_explicit_unknown_user() {
        let (title, body) =
            tunnel_jobs::passive_tunnel_popup_text(Some("zh-CN"), "alice", "笔记本上虚拟机60");
        assert_eq!(title, "远程设备已连接");
        assert_eq!(body, "用户 alice 已使用设备 笔记本上虚拟机60 连接到本机。");

        let (_, unknown_body) =
            tunnel_jobs::passive_tunnel_popup_text(Some("zh-CN"), "", "设备 #42");
        assert_eq!(unknown_body, "未知用户已使用设备 设备 #42 连接到本机。");

        let (english_title, english_body) =
            tunnel_jobs::passive_tunnel_popup_text(Some("en"), "alice", "Laptop");
        assert_eq!(english_title, "Remote device connected");
        assert_eq!(
            english_body,
            "User alice connected to this device using Laptop."
        );
    }

    #[test]
    fn attempt_start_carries_source_device_identity_to_passive_peer() {
        let message = P2PAttemptMessage::AttemptStart {
            protocol_version: 1,
            client_job_id: "job-1".to_string(),
            attempt_id: "attempt-1".to_string(),
            attempt: 1,
            max_attempts: 30,
            punch_token: "token".to_string(),
            source_user_id: 7,
            source_username: "alice".to_string(),
            source_email: "alice@example.com".to_string(),
            source_device_name: "Living Room PC".to_string(),
            source_device_alias: "客厅电脑".to_string(),
            traversal_negotiation: None,
        };

        let encoded = serde_json::to_string(&message).expect("serialize attempt start");
        let decoded: P2PAttemptMessage =
            serde_json::from_str(&encoded).expect("deserialize attempt start");

        match decoded {
            P2PAttemptMessage::AttemptStart {
                source_device_name,
                source_device_alias,
                ..
            } => {
                assert_eq!(source_device_name, "Living Room PC");
                assert_eq!(source_device_alias, "客厅电脑");
            }
            _ => panic!("expected attempt_start"),
        }
    }

    #[test]
    fn p2p_attempt_error_is_structured_and_localized() {
        let error = P2PAttemptErrorPayload::new(
            "wireguard_handshake_failed",
            "wireguard handshake timeout",
        );
        let json = serde_json::to_value(&error).unwrap();
        assert_eq!(
            json["message_key"],
            "p2p.attempt.wireguard_handshake_failed"
        );
        assert_eq!(
            error.localized_message(Some("en")),
            "WireGuard handshake failed: wireguard handshake timeout"
        );
    }

    #[test]
    fn p2p_attempt_error_keeps_old_payload_compatibility() {
        let error: P2PAttemptErrorPayload =
            serde_json::from_str(r#"{"code":"legacy_error","message":"legacy message"}"#).unwrap();
        assert_eq!(error.localized_message(Some("en")), "legacy message");
    }

    #[test]
    fn only_hole_punch_wait_timeout_is_retryable() {
        assert!(is_retryable_tunnel_error(
            "hole_punch_wait_timeout: rendezvous timed out"
        ));
        assert!(is_retryable_tunnel_error(
            "exchange failed: wait response timeout"
        ));
        assert!(is_retryable_tunnel_error(
            "udp tunnel failed: failed to establish gonc p2p tunnel: direct P2P connection failed"
        ));
        assert!(is_retryable_tunnel_error(
            "udp tunnel failed: gonc hole punch timeout"
        ));
        for fatal in [
            "peer_prepare_timeout",
            "peer_offline",
            "invalid_lan_cidr: invalid IPv4 CIDR",
            "wireguard_config_failed: access denied",
            "internal_error: unexpected response",
        ] {
            assert!(!is_retryable_tunnel_error(fatal), "{fatal}");
        }
    }

    #[test]
    fn structured_attempt_error_code_is_preserved_for_server_reporting() {
        assert_eq!(
            classify_tunnel_error_code("peer_prepare_timeout: peer did not become ready"),
            "peer_prepare_timeout"
        );
        assert_eq!(
            classify_tunnel_error_code("config_load_failed: access denied"),
            "config_load_failed"
        );
    }

    #[test]
    fn relocalize_runtime_status_updates_existing_job_messages() {
        let mut status = RuntimeStatus {
            locale: Some("zh-CN".to_string()),
            active_tunnel_jobs: vec![ActiveTunnelJobStatus {
                tcp_retry_recommended: false,
                target_device_id: 29,
                target_device_uuid: "peer-29".to_string(),
                state: ActiveTunnelJobState::Waiting,
                attempt: 2,
                max_attempts: ACTIVE_TUNNEL_JOB_MAX_ATTEMPTS,
                message: "本次建立失败：等待打洞响应超时；60 秒后自动重试".to_string(),
                result: None,
                updated_at: 1,
            }],
            tunnel_lifecycles: vec![TunnelLifecycleStatus {
                peer_device_id: 29,
                source_user_id: 0,
                source_username: String::new(),
                source_email: String::new(),
                peer_device_name: String::new(),
                peer_device_alias: String::new(),
                peer_public_ip: String::new(),
                role: TunnelLifecycleRole::Active,
                state: TunnelLifecycleState::Connecting,
                attempt: 2,
                max_attempts: ACTIVE_TUNNEL_JOB_MAX_ATTEMPTS,
                stage: Some("retry_wait".to_string()),
                virtual_ip: None,
                peer_virtual_ip: None,
                last_result: TunnelLastResult::None,
                error_code: Some("hole_punch_wait_timeout".to_string()),
                message: Some("本次建立失败：等待打洞响应超时；60 秒后自动重试".to_string()),
                health_failures: 0,
                health_grace_deadline: None,
                connected_at: None,
                updated_at: 1,
            }],
            ..RuntimeStatus::default()
        };

        status.locale = Some("en".to_string());
        relocalize_runtime_status(&mut status);

        assert_eq!(
            status.active_tunnel_jobs[0].message,
            "Establishment failed: Timed out waiting for hole-punch response; retrying in 60s"
        );
        assert_eq!(
            status.tunnel_lifecycles[0].message.as_deref(),
            Some(
                "Establishment failed: Timed out waiting for hole-punch response; retrying in 60s"
            )
        );
    }

    #[test]
    fn lan_cidrs_for_config_are_normalized_and_deduped() {
        let cidrs = normalize_lan_cidrs_for_config(vec![
            "192.0.2.181/24".to_string(),
            "192.0.2.0/24".to_string(),
            "192.0.2.130/32".to_string(),
        ])
        .unwrap();

        assert_eq!(cidrs, vec!["192.0.2.0/24", "192.0.2.130/32"]);
    }

    #[test]
    fn stale_wgvpn_generation_cannot_overwrite_or_remove_current_job() {
        let shared = Arc::new(Mutex::new(SharedRuntimeState::default()));
        let (cancel_tx, _cancel_rx) = watch::channel(false);
        shared.lock().wgvpn_job_cancels.insert(
            29,
            WgvpnJobControl {
                generation: 2,
                cancel_tx,
            },
        );

        let stale_status = WgvpnJobStatus {
            peer_device_id: 29,
            is_active: false,
            state: WgvpnJobState::Running,
            attempt: 1,
            max_attempts: WGVPN_JOB_MAX_ATTEMPTS,
            message: "stale token".to_string(),
            updated_at: 1,
        };
        assert!(!update_wgvpn_job_status(&shared, stale_status, Some(1)));
        remove_wgvpn_job_if_current(&shared, 29, 1);

        let state = shared.lock();
        assert!(state.status.wgvpn_jobs.is_empty());
        assert_eq!(state.wgvpn_job_cancels.get(&29).unwrap().generation, 2);
    }

    #[test]
    fn stale_health_monitor_event_cannot_change_current_session() {
        let shared = Arc::new(Mutex::new(SharedRuntimeState::default()));
        let (stop_tx, _stop_rx) = watch::channel(false);
        let (speed_tx, _speed_rx) = mpsc::unbounded_channel();
        shared.lock().wgvpn_health_controls.insert(
            29,
            WgvpnHealthControl {
                generation: 2,
                stop_tx,
                speed_tx,
                speed_test_busy: Arc::new(AtomicBool::new(false)),
            },
        );

        record_wgvpn_health_failure(shared.clone(), 29, Some(1));
        assert!(!shared.lock().wgvpn_health_runtime.contains_key(&29));

        record_wgvpn_health_failure(shared.clone(), 29, Some(2));
        assert_eq!(
            shared
                .lock()
                .wgvpn_health_runtime
                .get(&29)
                .unwrap()
                .consecutive_failures,
            1
        );
    }

    #[tokio::test]
    async fn health_enters_degraded_after_twelve_failures() {
        let shared = Arc::new(Mutex::new(SharedRuntimeState::default()));
        record_wgvpn_health_success(&shared, 29, None, Some(42));
        assert_eq!(shared.lock().wgvpn_health_runtime[&29].latency_ms, Some(42));

        for _ in 0..(WGVPN_HEALTH_FAILURE_THRESHOLD - 1) {
            record_wgvpn_health_failure(shared.clone(), 29, None);
        }
        assert_eq!(
            shared.lock().wgvpn_health_runtime[&29].state,
            WgvpnHealthState::Connected
        );

        record_wgvpn_health_failure(shared.clone(), 29, None);
        let state = shared.lock();
        let health = &state.wgvpn_health_runtime[&29];
        assert_eq!(health.state, WgvpnHealthState::Degraded);
        assert_eq!(health.consecutive_failures, WGVPN_HEALTH_FAILURE_THRESHOLD);
        assert_eq!(health.latency_ms, None);
        assert!(health.grace_deadline.is_some());
    }

    #[test]
    fn recovered_session_cannot_be_claimed_by_expired_grace_task() {
        let mut state = SharedRuntimeState::default();
        let (cancel_tx, _cancel_rx) = watch::channel(false);
        state.passive_health_grace_controls.insert(
            29,
            PassiveHealthGraceControl {
                generation: 7,
                cancel_tx,
            },
        );
        state
            .wgvpn_health_runtime
            .insert(29, WgvpnHealthRuntime::default());

        assert!(!claim_wgvpn_cleanup_locked(
            &mut state,
            29,
            CleanupGuard::Grace(7)
        ));
        assert!(state.wgvpn_health_runtime.contains_key(&29));
    }

    #[test]
    fn websocket_auth_failures_are_not_retried_as_network_errors() {
        assert!(is_auth_session_error(
            "failed to connect service websocket: HTTP error: 403 Forbidden"
        ));
        assert!(is_auth_session_error("401 Unauthorized"));
        assert!(is_auth_session_error("expired token"));
        assert!(!is_auth_session_error("connection reset by peer"));
        assert!(!is_auth_session_error("operation timed out"));
    }

    #[test]
    fn passive_watchdog_only_expires_without_health_connection() {
        let mut state = SharedRuntimeState::default();
        state.passive_health_watchdog_generations.insert(29, 7);
        assert!(!claim_passive_health_watchdog_timeout(&mut state, 29, 6));
        assert_eq!(state.passive_health_watchdog_generations.get(&29), Some(&7));

        state.passive_health_connection_generations.insert(29, 3);
        assert!(!claim_passive_health_watchdog_timeout(&mut state, 29, 7));
        assert!(!state.passive_health_watchdog_generations.contains_key(&29));

        state.passive_health_connection_generations.remove(&29);
        state.passive_health_watchdog_generations.insert(29, 8);
        assert!(claim_passive_health_watchdog_timeout(&mut state, 29, 8));
    }

    #[test]
    fn only_current_monitor_generation_can_claim_immediate_cleanup() {
        let mut state = SharedRuntimeState::default();
        let (stop_tx, _stop_rx) = watch::channel(false);
        let (speed_tx, _speed_rx) = mpsc::unbounded_channel();
        state.wgvpn_health_controls.insert(
            29,
            WgvpnHealthControl {
                generation: 4,
                stop_tx,
                speed_tx,
                speed_test_busy: Arc::new(AtomicBool::new(false)),
            },
        );
        state
            .wgvpn_health_runtime
            .insert(29, WgvpnHealthRuntime::default());

        assert!(!claim_wgvpn_cleanup_locked(
            &mut state,
            29,
            CleanupGuard::Monitor(3)
        ));
        assert!(claim_wgvpn_cleanup_locked(
            &mut state,
            29,
            CleanupGuard::Monitor(4)
        ));
    }

    #[test]
    fn service_restart_login_requires_preferences_and_tokens() {
        let mut config = crate::config::MachineConfig::default();
        config.auth_token = Some("token".to_string());
        config.refresh_token = Some("refresh".to_string());
        assert!(!persistent_login_enabled(&config));

        config.remember_me = true;
        config.auto_login = true;
        assert!(persistent_login_enabled(&config));

        config.auto_login = false;
        assert!(!persistent_login_enabled(&config));
    }

    #[test]
    fn negotiated_punch_failures_retry_but_invalid_plans_and_cancellation_do_not() {
        for error in ["punch_exhausted", "punch_exhausted:udp4,udp6", "punch_exhausted:tcp4,udp4,udp6", "traversal_signal_timeout"] {
            assert_eq!(classify_tunnel_error_code(error), "hole_punch_wait_timeout");
            assert!(is_retryable_tunnel_error(error));
        }
        for error in ["invalid_traversal_sequence", "traversal_plan_mismatch", "traversal_cancelled", "approval_denied", "peer_offline"] {
            assert!(!is_retryable_tunnel_error(error));
        }
    }

    fn passive_lifecycle(peer_device_id: i64) -> TunnelLifecycleStatus {
        TunnelLifecycleStatus {
            peer_device_id,
            source_user_id: 0,
            source_username: String::new(),
            source_email: String::new(),
            peer_device_name: String::new(),
            peer_device_alias: String::new(),
            peer_public_ip: String::new(),
            role: TunnelLifecycleRole::Passive,
            state: TunnelLifecycleState::Connecting,
            attempt: 1,
            max_attempts: ACTIVE_TUNNEL_JOB_MAX_ATTEMPTS,
            stage: None,
            virtual_ip: None,
            peer_virtual_ip: None,
            last_result: TunnelLastResult::None,
            error_code: None,
            message: None,
            health_failures: 0,
            health_grace_deadline: None,
            connected_at: None,
            updated_at: 1,
        }
    }

    #[test]
    fn upsert_lifecycle_preserves_source_user_when_new_value_is_empty() {
        // Bug1 回归：AttemptStart 已写入 source_user，后续 Running/connected 路径
        // 用空 source_user 整体 upsert 时，必须保留已有值，不能覆盖成空。
        let mut status = RuntimeStatus::default();

        // 第一次：AttemptStart 写入正确用户名。
        let mut attempt = passive_lifecycle(31);
        attempt.source_username = "alice".to_string();
        attempt.source_user_id = 7;
        attempt.source_email = "alice@example.com".to_string();
        attempt.peer_device_name = "Alice PC".to_string();
        attempt.peer_device_alias = "Office".to_string();
        attempt.peer_public_ip = "203.0.113.10".to_string();
        upsert_tunnel_lifecycle(&mut status, attempt);

        // 第二次：被动端 Running 首次写入（source_user 尚未到达，全空）。
        let mut running = passive_lifecycle(31);
        running.state = TunnelLifecycleState::Connected;
        upsert_tunnel_lifecycle(&mut status, running);

        let got = &status.tunnel_lifecycles[0];
        assert_eq!(got.source_username, "alice");
        assert_eq!(got.source_user_id, 7);
        assert_eq!(got.source_email, "alice@example.com");
        assert_eq!(got.peer_device_name, "Alice PC");
        assert_eq!(got.peer_device_alias, "Office");
        assert_eq!(got.peer_public_ip, "203.0.113.10");
    }

    #[test]
    fn upsert_lifecycle_records_connected_at_only_on_first_connected() {
        // Bug2 回归：连接时间只在首次进入 Connected 时记录一次，
        // 后续心跳刷新（仍 Connected）不更新 connected_at。
        let mut status = RuntimeStatus::default();

        // Connecting 态：不记录 connected_at。
        upsert_tunnel_lifecycle(&mut status, passive_lifecycle(42));
        assert!(status.tunnel_lifecycles[0].connected_at.is_none());

        // 首次转 Connected：记录 connected_at。
        let mut connected = passive_lifecycle(42);
        connected.state = TunnelLifecycleState::Connected;
        upsert_tunnel_lifecycle(&mut status, connected);
        let first_connected_at = status.tunnel_lifecycles[0].connected_at;
        assert!(first_connected_at.is_some());

        // 再次 Connected（心跳刷新）：connected_at 不变。
        let mut refresh = passive_lifecycle(42);
        refresh.state = TunnelLifecycleState::Connected;
        upsert_tunnel_lifecycle(&mut status, refresh);
        let after = now_ts();
        assert_eq!(
            status.tunnel_lifecycles[0].connected_at, first_connected_at,
            "connected_at must be stable across subsequent connected refreshes"
        );
        // 即使经过了一次 now_ts() 调用，记录的值也不应晚于当前时刻。
        if let Some(ts) = status.tunnel_lifecycles[0].connected_at {
            assert!(ts <= after && ts >= 1);
        }
    }

    #[test]
    fn upsert_lifecycle_disconnect_keeps_state_not_established() {
        // 断开后 lifecycle state 应为 NotEstablished；前端仅在 connected/recovering
        // 态渲染连接时间，connected_at 保留与否不影响显示。
        let mut status = RuntimeStatus::default();

        let mut connected = passive_lifecycle(99);
        connected.state = TunnelLifecycleState::Connected;
        upsert_tunnel_lifecycle(&mut status, connected);
        assert!(status.tunnel_lifecycles[0].connected_at.is_some());

        let mut disconnected = passive_lifecycle(99);
        disconnected.state = TunnelLifecycleState::NotEstablished;
        upsert_tunnel_lifecycle(&mut status, disconnected);
        assert_eq!(
            status.tunnel_lifecycles[0].state,
            TunnelLifecycleState::NotEstablished
        );
    }
}
