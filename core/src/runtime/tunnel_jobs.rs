//! WGVPN job orchestration, retry, cancellation, and session projection.

use super::*;

/// 启动 wgvpn job（fire-and-forget）：去重 + 建 watch + spawn 重试任务。
pub(super) fn start_wgvpn_job(
    shared: &Arc<Mutex<SharedRuntimeState>>,
    peer_device_id: i64,
    token: String,
    is_active: bool,
    lan_cidrs: Vec<String>,
    approval_required: bool,
    approval_attempt_id: Option<String>,
) -> Option<Data> {
    if wgvpn_flow::snapshot_sessions()
        .iter()
        .any(|session| session.peer_device_id == peer_device_id)
    {
        return Some(cmd_response(
            false,
            "wgvpn tunnel already connected",
            Some(shared.lock().status.clone()),
        ));
    }
    let generation = NEXT_WGVPN_JOB_GENERATION.fetch_add(1, Ordering::Relaxed);
    {
        let mut state = shared.lock();
        if let Some(existing) = state.wgvpn_job_cancels.get(&peer_device_id) {
            if is_active {
                return Some(cmd_response(
                    true,
                    "wgvpn job already running",
                    Some(state.status.clone()),
                ));
            }
            // 被动端每次 p2p_start 都带新 token。旧 job 必须被替换，否则两端会在
            // 不同 MQTT exchange topic 上永久等待。
            let _ = existing.cancel_tx.send(true);
        }
        let (cancel_tx, cancel_rx) = watch::channel(false);
        state.wgvpn_job_cancels.insert(
            peer_device_id,
            WgvpnJobControl {
                generation,
                cancel_tx,
            },
        );
        drop(state);

        spawn_wgvpn_job_task(
            shared.clone(),
            cancel_rx,
            generation,
            peer_device_id,
            token,
            is_active,
            lan_cidrs,
            approval_required,
            approval_attempt_id,
        );
    }
    Some(cmd_response(
        true,
        "wgvpn job started",
        Some(shared.lock().status.clone()),
    ))
}
/// wgvpn job 重试任务：最多 WGVPN_JOB_MAX_ATTEMPTS 次，每次跑 start_active/passive_wgvpn，
/// select! 覆盖 attempt / 超时 / cancel。终态移除自己的 cancel_tx 并更新 status。
pub(super) fn spawn_wgvpn_job_task(
    shared: Arc<Mutex<SharedRuntimeState>>,
    mut cancel_rx: watch::Receiver<bool>,
    generation: u64,
    peer_device_id: i64,
    token: String,
    is_active: bool,
    lan_cidrs: Vec<String>,
    approval_required: bool,
    approval_attempt_id: Option<String>,
) {
    tokio::spawn(async move {
        // A passive attempt is coordinated by one p2p_notify attempt_id and must
        // run exactly once. Only the active tunnel job owns the cross-attempt retry loop.
        let max_attempts = if is_active { WGVPN_JOB_MAX_ATTEMPTS } else { 1 };
        for attempt in 1..=max_attempts {
            if !is_current_wgvpn_job(&shared, peer_device_id, generation) {
                return;
            }
            if *cancel_rx.borrow() {
                publish_wgvpn_job_cancelled(&shared, peer_device_id, is_active, generation);
                return;
            }

            update_wgvpn_job_status(
                &shared,
                WgvpnJobStatus {
                    peer_device_id,
                    is_active,
                    state: WgvpnJobState::Running,
                    attempt,
                    max_attempts,
                    message: localized_message(
                        current_locale(&shared).as_deref(),
                        "wgvpn.job.building",
                        &[
                            ("attempt", &attempt.to_string()),
                            ("max", &max_attempts.to_string()),
                        ],
                    ),
                    updated_at: now_ts(),
                },
                Some(generation),
            );

            let config = match load_machine_config() {
                Ok(config) => config,
                Err(err) => {
                    update_wgvpn_job_status(
                        &shared,
                        WgvpnJobStatus {
                            peer_device_id,
                            is_active,
                            state: WgvpnJobState::Failed,
                            attempt,
                            max_attempts,
                            message: localized_message(
                                current_locale(&shared).as_deref(),
                                "wgvpn.job.config_load_failed",
                                &[("reason", &err.to_string())],
                            ),
                            updated_at: now_ts(),
                        },
                        Some(generation),
                    );
                    break;
                }
            };

            // gonc FFI / WireGuard CLI 都是阻塞调用，必须放到 blocking pool，
            // 否则会饿死 service control pipe 的 Tokio worker。
            let attempt_config = config.clone();
            let attempt_token = token.clone();
            let attempt_lan_cidrs = lan_cidrs.clone();
            let runtime_handle = tokio::runtime::Handle::current();
            let mut attempt_handle = tokio::task::spawn_blocking(move || {
                if is_active {
                    runtime_handle.block_on(wgvpn_flow::start_active_wgvpn(
                        &attempt_config,
                        peer_device_id,
                        attempt_token,
                        attempt_lan_cidrs,
                    ))
                } else {
                    runtime_handle.block_on(wgvpn_flow::start_passive_wgvpn(
                        &attempt_config,
                        peer_device_id,
                        attempt_token,
                        attempt_lan_cidrs,
                    ))
                }
            });

            // select! 覆盖 attempt 完成 / 超时 / cancel 三种情况。
            // 关键：超时和 cancel 分支不立即 stop_wgvpn，而是先 await detached 的
            // attempt_handle 完成，再清理。这样彻底消除"attempt 在 stop 之后才 insert
            // session 导致幽灵会话"的竞态（N-H2）。spawn_blocking 不可取消，但 await
            // 它能保证 attempt 的所有副作用（含 insert session + persist）都已落地，
            // 随后的 stop_wgvpn 会把它们一并清理。
            let mut timed_out = false;
            let mut cancelled = false;
            let mut attempt_result = tokio::select! {
                result = &mut attempt_handle => Some(match result {
                    Ok(result) => result,
                    Err(err) => Err(anyhow::anyhow!("wgvpn attempt task failed: {}", err)),
                }),
                _ = tokio::time::sleep(Duration::from_secs(WGVPN_JOB_ATTEMPT_SECS)) => {
                    timed_out = true;
                    None
                }
                _ = cancel_rx.changed() => {
                    if *cancel_rx.borrow() {
                        cancelled = true;
                    }
                    None
                }
            };

            // 超时或取消：先等 detached attempt 完全结束，再清理它可能已建的资源。
            if timed_out || cancelled {
                let _ = attempt_handle.await;
                if !is_current_wgvpn_job(&shared, peer_device_id, generation) {
                    return;
                }
                let _ = wgvpn_flow::stop_wgvpn(&config, peer_device_id).await;
                if cancelled {
                    publish_wgvpn_job_cancelled(&shared, peer_device_id, is_active, generation);
                    return;
                }
                // timed_out：转为可重试失败，交给下面的 match 处理
                attempt_result = Some(Err(anyhow::anyhow!(
                    "{}",
                    localized_message(
                        current_locale(&shared).as_deref(),
                        "wgvpn.job.attempt_progress_secs",
                        &[
                            ("attempt", &attempt.to_string()),
                            ("secs", &WGVPN_JOB_ATTEMPT_SECS.to_string()),
                        ],
                    )
                )));
            }

            match attempt_result {
                Some(Ok(result)) if result.success => {
                    if !is_active && approval_required {
                        if let Err(err) = await_passive_inbound_approval(
                            &shared,
                            &config,
                            peer_device_id,
                            approval_attempt_id.as_deref(),
                            generation,
                            attempt,
                            max_attempts,
                        )
                        .await
                        {
                            if let Some(attempt_id) = approval_attempt_id.as_deref() {
                                let _ = wgvpn_flow::stop_wgvpn(&config, peer_device_id).await;
                                let mut state = shared.lock();
                                if state
                                    .pending_inbound_approval
                                    .as_ref()
                                    .is_some_and(|pending| pending.status.attempt_id == attempt_id)
                                {
                                    state.pending_inbound_approval = None;
                                }
                                state
                                    .passive_p2p_attempts
                                    .retain(|_, current| current.attempt_id != attempt_id);
                                drop(state);
                                refresh_pending_inbound_approvals(&shared);
                            }
                            update_wgvpn_job_status(
                                &shared,
                                WgvpnJobStatus {
                                    peer_device_id,
                                    is_active,
                                    state: WgvpnJobState::Failed,
                                    attempt,
                                    max_attempts,
                                    message: err.to_string(),
                                    updated_at: now_ts(),
                                },
                                Some(generation),
                            );
                            remove_wgvpn_job_if_current(&shared, peer_device_id, generation);
                            return;
                        }
                    }
                    let peer_virtual_ip = result.peer_virtual_ip.clone();
                    shared
                        .lock()
                        .wgvpn_health_runtime
                        .insert(peer_device_id, WgvpnHealthRuntime::default());
                    // 成功：更新 job 状态为 Succeeded，刷新 sessions 快照
                    update_wgvpn_job_status(
                        &shared,
                        WgvpnJobStatus {
                            peer_device_id,
                            is_active,
                            state: WgvpnJobState::Succeeded,
                            attempt,
                            max_attempts,
                            message: result.message,
                            updated_at: now_ts(),
                        },
                        Some(generation),
                    );
                    refresh_wgvpn_sessions(&shared);
                    if is_active && !peer_virtual_ip.is_empty() {
                        spawn_wgvpn_health_monitor(
                            shared.clone(),
                            peer_device_id,
                            peer_virtual_ip,
                            result.peer_health_port,
                        );
                    } else if !is_active {
                        spawn_passive_health_watchdog(shared.clone(), peer_device_id);
                    }
                    remove_wgvpn_job_if_current(&shared, peer_device_id, generation);
                    return;
                }
                Some(Err(err)) => {
                    let err_text = err.to_string();
                    tracing::error!(
                        "[wgvpn] job attempt failed: peer_device_id={}, role={}, attempt={}/{}, error={:#}",
                        peer_device_id,
                        if is_active { "active" } else { "passive" },
                        attempt,
                        max_attempts,
                        err
                    );
                    if is_non_retryable_wgvpn_error(&err_text) {
                        update_wgvpn_job_status(
                            &shared,
                            WgvpnJobStatus {
                                peer_device_id,
                                is_active,
                                state: WgvpnJobState::Failed,
                                attempt,
                                max_attempts,
                                message: err_text,
                                updated_at: now_ts(),
                            },
                            Some(generation),
                        );
                        remove_wgvpn_job_if_current(&shared, peer_device_id, generation);
                        return;
                    }
                    if attempt >= max_attempts {
                        update_wgvpn_job_status(
                            &shared,
                            WgvpnJobStatus {
                                peer_device_id,
                                is_active,
                                state: WgvpnJobState::Failed,
                                attempt,
                                max_attempts,
                                message: err_text,
                                updated_at: now_ts(),
                            },
                            Some(generation),
                        );
                        remove_wgvpn_job_if_current(&shared, peer_device_id, generation);
                        return;
                    }
                    update_wgvpn_job_status(
                        &shared,
                        WgvpnJobStatus {
                            peer_device_id,
                            is_active,
                            state: WgvpnJobState::Waiting,
                            attempt,
                            max_attempts,
                            message: localized_message(
                                current_locale(&shared).as_deref(),
                                "wgvpn.job.attempt_failed_retry",
                                &[
                                    ("attempt", &attempt.to_string()),
                                    ("reason", &err_text),
                                    ("secs", &WGVPN_JOB_BACKOFF_SECS.to_string()),
                                ],
                            ),
                            updated_at: now_ts(),
                        },
                        Some(generation),
                    );
                    // 退避（可被 cancel 打断）
                    tokio::select! {
                        _ = tokio::time::sleep(Duration::from_secs(WGVPN_JOB_BACKOFF_SECS)) => {}
                        _ = cancel_rx.changed() => {
                            if *cancel_rx.borrow() {
                                publish_wgvpn_job_cancelled(
                                    &shared,
                                    peer_device_id,
                                    is_active,
                                    generation,
                                );
                                return;
                            }
                        }
                    }
                }
                _ => {}
            }
        }

        // 重试耗尽
        update_wgvpn_job_status(
            &shared,
            WgvpnJobStatus {
                peer_device_id,
                is_active,
                state: WgvpnJobState::Failed,
                attempt: max_attempts,
                max_attempts,
                message: localized_message(
                    current_locale(&shared).as_deref(),
                    "wgvpn.job.all_attempts_failed",
                    &[("max", &max_attempts.to_string())],
                ),
                updated_at: now_ts(),
            },
            Some(generation),
        );
        remove_wgvpn_job_if_current(&shared, peer_device_id, generation);
    });
}

