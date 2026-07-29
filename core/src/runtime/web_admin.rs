// ---- 浏览器管理服务（默认 http://127.0.0.1:48083）----

use super::{handle_data, SharedRuntimeState};
use crate::config::load_machine_config;
use crate::control::{Data, RuntimeStatus};
use crate::http::ApiResponse;
use crate::service_control::{disable_service, enable_service, query_service_status};
use anyhow::{Context, Result};
use axum::{
    body::Body,
    extract::ConnectInfo,
    extract::{
        ws::{Message, WebSocket, WebSocketUpgrade},
        DefaultBodyLimit, Path, State,
    },
    http::{header, HeaderMap, Request, StatusCode},
    middleware::{self, Next},
    response::{IntoResponse, Response},
    routing::{get, post},
    Json, Router,
};
use futures_util::{SinkExt, StreamExt};
use parking_lot::Mutex;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::collections::HashMap;
use std::net::{IpAddr, SocketAddr};
use std::sync::Arc;
use std::time::{Duration, Instant};
use subtle::ConstantTimeEq;
use tokio::sync::{watch, Notify};
use tower_http::services::{ServeDir, ServeFile};
use tracing::{error, info};

const DEFAULT_WEB_ADMIN_ADDR: &str = "127.0.0.1:48083";
const DEFAULT_WEB_ADMIN_PORT: u16 = 48083;
const SERVICE_SESSION_PLACEHOLDER: &str = "__service_session__";
const WEB_SESSION_COOKIE: &str = "p2premote_web_session";
const WEB_SESSION_IDLE: Duration = Duration::from_secs(30 * 60);
const WEB_SESSION_MAX_AGE: Duration = Duration::from_secs(8 * 60 * 60);
const WEB_UNLOCK_FAILURE_LIMIT: u32 = 5;
const WEB_UNLOCK_LOCKOUT: Duration = Duration::from_secs(60);

#[derive(Clone)]
struct WebAdminState {
    shared: Arc<Mutex<SharedRuntimeState>>,
    wake: Arc<Notify>,
    security: Arc<WebSecurityState>,
}

struct WebSecurityState {
    allowed_remote_ip: Option<IpAddr>,
    security_code_hash: Option<[u8; 32]>,
    session: Mutex<Option<WebSession>>,
    session_revision: watch::Sender<u64>,
    failures: Mutex<HashMap<IpAddr, UnlockFailure>>,
}

struct WebSession {
    token: String,
    source_ip: IpAddr,
    created_at: Instant,
    last_active_at: Instant,
}

struct UnlockFailure {
    failures: u32,
    locked_until: Option<Instant>,
}

#[derive(Deserialize)]
struct WebUnlockRequest {
    security_code: String,
}

#[derive(Serialize)]
struct WebAuthStatus {
    authenticated: bool,
    security_code_required: bool,
}

#[derive(Debug, Serialize)]
struct WebInvokeResponse {
    ok: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    value: Option<serde_json::Value>,
    #[serde(skip_serializing_if = "Option::is_none")]
    error: Option<String>,
}

#[derive(Debug, Serialize)]
struct WebServiceStatus {
    service: WebServiceInfo,
    runtime: Option<RuntimeStatus>,
    machine_logged_in: bool,
    config_path: String,
    log_dir: String,
}

#[derive(Debug, Serialize)]
struct WebServiceInfo {
    installed: bool,
    running: bool,
    enabled: bool,
    pid: Option<u32>,
    raw_state: String,
}

#[derive(Debug, Deserialize)]
struct VersionPolicyResponse {
    data: Option<VersionPolicyData>,
}

#[derive(Debug, Deserialize)]
struct VersionPolicyData {
    latest_version: String,
    min_supported_version: String,
    download_url: String,
    #[serde(default)]
    release_notes: String,
}

impl WebSecurityState {
    fn from_config() -> Result<Self> {
        let config = load_machine_config().context("failed to load machine config")?;
        Self::from_values(
            config.web_admin_allowed_ip.as_deref(),
            config.web_admin_security_code.as_deref(),
        )
    }

    fn from_values(allowed_remote_ip: Option<&str>, security_code: Option<&str>) -> Result<Self> {
        let allowed_remote_ip = allowed_remote_ip
            .map(str::trim)
            .map(ToOwned::to_owned)
            .filter(|value| !value.is_empty())
            .map(|value| {
                let ip: IpAddr = value
                    .parse()
                    .with_context(|| format!("invalid web_admin_allowed_ip: {value}"))?;
                if ip.is_unspecified() {
                    anyhow::bail!("web_admin_allowed_ip cannot be an unspecified address");
                }
                Ok(ip)
            })
            .transpose()?;
        let security_code = security_code
            .map(str::trim)
            .map(ToOwned::to_owned)
            .filter(|value| !value.is_empty());
        if security_code
            .as_ref()
            .is_some_and(|value| value.len() > 256)
        {
            anyhow::bail!("web_admin_security_code exceeds 256 bytes");
        }
        if allowed_remote_ip.is_some() && security_code.is_none() {
            anyhow::bail!(
                "web_admin_security_code is required when remote Web UI access is enabled"
            );
        }
        let (session_revision, _) = watch::channel(0);
        Ok(Self {
            allowed_remote_ip,
            security_code_hash: security_code.as_deref().map(hash_security_code),
            session: Mutex::new(None),
            session_revision,
            failures: Mutex::new(HashMap::new()),
        })
    }

    fn localhost_only() -> Self {
        let (session_revision, _) = watch::channel(0);
        Self {
            allowed_remote_ip: None,
            security_code_hash: None,
            session: Mutex::new(None),
            session_revision,
            failures: Mutex::new(HashMap::new()),
        }
    }

