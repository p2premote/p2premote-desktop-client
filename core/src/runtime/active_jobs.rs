//! Active tunnel job orchestration, retry, cancellation, and peer notification.

use super::*;
use crate::config::MachineConfig;

#[path = "active_job_effects.rs"]
mod effects;
use effects::{ActiveJobEffects, JobReport, LiveActiveJob};

#[cfg(test)]
#[path = "active_job_tests.rs"]
mod execution_tests;

struct AttemptWaiterRegistration {
    shared: Arc<Mutex<SharedRuntimeState>>,
    attempt_id: String,
}

impl Drop for AttemptWaiterRegistration {
    fn drop(&mut self) {
        self.shared
            .lock()
            .p2p_attempt_waiters
            .remove(&self.attempt_id);
    }
}

pub(super) fn cancel_all_active_tunnel_jobs(shared: &Arc<Mutex<SharedRuntimeState>>) {
    let cancel_txs: Vec<watch::Sender<bool>> = {
        let state = shared.lock();
        state.active_tunnel_job_cancels.values().cloned().collect()
    };
    for tx in cancel_txs {
        let _ = tx.send(true);
    }
}

pub(super) async fn start_anonymous_active_tunnel_job(
    shared: &Arc<Mutex<SharedRuntimeState>>,
    connect_code: String,
    temporary_password: String,
) -> Option<Data> {
    let config = match load_config_or_err() {
        Ok(config) => config,
        Err(resp) => return Some(resp),
    };
    let device =
        match anonymous_connect(&config, connect_code.clone(), temporary_password.clone()).await {
            Ok(device) => device,
            Err(err) => {
                return Some(cmd_response(
                    false,
                    &err.to_string(),
                    Some(shared.lock().status.clone()),
                ));
            }
        };
    let device_json = serde_json::to_value(&device).unwrap_or(serde_json::Value::Null);
    let peer_device_id = device.device_id;
    let peer_device_uuid = device.device_uuid.clone();

    match start_active_tunnel_job(
        shared,
        peer_device_id,
        peer_device_uuid,
        Some(connect_code),
        Some(temporary_password),
        Vec::new(),
    ) {
        Some(Data::CommandResponse {
            ok,
            message,
            status,
            ..
        }) => Some(cmd_response_with_data(
            ok,
            &message,
            status,
            serde_json::json!({ "device": device_json }),
        )),
        _ => Some(cmd_response(
            false,
            "failed to start anonymous active tunnel job",
            Some(shared.lock().status.clone()),
        )),
    }
}

pub(super) fn start_active_tunnel_job(
    shared: &Arc<Mutex<SharedRuntimeState>>,
    peer_device_id: i64,
    peer_device_uuid: String,
    connect_code: Option<String>,
    temporary_password: Option<String>,
    lan_cidrs: Vec<String>,
) -> Option<Data> {
    launch_active_tunnel_job(
        shared,
        peer_device_id,
        peer_device_uuid,
        connect_code,
        temporary_password,
        lan_cidrs,
        ActiveTunnelJobOrigin::Manual,
    )
}

#[derive(Clone, Copy)]
enum ActiveTunnelJobOrigin {
    Manual,
    AutoKeep,
}

pub(super) fn start_auto_keep_tunnel_job(
    shared: &Arc<Mutex<SharedRuntimeState>>,
    peer_device_id: i64,
    peer_device_uuid: String,
    lan_cidrs: Vec<String>,
) -> Option<Data> {
    launch_active_tunnel_job(
        shared,
        peer_device_id,
        peer_device_uuid,
        None,
        None,
        lan_cidrs,
        ActiveTunnelJobOrigin::AutoKeep,
    )
}