async fn await_passive_inbound_approval(
    shared: &Arc<Mutex<SharedRuntimeState>>,
    config: &crate::config::MachineConfig,
    peer_device_id: i64,
    attempt_id: Option<&str>,
    generation: u64,
    attempt: u8,
    max_attempts: u8,
) -> Result<()> {
    let attempt_id = attempt_id.ok_or_else(|| anyhow!("approval_request_missing"))?;
    let requested_at = now_ts();
    let expires_at = requested_at + 60;
    {
        let mut state = shared.lock();
        let pending = state
            .pending_inbound_approval
            .as_mut()
            .filter(|pending| pending.status.attempt_id == attempt_id)
            .ok_or_else(|| anyhow!("approval_request_missing"))?;
        pending.status.requested_at = requested_at;
        pending.status.expires_at = expires_at;
    }
    let context = {
        let state = shared.lock();
        state
            .pending_inbound_approval
            .as_ref()
            .filter(|pending| pending.status.attempt_id == attempt_id)
            .map(|pending| {
                (
                    pending.decision_tx.subscribe(),
                    pending.connection_id.clone(),
                    pending.source_device_id,
                    pending.access_grant.clone(),
                    pending.ws_client.clone(),
                )
            })
    }
    .ok_or_else(|| anyhow!("approval_request_missing"))?;

    wgvpn_flow::set_wgvpn_allowed(config, peer_device_id, false).await?;
    refresh_pending_inbound_approvals(shared);
    update_wgvpn_job_status(
        shared,
        WgvpnJobStatus {
            peer_device_id,
            is_active: false,
            state: WgvpnJobState::Waiting,
            attempt,
            max_attempts,
            message: localized_message(
                current_locale(shared).as_deref(),
                "tunnel.lifecycle.awaiting_approval",
                &[],
            ),
            updated_at: now_ts(),
        },
        Some(generation),
    );
    refresh_wgvpn_sessions(shared);
    send_p2p_attempt_message(
        &context.4,
        context.1.clone(),
        context.2,
        context.3.clone(),
        P2PAttemptMessage::ApprovalRequired {
            protocol_version: 1,
            attempt_id: attempt_id.to_string(),
            expires_at,
        },
    )
    .await?;

    let mut decision_rx = context.0;
    let decision = tokio::time::timeout(Duration::from_secs(60), async {
        let decision = loop {
            if *decision_rx.borrow() != PassiveApprovalDecision::Pending {
                break *decision_rx.borrow();
            }
            decision_rx.changed().await?;
        };
        Ok::<PassiveApprovalDecision, tokio::sync::watch::error::RecvError>(decision)
    })
    .await
    .unwrap_or(Ok(PassiveApprovalDecision::Pending))
    .unwrap_or(PassiveApprovalDecision::Pending);

    let result = match decision {
        PassiveApprovalDecision::Granted => {
            wgvpn_flow::set_wgvpn_allowed(config, peer_device_id, true).await?;
            send_p2p_attempt_message(
                &context.4,
                context.1.clone(),
                context.2,
                context.3.clone(),
                P2PAttemptMessage::ApprovalGranted {
                    protocol_version: 1,
                    attempt_id: attempt_id.to_string(),
                },
            )
            .await?;
            Ok(())
        }
        PassiveApprovalDecision::Denied => {
            let _ = send_p2p_attempt_message(
                &context.4,
                context.1.clone(),
                context.2,
                context.3.clone(),
                P2PAttemptMessage::ApprovalDenied {
                    protocol_version: 1,
                    attempt_id: attempt_id.to_string(),
                },
            )
            .await;
            let _ = wgvpn_flow::stop_wgvpn(config, peer_device_id).await;
            Err(anyhow!("approval_denied"))
        }
        PassiveApprovalDecision::Pending => {
            let _ = send_p2p_attempt_message(
                &context.4,
                context.1.clone(),
                context.2,
                context.3.clone(),
                P2PAttemptMessage::ApprovalTimeout {
                    protocol_version: 1,
                    attempt_id: attempt_id.to_string(),
                },
            )
            .await;
            let _ = wgvpn_flow::stop_wgvpn(config, peer_device_id).await;
            Err(anyhow!("approval_timeout"))
        }
    };
    {
        let mut state = shared.lock();
        if state
            .pending_inbound_approval
            .as_ref()
            .is_some_and(|pending| pending.status.attempt_id == attempt_id)
        {
            state.pending_inbound_approval = None;
        }
        state
            .passive_p2p_attempts
            .retain(|_, current| current.attempt_id != attempt_id);
    }
    refresh_pending_inbound_approvals(shared);
    result
}

