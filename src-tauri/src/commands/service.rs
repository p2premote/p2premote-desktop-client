use std::collections::VecDeque;
use std::path::PathBuf;
use std::sync::Arc;

use once_cell::sync::OnceCell;
use parking_lot::Mutex;
use serde::Serialize;
use tauri::{AppHandle, Emitter, Manager};
use tokio::sync::{mpsc, oneshot};
use tracing::{debug, warn};

use p2premote_core::config::{
    default_service_binary_name, machine_config_path, machine_log_dir, platform_executable_name,
};
use p2premote_core::control::{connect_with_handshake, send_command, Data, RuntimeStatus};
use p2premote_core::service_control::{
    query_service_status, start_service, stop_service, ServiceStatus,
};

// ---- UI 侧持久连接管理 ----

/// 带回调确认的 IPC 命令（fire-and-forget 模式）
struct PendingCommand {
    data: Data,
    /// conn.send() 完成后通过这个 channel 回复结果
    done: oneshot::Sender<Result<(), String>>,
}

/// FIFO 队列条目：区分 fire-and-forget 和 request-response
enum PendingQueueEntry {
    /// fire-and-forget 命令的服务端响应，直接丢弃
    FireAndForget,
    /// request-response 命令，通过 channel 回传响应给调用方
    WaitForResponse(oneshot::Sender<Result<Data, String>>),
}

struct ServiceConnection {
    /// fire-and-forget 命令
    sender: mpsc::UnboundedSender<PendingCommand>,
    /// request-response 命令（FIFO 匹配，不需要 request_id）
    req_tx: mpsc::UnboundedSender<(Data, oneshot::Sender<Result<Data, String>>)>,
}

static SERVICE_CONNECTION: OnceCell<Arc<Mutex<Option<ServiceConnection>>>> = OnceCell::new();
static SERVICE_SESSION_LOCK: OnceCell<tokio::sync::Mutex<()>> = OnceCell::new();
static LAST_KNOWN_STATUS: OnceCell<Arc<Mutex<Option<RuntimeStatus>>>> = OnceCell::new();
// SCM 冷启动最多会等待 10 秒 IPC，就绪检查的总超时必须留出状态收集余量。
const SERVICE_CHECK_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(15);
const IPC_COMMAND_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(10);

fn get_connection_holder() -> &'static Arc<Mutex<Option<ServiceConnection>>> {
    SERVICE_CONNECTION.get_or_init(|| Arc::new(Mutex::new(None)))
}

fn get_service_session_lock() -> &'static tokio::sync::Mutex<()> {
    SERVICE_SESSION_LOCK.get_or_init(|| tokio::sync::Mutex::new(()))
}

fn get_last_known_status() -> &'static Arc<Mutex<Option<RuntimeStatus>>> {
    LAST_KNOWN_STATUS.get_or_init(|| Arc::new(Mutex::new(None)))
}

/// 确保持久连接存在，不存在则创建。返回 true 表示已连接，false 表示 service 未运行
async fn ensure_persistent_connection(app: &AppHandle) -> Result<bool, String> {
    let holder = get_connection_holder();
    {
        let guard = holder.lock();
        if guard.is_some() {
            return Ok(true);
        }
    }

    // 尝试建立持久连接
    let conn = match connect_with_handshake().await {
        Ok(c) => c,
        Err(e) => {
            debug!("[ServiceIPC] service not available: {}", e);
            return Ok(false);
        }
    };

    let (tx, rx) = mpsc::unbounded_channel::<PendingCommand>();
    let (req_tx, req_rx) = mpsc::unbounded_channel();
    *holder.lock() = Some(ServiceConnection { sender: tx, req_tx });

    // spawn 事件监听循环
    let app_clone = app.clone();
    let holder_clone = holder.clone();
    tokio::spawn(async move {
        run_connection_loop(conn, rx, req_rx, &app_clone).await;
        *holder_clone.lock() = None;
        // [P2] 连接断开，清空缓存状态，避免 UI 显示过期数据
        *get_last_known_status().lock() = None;
        // 通知 UI 连接已断开，触发重连
        let _ = app_clone.emit("service-connection-lost", ());
        debug!("[ServiceIPC] persistent connection closed");
    });

    Ok(true)
}

