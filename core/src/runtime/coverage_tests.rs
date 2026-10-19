use super::*;

fn shared() -> Arc<Mutex<SharedRuntimeState>> {
    Arc::new(Mutex::new(SharedRuntimeState::default()))
}
fn job(peer: i64, active: bool, state: &str) -> TunnelJobStatus {
    serde_json::from_value(serde_json::json!({"peer_device_id":peer,"is_active":active,"state":state,"attempt":1,"max_attempts":30,"message":"test","updated_at":1})).unwrap()
}

#[test]
fn failed_feedback_survives_locale_refresh_for_both_roles_without_raw_errors() {
    for active in [true, false] {
        let shared = shared();
        let mut failed = job(42, active, "failed");
        failed.error_code = "connection.approval.denied".into();
        failed.message = "secret dependency error: socket https://private.example".into();
        if active {
            update_active_tunnel_job_status(&shared, failed);
        } else {
            assert!(update_wgvpn_job_status(&shared, failed, None));
        }
        let mut state = shared.lock();
        let lifecycle = &state.status.tunnel_lifecycles[0];
        assert_eq!(
            lifecycle.error_code.as_deref(),
            Some("connection.approval.denied")
        );
        let zh = lifecycle.message.clone().unwrap();
        assert!(zh.contains("未获允许") && !zh.contains("secret"));
        if !active {
            assert!(zh.contains("发起方"));
        }
        state.status.locale = Some("en".into());
        relocalize_runtime_status(&mut state.status);
        let en = state.status.tunnel_lifecycles[0].message.clone().unwrap();
        assert!(en.contains("not approved") && !en.contains("secret"));
        // The lifecycle keeps its explanation even after the job collection is cleared.
        state.status.active_tunnel_jobs.clear();
        state.status.wgvpn_jobs.clear();
        state.status.locale = Some("zh-CN".into());
        relocalize_runtime_status(&mut state.status);
        assert_eq!(
            state.status.tunnel_lifecycles[0].message.as_deref(),
            Some(zh.as_str())
        );
    }
}

#[test]
fn feedback_respects_retry_wait_cancellation_and_a_single_final_network_suggestion() {
    let mut status = job(42, true, "waiting");
    status.error_code = "traversal.punch.timeout".into();
    status.tcp_retry_recommended = true;
    status.symmetric_nat_help_recommended = true;
    localize_tunnel_job_feedback(Some("en"), &mut status, 30);
    assert!(!status.tcp_retry_recommended && !status.symmetric_nat_help_recommended);
    assert!(status.message.contains("retrying in 30s"));
    assert!(!status.message.contains("different network"));

    status.state = TunnelJobState::Failed;
    status.tcp_retry_recommended = true;
    status.symmetric_nat_help_recommended = true;
    localize_tunnel_job_feedback(None, &mut status, 60);
    assert!(status.tcp_retry_recommended && !status.symmetric_nat_help_recommended);
    assert!(status.message.contains("TCP 优先") && !status.message.contains("对称"));

    status.error_code = "connection.execution.peer_cancelled".into();
    localize_tunnel_job_feedback(None, &mut status, 60);
    assert_eq!(status.state, TunnelJobState::Cancelled);
    assert!(!status.tcp_retry_recommended && !status.symmetric_nat_help_recommended);
    assert_eq!(status.message, "对端已取消本次连接。");
    let (_, last_result, code) = tunnel_job_lifecycle_projection(status.state, &status.error_code);
    assert_eq!(last_result, TunnelLastResult::Cancelled);
    assert_eq!(code.as_deref(), Some("connection.execution.peer_cancelled"));

    status.state = TunnelJobState::Failed;
    status.error_code = "traversal.execution.cancelled".into();
    localize_tunnel_job_feedback(None, &mut status, 60);
    assert_eq!(status.state, TunnelJobState::Cancelled);
    assert_eq!(status.message, "本次连接已取消。");

    status.state = TunnelJobState::Succeeded;
    status.message = "connected".into();
    status.tcp_retry_recommended = true;
    localize_tunnel_job_feedback(None, &mut status, 60);
    assert_eq!(status.message, "connected");
    assert!(!status.tcp_retry_recommended);
}
fn monitor(generation: u64) -> WgvpnHealthControl {
    let (stop_tx, _) = watch::channel(false);
    let (speed_tx, _) = mpsc::unbounded_channel();
    WgvpnHealthControl {
        generation,
        stop_tx,
        speed_tx,
        speed_test_busy: Arc::new(AtomicBool::new(false)),
    }
}
fn lifecycle(peer: i64, role: &str, state: &str) -> TunnelLifecycleStatus {
    serde_json::from_value(serde_json::json!({"peer_device_id":peer,"role":role,"state":state,"attempt":1,"max_attempts":30,"last_result":"none","updated_at":1})).unwrap()
}