/// 停止 wgvpn：发 cancel 信号 + 调 stop_wgvpn 清理隧道/peer。
pub(super) async fn stop_wgvpn_job(
    shared: &Arc<Mutex<SharedRuntimeState>>,
    peer_device_id: i64,
) -> Option<Data> {
    {
        let mut state = shared.lock();
        state.passive_p2p_attempts.remove(&peer_device_id);
    }
    let removed_pending = {
        let mut state = shared.lock();
        if state
            .pending_inbound_approval
            .as_ref()
            .is_some_and(|pending| pending.source_device_id == peer_device_id)
        {
            state.pending_inbound_approval = None;
            true
        } else {
            false
        }
    };
    if removed_pending {
        refresh_pending_inbound_approvals(shared);
    }
    let stopped_role = wgvpn_flow::snapshot_sessions()
        .into_iter()
        .find(|session| session.peer_device_id == peer_device_id)
        .map(|session| {
            if session.is_active {
                TunnelLifecycleRole::Active
            } else {
                TunnelLifecycleRole::Passive
            }
        })
        .unwrap_or(TunnelLifecycleRole::Active);
    stop_wgvpn_health_monitor(shared, peer_device_id);
    cancel_passive_health_grace(shared, peer_device_id);
    {
        let mut state = shared.lock();
        state
            .passive_health_connection_generations
            .remove(&peer_device_id);
        state
            .passive_health_watchdog_generations
            .remove(&peer_device_id);
    }
    // 被动端断开时，主动关闭 health server 上该 peer 的 TCP 连接，
    // 触发 TCP FIN 立即送达主动端（conn.next() → None → ConnectionClosed → 立即清理），
    // 避免主动端长时间无感知（只能等心跳超时 + 宽限期）。
    // 仅对被动隧道生效；主动隧道已由 stop_wgvpn 内的 notify_remote_tunnel_stop 处理。
    if stopped_role == TunnelLifecycleRole::Passive {
        if let Some(handle) = shared.lock().health_server_handle.as_ref() {
            handle.close_peer_connection(peer_device_id);
        }
    }
    let cancel_tx = shared
        .lock()
        .wgvpn_job_cancels
        .get(&peer_device_id)
        .map(|control| control.cancel_tx.clone());
    if let Some(tx) = cancel_tx {
        let _ = tx.send(true);
    }

    let config = match load_machine_config() {
        Ok(c) => c,
        Err(err) => {
            let msg = localized_message(
                current_locale(shared).as_deref(),
                "wgvpn.job.config_load_failed",
                &[("reason", &err.to_string())],
            );
            return Some(cmd_response(
                false,
                &msg,
                Some(shared.lock().status.clone()),
            ));
        }
    };

    match wgvpn_flow::stop_wgvpn(&config, peer_device_id).await {
        Ok(()) => {
            {
                let mut state = shared.lock();
                state
                    .status
                    .wgvpn_jobs
                    .retain(|j| j.peer_device_id != peer_device_id);
                state.wgvpn_health_runtime.remove(&peer_device_id);
                let locale = state.status.locale.clone();
                upsert_tunnel_lifecycle(
                    &mut state.status,
                    TunnelLifecycleStatus {
                        peer_device_id,
                        source_user_id: 0,
                        source_username: String::new(),
                        source_email: String::new(),
                        peer_device_name: String::new(),
                        peer_device_alias: String::new(),
                        peer_public_ip: String::new(),
                        role: stopped_role,
                        state: TunnelLifecycleState::NotEstablished,
                        attempt: 0,
                        max_attempts: ACTIVE_TUNNEL_JOB_MAX_ATTEMPTS,
                        stage: None,
                        virtual_ip: None,
                        peer_virtual_ip: None,
                        last_result: TunnelLastResult::UserDisconnected,
                        error_code: None,
                        message: Some(localized_message(
                            locale.as_deref(),
                            "tunnel.lifecycle.passive_disconnected",
                            &[],
                        )),
                        health_failures: 0,
                        health_grace_deadline: None,
                        connected_at: None,
                        updated_at: now_ts(),
                    },
                );
            }
            refresh_wgvpn_sessions(shared);
            Some(cmd_response(
                true,
                "wgvpn stopped",
                Some(shared.lock().status.clone()),
            ))
        }
        Err(err) => {
            warn!(
                "[wgvpn] cleanup failed: peer_device_id={}, error={:#}",
                peer_device_id, err
            );
            {
                let mut state = shared.lock();
                state
                    .status
                    .wgvpn_jobs
                    .retain(|j| j.peer_device_id != peer_device_id);
                state.wgvpn_health_runtime.remove(&peer_device_id);
                if let Some(lifecycle) = state
                    .status
                    .tunnel_lifecycles
                    .iter_mut()
                    .find(|item| item.peer_device_id == peer_device_id)
                {
                    lifecycle.state = TunnelLifecycleState::NotEstablished;
                    lifecycle.last_result = TunnelLastResult::AttemptFailed;
                    lifecycle.error_code = Some("cleanup_failed".to_string());
                    lifecycle.message = Some(err.to_string());
                    lifecycle.connected_at = None;
                    lifecycle.updated_at = now_ts();
                }
            }
            refresh_wgvpn_sessions(shared);
            let message = err.to_string();
            Some(cmd_response(
                false,
                &message,
                Some(shared.lock().status.clone()),
            ))
        }
    }
}

