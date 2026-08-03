//! WGVPN health monitoring, grace periods, watchdogs, and cleanup arbitration.

use super::*;

pub(super) fn spawn_passive_health_watchdog(
    shared: Arc<Mutex<SharedRuntimeState>>,
    peer_device_id: i64,
) {
    let generation = NEXT_PASSIVE_HEALTH_WATCHDOG_GENERATION.fetch_add(1, Ordering::Relaxed);
    {
        let mut state = shared.lock();
        if state
            .passive_health_connection_generations
            .contains_key(&peer_device_id)
        {
            return;
        }
        state
            .passive_health_watchdog_generations
            .insert(peer_device_id, generation);
    }
    info!(
        "[wgvpn-health] passive first-connection watchdog started: peer_device_id={}, seconds={}",
        peer_device_id, PASSIVE_HEALTH_CONNECT_WAIT_SECS
    );
    tokio::spawn(async move {
        tokio::time::sleep(Duration::from_secs(PASSIVE_HEALTH_CONNECT_WAIT_SECS)).await;
        let expired =
            claim_passive_health_watchdog_timeout(&mut shared.lock(), peer_device_id, generation);
        let passive_session_exists = wgvpn_flow::snapshot_sessions()
            .iter()
            .any(|session| session.peer_device_id == peer_device_id && !session.is_active);
        if expired && passive_session_exists {
            info!(
                "[wgvpn-health] passive first health connection timed out: peer_device_id={}",
                peer_device_id
            );
            mark_wgvpn_health_degraded(shared, peer_device_id, "initial_health_connection_timeout");
        }
    });
}

pub(super) fn claim_passive_health_watchdog_timeout(
    state: &mut SharedRuntimeState,
    peer_device_id: i64,
    generation: u64,
) -> bool {
    let is_current = state
        .passive_health_watchdog_generations
        .get(&peer_device_id)
        .copied()
        == Some(generation);
    let connected = state
        .passive_health_connection_generations
        .contains_key(&peer_device_id);
    if is_current {
        state
            .passive_health_watchdog_generations
            .remove(&peer_device_id);
    }
    is_current && !connected
}

pub(super) fn spawn_wgvpn_health_monitor(
    shared: Arc<Mutex<SharedRuntimeState>>,
    peer_device_id: i64,
    peer_virtual_ip: String,
) {
    let source_device_id = match load_machine_config()
        .ok()
        .and_then(|config| config.device_id)
    {
        Some(device_id) => device_id,
        None => {
            info!(
                "[wgvpn-health] skipped without local device id: peer_device_id={}",
                peer_device_id
            );
            return;
        }
    };
    let generation = NEXT_WGVPN_HEALTH_GENERATION.fetch_add(1, Ordering::Relaxed);
    let (stop_tx, stop_rx) = watch::channel(false);
    let (speed_tx, speed_rx) = mpsc::unbounded_channel();
    let speed_test_busy = Arc::new(std::sync::atomic::AtomicBool::new(false));
    {
        let mut state = shared.lock();
        if let Some(previous) = state.wgvpn_health_controls.insert(
            peer_device_id,
            WgvpnHealthControl {
                generation,
                stop_tx,
                speed_tx,
                speed_test_busy: speed_test_busy.clone(),
            },
        ) {
            let _ = previous.stop_tx.send(true);
        }
    }
    let health_addr = format!("{}:{}", peer_virtual_ip, HEALTH_PORT);
    info!(
        "[wgvpn-health] monitor started: peer_device_id={}, address={}",
        peer_device_id, health_addr
    );

    let event_shared = shared.clone();
    let event_handler = Arc::new(move |event| match event {
        TunnelHealthEvent::HeartbeatSucceeded { latency_ms } => {
            record_wgvpn_health_success(&event_shared, peer_device_id, Some(generation), latency_ms)
        }
        TunnelHealthEvent::HeartbeatFailed => {
            record_wgvpn_health_failure(event_shared.clone(), peer_device_id, Some(generation))
        }
        TunnelHealthEvent::ConnectionClosed => {
            cleanup_wgvpn_session_async(
                event_shared.clone(),
                peer_device_id,
                "tcp_fin",
                CleanupGuard::Monitor(generation),
            );
        }
    });

    tokio::spawn(async move {
        let result = wgvpn_health_monitor_loop(
            source_device_id,
            health_addr,
            stop_rx,
            speed_rx,
            speed_test_busy,
            event_handler,
        )
        .await;
        let is_current = {
            let mut state = shared.lock();
            let current = state
                .wgvpn_health_controls
                .get(&peer_device_id)
                .map(|control| control.generation == generation)
                .unwrap_or(false);
            if current {
                state.wgvpn_health_controls.remove(&peer_device_id);
            }
            current
        };
        if !is_current {
            return;
        }

        match result {
            Ok(()) => info!(
                "[wgvpn-health] monitor stopped: peer_device_id={}",
                peer_device_id
            ),
            Err(err) => info!(
                "[wgvpn-health] monitor exited with error: peer_device_id={}, error={:#}",
                peer_device_id, err
            ),
        }
    });
}