fn launch_active_tunnel_job(
    shared: &Arc<Mutex<SharedRuntimeState>>,
    peer_device_id: i64,
    peer_device_uuid: String,
    connect_code: Option<String>,
    temporary_password: Option<String>,
    lan_cidrs: Vec<String>,
    origin: ActiveTunnelJobOrigin,
) -> Option<Data> {
    {
        let mut state = shared.lock();
        if state.shutdown_requested {
            return Some(cmd_response(
                false,
                "service session unavailable",
                Some(state.status.clone()),
            ));
        }
        if matches!(origin, ActiveTunnelJobOrigin::AutoKeep)
            && (!state.active_tunnel_job_cancels.is_empty()
                || !state.status.logged_in
                || !state.status.ws_connected
                || has_passive_tunnel(&state, peer_device_id)
                || !state.auto_keep_settings.iter().any(|setting| {
                    setting.target_device_id == peer_device_id && !setting.paused_remote_stop
                }))
        {
            return Some(cmd_response(
                false,
                "auto keep target is no longer ready",
                None,
            ));
        }
        let Some(ws_client) = state.ws_client.clone() else {
            return Some(cmd_response(
                false,
                "service websocket unavailable",
                Some(state.status.clone()),
            ));
        };
        if state.status.active_tunnel_jobs.iter().any(|job| {
            job.peer_device_id == peer_device_id
                && job.state == TunnelJobState::Succeeded
                && job.result.as_ref().is_some_and(|result| result.success)
        }) {
            return Some(cmd_response(
                false,
                "active tunnel already connected",
                Some(state.status.clone()),
            ));
        }
        if state
            .active_tunnel_job_cancels
            .contains_key(&peer_device_id)
        {
            return Some(cmd_response(
                true,
                "active tunnel job already running",
                Some(state.status.clone()),
            ));
        }
        let (cancel_tx, cancel_rx) = watch::channel(false);
        state
            .active_tunnel_job_cancels
            .insert(peer_device_id, cancel_tx);
        if matches!(origin, ActiveTunnelJobOrigin::AutoKeep) {
            note_auto_keep_job_started(&mut state, peer_device_id, now_ts());
            if let Some(tx) = &state.status_tx {
                let _ = tx.send(state.status.clone());
            }
        }
        drop(state);

        spawn_active_tunnel_job_task(
            shared.clone(),
            ws_client,
            cancel_rx,
            peer_device_id,
            peer_device_uuid,
            connect_code,
            temporary_password,
            lan_cidrs,
        );
    }

    Some(cmd_response(
        true,
        "active tunnel job started",
        Some(shared.lock().status.clone()),
    ))
}

pub(super) fn spawn_active_tunnel_job_task(
    shared: Arc<Mutex<SharedRuntimeState>>,
    ws_client: ServiceWsClient,
    cancel_rx: watch::Receiver<bool>,
    peer_device_id: i64,
    peer_device_uuid: String,
    connect_code: Option<String>,
    temporary_password: Option<String>,
    lan_cidrs: Vec<String>,
) {
    let shared_for_task = shared.clone();
    let target_uuid_for_task = peer_device_uuid.clone();
    tokio::spawn(async move {
        // Keep the slot until the task has finished native cleanup and reporting.
        // A cancellation request or terminal display status is not task completion.
        let _slot = ActiveTunnelJobSlot {
            shared: shared_for_task.clone(),
            peer_device_id,
            owner: shared_for_task
                .lock()
                .active_tunnel_job_cancels
                .get(&peer_device_id)
                .cloned(),
        };
        let mut config = match load_machine_config() {
            Ok(config) => config,
            Err(err) => {
                publish_active_tunnel_job_failed(
                    &shared_for_task,
                    peer_device_id,
                    target_uuid_for_task.clone(),
                    0,
                    err.into(),
                );
                return;
            }
        };
        let preferences = shared_for_task.lock().connection_preferences;
        config.prefer_ipv6 = preferences.prefer_ipv6;
        config.prefer_tcp = preferences.prefer_tcp;
        let client_job_id = build_punch_token(&format!(
            "job-{}-{}-{}",
            peer_device_id,
            now_ts(),
            rand::thread_rng().gen::<u32>()
        ));
        let opened = match open_active_p2p_job(
            &config,
            client_job_id.clone(),
            peer_device_id,
            target_uuid_for_task.clone(),
            connect_code,
            temporary_password,
        )
        .await
        {
            Ok(opened) => opened,
            Err(err) => {
                publish_active_tunnel_job_failed(
                    &shared_for_task,
                    peer_device_id,
                    target_uuid_for_task.clone(),
                    0,
                    err.into(),
                );
                return;
            }
        };
        if !lan_cidrs.is_empty() {
            tracing::info!(
                "[active-tunnel-job] LAN access requested with {} CIDR(s)",
                lan_cidrs.len()
            );
        }

        let backend = Arc::new(LiveActiveJob {
            shared: shared_for_task.clone(),
            ws: ws_client,
            config: config.clone(),
            opened,
            peer: peer_device_id,
            client_job_id,
        });
        run_active_job(
            shared_for_task,
            cancel_rx,
            peer_device_id,
            target_uuid_for_task,
            config,
            backend,
        )
        .await;
    });
}