/// 取消所有 wgvpn job 并同步停止所有活跃会话（Logout 时调用）。
///
/// 仅发 cancel 信号不够：job task 可能正卡在不可取消的 spawn_blocking 里（最长 120s），
/// 期间 wg0 peer / gonc UDP tunnel 仍在传输。为满足"登出即断开"的安全预期，
/// 这里额外同步 stop 所有已知会话（基于 WGVPN_SESSIONS 快照），立即释放隧道资源。
/// job task 后续醒来时会发现 session 已被清理（stop_wgvpn 幂等）。
pub(super) async fn cancel_all_wgvpn_jobs(shared: &Arc<Mutex<SharedRuntimeState>>) {
    stop_all_wgvpn_health_monitors(shared);
    let cancel_txs: Vec<watch::Sender<bool>> = shared
        .lock()
        .wgvpn_job_cancels
        .values()
        .map(|control| control.cancel_tx.clone())
        .collect();
    for tx in cancel_txs {
        let _ = tx.send(true);
    }
    // 同步停止所有活跃会话，确保登出后隧道立即断开。
    if let Ok(config) = load_machine_config() {
        let peer_ids: Vec<i64> = wgvpn_flow::snapshot_sessions()
            .into_iter()
            .map(|s| s.target_device_id)
            .collect();
        for peer_id in peer_ids {
            if let Err(err) = wgvpn_flow::stop_wgvpn(&config, peer_id).await {
                warn!(
                    "[wgvpn] logout cleanup failed: peer_device_id={}, error={:#}",
                    peer_id, err
                );
            }
        }
    }
    {
        let mut state = shared.lock();
        state.status.wgvpn_jobs.clear();
        state.pending_inbound_approval = None;
    }
    refresh_pending_inbound_approvals(shared);
    refresh_wgvpn_sessions(shared);
}

