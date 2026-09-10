use super::*;
use crate::tunnel_control::{DesktopControlRequest, TunnelControlMessage, TunnelDesktopCommand};
use std::{
    path::{Path, PathBuf},
    process::{Command, Stdio},
};
#[cfg(windows)]
use std::os::windows::process::CommandExt;
use uuid::Uuid;

const ENGINE_START_TIMEOUT: Duration = Duration::from_secs(30);
const ENGINE_STOP_TIMEOUT: Duration = Duration::from_secs(5);
const DESKTOP_PORT: u16 = 39090;
#[cfg(windows)]
const DETACHED_PROCESS: u32 = 0x00000008;
#[cfg(windows)]
const CREATE_NEW_PROCESS_GROUP: u32 = 0x00000200;
#[cfg(windows)]
const CREATE_BREAKAWAY_FROM_JOB: u32 = 0x01000000;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(super) enum EngineStage {
    Configuration,
    SessionDiscovery,
    Bind,
}

#[derive(Clone, Debug)]
pub(super) struct EngineFailure {
    pub(super) code: String,
    pub(super) stage: EngineStage,
    pub(super) message: String,
    pub(super) platform_detail: Option<String>,
}

impl EngineFailure {
    fn new(code: impl Into<String>, stage: EngineStage, message: impl Into<String>) -> Self {
        Self {
            code: code.into(),
            stage,
            message: message.into(),
            platform_detail: None,
        }
    }

    fn io(
        code: &'static str,
        stage: EngineStage,
        context: &'static str,
        error: std::io::Error,
    ) -> Self {
        let platform_detail = format!(
            "kind={:?}, raw_os_error={:?}",
            error.kind(),
            error.raw_os_error()
        );
        let mut failure = Self::new(code, stage, format!("{context}: {error}"));
        failure.platform_detail = Some(platform_detail);
        failure
    }
}

impl std::fmt::Display for EngineFailure {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            formatter,
            "{} at {:?}: {}{}",
            self.code,
            self.stage,
            self.message,
            self.platform_detail
                .as_deref()
                .map(|detail| format!(" ({detail})"))
                .unwrap_or_default()
        )
    }
}

impl std::error::Error for EngineFailure {}

#[derive(Debug)]
pub(super) struct SessionConfig {
    attempt_id: String,
    peer_device_id: i64,
    local_virtual_ip: String,
    peer_virtual_ip: String,
    port: u16,
}