pub(super) fn stop_wgvpn_health_monitor(
    shared: &Arc<Mutex<SharedRuntimeState>>,
    peer_device_id: i64,
) {
    if let Some(control) = shared.lock().wgvpn_health_controls.remove(&peer_device_id) {
        let _ = control.stop_tx.send(true);
    }
}

pub(super) fn stop_all_wgvpn_health_monitors(shared: &Arc<Mutex<SharedRuntimeState>>) {
    let controls = std::mem::take(&mut shared.lock().wgvpn_health_controls);
    for (_, control) in controls {
        let _ = control.stop_tx.send(true);
    }
    let grace_controls = std::mem::take(&mut shared.lock().passive_health_grace_controls);
    for (_, control) in grace_controls {
        let _ = control.cancel_tx.send(true);
    }
    shared.lock().passive_health_watchdog_generations.clear();
}

pub(super) fn publish_wgvpn_health_status_locked(
    state: &mut SharedRuntimeState,
    peer_device_id: i64,
) {
    let health = state
        .wgvpn_health_runtime
        .get(&peer_device_id)
        .cloned()
        .unwrap_or_default();
    let session_snapshot = if let Some(session) = state
        .status
        .wgvpn_sessions
        .iter_mut()
        .find(|session| session.peer_device_id == peer_device_id)
    {
        session.health_state = health.state;
        session.consecutive_failures = health.consecutive_failures;
        session.health_grace_deadline = health.grace_deadline;
        session.latency_ms = health.latency_ms;
        Some(session.clone())
    } else {
        None
    };
    if let Some(session) = session_snapshot {
        let role = if session.is_active {
            TunnelLifecycleRole::Active
        } else {
            TunnelLifecycleRole::Passive
        };
        let locale = state.status.locale.clone();
        if let Some(lifecycle) = state
            .status
            .tunnel_lifecycles
            .iter_mut()
            .find(|item| item.peer_device_id == peer_device_id && item.role == role)
        {
            lifecycle.state = if health.state == WgvpnHealthState::Degraded {
                TunnelLifecycleState::Recovering
            } else {
                TunnelLifecycleState::Connected
            };
            lifecycle.health_failures = health.consecutive_failures;
            lifecycle.health_grace_deadline = health.grace_deadline;
            lifecycle.message = Some(if health.state == WgvpnHealthState::Degraded {
                localized_message(locale.as_deref(), "tunnel.lifecycle.recovering", &[])
            } else {
                localized_message(locale.as_deref(), "tunnel.lifecycle.connected", &[])
            });
            lifecycle.updated_at = now_ts();
        }
    }
    if let Some(tx) = &state.status_tx {
        let _ = tx.send(state.status.clone());
    }
}

pub(super) fn monitor_generation_matches(
    state: &SharedRuntimeState,
    peer_device_id: i64,
    expected_generation: Option<u64>,
) -> bool {
    expected_generation.is_none_or(|generation| {
        state
            .wgvpn_health_controls
            .get(&peer_device_id)
            .map(|control| control.generation == generation)
            .unwrap_or(false)
    })
}

