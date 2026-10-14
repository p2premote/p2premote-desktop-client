use super::*;
use crate::config::MachineConfig;

const AUTO_KEEP_LOOP_SECS: u64 = 1;
const AUTO_KEEP_MAX_BACKOFF_SECS: u64 = 300;

#[derive(Debug, Clone, Default)]
pub(super) struct AutoKeepRetryRuntime {
    failures: u32,
    next_retry_at: i64,
    waiting: AutoKeepWaiting,
    last_trigger_at: Option<i64>,
}

#[derive(Debug, Clone, Copy, Default)]
enum AutoKeepWaiting {
    #[default]
    None,
    Network,
    PeerOnline,
}

fn rebuild_auto_keep_statuses(state: &mut SharedRuntimeState) {
    state.status.auto_keep_statuses = state
        .auto_keep_settings
        .iter()
        .map(|setting| {
            let mut status = AutoKeepStatus::stopped(setting.target_device_id);
            status.enabled = true;
            status.state = "active".into();
            if setting.paused_remote_stop {
                status.state = "paused_remote_stop".into();
                status.stop_reason = Some("remote_manual_disconnect".into());
            } else if let Some(retry) = state.auto_keep_retries.get(&setting.target_device_id) {
                status.state = match retry.waiting {
                    AutoKeepWaiting::None => "active",
                    AutoKeepWaiting::Network => "waiting_network",
                    AutoKeepWaiting::PeerOnline => "waiting_peer_online",
                }
                .into();
                status.next_retry_at = (retry.next_retry_at > 0).then_some(retry.next_retry_at);
                if let Some(triggered_at) = retry.last_trigger_at {
                    status.last_trigger = Some("lifecycle_not_established".into());
                    status.updated_at = triggered_at;
                }
            }
            status
        })
        .collect();
}

fn apply_local_settings(state: &mut SharedRuntimeState, config: &MachineConfig) {
    state.auto_keep_settings = config
        .auto_keep_settings
        .iter()
        .filter(|setting| setting.belongs_to(config) && setting.active_side)
        .cloned()
        .collect();
    state.auto_keep_retries.retain(|target, _| {
        state
            .auto_keep_settings
            .iter()
            .any(|setting| setting.target_device_id == *target)
    });
    rebuild_auto_keep_statuses(state);
    if let Some(tx) = &state.status_tx {
        let _ = tx.send(state.status.clone());
    }
}

pub(super) fn spawn_auto_keep_coordinator(shared: Arc<Mutex<SharedRuntimeState>>) {
    tokio::spawn(async move {
        let mut tick = interval(Duration::from_secs(AUTO_KEEP_LOOP_SECS));
        let mut loaded_scope = None;
        loop {
            tick.tick().await;
            let scope = {
                let state = shared.lock();
                if state.shutdown_requested {
                    return;
                }
                (state.status.logged_in && state.status.device_id.is_some()).then(|| {
                    (
                        state.config_server_url.clone(),
                        state.current_user_id,
                        state.status.device_id,
                    )
                })
            };
            let Some(scope) = scope else {
                loaded_scope = None;
                continue;
            };
            // Read local preferences once after login/device registration. There
            // is no cloud settings endpoint, sync timer, or entitlement refresh.
            if loaded_scope.as_ref() != Some(&scope) {
                match refresh_auto_keep_statuses(&shared).await {
                    Ok(_) => loaded_scope = Some(scope),
                    Err(error) => {
                        debug!("[AutoKeep] local settings load failed: {}", error);
                        continue;
                    }
                }
            }
            if let Err(error) = evaluate_auto_keep(&shared).await {
                debug!("[AutoKeep] evaluation skipped: {}", error);
            }
        }
    });
}