pub(super) fn publish_wgvpn_job_cancelled(
    shared: &Arc<Mutex<SharedRuntimeState>>,
    peer_device_id: i64,
    is_active: bool,
    generation: u64,
) {
    let locale = current_locale(shared);
    update_wgvpn_job_status(
        shared,
        WgvpnJobStatus {
            peer_device_id,
            is_active,
            state: WgvpnJobState::Cancelled,
            attempt: 0,
            max_attempts: WGVPN_JOB_MAX_ATTEMPTS,
            message: localized_message(locale.as_deref(), "wgvpn.job.cancelled", &[]),
            updated_at: now_ts(),
        },
        Some(generation),
    );
    remove_wgvpn_job_if_current(shared, peer_device_id, generation);
}

pub(super) fn is_current_wgvpn_job(
    shared: &Arc<Mutex<SharedRuntimeState>>,
    peer_device_id: i64,
    generation: u64,
) -> bool {
    shared
        .lock()
        .wgvpn_job_cancels
        .get(&peer_device_id)
        .is_some_and(|control| control.generation == generation)
}

pub(super) fn remove_wgvpn_job_if_current(
    shared: &Arc<Mutex<SharedRuntimeState>>,
    peer_device_id: i64,
    generation: u64,
) {
    let mut state = shared.lock();
    if state
        .wgvpn_job_cancels
        .get(&peer_device_id)
        .is_some_and(|control| control.generation == generation)
    {
        state.wgvpn_job_cancels.remove(&peer_device_id);
    }
}

