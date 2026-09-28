//! Active tunnel job orchestration, retry, cancellation, and peer notification.

use super::*;

pub(super) fn cancel_all_active_tunnel_jobs(shared: &Arc<Mutex<SharedRuntimeState>>) {
    let cancel_txs: Vec<watch::Sender<bool>> = shared
        .lock()
        .active_tunnel_job_cancels
        .values()
        .cloned()
        .collect();
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
    let target_device_id = device.device_id;
    let target_device_uuid = device.device_uuid.clone();

    match start_active_tunnel_job(
        shared,
        target_device_id,
        target_device_uuid,
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
    target_device_id: i64,
    target_device_uuid: String,
    connect_code: Option<String>,
    temporary_password: Option<String>,
    lan_cidrs: Vec<String>,
) -> Option<Data> {
    {
        let mut state = shared.lock();
        let Some(ws_client) = state.ws_client.clone() else {
            return Some(cmd_response(
                false,
                "service websocket unavailable",
                Some(state.status.clone()),
            ));
        };
        if state.status.active_tunnel_jobs.iter().any(|job| {
            job.target_device_id == target_device_id
                && job.state == ActiveTunnelJobState::Succeeded
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
            .contains_key(&target_device_id)
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
            .insert(target_device_id, cancel_tx);
        drop(state);

        spawn_active_tunnel_job_task(
            shared.clone(),
            ws_client,
            cancel_rx,
            target_device_id,
            target_device_uuid,
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
    mut cancel_rx: watch::Receiver<bool>,
    target_device_id: i64,
    target_device_uuid: String,
    connect_code: Option<String>,
    temporary_password: Option<String>,
    lan_cidrs: Vec<String>,
) {
    let shared_for_task = shared.clone();
    let target_uuid_for_task = target_device_uuid.clone();
    tokio::spawn(async move {
        let mut last_message = String::new();
        let mut last_attempt = 0;
        let config = match load_machine_config() {
            Ok(config) => config,
            Err(err) => {
                publish_active_tunnel_job_failed(
                    &shared_for_task,
                    target_device_id,
                    target_uuid_for_task.clone(),
                    0,
                    err.to_string(),
                );
                shared_for_task
                    .lock()
                    .active_tunnel_job_cancels
                    .remove(&target_device_id);
                return;
            }
        };
        let client_job_id = build_punch_token(&format!(
            "job-{}-{}-{}",
            target_device_id,
            now_ts(),
            rand::thread_rng().gen::<u32>()
        ));
        let opened = match open_active_p2p_job(
            &config,
            client_job_id.clone(),
            target_device_id,
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
                    target_device_id,
                    target_uuid_for_task.clone(),
                    0,
                    err.to_string(),
                );
                shared_for_task
                    .lock()
                    .active_tunnel_job_cancels
                    .remove(&target_device_id);
                return;
            }
        };
        if !lan_cidrs.is_empty() {
            tracing::info!(
                "[active-tunnel-job] LAN access requested with {} CIDR(s)",
                lan_cidrs.len()
            );
        }

        for attempt in 1..=ACTIVE_TUNNEL_JOB_MAX_ATTEMPTS {
            last_attempt = attempt;
            if *cancel_rx.borrow() {
                publish_active_tunnel_job_cancelled(
                    &shared_for_task,
                    target_device_id,
                    target_uuid_for_task.clone(),
                );
                let _ = close_active_p2p_job(
                    &config,
                    &opened,
                    target_device_id,
                    false,
                    "user_cancelled".to_string(),
                    "user_cancelled".to_string(),
                )
                .await;
                return;
            }

            update_active_tunnel_job_status(
                &shared_for_task,
                ActiveTunnelJobStatus {
                    target_device_id,
                    target_device_uuid: target_uuid_for_task.clone(),
                    state: ActiveTunnelJobState::Running,
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
                target_device_id,
                attempt,
                now_ts()
            ));
            let punch_token = build_punch_token(&format!(
                "{}-{}-{}",
                config.device_id.unwrap_or_default(),
                target_device_id,
                now_ts()
            ));
            let attempt_shared = shared_for_task.clone();
            let attempt_ws = ws_client.clone();
            let attempt_config = config.clone();
            let attempt_client_job_id = client_job_id.clone();
            let attempt_opened = opened.clone();
            let attempt_id_for_task = attempt_id.clone();
            let mut attempt_handle = tokio::spawn(async move {
                start_wgvpn_active_with_notify(
                    &attempt_shared,
                    &attempt_ws,
                    &attempt_config,
                    target_device_id,
                    &attempt_client_job_id,
                    &attempt_opened,
                    attempt,
                    attempt_id_for_task,
                    punch_token,
                )
                .await
            });

            let attempt_result = tokio::select! {
                result = &mut attempt_handle => Some(match result {
                    Ok(result) => result,
                    Err(err) => Err(anyhow!("active attempt task failed: {}", err)),
                }),
                _ = tokio::time::sleep(Duration::from_secs(ACTIVE_TUNNEL_JOB_ATTEMPT_TOTAL_SECS)) => {
                    crate::traversal::cancel(&attempt_id);
                    let _ = send_p2p_attempt_message(
                        &ws_client,
                        opened.connection_id.clone(),
                        target_device_id,
                        opened.access_grant.clone(),
                        P2PAttemptMessage::AttemptCancel {
                            protocol_version: 1,
                            attempt_id: attempt_id.clone(),
                            reason: "attempt_deadline_exceeded".to_string(),
                        },
                    ).await;
                    let _ = attempt_handle.await;
                    let _ = wgvpn_flow::stop_wgvpn(&config, target_device_id).await;
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
                        crate::traversal::cancel(&attempt_id);
                        let _ = send_p2p_attempt_message(
                            &ws_client,
                            opened.connection_id.clone(),
                            target_device_id,
                            opened.access_grant.clone(),
                            P2PAttemptMessage::AttemptCancel {
                                protocol_version: 1,
                                attempt_id: attempt_id.clone(),
                                reason: "user_cancelled".to_string(),
                            },
                        ).await;
                        let _ = attempt_handle.await;
                        let _ = wgvpn_flow::stop_wgvpn(&config, target_device_id).await;
                        publish_active_tunnel_job_cancelled(
                            &shared_for_task,
                            target_device_id,
                            target_uuid_for_task.clone(),
                        );
                        let _ = close_active_p2p_job(&config, &opened, target_device_id, false, "user_cancelled".to_string(), "user_cancelled".to_string()).await;
                        return;
                    }
                    None
                }
            };

            match attempt_result {
                Some(Ok(result)) if result.success => {
                    let source_nat_type = result.source_nat_type.clone();
                    let target_nat_type = result.target_nat_type.clone();
                    let result_for_status = result;
                    let session_info = wgvpn_flow::snapshot_sessions()
                        .into_iter()
                        .find(|session| {
                            session.peer_device_id == target_device_id && session.is_active
                        })
                        .map(|session| (session.peer_virtual_ip, session.peer_health_port));
                    if *cancel_rx.borrow() {
                        publish_active_tunnel_job_cancelled(
                            &shared_for_task,
                            target_device_id,
                            target_uuid_for_task.clone(),
                        );
                        let _ = close_active_p2p_job(
                            &config,
                            &opened,
                            target_device_id,
                            false,
                            "user_cancelled".to_string(),
                            "user_cancelled".to_string(),
                        )
                        .await;
                        return;
                    }
                    update_active_tunnel_job_status(
                        &shared_for_task,
                        ActiveTunnelJobStatus {
                            target_device_id,
                            target_device_uuid: target_uuid_for_task.clone(),
                            state: ActiveTunnelJobState::Succeeded,
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
                    shared_for_task
                        .lock()
                        .wgvpn_health_runtime
                        .insert(target_device_id, WgvpnHealthRuntime::default());
                    refresh_wgvpn_sessions(&shared_for_task);
                    if let Some((peer_virtual_ip, peer_health_port)) = session_info {
                        if !peer_virtual_ip.is_empty() {
                            spawn_wgvpn_health_monitor(
                                shared_for_task.clone(),
                                target_device_id,
                                peer_virtual_ip,
                                peer_health_port,
                            );
                        } else {
                            mark_wgvpn_health_degraded(
                                shared_for_task.clone(),
                                target_device_id,
                                "missing_peer_virtual_ip",
                            );
                        }
                    } else {
                        mark_wgvpn_health_degraded(
                            shared_for_task.clone(),
                            target_device_id,
                            "missing_peer_virtual_ip",
                        );
                    }
                    shared_for_task
                        .lock()
                        .active_tunnel_job_cancels
                        .remove(&target_device_id);
                    let _ = crate::p2p::close_active_p2p_job_with_nat(
                        &config,
                        &opened,
                        target_device_id,
                        true,
                        source_nat_type,
                        target_nat_type,
                        String::new(),
                        String::new(),
                    )
                    .await;
                    return;
                }
                Some(Ok(result)) => {
                    last_message = result.message;
                }
                Some(Err(err)) => {
                    tracing::error!(
                        "[wgvpn] active job attempt failed: target_device_id={}, attempt={}/{}, error={:#}",
                        target_device_id,
                        attempt,
                        ACTIVE_TUNNEL_JOB_MAX_ATTEMPTS,
                        err
                    );
                    last_message = err.to_string();
                }
                None => {}
            }

            if !is_retryable_tunnel_error(&last_message) {
                break;
            }

            if attempt < ACTIVE_TUNNEL_JOB_MAX_ATTEMPTS {
                update_active_tunnel_job_status(
                    &shared_for_task,
                    ActiveTunnelJobStatus {
                        target_device_id,
                        target_device_uuid: target_uuid_for_task.clone(),
                        state: ActiveTunnelJobState::Waiting,
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
                                target_device_id,
                                target_uuid_for_task.clone(),
                            );
                            let _ = close_active_p2p_job(&config, &opened, target_device_id, false, "user_cancelled".to_string(), "user_cancelled".to_string()).await;
                            return;
                        }
                    }
                    _ = tokio::time::sleep(Duration::from_secs(ACTIVE_TUNNEL_JOB_BACKOFF_SECS)) => {}
                }
            }
        }

        update_active_tunnel_job_status(
            &shared_for_task,
            ActiveTunnelJobStatus {
                target_device_id,
                target_device_uuid: target_uuid_for_task,
                state: ActiveTunnelJobState::Failed,
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
        shared_for_task
            .lock()
            .active_tunnel_job_cancels
            .remove(&target_device_id);
        let error_code = classify_tunnel_error_code(&last_message).to_string();
        let _ = close_active_p2p_job(
            &config,
            &opened,
            target_device_id,
            false,
            error_code,
            last_message,
        )
        .await;
    });
}

pub(super) async fn start_wgvpn_active_with_notify(
    shared: &Arc<Mutex<SharedRuntimeState>>,
    ws_client: &ServiceWsClient,
    config: &crate::config::MachineConfig,
    target_device_id: i64,
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

    let start_message = P2PAttemptMessage::AttemptStart {
        protocol_version: 1,
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
            preferences: crate::traversal_policy::Preferences { prefer_ipv6: config.prefer_ipv6, prefer_tcp: config.prefer_tcp },
        }),
    };
    if let Err(err) = send_p2p_attempt_message(
        ws_client,
        opened.connection_id.clone(),
        target_device_id,
        opened.access_grant.clone(),
        start_message,
    )
    .await
    {
        shared.lock().p2p_attempt_waiters.remove(&attempt_id);
        return Err(anyhow!("notify_failed: {}", err));
    }

    let ready = tokio::time::timeout(Duration::from_secs(30), event_rx.recv()).await;
    let (rdp_port, approval_required, traversal_negotiation) = match ready {
        Ok(Some(P2PAttemptEvent::Ready {
            rdp_port,
            approval_required,
            traversal_negotiation,
        })) => (rdp_port, approval_required, traversal_negotiation),
        Ok(Some(P2PAttemptEvent::Failed {
            error_code,
            message,
        })) => {
            shared.lock().p2p_attempt_waiters.remove(&attempt_id);
            return Err(anyhow!("{}: {}", error_code, message));
        }
        Ok(Some(P2PAttemptEvent::Cancelled)) => {
            shared.lock().p2p_attempt_waiters.remove(&attempt_id);
            return Err(anyhow!("peer_cancelled"));
        }
        _ => {
            shared.lock().p2p_attempt_waiters.remove(&attempt_id);
            let _ = send_p2p_attempt_message(
                ws_client,
                opened.connection_id.clone(),
                target_device_id,
                opened.access_grant.clone(),
                P2PAttemptMessage::AttemptCancel {
                    protocol_version: 1,
                    attempt_id,
                    reason: "peer_prepare_timeout".to_string(),
                },
            )
            .await;
            return Err(anyhow!("peer_prepare_timeout"));
        }
    };

    if let Some(negotiation) = traversal_negotiation {
        negotiation.validate()?;
        if negotiation.preferences != (crate::traversal_policy::Preferences { prefer_ipv6: config.prefer_ipv6, prefer_tcp: config.prefer_tcp }) {
            return Err(anyhow!("traversal_preference_mismatch"));
        }
        crate::traversal::register(punch_token.clone(), attempt_id.clone(), opened.connection_id.clone(),
            target_device_id, opened.access_grant.clone(), ws_client.clone(), true, negotiation)?;
    }
    let blocking_config = config.clone();
    let runtime_handle = tokio::runtime::Handle::current();
    let mut start_handle = tokio::task::spawn_blocking(move || {
        runtime_handle.block_on(wgvpn_flow::start_active_wgvpn(
            &blocking_config,
            target_device_id,
            punch_token,
            Vec::new(),
        ))
    });
    let mut approval_signal_received = false;
    let mut early_approval_result: Option<Result<()>> = None;
    let result = tokio::select! {
        result = &mut start_handle => match result {
            Ok(result) => result,
            Err(err) => Err(anyhow!("active wgvpn task failed: {}", err)),
        },
        event = event_rx.recv() => match event {
            Some(P2PAttemptEvent::Failed { error_code, message }) => {
                let _ = start_handle.await;
                let _ = wgvpn_flow::stop_wgvpn(config, target_device_id).await;
                Err(anyhow!("{}: {}", error_code, message))
            },
            Some(P2PAttemptEvent::Cancelled) => {
                let _ = start_handle.await;
                let _ = wgvpn_flow::stop_wgvpn(config, target_device_id).await;
                Err(anyhow!("peer_cancelled"))
            },
            Some(P2PAttemptEvent::ApprovalRequired) => {
                approval_signal_received = true;
                match start_handle.await {
                    Ok(result) => result,
                    Err(err) => Err(anyhow!("active wgvpn task failed: {}", err)),
                }
            },
            Some(P2PAttemptEvent::ApprovalGranted) => {
                early_approval_result = Some(Ok(()));
                match start_handle.await {
                    Ok(result) => result,
                    Err(err) => Err(anyhow!("active wgvpn task failed: {}", err)),
                }
            },
            Some(P2PAttemptEvent::ApprovalDenied) => {
                early_approval_result = Some(Err(anyhow!("approval_denied")));
                match start_handle.await {
                    Ok(result) => result,
                    Err(err) => Err(anyhow!("active wgvpn task failed: {}", err)),
                }
            },
            Some(P2PAttemptEvent::ApprovalTimeout) => {
                early_approval_result = Some(Err(anyhow!("approval_timeout")));
                match start_handle.await {
                    Ok(result) => result,
                    Err(err) => Err(anyhow!("active wgvpn task failed: {}", err)),
                }
            },
            _ => match start_handle.await {
                Ok(result) => result,
                Err(err) => Err(anyhow!("active wgvpn task failed: {}", err)),
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
    shared.lock().p2p_attempt_waiters.remove(&attempt_id);
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
        reused: false,
        local_port: 0,
        rdp_address: if remote_protocol == "rdp" {
            remote_address.clone()
        } else {
            String::new()
        },
        remote_address,
        remote_protocol,
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
                Some(P2PAttemptEvent::ApprovalDenied) => return Err(anyhow!("approval_denied")),
                Some(P2PAttemptEvent::ApprovalTimeout) => return Err(anyhow!("approval_timeout")),
                Some(P2PAttemptEvent::Failed {
                    error_code,
                    message,
                }) => return Err(anyhow!("{}: {}", error_code, message)),
                Some(P2PAttemptEvent::Cancelled) => return Err(anyhow!("peer_cancelled")),
                Some(_) => {}
                None => return Err(anyhow!("approval_channel_closed")),
            }
        }
    })
    .await;
    result.map_err(|_| anyhow!("approval_timeout"))?
}

pub(super) fn publish_active_tunnel_job_failed(
    shared: &Arc<Mutex<SharedRuntimeState>>,
    target_device_id: i64,
    target_device_uuid: String,
    attempt: u8,
    message: String,
) {
    update_active_tunnel_job_status(
        shared,
        ActiveTunnelJobStatus {
            target_device_id,
            target_device_uuid,
            state: ActiveTunnelJobState::Failed,
            attempt,
            max_attempts: ACTIVE_TUNNEL_JOB_MAX_ATTEMPTS,
            message,
            result: None,
            updated_at: now_ts(),
        },
    );
}

pub(super) fn publish_active_tunnel_job_cancelled(
    shared: &Arc<Mutex<SharedRuntimeState>>,
    target_device_id: i64,
    target_device_uuid: String,
) {
    let locale = current_locale(shared);
    update_active_tunnel_job_status(
        shared,
        ActiveTunnelJobStatus {
            target_device_id,
            target_device_uuid,
            state: ActiveTunnelJobState::Cancelled,
            attempt: 0,
            max_attempts: ACTIVE_TUNNEL_JOB_MAX_ATTEMPTS,
            message: localized_message(locale.as_deref(), "tunnel.job.cancelled", &[]),
            result: None,
            updated_at: now_ts(),
        },
    );
    shared
        .lock()
        .active_tunnel_job_cancels
        .remove(&target_device_id);
}

pub(super) fn stop_active_tunnel_job(
    shared: &Arc<Mutex<SharedRuntimeState>>,
    target_device_id: i64,
) -> Option<Data> {
    let cancel_tx = shared
        .lock()
        .active_tunnel_job_cancels
        .get(&target_device_id)
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

// ============ wgvpn job 生命周期（仿 active tunnel job） ============
