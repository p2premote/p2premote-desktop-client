//! P2P attempt signaling and passive-side tunnel orchestration.

use super::*;

pub(super) async fn handle_p2p_start(
    shared: &Arc<Mutex<SharedRuntimeState>>,
    ws_client: &ServiceWsClient,
    source_device_id: i64,
    punch_token: String,
    peer_info: PassivePeerInfo,
) -> Result<()> {
    let legacy_same_account = shared
        .lock()
        .current_user_id
        .zip(Some(peer_info.source_user_id))
        .is_some_and(|(local_user_id, source_user_id)| local_user_id == source_user_id);
    if !legacy_same_account {
        // The legacy p2p_start envelope has no attempt_id/approval channel. Do
        // not silently create an unguarded cross-account tunnel; upgraded
        // peers must use attempt_start so the approval gate can be enforced.
        return Err(anyhow!(
            "legacy passive request requires the approval-capable attempt protocol"
        ));
    }
    shared
        .lock()
        .passive_peer_infos
        .insert(source_device_id, peer_info);
    let config = load_machine_config()?;
    // 旧主动端或异常掉线可能未送达 stop 通知。新的被动连接请求到达时，
    // 同设备的旧被动会话已不可能继续被复用，先清理后再使用新 token 建立。
    if wgvpn_flow::snapshot_sessions()
        .iter()
        .any(|session| session.peer_device_id == source_device_id && !session.is_active)
    {
        info!(
            "[ServiceRuntime] replacing stale passive wgvpn session: source_device_id={}",
            source_device_id
        );
        let _ = stop_wgvpn_job(shared, source_device_id).await;
    }
    let lan_cidrs = if config.wgvpn_lan_access_enabled {
        config.wgvpn_lan_cidrs.clone()
    } else {
        Vec::new()
    };
    // 新请求开始前丢弃同设备旧健康连接的代次，避免旧连接让新会话跳过首次连接看门狗。
    cancel_passive_health_grace(shared, source_device_id);
    {
        let mut state = shared.lock();
        state
            .passive_health_connection_generations
            .remove(&source_device_id);
        state
            .passive_health_watchdog_generations
            .remove(&source_device_id);
        state.wgvpn_health_runtime.remove(&source_device_id);
    }
    info!(
        "[ServiceRuntime] starting passive wgvpn: source_device_id={}, lan_cidrs={}",
        source_device_id,
        lan_cidrs.len()
    );
    if let Some(resp) = start_wgvpn_job(
        shared,
        source_device_id,
        punch_token.clone(),
        false,
        lan_cidrs,
        false,
        None,
    ) {
        if let Data::CommandResponse {
            ok: false, message, ..
        } = resp
        {
            return Err(anyhow!("failed to start passive wgvpn job: {}", message));
        }
    }
    let public_ip = get_public_network_info()
        .await
        .map(|info| info.ip)
        .unwrap_or_default();
    let public_port = i64::from(get_rdp_port_from_registry());
    ws_client
        .send_p2p_ready(punch_token, public_ip, public_port, source_device_id)
        .await
        .map_err(|err| anyhow!("failed to send p2p_ready: {}", err))?;
    update_status(shared, |s| {
        s.last_error = None;
    });
    Ok(())
}