async fn run_active_job(
    shared_for_task: Arc<Mutex<SharedRuntimeState>>,
    mut cancel_rx: watch::Receiver<bool>,
    peer_device_id: i64,
    target_uuid_for_task: String,
    config: MachineConfig,
    backend: Arc<dyn ActiveJobEffects>,
) {
    let mut last_message = String::new();
    let mut last_code = ErrorCode::RuntimeExecutionUnclassified;
    let mut last_attempt = 0;
    let mut last_attempt_id = String::new();
    // Punch diagnostics for the /api/v1/p2p/end report: last failure's
    // NAT/network from the library diagnostic, plus task elapsed time.
    let started_at = std::time::Instant::now();
    let mut last_source_nat = String::new();
    let mut last_target_nat = String::new();
    let mut last_network = String::new();
    let mut attempted_networks = Vec::new();
    for attempt in 1..=ACTIVE_TUNNEL_JOB_MAX_ATTEMPTS {
        last_attempt = attempt;
        if *cancel_rx.borrow() {
            publish_active_tunnel_job_cancelled(
                &shared_for_task,
                peer_device_id,
                target_uuid_for_task.clone(),
            );
            let _ = backend
                .report(JobReport {
                    attempt_id: (&last_attempt_id).to_string(),
                    success: false,
                    source_nat: last_source_nat.clone(),
                    target_nat: last_target_nat.clone(),
                    network: last_network.clone(),
                    elapsed_secs: started_at.elapsed().as_secs(),
                    error_code: "connection.execution.user_cancelled".to_string(),
                    message: "connection.execution.user_cancelled".to_string(),
                })
                .await;
            return;
        }

        update_active_tunnel_job_status(
            &shared_for_task,
            TunnelJobStatus {
                error_code: String::new(),
                is_active: true,
                tcp_retry_recommended: false,
                symmetric_nat_help_recommended: false,
                peer_device_id,
                peer_device_uuid: target_uuid_for_task.clone(),
                state: TunnelJobState::Running,
                attempt,
                max_attempts: ACTIVE_TUNNEL_JOB_MAX_ATTEMPTS,
                message: localized_message(
                    current_locale(&shared_for_task).as_deref(),
                    "tunnel.job.attempt_total_secs",
                    &[("secs", &ACTIVE_TUNNEL_JOB_ATTEMPT_TOTAL_SECS.to_string())],
                ),
                result: None,
                updated_at: now_ts(),
            },
        );

        let attempt_id = build_punch_token(&format!(
            "attempt-{}-{}-{}",
            peer_device_id,
            attempt,
            now_ts()
        ));
        last_attempt_id = attempt_id.clone();
        let punch_token = build_punch_token(&format!(
            "{}-{}-{}",
            config.device_id.unwrap_or_default(),
            peer_device_id,
            now_ts()
        ));
        let attempt_backend = backend.clone();
        let id_for_task = attempt_id.clone();
        let mut attempt_handle = tokio::spawn(async move {
            attempt_backend
                .attempt(attempt, id_for_task, punch_token)
                .await
        });

        let attempt_result = tokio::select! {
            result = &mut attempt_handle => Some(match result {
                Ok(result) => result,
                Err(err) => Err(anyhow!("active attempt task failed: {}", err)),
            }),
            _ = tokio::time::sleep(Duration::from_secs(ACTIVE_TUNNEL_JOB_ATTEMPT_TOTAL_SECS)) => {
                backend.cancel(attempt_id.clone(), "attempt_deadline_exceeded").await;
                    let _ = attempt_handle.await;
                let _ = backend.stop().await;
                last_code = ErrorCode::ConnectionExecutionDeadlineExceeded;
                last_message = localized_message(
                    current_locale(&shared_for_task).as_deref(),
                    "tunnel.job.attempt_progress_secs",
                    &[
                        ("attempt", &attempt.to_string()),
                        ("secs", &ACTIVE_TUNNEL_JOB_ATTEMPT_TOTAL_SECS.to_string()),
                    ],
                );
                None
            }
            _ = cancel_rx.changed() => {
                if *cancel_rx.borrow() {
                    backend.cancel(attempt_id.clone(), "connection.execution.user_cancelled").await;
                    let _ = attempt_handle.await;
                    let _ = backend.stop().await;
                    publish_active_tunnel_job_cancelled(
                        &shared_for_task,
                        peer_device_id,
                        target_uuid_for_task.clone(),
                    );
                    let _ = backend.report(JobReport { attempt_id: (&attempt_id).to_string(), success: false, source_nat: last_source_nat.clone(), target_nat: last_target_nat.clone(), network: last_network.clone(), elapsed_secs: started_at.elapsed().as_secs(), error_code: "connection.execution.user_cancelled".to_string(), message: "connection.execution.user_cancelled".to_string() }).await;
                    return;
                }
                None
            }
        };

        match attempt_result {
            Some(Ok(result)) if result.success => {
                let source_nat_type = result.source_nat_type.clone();
                let target_nat_type = result.target_nat_type.clone();
                let success_network = result.network.clone();
                let result_for_status = result;
                if *cancel_rx.borrow() {
                    backend.stop().await;
                    publish_active_tunnel_job_cancelled(
                        &shared_for_task,
                        peer_device_id,
                        target_uuid_for_task.clone(),
                    );
                    let _ = backend
                        .report(JobReport {
                            attempt_id: (&attempt_id).to_string(),
                            success: false,
                            source_nat: source_nat_type,
                            target_nat: target_nat_type,
                            network: success_network.clone(),
                            elapsed_secs: started_at.elapsed().as_secs(),
                            error_code: "connection.execution.user_cancelled".to_string(),
                            message: "connection.execution.user_cancelled".to_string(),
                        })
                        .await;
                    return;
                }
                update_active_tunnel_job_status(
                    &shared_for_task,
                    TunnelJobStatus {
                        error_code: String::new(),
                        is_active: true,
                        tcp_retry_recommended: false,
                        symmetric_nat_help_recommended: false,
                        peer_device_id,
                        peer_device_uuid: target_uuid_for_task.clone(),
                        state: TunnelJobState::Succeeded,
                        attempt,
                        max_attempts: ACTIVE_TUNNEL_JOB_MAX_ATTEMPTS,
                        message: localized_message(
                            current_locale(&shared_for_task).as_deref(),
                            "tunnel.job.auto_established",
                            &[],
                        ),
                        result: Some(result_for_status),
                        updated_at: now_ts(),
                    },
                );
                backend.established();
                let _ = backend
                    .report(JobReport {
                        attempt_id: (&attempt_id).to_string(),
                        success: true,
                        source_nat: source_nat_type,
                        target_nat: target_nat_type,
                        network: success_network,
                        elapsed_secs: started_at.elapsed().as_secs(),
                        error_code: String::new(),
                        message: String::new(),
                    })
                    .await;
                return;
            }
            Some(Ok(result)) => {
                last_code = ErrorCode::TraversalPunchFailed;
                last_message = result.message;
            }
            Some(Err(err)) => {
                tracing::error!(
                        "[wgvpn] active job attempt failed: peer_device_id={}, attempt={}/{}, error={:#}",
                        peer_device_id,
                        attempt,
                        ACTIVE_TUNNEL_JOB_MAX_ATTEMPTS,
                        err
                    );
                if let Some(failure) = err.downcast_ref::<crate::gonc_ffi::UdpTunnelFailure>() {
                    if !failure.result.local_nat_type.is_empty() {
                        last_source_nat = failure.result.local_nat_type.clone();
                    }
                    if !failure.result.remote_nat_type.is_empty() {
                        last_target_nat = failure.result.remote_nat_type.clone();
                    }
                    if !failure.result.network.is_empty() {
                        last_network = failure.result.network.clone();
                    }
                }
                last_code = code_of(&err);
                last_message = format!("{:#}", err);
            }
            None => {}
        }

        let report = crate::traversal::peek_plan(&attempt_id);
        for network in report.attempted_networks {
            if !attempted_networks.contains(&network) {
                attempted_networks.push(network);
            }
        }
        if !report.local_nat.is_empty() {
            last_source_nat = report.local_nat;
        }
        if !report.remote_nat.is_empty() {
            last_target_nat = report.remote_nat;
        }
        if !report.network.is_empty() {
            last_network = report.network;
        }
        if !retry_attempt(&last_code) {
            break;
        }

        if attempt < ACTIVE_TUNNEL_JOB_MAX_ATTEMPTS {
            update_active_tunnel_job_status(
                &shared_for_task,
                TunnelJobStatus {
                    error_code: last_code.as_str().to_owned(),
                    is_active: true,
                    tcp_retry_recommended: crate::traversal_policy::tcp_retry_recommended(
                        config.prefer_tcp,
                        &last_code,
                        &attempted_networks,
                    ),
                    symmetric_nat_help_recommended:
                        crate::traversal_policy::symmetric_nat_help_recommended(
                            last_code.as_str(),
                            &last_source_nat,
                            &last_target_nat,
                        ),
                    peer_device_id,
                    peer_device_uuid: target_uuid_for_task.clone(),
                    state: TunnelJobState::Waiting,
                    attempt,
                    max_attempts: ACTIVE_TUNNEL_JOB_MAX_ATTEMPTS,
                    message: localized_message(
                        current_locale(&shared_for_task).as_deref(),
                        "tunnel.job.attempt_failed_retry",
                        &[
                            ("reason", &last_message),
                            ("secs", &ACTIVE_TUNNEL_JOB_BACKOFF_SECS.to_string()),
                        ],
                    ),
                    result: None,
                    updated_at: now_ts(),
                },
            );
            tokio::select! {
                _ = cancel_rx.changed() => {
                    if *cancel_rx.borrow() {
                        publish_active_tunnel_job_cancelled(
                            &shared_for_task,
                            peer_device_id,
                            target_uuid_for_task.clone(),
                        );
                        let _ = backend.report(JobReport { attempt_id: (&attempt_id).to_string(), success: false, source_nat: last_source_nat.clone(), target_nat: last_target_nat.clone(), network: last_network.clone(), elapsed_secs: started_at.elapsed().as_secs(), error_code: "connection.execution.user_cancelled".to_string(), message: "connection.execution.user_cancelled".to_string() }).await;
                        return;
                    }
                }
                _ = tokio::time::sleep(Duration::from_secs(ACTIVE_TUNNEL_JOB_BACKOFF_SECS)) => {}
            }
        }
    }

    update_active_tunnel_job_status(
        &shared_for_task,
        TunnelJobStatus {
            error_code: last_code.as_str().to_owned(),
            is_active: true,
            tcp_retry_recommended: crate::traversal_policy::tcp_retry_recommended(
                config.prefer_tcp,
                &last_code,
                &attempted_networks,
            ),
            symmetric_nat_help_recommended: crate::traversal_policy::symmetric_nat_help_recommended(
                last_code.as_str(),
                &last_source_nat,
                &last_target_nat,
            ),
            peer_device_id,
            peer_device_uuid: target_uuid_for_task,
            state: TunnelJobState::Failed,
            attempt: last_attempt,
            max_attempts: ACTIVE_TUNNEL_JOB_MAX_ATTEMPTS,
            message: if last_message.is_empty() {
                localized_message(
                    current_locale(&shared_for_task).as_deref(),
                    "tunnel.job.ended_not_established",
                    &[],
                )
            } else {
                localized_message(
                    current_locale(&shared_for_task).as_deref(),
                    "tunnel.job.ended_not_established_with_reason",
                    &[("reason", &last_message)],
                )
            },
            result: None,
            updated_at: now_ts(),
        },
    );
    let error_code = last_code.as_str().to_string();
    let _ = backend
        .report(JobReport {
            attempt_id: (&last_attempt_id).to_string(),
            success: false,
            source_nat: last_source_nat,
            target_nat: last_target_nat,
            network: last_network,
            elapsed_secs: started_at.elapsed().as_secs(),
            error_code: error_code,
            message: last_message,
        })
        .await;
}

