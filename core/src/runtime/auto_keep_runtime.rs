use super::*;

const AUTO_KEEP_SYNC_SECS: u64 = 30;
const AUTO_KEEP_LOOP_SECS: u64 = 1;
const AUTO_KEEP_MAX_BACKOFF_SECS: u64 = 300;

#[derive(Debug, Clone, Default)]
pub(super) struct AutoKeepRetryRuntime {
    failures: u32,
    next_retry_at: i64,
}

pub(super) fn spawn_auto_keep_coordinator(shared: Arc<Mutex<SharedRuntimeState>>) {
    tokio::spawn(async move {
        let mut tick = interval(Duration::from_secs(AUTO_KEEP_LOOP_SECS));
        let mut last_sync_at = 0_i64;
        loop {
            tick.tick().await;
            if shared.lock().shutdown_requested {
                return;
            }
            let now = now_ts();
            if now - last_sync_at >= AUTO_KEEP_SYNC_SECS as i64 {
                if let Err(error) = refresh_auto_keep_statuses(&shared).await {
                    debug!("[AutoKeep] status sync skipped: {}", error);
                } else {
                    last_sync_at = now;
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
    let changed = {
        let mut state = shared.lock();
        let Some(status) = state
            .status
            .auto_keep_statuses
            .iter_mut()
            .find(|status| status.target_device_id == target_device_id && status.enabled)
        else {
            return;
        };
        status.state = "paused_remote_stop".to_string();
        status.stop_reason = Some("remote_manual_disconnect".to_string());
        status.updated_at = now_ts();
        state.auto_keep_retries.remove(&target_device_id);
        state
            .auto_keep_remote_stop_barriers
            .insert(target_device_id);
        let snapshot = state.status.clone();
        if let Some(tx) = state.status_tx.as_ref() {
            let _ = tx.send(snapshot);
        }
        true
    };
    if changed {
        match load_machine_config() {
            Ok(mut config) => {
                if !config
                    .auto_keep_remote_stop_targets
                    .contains(&target_device_id)
                {
                    config.auto_keep_remote_stop_targets.push(target_device_id);
                    if let Err(error) = save_machine_config(&config) {
                        warn!("[AutoKeep] local remote-stop persistence failed: {}", error);
                    }
                }
            }
            Err(error) => warn!("[AutoKeep] local remote-stop config load failed: {}", error),
        }
    }
}

pub(super) async fn refresh_auto_keep_statuses(
    shared: &Arc<Mutex<SharedRuntimeState>>,
) -> Result<Vec<AutoKeepStatus>> {
    if !shared.lock().status.logged_in {
        return Err(anyhow!("not logged in"));
    }
    let mut config = load_machine_config()?;
    {
        let mut state = shared.lock();
        state
            .auto_keep_remote_stop_barriers
            .extend(config.auto_keep_remote_stop_targets.iter().copied());
    }
    let source_device_id = config
        .device_id
        .ok_or_else(|| anyhow!("current device is not registered"))?;
    let mut statuses = list_auto_keep(&mut config, source_device_id).await?;
    let barriers = shared.lock().auto_keep_remote_stop_barriers.clone();
    for status in &mut statuses {
        if status.enabled && barriers.contains(&status.target_device_id) {
            status.state = "paused_remote_stop".to_string();
            status.stop_reason = Some("remote_manual_disconnect".to_string());
        }
    }
    update_status(shared, |runtime| {
        runtime.auto_keep_statuses = statuses.clone();
    });
    let enabled: std::collections::HashSet<i64> = statuses
        .iter()
        .filter(|status| status.enabled)
        .map(|status| status.target_device_id)
        .collect();
    shared
        .lock()
        .auto_keep_retries
        .retain(|target, _| enabled.contains(target));
    Ok(statuses)
}

pub(super) async fn set_auto_keep_setting(
    shared: &Arc<Mutex<SharedRuntimeState>>,
    target_device_id: i64,
    enabled: bool,
    expected_revision: Option<i64>,
) -> Option<Data> {
    let mut config = match load_config_or_err() {
        Ok(config) => config,
        Err(response) => return Some(response),
    };
    let source_device_id = match config.device_id {
        Some(device_id) => device_id,
        None => {
            return Some(cmd_response(
                false,
                "current device is not registered",
                None,
            ))
        }
    };
    match set_auto_keep(
        &mut config,
        source_device_id,
        target_device_id,
        enabled,
        expected_revision,
    )
    .await
    {
        Ok(status) => {
            shared
                .lock()
                .auto_keep_remote_stop_barriers
                .remove(&target_device_id);
            if config
                .auto_keep_remote_stop_targets
                .contains(&target_device_id)
            {
                config
                    .auto_keep_remote_stop_targets
                    .retain(|target| *target != target_device_id);
                if let Err(error) = save_machine_config(&config) {
                    warn!(
                        "[AutoKeep] clearing local remote-stop pause failed: {}",
                        error
                    );
                }
            }
            update_status(shared, |runtime| {
                if let Some(current) = runtime
                    .auto_keep_statuses
                    .iter_mut()
                    .find(|item| item.target_device_id == target_device_id)
                {
                    *current = status.clone();
                } else {
                    runtime.auto_keep_statuses.push(status.clone());
                }
            });
            if !enabled {
                shared.lock().auto_keep_retries.remove(&target_device_id);
            }
            // Opening the setting only triggers work when the lifecycle is already
            // not established. Other states are left untouched and observed later.
            if enabled {
                if let Err(error) = evaluate_auto_keep(shared).await {
                    debug!("[AutoKeep] post-save evaluation skipped: {}", error);
                }
            }
            Some(cmd_response_with_data(
                true,
                "auto keep setting saved",
                Some(shared.lock().status.clone()),
                serde_json::to_value(status).unwrap_or(serde_json::Value::Null),
            ))
        }
        Err(error) => Some(cmd_response(
            false,
            &error.to_string(),
            Some(shared.lock().status.clone()),
        )),
    }
}

async fn evaluate_auto_keep(shared: &Arc<Mutex<SharedRuntimeState>>) -> Result<()> {
    let now = now_ts();
    let target = {
        let mut state = shared.lock();
        if !state.status.logged_in || !state.status.ws_connected {
            return Ok(());
        }
        // A successful connection starts a fresh retry series. Without this,
        // a later unrelated outage would inherit an old, unnecessarily long delay.
        let connected_targets = state
            .status
            .tunnel_lifecycles
            .iter()
            .filter(|lifecycle| lifecycle.state == TunnelLifecycleState::Connected)
            .map(|lifecycle| lifecycle.peer_device_id)
            .collect::<Vec<_>>();
        for target_device_id in connected_targets {
            state.auto_keep_retries.remove(&target_device_id);
            if let Some(status) = state
                .status
                .auto_keep_statuses
                .iter_mut()
                .find(|status| status.target_device_id == target_device_id)
            {
                status.next_retry_at = None;
            }
        }
        // Global recovery concurrency is one. Manual and automatic active jobs
        // share the same slot, so automatic recovery never competes with a user job.
        if !state.active_tunnel_job_cancels.is_empty() {
            return Ok(());
        }
        state
            .status
            .auto_keep_statuses
            .iter()
            .filter(|status| status.enabled && status.state == "active")
            .find(|status| {
                let lifecycle = state
                    .status
                    .tunnel_lifecycles
                    .iter()
                    .find(|lifecycle| lifecycle.peer_device_id == status.target_device_id);
                let not_established = lifecycle
                    .map(|lifecycle| lifecycle.state == TunnelLifecycleState::NotEstablished)
                    .unwrap_or(true);
                let retry_ready = state
                    .auto_keep_retries
                    .get(&status.target_device_id)
                    .map(|retry| retry.next_retry_at <= now)
                    .unwrap_or(true);
                not_established && retry_ready
            })
            .map(|status| status.target_device_id)
    };
    let Some(target_device_id) = target else {
        return Ok(());
    };

    let mut config = load_machine_config()?;
    let devices = match get_device_list(&mut config).await {
        Ok(devices) => devices,
        Err(error) => {
            defer_auto_keep_probe(shared, target_device_id, "waiting_network", now);
            return Err(error);
        }
    };
    let Some(device) = devices
        .into_iter()
        .find(|device| device.device_id == target_device_id)
    else {
        defer_auto_keep_probe(shared, target_device_id, "waiting_peer_online", now);
        return Ok(());
    };
    if device.status != "online" {
        defer_auto_keep_probe(shared, target_device_id, "waiting_peer_online", now);
        return Ok(());
    }

    let lan_cidrs = if config.wgvpn_lan_access_enabled {
        config.wgvpn_lan_cidrs.clone()
    } else {
        Vec::new()
    };
    let response = start_active_tunnel_job(
        shared,
        device.device_id,
        device.device_uuid,
        None,
        None,
        lan_cidrs,
    );
    let started = matches!(response, Some(Data::CommandResponse { ok: true, .. }));
    if started {
        let mut state = shared.lock();
        state.auto_keep_retries.remove(&target_device_id);
        if let Some(status) = state
            .status
            .auto_keep_statuses
            .iter_mut()
            .find(|status| status.target_device_id == target_device_id)
        {
            status.last_trigger = Some("lifecycle_not_established".to_string());
            status.next_retry_at = None;
            status.updated_at = now;
        }
        let snapshot = state.status.clone();
        if let Some(tx) = state.status_tx.as_ref() {
            let _ = tx.send(snapshot);
        }
    }
    Ok(())
}

fn defer_auto_keep_probe(
    shared: &Arc<Mutex<SharedRuntimeState>>,
    target_device_id: i64,
    execution_state: &str,
    now: i64,
) {
    let mut state = shared.lock();
    let next_retry_at = now + jittered_backoff_secs(u32::MAX) as i64;
    state
        .auto_keep_retries
        .entry(target_device_id)
        .or_default()
        .next_retry_at = next_retry_at;
    if let Some(status) = state
        .status
        .auto_keep_statuses
        .iter_mut()
        .find(|status| status.target_device_id == target_device_id)
    {
        status.state = execution_state.to_string();
        status.next_retry_at = Some(next_retry_at);
        status.updated_at = now;
    }
}

pub(super) fn note_auto_keep_job_failed(
    state: &mut SharedRuntimeState,
    target_device_id: i64,
    failed_at: i64,
) {
    let enabled = state.status.auto_keep_statuses.iter().any(|status| {
        status.target_device_id == target_device_id && status.enabled && status.state == "active"
    });
    if !enabled {
        return;
    }
    let retry = state.auto_keep_retries.entry(target_device_id).or_default();
    retry.failures = retry.failures.saturating_add(1);
    retry.next_retry_at = failed_at + jittered_backoff_secs(retry.failures) as i64;
    if let Some(status) = state
        .status
        .auto_keep_statuses
        .iter_mut()
        .find(|status| status.target_device_id == target_device_id)
    {
        status.next_retry_at = Some(retry.next_retry_at);
        status.updated_at = failed_at;
    }
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
    let percent = rand::thread_rng().gen_range(80_u64..=120_u64);
    (base * percent / 100).max(1)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn retry_backoff_is_bounded_and_jittered() {
        for _ in 0..100 {
            let value = jittered_backoff_secs(20);
            assert!((240..=360).contains(&value));
        }
    }

    #[test]
    fn failed_job_starts_backoff_from_failure_time() {
        let mut state = SharedRuntimeState::default();
        state.status.auto_keep_statuses.push(AutoKeepStatus {
            target_device_id: 42,
            enabled: true,
            state: "active".to_string(),
            revision: 1,
            eligible: true,
            ineligible_reason: None,
            last_trigger: None,
            stop_reason: None,
            error_code: None,
            next_retry_at: None,
            updated_at: 0,
        });

        note_auto_keep_job_failed(&mut state, 42, 1_000);

        let retry = state.auto_keep_retries.get(&42).expect("retry state");
        assert_eq!(retry.failures, 1);
        assert!((1_004..=1_006).contains(&retry.next_retry_at));
        assert_eq!(
            state.status.auto_keep_statuses[0].next_retry_at,
            Some(retry.next_retry_at)
        );
    }
}