    fn source_allowed(&self, ip: IpAddr) -> bool {
        let ip = normalize_ip(ip);
        ip.is_loopback() || self.allowed_remote_ip == Some(ip)
    }

    fn session_valid(&self, headers: &HeaderMap, source_ip: IpAddr, refresh: bool) -> bool {
        if self.security_code_hash.is_none() {
            return source_ip.is_loopback();
        }
        let Some(token) = session_cookie(headers) else {
            return false;
        };
        self.session_token_valid(token, source_ip, refresh)
    }

    fn session_token_valid(&self, token: &str, source_ip: IpAddr, refresh: bool) -> bool {
        let now = Instant::now();
        let mut slot = self.session.lock();
        let valid_time = slot.as_ref().is_some_and(|session| {
            now.duration_since(session.last_active_at) <= WEB_SESSION_IDLE
                && now.duration_since(session.created_at) <= WEB_SESSION_MAX_AGE
        });
        if !valid_time {
            slot.take();
        }
        let Some(session) = slot.as_mut() else {
            return false;
        };
        if session.token != token || session.source_ip != normalize_ip(source_ip) {
            return false;
        }
        if refresh {
            session.last_active_at = now;
        }
        true
    }

    fn replace_session(&self, session: WebSession) {
        *self.session.lock() = Some(session);
        self.session_revision
            .send_modify(|revision| *revision = revision.wrapping_add(1));
    }

    fn remove_session(&self, token: &str) {
        let mut slot = self.session.lock();
        if slot.as_ref().is_some_and(|session| session.token == token) {
            slot.take();
            drop(slot);
            self.session_revision
                .send_modify(|revision| *revision = revision.wrapping_add(1));
        }
    }
}

fn hash_security_code(code: &str) -> [u8; 32] {
    Sha256::digest(code.as_bytes()).into()
}

fn normalize_ip(ip: IpAddr) -> IpAddr {
    match ip {
        IpAddr::V6(ip) => ip
            .to_ipv4_mapped()
            .map(IpAddr::V4)
            .unwrap_or(IpAddr::V6(ip)),
        ip => ip,
    }
}

fn session_cookie(headers: &HeaderMap) -> Option<&str> {
    headers
        .get(header::COOKIE)?
        .to_str()
        .ok()?
        .split(';')
        .map(str::trim)
        .find_map(|part| part.strip_prefix(&format!("{}=", WEB_SESSION_COOKIE)))
}

pub(super) fn spawn_web_admin_server(shared: Arc<Mutex<SharedRuntimeState>>, wake: Arc<Notify>) {
    tokio::spawn(async move {
        if !load_machine_config().unwrap_or_default().webui_enabled {
            info!("[WebAdmin] listener disabled by webui_enabled=false");
            return;
        }
        let security = match WebSecurityState::from_config() {
            Ok(security) => Arc::new(security),
            Err(err) => {
                error!(
                    "[WebAdmin] invalid security configuration: {}; remote access disabled",
                    err
                );
                Arc::new(WebSecurityState::localhost_only())
            }
        };
        if let Err(err) = web_admin_server_loop(WebAdminState {
            shared,
            wake,
            security,
        })
        .await
        {
            error!("[WebAdmin] {}", err);
        }
    });
}

async fn web_admin_server_loop(state: WebAdminState) -> Result<()> {
    let web_dir = resolve_web_ui_dir();
    let index_file = web_dir.join("index.html");
    let app = Router::new()
        .route("/api/health", get(web_health))
        .route("/api/web-auth/status", get(web_auth_status))
        .route(
            "/api/web-auth/unlock",
            post(web_auth_unlock).layer(DefaultBodyLimit::max(1024)),
        )
        .route("/api/web-auth/logout", post(web_auth_logout))
        .route("/api/events", get(web_events))
        .route("/api/invoke/{command}", post(web_invoke))
        .fallback_service(ServeDir::new(&web_dir).fallback(ServeFile::new(index_file)))
        .with_state(state.clone())
        .layer(middleware::from_fn_with_state(
            state.clone(),
            web_source_ip_guard,
        ));

    let addr = web_admin_addr(state.security.allowed_remote_ip.is_some())?;
    info!(
        "[WebAdmin] HTTP listening on http://{} , ui_dir={}, remote_client={}, authentication={}",
        addr,
        web_dir.display(),
        state
            .security
            .allowed_remote_ip
            .map(|ip| ip.to_string())
            .unwrap_or_else(|| "disabled".to_string()),
        if state.security.security_code_hash.is_some() {
            "enabled"
        } else {
            "disabled"
        },
    );
    let listener = tokio::net::TcpListener::bind(addr)
        .await
        .context("failed to bind web admin listener")?;
    axum::serve(
        listener,
        app.into_make_service_with_connect_info::<SocketAddr>(),
    )
    .await
    .context("web admin server failed")
}

fn web_admin_addr(remote_enabled: bool) -> Result<SocketAddr> {
    let configured = std::env::var("P2PREMOTE_WEB_ADMIN_ADDR")
        .ok()
        .map(|value| value.trim().to_string())
        .filter(|value| !value.is_empty())
        .unwrap_or_else(|| {
            if remote_enabled {
                format!("0.0.0.0:{}", DEFAULT_WEB_ADMIN_PORT)
            } else {
                DEFAULT_WEB_ADMIN_ADDR.to_string()
            }
        });
    normalize_web_admin_addr(&configured)
        .parse()
        .with_context(|| format!("invalid web admin listen addr: {}", configured))
}