pub(super) async fn start_wgvpn_active_with_notify(
    shared: &Arc<Mutex<SharedRuntimeState>>,
    ws_client: &ServiceWsClient,
    config: &crate::config::MachineConfig,
    peer_device_id: i64,
    client_job_id: &str,
    opened: &ActiveP2POpenResult,
    attempt: u8,
    attempt_id: String,
    punch_token: String,
) -> Result<ActiveStartResult> {
    let (event_tx, mut event_rx) = mpsc::unbounded_channel();
    let source_device = {
        let mut state = shared.lock();
        state
            .p2p_attempt_waiters
            .insert(attempt_id.clone(), event_tx);
        state.status.current_device.clone()
    };

    let _waiter = AttemptWaiterRegistration {
        shared: shared.clone(),
        attempt_id: attempt_id.clone(),
    };
    let signal_offer = crate::punch_signal::local_offer();
    let start_message = P2PAttemptMessage::AttemptStart {
        protocol_version: 1,
        error_contract_version: crate::error::CONTRACT_VERSION,
        client_job_id: client_job_id.to_string(),
        attempt_id: attempt_id.clone(),
        attempt,
        max_attempts: ACTIVE_TUNNEL_JOB_MAX_ATTEMPTS,
        punch_token: punch_token.clone(),
        source_user_id: opened.source_user_id,
        source_username: opened.source_username.clone(),
        source_email: opened.source_email.clone(),
        source_device_name: source_device
            .as_ref()
            .map(|device| device.device_name.clone())
            .unwrap_or_default(),
        source_device_alias: source_device
            .and_then(|device| device.device_alias)
            .unwrap_or_default(),
        traversal_negotiation: Some(crate::traversal_policy::Negotiation {
            version: crate::traversal_policy::VERSION,
            preferences: crate::traversal_policy::Preferences {
                prefer_ipv6: config.prefer_ipv6,
                prefer_tcp: config.prefer_tcp,
            },
        }),
        signal_transport: Some(signal_offer.clone()),
    };
    if let Err(err) = send_p2p_attempt_message(
        ws_client,
        opened.connection_id.clone(),
        peer_device_id,
        opened.access_grant.clone(),
        start_message,
    )
    .await
    {
        return Err(crate::error::classified(
            err,
            ErrorCode::SignalSendRejected,
            "attempt notification failed",
        ));
    }

    let ready = tokio::time::timeout(Duration::from_secs(30), event_rx.recv()).await;
    let (rdp_port, approval_required, traversal_negotiation, signal_transport) = match ready {
        Ok(Some(P2PAttemptEvent::Ready {
            rdp_port,
            approval_required,
            traversal_negotiation,
            signal_transport,
        })) => (
            rdp_port,
            approval_required,
            traversal_negotiation,
            signal_transport,
        ),
        Ok(Some(P2PAttemptEvent::Failed {
            error_code,
            message,
        })) => {
            return Err(AppError::from_wire(&error_code, message).into());
        }
        Ok(Some(P2PAttemptEvent::Cancelled)) => {
            return Err(coded(
                ErrorCode::ConnectionExecutionPeerCancelled,
                "connection.execution.peer_cancelled",
            ));
        }
        _ => {
            let _ = send_p2p_attempt_message(
                ws_client,
                opened.connection_id.clone(),
                peer_device_id,
                opened.access_grant.clone(),
                P2PAttemptMessage::AttemptCancel {
                    protocol_version: 1,
                    attempt_id,
                    reason: "connection.peer_prepare.timeout".to_string(),
                },
            )
            .await;
            return Err(coded(
                ErrorCode::ConnectionPeerPrepareTimeout,
                "connection.peer_prepare.timeout",
            ));
        }
    };

    let use_ws_signal = signal_offer
        .validate_selection(signal_transport.as_ref(), traversal_negotiation.is_some())
        .map_err(|e| anyhow!(e.to_string()))?;
    if let Some(negotiation) = traversal_negotiation {
        negotiation.validate()?;
        if negotiation.preferences
            != (crate::traversal_policy::Preferences {
                prefer_ipv6: config.prefer_ipv6,
                prefer_tcp: config.prefer_tcp,
            })
        {
            return Err(coded(
                ErrorCode::TraversalPlanNegotiationMismatch,
                "traversal_preference_mismatch",
            ));
        }
        crate::traversal::register_with_signal(
            punch_token.clone(),
            attempt_id.clone(),
            opened.connection_id.clone(),
            peer_device_id,
            opened.access_grant.clone(),
            ws_client.clone(),
            true,
            negotiation,
            if use_ws_signal {
                Some(config.device_id.ok_or_else(|| {
                    coded(
                        ErrorCode::RuntimeDeviceNotRegistered,
                        "missing_signal_device_id",
                    )
                })?)
            } else {
                None
            },
        )?;
    }
    let blocking_config = config.clone();
    let runtime_handle = tokio::runtime::Handle::current();
    let mut start_handle = tokio::task::spawn_blocking(move || {
        runtime_handle.block_on(wgvpn_flow::start_active_wgvpn(
            &blocking_config,
            peer_device_id,
            punch_token,
            Vec::new(),
        ))
    });
    let mut approval_signal_received = false;
    let mut early_approval_result: Option<Result<()>> = None;
    let result = tokio::select! {
        result = &mut start_handle => match result {
            Ok(result) => result,
            Err(err) => Err(AppError::with_source(if err.is_cancelled() { ErrorCode::TraversalExecutionCancelled } else { ErrorCode::RuntimeExecutionPanicked }, "active wgvpn task failed", err).into()),
        },
        event = event_rx.recv() => match event {
            Some(P2PAttemptEvent::Failed { error_code, message }) => {
                let local_result = start_handle.await;
                let _ = wgvpn_flow::stop_wgvpn(config, peer_device_id).await;
                match local_result {
                    Ok(Err(error)) if code_of(&error) == ErrorCode::TraversalExecutionExhausted => Err(error),
                    _ => Err(AppError::from_wire(&error_code, message).into()),
                }
            },
            Some(P2PAttemptEvent::Cancelled) => {
                let _ = start_handle.await;
                let _ = wgvpn_flow::stop_wgvpn(config, peer_device_id).await;
                Err(coded(ErrorCode::ConnectionExecutionPeerCancelled, "connection.execution.peer_cancelled"))
            },
            Some(P2PAttemptEvent::ApprovalRequired) => {
                approval_signal_received = true;
                match start_handle.await {
                    Ok(result) => result,
                    Err(err) => Err(AppError::with_source(if err.is_cancelled() { ErrorCode::TraversalExecutionCancelled } else { ErrorCode::RuntimeExecutionPanicked }, "active wgvpn task failed", err).into()),
                }
            },
            Some(P2PAttemptEvent::ApprovalGranted) => {
                early_approval_result = Some(Ok(()));
                match start_handle.await {
                    Ok(result) => result,
                    Err(err) => Err(AppError::with_source(if err.is_cancelled() { ErrorCode::TraversalExecutionCancelled } else { ErrorCode::RuntimeExecutionPanicked }, "active wgvpn task failed", err).into()),
                }
            },
            Some(P2PAttemptEvent::ApprovalDenied) => {
                early_approval_result = Some(Err(coded(ErrorCode::ConnectionApprovalDenied, "connection.approval.denied")));
                match start_handle.await {
                    Ok(result) => result,
                    Err(err) => Err(AppError::with_source(if err.is_cancelled() { ErrorCode::TraversalExecutionCancelled } else { ErrorCode::RuntimeExecutionPanicked }, "active wgvpn task failed", err).into()),
                }
            },
            Some(P2PAttemptEvent::ApprovalTimeout) => {
                early_approval_result = Some(Err(coded(ErrorCode::ConnectionApprovalTimeout, "connection.approval.timeout")));
                match start_handle.await {
                    Ok(result) => result,
                    Err(err) => Err(AppError::with_source(if err.is_cancelled() { ErrorCode::TraversalExecutionCancelled } else { ErrorCode::RuntimeExecutionPanicked }, "active wgvpn task failed", err).into()),
                }
            },
            _ => match start_handle.await {
                Ok(result) => result,
                Err(err) => Err(AppError::with_source(if err.is_cancelled() { ErrorCode::TraversalExecutionCancelled } else { ErrorCode::RuntimeExecutionPanicked }, "active wgvpn task failed", err).into()),
            },
        }
    };
    let started = result?;
    if approval_required {
        if let Some(approval_result) = early_approval_result {
            approval_result?;
        } else {
            await_active_inbound_approval(&mut event_rx, approval_signal_received).await?;
        }
    }
    let target_remote_port = if rdp_port == 0 {
        opened.target_remote_access.port.max(opened.target_rdp_port)
    } else {
        rdp_port
    };
    let remote_protocol = if opened.target_remote_access.protocol.is_empty() {
        "rdp".to_string()
    } else {
        opened.target_remote_access.protocol.clone()
    };
    let remote_address = format!("{}:{}", started.peer_virtual_ip, target_remote_port);
    Ok(ActiveStartResult {
        success: started.success,
        rdp_address: if remote_protocol == "rdp" {
            remote_address.clone()
        } else {
            String::new()
        },
        remote_address,
        remote_protocol,
        network: started.network,
        source_nat_type: started.local_nat_type,
        target_nat_type: started.remote_nat_type,
        message: started.message,
        warning: started.warning,
    })
}