pub(super) fn mark_auto_keep_remote_stop(
    shared: &Arc<Mutex<SharedRuntimeState>>,
    target_device_id: i64,
) {
    {
        let mut state = shared.lock();
        let Some(setting) = state
            .auto_keep_settings
            .iter_mut()
            .find(|setting| setting.target_device_id == target_device_id)
        else {
            return;
        };
        setting.paused_remote_stop = true;
        state.auto_keep_retries.remove(&target_device_id);
        rebuild_auto_keep_statuses(&mut state);
        if let Some(tx) = &state.status_tx {
            let _ = tx.send(state.status.clone());
        }
    }
    if let Err(error) = crate::config::update_machine_settings(|config| {
        let index = config.auto_keep_settings.iter().position(|setting| {
            setting.target_device_id == target_device_id && setting.belongs_to(config)
        });
        if let Some(index) = index {
            config.auto_keep_settings[index].paused_remote_stop = true;
        }
        Ok(())
    }) {
        warn!("[AutoKeep] local remote-stop persistence failed: {}", error);
    }
}

pub(super) async fn refresh_auto_keep_statuses(
    shared: &Arc<Mutex<SharedRuntimeState>>,
) -> Result<Vec<AutoKeepStatus>> {
    let config = load_machine_config()?;
    let mut state = shared.lock();
    if !state.status.logged_in {
        return Err(anyhow!("not logged in"));
    }
    apply_local_settings(&mut state, &config);
    Ok(state.status.auto_keep_statuses.clone())
}

fn save_local_setting(
    config: &mut MachineConfig,
    target_device_id: i64,
    enabled: bool,
) -> Result<()> {
    let index = config.auto_keep_settings.iter().position(|setting| {
        setting.target_device_id == target_device_id && setting.belongs_to(config)
    });
    if let Some(index) = index {
        if enabled {
            config.auto_keep_settings[index].active_side = true;
            config.auto_keep_settings[index].paused_remote_stop = false;
        } else {
            config.auto_keep_settings.remove(index);
        }
    } else if enabled {
        config.auto_keep_settings.push(AutoKeepSetting {
            server_url: config.server_url.clone(),
            user_email: config
                .user_email
                .clone()
                .ok_or_else(|| anyhow!("not logged in"))?,
            source_device_uuid: config
                .device_uuid
                .clone()
                .ok_or_else(|| anyhow!("current device is not registered"))?,
            target_device_id,
            active_side: true,
            paused_remote_stop: false,
        });
    }
    Ok(())
}

pub(super) fn has_passive_tunnel(state: &SharedRuntimeState, target_device_id: i64) -> bool {
    state.passive_p2p_attempts.contains_key(&target_device_id)
        || state
            .status
            .wgvpn_sessions
            .iter()
            .any(|session| session.peer_device_id == target_device_id && !session.is_active)
        || state.status.tunnel_lifecycles.iter().any(|lifecycle| {
            lifecycle.peer_device_id == target_device_id
                && lifecycle.role == TunnelLifecycleRole::Passive
                && lifecycle.state != TunnelLifecycleState::NotEstablished
        })
}

fn may_enable_auto_keep(state: &SharedRuntimeState, target_device_id: i64) -> bool {
    if has_passive_tunnel(state, target_device_id) {
        return false;
    }
    state
        .auto_keep_settings
        .iter()
        .any(|setting| setting.target_device_id == target_device_id && setting.active_side)
        || state
            .status
            .wgvpn_sessions
            .iter()
            .any(|session| session.peer_device_id == target_device_id && session.is_active)
        || state.status.tunnel_lifecycles.iter().any(|lifecycle| {
            lifecycle.peer_device_id == target_device_id
                && lifecycle.role == TunnelLifecycleRole::Active
                && lifecycle.state == TunnelLifecycleState::Connected
        })
}

/// Accepting an inbound connection hands recovery ownership to its initiator.
/// Remove the local preference so it cannot restart in reverse after a reboot.
pub(super) async fn disable_auto_keep_for_passive(
    shared: &Arc<Mutex<SharedRuntimeState>>,
    target_device_id: i64,
) -> Result<()> {
    let task = {
        let mut state = shared.lock();
        if !state
            .auto_keep_settings
            .iter()
            .any(|setting| setting.target_device_id == target_device_id)
        {
            return Ok(());
        }
        crate::config::update_machine_settings(|config| {
            save_local_setting(config, target_device_id, false)
        })?;
        state
            .auto_keep_settings
            .retain(|setting| setting.target_device_id != target_device_id);
        state.auto_keep_retries.remove(&target_device_id);
        rebuild_auto_keep_statuses(&mut state);
        if let Some(tx) = &state.status_tx {
            let _ = tx.send(state.status.clone());
        }
        state
            .active_tunnel_job_cancels
            .get(&target_device_id)
            .cloned()
    };
    if let Some(task) = task {
        let _ = task.send(true);
        task.closed().await;
    }
    Ok(())
}