pub(super) fn web_admin_addr_for_log() -> String {
    if !load_machine_config().unwrap_or_default().webui_enabled {
        return "disabled".to_string();
    }
    let remote_enabled = load_machine_config()
        .ok()
        .and_then(|config| config.web_admin_allowed_ip)
        .is_some_and(|value| !value.trim().is_empty());
    web_admin_addr(remote_enabled)
        .map(|addr| format!("http://{}", addr))
        .unwrap_or_else(|_| format!("http://{}", DEFAULT_WEB_ADMIN_ADDR))
}

fn normalize_web_admin_addr(addr: &str) -> String {
    if addr.contains(':') {
        addr.to_string()
    } else {
        format!("{}:{}", addr, DEFAULT_WEB_ADMIN_PORT)
    }
}

fn resolve_web_ui_dir() -> std::path::PathBuf {
    if let Ok(dir) = std::env::var("P2PREMOTE_WEB_UI_DIR") {
        let path = std::path::PathBuf::from(dir);
        if path.join("index.html").exists() {
            return path;
        }
    }

    let mut candidates = Vec::new();
    if let Ok(exe) = std::env::current_exe() {
        if let Some(dir) = exe.parent() {
            candidates.push(dir.join("web"));
            candidates.push(dir.join("resources").join("web"));
            candidates.push(dir.join("../web"));
        }
    }
    #[cfg(target_os = "linux")]
    {
        candidates.push(crate::config::linux_resources_dir().join("web"));
        candidates.push(crate::config::linux_install_root_dir().join("web"));
    }
    #[cfg(windows)]
    {
        candidates.push(
            crate::config::install_root_dir()
                .join("resources")
                .join("web"),
        );
        candidates.push(crate::config::install_root_dir().join("web"));
    }

    candidates
        .into_iter()
        .find(|path| path.join("index.html").exists())
        .unwrap_or_else(|| crate::config::install_data_dir().join("web"))
}

async fn web_source_ip_guard(
    State(state): State<WebAdminState>,
    ConnectInfo(peer): ConnectInfo<SocketAddr>,
    request: Request<Body>,
    next: Next,
) -> Response {
    if !state.security.source_allowed(normalize_ip(peer.ip())) {
        return StatusCode::FORBIDDEN.into_response();
    }
    next.run(request).await
}

async fn web_health() -> Json<serde_json::Value> {
    Json(serde_json::json!({ "ok": true }))
}

async fn web_auth_status(
    ConnectInfo(peer): ConnectInfo<SocketAddr>,
    headers: HeaderMap,
    State(state): State<WebAdminState>,
) -> Json<WebAuthStatus> {
    let required = state.security.security_code_hash.is_some();
    Json(WebAuthStatus {
        authenticated: !required || state.security.session_valid(&headers, peer.ip(), false),
        security_code_required: required,
    })
}

async fn web_auth_unlock(
    ConnectInfo(peer): ConnectInfo<SocketAddr>,
    headers: HeaderMap,
    State(state): State<WebAdminState>,
    Json(request): Json<WebUnlockRequest>,
) -> Response {
    if !web_origin_allowed(&headers) {
        return StatusCode::FORBIDDEN.into_response();
    }
    let source_ip = normalize_ip(peer.ip());
    let Some(expected) = state.security.security_code_hash else {
        return Json(serde_json::json!({ "ok": true })).into_response();
    };
    let now = Instant::now();
    {
        let failures = state.security.failures.lock();
        if failures
            .get(&source_ip)
            .and_then(|failure| failure.locked_until)
            .is_some_and(|until| until > now)
        {
            return (
                StatusCode::TOO_MANY_REQUESTS,
                Json(serde_json::json!({ "ok": false, "error": "temporarily_locked" })),
            )
                .into_response();
        }
    }
    let provided = hash_security_code(request.security_code.trim());
    if expected.ct_eq(&provided).unwrap_u8() != 1 {
        let mut failures = state.security.failures.lock();
        let failure = failures.entry(source_ip).or_insert(UnlockFailure {
            failures: 0,
            locked_until: None,
        });
        failure.failures += 1;
        if failure.failures >= WEB_UNLOCK_FAILURE_LIMIT {
            failure.failures = 0;
            failure.locked_until = Some(now + WEB_UNLOCK_LOCKOUT);
        }
        info!("[WebAdmin] unlock failed, source_ip={}", source_ip);
        return (
            StatusCode::UNAUTHORIZED,
            Json(serde_json::json!({ "ok": false, "error": "invalid_security_code" })),
        )
            .into_response();
    }
    state.security.failures.lock().remove(&source_ip);
    let token = format!(
        "{}{}",
        uuid::Uuid::new_v4().simple(),
        uuid::Uuid::new_v4().simple()
    );
    state.security.replace_session(WebSession {
        token: token.clone(),
        source_ip,
        created_at: now,
        last_active_at: now,
    });
    let cookie = format!(
        "{}={}; Path=/; HttpOnly; Secure; SameSite=Strict; Max-Age={}",
        WEB_SESSION_COOKIE,
        token,
        WEB_SESSION_MAX_AGE.as_secs()
    );
    let mut response = Json(serde_json::json!({ "ok": true })).into_response();
    if let Ok(value) = cookie.parse() {
        response.headers_mut().insert(header::SET_COOKIE, value);
    }
    response
}

async fn web_auth_logout(
    ConnectInfo(peer): ConnectInfo<SocketAddr>,
    headers: HeaderMap,
    State(state): State<WebAdminState>,
) -> Response {
    if !web_origin_allowed(&headers) {
        return StatusCode::FORBIDDEN.into_response();
    }
    if let Some(token) = session_cookie(&headers) {
        state.security.remove_session(token);
    }
    let mut response = Json(serde_json::json!({ "ok": true })).into_response();
    let cookie = format!(
        "{}=; Path=/; HttpOnly; Secure; SameSite=Strict; Max-Age=0",
        WEB_SESSION_COOKIE
    );
    if let Ok(value) = cookie.parse() {
        response.headers_mut().insert(header::SET_COOKIE, value);
    }
    info!(
        "[WebAdmin] session logged out, source_ip={}",
        normalize_ip(peer.ip())
    );
    response
}