pub(super) async fn handle_p2p_notify(
    shared: &Arc<Mutex<SharedRuntimeState>>,
    ws_client: &ServiceWsClient,
    connection_id: String,
    source_device_id: i64,
    access_grant: String,
    data: String,
) -> Result<()> {
    let message: P2PAttemptMessage =
        serde_json::from_str(&data).context("invalid p2p_notify business json")?;
    match message {
        P2PAttemptMessage::AttemptStart {
            client_job_id,
            attempt_id,
            attempt,
            max_attempts,
            punch_token,
            source_user_id,
            source_username,
            source_email,
            source_device_name,
            source_device_alias,
            traversal_negotiation,
            ..
        } => {
            if let Some(negotiation) = &traversal_negotiation { negotiation.validate()?; }
            info!(
                "[ServiceRuntime] passive attempt start: source_device_id={}, attempt={}/{}, attempt_id={}, source_user_id={}, source_username_present={}, source_email_present={}",
                source_device_id,
                attempt,
                max_attempts,
                attempt_id,
                source_user_id,
                !source_username.trim().is_empty(),
                !source_email.trim().is_empty(),
            );
            let approval_required = {
                let state = shared.lock();
                state
                    .current_user_id
                    .map(|current_user_id| current_user_id != source_user_id)
                    .unwrap_or(true)
            };
            if approval_required && shared.lock().pending_inbound_approval.is_some() {
                send_p2p_attempt_message(
                    ws_client,
                    connection_id,
                    source_device_id,
                    access_grant,
                    P2PAttemptMessage::AttemptFailed {
                        protocol_version: 1,
                        attempt_id,
                        stage: "passive_preflight".to_string(),
                        error: P2PAttemptErrorPayload::new(
                            "approval_busy",
                            "another inbound approval is already pending",
                        ),
                    },
                )
                .await?;
                return Ok(());
            }
            if wgvpn_flow::snapshot_sessions()
                .iter()
                .any(|session| session.peer_device_id == source_device_id && !session.is_active)
            {
                let _ = stop_wgvpn_job(shared, source_device_id).await;
            }
            shared.lock().passive_p2p_attempts.insert(
                source_device_id,
                PassiveP2PAttempt {
                    connection_id: connection_id.clone(),
                    attempt_id: attempt_id.clone(),
                },
            );
            {
                let mut state = shared.lock();
                let locale = state.status.locale.clone();
                let peer_info = state
                    .passive_peer_infos
                    .get(&source_device_id)
                    .cloned()
                    .unwrap_or_default();
                upsert_tunnel_lifecycle(
                    &mut state.status,
                    TunnelLifecycleStatus {
                        peer_device_id: source_device_id,
                        source_user_id,
                        source_username: source_username.clone(),
                        source_email: source_email.clone(),
                        peer_device_name: if source_device_name.trim().is_empty() {
                            peer_info.source_device_name
                        } else {
                            source_device_name.clone()
                        },
                        peer_device_alias: if source_device_alias.trim().is_empty() {
                            peer_info.source_device_alias
                        } else {
                            source_device_alias.clone()
                        },
                        peer_public_ip: peer_info.source_public_ip,
                        role: TunnelLifecycleRole::Passive,
                        state: TunnelLifecycleState::Connecting,
                        attempt,
                        max_attempts,
                        stage: Some("hole_punch_wait".to_string()),
                        virtual_ip: None,
                        peer_virtual_ip: None,
                        last_result: TunnelLastResult::None,
                        error_code: None,
                        message: Some(localized_message(
                            locale.as_deref(),
                            "tunnel.job.waiting_passive",
                            &[],
                        )),
                        health_failures: 0,
                        health_grace_deadline: None,
                        connected_at: None,
                        updated_at: now_ts(),
                    },
                );
            }
            if approval_required {
                let peer_info = shared
                    .lock()
                    .passive_peer_infos
                    .get(&source_device_id)
                    .cloned()
                    .unwrap_or_default();
                let requested_at = now_ts();
                let (decision_tx, _decision_rx) = watch::channel(PassiveApprovalDecision::Pending);
                let approval = PendingInboundApprovalRuntime {
                    status: crate::control::InboundApprovalStatus {
                        attempt_id: attempt_id.clone(),
                        source_user_id,
                        source_username: source_username.clone(),
                        source_email: source_email.clone(),
                        source_device_id,
                        source_device_name: if source_device_name.trim().is_empty() {
                            peer_info.source_device_name.clone()
                        } else {
                            source_device_name.clone()
                        },
                        source_device_alias: if source_device_alias.trim().is_empty() {
                            peer_info.source_device_alias.clone()
                        } else {
                            source_device_alias.clone()
                        },
                        requested_at,
                        expires_at: requested_at + 60,
                        // Filled with the actual CIDRs immediately before the job starts.
                        lan_cidrs: Vec::new(),
                    },
                    decision_tx,
                    connection_id: connection_id.clone(),
                    source_device_id,
                    access_grant: access_grant.clone(),
                    ws_client: ws_client.clone(),
                };
                let mut state = shared.lock();
                state.pending_inbound_approval = Some(approval);
                drop(state);
            }
            let config = match load_machine_config() {
                Ok(config) => config,
                Err(err) => {
                    shared.lock().passive_p2p_attempts.remove(&source_device_id);
                    if approval_required {
                        let mut state = shared.lock();
                        if state
                            .pending_inbound_approval
                            .as_ref()
                            .is_some_and(|pending| pending.status.attempt_id == attempt_id)
                        {
                            state.pending_inbound_approval = None;
                        }
                        drop(state);
                        refresh_pending_inbound_approvals(shared);
                    }
                    send_p2p_attempt_message(
                        ws_client,
                        connection_id,
                        source_device_id,
                        access_grant,
                        P2PAttemptMessage::AttemptFailed {
                            protocol_version: 1,
                            attempt_id: attempt_id.clone(),
                            stage: "passive_preflight".to_string(),
                            error: P2PAttemptErrorPayload::new(
                                "config_load_failed",
                                err.to_string(),
                            ),
                        },
                    )
                    .await?;
                    return Ok(());
                }
            };
            let lan_cidrs = if config.wgvpn_lan_access_enabled {
                config.wgvpn_lan_cidrs.clone()
            } else {
                Vec::new()
            };
            if approval_required {
                let mut state = shared.lock();
                if let Some(pending) = state
                    .pending_inbound_approval
                    .as_mut()
                    .filter(|pending| pending.status.attempt_id == attempt_id)
                {
                    pending.status.lan_cidrs = lan_cidrs.clone();
                }
                drop(state);
            }
            if let Some(negotiation) = &traversal_negotiation {
                crate::traversal::register(punch_token.clone(), attempt_id.clone(), connection_id.clone(),
                    source_device_id, access_grant.clone(), ws_client.clone(), false, negotiation.clone())?;
            }
            let start_result = start_wgvpn_job(
                shared,
                source_device_id,
                punch_token,
                false,
                lan_cidrs,
                approval_required,
                approval_required.then_some(attempt_id.clone()),
            );
            if let Some(Data::CommandResponse {
                ok: false, message, ..
            }) = start_result
            {
                shared.lock().passive_p2p_attempts.remove(&source_device_id);
                if approval_required {
                    let mut state = shared.lock();
                    if state
                        .pending_inbound_approval
                        .as_ref()
                        .is_some_and(|pending| pending.status.attempt_id == attempt_id)
                    {
                        state.pending_inbound_approval = None;
                    }
                    drop(state);
                    refresh_pending_inbound_approvals(shared);
                }
                send_p2p_attempt_message(
                    ws_client,
                    connection_id,
                    source_device_id,
                    access_grant,
                    P2PAttemptMessage::AttemptFailed {
                        protocol_version: 1,
                        attempt_id,
                        stage: "passive_preflight".to_string(),
                        error: P2PAttemptErrorPayload::new("passive_start_failed", message),
                    },
                )
                .await?;
                return Ok(());
            }
            let ready_attempt_id = attempt_id.clone();
            if let Err(err) = send_p2p_attempt_message(
                ws_client,
                connection_id.clone(),
                source_device_id,
                access_grant.clone(),
                P2PAttemptMessage::AttemptReady {
                    protocol_version: 1,
                    attempt_id,
                    stage: "hole_punch_wait".to_string(),
                    rdp_enabled: is_rdp_enabled(),
                    rdp_port: get_rdp_port_from_registry(),
                    approval_required,
                    traversal_negotiation,
                },
            )
            .await
            {
                shared.lock().passive_p2p_attempts.remove(&source_device_id);
                if approval_required {
                    let mut state = shared.lock();
                    if state
                        .pending_inbound_approval
                        .as_ref()
                        .is_some_and(|pending| pending.status.attempt_id == ready_attempt_id)
                    {
                        state.pending_inbound_approval = None;
                    }
                    drop(state);
                    refresh_pending_inbound_approvals(shared);
                }
                let _ = stop_wgvpn_job(shared, source_device_id).await;
                return Err(err);
            }
            spawn_passive_attempt_failure_reporter(
                shared.clone(),
                ws_client.clone(),
                connection_id,
                source_device_id,
                access_grant,
                client_job_id,
                ready_attempt_id,
            );
        }
        P2PAttemptMessage::AttemptReady {
            attempt_id,
            rdp_port,
            approval_required,
            traversal_negotiation,
            ..
        } => {
            if let Some(waiter) = shared.lock().p2p_attempt_waiters.get(&attempt_id).cloned() {
                let _ = waiter.send(P2PAttemptEvent::Ready {
                    rdp_port,
                    approval_required,
                    traversal_negotiation,
                });
            }
        }
        P2PAttemptMessage::ApprovalRequired { attempt_id, .. } => {
            if let Some(waiter) = shared.lock().p2p_attempt_waiters.get(&attempt_id).cloned() {
                let _ = waiter.send(P2PAttemptEvent::ApprovalRequired);
            }
        }
        P2PAttemptMessage::ApprovalGranted { attempt_id, .. } => {
            if let Some(waiter) = shared.lock().p2p_attempt_waiters.get(&attempt_id).cloned() {
                let _ = waiter.send(P2PAttemptEvent::ApprovalGranted);
            }
        }
        P2PAttemptMessage::ApprovalDenied { attempt_id, .. } => {
            if let Some(waiter) = shared.lock().p2p_attempt_waiters.get(&attempt_id).cloned() {
                let _ = waiter.send(P2PAttemptEvent::ApprovalDenied);
            }
        }
        P2PAttemptMessage::ApprovalTimeout { attempt_id, .. } => {
            if let Some(waiter) = shared.lock().p2p_attempt_waiters.get(&attempt_id).cloned() {
                let _ = waiter.send(P2PAttemptEvent::ApprovalTimeout);
            }
        }
        P2PAttemptMessage::AttemptFailed {
            attempt_id,
            stage,
            error,
            ..
        } => {
            tracing::error!(
                "[wgvpn] peer attempt failed: source_device_id={}, attempt_id={}, stage={}, error_code={}, error={}",
                source_device_id,
                attempt_id,
                stage,
                error.code,
                error.message
            );
            let locale = current_locale(shared);
            let message = error.localized_message(locale.as_deref());
            if let Some(waiter) = shared.lock().p2p_attempt_waiters.get(&attempt_id).cloned() {
                let _ = waiter.send(P2PAttemptEvent::Failed {
                    error_code: error.code,
                    message,
                });
            }
        }
        P2PAttemptMessage::AttemptCancel { attempt_id, .. } => {
            let is_current_passive_attempt = shared
                .lock()
                .passive_p2p_attempts
                .get(&source_device_id)
                .map(|current| {
                    current.connection_id == connection_id && current.attempt_id == attempt_id
                })
                .unwrap_or(false);
            if is_current_passive_attempt {
                crate::traversal::cancel(&attempt_id);
                {
                    let mut state = shared.lock();
                    state.passive_p2p_attempts.remove(&source_device_id);
                    if state
                        .pending_inbound_approval
                        .as_ref()
                        .is_some_and(|pending| pending.status.attempt_id == attempt_id)
                    {
                        state.pending_inbound_approval = None;
                    }
                }
                refresh_pending_inbound_approvals(shared);
                let _ = stop_wgvpn_job(shared, source_device_id).await;
            } else {
                tracing::debug!(
                    "[ServiceRuntime] ignored stale passive attempt cancel: source_device_id={}, attempt_id={}",
                    source_device_id,
                    attempt_id
                );
            }
            if let Some(waiter) = shared.lock().p2p_attempt_waiters.get(&attempt_id).cloned() {
                crate::traversal::cancel(&attempt_id);
                let _ = waiter.send(P2PAttemptEvent::Cancelled);
            }
        }
        P2PAttemptMessage::Traversal { attempt_id, frame } => {
            crate::traversal::deliver(&connection_id, source_device_id, &attempt_id, frame)?;
        }
    }
    Ok(())
}