pub(super) async fn set_auto_keep_setting(
    shared: &Arc<Mutex<SharedRuntimeState>>,
    target_device_id: i64,
    enabled: bool,
) -> Option<Data> {
    {
        let mut state = shared.lock();
        if !state.status.logged_in {
            return Some(cmd_response(false, "not logged in", None));
        }
        if enabled && !may_enable_auto_keep(&state, target_device_id) {
            return Some(cmd_response(false, "AUTO_KEEP_ACTIVE_SIDE_REQUIRED", None));
        }
        let config = match crate::config::update_machine_settings(|config| {
            save_local_setting(config, target_device_id, enabled)
        }) {
            Ok(config) => config,
            Err(error) => return Some(cmd_response(false, &error.to_string(), None)),
        };
        state.auto_keep_retries.remove(&target_device_id);
        apply_local_settings(&mut state, &config);
    }
    if enabled {
        if let Err(error) = evaluate_auto_keep(shared).await {
            debug!("[AutoKeep] post-save evaluation skipped: {}", error);
        }
    }
    let snapshot = shared.lock().status.clone();
    let status = snapshot
        .auto_keep_statuses
        .iter()
        .find(|status| status.target_device_id == target_device_id)
        .cloned()
        .unwrap_or_else(|| AutoKeepStatus::stopped(target_device_id));
    Some(cmd_response_with_data(
        true,
        "auto keep setting saved locally",
        Some(snapshot),
        serde_json::to_value(status).unwrap_or(serde_json::Value::Null),
    ))
}

async fn evaluate_auto_keep(shared: &Arc<Mutex<SharedRuntimeState>>) -> Result<()> {
    let now = now_ts();
    let target = {
        let mut state = shared.lock();
        if !state.status.logged_in || !state.status.ws_connected {
            return Ok(());
        }
        let connected_targets = state
            .status
            .tunnel_lifecycles
            .iter()
            .filter(|lifecycle| lifecycle.state == TunnelLifecycleState::Connected)
            .map(|lifecycle| lifecycle.peer_device_id)
            .collect::<Vec<_>>();
        for target in connected_targets {
            state.auto_keep_retries.remove(&target);
        }
        rebuild_auto_keep_statuses(&mut state);
        if !state.active_tunnel_job_cancels.is_empty() {
            return Ok(());
        }
        state
            .auto_keep_settings
            .iter()
            .filter(|setting| {
                !setting.paused_remote_stop && !has_passive_tunnel(&state, setting.target_device_id)
            })
            .find(|setting| {
                let lifecycle = state
                    .status
                    .tunnel_lifecycles
                    .iter()
                    .find(|lifecycle| lifecycle.peer_device_id == setting.target_device_id);
                let not_established = lifecycle
                    .map(|lifecycle| lifecycle.state == TunnelLifecycleState::NotEstablished)
                    .unwrap_or(true);
                let retry_ready = state
                    .auto_keep_retries
                    .get(&setting.target_device_id)
                    .map(|retry| retry.next_retry_at <= now)
                    .unwrap_or(true);
                not_established && retry_ready
            })
            .map(|setting| setting.target_device_id)
    };
    let Some(target_device_id) = target else {
        return Ok(());
    };
    let mut config = load_machine_config()?;
    let devices = match get_device_list(&mut config).await {
        Ok(devices) => devices,
        Err(error) => {
            defer_auto_keep_probe(shared, target_device_id, AutoKeepWaiting::Network, now);
            return Err(error);
        }
    };
    let Some(device) = devices
        .into_iter()
        .find(|device| device.device_id == target_device_id)
    else {
        defer_auto_keep_probe(shared, target_device_id, AutoKeepWaiting::PeerOnline, now);
        return Ok(());
    };
    if device.status != "online" {
        defer_auto_keep_probe(shared, target_device_id, AutoKeepWaiting::PeerOnline, now);
        return Ok(());
    }
    let lan_cidrs = if config.wgvpn_lan_access_enabled {
        config.wgvpn_lan_cidrs
    } else {
        Vec::new()
    };
    let _ = start_auto_keep_tunnel_job(shared, device.device_id, device.device_uuid, lan_cidrs);
    Ok(())
}