async fn web_events(
    ws: WebSocketUpgrade,
    ConnectInfo(peer): ConnectInfo<SocketAddr>,
    headers: HeaderMap,
    State(state): State<WebAdminState>,
) -> impl IntoResponse {
    if !web_origin_allowed(&headers) {
        return StatusCode::FORBIDDEN.into_response();
    }
    if !state.security.session_valid(&headers, peer.ip(), true) {
        return StatusCode::UNAUTHORIZED.into_response();
    }
    let session_token = session_cookie(&headers).map(ToOwned::to_owned);
    ws.on_upgrade(move |socket| web_events_socket(socket, state, peer.ip(), session_token))
        .into_response()
}

async fn web_events_socket(
    socket: WebSocket,
    state: WebAdminState,
    source_ip: IpAddr,
    session_token: Option<String>,
) {
    let Some(tx) = state.shared.lock().status_tx().cloned() else {
        return;
    };
    let mut rx = tx.subscribe();
    let mut session_revision = state.security.session_revision.subscribe();
    let (mut sender, mut receiver) = socket.split();

    let initial = state.shared.lock().status.clone();
    let mut auth_check = tokio::time::interval(Duration::from_secs(30));
    let _ = sender
        .send(Message::Text(
            serde_json::json!({
                "event": "service-status-changed",
                "payload": initial,
            })
            .to_string()
            .into(),
        ))
        .await;

    loop {
        tokio::select! {
            changed = session_revision.changed(), if state.security.security_code_hash.is_some() => {
                if changed.is_err() || !session_token.as_deref().is_some_and(|token| {
                    state.security.session_token_valid(token, source_ip, false)
                }) {
                    let _ = sender.send(Message::Close(Some(axum::extract::ws::CloseFrame {
                        code: 4001,
                        reason: "web session replaced or logged out".into(),
                    }))).await;
                    break;
                }
            }
            _ = auth_check.tick() => {
                if state.security.security_code_hash.is_some()
                    && !session_token.as_deref().is_some_and(|token| {
                        state.security.session_token_valid(token, source_ip, false)
                    })
                {
                    let _ = sender.send(Message::Close(Some(axum::extract::ws::CloseFrame {
                        code: 4001,
                        reason: "web authentication expired".into(),
                    }))).await;
                    break;
                }
            }
            status = rx.recv() => {
                match status {
                    Ok(status) => {
                        let payload = serde_json::json!({
                            "event": "service-status-changed",
                            "payload": status,
                        });
                        if sender.send(Message::Text(payload.to_string().into())).await.is_err() {
                            break;
                        }
                    }
                    Err(tokio::sync::broadcast::error::RecvError::Lagged(_)) => {}
                    Err(_) => break,
                }
            }
            msg = receiver.next() => {
                match msg {
                    Some(Ok(Message::Close(_))) | None => break,
                    Some(Ok(_)) => {}
                    Some(Err(_)) => break,
                }
            }
        }
    }
}

async fn web_invoke(
    Path(command): Path<String>,
    ConnectInfo(peer): ConnectInfo<SocketAddr>,
    headers: HeaderMap,
    State(state): State<WebAdminState>,
    Json(args): Json<serde_json::Value>,
) -> impl IntoResponse {
    if !web_origin_allowed(&headers) {
        return (
            StatusCode::FORBIDDEN,
            Json(WebInvokeResponse {
                ok: false,
                value: None,
                error: Some("forbidden_origin".to_string()),
            }),
        );
    }
    if !state.security.session_valid(&headers, peer.ip(), true) {
        return (
            StatusCode::UNAUTHORIZED,
            Json(WebInvokeResponse {
                ok: false,
                value: None,
                error: Some("web_auth_required".to_string()),
            }),
        );
    }
    match handle_web_command(&command, args, &state).await {
        Ok(value) => (
            StatusCode::OK,
            Json(WebInvokeResponse {
                ok: true,
                value: Some(value),
                error: None,
            }),
        ),
        Err(error) => (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(WebInvokeResponse {
                ok: false,
                value: None,
                error: Some(error),
            }),
        ),
    }
}