pub(super) fn update_wgvpn_job_status(
    shared: &Arc<Mutex<SharedRuntimeState>>,
    status: WgvpnJobStatus,
    generation: Option<u64>,
) -> bool {
    let mut state = shared.lock();
    let mut passive_popup = None;
    if let Some(generation) = generation {
        let is_current = state
            .wgvpn_job_cancels
            .get(&status.peer_device_id)
            .is_some_and(|control| control.generation == generation);
        if !is_current {
            return false;
        }
    }
    if let Some(existing) = state
        .status
        .wgvpn_jobs
        .iter_mut()
        .find(|j| j.peer_device_id == status.peer_device_id)
    {
        *existing = status.clone();
    } else {
        state.status.wgvpn_jobs.push(status.clone());
    }
    if !status.is_active {
        let (lifecycle_state, last_result, error_code) = match status.state {
            WgvpnJobState::Running | WgvpnJobState::Waiting => (
                TunnelLifecycleState::Connecting,
                TunnelLastResult::None,
                None,
            ),
            WgvpnJobState::Succeeded => (
                TunnelLifecycleState::Connected,
                TunnelLastResult::None,
                None,
            ),
            WgvpnJobState::Failed => (
                TunnelLifecycleState::NotEstablished,
                TunnelLastResult::AttemptFailed,
                Some(classify_tunnel_error_code(&status.message).to_string()),
            ),
            WgvpnJobState::Cancelled => (
                TunnelLifecycleState::NotEstablished,
                TunnelLastResult::Cancelled,
                Some("user_cancelled".to_string()),
            ),
        };
        let existing_lifecycle = state
            .status
            .tunnel_lifecycles
            .iter()
            .find(|item| {
                item.peer_device_id == status.peer_device_id
                    && item.role == TunnelLifecycleRole::Passive
            })
            .cloned();
        upsert_tunnel_lifecycle(
            &mut state.status,
            TunnelLifecycleStatus {
                peer_device_id: status.peer_device_id,
                source_user_id: existing_lifecycle
                    .as_ref()
                    .map(|item| item.source_user_id)
                    .unwrap_or(0),
                source_username: existing_lifecycle
                    .as_ref()
                    .map(|item| item.source_username.clone())
                    .unwrap_or_default(),
                source_email: existing_lifecycle
                    .as_ref()
                    .map(|item| item.source_email.clone())
                    .unwrap_or_default(),
                peer_device_name: existing_lifecycle
                    .as_ref()
                    .map(|item| item.peer_device_name.clone())
                    .unwrap_or_default(),
                peer_device_alias: existing_lifecycle
                    .as_ref()
                    .map(|item| item.peer_device_alias.clone())
                    .unwrap_or_default(),
                peer_public_ip: existing_lifecycle
                    .as_ref()
                    .map(|item| item.peer_public_ip.clone())
                    .unwrap_or_default(),
                role: TunnelLifecycleRole::Passive,
                state: lifecycle_state,
                attempt: status.attempt,
                max_attempts: status.max_attempts,
                stage: (lifecycle_state == TunnelLifecycleState::Connecting)
                    .then(|| "passive_preflight".to_string()),
                virtual_ip: None,
                peer_virtual_ip: None,
                last_result,
                error_code,
                message: Some(status.message.clone()),
                health_failures: 0,
                health_grace_deadline: None,
                connected_at: None,
                updated_at: status.updated_at,
            },
        );
        if status.state == WgvpnJobState::Succeeded {
            let username = state
                .status
                .tunnel_lifecycles
                .iter()
                .find(|item| {
                    item.peer_device_id == status.peer_device_id
                        && item.role == TunnelLifecycleRole::Passive
                })
                .map(|item| item.source_username.clone())
                .unwrap_or_default();
            let fallback_device_name = if state.status.locale.as_deref() == Some("en") {
                format!("Device #{}", status.peer_device_id)
            } else {
                format!("设备 #{}", status.peer_device_id)
            };
            let lifecycle_identity = state.status.tunnel_lifecycles.iter().find(|item| {
                item.peer_device_id == status.peer_device_id
                    && item.role == TunnelLifecycleRole::Passive
            });
            let device_name = lifecycle_identity
                .and_then(|item| {
                    let alias = item.peer_device_alias.trim();
                    if !alias.is_empty() {
                        Some(alias.to_string())
                    } else {
                        let name = item.peer_device_name.trim();
                        (!name.is_empty()).then(|| name.to_string())
                    }
                })
                .or_else(|| {
                    state
                        .passive_peer_infos
                        .get(&status.peer_device_id)
                        .and_then(|peer| {
                            let alias = peer.source_device_alias.trim();
                            if !alias.is_empty() {
                                Some(alias.to_string())
                            } else {
                                let name = peer.source_device_name.trim();
                                (!name.is_empty()).then(|| name.to_string())
                            }
                        })
                })
                .unwrap_or(fallback_device_name);
            passive_popup = Some((state.status.locale.clone(), username, device_name));
        }
    }
    // 仅刷新基础会话快照（不做 subnet-router FFI 查询，避免持锁阻塞 IPC）。
    // subnet-router 运行时统计的 FFI 增强由 refresh_wgvpn_sessions 在锁外完成。
    refresh_wgvpn_sessions_locked(&mut state);
    if let Some(tx) = &state.status_tx {
        let _ = tx.send(state.status.clone());
    }
    drop(state);
    if let Some((locale, username, device_name)) = passive_popup {
        show_passive_tunnel_popup(locale.as_deref(), &username, &device_name);
    }
    true
}

/// 被动端隧道建立后，把通知投递给当前用户会话中的 p2premote-notifier。
/// notifier 不在线时只记日志，不能影响隧道成功结果。
#[cfg(windows)]
fn show_passive_tunnel_popup(locale: Option<&str>, username: &str, device_name: &str) {
    let (title, body) = passive_tunnel_popup_text(locale, username, device_name);
    let payload = serde_json::json!({
        "title": title,
        "body": body,
        "locale": locale.unwrap_or("zh-CN"),
    })
    .to_string();
    let result = std::net::TcpStream::connect_timeout(
        &"127.0.0.1:48086".parse().expect("valid notifier address"),
        Duration::from_millis(300),
    )
    .and_then(|mut stream| std::io::Write::write_all(&mut stream, payload.as_bytes()));
    match result {
        Ok(_) => info!(
            "[ServiceRuntime] passive tunnel notification delivered: device={}",
            device_name
        ),
        Err(err) => info!(
            "[ServiceRuntime] notifier unavailable: device={}, error={}",
            device_name, err
        ),
    }
}

#[cfg(not(windows))]
fn show_passive_tunnel_popup(_locale: Option<&str>, _username: &str, _device_name: &str) {}

#[cfg(any(windows, test))]
pub(super) fn passive_tunnel_popup_text(
    locale: Option<&str>,
    username: &str,
    device_name: &str,
) -> (&'static str, String) {
    let username = username.trim();
    if locale == Some("en") {
        let body = if username.is_empty() {
            format!(
                "Unknown user connected to this device using {}.",
                device_name
            )
        } else {
            format!(
                "User {} connected to this device using {}.",
                username, device_name
            )
        };
        ("Remote device connected", body)
    } else {
        let body = if username.is_empty() {
            format!("未知用户已使用设备 {} 连接到本机。", device_name)
        } else {
            format!("用户 {} 已使用设备 {} 连接到本机。", username, device_name)
        };
        ("远程设备已连接", body)
    }
}