pub(super) fn record_wgvpn_health_success(
    shared: &Arc<Mutex<SharedRuntimeState>>,
    peer_device_id: i64,
    expected_generation: Option<u64>,
    latency_ms: Option<u32>,
) {
    let changed = {
        let mut state = shared.lock();
        if !monitor_generation_matches(&state, peer_device_id, expected_generation) {
            return;
        }
        let health = state
            .wgvpn_health_runtime
            .entry(peer_device_id)
            .or_default();
        let changed = health.state != WgvpnHealthState::Connected
            || health.consecutive_failures != 0
            || health.grace_deadline.is_some()
            || latency_ms.is_some_and(|value| health.latency_ms != Some(value));
        health.state = WgvpnHealthState::Connected;
        health.consecutive_failures = 0;
        health.grace_deadline = None;
        if let Some(latency_ms) = latency_ms {
            health.latency_ms = Some(latency_ms);
        }
        if changed {
            publish_wgvpn_health_status_locked(&mut state, peer_device_id);
        }
        changed
    };
    if changed {
        cancel_passive_health_grace(shared, peer_device_id);
        info!(
            "[wgvpn-health] connection recovered: peer_device_id={}",
            peer_device_id
        );
    }
}

pub(super) fn record_wgvpn_health_failure(
    shared: Arc<Mutex<SharedRuntimeState>>,
    peer_device_id: i64,
    expected_generation: Option<u64>,
) {
    let should_start_grace = {
        let mut state = shared.lock();
        if !monitor_generation_matches(&state, peer_device_id, expected_generation) {
            return;
        }
        let health = state
            .wgvpn_health_runtime
            .entry(peer_device_id)
            .or_default();
        health.consecutive_failures = health.consecutive_failures.saturating_add(1);
        if health.consecutive_failures >= WGVPN_HEALTH_FAILURE_THRESHOLD
            && health.state != WgvpnHealthState::Degraded
        {
            health.state = WgvpnHealthState::Degraded;
            health.grace_deadline = Some(now_ts() + WGVPN_HEALTH_GRACE_SECS as i64);
            health.latency_ms = None;
            publish_wgvpn_health_status_locked(&mut state, peer_device_id);
            true
        } else {
            false
        }
    };
    if should_start_grace {
        schedule_passive_health_grace(shared, peer_device_id, "heartbeat_failures");
    }
}

pub(super) fn mark_wgvpn_health_degraded(
    shared: Arc<Mutex<SharedRuntimeState>>,
    peer_device_id: i64,
    reason: &'static str,
) {
    let should_start_grace = {
        let mut state = shared.lock();
        let health = state
            .wgvpn_health_runtime
            .entry(peer_device_id)
            .or_default();
        if health.state == WgvpnHealthState::Degraded && health.grace_deadline.is_some() {
            false
        } else {
            health.state = WgvpnHealthState::Degraded;
            health.consecutive_failures = WGVPN_HEALTH_FAILURE_THRESHOLD;
            health.grace_deadline = Some(now_ts() + WGVPN_HEALTH_GRACE_SECS as i64);
            health.latency_ms = None;
            publish_wgvpn_health_status_locked(&mut state, peer_device_id);
            true
        }
    };
    if should_start_grace {
        schedule_passive_health_grace(shared, peer_device_id, reason);
    }
}

pub(super) fn cancel_passive_health_grace(
    shared: &Arc<Mutex<SharedRuntimeState>>,
    peer_device_id: i64,
) {
    if let Some(control) = shared
        .lock()
        .passive_health_grace_controls
        .remove(&peer_device_id)
    {
        let _ = control.cancel_tx.send(true);
    }
}

pub(super) fn schedule_passive_health_grace(
    shared: Arc<Mutex<SharedRuntimeState>>,
    peer_device_id: i64,
    reason: &'static str,
) {
    let generation = NEXT_PASSIVE_HEALTH_GRACE_GENERATION.fetch_add(1, Ordering::Relaxed);
    let (cancel_tx, mut cancel_rx) = watch::channel(false);
    {
        let mut state = shared.lock();
        if let Some(previous) = state.passive_health_grace_controls.insert(
            peer_device_id,
            PassiveHealthGraceControl {
                generation,
                cancel_tx,
            },
        ) {
            let _ = previous.cancel_tx.send(true);
        }
    }
    info!(
        "[wgvpn-health] cleanup grace started: peer_device_id={}, reason={}, seconds={}",
        peer_device_id, reason, WGVPN_HEALTH_GRACE_SECS
    );

    tokio::spawn(async move {
        tokio::select! {
            _ = tokio::time::sleep(Duration::from_secs(WGVPN_HEALTH_GRACE_SECS)) => {}
            _ = cancel_rx.changed() => return,
        }
        cleanup_wgvpn_session_async(
            shared,
            peer_device_id,
            "health_grace_expired",
            CleanupGuard::Grace(generation),
        );
    });
}