async fn await_active_inbound_approval(
    event_rx: &mut mpsc::UnboundedReceiver<P2PAttemptEvent>,
    approval_signal_received: bool,
) -> Result<()> {
    let result = tokio::time::timeout(Duration::from_secs(65), async {
        let mut required = approval_signal_received;
        loop {
            match event_rx.recv().await {
                Some(P2PAttemptEvent::ApprovalRequired) => required = true,
                Some(P2PAttemptEvent::ApprovalGranted) if required => return Ok(()),
                Some(P2PAttemptEvent::ApprovalDenied) => {
                    return Err(coded(
                        ErrorCode::ConnectionApprovalDenied,
                        "connection.approval.denied",
                    ))
                }
                Some(P2PAttemptEvent::ApprovalTimeout) => {
                    return Err(coded(
                        ErrorCode::ConnectionApprovalTimeout,
                        "connection.approval.timeout",
                    ))
                }
                Some(P2PAttemptEvent::Failed {
                    error_code,
                    message,
                }) => return Err(AppError::from_wire(&error_code, message).into()),
                Some(P2PAttemptEvent::Cancelled) => {
                    return Err(coded(
                        ErrorCode::ConnectionExecutionPeerCancelled,
                        "connection.execution.peer_cancelled",
                    ))
                }
                Some(_) => {}
                None => {
                    return Err(coded(
                        ErrorCode::SignalReceiveClosed,
                        "approval channel closed",
                    ))
                }
            }
        }
    })
    .await;
    result.map_err(|_| {
        coded(
            ErrorCode::ConnectionApprovalTimeout,
            "connection.approval.timeout",
        )
    })?
}

