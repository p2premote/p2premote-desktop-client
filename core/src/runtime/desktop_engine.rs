use super::*;
use p2premote_remote_engine_manager::{EngineManager, EngineProcess, EngineRole};
use p2premote_remote_engine_protocol::{EngineEvent, EngineFailure, SessionConfig};
use std::{
    path::PathBuf,
    sync::mpsc as std_mpsc,
    thread::{self, JoinHandle},
};
use uuid::Uuid;

const ENGINE_START_TIMEOUT: Duration = Duration::from_secs(30);
const ENGINE_STOP_TIMEOUT: Duration = Duration::from_secs(5);
const ENGINE_POLL_INTERVAL: Duration = Duration::from_millis(200);
const DESKTOP_PORT: u16 = 39090;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(super) enum DesktopEngineRole {
    Host,
    Controller,
}

pub(super) struct DesktopEngineTask {
    attempt_id: String,
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
                p2premote_remote_engine_protocol::EngineStage::Streaming,
                "desktop engine supervisor exited without a terminal event",
            )),
        }
    }

    pub(super) fn stop(mut self, reason: impl Into<String>) -> Result<(), EngineFailure> {
        self.stop_tx.send(reason.into()).map_err(|error| {
            EngineFailure::new(
                "engine_supervisor_closed",
                p2premote_remote_engine_protocol::EngineStage::Shutdown,
                format!("send stop to desktop engine supervisor: {error}"),
            )
        })?;
        if let Some(worker) = self.worker.take() {
            worker.join().map_err(|_| {
                EngineFailure::new(
                    "engine_supervisor_panicked",
                    p2premote_remote_engine_protocol::EngineStage::Shutdown,
                    "desktop engine supervisor panicked while stopping",
                )
            })?;
        }
        match self.outcome_rx.try_recv() {
            Ok(Ok(EngineEvent::Stopped { .. })) => Ok(()),
            Ok(Ok(other)) => Err(EngineFailure::new(
                "unexpected_engine_event",
                p2premote_remote_engine_protocol::EngineStage::Shutdown,
                format!("expected stopped event, received {other:?}"),
            )),
            Ok(Err(error)) => Err(error),
            Err(error) => Err(EngineFailure::new(
                "engine_stop_result_missing",
                p2premote_remote_engine_protocol::EngineStage::Shutdown,
                format!("engine supervisor returned no stop result: {error}"),
            )),
        }
    }

    fn join_supervisor(mut self) -> Result<(), EngineFailure> {
        if let Some(worker) = self.worker.take() {
            worker.join().map_err(|_| {
                EngineFailure::new(
                    "engine_supervisor_panicked",
                    p2premote_remote_engine_protocol::EngineStage::Shutdown,
                    "desktop engine supervisor panicked after reporting its terminal event",
                )
            })?;
        }
        Ok(())
    }
}

pub(super) fn start_desktop_engine(
    role: DesktopEngineRole,
    config: SessionConfig,
) -> Result<DesktopEngineTask, EngineFailure> {
    let attempt_id = config.attempt_id.clone();
    let executable = resolve_engine_executable()?;
    let manager = EngineManager::current_session(executable);
    let engine_role = privileged_engine_role(role)?;
    let mut process = manager.start(engine_role, config)?;
    match role {
        DesktopEngineRole::Host => {
            process.wait_for_host_ready(ENGINE_START_TIMEOUT)?;
        }
        DesktopEngineRole::Controller => {
            process.wait_for_streaming(ENGINE_START_TIMEOUT)?;
        }
    }
    Ok(spawn_supervisor(attempt_id, process))
}