#[test]
fn lifecycle_updates_preserve_identity_timestamp_and_independent_roles() {
    let mut state = RuntimeStatus::default();
    let mut first = lifecycle(42, "active", "connecting");
    first.source_user_id = 9;
    first.source_username = "user".into();
    first.source_email = "unit@example.invalid".into();
    first.peer_device_name = "device".into();
    first.peer_device_alias = "alias".into();
    first.peer_public_ip = "1.1.1.1".into();
    upsert_tunnel_lifecycle(&mut state, first.clone());
    upsert_tunnel_lifecycle(&mut state, lifecycle(42, "active", "connected"));
    let connected = state.tunnel_lifecycles[0].clone();
    assert_eq!(connected.source_user_id, 9);
    assert_eq!(connected.source_username, "user");
    assert_eq!(connected.source_email, "unit@example.invalid");
    assert_eq!(connected.peer_device_name, "device");
    assert_eq!(connected.peer_device_alias, "alias");
    assert_eq!(connected.peer_public_ip, "1.1.1.1");
    assert!(connected.connected_at.is_some());
    upsert_tunnel_lifecycle(&mut state, lifecycle(42, "active", "recovering"));
    assert_eq!(
        state.tunnel_lifecycles[0].connected_at,
        connected.connected_at
    );
    first.source_user_id = 10;
    first.source_username = "updated".into();
    first.connected_at = Some(123);
    upsert_tunnel_lifecycle(&mut state, first);
    assert_eq!(state.tunnel_lifecycles[0].connected_at, Some(123));
    assert_eq!(state.tunnel_lifecycles[0].source_username, "updated");
    upsert_tunnel_lifecycle(&mut state, lifecycle(42, "passive", "connected"));
    assert_eq!(state.tunnel_lifecycles.len(), 2);
    assert!(state.tunnel_lifecycles[1].connected_at.is_some());
}

#[test]
fn job_status_projection_and_broadcast_keep_state_and_canonical_reason() {
    let shared = shared();
    let (tx, mut rx) = tokio::sync::broadcast::channel(16);
    shared.lock().status_tx = Some(tx);
    for state in ["running", "waiting", "succeeded", "failed", "cancelled"] {
        let mut status = job(42, true, state);
        status.error_code = "traversal.punch.timeout".into();
        update_active_tunnel_job_status(&shared, status);
        let snapshot = rx.try_recv().unwrap();
        assert_eq!(snapshot.active_tunnel_jobs.len(), 1);
        assert_eq!(snapshot.tunnel_lifecycles.len(), 1);
        let lifecycle = &snapshot.tunnel_lifecycles[0];
        match state {
            "running" => assert_eq!(lifecycle.stage.as_deref(), Some("hole_punch_wait")),
            "waiting" => {
                assert_eq!(lifecycle.stage.as_deref(), Some("retry_wait"));
                assert_eq!(
                    lifecycle.error_code.as_deref(),
                    Some("traversal.punch.timeout")
                );
            }
            "succeeded" => assert_eq!(lifecycle.state, TunnelLifecycleState::Connected),
            "failed" => assert_eq!(
                lifecycle.last_result,
                crate::control::TunnelLastResult::AttemptFailed
            ),
            _ => assert_eq!(
                lifecycle.last_result,
                crate::control::TunnelLastResult::Cancelled
            ),
        }
    }
    clear_active_tunnel_job_status(&shared, 41);
    assert_eq!(shared.lock().status.active_tunnel_jobs.len(), 1);
    clear_active_tunnel_job_status(&shared, 42);
    assert!(shared.lock().status.active_tunnel_jobs.is_empty());
    update_status(&shared, |s| s.locale = Some("en".into()));
    assert_eq!(current_locale(&shared).as_deref(), Some("en"));
}