async fn handle_web_command(
    command: &str,
    args: serde_json::Value,
    state: &WebAdminState,
) -> Result<serde_json::Value, String> {
    match command {
        "check_required_client_files" => Ok(serde_json::json!([])),
        "ensure_background_service_session" | "get_service_status" => Ok(web_status_value(state)),
        "listen_service_events" => Ok(serde_json::json!(true)),
        "sync_service_runtime_config" => {
            let _ = dispatch_web_data(Data::UpdateAuth, state).await?;
            let _ = dispatch_web_data(Data::ReloadConfig, state).await?;
            Ok(web_status_value(state))
        }
        "refresh_service_network_info" => {
            let _ = dispatch_web_data(Data::RefreshNetworkInfo, state).await?;
            Ok(web_status_value(state))
        }
        "refresh_tunnel_status" => {
            let _ = dispatch_web_data(Data::RefreshTunnelStatus, state).await?;
            Ok(web_status_value(state))
        }
        "get_settings" => web_settings_value(state).await,
        "set_locale" => {
            let locale = arg_string(&args, &["locale"])?;
            let _ = dispatch_web_data(Data::SetLocale { locale }, state).await?;
            Ok(serde_json::Value::Null)
        }
        "set_auto_start" => {
            let enabled = arg_bool(&args, &["enabled"]).unwrap_or(false);
            if enabled {
                enable_service().map_err(|e| e.to_string())?;
            } else {
                disable_service().map_err(|e| e.to_string())?;
            }
            let _ = dispatch_web_data(Data::SetAutoStartConfig { enabled }, state).await?;
            Ok(serde_json::Value::Null)
        }
        "enable_background_service_autostart" => {
            enable_service().map_err(|e| e.to_string())?;
            Ok(web_status_value(state))
        }
        "disable_background_service_autostart" => {
            disable_service().map_err(|e| e.to_string())?;
            Ok(web_status_value(state))
        }
        "install_background_service"
        | "uninstall_background_service"
        | "start_background_service"
        | "stop_background_service"
        | "restart_background_service" => Err(
            "浏览器管理界面不支持安装、启动、停止或重启系统服务，请通过安装包或系统服务管理器操作"
                .to_string(),
        ),
        "check_update" => check_update_for_web().await,
        "register_no_verify" => register_no_verify_for_web(args).await,
        "login" => {
            let identifier = arg_string(&args, &["identifier"])?;
            let password = arg_string(&args, &["password"])?;
            let resp = dispatch_web_data(
                Data::Login {
                    identifier,
                    password,
                },
                state,
            )
            .await?;
            let user = command_data(resp)?;
            Ok(serde_json::json!({
                "code": 0,
                "msg": "ok",
                "data": { "user": user },
            }))
        }
        "logout" => {
            let _ = dispatch_web_data(Data::Logout, state).await?;
            Ok(serde_json::Value::Null)
        }
        "mark_current_device_offline" => {
            let _ = dispatch_web_data(Data::MarkCurrentDeviceOffline, state).await?;
            Ok(serde_json::Value::Null)
        }
        "try_auto_login" => {
            let resp = dispatch_web_data(Data::TryAutoLogin, state).await?;
            let data = command_data(resp)?;
            let logged_in = data
                .get("logged_in")
                .and_then(|value| value.as_bool())
                .unwrap_or(false);
            Ok(serde_json::json!(if logged_in {
                SERVICE_SESSION_PLACEHOLDER
            } else {
                ""
            }))
        }
        "is_logged_in" => Ok(serde_json::json!(state.shared.lock().status.logged_in)),
        "get_user_info" | "fetch_user_profile" => {
            match dispatch_web_data(Data::GetUserProfile, state).await {
                Ok(resp) => command_data(resp),
                Err(_) if command == "get_user_info" => Ok(serde_json::Value::Null),
                Err(err) => Err(err),
            }
        }
        "get_invite_info" => {
            let resp = dispatch_web_data(Data::GetInviteInfo, state).await?;
            command_data(resp)
        }
        "get_saved_login" => {
            let resp = dispatch_web_data(Data::GetSavedLogin, state).await?;
            command_data(resp)
        }
        "save_login_settings" => {
            let identifier = arg_string(&args, &["identifier"])?;
            let remember_me = arg_bool(&args, &["rememberMe", "remember_me"]).unwrap_or(false);
            let auto_login = arg_bool(&args, &["autoLogin", "auto_login"]).unwrap_or(false);
            let _ = dispatch_web_data(
                Data::SaveLoginSettings {
                    identifier,
                    remember_me,
                    auto_login,
                },
                state,
            )
            .await?;
            Ok(serde_json::Value::Null)
        }
        "get_wgvpn_lan_access_config" => {
            let resp = dispatch_web_data(Data::GetWgvpnLanAccessConfig, state).await?;
            command_data(resp)
        }
        "save_wgvpn_lan_access_config" => {
            let enabled = arg_bool(&args, &["enabled"]).unwrap_or(false);
            let cidrs = arg_string_array(&args, &["cidrs"]).unwrap_or_default();
            let resp =
                dispatch_web_data(Data::SaveWgvpnLanAccessConfig { enabled, cidrs }, state).await?;
            command_data(resp)
        }
        "get_device_list" => {
            let resp = dispatch_web_data(Data::GetDeviceList, state).await?;
            command_data(resp)
        }
        "register_current_device_auto" => {
            let resp = dispatch_web_data(Data::RegisterDevice, state).await?;
            command_data(resp)
        }
        "update_device_alias" => {
            let device_id = arg_i64(&args, &["deviceId", "device_id"])?;
            let alias = arg_string(&args, &["alias"])?;
            let _ = dispatch_web_data(Data::UpdateDeviceAlias { device_id, alias }, state).await?;
            Ok(serde_json::Value::Null)
        }
        "delete_device" => {
            let device_id = arg_i64(&args, &["deviceId", "device_id"])?;
            let _ = dispatch_web_data(Data::DeleteDevice { device_id }, state).await?;
            Ok(serde_json::Value::Null)
        }
        "set_device_password" => {
            let device_id = arg_i64(&args, &["deviceId", "device_id"])?;
            let password = arg_string(&args, &["password"])?;
            let _ = dispatch_web_data(
                Data::SetDevicePassword {
                    device_id,
                    password,
                },
                state,
            )
            .await?;
            Ok(serde_json::Value::Null)
        }
        "generate_connect_code" => {
            let device_id = arg_i64(&args, &["deviceId", "device_id"])?;
            let resp = dispatch_web_data(Data::GenerateConnectCode { device_id }, state).await?;
            let data = command_data(resp)?;
            Ok(data
                .get("connect_code")
                .cloned()
                .unwrap_or(serde_json::Value::Null))
        }
        "start_service_active_tunnel" => {
            let target_device_id = arg_i64(&args, &["targetDeviceId", "target_device_id"])?;
            let target_device_uuid =
                arg_string(&args, &["targetDeviceUuid", "target_device_uuid"])?;
            let connect_code = arg_optional_string(&args, &["connectCode", "connect_code"]);
            let temporary_password =
                arg_optional_string(&args, &["temporaryPassword", "temporary_password"]);
            let lan_cidrs = arg_string_array(&args, &["lanCidrs", "lan_cidrs"]).unwrap_or_default();
            let resp = dispatch_web_data(
                Data::StartActiveTunnelJob {
                    target_device_id,
                    target_device_uuid,
                    connect_code,
                    temporary_password,
                    lan_cidrs,
                },
                state,
            )
            .await?;
            command_message(resp)
        }
        "start_service_anonymous_active_tunnel" => {
            let connect_code = arg_string(&args, &["connectCode", "connect_code"])?;
            let temporary_password =
                arg_string(&args, &["temporaryPassword", "temporary_password"])?;
            let resp = dispatch_web_data(
                Data::StartAnonymousActiveTunnelJob {
                    connect_code,
                    temporary_password,
                },
                state,
            )
            .await?;
            let Data::CommandResponse { message, data, .. } = resp else {
                return Err("unexpected service response".to_string());
            };
            Ok(serde_json::json!({
                "success": true,
                "message": message,
                "device": data.and_then(|value| value.get("device").cloned()),
            }))
        }
        "stop_active_tunnel_job" => {
            let target_device_id = arg_i64(&args, &["targetDeviceId", "target_device_id"])?;
            let resp =
                dispatch_web_data(Data::StopActiveTunnelJob { target_device_id }, state).await?;
            command_message(resp)
        }
        "stop_service_active_tunnel" => {
            let target_device_id = arg_i64(&args, &["targetDeviceId", "target_device_id"])?;
            let _ = dispatch_web_data(Data::StopActiveTunnel { target_device_id }, state).await?;
            Ok(web_status_value(state))
        }
        "test_tunnel_speed" => {
            let peer_device_id = arg_i64(&args, &["peerDeviceId", "peer_device_id"])?;
            let response =
                dispatch_web_data(Data::TestTunnelSpeed { peer_device_id }, state).await?;
            command_data(response)
        }
        "stop_service_tunnel" => {
            let source_device_id = arg_i64(&args, &["sourceDeviceId", "source_device_id"])?;
            let _ = dispatch_web_data(Data::StopTunnel { source_device_id }, state).await?;
            Ok(web_status_value(state))
        }
        "acknowledge_device_identity_notification" => {
            let _ = dispatch_web_data(Data::AcknowledgeDeviceIdentityNotification, state).await?;
            Ok(serde_json::Value::Null)
        }
        "show_system_notification" | "flash_main_window" | "exit_application" => {
            Ok(serde_json::Value::Null)
        }
        other => Err(format!("unsupported browser command: {}", other)),
    }
}