/// 持久连接的读写循环
async fn run_connection_loop(
    mut conn: p2premote_core::control::Connection<p2premote_core::control::IpcStream>,
    mut rx: mpsc::UnboundedReceiver<PendingCommand>,
    mut req_rx: mpsc::UnboundedReceiver<(Data, oneshot::Sender<Result<Data, String>>)>,
    app: &AppHandle,
) {
    // FIFO 队列：服务端单线程处理命令，响应顺序与请求一致
    // [P1] fire-and-forget 也入队，其响应由后台循环消费丢弃，不污染 request-response 匹配
    let mut pending_queue: VecDeque<PendingQueueEntry> = VecDeque::new();

    loop {
        tokio::select! {
            // 服务端推送 / 响应
            result = conn.next() => {
                let data = match result {
                    Ok(Some(d)) => d,
                    Ok(None) | Err(_) => break,
                };
                match data {
                    Data::StatusChanged(status) => {
                        *get_last_known_status().lock() = Some(status.clone());
                        let _ = app.emit("service-status-changed", &status);
                    }
                    Data::CommandResponse { .. } => {
                        match pending_queue.pop_front() {
                            Some(PendingQueueEntry::WaitForResponse(done)) => {
                                let _ = done.send(Ok(data));
                            }
                            Some(PendingQueueEntry::FireAndForget) => {
                                // fire-and-forget 的响应，丢弃
                            }
                            None => {
                                tracing::warn!("[ServiceIPC] received CommandResponse with no pending request");
                            }
                        }
                    }
                    other => {
                        tracing::debug!("[ServiceIPC] received: {:?}", other);
                    }
                }
            }
            // fire-and-forget 命令
            cmd = rx.recv() => {
                match cmd {
                    Some(pending) => {
                        let result = conn.send(&pending.data).await
                            .map_err(|e| format!("ipc send failed: {}", e));
                        let _ = pending.done.send(result.clone());
                        if result.is_err() {
                            break;
                        }
                        // 入队标记：等待消费服务端的响应
                        pending_queue.push_back(PendingQueueEntry::FireAndForget);
                    }
                    None => break,
                }
            }
            // request-response 命令
            req = req_rx.recv() => {
                match req {
                    Some((data, done)) => {
                        if let Err(e) = conn.send(&data).await {
                            let _ = done.send(Err(format!("ipc send failed: {}", e)));
                            break;
                        }
                        pending_queue.push_back(PendingQueueEntry::WaitForResponse(done));
                    }
                    None => break,
                }
            }
        }
    }
}

/// 通过持久连接发送命令，等待后台 task 确认实际写入 socket 后返回
async fn send_via_persistent(data: Data) -> Result<(), String> {
    let (done_tx, done_rx) = oneshot::channel();
    let pending = PendingCommand {
        data,
        done: done_tx,
    };

    // scope 确保 MutexGuard 在 await 前释放
    {
        let holder = get_connection_holder();
        let guard = holder.lock();
        if let Some(ref conn) = *guard {
            conn.sender
                .send(pending)
                .map_err(|e| format!("failed to send: {}", e))?;
        } else {
            return Err("no persistent connection".to_string());
        }
    }

    // 等待后台 task 的 conn.send() 结果
    match tokio::time::timeout(IPC_COMMAND_TIMEOUT, done_rx).await {
        Ok(Ok(result)) => result,
        Ok(Err(_)) => Err("persistent connection died while sending".to_string()),
        Err(_) => Err("persistent connection send timed out".to_string()),
    }
}