fn defer_auto_keep_probe(
    shared: &Arc<Mutex<SharedRuntimeState>>,
    target_device_id: i64,
    waiting: AutoKeepWaiting,
    now: i64,
) {
    let mut state = shared.lock();
    let retry = state.auto_keep_retries.entry(target_device_id).or_default();
    retry.next_retry_at = now + jittered_backoff_secs(u32::MAX) as i64;
    retry.waiting = waiting;
    rebuild_auto_keep_statuses(&mut state);
    if let Some(tx) = &state.status_tx {
        let _ = tx.send(state.status.clone());
    }
}

pub(super) fn note_auto_keep_job_started(
    state: &mut SharedRuntimeState,
    target_device_id: i64,
    now: i64,
) {
    let retry = state.auto_keep_retries.entry(target_device_id).or_default();
    retry.next_retry_at = 0;
    retry.waiting = AutoKeepWaiting::None;
    retry.last_trigger_at = Some(now);
    rebuild_auto_keep_statuses(state);
}

pub(super) fn note_auto_keep_job_failed(
    state: &mut SharedRuntimeState,
    target_device_id: i64,
    failed_at: i64,
) {
    if !state
        .auto_keep_settings
        .iter()
        .any(|setting| setting.target_device_id == target_device_id && !setting.paused_remote_stop)
    {
        return;
    }
    let retry = state.auto_keep_retries.entry(target_device_id).or_default();
    retry.failures = retry.failures.saturating_add(1);
    retry.next_retry_at = failed_at + jittered_backoff_secs(retry.failures) as i64;
    rebuild_auto_keep_statuses(state);
}