pub(super) async fn send_p2p_attempt_message(
    ws_client: &ServiceWsClient,
    connection_id: String,
    target_device_id: i64,
    access_grant: String,
    message: P2PAttemptMessage,
) -> Result<()> {
    let data = serde_json::to_string(&message).context("serialize p2p attempt message")?;
    let message_id = build_punch_token(&format!("msg-{}", now_ts()));
    ws_client
        .send_p2p_notify(
            connection_id,
            target_device_id,
            message_id,
            access_grant,
            data,
        )
        .await
        .map_err(|err| anyhow!(err))
}

pub(super) fn spawn_passive_attempt_failure_reporter(
    shared: Arc<Mutex<SharedRuntimeState>>,
    ws_client: ServiceWsClient,
    connection_id: String,
    source_device_id: i64,
    access_grant: String,
    _client_job_id: String,
    attempt_id: String,
) {
    tokio::spawn(async move {
        loop {
            tokio::time::sleep(Duration::from_millis(250)).await;
            let status = {
                let state = shared.lock();
                let is_current = state
                    .passive_p2p_attempts
                    .get(&source_device_id)
                    .map(|current| {
                        current.connection_id == connection_id && current.attempt_id == attempt_id
                    })
                    .unwrap_or(false);
                if !is_current {
                    return;
                }
                state
                    .status
                    .wgvpn_jobs
                    .iter()
                    .find(|job| job.peer_device_id == source_device_id && !job.is_active)
                    .cloned()
            };
            match status {
                Some(job) if job.state == WgvpnJobState::Failed => {
                    let _ = send_p2p_attempt_message(
                        &ws_client,
                        connection_id,
                        source_device_id,
                        access_grant,
                        P2PAttemptMessage::AttemptFailed {
                            protocol_version: 1,
                            attempt_id: attempt_id.clone(),
                            stage: "passive_attempt".to_string(),
                            error: P2PAttemptErrorPayload::new(
                                classify_tunnel_error_code(&job.message),
                                job.message,
                            ),
                        },
                    )
                    .await;
                    {
                        let mut state = shared.lock();
                        state.passive_p2p_attempts.remove(&source_device_id);
                        if state
                            .pending_inbound_approval
                            .as_ref()
                            .is_some_and(|pending| pending.status.attempt_id == attempt_id)
                        {
                            state.pending_inbound_approval = None;
                        }
                    }
                    refresh_pending_inbound_approvals(&shared);
                    return;
                }
                Some(job) if job.state == WgvpnJobState::Succeeded => return,
                None => {
                    let removed = {
                        let mut state = shared.lock();
                        if state
                            .pending_inbound_approval
                            .as_ref()
                            .is_some_and(|pending| pending.status.attempt_id == attempt_id)
                        {
                            state.pending_inbound_approval = None;
                            true
                        } else {
                            false
                        }
                    };
                    if removed {
                        refresh_pending_inbound_approvals(&shared);
                    }
                    return;
                }
                _ => {}
            }
        }
    });
}