/// 通过持久连接发送命令并等待服务端响应（request-response 模式）
async fn send_req_via_persistent(data: Data) -> Result<Data, String> {
    let (done_tx, done_rx) = oneshot::channel();

    {
        let holder = get_connection_holder();
        let guard = holder.lock();
        if let Some(ref conn) = *guard {
            conn.req_tx
                .send((data, done_tx))
                .map_err(|e| format!("failed to send: {}", e))?;
        } else {
            return Err("no persistent connection".to_string());
        }
    }

    match tokio::time::timeout(IPC_COMMAND_TIMEOUT, done_rx).await {
        Ok(Ok(result)) => result,
        Ok(Err(_)) => Err("persistent connection died while waiting for response".to_string()),
        Err(_) => Err("persistent connection response timed out".to_string()),
    }
}

#[derive(Debug, Clone, Serialize)]
pub struct ServiceStatusResponse {
    pub service: ServiceStatus,
    pub runtime: Option<RuntimeStatus>,
    pub machine_logged_in: bool,
    pub config_path: String,
    pub log_dir: String,
}

#[derive(Debug, Clone, Serialize)]
pub struct RequiredClientFile {
    pub name: String,
    pub path: String,
}

fn resolve_binary_from_install_layout(
    app: &AppHandle,
    binary_name: &str,
    extra_candidates: Vec<PathBuf>,
) -> Result<PathBuf, String> {
    let current_exe = std::env::current_exe().map_err(|e| e.to_string())?;
    let exe_dir = current_exe
        .parent()
        .ok_or_else(|| crate::commands::localized("errors.resolve_install_dir", &[]))?;

    let mut candidates = Vec::new();
    candidates.extend(extra_candidates);

    candidates.push(exe_dir.join(binary_name));
    candidates.push(exe_dir.join("resources").join(binary_name));
    candidates.push(exe_dir.join("deps").join(binary_name));
    candidates.push(exe_dir.join("service").join(binary_name));

    if let Ok(resource_dir) = app.path().resource_dir() {
        candidates.push(resource_dir.join(binary_name));
        candidates.push(resource_dir.join("resources").join(binary_name));
        candidates.push(resource_dir.join("deps").join(binary_name));
    }

    candidates.push(PathBuf::from(binary_name));
    candidates.push(PathBuf::from("deps").join(binary_name));

    for candidate in candidates {
        if candidate.exists() {
            return Ok(candidate);
        }
    }

    let dir_display = exe_dir.display().to_string();
    Err(crate::commands::localized(
        "errors.binary_not_found",
        &[("name", binary_name), ("path", &dir_display)],
    ))
}

/// 解析 service 可执行文件路径：在 exe 同级目录及常见安装布局下搜索。
///
/// 复用 [`resolve_binary_from_install_layout`]（候选列表最全），并额外注入
/// Linux 下 `default_service_path()` 作为首选候选。config.rs 的开机自启逻辑
/// 也走此函数，避免两份候选列表漂移。
pub(crate) fn resolve_service_executable(app: &AppHandle) -> Result<PathBuf, String> {
    let binary_name = default_service_binary_name();
    // Linux 下额外把 default_service_path() 作为首选候选
    #[cfg(not(windows))]
    let extra = vec![p2premote_core::config::default_service_path()];
    #[cfg(windows)]
    let extra = Vec::new();
    resolve_binary_from_install_layout(app, &binary_name, extra)
}

#[tauri::command]
pub fn check_required_client_files(app: AppHandle) -> Result<Vec<RequiredClientFile>, String> {
    let mut files = Vec::new();

    let service_path = resolve_service_executable(&app)?;
    files.push(RequiredClientFile {
        name: default_service_binary_name().to_string(),
        path: service_path.to_string_lossy().to_string(),
    });

    let cli_name = platform_executable_name("p2premote-cli");
    let cli_path = resolve_binary_from_install_layout(&app, &cli_name, Vec::new())?;
    files.push(RequiredClientFile {
        name: cli_name,
        path: cli_path.to_string_lossy().to_string(),
    });

    Ok(files)
}