pub(super) fn publish_active_tunnel_job_failed(
    shared: &Arc<Mutex<SharedRuntimeState>>,
    peer_device_id: i64,
    peer_device_uuid: String,
    attempt: u8,
    error: anyhow::Error,
) {
    let error_code = code_of(&error).as_str().to_owned();
    let message = error.to_string();
    update_active_tunnel_job_status(
        shared,
        TunnelJobStatus {
            error_code: error_code.clone(),
            is_active: true,
            tcp_retry_recommended: false,
            symmetric_nat_help_recommended: false,
            peer_device_id,
            peer_device_uuid,
            state: TunnelJobState::Failed,
            attempt,
            max_attempts: ACTIVE_TUNNEL_JOB_MAX_ATTEMPTS,
            message,
            result: None,
            updated_at: now_ts(),
        },
    );
}

struct ActiveTunnelJobSlot {
    shared: Arc<Mutex<SharedRuntimeState>>,
    peer_device_id: i64,
    owner: Option<watch::Sender<bool>>,
}

impl Drop for ActiveTunnelJobSlot {
    fn drop(&mut self) {
        let mut state = self.shared.lock();
        if state
            .active_tunnel_job_cancels
            .get(&self.peer_device_id)
            .zip(self.owner.as_ref())
            .is_some_and(|(current, owner)| current.same_channel(owner))
        {
            state.active_tunnel_job_cancels.remove(&self.peer_device_id);
        }
    }
}