#[tokio::test]
async fn health_snapshots_cover_both_roles_and_repeated_degradation() {
    for active in [false, true] {
        let shared = shared();
        {
            let mut state = shared.lock();
            state.status.wgvpn_sessions.push(serde_json::from_value(serde_json::json!({"peer_device_id":42,"is_active":active,"virtual_ip":"100.99.0.1","peer_virtual_ip":"100.99.0.2","peer_pubkey":"test","tunnel_name":"test"})).unwrap());
            state.status.tunnel_lifecycles.push(lifecycle(
                42,
                if active { "active" } else { "passive" },
                "connected",
            ));
            let (tx, _) = tokio::sync::broadcast::channel(16);
            state.status_tx = Some(tx);
        }
        mark_wgvpn_health_degraded(shared.clone(), 42, "unit-test");
        let deadline = shared.lock().status.wgvpn_sessions[0].health_grace_deadline;
        assert!(deadline.is_some());
        assert_eq!(
            shared.lock().status.tunnel_lifecycles[0].state,
            TunnelLifecycleState::Recovering
        );
        mark_wgvpn_health_degraded(shared.clone(), 42, "duplicate");
        assert_eq!(
            shared.lock().status.wgvpn_sessions[0].health_grace_deadline,
            deadline
        );
        record_wgvpn_health_success(&shared, 42, None, Some(4));
        assert_eq!(
            shared.lock().status.wgvpn_sessions[0].health_state,
            WgvpnHealthState::Connected
        );
        assert_eq!(
            shared.lock().status.tunnel_lifecycles[0].state,
            TunnelLifecycleState::Connected
        );
        cancel_passive_health_grace(&shared, 42);
    }
}

#[test]
fn stale_job_updates_and_removals_cannot_replace_current_generation() {
    let shared = shared();
    let (cancel_tx, cancelled) = watch::channel(false);
    shared.lock().wgvpn_job_cancels.insert(
        42,
        WgvpnJobControl {
            generation: 2,
            cancel_tx,
        },
    );
    assert!(!is_current_wgvpn_job(&shared, 41, 2));
    assert!(!is_current_wgvpn_job(&shared, 42, 1));
    assert!(is_current_wgvpn_job(&shared, 42, 2));
    assert!(!update_wgvpn_job_status(
        &shared,
        job(42, true, "failed"),
        Some(1)
    ));
    assert!(shared.lock().status.wgvpn_jobs.is_empty());
    remove_wgvpn_job_if_current(&shared, 42, 1);
    assert!(is_current_wgvpn_job(&shared, 42, 2));
    assert!(update_wgvpn_job_status(
        &shared,
        job(42, true, "running"),
        Some(2)
    ));
    assert!(update_wgvpn_job_status(
        &shared,
        job(42, true, "waiting"),
        Some(2)
    ));
    assert_eq!(shared.lock().status.wgvpn_jobs.len(), 1);
    assert_eq!(
        shared.lock().status.wgvpn_jobs[0].state,
        TunnelJobState::Waiting
    );
    publish_wgvpn_job_cancelled(&shared, 42, true, 1);
    assert!(is_current_wgvpn_job(&shared, 42, 2));
    publish_wgvpn_job_cancelled(&shared, 42, true, 2);
    assert!(!is_current_wgvpn_job(&shared, 42, 2));
    assert_eq!(
        shared.lock().status.wgvpn_jobs[0].state,
        TunnelJobState::Cancelled
    );
    assert!(cancelled.has_changed().is_err());
}