async fn maybe_notify_service(command: Data) -> bool {
    // 优先用持久连接，失败则回退到一次性连接
    if send_via_persistent(command.clone()).await.is_err() {
        if let Err(err) = send_command(command).await {
            warn!("[service] notify service failed: {}", err);
            return false;
        }
    }
    true
}

async fn collect_service_status() -> Result<ServiceStatusResponse, String> {
    let mut service = query_service_status().map_err(|e| e.to_string())?;

    // 优先使用持久连接缓存的状态（避免每次建新连接）
    let cached_status = get_last_known_status().lock().clone();
    let runtime = if let Some(cached) = cached_status {
        Some(cached)
    } else {
        match send_command(Data::Status).await {
            Ok(Data::CommandResponse {
                status: Some(s), ..
            }) => Some(s),
            Ok(Data::CommandResponse { .. }) => None,
            Ok(_) => None,
            Err(err) => {
                if service.running {
                    warn!("[service] query runtime status failed: {}", err);
                }
                None
            }
        }
    };

    if runtime.is_some() && !service.running {
        service.running = true;
        if service.raw_state.is_empty() {
            service.raw_state = "ForegroundIpc".to_string();
        }
    }

    // SCM 说 running 但 IPC 实际不通 → 进程已坏，降级为 not running
    if service.running && runtime.is_none() {
        warn!(
            "[service] SCM reports running but IPC unavailable, marking as not running (raw_state={})",
            service.raw_state
        );
        service.running = false;
    }

    // machine_logged_in 从 service 运行时状态派生（不读受保护的 machine config）
    let machine_logged_in = runtime.as_ref().map(|r| r.logged_in).unwrap_or(false);

    Ok(ServiceStatusResponse {
        service,
        runtime,
        machine_logged_in,
        config_path: machine_config_path().to_string_lossy().to_string(),
        log_dir: machine_log_dir().to_string_lossy().to_string(),
    })
}

fn collect_service_status_with_runtime(
    runtime: Option<RuntimeStatus>,
) -> Result<ServiceStatusResponse, String> {
    let service = query_service_status().map_err(|e| e.to_string())?;

    if let Some(status) = &runtime {
        *get_last_known_status().lock() = Some(status.clone());
    }

    // machine_logged_in 从 service 运行时状态派生（不读受保护的 machine config）
    let machine_logged_in = runtime.as_ref().map(|r| r.logged_in).unwrap_or(false);

    Ok(ServiceStatusResponse {
        service,
        runtime,
        machine_logged_in,
        config_path: machine_config_path().to_string_lossy().to_string(),
        log_dir: machine_log_dir().to_string_lossy().to_string(),
    })
}

#[tauri::command]
pub async fn get_service_status() -> Result<ServiceStatusResponse, String> {
    debug!("[service] get_service_status called");
    collect_service_status().await
}

/// UI 调用：建立到 service 的持久连接，监听服务端推送事件。返回 true=已连接, false=service 未运行
#[tauri::command]
pub async fn listen_service_events(app: AppHandle) -> Result<bool, String> {
    debug!("[service] listen_service_events called");
    ensure_persistent_connection(&app).await
}

#[tauri::command]
pub async fn acknowledge_device_identity_notification() -> Result<(), String> {
    match send_command_responsive(Data::AcknowledgeDeviceIdentityNotification).await? {
        Data::CommandResponse { ok: true, .. } => Ok(()),
        Data::CommandResponse { message, .. } => Err(message),
        _ => Err("unexpected service response".to_string()),
    }
}

#[tauri::command]
pub async fn sync_service_runtime_config() -> Result<ServiceStatusResponse, String> {
    maybe_notify_service(Data::UpdateAuth).await;
    maybe_notify_service(Data::ReloadConfig).await;
    collect_service_status().await
}