pub(super) fn publish_active_tunnel_job_cancelled(
    shared: &Arc<Mutex<SharedRuntimeState>>,
    peer_device_id: i64,
    peer_device_uuid: String,
) {
    let locale = current_locale(shared);
    update_active_tunnel_job_status(
        shared,
        TunnelJobStatus {
            error_code: crate::error::ErrorCode::ConnectionExecutionUserCancelled
                .as_str()
                .to_owned(),
            is_active: true,
            tcp_retry_recommended: false,
            symmetric_nat_help_recommended: false,
            peer_device_id,
            peer_device_uuid,
            state: TunnelJobState::Cancelled,
            attempt: 0,
            max_attempts: ACTIVE_TUNNEL_JOB_MAX_ATTEMPTS,
            message: localized_message(locale.as_deref(), "tunnel.job.cancelled", &[]),
            result: None,
            updated_at: now_ts(),
        },
    );
}

pub(super) fn stop_active_tunnel_job(
    shared: &Arc<Mutex<SharedRuntimeState>>,
    peer_device_id: i64,
) -> Option<Data> {
    let cancel_tx = shared
        .lock()
        .active_tunnel_job_cancels
        .get(&peer_device_id)
        .cloned();
    match cancel_tx {
        Some(tx) => {
            let _ = tx.send(true);
            Some(cmd_response(
                true,
                "active tunnel job cancellation requested",
                Some(shared.lock().status.clone()),
            ))
        }
        None => Some(cmd_response(
            false,
            "active tunnel job not found",
            Some(shared.lock().status.clone()),
        )),
    }
}

