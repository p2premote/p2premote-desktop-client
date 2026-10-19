use super::*;

#[tokio::test]
async fn recovery_guards_do_not_load_config_or_start_network_work() {
    for blocked in [
        "login",
        "ws",
        "job",
        "paused",
        "passive",
        "connecting",
        "backoff",
        "account",
    ] {
        let shared = Arc::new(Mutex::new(SharedRuntimeState::default()));
        let mut config = config();
        save_local_setting(&mut config, 42, true).unwrap();
        let (tx, _job_rx) = watch::channel(false);
        {
            let mut s = shared.lock();
            s.status.logged_in = true;
            s.status.ws_connected = true;
            apply_local_settings(&mut s, &config);
            match blocked {
                "login"=>s.status.logged_in=false,"ws"=>s.status.ws_connected=false,
                "job"=>{s.active_tunnel_job_cancels.insert(99,tx);},
                "paused"=>s.auto_keep_settings[0].paused_remote_stop=true,
                "passive"=>{s.passive_p2p_attempts.insert(42,PassiveP2PAttempt{connection_id:"unit".into(),attempt_id:"unit".into()});},
                "connecting"=>s.status.tunnel_lifecycles.push(serde_json::from_value(serde_json::json!({"peer_device_id":42,"role":"active","state":"connecting","attempt":1,"max_attempts":30,"last_result":"none","updated_at":1})).unwrap()),
                "backoff"=>{s.auto_keep_retries.insert(42,AutoKeepRetryRuntime{next_retry_at:now_ts()+3600,..Default::default()});},
                "account"=>{note_auto_keep_job_failed(&mut s,42,1);config.user_email=Some("other@example.invalid".into());apply_local_settings(&mut s,&config);assert!(s.auto_keep_settings.is_empty());assert!(s.auto_keep_retries.is_empty());},
                _=>unreachable!(),
            }
        }
        evaluate_auto_keep(&shared).await.unwrap();
        let s = shared.lock();
        assert!(!s.active_tunnel_job_cancels.contains_key(&42));
        assert!(s.status.active_tunnel_jobs.is_empty());
    }
}

fn config() -> MachineConfig {
    MachineConfig {
        server_url: "https://unit.invalid".into(),
        user_email: Some("unit@example.invalid".into()),
        device_uuid: Some("local".into()),
        ..Default::default()
    }
}

#[test]
fn setting_updates_require_identity_and_only_change_the_current_account_entry() {
    let mut missing = MachineConfig::default();
    assert!(save_local_setting(&mut missing, 42, true).is_err());
    missing.user_email = Some("unit@example.invalid".into());
    assert!(save_local_setting(&mut missing, 42, true).is_err());
    assert!(missing.auto_keep_settings.is_empty());
    assert!(save_local_setting(&mut missing, 42, false).is_ok());
    let mut config = config();
    save_local_setting(&mut config, 42, true).unwrap();
    config.auto_keep_settings[0].paused_remote_stop = true;
    config.auto_keep_settings[0].active_side = false;
    save_local_setting(&mut config, 42, true).unwrap();
    assert_eq!(config.auto_keep_settings.len(), 1);
    assert!(config.auto_keep_settings[0].active_side);
    assert!(!config.auto_keep_settings[0].paused_remote_stop);
    config.user_email = Some("other@example.invalid".into());
    save_local_setting(&mut config, 42, false).unwrap();
    assert_eq!(config.auto_keep_settings.len(), 1);
    config.user_email = Some("unit@example.invalid".into());
    save_local_setting(&mut config, 42, false).unwrap();
    assert!(config.auto_keep_settings.is_empty());
}

#[test]
fn waiting_states_retry_times_and_paused_state_are_projected_without_persistence() {
    let mut config = config();
    save_local_setting(&mut config, 42, true).unwrap();
    for (waiting, expected) in [
        (AutoKeepWaiting::None, "active"),
        (AutoKeepWaiting::Network, "waiting_network"),
        (AutoKeepWaiting::PeerOnline, "waiting_peer_online"),
    ] {
        for next in [0, 123] {
            let mut state = SharedRuntimeState::default();
            state.auto_keep_retries.insert(
                42,
                AutoKeepRetryRuntime {
                    waiting,
                    next_retry_at: next,
                    last_trigger_at: Some(99),
                    ..Default::default()
                },
            );
            state
                .auto_keep_retries
                .insert(7, AutoKeepRetryRuntime::default());
            let (tx, _) = tokio::sync::broadcast::channel(4);
            state.status_tx = Some(tx);
            apply_local_settings(&mut state, &config);
            assert!(!state.auto_keep_retries.contains_key(&7));
            let projected = &state.status.auto_keep_statuses[0];
            assert_eq!(projected.state, expected);
            assert_eq!(
                projected.next_retry_at,
                if next > 0 { Some(next) } else { None }
            );
            assert_eq!(projected.updated_at, 99);
            assert_eq!(
                projected.last_trigger.as_deref(),
                Some("lifecycle_not_established")
            );
        }
    }
    config.auto_keep_settings[0].paused_remote_stop = true;
    let mut state = SharedRuntimeState::default();
    apply_local_settings(&mut state, &config);
    assert_eq!(
        state.status.auto_keep_statuses[0].state,
        "paused_remote_stop"
    );
    assert_eq!(
        state.status.auto_keep_statuses[0].stop_reason.as_deref(),
        Some("remote_manual_disconnect")
    );
}

#[test]
fn jitter_and_failure_saturation_remain_bounded() {
    for (failures, base) in [
        (0, 5),
        (1, 5),
        (2, 10),
        (3, 20),
        (4, 30),
        (5, 60),
        (6, 120),
        (7, 300),
        (u32::MAX, 300),
    ] {
        let delay = jittered_backoff_secs(failures);
        assert!((base * 80 / 100..=base * 120 / 100).contains(&delay));
    }
    let shared = Arc::new(Mutex::new(SharedRuntimeState::default()));
    let mut config = config();
    save_local_setting(&mut config, 42, true).unwrap();
    apply_local_settings(&mut shared.lock(), &config);
    for (waiting, expected) in [
        (AutoKeepWaiting::Network, "waiting_network"),
        (AutoKeepWaiting::PeerOnline, "waiting_peer_online"),
    ] {
        defer_auto_keep_probe(&shared, 42, waiting, 1000);
        let state = shared.lock();
        assert_eq!(state.status.auto_keep_statuses[0].state, expected);
        assert!((1240..=1360).contains(&state.auto_keep_retries[&42].next_retry_at));
    }
    let mut state = shared.lock();
    state.auto_keep_retries.get_mut(&42).unwrap().failures = u32::MAX;
    note_auto_keep_job_failed(&mut state, 42, 2000);
    assert_eq!(state.auto_keep_retries[&42].failures, u32::MAX);
    note_auto_keep_job_failed(&mut state, 7, 2000);
    assert!(!state.auto_keep_retries.contains_key(&7));
    state.auto_keep_settings[0].paused_remote_stop = true;
    let previous = state.auto_keep_retries[&42].next_retry_at;
    note_auto_keep_job_failed(&mut state, 42, 9999);
    assert_eq!(state.auto_keep_retries[&42].next_retry_at, previous);
}