#[tauri::command]
pub async fn ensure_background_service_session(
    _app: AppHandle,
) -> Result<ServiceStatusResponse, String> {
    match tokio::time::timeout(
        SERVICE_CHECK_TIMEOUT,
        ensure_background_service_session_inner(),
    )
    .await
    {
        Ok(result) => result,
        Err(_) => {
            warn!("[service] ensure_background_service_session timed out after 15s");
            Err(crate::commands::localized(
                "errors.service_check_timeout",
                &[],
            ))
        }
    }
}

async fn ensure_background_service_session_inner() -> Result<ServiceStatusResponse, String> {
    debug!("[service] === ensure_background_service_session begin ===");
    let _guard = get_service_session_lock().lock().await;
    debug!("[service] acquired session lock");

    // 1) 先通过 IPC pipe 检测 service 是否已在运行
    match connect_with_handshake().await {
        Ok(_) => {
            debug!("[service] service already running (IPC OK)");
            // 这里只确认 service/IPC 生命周期，不执行设备注册或网络请求。
            // 登录后的配置同步由 sync_service_runtime_config 负责；service 自身 bootstrap
            // 会在缺少 device_id 时注册设备。把 RegisterDevice 放在这里会让正常 IPC
            // 因外网延迟被误报为“后台服务检查超时”。
            let result = collect_service_status().await;
            debug!(
                "[service] === ensure_background_service_session end (already running), running={} ===",
                result.as_ref().map(|r| r.service.running).unwrap_or(false)
            );
            return result;
        }
        Err(e) => {
            debug!("[service] IPC not available yet: {}", e);
        }
    }

    // 2) 查询 SCM 状态
    let scm_status = query_service_status().map_err(|e| {
        warn!("[service] query_service_status failed: {}", e);
        e.to_string()
    })?;
    debug!(
        "[service] SCM status: installed={}, running={}, enabled={}, raw_state={}",
        scm_status.installed, scm_status.running, scm_status.enabled, scm_status.raw_state
    );

    if scm_status.installed {
        if !scm_status.running {
            debug!("[service] attempting SCM start...");
            match start_service() {
                Ok(()) => debug!("[service] SCM start returned Ok"),
                Err(err) => {
                    warn!("[service] SCM start failed: {:#}", err);
                }
            }
        } else {
            debug!("[service] SCM reports running, waiting for IPC...");
        }

        let mut ipc_ready = false;
        for attempt in 0..20 {
            tokio::time::sleep(std::time::Duration::from_millis(500)).await;
            match connect_with_handshake().await {
                Ok(_) => {
                    ipc_ready = true;
                    debug!("[service] IPC ready after {}ms", (attempt + 1) * 500);
                    break;
                }
                Err(e) => {
                    if attempt % 5 == 4 {
                        debug!(
                            "[service] IPC not ready after {}ms: {}",
                            (attempt + 1) * 500,
                            e
                        );
                    }
                }
            }
            if let Ok(status) = query_service_status() {
                if !status.running && !matches!(status.raw_state.as_str(), "StartPending") {
                    warn!(
                        "[service] SCM service not running and not StartPending, state={}",
                        status.raw_state
                    );
                    break;
                }
            }
        }

        if !ipc_ready {
            warn!("[service] IPC unavailable after 10s");
        }
    } else {
        // service 未安装 — 客户端安装包一定会装 service，走到这里说明安装异常
        // 不尝试 IPC 通知，直接返回当前状态
        debug!("[service] service not installed, skipping IPC notifications");
        let result = collect_service_status().await;
        debug!(
            "[service] === ensure_background_service_session end (not installed), running={} ===",
            result.as_ref().map(|r| r.service.running).unwrap_or(false)
        );
        return result;
    }

    let result = collect_service_status().await;
    debug!(
        "[service] === ensure_background_service_session end, running={} ===",
        result.as_ref().map(|r| r.service.running).unwrap_or(false)
    );
    result
}