async fn dispatch_web_data(data: Data, state: &WebAdminState) -> Result<Data, String> {
    let response = handle_data(data, &state.shared)
        .await
        .ok_or_else(|| "service command produced no response".to_string())?;
    {
        let runtime = state.shared.lock();
        if runtime.reconnect_requested || runtime.shutdown_requested {
            state.wake.notify_one();
        }
    }
    match &response {
        Data::CommandResponse {
            ok: false, message, ..
        } => Err(message.clone()),
        _ => Ok(response),
    }
}

fn command_data(resp: Data) -> Result<serde_json::Value, String> {
    match resp {
        Data::CommandResponse {
            data: Some(data), ..
        } => Ok(data),
        Data::CommandResponse { data: None, .. } => Ok(serde_json::Value::Null),
        other => Err(format!("unexpected service response: {:?}", other)),
    }
}

fn command_message(resp: Data) -> Result<serde_json::Value, String> {
    match resp {
        Data::CommandResponse { message, .. } => Ok(serde_json::json!(message)),
        other => Err(format!("unexpected service response: {:?}", other)),
    }
}

fn web_status_value(state: &WebAdminState) -> serde_json::Value {
    let runtime = state.shared.lock().status.clone();
    let service = web_service_info();
    let status = WebServiceStatus {
        service,
        machine_logged_in: runtime.logged_in,
        runtime: Some(runtime),
        config_path: crate::config::machine_config_path()
            .to_string_lossy()
            .to_string(),
        log_dir: crate::config::machine_log_dir()
            .to_string_lossy()
            .to_string(),
    };
    serde_json::to_value(status).unwrap_or(serde_json::Value::Null)
}

async fn web_settings_value(state: &WebAdminState) -> Result<serde_json::Value, String> {
    let data = command_data(dispatch_web_data(Data::GetLoginPreferences, state).await?)?;
    Ok(serde_json::json!({
        "auto_start": data.get("auto_start").and_then(|value| value.as_bool()).unwrap_or(false),
        "remember_me": data.get("remember_me").and_then(|value| value.as_bool()).unwrap_or(false),
        "auto_login": data.get("auto_login").and_then(|value| value.as_bool()).unwrap_or(false),
        "version": client_version(),
    }))
}

fn web_service_info() -> WebServiceInfo {
    match query_service_status() {
        Ok(status) => WebServiceInfo {
            installed: status.installed,
            running: status.running,
            enabled: status.enabled,
            raw_state: status.raw_state,
            pid: Some(std::process::id()),
        },
        Err(err) => WebServiceInfo {
            installed: true,
            running: true,
            enabled: false,
            raw_state: format!("query failed: {}", err),
            pid: Some(std::process::id()),
        },
    }
}