fn spawn_supervisor(attempt_id: String, mut process: EngineProcess) -> DesktopEngineTask {
    let (stop_tx, stop_rx) = std_mpsc::channel::<String>();
    let (outcome_tx, outcome_rx) = std_mpsc::channel();
    let worker_attempt_id = attempt_id.clone();
    let worker = thread::spawn(move || loop {
        match stop_rx.try_recv() {
            Ok(reason) => {
                let result =
                    process
                        .stop(reason, ENGINE_STOP_TIMEOUT)
                        .map(|_| EngineEvent::Stopped {
                            attempt_id: worker_attempt_id.clone(),
                            reason: "stop_requested".into(),
                        });
                let _ = outcome_tx.send(result);
                return;
            }
            Err(std_mpsc::TryRecvError::Disconnected) => {
                let result = process
                    .stop("supervisor_owner_dropped", ENGINE_STOP_TIMEOUT)
                    .map(|_| EngineEvent::Stopped {
                        attempt_id: worker_attempt_id.clone(),
                        reason: "supervisor_owner_dropped".into(),
                    });
                let _ = outcome_tx.send(result);
                return;
            }
            Err(std_mpsc::TryRecvError::Empty) => {}
        }
        match process.poll_event(ENGINE_POLL_INTERVAL) {
            Ok(None) => {}
            Ok(Some(event @ EngineEvent::Stopped { .. }))
            | Ok(Some(event @ EngineEvent::Failed { .. })) => {
                let _ = outcome_tx.send(Ok(event));
                return;
            }
            Ok(Some(event)) => {
                let _ = outcome_tx.send(Err(EngineFailure::new(
                    "unexpected_engine_event",
                    p2premote_remote_engine_protocol::EngineStage::Streaming,
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
        stop_tx,
        outcome_rx,
        worker: Some(worker),
    }
}

fn privileged_engine_role(role: DesktopEngineRole) -> Result<EngineRole, EngineFailure> {
    #[cfg(any(windows, target_os = "linux"))]
    {
        Ok(match role {
            DesktopEngineRole::Host => EngineRole::HostUserSession,
            DesktopEngineRole::Controller => EngineRole::ControllerUserSession,
        })
    }
    #[cfg(not(any(windows, target_os = "linux")))]
    {
        let _ = role;
        Err(EngineFailure::new(
            "service_session_launcher_unsupported",
            p2premote_remote_engine_protocol::EngineStage::SessionDiscovery,
            "the privileged desktop-engine session launcher is not implemented on this platform",
        ))
    }
}

fn resolve_engine_executable() -> Result<PathBuf, EngineFailure> {
    if let Some(path) = std::env::var_os("P2PREMOTE_DESKTOP_ENGINE_PATH") {
        return validate_engine_path(PathBuf::from(path));
    }
    #[cfg(target_os = "linux")]
    let path = crate::config::linux_resources_dir().join("p2premote-desktop-engine");
    #[cfg(windows)]
    let path = std::env::current_exe()
        .ok()
        .and_then(|path| {
            path.parent()
                .map(|parent| parent.join("p2premote-desktop-engine.exe"))
        })
        .ok_or_else(|| {
            EngineFailure::new(
                "engine_path_resolution_failed",
                p2premote_remote_engine_protocol::EngineStage::Configuration,
                "cannot resolve the service executable directory",
            )
        })?;
    #[cfg(target_os = "macos")]
    let path = crate::config::macos_resources_dir().join("p2premote-desktop-engine");
    #[cfg(not(any(windows, target_os = "linux", target_os = "macos")))]
    let path = PathBuf::from("p2premote-desktop-engine");
    validate_engine_path(path)
}

fn validate_engine_path(path: PathBuf) -> Result<PathBuf, EngineFailure> {
    if !path.is_file() {
        return Err(EngineFailure::new(
            "engine_not_found",
            p2premote_remote_engine_protocol::EngineStage::Configuration,
            format!("desktop engine executable not found: {}", path.display()),
        ));
    }
    Ok(path)
}

pub(super) async fn start_active_desktop_session(
    shared: &Arc<Mutex<SharedRuntimeState>>,
    peer_device_id: i64,
) -> Result<serde_json::Value> {
    let session = require_wgvpn_session(peer_device_id)?;
    let attempt_id = Uuid::new_v4().to_string();
    let reservation = reserve_desktop_start(shared, peer_device_id, &attempt_id)?;
    let (signal, ws_client) = {
        let state = shared.lock();
        let signal = state
            .desktop_signal_peers
            .get(&peer_device_id)
            .cloned()
            .or_else(|| {
                state
                    .passive_p2p_attempts
                    .get(&peer_device_id)
                    .map(|attempt| DesktopSignalPeer {
                        connection_id: attempt.connection_id.clone(),
                        access_grant: attempt.access_grant.clone(),
                    })
            })
            .ok_or_else(|| anyhow!("desktop_signal_context_missing"))?;
        let ws_client = state
            .ws_client
            .clone()
            .ok_or_else(|| anyhow!("desktop_signal_connection_missing"))?;
        (signal, ws_client)
    };
    let session_secret = format!("{}{}", Uuid::new_v4().simple(), Uuid::new_v4().simple());
    let (event_tx, mut event_rx) = mpsc::unbounded_channel();
    shared
        .lock()
        .p2p_attempt_waiters
        .insert(attempt_id.clone(), event_tx);
    let send_result = send_p2p_attempt_message(
        &ws_client,
        signal.connection_id.clone(),
        peer_device_id,
        signal.access_grant.clone(),
        P2PAttemptMessage::DesktopStart {
            protocol_version: 1,
            attempt_id: attempt_id.clone(),
            session_secret: session_secret.clone(),
            port: DESKTOP_PORT,
        },
    )
    .await;
    if let Err(error) = send_result {
        shared.lock().p2p_attempt_waiters.remove(&attempt_id);
        return Err(error.context("desktop_start_notify_failed"));
    }
    let ready = tokio::time::timeout(ENGINE_START_TIMEOUT, event_rx.recv()).await;
    shared.lock().p2p_attempt_waiters.remove(&attempt_id);
    match ready {
        Ok(Some(P2PAttemptEvent::DesktopHostReady)) => {}
        Ok(Some(P2PAttemptEvent::DesktopFailed {
            error_code,
            message,
        })) => return Err(anyhow!("{error_code}: {message}")),
        Ok(Some(other)) => return Err(anyhow!("unexpected_desktop_signal_event: {other:?}")),
        Ok(None) => return Err(anyhow!("desktop_signal_channel_closed")),
        Err(_) => return Err(anyhow!("desktop_host_ready_timeout")),
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
            let stop_result = send_p2p_attempt_message(
                &ws_client,
                signal.connection_id,
                peer_device_id,
                signal.access_grant,
                P2PAttemptMessage::DesktopStop {
                    protocol_version: 1,
                    attempt_id: attempt_id.clone(),
                    reason: "controller_start_failed".into(),
                },
            )
            .await;
            return match stop_result {
                Ok(()) => Err(anyhow!(controller_error.to_string())),
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

pub(super) async fn start_passive_desktop_host(
    shared: &Arc<Mutex<SharedRuntimeState>>,
    peer_device_id: i64,
    attempt_id: String,
    session_secret: String,
    port: u16,
) -> Result<()> {
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
    Ok(())
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
    let reason = reason.to_owned();
    tokio::task::spawn_blocking(move || task.stop(reason))
        .await
        .map_err(|error| anyhow!("desktop_stop_task_failed: {error}"))?
        .map_err(|error| anyhow!(error.to_string()))
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
    session_secret: String,
) -> SessionConfig {
    SessionConfig {
        attempt_id,
        peer_device_id,
        local_virtual_ip: local_virtual_ip.to_owned(),
        peer_virtual_ip: peer_virtual_ip.to_owned(),
        port,
        session_secret,
        frames_per_second: 30,
        bitrate_kbps: 4_000,
        decoder_threads: 2,
        display_id: None,
        linux_display: None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn missing_configured_engine_is_an_explicit_error() {
        let missing =
            std::env::temp_dir().join(format!("p2premote-missing-engine-{}", std::process::id()));
        let error = validate_engine_path(missing).unwrap_err();
        assert_eq!(error.code, "engine_not_found");
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
        assert!(shared.lock().desktop_engine_tasks.is_empty());
        assert!(shared.lock().p2p_attempt_waiters.is_empty());
    }
}