pub fn cleanup_background_service_on_app_exit() {
    debug!("[service] === cleanup on app exit begin ===");

    let shutdown_sent =
        tauri::async_runtime::block_on(send_command(Data::ShutdownGracefully)).is_ok();
    debug!("[service] shutdown sent: {}", shutdown_sent);

    if shutdown_sent {
        for i in 0..20 {
            std::thread::sleep(std::time::Duration::from_millis(300));
            let pipe_gone = tauri::async_runtime::block_on(connect_with_handshake()).is_err();
            let service_stopped = query_service_status()
                .map(|status| !status.running)
                .unwrap_or(false);
            debug!(
                "[service] waiting service stop: attempt={}, pipe_gone={}, service_stopped={}",
                i + 1,
                pipe_gone,
                service_stopped
            );
            if pipe_gone && service_stopped {
                break;
            }
        }
    }

    match query_service_status() {
        Ok(status) if status.installed && status.running => {
            debug!("[service] service still running after graceful, calling stop_service()");
            if let Err(err) = stop_service() {
                warn!("[service] stop SCM service on app exit failed: {}", err);
            } else {
                for i in 0..20 {
                    std::thread::sleep(std::time::Duration::from_millis(300));
                    let service_stopped =
                        query_service_status().map(|s| !s.running).unwrap_or(false);
                    debug!(
                        "[service] waiting SCM stop: attempt={}, stopped={}",
                        i + 1,
                        service_stopped
                    );
                    if service_stopped {
                        break;
                    }
                }
            }
        }
        Ok(status) => debug!(
            "[service] service state on exit: installed={}, running={}",
            status.installed, status.running
        ),
        Err(err) => warn!("[service] query SCM service on app exit failed: {}", err),
    }

    debug!("[service] === cleanup on app exit done ===");
}

/// 持久连接发送命令（带响应），失败则回退到一次性连接
pub(crate) async fn send_command_responsive(data: Data) -> Result<Data, String> {
    match send_req_via_persistent(data.clone()).await {
        Ok(resp) => Ok(resp),
        Err(_) => send_command(data).await.map_err(|e| e.to_string()),
    }
}

#[tauri::command]
pub async fn stop_service_tunnel(source_device_id: i64) -> Result<ServiceStatusResponse, String> {
    match send_command_responsive(Data::StopTunnel { source_device_id }).await {
        Ok(Data::CommandResponse {
            ok: true, status, ..
        }) => {
            if status.is_some() {
                collect_service_status_with_runtime(status)
            } else {
                collect_service_status().await
            }
        }
        Ok(Data::CommandResponse { message, .. }) => Err(message),
        Ok(other) => Err(format!("unexpected service response: {:?}", other)),
        Err(err) => Err(err),
    }
}

#[tauri::command]
pub async fn stop_service_active_tunnel(
    target_device_id: i64,
) -> Result<ServiceStatusResponse, String> {
    match send_command_responsive(Data::StopActiveTunnel { target_device_id }).await {
        Ok(Data::CommandResponse {
            ok: true, status, ..
        }) => {
            if status.is_some() {
                collect_service_status_with_runtime(status)
            } else {
                collect_service_status().await
            }
        }
        Ok(Data::CommandResponse { message, .. }) => Err(message),
        Ok(other) => Err(format!("unexpected service response: {:?}", other)),
        Err(err) => Err(err),
    }
}

#[tauri::command]
pub async fn test_tunnel_speed(peer_device_id: i64) -> Result<serde_json::Value, String> {
    // 测速会持续数秒，使用独立 IPC 连接，避免占用 GUI 的持久连接并阻塞
    // 设备列表刷新、状态查询等交互。
    match send_command(Data::TestTunnelSpeed { peer_device_id }).await {
        Ok(Data::CommandResponse {
            ok: true,
            data: Some(data),
            ..
        }) => Ok(data),
        Ok(Data::CommandResponse { message, .. }) => Err(message),
        Ok(other) => Err(format!("unexpected service response: {:?}", other)),
        Err(err) => Err(err.to_string()),
    }
}