/// Complete the stop before acknowledging it; callers decide whether to start again.
pub(super) async fn stop_active_tunnel(
    shared: &Arc<Mutex<SharedRuntimeState>>,
    peer_device_id: i64,
) -> Option<Data> {
    let task = shared
        .lock()
        .active_tunnel_job_cancels
        .get(&peer_device_id)
        .cloned();
    if let Some(task) = task {
        let _ = task.send(true);
        task.closed().await;
    }
    let mut response = stop_wgvpn_job(shared, peer_device_id).await;
    if let Some(Data::CommandResponse {
        ok: true, status, ..
    }) = &mut response
    {
        clear_active_tunnel_job_status(shared, peer_device_id);
        *status = Some(shared.lock().status.clone());
    }
    response
}

#[cfg(test)]
mod lifecycle_tests {
    use super::*;

    #[test]
    fn auto_keep_rechecks_readiness_when_claiming_the_job_slot() {
        let shared = Arc::new(Mutex::new(SharedRuntimeState::default()));
        {
            let mut state = shared.lock();
            state.status.logged_in = true;
            state.status.ws_connected = true;
        }
        assert!(
            matches!(start_auto_keep_tunnel_job(&shared, 42, "peer".into(), Vec::new()),
            Some(Data::CommandResponse { ok: false, message, .. }) if message == "auto keep target is no longer ready")
        );
        assert!(shared.lock().active_tunnel_job_cancels.is_empty());
    }

    #[tokio::test]
    async fn stop_waits_for_task_completion_after_cancellation() {
        let (old_job, mut old_receiver) = watch::channel(false);
        let shared = Arc::new(Mutex::new(SharedRuntimeState::default()));
        {
            let mut state = shared.lock();
            state.status.logged_in = true;
            state.active_tunnel_job_cancels.insert(42, old_job);
        }
        let shared_for_stop = shared.clone();
        let stop = tokio::spawn(async move { stop_active_tunnel(&shared_for_stop, 42).await });
        tokio::time::timeout(Duration::from_secs(1), old_receiver.changed())
            .await
            .unwrap()
            .unwrap();
        assert!(*old_receiver.borrow());
        let slot = ActiveTunnelJobSlot {
            shared: shared.clone(),
            peer_device_id: 42,
            owner: shared.lock().active_tunnel_job_cancels.get(&42).cloned(),
        };
        publish_active_tunnel_job_cancelled(&shared, 42, "peer".into());
        tokio::task::yield_now().await;
        assert!(!stop.is_finished());
        assert!(shared.lock().active_tunnel_job_cancels.contains_key(&42));
        stop.abort();
        let _ = stop.await;
        drop(slot);
        assert!(!shared.lock().active_tunnel_job_cancels.contains_key(&42));
    }
}

// ============ wgvpn job 生命周期（仿 active tunnel job） ============
