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
            error_contract_version,
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
            signal_transport,
            ..
        } => {
            if let Err(error) = crate::error::require_contract(error_contract_version) {
                send_p2p_attempt_message(
                    ws_client,
                    connection_id,
                    source_device_id,
                    access_grant,
                    P2PAttemptMessage::AttemptFailed {
                        protocol_version: 1,
                        attempt_id,
                        stage: "compatibility".into(),
                        error: P2PAttemptErrorPayload::new(
                            code_of(&error).as_str(),
                            error.to_string(),
                        ),
                    },
                )
                .await?;
                return Ok(());
            }
            if let Some(negotiation) = &traversal_negotiation {
                negotiation.validate()?;
            }
            let signal_selection = match crate::punch_signal::local_offer()
                .select(signal_transport.as_ref(), traversal_negotiation.is_some())
            {
                Ok(selection) => selection,
                Err(error) => {
                    send_p2p_attempt_message(
                        ws_client,
                        connection_id,
                        source_device_id,
                        access_grant,
                        P2PAttemptMessage::AttemptFailed {
                            protocol_version: 1,
                            attempt_id,
                            stage: "signal_negotiation".into(),
                            error: P2PAttemptErrorPayload::new(
                                error.code().as_str(),
                                error.to_string(),
                            ),
                        },
                    )
                    .await?;
                    return Ok(());
                }
            };
            let use_ws_signal = signal_selection.as_ref().map(|s| s.selected.as_str())
                == Some(p2premote_punch::easyp2p::signal::WS_TRANSPORT);
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
                            "connection.approval.busy",
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
            if let Err(error) = disable_auto_keep_for_passive(shared, source_device_id).await {
                shared.lock().passive_p2p_attempts.remove(&source_device_id);
                return Err(error);
            }
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
                                "runtime.config.load_failed",
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
                crate::traversal::register_with_signal(
                    punch_token.clone(),
                    attempt_id.clone(),
                    connection_id.clone(),
                    source_device_id,
                    access_grant.clone(),
                    ws_client.clone(),
                    false,
                    negotiation.clone(),
                    if use_ws_signal {
                        Some(
                            config
                                .device_id
                                .ok_or_else(|| anyhow!("missing_signal_device_id"))?,
                        )
                    } else {
                        None
                    },
                )?;
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
                        error: P2PAttemptErrorPayload::new(
                            "connection.peer_prepare.rejected",
                            message,
                        ),
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
                    error_contract_version: crate::error::CONTRACT_VERSION,
                    attempt_id,
                    stage: "hole_punch_wait".to_string(),
                    rdp_enabled: is_rdp_enabled(),
                    rdp_port: get_rdp_port_from_registry(),
                    approval_required,
                    traversal_negotiation,
                    signal_transport: signal_selection,
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
            error_contract_version,
            attempt_id,
            rdp_port,
            approval_required,
            traversal_negotiation,
            signal_transport,
            ..
        } => {
            if let Some(waiter) = shared.lock().p2p_attempt_waiters.get(&attempt_id).cloned() {
                if let Err(error) = crate::error::require_contract(error_contract_version) {
                    let _ = waiter.send(P2PAttemptEvent::Failed {
                        error_code: code_of(&error).as_str().to_owned(),
                        message: error.to_string(),
                    });
                    return Ok(());
                }
                let _ = waiter.send(P2PAttemptEvent::Ready {
                    rdp_port,
                    approval_required,
                    traversal_negotiation,
                    signal_transport,
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
                crate::traversal::cancel(&attempt_id);
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
        P2PAttemptMessage::PunchSignal { frame } => {
            crate::traversal::deliver_signal(
                &connection_id,
                source_device_id,
                ws_client.generation(),
                frame,
            )?;
        }
    }
    Ok(())
}

pub(super) async fn send_p2p_attempt_message(
    ws_client: &ServiceWsClient,
    connection_id: String,
    peer_device_id: i64,
    access_grant: String,
    message: P2PAttemptMessage,
) -> Result<()> {
    let data = serde_json::to_string(&message).context("serialize p2p attempt message")?;
    let message_id = build_punch_token(&format!("msg-{}", now_ts()));
    ws_client
        .send_p2p_notify(
            connection_id,
            peer_device_id,
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
                Some(job) if job.state == TunnelJobState::Failed => {
                    let _ = send_p2p_attempt_message(
                        &ws_client,
                        connection_id,
                        source_device_id,
                        access_grant,
                        P2PAttemptMessage::AttemptFailed {
                            protocol_version: 1,
                            attempt_id: attempt_id.clone(),
                            stage: "passive_attempt".to_string(),
                            error: P2PAttemptErrorPayload::new(&job.error_code, job.message),
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
                Some(job) if job.state == TunnelJobState::Succeeded => return,
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

#[cfg(test)]
mod tests {
    use super::*;

    fn state() -> Arc<Mutex<SharedRuntimeState>> {
        Arc::new(Mutex::new(SharedRuntimeState::default()))
    }

    async fn dispatch(shared: &Arc<Mutex<SharedRuntimeState>>, wire: serde_json::Value) -> Result<()> {
        handle_p2p_notify(shared, &ServiceWsClient::new(), "test-connection".into(),
            987654321, "synthetic-grant".into(), wire.to_string()).await
    }

    fn ready() -> serde_json::Value {
        serde_json::json!({"type":"attempt_ready","protocol_version":1,
            "attempt_id":"handler-test-ready","stage":"ready","rdp_enabled":false,"rdp_port":3390})
    }

    #[tokio::test]
    async fn legacy_and_current_ready_reach_actual_waiter_without_native_or_network() {
        for version in [None, Some(0), Some(1)] {
            for approval in [false, true] {
                let shared = state();
                let (tx, mut rx) = mpsc::unbounded_channel();
                shared.lock().p2p_attempt_waiters.insert("handler-test-ready".into(), tx);
                let mut wire = ready();
                wire["approval_required"] = approval.into();
                if let Some(version) = version { wire["error_contract_version"] = version.into(); }
                dispatch(&shared, wire).await.unwrap();
                match rx.try_recv().unwrap() {
                    P2PAttemptEvent::Ready { rdp_port, approval_required, traversal_negotiation, signal_transport } => {
                        assert_eq!(rdp_port, 3390);
                        assert_eq!(approval_required, approval);
                        assert!(traversal_negotiation.is_none());
                        assert!(signal_transport.is_none());
                    }
                    event => panic!("unexpected {event:?}"),
                }
                assert!(rx.try_recv().is_err());
            }
        }
    }

    #[tokio::test]
    async fn unsupported_ready_fails_waiter_with_upgrade_code() {
        let shared = state();
        let (tx, mut rx) = mpsc::unbounded_channel();
        shared.lock().p2p_attempt_waiters.insert("handler-test-ready".into(), tx);
        let mut wire = ready();
        wire["error_contract_version"] = 2.into();
        dispatch(&shared, wire).await.unwrap();
        match rx.try_recv().unwrap() {
            P2PAttemptEvent::Failed { error_code, message } => {
                assert_eq!(error_code, "connection.compatibility.client_upgrade_required");
                assert!(!message.is_empty());
            }
            event => panic!("unexpected {event:?}"),
        }
        assert!(rx.try_recv().is_err());
    }

    #[tokio::test]
    async fn transitional_start_reaches_traversal_validation_before_any_side_effect() {
        for version in [None, Some(0), Some(1)] {
            let shared = state();
            let mut wire = serde_json::json!({"type":"attempt_start","protocol_version":1,
                "client_job_id":"j","attempt_id":"invalid-traversal-handler-test",
                "attempt":1,"max_attempts":3,"punch_token":"synthetic-token",
                "traversal_negotiation":{"version":1,"preferences":{}}});
            if let Some(version) = version { wire["error_contract_version"] = version.into(); }
            let error = dispatch(&shared, wire).await.unwrap_err();
            assert_eq!(code_of(&error), ErrorCode::TraversalCapabilityExchangeInvalidPayload);
            let state = shared.lock();
            assert!(state.passive_p2p_attempts.is_empty());
            assert!(state.wgvpn_job_cancels.is_empty());
            assert!(state.pending_inbound_approval.is_none());
        }
    }

    #[tokio::test]
    async fn approval_events_route_only_to_matching_attempt() {
        for kind in ["approval_required", "approval_granted", "approval_denied", "approval_timeout"] {
            let shared = state();
            let (tx, mut rx) = mpsc::unbounded_channel();
            shared.lock().p2p_attempt_waiters.insert("approval-handler-test".into(), tx);
            let mut wire = serde_json::json!({"type":kind,"protocol_version":1,
                "attempt_id":"other-attempt","expires_at":123});
            dispatch(&shared, wire.clone()).await.unwrap();
            assert!(rx.try_recv().is_err());
            wire["attempt_id"] = "approval-handler-test".into();
            dispatch(&shared, wire).await.unwrap();
            let event = rx.try_recv().unwrap();
            assert!(matches!((kind, event),
                ("approval_required", P2PAttemptEvent::ApprovalRequired)
                | ("approval_granted", P2PAttemptEvent::ApprovalGranted)
                | ("approval_denied", P2PAttemptEvent::ApprovalDenied)
                | ("approval_timeout", P2PAttemptEvent::ApprovalTimeout)));
            assert!(rx.try_recv().is_err());
        }
    }

    #[tokio::test]
    async fn legacy_and_canonical_failure_reach_actual_waiter() {
        for error in [
            serde_json::json!({"code":"hole_punch_wait_timeout","message":"旧端失败"}),
            serde_json::json!({"error_code":"traversal.execution.exhausted","message":"new failure"}),
        ] {
            let shared = state();
            let (tx, mut rx) = mpsc::unbounded_channel();
            shared.lock().p2p_attempt_waiters.insert("failure-handler-test".into(), tx);
            let wire = serde_json::json!({"type":"attempt_failed","protocol_version":1,
                "attempt_id":"failure-handler-test","stage":"passive_start","error":error});
            dispatch(&shared, wire.clone()).await.unwrap();
            match rx.try_recv().unwrap() {
                P2PAttemptEvent::Failed { error_code, message } => {
                    assert_eq!(error_code, "traversal.execution.exhausted");
                    assert!(message.contains(error["message"].as_str().unwrap()));
                }
                event => panic!("unexpected {event:?}"),
            }
            shared.lock().p2p_attempt_waiters.clear();
            dispatch(&shared, wire).await.unwrap();
        }
    }

    #[tokio::test]
    async fn stale_cancel_preserves_new_passive_attempt_and_notifies_matching_active_waiter() {
        for (connection, attempt) in [("different-connection", "cancel-handler-test"),
            ("test-connection", "new-attempt")] {
            let shared = state();
            shared.lock().passive_p2p_attempts.insert(987654321, PassiveP2PAttempt {
                connection_id: connection.into(), attempt_id: attempt.into(),
            });
            let (tx, mut rx) = mpsc::unbounded_channel();
            shared.lock().p2p_attempt_waiters.insert("cancel-handler-test".into(), tx);
            let wire = serde_json::json!({"type":"attempt_cancel","protocol_version":1,
                "attempt_id":"cancel-handler-test","reason":"user_cancelled"});
            dispatch(&shared, wire.clone()).await.unwrap();
            assert!(matches!(rx.try_recv().unwrap(), P2PAttemptEvent::Cancelled));
            assert_eq!(shared.lock().passive_p2p_attempts[&987654321].attempt_id, attempt);
            shared.lock().p2p_attempt_waiters.clear();
            dispatch(&shared, wire).await.unwrap();
            assert!(rx.try_recv().is_err());
        }
    }

    #[tokio::test]
    async fn missing_and_closed_waiters_are_harmless_but_invalid_payloads_are_rejected() {
        let shared = state();
        dispatch(&shared, ready()).await.unwrap();
        let (tx, rx) = mpsc::unbounded_channel();
        drop(rx);
        shared.lock().p2p_attempt_waiters.insert("handler-test-ready".into(), tx);
        dispatch(&shared, ready()).await.unwrap();
        for invalid in [
            serde_json::json!({"type":"unknown"}),
            serde_json::json!({"type":"attempt_ready","protocol_version":1}),
            serde_json::json!({"type":"attempt_failed","protocol_version":1,
                "attempt_id":"a","stage":"x","error":{"error_code":"INVALID","message":"bad"}}),
        ] {
            let error = dispatch(&shared, invalid).await.unwrap_err();
            assert!(error.to_string().contains("invalid p2p_notify business json"));
        }
    }
}