/// 从 WGVPN_SESSIONS 刷新 status.wgvpn_sessions 快照（含 subnet-router FFI 增强）。
///
/// 关键：subnet-router status 的 FFI 查询在锁外完成，避免持 shared 锁阻塞所有 IPC。
/// 流程：锁内 snapshot + 基础映射 → 释放锁 → 锁外批量 FFI → 再加锁回填 + 推送。
pub(super) fn refresh_wgvpn_sessions(shared: &Arc<Mutex<SharedRuntimeState>>) {
    refresh_wgvpn_sessions_with_options(shared, true, false);
}

pub(super) fn refresh_wgvpn_sessions_with_options(
    shared: &Arc<Mutex<SharedRuntimeState>>,
    broadcast: bool,
    include_transfer: bool,
) {
    // 阶段 1：锁内 snapshot 基础会话（不做 FFI）。
    let mut sessions = {
        let mut state = shared.lock();
        refresh_wgvpn_sessions_locked(&mut state);
        state.status.wgvpn_sessions.clone()
    };
    // 阶段 2：锁外批量查询 subnet-router 运行时统计（可能慢，跨语言 FFI）。
    let config = load_machine_config().ok();
    let punch_lib = config.as_ref().map(|cfg| {
        if cfg.p2p_punch_path.is_empty() {
            crate::config::default_p2p_punch_path()
        } else {
            std::path::PathBuf::from(&cfg.p2p_punch_path)
        }
    });
    if let Some(library_path) = punch_lib.as_deref() {
        for s in sessions.iter_mut() {
            if !s.userspace_wg_peer_handle.is_empty() {
                if let Ok(peer) = gonc_ffi::get_userspace_wg_peer_status(
                    library_path,
                    &s.userspace_wg_peer_handle,
                ) {
                    s.subnet_router_started = peer.started;
                    s.subnet_tcp_sessions = peer.tcp_sessions;
                    s.subnet_udp_sessions = peer.udp_sessions;
                    s.subnet_wg_rx_packets = peer.rx_packets.max(0) as u64;
                    s.subnet_wg_tx_packets = peer.tx_packets.max(0) as u64;
                    s.userspace_wg_rx_batches = peer.rx_batches.max(0) as u64;
                    s.userspace_wg_tx_batches = peer.tx_batches.max(0) as u64;
                    s.subnet_icmp_success = peer.icmp_success.max(0) as u64;
                    s.subnet_icmp_failed = peer.icmp_failed.max(0) as u64;
                    s.subnet_rejected_flows = peer.rejected_flows.max(0) as u64;
                    s.subnet_last_error = peer.last_error;
                    if include_transfer {
                        s.received_bytes = peer.rx_bytes.max(0) as u64;
                        s.transmitted_bytes = peer.tx_bytes.max(0) as u64;
                    }
                }
                continue;
            }
            if s.subnet_router_handle_id.is_empty() {
                continue;
            }
            if let Ok(router) =
                gonc_ffi::get_subnet_router_status(library_path, &s.subnet_router_handle_id)
            {
                if let Some(mode) = subnet_router::lan_mode_from_backend_label(&router.lan_mode) {
                    s.lan_mode = mode;
                }
                s.advertised_lan_routes = router.advertised_routes;
                s.subnet_router_started = router.started;
                s.subnet_tcp_sessions = router.tcp_sessions;
                s.subnet_udp_sessions = router.udp_sessions;
                s.subnet_wg_rx_packets = router.wg_rx_packets;
                s.subnet_wg_tx_packets = router.wg_tx_packets;
                s.subnet_icmp_success = router.icmp_success;
                s.subnet_icmp_failed = router.icmp_failed;
                s.subnet_rejected_flows = router.rejected_flows;
                s.subnet_last_error = router.last_error;
            }
        }
    }
    if include_transfer {
        let wg_cli = crate::config::default_wg_path()
            .to_string_lossy()
            .to_string();
        for session in sessions.iter_mut() {
            if session.userspace_wg {
                continue;
            }
            if let Some((received, transmitted)) = crate::wgvpn::peer_transfer_bytes(
                &wg_cli,
                &session.tunnel_name,
                &session.peer_pubkey,
            ) {
                session.received_bytes = received;
                session.transmitted_bytes = transmitted;
            }
        }
    }
    update_wgvpn_session_snapshot(shared, sessions, broadcast);
}

// 阶段 3：再加锁回填；仅生命周期变化需要广播，按需流量查询直接随命令响应返回。
pub(super) fn update_wgvpn_session_snapshot(
    shared: &Arc<Mutex<SharedRuntimeState>>,
    sessions: Vec<WgvpnSessionStatus>,
    broadcast: bool,
) {
    {
        let mut state = shared.lock();
        state.status.wgvpn_sessions = sessions;
        if broadcast {
            if let Some(tx) = &state.status_tx {
                let _ = tx.send(state.status.clone());
            }
        }
    }
}