async fn register_no_verify_for_web(args: serde_json::Value) -> Result<serde_json::Value, String> {
    let username = arg_string(&args, &["username"])?;
    let email = arg_string(&args, &["email"])?;
    let password = arg_string(&args, &["password"])?;
    let invite_code = arg_optional_string(&args, &["inviteCode", "invite_code"]);
    let config = load_machine_config().unwrap_or_default();
    let url = format!(
        "{}{}",
        config.server_url.trim_end_matches('/'),
        "/api/v1/auth/register/email"
    );
    let response = crate::http::shared_client()
        .post(&url)
        .json(&serde_json::json!({
            "username": username,
            "email": email,
            "password": password,
            "invite_code": invite_code,
        }))
        .send()
        .await
        .map_err(|e| e.to_string())?;
    let response = response
        .json::<ApiResponse<Option<serde_json::Value>>>()
        .await
        .map_err(|e| e.to_string())?;
    let message = if response.code == 0 {
        response.msg.clone()
    } else {
        response.localized_error_message(config.locale.as_deref())
    };
    Ok(serde_json::json!({
        "code": response.code,
        "msg": message,
        "data": response.data,
    }))
}

async fn check_update_for_web() -> Result<serde_json::Value, String> {
    let config = load_machine_config().unwrap_or_default();
    let url = format!(
        "{}{}",
        config.server_url.trim_end_matches('/'),
        "/api/v1/client/version-policy"
    );
    let current = client_version();
    let resp = crate::http::shared_client()
        .get(&url)
        .send()
        .await
        .map_err(|e| e.to_string())?;
    if !resp.status().is_success() {
        return Ok(update_response(
            "none",
            false,
            false,
            &current,
            "",
            "",
            "",
            "",
            Some(&format!("服务器返回状态码: {}", resp.status())),
        ));
    }
    let policy = resp
        .json::<VersionPolicyResponse>()
        .await
        .map_err(|e| e.to_string())?;
    let Some(data) = policy.data else {
        return Ok(update_response(
            "none",
            false,
            false,
            &current,
            "",
            "",
            "",
            "",
            Some("版本策略数据为空"),
        ));
    };
    let force_update = is_version_less(&current, &data.min_supported_version);
    let has_update = is_version_less(&current, &data.latest_version);
    let mode = if force_update {
        "force"
    } else if has_update {
        "optional"
    } else {
        "none"
    };
    Ok(update_response(
        mode,
        has_update,
        force_update,
        &current,
        &data.latest_version,
        &data.min_supported_version,
        &data.download_url,
        &data.release_notes,
        None,
    ))
}

fn client_version() -> String {
    std::env::var("P2PREMOTE_CLIENT_VERSION")
        .ok()
        .filter(|value| !value.trim().is_empty())
        .or_else(|| option_env!("P2PREMOTE_CLIENT_VERSION").map(|value| value.to_string()))
        .unwrap_or_else(|| env!("CARGO_PKG_VERSION").to_string())
}

#[allow(clippy::too_many_arguments)]
fn update_response(
    mode: &str,
    has_update: bool,
    force_update: bool,
    current: &str,
    latest: &str,
    min_supported: &str,
    download_url: &str,
    release_notes: &str,
    error: Option<&str>,
) -> serde_json::Value {
    serde_json::json!({
        "mode": mode,
        "has_update": has_update,
        "force_update": force_update,
        "current": current,
        "latest": latest,
        "min_supported": min_supported,
        "download_url": download_url,
        "release_notes": release_notes,
        "error": error,
    })
}

fn is_version_less(current: &str, target: &str) -> bool {
    let parse = |version: &str| {
        version
            .trim_start_matches('v')
            .split('.')
            .map(|part| part.parse::<u64>().unwrap_or(0))
            .collect::<Vec<_>>()
    };
    let current_parts = parse(current);
    let target_parts = parse(target);
    for i in 0..current_parts.len().max(target_parts.len()) {
        let c = *current_parts.get(i).unwrap_or(&0);
        let t = *target_parts.get(i).unwrap_or(&0);
        if c < t {
            return true;
        }
        if c > t {
            return false;
        }
    }
    false
}

fn arg_value<'a>(args: &'a serde_json::Value, names: &[&str]) -> Option<&'a serde_json::Value> {
    names.iter().find_map(|name| args.get(*name))
}

fn arg_string(args: &serde_json::Value, names: &[&str]) -> Result<String, String> {
    arg_value(args, names)
        .and_then(|value| value.as_str())
        .map(|value| value.to_string())
        .ok_or_else(|| format!("missing string argument: {}", names.join("/")))
}

fn arg_optional_string(args: &serde_json::Value, names: &[&str]) -> Option<String> {
    arg_value(args, names)
        .and_then(|value| value.as_str())
        .map(|value| value.to_string())
        .filter(|value| !value.is_empty())
}

fn arg_i64(args: &serde_json::Value, names: &[&str]) -> Result<i64, String> {
    arg_value(args, names)
        .and_then(|value| value.as_i64())
        .ok_or_else(|| format!("missing integer argument: {}", names.join("/")))
}

fn arg_bool(args: &serde_json::Value, names: &[&str]) -> Option<bool> {
    arg_value(args, names).and_then(|value| value.as_bool())
}

fn arg_string_array(args: &serde_json::Value, names: &[&str]) -> Option<Vec<String>> {
    arg_value(args, names).and_then(|value| {
        value.as_array().map(|items| {
            items
                .iter()
                .filter_map(|item| item.as_str())
                .map(str::trim)
                .filter(|item| !item.is_empty())
                .map(ToString::to_string)
                .collect()
        })
    })
}

