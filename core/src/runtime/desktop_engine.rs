use super::*;
use crate::tunnel_control::{DesktopControlRequest, TunnelControlMessage, TunnelDesktopCommand};
use serde::Deserialize;
use std::{
    io::{BufRead, BufReader},
    path::{Path, PathBuf},
    process::{Child, Command, Stdio},
    sync::mpsc::{self as std_mpsc, Receiver},
    thread::{self, JoinHandle},
};
use uuid::Uuid;

const ENGINE_START_TIMEOUT: Duration = Duration::from_secs(30);
const ENGINE_STOP_TIMEOUT: Duration = Duration::from_secs(5);
const ENGINE_POLL_INTERVAL: Duration = Duration::from_millis(200);
const DESKTOP_PORT: u16 = 39090;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(super) enum EngineStage {
    Configuration,
    SessionDiscovery,
    Bind,
    Accept,
    Connect,
    Authentication,
    FirstFrame,
    Streaming,
    Shutdown,
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

#[derive(Clone, Debug)]
pub(super) enum EngineEvent {
    Starting,
    HostReady,
    Connecting,
    Streaming,
    Stopped,
}

#[derive(Debug)]
pub(super) struct SessionConfig {
    attempt_id: String,
    peer_device_id: i64,
    local_virtual_ip: String,
    peer_virtual_ip: String,
    port: u16,
}

#[derive(Debug, Deserialize)]
struct DesktopCliEvent {
    event: String,
    #[serde(default)]
    stage: Option<String>,
    #[serde(default)]
    error_code: Option<String>,
    #[serde(default)]
    message: Option<String>,
    #[serde(default)]
    platform_error: Option<String>,
}

struct DesktopCliProcess {
    child: Child,
    events: Receiver<Result<EngineEvent, EngineFailure>>,
}

impl DesktopCliProcess {
    fn start(
        session_helper: &Path,
        role: DesktopEngineRole,
        config: &SessionConfig,
    ) -> Result<Self, EngineFailure> {
        let address = match role {
            DesktopEngineRole::Host => format!("{}:{}", config.local_virtual_ip, config.port),
            DesktopEngineRole::Controller => {
                format!("{}:{}", config.peer_virtual_ip, config.port)
            }
        };
        let mut command = Command::new(session_helper);
        command.arg("--");
        match role {
            DesktopEngineRole::Host => {
                command.args(["host", "--listen", &address, "--machine-readable"]);
            }
            DesktopEngineRole::Controller => {
                command.args(["connect", "--address", &address, "--machine-readable"]);
            }
        }
        let mut child = command
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .map_err(|error| {
                EngineFailure::io(
                    "desktop_process_spawn_failed",
                    EngineStage::SessionDiscovery,
                    "spawn p2premote desktop session helper",
                    error,
                )
            })?;
        let stdout = child.stdout.take().ok_or_else(|| {
            cleanup_cli_start(
                &mut child,
                EngineFailure::new(
                    "desktop_stdout_unavailable",
                    EngineStage::Configuration,
                    "desktop process stdout was not piped",
                ),
            )
        })?;
        let stderr = child.stderr.take().ok_or_else(|| {
            cleanup_cli_start(
                &mut child,
                EngineFailure::new(
                    "desktop_stderr_unavailable",
                    EngineStage::Configuration,
                    "desktop process stderr was not piped",
                ),
            )
        })?;
        let (event_tx, event_rx) = std_mpsc::channel();
        thread::spawn(move || {
            for line in BufReader::new(stdout).lines() {
                let event = match line {
                    Ok(line) => decode_cli_event(&line),
                    Err(error) => Err(EngineFailure::io(
                        "desktop_event_read_failed",
                        EngineStage::Streaming,
                        "read p2premote desktop event",
                        error,
                    )),
                };
                if event_tx.send(event).is_err() {
                    break;
                }
            }
        });
        thread::spawn(move || {
            for line in BufReader::new(stderr).lines() {
                match line {
                    Ok(line) => warn!(message = %line, "[p2pRemoteDesktop] stderr"),
                    Err(error) => {
                        error!(error = %error, "[p2pRemoteDesktop] stderr read failed");
                        break;
                    }
                }
            }
        });
        Ok(Self {
            child,
            events: event_rx,
        })
    }

    fn id(&self) -> u32 {
        self.child.id()
    }

    fn poll_event(&mut self, timeout: Duration) -> Result<Option<EngineEvent>, EngineFailure> {
        match self.events.recv_timeout(timeout) {
            Ok(result) => result.map(Some),
            Err(std_mpsc::RecvTimeoutError::Timeout) => {
                match self.child.try_wait().map_err(|error| {
                    EngineFailure::io(
                        "desktop_process_wait_failed",
                        EngineStage::Streaming,
                        "query p2premote desktop process",
                        error,
                    )
                })? {
                    Some(status) => Err(EngineFailure::new(
                        "desktop_process_exited",
                        EngineStage::Streaming,
                        format!("p2premote desktop exited with {status}"),
                    )),
                    None => Ok(None),
                }
            }
            Err(std_mpsc::RecvTimeoutError::Disconnected) => Err(EngineFailure::new(
                "desktop_event_channel_closed",
                EngineStage::Streaming,
                "p2premote desktop event channel closed",
            )),
        }
    }

    fn next_event(&mut self, timeout: Duration) -> Result<EngineEvent, EngineFailure> {
        self.poll_event(timeout)?.ok_or_else(|| {
            EngineFailure::new(
                "desktop_event_timeout",
                EngineStage::Streaming,
                format!("no p2premote desktop event within {timeout:?}"),
            )
        })
    }

    fn wait_for_host_ready(&mut self, timeout: Duration) -> Result<(), EngineFailure> {
        let deadline = Instant::now() + timeout;
        loop {
            let remaining = deadline.saturating_duration_since(Instant::now());
            if remaining.is_zero() {
                return Err(EngineFailure::new(
                    "host_ready_timeout",
                    EngineStage::Bind,
                    format!("host was not ready within {timeout:?}"),
                ));
            }
            let event = self.next_event(remaining).map_err(|error| {
                if error.code == "desktop_event_timeout" {
                    EngineFailure::new(
                        "host_ready_timeout",
                        EngineStage::Bind,
                        format!("host was not ready within {timeout:?}"),
                    )
                } else {
                    error
                }
            })?;
            match event {
                EngineEvent::Starting => {}
                EngineEvent::HostReady => return Ok(()),
                other => {
                    return Err(EngineFailure::new(
                        "unexpected_desktop_event",
                        EngineStage::Bind,
                        format!("expected host_ready, received {other:?}"),
                    ))
                }
            }
        }
    }

    fn wait_for_streaming(&mut self, timeout: Duration) -> Result<(), EngineFailure> {
        let deadline = Instant::now() + timeout;
        loop {
            let remaining = deadline.saturating_duration_since(Instant::now());
            if remaining.is_zero() {
                return Err(EngineFailure::new(
                    "controller_streaming_timeout",
                    EngineStage::FirstFrame,
                    format!("controller did not reach streaming within {timeout:?}"),
                ));
            }
            let event = self.next_event(remaining).map_err(|error| {
                if error.code == "desktop_event_timeout" {
                    EngineFailure::new(
                        "controller_streaming_timeout",
                        EngineStage::FirstFrame,
                        format!("controller did not reach streaming within {timeout:?}"),
                    )
                } else {
                    error
                }
            })?;
            match event {
                EngineEvent::Connecting | EngineEvent::Starting => {}
                EngineEvent::Streaming => return Ok(()),
                other => {
                    return Err(EngineFailure::new(
                        "unexpected_desktop_event",
                        EngineStage::FirstFrame,
                        format!("expected streaming, received {other:?}"),
                    ))
                }
            }
        }
    }

    fn stop(mut self, _reason: impl Into<String>, timeout: Duration) -> Result<(), EngineFailure> {
        self.child.kill().map_err(|error| {
            EngineFailure::io(
                "desktop_process_terminate_failed",
                EngineStage::Shutdown,
                "terminate p2premote desktop session helper",
                error,
            )
        })?;
        let deadline = Instant::now() + timeout;
        while Instant::now() < deadline {
            if self
                .child
                .try_wait()
                .map_err(|error| {
                    EngineFailure::io(
                        "desktop_process_wait_failed",
                        EngineStage::Shutdown,
                        "query terminated p2premote desktop session helper",
                        error,
                    )
                })?
                .is_some()
            {
                return Ok(());
            }
            thread::sleep(Duration::from_millis(10));
        }
        Err(EngineFailure::new(
            "desktop_process_stop_timeout",
            EngineStage::Shutdown,
            format!("desktop session helper did not exit within {timeout:?}"),
        ))
    }
}

impl Drop for DesktopCliProcess {
    fn drop(&mut self) {
        match self.child.try_wait() {
            Ok(Some(_)) => {}
            Ok(None) => {
                if let Err(error) = self.child.kill() {
                    error!(error = %error, "[p2pRemoteDesktop] process cleanup kill failed");
                }
                if let Err(error) = self.child.wait() {
                    error!(error = %error, "[p2pRemoteDesktop] process cleanup reap failed");
                }
            }
            Err(error) => {
                error!(error = %error, "[p2pRemoteDesktop] process cleanup status failed");
            }
        }
    }
}

fn decode_cli_event(line: &str) -> Result<EngineEvent, EngineFailure> {
    let event = serde_json::from_str::<DesktopCliEvent>(line).map_err(|error| {
        EngineFailure::new(
            "desktop_protocol_error",
            EngineStage::Configuration,
            format!("decode desktop event: {error}; payload={line:?}"),
        )
    })?;
    match event.event.as_str() {
        "starting" => Ok(EngineEvent::Starting),
        "host_ready" => Ok(EngineEvent::HostReady),
        "connecting" => Ok(EngineEvent::Connecting),
        "streaming" => Ok(EngineEvent::Streaming),
        "stopped" => Ok(EngineEvent::Stopped),
        "failed" => Err(cli_failure(event)),
        other => Err(EngineFailure::new(
            "desktop_protocol_error",
            EngineStage::Configuration,
            format!("unknown desktop event: {other}"),
        )),
    }
}

fn cli_failure(event: DesktopCliEvent) -> EngineFailure {
    let stage = match event.stage.as_deref() {
        Some("bind") => EngineStage::Bind,
        Some("accept") => EngineStage::Accept,
        Some("handshake") | Some("authentication") => EngineStage::Authentication,
        Some("connect") => EngineStage::Connect,
        Some("streaming") => EngineStage::Streaming,
        Some("shutdown") => EngineStage::Shutdown,
        _ => EngineStage::Configuration,
    };
    let mut failure = EngineFailure::new(
        event
            .error_code
            .unwrap_or_else(|| "desktop_failed".to_owned()),
        stage,
        event
            .message
            .unwrap_or_else(|| "p2premote desktop reported an unspecified failure".to_owned()),
    );
    failure.platform_detail = event.platform_error;
    failure
}

fn cleanup_cli_start(child: &mut Child, mut failure: EngineFailure) -> EngineFailure {
    let mut cleanup = Vec::new();
    if let Err(error) = child.kill() {
        cleanup.push(format!("kill failed: {error}"));
    }
    if let Err(error) = child.wait() {
        cleanup.push(format!("reap failed: {error}"));
    }
    if !cleanup.is_empty() {
        failure.platform_detail = Some(cleanup.join("; "));
    }
    failure
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(super) enum DesktopEngineRole {
    Host,
    Controller,
}

pub(super) struct DesktopEngineTask {
    attempt_id: String,
    role: DesktopEngineRole,
    stop_tx: std_mpsc::Sender<String>,
    outcome_rx: std_mpsc::Receiver<Result<EngineEvent, EngineFailure>>,
    worker: Option<JoinHandle<()>>,
}

struct DesktopStartReservation {
    shared: Arc<Mutex<SharedRuntimeState>>,
    peer_device_id: i64,
    attempt_id: String,
    committed: bool,
}

impl DesktopStartReservation {
    fn commit(mut self, task: DesktopEngineTask) {
        let mut state = self.shared.lock();
        state.desktop_engine_starting.remove(&self.peer_device_id);
        state.desktop_engine_tasks.insert(self.peer_device_id, task);
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
    if state.desktop_engine_tasks.contains_key(&peer_device_id) {
        return Err(anyhow!("desktop_session_already_running"));
    }
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

impl DesktopEngineTask {
    pub(super) fn attempt_id(&self) -> &str {
        &self.attempt_id
    }

    pub(super) fn poll_outcome(&self) -> Result<Option<EngineEvent>, EngineFailure> {
        match self.outcome_rx.try_recv() {
            Ok(result) => result.map(Some),
            Err(std_mpsc::TryRecvError::Empty) => Ok(None),
            Err(std_mpsc::TryRecvError::Disconnected) => Err(EngineFailure::new(
                "engine_supervisor_closed",
                EngineStage::Streaming,
                "desktop engine supervisor exited without a terminal event",
            )),
        }
    }

    pub(super) fn stop(mut self, reason: impl Into<String>) -> Result<(), EngineFailure> {
        let _ = self.stop_tx.send(reason.into());
        if let Some(worker) = self.worker.take() {
            worker.join().map_err(|_| {
                EngineFailure::new(
                    "engine_supervisor_panicked",
                    EngineStage::Shutdown,
                    "desktop engine supervisor panicked while stopping",
                )
            })?;
        }
        match self.outcome_rx.try_recv() {
            Ok(Ok(EngineEvent::Stopped)) => Ok(()),
            Ok(Ok(other)) => Err(EngineFailure::new(
                "unexpected_engine_event",
                EngineStage::Shutdown,
                format!("expected stopped event, received {other:?}"),
            )),
            Ok(Err(error)) if is_expected_peer_close(&error) => Ok(()),
            Ok(Err(error)) => Err(error),
            Err(error) => Err(EngineFailure::new(
                "engine_stop_result_missing",
                EngineStage::Shutdown,
                format!("engine supervisor returned no stop result: {error}"),
            )),
        }
    }

    fn join_supervisor(mut self) -> Result<(), EngineFailure> {
        if let Some(worker) = self.worker.take() {
            worker.join().map_err(|_| {
                EngineFailure::new(
                    "engine_supervisor_panicked",
                    EngineStage::Shutdown,
                    "desktop engine supervisor panicked after reporting its terminal event",
                )
            })?;
        }
        Ok(())
    }
}

fn is_expected_peer_close(error: &EngineFailure) -> bool {
    matches!(
        error.code.as_str(),
        "controller_streaming_failed" | "clipboard_read_failed"
    ) && [
        "peer closed",
        "connection reset",
        "forcibly closed",
        "强迫关闭",
        "broken pipe",
    ]
    .iter()
    .any(|needle| error.message.to_ascii_lowercase().contains(needle))
}

pub(super) fn start_desktop_engine(
    role: DesktopEngineRole,
    config: SessionConfig,
) -> Result<DesktopEngineTask, EngineFailure> {
    let attempt_id = config.attempt_id.clone();
    let peer_device_id = config.peer_device_id;
    let session_helper = resolve_desktop_session_helper()?;
    info!(peer_device_id, attempt_id = %attempt_id, role = ?role, executable = %session_helper.display(),
        "[DesktopEngine] starting process");
    let mut process = DesktopCliProcess::start(&session_helper, role, &config)?;
    info!(peer_device_id, attempt_id = %attempt_id, role = ?role, pid = process.id(),
        "[DesktopEngine] process spawned");
    match role {
        DesktopEngineRole::Host => {
            process.wait_for_host_ready(ENGINE_START_TIMEOUT)?;
        }
        DesktopEngineRole::Controller => {
            process.wait_for_streaming(ENGINE_START_TIMEOUT)?;
        }
    }
    info!(peer_device_id, attempt_id = %attempt_id, role = ?role, pid = process.id(),
        "[DesktopEngine] startup milestone reached");
    Ok(spawn_supervisor(attempt_id, role, process))
}

fn spawn_supervisor(
    attempt_id: String,
    role: DesktopEngineRole,
    mut process: DesktopCliProcess,
) -> DesktopEngineTask {
    let (stop_tx, stop_rx) = std_mpsc::channel::<String>();
    let (outcome_tx, outcome_rx) = std_mpsc::channel();
    let worker = thread::spawn(move || loop {
        match stop_rx.try_recv() {
            Ok(reason) => {
                let result = process
                    .stop(reason, ENGINE_STOP_TIMEOUT)
                    .map(|_| EngineEvent::Stopped);
                let _ = outcome_tx.send(result);
                return;
            }
            Err(std_mpsc::TryRecvError::Disconnected) => {
                let result = process
                    .stop("supervisor_owner_dropped", ENGINE_STOP_TIMEOUT)
                    .map(|_| EngineEvent::Stopped);
                let _ = outcome_tx.send(result);
                return;
            }
            Err(std_mpsc::TryRecvError::Empty) => {}
        }
        match process.poll_event(ENGINE_POLL_INTERVAL) {
            Ok(None) => {}
            Ok(Some(event @ EngineEvent::Stopped)) => {
                let _ = outcome_tx.send(Ok(event));
                return;
            }
            Ok(Some(event)) => {
                let _ = outcome_tx.send(Err(EngineFailure::new(
                    "unexpected_engine_event",
                    EngineStage::Streaming,
                    format!("unexpected post-start event: {event:?}"),
                )));
                return;
            }
            Err(error) => {
                let _ = outcome_tx.send(Err(error));
                return;
            }
        }
    });
    DesktopEngineTask {
        attempt_id,
        role,
        stop_tx,
        outcome_rx,
        worker: Some(worker),
    }
}

fn resolve_desktop_session_helper() -> Result<PathBuf, EngineFailure> {
    if let Some(path) = std::env::var_os("P2PREMOTE_DESKTOP_SESSION_HELPER_PATH") {
        return validate_desktop_path(PathBuf::from(path));
    }
    #[cfg(windows)]
    let path = std::env::current_exe()
        .ok()
        .and_then(|path| {
            path.parent().map(|parent| {
                parent
                    .join("p2premote-desktop")
                    .join("p2premote-desktop-session-helper.exe")
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
            "desktop_session_helper_not_found",
            EngineStage::Configuration,
            format!("desktop session helper not found: {}", path.display()),
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
    let session_secret = format!("{}{}", Uuid::new_v4().simple(), Uuid::new_v4().simple());
    info!(peer_device_id, attempt_id = %attempt_id, port = DESKTOP_PORT,
        "[Desktop] sending DesktopStart over tunnel control channel");
    match request_desktop_control(
        shared,
        peer_device_id,
        DesktopControlRequest::Start {
            attempt_id: attempt_id.clone(),
            session_secret: session_secret.clone(),
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
        session_secret,
    );
    let controller_result = tokio::task::spawn_blocking(move || {
        start_desktop_engine(DesktopEngineRole::Controller, config)
    })
    .await
    .map_err(|error| anyhow!("desktop_controller_start_task_failed: {error}"))?;
    let task = match controller_result {
        Ok(task) => task,
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
    };
    reservation.commit(task);
    Ok(serde_json::json!({
        "attempt_id": attempt_id,
        "state": "streaming",
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
    session_secret: String,
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
        session_secret,
    );
    let task =
        tokio::task::spawn_blocking(move || start_desktop_engine(DesktopEngineRole::Host, config))
            .await
            .map_err(|error| anyhow!("desktop_host_start_task_failed: {error}"))?
            .map_err(|error| anyhow!(error.to_string()))?;
    reservation.commit(task);
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
            session_secret,
            port,
        } => {
            info!(peer_device_id, attempt_id = %attempt_id, port,
                "[Desktop] processing DesktopStart");
            match start_passive_desktop_host(
                shared,
                peer_device_id,
                attempt_id.clone(),
                session_secret,
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
            let current_attempt = shared
                .lock()
                .desktop_engine_tasks
                .get(&peer_device_id)
                .map(|task| task.attempt_id().to_owned());
            if current_attempt.as_deref() != Some(attempt_id.as_str()) {
                return TunnelControlMessage::DesktopFailed {
                    attempt_id,
                    error_code: "desktop_stop_attempt_mismatch".to_string(),
                    message: format!("expected active attempt {current_attempt:?}"),
                };
            }
            match stop_desktop_session(shared, peer_device_id, &reason).await {
                Ok(()) => TunnelControlMessage::DesktopStopped { attempt_id },
                Err(error) => TunnelControlMessage::DesktopFailed {
                    attempt_id,
                    error_code: desktop_error_code(&error),
                    message: error.to_string(),
                },
            }
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
    let task = shared
        .lock()
        .desktop_engine_tasks
        .remove(&peer_device_id)
        .ok_or_else(|| anyhow!("desktop_session_not_running"))?;
    let remote_result = if task.role == DesktopEngineRole::Controller {
        Some(
            request_desktop_control(
                shared,
                peer_device_id,
                DesktopControlRequest::Stop {
                    attempt_id: task.attempt_id().to_owned(),
                    reason: reason.to_owned(),
                },
                ENGINE_STOP_TIMEOUT,
            )
            .await
            .and_then(|message| match message {
                TunnelControlMessage::DesktopStopped { .. } => Ok(()),
                TunnelControlMessage::DesktopFailed {
                    error_code,
                    message,
                    ..
                } => Err(anyhow!("{error_code}: {message}")),
                other => Err(anyhow!("unexpected_desktop_stop_response: {other:?}")),
            }),
        )
    } else {
        None
    };
    let reason = reason.to_owned();
    let local_result = tokio::task::spawn_blocking(move || task.stop(reason))
        .await
        .map_err(|error| anyhow!("desktop_stop_task_failed: {error}"))?
        .map_err(|error| anyhow!(error.to_string()));
    match (local_result, remote_result) {
        (Ok(()), None | Some(Ok(()))) => Ok(()),
        (Err(local), None | Some(Ok(()))) => Err(local),
        (Ok(()), Some(Err(remote))) => Err(anyhow!("remote_desktop_stop_failed: {remote}")),
        (Err(local), Some(Err(remote))) => Err(anyhow!(
            "desktop_stop_failed: local={local}; remote={remote}"
        )),
    }
}

pub(super) fn poll_desktop_engine_tasks(
    shared: &Arc<Mutex<SharedRuntimeState>>,
) -> Vec<(i64, Result<EngineEvent, EngineFailure>)> {
    let completed = {
        let state = shared.lock();
        state
            .desktop_engine_tasks
            .iter()
            .filter_map(|(peer_device_id, task)| match task.poll_outcome() {
                Ok(Some(event)) => Some((*peer_device_id, Ok(event))),
                Ok(None) => None,
                Err(error) => Some((*peer_device_id, Err(error))),
            })
            .collect::<Vec<_>>()
    };
    if completed.is_empty() {
        return completed;
    }
    let mut completed = completed;
    let tasks = {
        let mut state = shared.lock();
        completed
            .iter()
            .filter_map(|(peer_device_id, _)| {
                state
                    .desktop_engine_tasks
                    .remove(peer_device_id)
                    .map(|task| (*peer_device_id, task))
            })
            .collect::<Vec<_>>()
    };
    for (peer_device_id, task) in tasks {
        if let Err(error) = task.join_supervisor() {
            if let Some((_, outcome)) = completed
                .iter_mut()
                .find(|(candidate, _)| *candidate == peer_device_id)
            {
                *outcome = Err(error);
            }
        }
    }
    completed
}

pub(super) async fn stop_all_desktop_sessions(
    shared: &Arc<Mutex<SharedRuntimeState>>,
    reason: &str,
) -> Vec<(i64, String)> {
    let tasks = std::mem::take(&mut shared.lock().desktop_engine_tasks);
    let mut failures = Vec::new();
    for (peer_device_id, task) in tasks {
        let reason = reason.to_owned();
        match tokio::task::spawn_blocking(move || task.stop(reason)).await {
            Ok(Ok(())) => {}
            Ok(Err(error)) => failures.push((peer_device_id, error.to_string())),
            Err(error) => {
                failures.push((peer_device_id, format!("desktop_stop_task_failed: {error}")))
            }
        }
    }
    failures
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
    _session_secret: String,
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
        assert_eq!(error.code, "desktop_session_helper_not_found");
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

    #[test]
    fn peer_close_after_remote_stop_is_not_reported_as_cleanup_failure() {
        let expected = EngineFailure::new(
            "clipboard_read_failed",
            EngineStage::Streaming,
            "transport I/O failed: 远程主机强迫关闭了一个现有的连接。 (os error 10054)",
        );
        assert!(is_expected_peer_close(&expected));

        let unrelated = EngineFailure::new(
            "capture_frame_failed",
            EngineStage::Streaming,
            "DXGI access denied",
        );
        assert!(!is_expected_peer_close(&unrelated));
    }

    #[tokio::test]
    async fn active_desktop_requires_an_established_active_wgvpn() {
        let shared = Arc::new(Mutex::new(SharedRuntimeState::default()));
        let error = start_active_desktop_session(&shared, i64::MAX)
            .await
            .unwrap_err();
        assert_eq!(error.to_string(), "desktop_wgvpn_session_not_found");
        assert!(shared.lock().desktop_engine_tasks.is_empty());
        assert!(shared.lock().p2p_attempt_waiters.is_empty());
    }
}