#[test]
fn passive_job_projection_preserves_identity_when_status_changes() {
    let shared = shared();
    assert!(update_wgvpn_job_status(
        &shared,
        job(42, false, "running"),
        None
    ));
    {
        let mut state = shared.lock();
        assert_eq!(state.status.tunnel_lifecycles.len(), 1);
        state.status.tunnel_lifecycles[0].source_username = "known-user".into();
        state.status.tunnel_lifecycles[0].peer_device_name = "known-device".into();
    }
    let mut failed = job(42, false, "failed");
    failed.error_code = crate::error::ErrorCode::TraversalPunchTimeout
        .as_str()
        .into();
    assert!(update_wgvpn_job_status(&shared, failed, None));
    let state = shared.lock();
    assert_eq!(state.status.wgvpn_jobs.len(), 1);
    assert_eq!(state.status.tunnel_lifecycles.len(), 1);
    assert_eq!(
        state.status.tunnel_lifecycles[0].source_username,
        "known-user"
    );
    assert_eq!(
        state.status.tunnel_lifecycles[0].peer_device_name,
        "known-device"
    );
    assert_eq!(
        state.status.tunnel_lifecycles[0].state,
        TunnelLifecycleState::NotEstablished
    );
}

#[test]
fn manual_start_checks_shutdown_websocket_existing_result_and_job_before_spawn() {
    let shared = shared();
    shared.lock().shutdown_requested = true;
    let run = || start_active_tunnel_job(&shared, 42, "uuid".into(), None, None, vec![]).unwrap();
    assert!(matches!(run(), Data::CommandResponse { ok: false, .. }));
    shared.lock().shutdown_requested = false;
    assert!(matches!(run(), Data::CommandResponse { ok: false, .. }));
    {
        let mut state = shared.lock();
        state.ws_client = Some(ServiceWsClient::new());
        state
            .active_tunnel_job_cancels
            .insert(42, watch::channel(false).0);
        state
            .status
            .active_tunnel_jobs
            .push(job(42, true, "succeeded"));
    }
    assert!(
        matches!(run(),Data::CommandResponse {ok:true,message,..} if message.contains("already running"))
    );
    shared.lock().status.active_tunnel_jobs[0].result=Some(serde_json::from_value(serde_json::json!({"success":true,"rdp_address":"100.99.0.1:3389","message":"connected"})).unwrap());
    assert!(
        matches!(run(),Data::CommandResponse {ok:false,message,..} if message.contains("already connected"))
    );
}

#[tokio::test]
async fn health_threshold_recovery_and_stale_monitor_results_are_separate() {
    let shared = shared();
    record_wgvpn_health_success(&shared, 42, Some(99), Some(3));
    record_wgvpn_health_failure(shared.clone(), 42, Some(99));
    assert!(shared.lock().wgvpn_health_runtime.is_empty());
    shared.lock().wgvpn_health_controls.insert(42, monitor(2));
    for _ in 0..WGVPN_HEALTH_FAILURE_THRESHOLD - 1 {
        record_wgvpn_health_failure(shared.clone(), 42, Some(2));
    }
    assert_eq!(
        shared.lock().wgvpn_health_runtime[&42].state,
        WgvpnHealthState::Connected
    );
    record_wgvpn_health_failure(shared.clone(), 42, Some(2));
    let deadline = shared.lock().wgvpn_health_runtime[&42].grace_deadline;
    assert!(deadline.is_some());
    assert_eq!(
        shared.lock().wgvpn_health_runtime[&42].state,
        WgvpnHealthState::Degraded
    );
    record_wgvpn_health_failure(shared.clone(), 42, Some(2));
    assert_eq!(
        shared.lock().wgvpn_health_runtime[&42].grace_deadline,
        deadline
    );
    record_wgvpn_health_success(&shared, 42, Some(1), Some(999));
    assert_eq!(
        shared.lock().wgvpn_health_runtime[&42].state,
        WgvpnHealthState::Degraded
    );
    record_wgvpn_health_success(&shared, 42, Some(2), Some(8));
    {
        let state = shared.lock();
        let health = &state.wgvpn_health_runtime[&42];
        assert_eq!(health.state, WgvpnHealthState::Connected);
        assert_eq!(health.consecutive_failures, 0);
        assert_eq!(health.latency_ms, Some(8));
        assert!(health.grace_deadline.is_none());
        assert!(!state.passive_health_grace_controls.contains_key(&42));
    }
    record_wgvpn_health_success(&shared, 42, None, None);
    record_wgvpn_health_success(&shared, 42, None, Some(8));
    record_wgvpn_health_success(&shared, 42, None, Some(9));
    assert_eq!(shared.lock().wgvpn_health_runtime[&42].latency_ms, Some(9));
}