#[tauri::command]
pub async fn start_service_active_tunnel(
    app: AppHandle,
    target_device_id: i64,
    target_device_uuid: String,
    connect_code: Option<String>,
    temporary_password: Option<String>,
    #[allow(unused_variables)] lan_cidrs: Option<Vec<String>>,
) -> Result<String, String> {
    debug!(
        "[service] start active tunnel requested: target_device_id={}, target_uuid={}, connect_code={}, temp_password={}, lan_cidrs={}",
        target_device_id,
        target_device_uuid,
        connect_code.as_ref().map(|v| !v.is_empty()).unwrap_or(false),
        temporary_password.as_ref().map(|v| !v.is_empty()).unwrap_or(false),
        lan_cidrs.as_ref().map(|v| v.len()).unwrap_or(0)
    );
    let _ = app.emit(
        "active-tunnel-stage",
        serde_json::json!({
            "target_device_id": target_device_id,
            "stage": "service_session",
            "message": crate::commands::localized("service.start.confirming", &[]),
        }),
    );
    debug!(
        "[service] ensuring background service before active tunnel: target_device_id={}",
        target_device_id
    );
    let _ = ensure_background_service_session(app.clone()).await?;
    let _ = app.emit(
        "active-tunnel-stage",
        serde_json::json!({
            "target_device_id": target_device_id,
            "stage": "service_ready",
            "message": crate::commands::localized("service.start.service_ready", &[]),
        }),
    );
    debug!(
        "[service] background service ready, sending StartActiveTunnelJob IPC: target_device_id={}",
        target_device_id
    );
    let _ = app.emit(
        "active-tunnel-stage",
        serde_json::json!({
            "target_device_id": target_device_id,
            "stage": "service_ipc",
            "message": crate::commands::localized("service.start.sent", &[]),
        }),
    );
    match send_command_responsive(Data::StartActiveTunnelJob {
        target_device_id,
        target_device_uuid,
        connect_code,
        temporary_password,
        lan_cidrs: lan_cidrs.unwrap_or_default(),
    })
    .await
    {
        Ok(Data::CommandResponse {
            ok: true, message, ..
        }) => {
            debug!(
                "[service] StartActiveTunnelJob IPC succeeded: target_device_id={}",
                target_device_id
            );
            let _ = app.emit(
                "active-tunnel-stage",
                serde_json::json!({
                    "target_device_id": target_device_id,
                    "stage": "service_done",
                    "message": crate::commands::localized("service.start.job_started", &[]),
                }),
            );
            Ok(message)
        }
        Ok(Data::CommandResponse { message, .. }) => {
            warn!(
                "[service] StartActiveTunnelJob IPC returned failure: target_device_id={}, message={}",
                target_device_id, message
            );
            let _ = app.emit(
                "active-tunnel-stage",
                serde_json::json!({
                    "target_device_id": target_device_id,
                    "stage": "service_failed",
                    "message": crate::commands::localized(
                        "service.start.service_failed",
                        &[("reason", &message)],
                    ),
                }),
            );
            Err(message)
        }
        Ok(other) => {
            warn!(
                "[service] StartActiveTunnelJob IPC returned unexpected response: target_device_id={}, response={:?}",
                target_device_id, other
            );
            let _ = app.emit(
                "active-tunnel-stage",
                serde_json::json!({
                    "target_device_id": target_device_id,
                    "stage": "service_unexpected",
                    "message": crate::commands::localized("service.start.unknown_response", &[]),
                }),
            );
            Err(format!("unexpected service response: {:?}", other))
        }
        Err(err) => {
            warn!(
                "[service] StartActiveTunnelJob IPC failed: target_device_id={}, error={}",
                target_device_id, err
            );
            let _ = app.emit(
                "active-tunnel-stage",
                serde_json::json!({
                    "target_device_id": target_device_id,
                    "stage": "service_ipc_failed",
                    "message": crate::commands::localized(
                        "service.start.ipc_failed",
                        &[("reason", &err)],
                    ),
                }),
            );
            Err(err)
        }
    }
}