/// 仅做基础会话快照映射（WGVPN_SESSIONS → WgvpnSessionStatus），不做 FFI。
/// 在持 shared 锁时调用安全（纯内存操作）。
pub(super) fn refresh_wgvpn_sessions_locked(state: &mut SharedRuntimeState) {
    let health_runtime = state.wgvpn_health_runtime.clone();
    let transfer_by_peer = state
        .status
        .wgvpn_sessions
        .iter()
        .map(|session| {
            (
                (session.peer_device_id, session.is_active),
                (session.received_bytes, session.transmitted_bytes),
            )
        })
        .collect::<HashMap<_, _>>();
    let userspace_batches_by_peer = state
        .status
        .wgvpn_sessions
        .iter()
        .map(|session| {
            (
                (session.peer_device_id, session.is_active),
                (
                    session.userspace_wg_rx_batches,
                    session.userspace_wg_tx_batches,
                ),
            )
        })
        .collect::<HashMap<_, _>>();
    state.status.wgvpn_sessions = wgvpn_flow::snapshot_sessions()
        .into_iter()
        .map(|s| {
            let health = health_runtime
                .get(&s.peer_device_id)
                .cloned()
                .unwrap_or_default();
            let (received_bytes, transmitted_bytes) = transfer_by_peer
                .get(&(s.peer_device_id, s.is_active))
                .copied()
                .unwrap_or_default();
            let (userspace_wg_rx_batches, userspace_wg_tx_batches) = userspace_batches_by_peer
                .get(&(s.peer_device_id, s.is_active))
                .copied()
                .unwrap_or_default();
            WgvpnSessionStatus {
                peer_device_id: s.peer_device_id,
                is_active: s.is_active,
                virtual_ip: s.virtual_ip,
                peer_virtual_ip: s.peer_virtual_ip,
                approval_pending: s.approval_pending,
                peer_pubkey: s.peer_pubkey,
                tunnel_name: s.tunnel_name,
                health_state: health.state,
                consecutive_failures: health.consecutive_failures,
                health_grace_deadline: health.grace_deadline,
                latency_ms: health.latency_ms,
                received_bytes,
                transmitted_bytes,
                local_forward_port: s.local_forward_port,
                exposed_lan_cidrs: s.exposed_lan_cidrs,
                advertised_lan_routes: s.advertised_lan_routes,
                lan_mode: s.lan_mode,
                subnet_router_handle_id: s.subnet_router_handle_id,
                userspace_wg_peer_handle: s.userspace_wg_peer_handle,
                userspace_wg: s.userspace_wg,
                subnet_router_started: s.subnet_router_started,
                subnet_tcp_sessions: s.subnet_tcp_sessions,
                subnet_udp_sessions: s.subnet_udp_sessions,
                subnet_wg_rx_packets: s.subnet_wg_rx_packets,
                subnet_wg_tx_packets: s.subnet_wg_tx_packets,
                userspace_wg_rx_batches,
                userspace_wg_tx_batches,
                subnet_icmp_success: s.subnet_icmp_success,
                subnet_icmp_failed: s.subnet_icmp_failed,
                subnet_rejected_flows: s.subnet_rejected_flows,
                subnet_last_error: s.subnet_last_error,
            }
        })
        .collect();
    let lifecycle_sessions = state.status.wgvpn_sessions.clone();
    for session in lifecycle_sessions {
        let role = if session.is_active {
            TunnelLifecycleRole::Active
        } else {
            TunnelLifecycleRole::Passive
        };
        let lifecycle_state = if !session.is_active && session.approval_pending {
            TunnelLifecycleState::AwaitingApproval
        } else if session.health_state == WgvpnHealthState::Degraded {
            TunnelLifecycleState::Recovering
        } else {
            TunnelLifecycleState::Connected
        };
        let existing = state
            .status
            .tunnel_lifecycles
            .iter()
            .find(|item| item.peer_device_id == session.peer_device_id && item.role == role)
            .cloned();
        // 提前取出 locale，避免在 upsert_tunnel_lifecycle(&mut state.status, ...) 期间重复借用
        let locale = state.status.locale.clone();
        upsert_tunnel_lifecycle(
            &mut state.status,
            TunnelLifecycleStatus {
                peer_device_id: session.peer_device_id,
                source_user_id: existing
                    .as_ref()
                    .map(|item| item.source_user_id)
                    .unwrap_or(0),
                source_username: existing
                    .as_ref()
                    .map(|item| item.source_username.clone())
                    .unwrap_or_default(),
                source_email: existing
                    .as_ref()
                    .map(|item| item.source_email.clone())
                    .unwrap_or_default(),
                peer_device_name: existing
                    .as_ref()
                    .map(|item| item.peer_device_name.clone())
                    .unwrap_or_default(),
                peer_device_alias: existing
                    .as_ref()
                    .map(|item| item.peer_device_alias.clone())
                    .unwrap_or_default(),
                peer_public_ip: existing
                    .as_ref()
                    .map(|item| item.peer_public_ip.clone())
                    .unwrap_or_default(),
                role,
                state: lifecycle_state,
                attempt: existing.as_ref().map(|item| item.attempt).unwrap_or(1),
                max_attempts: existing
                    .as_ref()
                    .map(|item| item.max_attempts)
                    .unwrap_or(ACTIVE_TUNNEL_JOB_MAX_ATTEMPTS),
                stage: None,
                virtual_ip: Some(session.virtual_ip),
                peer_virtual_ip: Some(session.peer_virtual_ip),
                last_result: TunnelLastResult::None,
                error_code: None,
                message: Some(
                    if lifecycle_state == TunnelLifecycleState::AwaitingApproval {
                        localized_message(
                            locale.as_deref(),
                            "tunnel.lifecycle.awaiting_approval",
                            &[],
                        )
                    } else if lifecycle_state == TunnelLifecycleState::Recovering {
                        localized_message(locale.as_deref(), "tunnel.lifecycle.recovering", &[])
                    } else {
                        localized_message(locale.as_deref(), "tunnel.lifecycle.connected", &[])
                    },
                ),
                health_failures: session.consecutive_failures,
                health_grace_deadline: session.health_grace_deadline,
                connected_at: None,
                updated_at: now_ts(),
            },
        );
    }
}