fn launch_desktop_process(
    executable: &Path,
    role: DesktopEngineRole,
    config: &SessionConfig,
) -> Result<u32, EngineFailure> {
        let address = match role {
            DesktopEngineRole::Host => format!("{}:{}", config.local_virtual_ip, config.port),
            DesktopEngineRole::Controller => {
                format!("{}:{}", config.peer_virtual_ip, config.port)
            }
        };
        let mut command = Command::new(executable);
        match role {
            DesktopEngineRole::Host => {
                command.args(["host", "--listen", &address]);
            }
            DesktopEngineRole::Controller => {
                command.args(["--connect", &address]);
            }
        }
        command.stdin(Stdio::null()).stdout(Stdio::null()).stderr(Stdio::null());
        #[cfg(windows)]
        command.creation_flags(
            DETACHED_PROCESS | CREATE_NEW_PROCESS_GROUP | CREATE_BREAKAWAY_FROM_JOB,
        );
        match role {
            DesktopEngineRole::Host => {
                let status = command.status().map_err(|error| {
                    EngineFailure::io(
                        "desktop_process_spawn_failed",
                        EngineStage::SessionDiscovery,
                        "start RustDeskTiny host",
                        error,
                    )
                })?;
                if !status.success() {
                    return Err(EngineFailure::new(
                        "desktop_host_start_failed",
                        EngineStage::Bind,
                        format!("RustDeskTiny host command exited with {status}"),
                    ));
                }
                Ok(0)
            }
            DesktopEngineRole::Controller => command.spawn().map(|child| child.id()).map_err(|error| {
                EngineFailure::io(
                    "desktop_process_spawn_failed",
                    EngineStage::SessionDiscovery,
                    "start detached RustDeskTiny controller",
                    error,
                )
            }),
        }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(super) enum DesktopEngineRole {
    Host,
    Controller,
}

struct DesktopStartReservation {
    shared: Arc<Mutex<SharedRuntimeState>>,
    peer_device_id: i64,
    attempt_id: String,
    committed: bool,
}

impl DesktopStartReservation {
    fn commit(mut self) {
        let mut state = self.shared.lock();
        state.desktop_engine_starting.remove(&self.peer_device_id);
        self.committed = true;
    }
}

impl Drop for DesktopStartReservation {
    fn drop(&mut self) {
        if self.committed {
            return;
        }
        let mut state = self.shared.lock();
        if state.desktop_engine_starting.get(&self.peer_device_id) == Some(&self.attempt_id) {
            state.desktop_engine_starting.remove(&self.peer_device_id);
        }
    }
}

fn reserve_desktop_start(
    shared: &Arc<Mutex<SharedRuntimeState>>,
    peer_device_id: i64,
    attempt_id: &str,
) -> Result<DesktopStartReservation> {
    let mut state = shared.lock();
    if state.desktop_engine_starting.contains_key(&peer_device_id) {
        return Err(anyhow!("desktop_session_start_in_progress"));
    }
    state
        .desktop_engine_starting
        .insert(peer_device_id, attempt_id.to_owned());
    drop(state);
    Ok(DesktopStartReservation {
        shared: shared.clone(),
        peer_device_id,
        attempt_id: attempt_id.to_owned(),
        committed: false,
    })
}

pub(super) fn start_desktop_engine(
    role: DesktopEngineRole,
    config: SessionConfig,
) -> Result<(), EngineFailure> {
    let attempt_id = config.attempt_id.clone();
    let peer_device_id = config.peer_device_id;
    let executable = resolve_desktop_executable()?;
    info!(peer_device_id, attempt_id = %attempt_id, role = ?role, executable = %executable.display(),
        "[DesktopEngine] starting process");
    let pid = launch_desktop_process(&executable, role, &config)?;
    info!(peer_device_id, attempt_id = %attempt_id, role = ?role, pid,
        "[DesktopEngine] process spawned");
    Ok(())
}

fn resolve_desktop_executable() -> Result<PathBuf, EngineFailure> {
    if let Some(path) = std::env::var_os("RUSTDESK_TINY_PATH") {
        return validate_desktop_path(PathBuf::from(path));
    }
    #[cfg(windows)]
    let path = std::env::current_exe()
        .ok()
        .and_then(|path| {
            path.parent().map(|parent| {
                parent
                    .join("RustDeskTiny")
                    .join("RustDeskTiny.exe")
            })
        })
        .ok_or_else(|| {
            EngineFailure::new(
                "engine_path_resolution_failed",
                EngineStage::Configuration,
                "cannot resolve the service executable directory",
            )
        })?;
    #[cfg(not(windows))]
    return Err(EngineFailure::new(
        "desktop_platform_unsupported",
        EngineStage::Configuration,
        "the RustDesk-based desktop component currently supports Windows only",
    ));
    #[cfg(windows)]
    validate_desktop_path(path)
}

fn validate_desktop_path(path: PathBuf) -> Result<PathBuf, EngineFailure> {
    if !path.is_file() {
        return Err(EngineFailure::new(
            "desktop_executable_not_found",
            EngineStage::Configuration,
            format!("desktop executable not found: {}", path.display()),
        ));
    }
    Ok(path)
}

pub(super) async fn start_active_desktop_session(
    shared: &Arc<Mutex<SharedRuntimeState>>,
    peer_device_id: i64,
) -> Result<serde_json::Value> {
    let session = require_wgvpn_session(peer_device_id)?;
    if !session.is_active {
        return Err(anyhow!("desktop_controller_requires_active_tunnel"));
    }
    let attempt_id = Uuid::new_v4().to_string();
    info!(peer_device_id, attempt_id = %attempt_id, peer_virtual_ip = %session.peer_virtual_ip,
        "[Desktop] active start initialized");
    let reservation = reserve_desktop_start(shared, peer_device_id, &attempt_id)?;
    info!(peer_device_id, attempt_id = %attempt_id, port = DESKTOP_PORT,
        "[Desktop] sending DesktopStart over tunnel control channel");
    match request_desktop_control(
        shared,
        peer_device_id,
        DesktopControlRequest::Start {
            attempt_id: attempt_id.clone(),
            port: DESKTOP_PORT,
        },
        ENGINE_START_TIMEOUT,
    )
    .await?
    {
        TunnelControlMessage::DesktopReady {
            attempt_id: response_id,
        } if response_id == attempt_id => {
            info!(peer_device_id, attempt_id = %attempt_id, "[Desktop] received DesktopReady");
        }
        TunnelControlMessage::DesktopFailed {
            error_code,
            message,
            ..
        } => return Err(anyhow!("{error_code}: {message}")),
        other => return Err(anyhow!("unexpected_desktop_start_response: {other:?}")),
    }

    let config = session_config(
        attempt_id.clone(),
        peer_device_id,
        &session.virtual_ip,
        &session.peer_virtual_ip,
        DESKTOP_PORT,
    );
    let controller_result = tokio::task::spawn_blocking(move || {
        start_desktop_engine(DesktopEngineRole::Controller, config)
    })
    .await
    .map_err(|error| anyhow!("desktop_controller_start_task_failed: {error}"))?;
    match controller_result {
        Ok(()) => {}
        Err(controller_error) => {
            let stop_result = request_desktop_control(
                shared,
                peer_device_id,
                DesktopControlRequest::Stop {
                    attempt_id: attempt_id.clone(),
                    reason: "controller_start_failed".into(),
                },
                ENGINE_STOP_TIMEOUT,
            )
            .await;
            return match stop_result {
                Ok(TunnelControlMessage::DesktopStopped { .. }) => Err(anyhow!(controller_error.to_string())),
                Ok(other) => Err(anyhow!("controller start failed: {controller_error}; unexpected remote cleanup response: {other:?}")),
                Err(stop_error) => Err(anyhow!(
                    "controller start failed: {controller_error}; remote host cleanup notification failed: {stop_error}"
                )),
            };
        }
    }
    reservation.commit();
    Ok(serde_json::json!({
        "attempt_id": attempt_id,
        "state": "launched",
        "window": "native"
    }))
}

async fn request_desktop_control(
    shared: &Arc<Mutex<SharedRuntimeState>>,
    peer_device_id: i64,
    request: DesktopControlRequest,
    timeout: Duration,
) -> Result<TunnelControlMessage> {
    let timeout_code = match &request {
        DesktopControlRequest::Start { .. } => "desktop_host_start_timeout",
        DesktopControlRequest::Stop { .. } => "desktop_host_stop_timeout",
    };
    let desktop_tx = shared
        .lock()
        .wgvpn_health_controls
        .get(&peer_device_id)
        .map(|control| control.desktop_tx.clone())
        .ok_or_else(|| anyhow!("desktop_control_channel_unavailable"))?;
    let (response_tx, response_rx) = tokio::sync::oneshot::channel();
    desktop_tx
        .send(TunnelDesktopCommand {
            request,
            response: response_tx,
        })
        .map_err(|_| anyhow!("desktop_control_channel_unavailable"))?;
    tokio::time::timeout(timeout, response_rx)
        .await
        .map_err(|_| anyhow!(timeout_code))?
        .map_err(|_| anyhow!("desktop_control_channel_closed"))?
        .map_err(anyhow::Error::msg)
}

pub(super) async fn start_passive_desktop_host(
    shared: &Arc<Mutex<SharedRuntimeState>>,
    peer_device_id: i64,
    attempt_id: String,
    port: u16,
) -> Result<()> {
    info!(peer_device_id, attempt_id = %attempt_id, port,
        "[Desktop] host received validated DesktopStart");
    if port != DESKTOP_PORT {
        return Err(anyhow!(
            "desktop_port_mismatch: expected {DESKTOP_PORT}, received {port}"
        ));
    }
    let session = require_wgvpn_session(peer_device_id)?;
    let reservation = reserve_desktop_start(shared, peer_device_id, &attempt_id)?;
    let config = session_config(
        attempt_id,
        peer_device_id,
        &session.virtual_ip,
        &session.peer_virtual_ip,
        port,
    );
    tokio::task::spawn_blocking(move || start_desktop_engine(DesktopEngineRole::Host, config))
            .await
            .map_err(|error| anyhow!("desktop_host_start_task_failed: {error}"))?
            .map_err(|error| anyhow!(error.to_string()))?;
    reservation.commit();
    info!(peer_device_id, "[Desktop] host engine ready");
    Ok(())
}

pub(super) async fn handle_tunnel_desktop_request(
    shared: &Arc<Mutex<SharedRuntimeState>>,
    peer_device_id: i64,
    message: TunnelControlMessage,
) -> TunnelControlMessage {
    match message {
        TunnelControlMessage::DesktopStart {
            attempt_id,
            port,
        } => {
            info!(peer_device_id, attempt_id = %attempt_id, port,
                "[Desktop] processing DesktopStart");
            match start_passive_desktop_host(
                shared,
                peer_device_id,
                attempt_id.clone(),
                port,
            )
            .await
            {
                Ok(()) => {
                    info!(peer_device_id, attempt_id = %attempt_id, "[Desktop] returning DesktopReady");
                    TunnelControlMessage::DesktopReady { attempt_id }
                }
                Err(error) => {
                    let error_code = desktop_error_code(&error);
                    error!(peer_device_id, attempt_id = %attempt_id, error_code = %error_code,
                        error = %error, "[Desktop] returning DesktopFailed");
                    TunnelControlMessage::DesktopFailed {
                        attempt_id,
                        error_code,
                        message: error.to_string(),
                    }
                }
            }
        }
        TunnelControlMessage::DesktopStop { attempt_id, reason } => {
            let _ = (shared, peer_device_id, reason);
            TunnelControlMessage::DesktopStopped { attempt_id }
        }
        other => TunnelControlMessage::DesktopFailed {
            attempt_id: String::new(),
            error_code: "unexpected_desktop_control_message".to_string(),
            message: format!("received {other:?}"),
        },
    }
}

fn desktop_error_code(error: &anyhow::Error) -> String {
    error
        .to_string()
        .split(':')
        .next()
        .unwrap_or("desktop_operation_failed")
        .trim()
        .to_string()
}

pub(super) async fn stop_desktop_session(
    shared: &Arc<Mutex<SharedRuntimeState>>,
    peer_device_id: i64,
    reason: &str,
) -> Result<()> {
    let _ = (shared, peer_device_id, reason);
    Ok(())
}

fn require_wgvpn_session(peer_device_id: i64) -> Result<crate::p2p::wgvpn_flow::WgVpnSession> {
    wgvpn_flow::snapshot_sessions()
        .into_iter()
        .find(|session| session.peer_device_id == peer_device_id)
        .ok_or_else(|| anyhow!("desktop_wgvpn_session_not_found"))
}

fn session_config(
    attempt_id: String,
    peer_device_id: i64,
    local_virtual_ip: &str,
    peer_virtual_ip: &str,
    port: u16,
) -> SessionConfig {
    SessionConfig {
        attempt_id,
        peer_device_id,
        local_virtual_ip: local_virtual_ip.to_owned(),
        peer_virtual_ip: peer_virtual_ip.to_owned(),
        port,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn missing_configured_engine_is_an_explicit_error() {
        let missing =
            std::env::temp_dir().join(format!("p2premote-missing-engine-{}", std::process::id()));
        let error = validate_desktop_path(missing).unwrap_err();
        assert_eq!(error.code, "desktop_executable_not_found");
    }

    #[test]
    fn concurrent_start_is_rejected_and_failed_start_releases_reservation() {
        let shared = Arc::new(Mutex::new(SharedRuntimeState::default()));
        let first = reserve_desktop_start(&shared, 42, "attempt-one").expect("first reservation");
        let error = reserve_desktop_start(&shared, 42, "attempt-two")
            .err()
            .expect("concurrent reservation must fail");
        assert_eq!(error.to_string(), "desktop_session_start_in_progress");
        drop(first);
        reserve_desktop_start(&shared, 42, "attempt-three")
            .expect("reservation should be released after failed start");
    }

    #[tokio::test]
    async fn active_desktop_requires_an_established_active_wgvpn() {
        let shared = Arc::new(Mutex::new(SharedRuntimeState::default()));
        let error = start_active_desktop_session(&shared, i64::MAX)
            .await
            .unwrap_err();
        assert_eq!(error.to_string(), "desktop_wgvpn_session_not_found");
        assert!(shared.lock().p2p_attempt_waiters.is_empty());
    }
}