#[tauri::command]
pub async fn start_service_anonymous_active_tunnel(
    app: AppHandle,
    connect_code: String,
    temporary_password: String,
) -> Result<serde_json::Value, String> {
    debug!(
        "[service] anonymous active tunnel requested: connect_code={}",
        connect_code
    );
    let _status = ensure_background_service_session(app.clone()).await?;
    let _ = app.emit(
        "active-tunnel-stage",
        serde_json::json!({
            "stage": "service_ipc",
            "message": crate::commands::localized("service.start_invite.sent", &[]),
        }),
    );
    match send_command_responsive(Data::StartAnonymousActiveTunnelJob {
        connect_code,
        temporary_password,
    })
    .await
    {
        Ok(Data::CommandResponse {
            ok: true,
            message,
            data,
            ..
        }) => Ok(serde_json::json!({
            "success": true,
            "message": message,
            "device": data.and_then(|value| value.get("device").cloned()),
        })),
        Ok(Data::CommandResponse { message, .. }) => Err(message),
        Ok(other) => Err(format!("unexpected service response: {:?}", other)),
        Err(err) => Err(err),
    }
}

#[tauri::command]
pub async fn stop_active_tunnel_job(target_device_id: i64) -> Result<String, String> {
    match send_command_responsive(Data::StopActiveTunnelJob { target_device_id }).await {
        Ok(Data::CommandResponse {
            ok: true, message, ..
        }) => Ok(message),
        Ok(Data::CommandResponse { message, .. }) => Err(message),
        Ok(other) => Err(format!("unexpected service response: {:?}", other)),
        Err(err) => Err(err),
    }
}

#[tauri::command]
pub async fn approve_inbound_tunnel(attempt_id: String) -> Result<String, String> {
    match send_command_responsive(Data::ApproveInboundTunnel { attempt_id }).await {
        Ok(Data::CommandResponse {
            ok: true, message, ..
        }) => Ok(message),
        Ok(Data::CommandResponse { message, .. }) => Err(message),
        Ok(other) => Err(format!("unexpected service response: {:?}", other)),
        Err(err) => Err(err),
    }
}

#[tauri::command]
pub async fn reject_inbound_tunnel(attempt_id: String) -> Result<String, String> {
    match send_command_responsive(Data::RejectInboundTunnel { attempt_id }).await {
        Ok(Data::CommandResponse {
            ok: true, message, ..
        }) => Ok(message),
        Ok(Data::CommandResponse { message, .. }) => Err(message),
        Ok(other) => Err(format!("unexpected service response: {:?}", other)),
        Err(err) => Err(err),
    }
}

#[tauri::command]
pub async fn refresh_service_network_info() -> Result<ServiceStatusResponse, String> {
    let service = query_service_status().map_err(|e| e.to_string())?;
    if !service.running {
        return Err(crate::commands::localized(
            "errors.service_not_running_no_public_ip",
            &[],
        ));
    }

    match send_command_responsive(Data::RefreshNetworkInfo).await {
        Ok(Data::CommandResponse {
            ok: true, status, ..
        }) => {
            if status.is_some() {
                collect_service_status_with_runtime(status)
            } else {
                collect_service_status().await
            }
        }
        Ok(Data::CommandResponse { message, .. }) => Err(message),
        Ok(other) => Err(format!("unexpected service response: {:?}", other)),
        Err(err) => Err(err),
    }
}

#[tauri::command]
pub async fn refresh_tunnel_status() -> Result<ServiceStatusResponse, String> {
    match send_command_responsive(Data::RefreshTunnelStatus).await {
        Ok(Data::CommandResponse {
            ok: true, status, ..
        }) => collect_service_status_with_runtime(status),
        Ok(Data::CommandResponse { message, .. }) => Err(message),
        Ok(other) => Err(format!("unexpected service response: {:?}", other)),
        Err(err) => Err(err),
    }
}