#[test]
fn cleanup_guards_do_not_clear_replaced_or_unexpired_sessions() {
    let mut state = SharedRuntimeState::default();
    assert!(!claim_wgvpn_cleanup_locked(
        &mut state,
        42,
        CleanupGuard::Monitor(1)
    ));
    state.wgvpn_health_controls.insert(42, monitor(2));
    assert!(!claim_wgvpn_cleanup_locked(
        &mut state,
        42,
        CleanupGuard::Monitor(1)
    ));
    assert!(claim_wgvpn_cleanup_locked(
        &mut state,
        42,
        CleanupGuard::Monitor(2)
    ));
    for (health, deadline, expected) in [
        (WgvpnHealthState::Connected, Some(now_ts() - 1), false),
        (WgvpnHealthState::Degraded, None, false),
        (WgvpnHealthState::Degraded, Some(now_ts() + 300), false),
        (WgvpnHealthState::Degraded, Some(now_ts() - 1), true),
    ] {
        let (cancel_tx, _) = watch::channel(false);
        state.passive_health_grace_controls.insert(
            42,
            PassiveHealthGraceControl {
                generation: 3,
                cancel_tx,
            },
        );
        state.wgvpn_health_runtime.insert(
            42,
            WgvpnHealthRuntime {
                state: health,
                grace_deadline: deadline,
                ..Default::default()
            },
        );
        assert!(!claim_wgvpn_cleanup_locked(
            &mut state,
            42,
            CleanupGuard::Grace(2)
        ));
        assert_eq!(
            claim_wgvpn_cleanup_locked(&mut state, 42, CleanupGuard::Grace(3)),
            expected
        );
        if expected {
            assert!(!state.wgvpn_health_runtime.contains_key(&42));
        }
    }
    assert!(claim_wgvpn_cleanup_locked(
        &mut state,
        42,
        CleanupGuard::None
    ));
}

#[tokio::test]
async fn pure_ipc_commands_and_response_builders_preserve_typed_errors() {
    let shared = shared();
    assert!(
        matches!(handle_data(Data::Ping,&shared).await,Some(Data::CommandResponse {ok:true,message,..}) if message=="pong")
    );
    assert!(matches!(
        handle_data(Data::Status, &shared).await,
        Some(Data::CommandResponse {
            ok: true,
            status: Some(_),
            ..
        })
    ));
    assert_eq!(
        normalize_lan_cidrs_for_config(vec![
            " ".into(),
            "10.0.0.8/24".into(),
            "10.0.0.0/24".into()
        ])
        .unwrap(),
        vec!["10.0.0.0/24"]
    );
    assert!(normalize_lan_cidrs_for_config(vec!["not-a-cidr".into()]).is_err());
    assert!(
        matches!(response_from_result(Ok(42),|n|serde_json::json!(n)),Data::CommandResponse {ok:true,data:Some(value),..} if value==42)
    );
    let error = crate::error::coded(crate::error::ErrorCode::SignalSendTimeout, "diagnostic");
    assert!(
        matches!(response_from_result::<()>(Err(error),|_|panic!("failure must not run success mapper")),Data::CommandResponse {ok:false,error:Some(error),..} if error.code=="signal.send.timeout")
    );
}