fn jittered_backoff_secs(failures: u32) -> u64 {
    let base = match failures {
        0 | 1 => 5,
        2 => 10,
        3 => 20,
        4 => 30,
        5 => 60,
        6 => 120,
        _ => AUTO_KEEP_MAX_BACKOFF_SECS,
    };
    (base * rand::thread_rng().gen_range(80_u64..=120_u64) / 100).max(1)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn config() -> MachineConfig {
        MachineConfig {
            server_url: "https://example.test".into(),
            user_email: Some("a@example.test".into()),
            device_uuid: Some("source".into()),
            ..Default::default()
        }
    }

    #[test]
    fn only_the_established_active_side_can_enable_recovery() {
        let mut state = SharedRuntimeState::default();
        assert!(!may_enable_auto_keep(&state, 42));
        let active: TunnelLifecycleStatus = serde_json::from_value(serde_json::json!({
            "peer_device_id": 42, "role": "active", "state": "connected", "attempt": 1,
            "max_attempts": 30, "last_result": "none", "updated_at": 1,
        }))
        .unwrap();
        state.status.tunnel_lifecycles.push(active.clone());
        assert!(may_enable_auto_keep(&state, 42));
        let mut config = config();
        save_local_setting(&mut config, 42, true).unwrap();
        apply_local_settings(&mut state, &config);
        state.status.tunnel_lifecycles[0].state = TunnelLifecycleState::NotEstablished;
        assert!(may_enable_auto_keep(&state, 42)); // An existing active-side preference can resume.
        let mut passive = active;
        passive.role = TunnelLifecycleRole::Passive;
        state.status.tunnel_lifecycles.push(passive);
        assert!(!may_enable_auto_keep(&state, 42)); // Even with a saved active-side preference.
        assert!(has_passive_tunnel(&state, 42));
    }

    #[test]
    fn old_preferences_without_an_initiator_confirmation_are_not_executed() {
        let mut config = config();
        config.auto_keep_settings = serde_json::from_value(serde_json::json!([{
            "server_url": config.server_url, "user_email": config.user_email,
            "source_device_uuid": config.device_uuid, "target_device_id": 42,
        }]))
        .unwrap();
        let mut state = SharedRuntimeState::default();
        apply_local_settings(&mut state, &config);
        assert!(state.auto_keep_settings.is_empty());
        save_local_setting(&mut config, 42, true).unwrap();
        apply_local_settings(&mut state, &config);
        assert_eq!(state.auto_keep_settings.len(), 1);
    }

    #[tokio::test]
    async fn direct_enable_without_an_active_tunnel_is_rejected_before_persistence() {
        let shared = Arc::new(Mutex::new(SharedRuntimeState::default()));
        shared.lock().status.logged_in = true;
        assert!(matches!(set_auto_keep_setting(&shared, 42, true).await,
            Some(Data::CommandResponse { ok: false, message, .. }) if message == "AUTO_KEEP_ACTIVE_SIDE_REQUIRED"));
    }

    #[test]
    fn local_setting_roundtrip_and_account_switch_do_not_require_cloud_or_membership() {
        let mut config = config();
        save_local_setting(&mut config, 42, true).unwrap();
        config.auto_keep_settings[0].paused_remote_stop = true;
        let serialized = serde_json::to_string(&config).unwrap();
        let mut config: MachineConfig = serde_json::from_str(&serialized).unwrap();
        let mut state = SharedRuntimeState::default();
        apply_local_settings(&mut state, &config);
        assert_eq!(
            state.status.auto_keep_statuses[0].state,
            "paused_remote_stop"
        );
        save_local_setting(&mut config, 42, true).unwrap();
        apply_local_settings(&mut state, &config);
        assert_eq!(state.status.auto_keep_statuses[0].state, "active");
        config.user_email = Some("b@example.test".into());
        apply_local_settings(&mut state, &config);
        assert!(state.auto_keep_settings.is_empty());
        save_local_setting(&mut config, 42, true).unwrap();
        save_local_setting(&mut config, 42, false).unwrap();
        assert_eq!(config.auto_keep_settings.len(), 1);
        config.user_email = Some("a@example.test".into());
        apply_local_settings(&mut state, &config);
        assert_eq!(state.status.auto_keep_statuses[0].state, "active");
    }

    #[test]
    fn repeated_job_starts_preserve_the_full_failure_backoff_series() {
        let mut config = config();
        save_local_setting(&mut config, 42, true).unwrap();
        let mut state = SharedRuntimeState::default();
        apply_local_settings(&mut state, &config);
        for (index, base) in [5, 10, 20, 30, 60, 120, 300].into_iter().enumerate() {
            let now = 1000 + index as i64 * 1000;
            note_auto_keep_job_started(&mut state, 42, now);
            note_auto_keep_job_failed(&mut state, 42, now + 1);
            let retry = &state.auto_keep_retries[&42];
            assert_eq!(retry.failures, index as u32 + 1);
            assert!((base * 80 / 100..=base * 120 / 100).contains(&(retry.next_retry_at - now - 1)));
        }
    }

    #[test]
    fn loading_local_preferences_preserves_waiting_and_failure_state() {
        let mut config = config();
        save_local_setting(&mut config, 42, true).unwrap();
        let mut state = SharedRuntimeState::default();
        apply_local_settings(&mut state, &config);
        state.auto_keep_retries.insert(
            42,
            AutoKeepRetryRuntime {
                failures: 3,
                next_retry_at: 500,
                waiting: AutoKeepWaiting::Network,
                ..Default::default()
            },
        );
        apply_local_settings(&mut state, &config);
        assert_eq!(state.status.auto_keep_statuses[0].state, "waiting_network");
        assert_eq!(state.status.auto_keep_statuses[0].next_retry_at, Some(500));
        assert_eq!(state.auto_keep_retries[&42].failures, 3);
    }
}