fn web_origin_allowed(headers: &HeaderMap) -> bool {
    let Some(origin) = headers.get("origin").and_then(|value| value.to_str().ok()) else {
        return false;
    };
    let Ok(uri) = origin.parse::<axum::http::Uri>() else {
        return false;
    };
    if uri.scheme_str() != Some("http") {
        return false;
    }
    let Some(origin_host) = uri.host() else {
        return false;
    };
    // 本机 Web UI 的来源 IP 已由 web_source_ip_guard 限制为 loopback。浏览器
    // 在 HTTP/2 下可能只提供 :authority，或以不同形式表示 Host 端口；因此
    // loopback Origin 不能再依赖 Host 做二次匹配。
    let origin_host = origin_host.trim_matches(|ch| matches!(ch, '[' | ']'));
    if matches!(origin_host, "127.0.0.1" | "localhost" | "::1") {
        return true;
    }

    // 远端 Web UI 仍要求 Origin 与 Host（含端口）精确一致。
    let Some(host) = headers
        .get(header::HOST)
        .and_then(|value| value.to_str().ok())
    else {
        return false;
    };
    let Ok(host_uri) = format!("https://{host}").parse::<axum::http::Uri>() else {
        return false;
    };
    let Some(request_host) = host_uri.host() else {
        return false;
    };
    let request_host = request_host.trim_matches(|ch| matches!(ch, '[' | ']'));
    if !origin_host.eq_ignore_ascii_case(request_host) {
        return false;
    }
    uri.port_u16() == host_uri.port_u16()
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::http::{HeaderMap, HeaderValue};

    fn headers_with_origin(origin: &str) -> HeaderMap {
        let mut headers = HeaderMap::new();
        headers.insert("origin", HeaderValue::from_str(origin).unwrap());
        let uri: axum::http::Uri = origin.parse().unwrap();
        headers.insert(
            "host",
            HeaderValue::from_str(uri.authority().unwrap().as_str()).unwrap(),
        );
        headers
    }

    #[test]
    fn web_origin_accepts_matching_http_host() {
        assert!(web_origin_allowed(&headers_with_origin(
            "http://127.0.0.1:48058"
        )));
        assert!(web_origin_allowed(&headers_with_origin(
            "http://localhost:48130"
        )));
        assert!(web_origin_allowed(&headers_with_origin(
            "http://[::1]:48083"
        )));
        let mut loopback_with_rewritten_host = headers_with_origin("http://127.0.0.1:48083");
        loopback_with_rewritten_host.insert("host", HeaderValue::from_static("127.0.0.1"));
        assert!(web_origin_allowed(&loopback_with_rewritten_host));
        let mut loopback_with_different_host_port = headers_with_origin("http://127.0.0.1:48083");
        loopback_with_different_host_port
            .insert("host", HeaderValue::from_static("127.0.0.1:48084"));
        assert!(web_origin_allowed(&loopback_with_different_host_port));
    }

    #[test]
    fn web_origin_rejects_mismatched_hosts_and_https() {
        assert!(!web_origin_allowed(&headers_with_origin(
            "https://example.com:48083"
        )));
        let mut remote = headers_with_origin("http://example.com:48083");
        remote.insert("host", HeaderValue::from_static("example.com:48083"));
        assert!(web_origin_allowed(&remote));
        remote.insert("host", HeaderValue::from_static("example.com:48084"));
        assert!(!web_origin_allowed(&remote));
    }

    #[test]
    fn source_allowlist_accepts_loopback_mapped_loopback_and_configured_ip() {
        let security = WebSecurityState::from_values(Some("192.0.2.191"), Some("1")).unwrap();
        assert!(security.source_allowed("127.0.0.1".parse().unwrap()));
        assert!(security.source_allowed("::ffff:127.0.0.1".parse().unwrap()));
        assert!(security.source_allowed("192.0.2.191".parse().unwrap()));
        assert!(!security.source_allowed("192.0.2.58".parse().unwrap()));
    }

    #[test]
    fn session_is_bound_to_source_and_expires_when_idle() {
        let security = WebSecurityState::from_values(Some("192.0.2.191"), Some("1")).unwrap();
        let now = Instant::now();
        security.replace_session(WebSession {
            token: "token".to_string(),
            source_ip: "192.0.2.191".parse().unwrap(),
            created_at: now,
            last_active_at: now,
        });
        assert!(security.session_token_valid("token", "192.0.2.191".parse().unwrap(), false));
        assert!(!security.session_token_valid("token", "192.0.2.58".parse().unwrap(), false));
        security.session.lock().as_mut().unwrap().last_active_at =
            now - WEB_SESSION_IDLE - Duration::from_secs(1);
        assert!(!security.session_token_valid("token", "192.0.2.191".parse().unwrap(), false));
    }

    #[test]
    fn a_new_web_session_replaces_the_previous_session() {
        let security = WebSecurityState::from_values(Some("192.0.2.191"), Some("1")).unwrap();
        let now = Instant::now();
        for token in ["first", "second"] {
            security.replace_session(WebSession {
                token: token.to_string(),
                source_ip: "192.0.2.191".parse().unwrap(),
                created_at: now,
                last_active_at: now,
            });
        }
        assert!(!security.session_token_valid("first", "192.0.2.191".parse().unwrap(), false));
        assert!(security.session_token_valid("second", "192.0.2.191".parse().unwrap(), false));
    }

    #[test]
    fn remote_access_requires_valid_ip_and_non_empty_code() {
        assert!(WebSecurityState::from_values(Some("192.0.2.191"), Some("1")).is_ok());
        assert!(WebSecurityState::from_values(Some("192.0.2.191"), None).is_err());
        assert!(WebSecurityState::from_values(Some("0.0.0.0"), Some("1")).is_err());
        assert!(WebSecurityState::from_values(Some("not-an-ip"), Some("1")).is_err());
        assert!(WebSecurityState::from_values(None, Some("1")).is_ok());
    }
}