pub(super) fn cleanup_wgvpn_session_async(
    shared: Arc<Mutex<SharedRuntimeState>>,
    peer_device_id: i64,
    reason: &'static str,
    guard: CleanupGuard,
) {
    tokio::spawn(async move {
        let locale = current_locale(&shared);
        let eligible = {
            let mut state = shared.lock();
            claim_wgvpn_cleanup_locked(&mut state, peer_device_id, guard)
        };
        if !eligible {
            return;
        }
        stop_wgvpn_health_monitor(&shared, peer_device_id);
        cancel_passive_health_grace(&shared, peer_device_id);
        info!(
            "[wgvpn-health] cleaning session: peer_device_id={}, reason={}",
            peer_device_id, reason
        );
        let role = wgvpn_flow::snapshot_sessions()
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
        match load_machine_config() {
            Ok(config) => {
                let _ = wgvpn_flow::stop_wgvpn(&config, peer_device_id).await;
                {
                    let mut state = shared.lock();
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
                            role,
                            state: TunnelLifecycleState::NotEstablished,
                            attempt: 0,
                            max_attempts: ACTIVE_TUNNEL_JOB_MAX_ATTEMPTS,
                            stage: None,
                            virtual_ip: None,
                            peer_virtual_ip: None,
                            last_result: if reason == "health_grace_expired" {
                                TunnelLastResult::HealthGraceExpired
                            } else {
                                TunnelLastResult::PeerDisconnected
                            },
                            error_code: Some(reason.to_string()),
                            message: Some(if reason == "health_grace_expired" {
                                localized_message(
                                    locale.as_deref(),
                                    "tunnel.lifecycle.health_grace_expired",
                                    &[],
                                )
                            } else {
                                localized_message(
                                    locale.as_deref(),
                                    "tunnel.lifecycle.disconnected",
                                    &[],
                                )
                            }),
                            health_failures: 0,
                            health_grace_deadline: None,
                            connected_at: None,
                            updated_at: now_ts(),
                        },
                    );
                }
                refresh_wgvpn_sessions(&shared);
            }
            Err(err) => info!(
                "[wgvpn-health] cleanup config load failed: peer_device_id={}, error={}",
                peer_device_id, err
            ),
        }
    });
}

pub(super) fn claim_wgvpn_cleanup_locked(
    state: &mut SharedRuntimeState,
    peer_device_id: i64,
    guard: CleanupGuard,
) -> bool {
    let eligible = match guard {
        CleanupGuard::None => true,
        CleanupGuard::Monitor(generation) => state
            .wgvpn_health_controls
            .get(&peer_device_id)
            .map(|control| control.generation == generation)
            .unwrap_or(false),
        CleanupGuard::Grace(generation) => {
            let grace_matches = state
                .passive_health_grace_controls
                .get(&peer_device_id)
                .map(|control| control.generation == generation)
                .unwrap_or(false);
            let health_expired = state
                .wgvpn_health_runtime
                .get(&peer_device_id)
                .map(|health| {
                    health.state == WgvpnHealthState::Degraded
                        && health
                            .grace_deadline
                            .is_some_and(|deadline| deadline <= now_ts())
                })
                .unwrap_or(false);
            grace_matches && health_expired
        }
    };
    if eligible {
        state.passive_health_grace_controls.remove(&peer_device_id);
        state
            .passive_health_connection_generations
            .remove(&peer_device_id);
        state
            .passive_health_watchdog_generations
            .remove(&peer_device_id);
        state.wgvpn_health_runtime.remove(&peer_device_id);
    }
    eligible
}
