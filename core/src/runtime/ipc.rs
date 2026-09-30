//! IPC server, client connection handling, and command response helpers.

use super::*;

fn control_secret_matches(received: &str, expected: &str) -> bool {
    if received.len() != expected.len() {
        return false;
    }
    received
        .as_bytes()
        .iter()
        .zip(expected.as_bytes())
        .fold(0_u8, |difference, (left, right)| {
            difference | (left ^ right)
        })
        == 0
}

pub(super) fn spawn_control_server(shared: Arc<Mutex<SharedRuntimeState>>, wake: Arc<Notify>) {
    tokio::spawn(async move {
        let wake_for_shutdown = wake.clone();
        if let Err(err) = control_server_loop(shared, wake).await {
            error!("[ServiceControl] {}", err);
            request_shutdown();
            wake_for_shutdown.notify_one();
        }
    });
}

pub(super) async fn control_server_loop(
    shared: Arc<Mutex<SharedRuntimeState>>,
    wake: Arc<Notify>,
) -> Result<()> {
    loop {
        let stream = accept_ipc_client().await?;

        let shared = shared.clone();
        let wake = wake.clone();

        // 每个连接 spawn 独立 task
        tokio::spawn(async move {
            if let Err(err) = handle_client(stream, &shared, &wake).await {
                tracing::debug!("[ServiceControl] client handler error: {}", err);
            }
        });
    }
}

pub(super) async fn handle_client(
    stream: IpcStream,
    shared: &Arc<Mutex<SharedRuntimeState>>,
    wake: &Arc<Notify>,
) -> Result<()> {
    use tokio::time::timeout;

    let mut conn = Connection::new(stream);

    // 握手：等待客户端发送 Data::Handshake
    let handshake = match timeout(Duration::from_secs(5), conn.next()).await {
        Ok(Ok(Some(Data::Handshake { secret }))) => secret,
        Ok(Ok(Some(_))) => return Err(anyhow!("expected handshake, got other message")),
        Ok(Ok(None)) => return Err(anyhow!("no handshake received")),
        Ok(Err(e)) => return Err(e),
        Err(_) => return Err(anyhow!("handshake timeout")),
    };

    let _ = ensure_machine_config()?;
    let expected_secret = derive_control_secret();

    if !control_secret_matches(&handshake, &expected_secret) {
        conn.send(&Data::CommandResponse {
            ok: false,
            message: "unauthorized".to_string(),
            status: None,
            data: None,
        })
        .await
        .ok();
        return Err(anyhow!("unauthorized control connection"));
    }

    // 握手成功
    tracing::debug!("[ServiceControl] client connected and authenticated");
    conn.send(&Data::CommandResponse {
        ok: true,
        message: "ok".to_string(),
        status: None,
        data: None,
    })
    .await
    .context("failed to send handshake ack")?;

    // 订阅状态变化广播
    let mut status_rx = shared
        .lock()
        .status_tx()
        .map(|tx| tx.subscribe())
        .unwrap_or_else(|| {
            let (tx, rx) = tokio::sync::broadcast::channel(1);
            drop(tx);
            rx
        });

    // 消息循环
    loop {
        tokio::select! {
            // 客户端命令
            msg = conn.next() => {
                match msg? {
                    Some(data) => {
                        let command_name = data_variant(&data);
                        tracing::debug!("[ServiceControl] received: {:?}", command_name);
                        let response = handle_data(data, shared).await;
                        // 批量唤醒：多条命令只触发一次 bootstrap / shutdown
                        {
                            let state = shared.lock();
                            if state.reconnect_requested || state.shutdown_requested {
                                wake.notify_one();
                            }
                        }
                        if let Some(resp) = response {
                            conn.send(&resp).await.ok();
                        }
                    }
                    None => {
                        break;
                    }
                }
            }
            // 服务端状态变化推送
            status = status_rx.recv() => {
                match status {
                    Ok(status) => {
                        conn.send(&Data::StatusChanged(status)).await.ok();
                    }
                    Err(tokio::sync::broadcast::error::RecvError::Lagged(_)) => {
                        // 消息太多跳过，下次继续
                    }
                    Err(_) => break,
                }
            }
        }
    }

    Ok(())
}

#[cfg(test)]
mod handshake_tests {
    use super::control_secret_matches;

    #[test]
    fn ipc_handshake_accepts_only_the_exact_secret() {
        assert!(control_secret_matches(
            "0123456789abcdef",
            "0123456789abcdef"
        ));
        assert!(!control_secret_matches(
            "0123456789abcdee",
            "0123456789abcdef"
        ));
        assert!(!control_secret_matches("short", "0123456789abcdef"));
    }
}

pub(super) fn cmd_response(ok: bool, message: &str, status: Option<RuntimeStatus>) -> Data {
    Data::CommandResponse {
        ok,
        message: message.to_string(),
        status,
        data: None,
    }
}

/// 构造带结构化数据（如设备列表）的命令响应。
pub(super) fn cmd_response_with_data(
    ok: bool,
    message: &str,
    status: Option<RuntimeStatus>,
    data: serde_json::Value,
) -> Data {
    Data::CommandResponse {
        ok,
        message: message.to_string(),
        status,
        data: Some(data),
    }
}

/// 加载 machine config；失败时返回错误响应（供 handle_data 用 `?`-like 早返回）。
///
/// 消除各 handler 重复的 `match load_machine_config() { Ok(c)=>c, Err(e)=>return Some(cmd_response(false,...)) }` 样板。
pub(super) fn load_config_or_err() -> Result<crate::config::MachineConfig, Data> {
    load_machine_config().map_err(|e| cmd_response(false, &e.to_string(), None))
}

pub(super) fn normalize_lan_cidrs_for_config(
    cidrs: Vec<String>,
) -> std::result::Result<Vec<String>, String> {
    let mut normalized = Vec::new();
    for raw in cidrs {
        let item = raw.trim();
        if item.is_empty() {
            continue;
        }
        let item = normalize_ipv4_cidr_for_config(item)?;
        if !normalized.iter().any(|existing| existing == &item) {
            normalized.push(item);
        }
    }
    Ok(normalized)
}

pub(super) fn normalize_ipv4_cidr_for_config(cidr: &str) -> std::result::Result<String, String> {
    let (ip, bits) =
        crate::wgvpn_exchange::parse_ipv4_cidr(cidr).map_err(|e| format!("{}: {}", e, cidr))?;
    let mask = crate::wgvpn_exchange::ipv4_mask_from_prefix(bits);
    let network = (ip & mask).to_be_bytes();
    Ok(format!(
        "{}.{}.{}.{}/{}",
        network[0], network[1], network[2], network[3], bits
    ))
}

/// 把 anyhow::Result 统一包装成 cmd_response。
/// Ok(()) → 成功响应；Err → 失败响应。
pub(super) fn response_from_result<T>(
    result: Result<T>,
    ok_data: impl FnOnce(T) -> serde_json::Value,
) -> Data {
    match result {
        Ok(v) => cmd_response_with_data(true, "ok", None, ok_data(v)),
        Err(e) => cmd_response(false, &e.to_string(), None),
    }
}

async fn resume_saved_token_session(
    shared: &Arc<Mutex<SharedRuntimeState>>,
    auto_login: bool,
) -> Result<serde_json::Value> {
    let mut config = load_machine_config()?;
    if !config.remember_me || config.refresh_token.is_none() {
        return Err(anyhow!("no saved session"));
    }

    let bundle = refresh_with_config(&mut config).await?;
    config.auto_login = auto_login;
    save_machine_config(&config)?;

    let user_json = serde_json::to_value(&bundle.user).unwrap_or(serde_json::Value::Null);
    update_status(shared, |s| {
        s.logged_in = true;
    });
    {
        let mut state = shared.lock();
        state.current_user_id = Some(bundle.user.user_id);
        state.login_session_enabled = true;
        state.reconnect_requested = true;
    }
    Ok(serde_json::json!({ "logged_in": true, "user": user_json }))
}

pub(super) fn data_variant(data: &Data) -> &'static str {
    match data {
        Data::Handshake { .. } => "Handshake",
        Data::Ping => "Ping",
        Data::Status => "Status",
        Data::RegisterDevice => "RegisterDevice",
        Data::StartActiveTunnelJob { .. } => "StartActiveTunnelJob",
        Data::StartAnonymousActiveTunnelJob { .. } => "StartAnonymousActiveTunnelJob",
        Data::StopActiveTunnelJob { .. } => "StopActiveTunnelJob",
        Data::StartDesktopSession { .. } => "StartDesktopSession",
        Data::StopDesktopSession { .. } => "StopDesktopSession",
        Data::ApproveInboundTunnel { .. } => "ApproveInboundTunnel",
        Data::RejectInboundTunnel { .. } => "RejectInboundTunnel",
        Data::RefreshTunnelStatus => "RefreshTunnelStatus",
        Data::RefreshNetworkInfo => "RefreshNetworkInfo",
        Data::ReloadConfig => "ReloadConfig",
        Data::UpdateAuth => "UpdateAuth",
        Data::Logout => "Logout",
        Data::AcknowledgeDeviceIdentityNotification => "AcknowledgeDeviceIdentityNotification",
        Data::StopTunnel { .. } => "StopTunnel",
        Data::StopActiveTunnel { .. } => "StopActiveTunnel",
        Data::TestTunnelSpeed { .. } => "TestTunnelSpeed",
        Data::ShutdownGracefully => "ShutdownGracefully",
        Data::GetDeviceList => "GetDeviceList",
        Data::UpdateDeviceAlias { .. } => "UpdateDeviceAlias",
        Data::DeleteDevice { .. } => "DeleteDevice",
        Data::WakeDevice { .. } => "WakeDevice",
        Data::SetDevicePassword { .. } => "SetDevicePassword",
        Data::GenerateConnectCode { .. } => "GenerateConnectCode",
        Data::MarkCurrentDeviceOffline => "MarkCurrentDeviceOffline",
        Data::Login { .. } => "Login",
        Data::RegisterByEmailCode { .. } => "RegisterByEmailCode",
        Data::SendVerificationCode { .. } => "SendVerificationCode",
        Data::ResetPasswordByEmailCode { .. } => "ResetPasswordByEmailCode",
        Data::CheckUpdate { .. } => "CheckUpdate",
        Data::TryAutoLogin => "TryAutoLogin",
        Data::ResumeSavedSession { .. } => "ResumeSavedSession",
        Data::GetUserProfile => "GetUserProfile",
        Data::GetInviteInfo => "GetInviteInfo",
        Data::SaveLoginSettings { .. } => "SaveLoginSettings",
        Data::GetSavedLogin => "GetSavedLogin",
        Data::GetLoginPreferences => "GetLoginPreferences",
        Data::SetAutoStartConfig { .. } => "SetAutoStartConfig",
        Data::SetLocale { .. } => "SetLocale",
        Data::GetWgvpnLanAccessConfig => "GetWgvpnLanAccessConfig",
        Data::GetConnectionPreferences => "GetConnectionPreferences",
        Data::SaveConnectionPreferences { .. } => "SaveConnectionPreferences",
        Data::SaveWgvpnLanAccessConfig { .. } => "SaveWgvpnLanAccessConfig",
        Data::CommandResponse { .. } => "CommandResponse",
        Data::StatusChanged(_) => "StatusChanged",
    }
}
pub(super) async fn handle_data(
    data: Data,
    shared: &Arc<Mutex<SharedRuntimeState>>,
) -> Option<Data> {
    match data {
        Data::Ping => Some(cmd_response(true, "pong", None)),
        Data::Status => Some(cmd_response(true, "ok", Some(shared.lock().status.clone()))),
        Data::RegisterDevice => {
            let mut config = match load_machine_config() {
                Ok(config) => config,
                Err(err) => {
                    return Some(cmd_response(
                        false,
                        &err.to_string(),
                        Some(shared.lock().status.clone()),
                    ));
                }
            };
            match register_current_device_auto(&mut config).await {
                Ok(device) => {
                    let status_report = collect_device_status_report(&mut config).await;
                    let heartbeat_result = match status_report {
                        Ok(report) => send_device_status_report(&mut config, &report).await,
                        Err(err) => Err(err),
                    };
                    let device_json =
                        serde_json::to_value(&device).unwrap_or(serde_json::Value::Null);
                    update_status(shared, |s| {
                        s.logged_in = true;
                        s.device_id = Some(device.device_id);
                        s.device_uuid = Some(device.device_uuid.clone());
                        s.current_device = Some(device);
                        if heartbeat_result.is_ok() {
                            s.last_heartbeat_at = Some(now_ts());
                        }
                    });
                    shared.lock().reconnect_requested = true;
                    match heartbeat_result {
                        Ok(()) => Some(cmd_response_with_data(
                            true,
                            "device registered",
                            Some(shared.lock().status.clone()),
                            device_json,
                        )),
                        Err(err) => {
                            set_last_error(
                                shared,
                                format!("device registered but heartbeat failed: {}", err),
                            );
                            Some(cmd_response(
                                false,
                                &format!("device registered but heartbeat failed: {}", err),
                                Some(shared.lock().status.clone()),
                            ))
                        }
                    }
                }
                Err(err) => Some(cmd_response(
                    false,
                    &err.to_string(),
                    Some(shared.lock().status.clone()),
                )),
            }
        }
        Data::StartActiveTunnelJob {
            target_device_id,
            target_device_uuid,
            connect_code,
            temporary_password,
            lan_cidrs,
        } => start_active_tunnel_job(
            shared,
            target_device_id,
            target_device_uuid,
            connect_code,
            temporary_password,
            lan_cidrs,
        ),
        Data::StartAnonymousActiveTunnelJob {
            connect_code,
            temporary_password,
        } => start_anonymous_active_tunnel_job(shared, connect_code, temporary_password).await,
        Data::StopActiveTunnelJob { target_device_id } => {
            stop_active_tunnel_job(shared, target_device_id)
        }
        Data::StartDesktopSession {
            peer_device_id,
            rustdesk_tiny_port,
        } => {
            let started = Instant::now();
            info!(
                peer_device_id,
                "[Desktop] service received StartDesktopSession"
            );
            let result = desktop_engine::start_active_desktop_session(
                shared,
                peer_device_id,
                rustdesk_tiny_port,
            )
            .await;
            match &result {
                Ok(_) => info!(
                    peer_device_id,
                    elapsed_ms = started.elapsed().as_millis(),
                    "[Desktop] service completed StartDesktopSession"
                ),
                Err(error) => {
                    error!(peer_device_id, elapsed_ms = started.elapsed().as_millis(), error = %error,
                    "[Desktop] service failed StartDesktopSession")
                }
            }
            Some(response_from_result(result, |value| value))
        }
        Data::StopDesktopSession { peer_device_id } => Some(response_from_result(
            desktop_engine::stop_desktop_session(shared, peer_device_id, "user_requested").await,
            |_| serde_json::json!({ "peer_device_id": peer_device_id }),
        )),
        Data::ApproveInboundTunnel { attempt_id } => {
            resolve_inbound_approval(shared, &attempt_id, true)
        }
        Data::RejectInboundTunnel { attempt_id } => {
            resolve_inbound_approval(shared, &attempt_id, false)
        }
        Data::RefreshTunnelStatus => {
            refresh_wgvpn_sessions_with_options(shared, false, true);
            Some(cmd_response(
                true,
                "tunnel status refreshed",
                Some(shared.lock().status.clone()),
            ))
        }
        Data::RefreshNetworkInfo => match refresh_network_info(shared).await {
            Ok(public_ip) => Some(cmd_response_with_data(
                true,
                "network info refreshed",
                Some(shared.lock().status.clone()),
                serde_json::json!({ "public_ip": public_ip }),
            )),
            Err(err) => Some(cmd_response(
                false,
                &err.to_string(),
                Some(shared.lock().status.clone()),
            )),
        },
        // --- 设备管理命令：统一由 service 持有 token 调服务器 ---
        Data::GetDeviceList => {
            let mut config = match load_config_or_err() {
                Ok(c) => c,
                Err(resp) => return Some(resp),
            };
            match get_device_list(&mut config).await {
                Ok(mut list) => {
                    // 旧服务端尚未返回 public_ip_location 时，当前设备仍可从本机
                    // 已验证的缓存恢复显示；远端设备必须继续以服务端上报值为准。
                    if let (Some(current_device_id), Some(location)) = (
                        config.device_id,
                        config
                            .cached_public_ip_location
                            .clone()
                            .filter(|value| !value.trim().is_empty()),
                    ) {
                        if let Some(current) = list
                            .iter_mut()
                            .find(|item| item.device_id == current_device_id)
                        {
                            if current
                                .public_ip_location
                                .as_deref()
                                .map_or(true, |value| value.trim().is_empty())
                            {
                                current.public_ip_location = Some(location);
                            }
                        }
                    }
                    if let Some(current) = config
                        .device_id
                        .and_then(|device_id| list.iter().find(|item| item.device_id == device_id))
                        .cloned()
                    {
                        update_status(shared, |status| {
                            status.public_ip = current.public_ip.clone();
                            status.current_device = Some(current);
                        });
                    }
                    Some(cmd_response_with_data(
                        true,
                        "ok",
                        Some(shared.lock().status.clone()),
                        serde_json::json!({ "code": 0, "msg": "ok", "data": list }),
                    ))
                }
                Err(err) => Some(cmd_response(false, &err.to_string(), None)),
            }
        }
        Data::UpdateDeviceAlias { device_id, alias } => {
            let mut config = match load_config_or_err() {
                Ok(c) => c,
                Err(resp) => return Some(resp),
            };
            Some(response_from_result(
                update_device_alias(&mut config, device_id, alias).await,
                |_| serde_json::Value::Null,
            ))
        }
        Data::DeleteDevice { device_id } => {
            let mut config = match load_config_or_err() {
                Ok(c) => c,
                Err(resp) => return Some(resp),
            };
            Some(response_from_result(
                delete_device(&mut config, device_id).await,
                |_| serde_json::Value::Null,
            ))
        }
        Data::WakeDevice { device_id } => {
            let mut config = match load_config_or_err() {
                Ok(c) => c,
                Err(resp) => return Some(resp),
            };
            Some(response_from_result(
                crate::device::wake_device(&mut config, device_id).await,
                |status| serde_json::json!({"status":status}),
            ))
        }
        Data::SetDevicePassword {
            device_id,
            password,
        } => {
            let mut config = match load_config_or_err() {
                Ok(c) => c,
                Err(resp) => return Some(resp),
            };
            let result = set_device_password(&mut config, device_id, password.clone()).await;
            // 本机设备的密码即邀请页展示的临时密码：成功后回写运行时状态并持久化，
            // 避免邀请页每次挂载（或 service 重启后）重新生成，使已分享未使用的邀请失效。
            if result.is_ok() && config.device_id == Some(device_id) && !password.is_empty() {
                config.invite_temporary_password = Some(password.clone());
                if let Err(err) = save_machine_config(&config) {
                    warn!("[ServiceControl] persist invite password failed: {}", err);
                }
                update_status(shared, |status| {
                    status.invite_temporary_password = Some(password);
                });
            }
            Some(response_from_result(result, |_| serde_json::Value::Null))
        }
        Data::GenerateConnectCode { device_id } => {
            let mut config = match load_config_or_err() {
                Ok(c) => c,
                Err(resp) => return Some(resp),
            };
            Some(response_from_result(
                generate_connect_code(&mut config, device_id).await,
                |code| serde_json::json!({ "connect_code": code }),
            ))
        }
        Data::MarkCurrentDeviceOffline => {
            let mut config = match load_config_or_err() {
                Ok(c) => c,
                Err(resp) => return Some(resp),
            };
            match mark_current_device_offline(&mut config).await {
                Ok(()) => {
                    // 仅通知服务器该设备下线，不改本地登录态（由后续 Data::Logout 处理）
                    Some(cmd_response(true, "offline", None))
                }
                Err(err) => Some(cmd_response(false, &err.to_string(), None)),
            }
        }
        // --- 认证命令：login/try_auto_login/profile/invite 统一由 service 处理 ---
        // 注意：登录响应**不返回真实 token**。service 是令牌唯一管理者，token 仅存
        // machine config。UI 用占位符 SERVICE_SESSION_PLACEHOLDER 表示"已登录"，
        // 不接触 access_token/refresh_token。
        Data::Login {
            identifier,
            password,
        } => match login_and_persist(&identifier, &password).await {
            Ok(bundle) => {
                let user_json =
                    serde_json::to_value(&bundle.user).unwrap_or(serde_json::Value::Null);
                update_status(shared, |s| {
                    s.logged_in = true;
                });
                {
                    let mut state = shared.lock();
                    state.current_user_id = Some(bundle.user.user_id);
                    state.login_session_enabled = true;
                    state.reconnect_requested = true;
                }
                Some(cmd_response_with_data(
                    true,
                    "login ok",
                    Some(shared.lock().status.clone()),
                    user_json,
                ))
            }
            Err(err) => Some(cmd_response(false, &err.to_string(), None)),
        },
        Data::RegisterByEmailCode {
            username,
            email,
            password,
            verification_code,
            invite_code,
        } => {
            let config = match load_config_or_err() {
                Ok(c) => c,
                Err(resp) => return Some(resp),
            };
            match register_by_email_code(
                &config.server_url,
                &username,
                &email,
                &password,
                &verification_code,
                invite_code.as_deref(),
            )
            .await
            {
                Ok(response) => {
                    let message = if response.code == 0 {
                        response.msg.clone()
                    } else {
                        response.localized_error_message(config.locale.as_deref())
                    };
                    Some(cmd_response_with_data(
                        true,
                        "ok",
                        None,
                        serde_json::json!({
                            "code": response.code,
                            "msg": message,
                            "data": response.data,
                        }),
                    ))
                }
                Err(err) => Some(cmd_response(false, &err.to_string(), None)),
            }
        }
        Data::SendVerificationCode { email, purpose } => {
            let config = match load_config_or_err() {
                Ok(c) => c,
                Err(resp) => return Some(resp),
            };
            match send_verification_code(&config.server_url, &email, &purpose).await {
                Ok(response) if response.code == 0 => Some(cmd_response_with_data(
                    true,
                    "ok",
                    None,
                    serde_json::json!({ "code": response.code, "msg": response.msg }),
                )),
                Ok(response) => Some(cmd_response(
                    false,
                    &response.localized_error_message(config.locale.as_deref()),
                    None,
                )),
                Err(err) => Some(cmd_response(false, &err.to_string(), None)),
            }
        }
        Data::ResetPasswordByEmailCode {
            email,
            verification_code,
            new_password,
        } => {
            let config = match load_config_or_err() {
                Ok(c) => c,
                Err(resp) => return Some(resp),
            };
            match reset_password_by_email_code(
                &config.server_url,
                &email,
                &verification_code,
                &new_password,
            )
            .await
            {
                Ok(response) if response.code == 0 => Some(cmd_response_with_data(
                    true,
                    "ok",
                    None,
                    serde_json::json!({ "code": response.code, "msg": response.msg }),
                )),
                Ok(response) => Some(cmd_response(
                    false,
                    &response.localized_error_message(config.locale.as_deref()),
                    None,
                )),
                Err(err) => Some(cmd_response(false, &err.to_string(), None)),
            }
        }
        Data::CheckUpdate { current_version } => {
            let config = match load_config_or_err() {
                Ok(c) => c,
                Err(resp) => return Some(resp),
            };
            match crate::update::fetch_version_policy(&config.server_url).await {
                Ok(data) => {
                    let evaluation =
                        crate::update::evaluate_version_policy(&current_version, &data);
                    Some(cmd_response_with_data(
                        true,
                        "ok",
                        None,
                        serde_json::json!({
                            "mode": evaluation.mode.to_string(),
                            "has_update": evaluation.has_update,
                            "force_update": evaluation.force_update,
                            "current": current_version,
                            "latest": data.latest_version,
                            "min_supported": data.min_supported_version,
                            "release_notes": data.release_notes,
                            "error": null,
                        }),
                    ))
                }
                Err(err) => Some(cmd_response(
                    false,
                    &err.localized_message(config.locale.as_deref()),
                    None,
                )),
            }
        }
        Data::TryAutoLogin => {
            // 仅在用户开启自动登录且有保存的 refresh token 时恢复会话。
            // 登出会保留"记住密码/自动登录"偏好（仅清除 token），此处对
            // 无 token 的情况按 skip 处理而不是报错。
            let config = match load_config_or_err() {
                Ok(c) => c,
                Err(resp) => return Some(resp),
            };
            if !config.remember_me || !config.auto_login || config.refresh_token.is_none() {
                return Some(cmd_response_with_data(
                    true,
                    "skip",
                    None,
                    serde_json::json!({ "logged_in": false }),
                ));
            }
            Some(response_from_result(
                resume_saved_token_session(shared, true).await,
                |data| data,
            ))
        }
        Data::ResumeSavedSession { auto_login } => Some(response_from_result(
            resume_saved_token_session(shared, auto_login).await,
            |data| data,
        )),
        Data::GetUserProfile => {
            let mut config = match load_config_or_err() {
                Ok(c) => c,
                Err(resp) => return Some(resp),
            };
            Some(response_from_result(
                fetch_profile(&mut config).await,
                |user| serde_json::to_value(&user).unwrap_or(serde_json::Value::Null),
            ))
        }
        Data::GetInviteInfo => {
            let mut config = match load_config_or_err() {
                Ok(c) => c,
                Err(resp) => return Some(resp),
            };
            Some(response_from_result(
                fetch_invite_info(&mut config).await,
                |invite| serde_json::to_value(&invite).unwrap_or(serde_json::Value::Null),
            ))
        }
        // --- 登录设置管理：machine config 受保护，统一由 service 读写 ---
        Data::SaveLoginSettings {
            identifier,
            remember_me,
            auto_login,
        } => {
            info!(
                "[ServiceRuntime] saving login preferences: remember_me={}, auto_login={}",
                remember_me, auto_login
            );
            let mut config = match load_config_or_err() {
                Ok(c) => c,
                Err(resp) => return Some(resp),
            };
            // identifier 为空时保留已有登录标识，兼容不修改账号的设置更新。
            if !identifier.trim().is_empty() {
                config.user_email = Some(identifier);
            }
            config.remember_me = remember_me;
            config.auto_login = auto_login;
            Some(response_from_result(save_machine_config(&config), |_| {
                serde_json::Value::Null
            }))
        }
        Data::GetSavedLogin => {
            let config = match load_config_or_err() {
                Ok(c) => c,
                Err(resp) => return Some(resp),
            };
            if !config.remember_me || config.refresh_token.is_none() {
                return Some(cmd_response_with_data(
                    true,
                    "none",
                    None,
                    serde_json::Value::Null,
                ));
            }
            let email = config.user_email.clone().unwrap_or_default();
            let auto_login = config.auto_login;
            Some(cmd_response_with_data(
                true,
                "ok",
                None,
                serde_json::json!({
                    "identifier": email,
                    "auto_login": auto_login,
                }),
            ))
        }
        Data::GetLoginPreferences => {
            let config = match load_config_or_err() {
                Ok(c) => c,
                Err(resp) => return Some(resp),
            };
            Some(cmd_response_with_data(
                true,
                "ok",
                None,
                serde_json::json!({
                    "remember_me": config.remember_me,
                    "auto_login": config.auto_login,
                    "auto_start": config.auto_start,
                }),
            ))
        }
        Data::SetAutoStartConfig { enabled } => {
            let mut config = match load_config_or_err() {
                Ok(c) => c,
                Err(resp) => return Some(resp),
            };
            #[cfg(target_os = "linux")]
            let previous_enabled = match crate::service_control::query_service_status() {
                Ok(status) => status.enabled,
                Err(err) => return Some(cmd_response(false, &err.to_string(), None)),
            };
            #[cfg(target_os = "linux")]
            {
                let result = if enabled {
                    crate::service_control::enable_service()
                } else {
                    crate::service_control::disable_service()
                };
                if let Err(err) = result {
                    return Some(cmd_response(false, &err.to_string(), None));
                }
            }
            config.auto_start = enabled;
            let result = save_machine_config(&config);
            #[cfg(target_os = "linux")]
            if result.is_err() {
                let rollback = if previous_enabled {
                    crate::service_control::enable_service()
                } else {
                    crate::service_control::disable_service()
                };
                if let Err(err) = rollback {
                    error!("[ServiceRuntime] auto start rollback failed: {}", err);
                }
            }
            Some(response_from_result(
                result,
                |_| serde_json::json!({ "auto_start": enabled }),
            ))
        }
        Data::SetLocale { locale } => {
            // 规范化：只接受 zh-CN / en，其余一律归一到 zh-CN，避免脏值入库。
            let normalized = match locale.as_str() {
                "en" | "en-US" | "en-us" => "en".to_string(),
                _ => "zh-CN".to_string(),
            };
            let mut config = match load_config_or_err() {
                Ok(c) => c,
                Err(resp) => return Some(resp),
            };
            config.locale = Some(normalized.clone());
            match save_machine_config(&config) {
                Ok(()) => {
                    // Existing job/lifecycle messages are service-owned strings. Rebuild the
                    // current snapshot and broadcast it immediately so every UI switches as a
                    // whole instead of waiting for the next state transition.
                    update_status(shared, |status| {
                        status.locale = Some(normalized.clone());
                        relocalize_runtime_status(status);
                    });
                    Some(cmd_response_with_data(
                        true,
                        "ok",
                        None,
                        serde_json::json!({ "locale": normalized }),
                    ))
                }
                Err(err) => Some(cmd_response(false, &err.to_string(), None)),
            }
        }
        Data::GetConnectionPreferences => {
            let config = match load_config_or_err() { Ok(c) => c, Err(resp) => return Some(resp) };
            Some(cmd_response_with_data(true, "ok", None, serde_json::json!({
                "prefer_ipv6": config.prefer_ipv6, "prefer_tcp": config.prefer_tcp,
            })))
        }
        Data::SaveConnectionPreferences { prefer_ipv6, prefer_tcp } => {
            let mut config = match load_config_or_err() { Ok(c) => c, Err(resp) => return Some(resp) };
            config.prefer_ipv6 = prefer_ipv6;
            config.prefer_tcp = prefer_tcp;
            Some(response_from_result(save_machine_config(&config), |_| serde_json::json!({
                "prefer_ipv6": prefer_ipv6, "prefer_tcp": prefer_tcp,
            })))
        }
        Data::GetWgvpnLanAccessConfig => {
            let config = match load_config_or_err() {
                Ok(c) => c,
                Err(resp) => return Some(resp),
            };
            Some(cmd_response_with_data(
                true,
                "ok",
                None,
                serde_json::json!({
                    "enabled": config.wgvpn_lan_access_enabled,
                    "cidrs": config.wgvpn_lan_cidrs,
                }),
            ))
        }
        Data::SaveWgvpnLanAccessConfig { enabled, cidrs } => {
            let normalized = match normalize_lan_cidrs_for_config(cidrs) {
                Ok(items) => items,
                Err(err) => return Some(cmd_response(false, &err, None)),
            };
            if enabled && normalized.is_empty() {
                let msg = localized_message(
                    current_locale(shared).as_deref(),
                    "errors.lan_empty_when_enabled",
                    &[],
                );
                return Some(cmd_response(false, &msg, None));
            }
            let mut config = match load_config_or_err() {
                Ok(c) => c,
                Err(resp) => return Some(resp),
            };
            config.wgvpn_lan_access_enabled = enabled;
            config.wgvpn_lan_cidrs = if enabled {
                normalized.clone()
            } else {
                Vec::new()
            };
            Some(response_from_result(save_machine_config(&config), |_| {
                serde_json::json!({
                    "enabled": enabled,
                    "cidrs": if enabled { normalized } else { Vec::new() },
                })
            }))
        }
        Data::ReloadConfig => {
            let config = match load_machine_config() {
                Ok(config) => config,
                Err(err) => {
                    return Some(cmd_response(
                        false,
                        &format!("failed to reload config: {}", err),
                        Some(shared.lock().status.clone()),
                    ));
                }
            };
            if let Err(err) = crate::logging::reload_log_level(&config.log_level) {
                return Some(cmd_response(
                    false,
                    &err,
                    Some(shared.lock().status.clone()),
                ));
            }
            shared.lock().reconnect_requested = true;
            Some(cmd_response(
                true,
                "config reloaded",
                Some(shared.lock().status.clone()),
            ))
        }
        Data::UpdateAuth => {
            shared.lock().reconnect_requested = true;
            Some(cmd_response(
                true,
                "reload scheduled",
                Some(shared.lock().status.clone()),
            ))
        }
        Data::StopTunnel { source_device_id } => stop_wgvpn_job(shared, source_device_id).await,
        Data::StopActiveTunnel { target_device_id } => {
            if shared.lock().active_tunnel_job_cancels.contains_key(&target_device_id) {
                stop_active_tunnel_job(shared, target_device_id);
            } else {
                clear_active_tunnel_job_status(shared, target_device_id);
            }
            stop_wgvpn_job(shared, target_device_id).await
        }
        Data::TestTunnelSpeed { peer_device_id } => {
            let passive_health_server = {
                let state = shared.lock();
                let Some(session) = state
                    .status
                    .wgvpn_sessions
                    .iter()
                    .find(|session| session.peer_device_id == peer_device_id)
                    .cloned()
                else {
                    return Some(cmd_response(false, "tunnel is not connected", None));
                };
                if session.is_active {
                    None
                } else {
                    let Some(handle) = state.health_server_handle.clone() else {
                        return Some(cmd_response(
                            false,
                            "health control connection is unavailable",
                            None,
                        ));
                    };
                    Some(handle)
                }
            };

            if let Some(handle) = passive_health_server {
                let result = handle.request_peer_speed_test(peer_device_id).await;
                return Some(response_from_result(result, |value| {
                    serde_json::to_value(value).unwrap_or_default()
                }));
            }

            let (speed_tx, busy_flag) = {
                let state = shared.lock();
                let Some(control) = state.wgvpn_health_controls.get(&peer_device_id) else {
                    return Some(cmd_response(false, "active tunnel is not connected", None));
                };
                // 并发去重：若已有测速在跑，拒绝新请求，避免 unbounded channel
                // 堆积串行导致 health loop 长时间被占用（进而可能误触发断线）。
                if control
                    .speed_test_busy
                    .swap(true, std::sync::atomic::Ordering::SeqCst)
                {
                    return Some(cmd_response(false, "speed test already in progress", None));
                }
                (control.speed_tx.clone(), control.speed_test_busy.clone())
            };
            let (response_tx, response_rx) = oneshot::channel();
            if speed_tx
                .send(TunnelSpeedTestCommand {
                    response: response_tx,
                })
                .is_err()
            {
                // health loop 已退出，释放 busy 标志供下次重试。
                busy_flag.store(false, std::sync::atomic::Ordering::SeqCst);
                return Some(cmd_response(
                    false,
                    "health control connection is unavailable",
                    None,
                ));
            }
            let result = tokio::time::timeout(Duration::from_secs(40), response_rx)
                .await
                .map_err(|_| anyhow!("tunnel speed test timed out"))
                .and_then(|value| value.map_err(|_| anyhow!("health control connection closed")))
                .and_then(|value| value.map_err(anyhow::Error::msg));
            // 兜底：无论结果如何都释放 busy（health loop 正常路径也会释放，
            // 这里防止 timeout 竞态：runtime 超时了但 health loop 还在跑）。
            busy_flag.store(false, std::sync::atomic::Ordering::SeqCst);
            Some(response_from_result(result, |value| {
                serde_json::to_value(value).unwrap_or_default()
            }))
        }
        Data::Logout => {
            cancel_all_active_tunnel_jobs(shared);
            cancel_all_wgvpn_jobs(shared).await;
            let mut config = load_machine_config().ok()?;
            // 登出：清除凭据（token/登录标识/设备绑定），下次启动不会自动登录。
            // "记住密码/自动登录"是登录偏好，保留——静默清除会让用户的
            // 无人值守配置在登出后失效。
            clear_machine_credentials(&mut config);
            save_machine_config(&config).ok()?;
            {
                let mut state = shared.lock();
                state.current_user_id = None;
                state.login_session_enabled = false;
                state.reconnect_requested = true;
            }
            update_status(shared, |s| {
                s.logged_in = false;
                s.last_error = None;
            });
            Some(cmd_response(
                true,
                "logged out",
                Some(shared.lock().status.clone()),
            ))
        }
        Data::AcknowledgeDeviceIdentityNotification => {
            update_status(shared, |status| {
                status.device_identity_rebuilt = false;
                status.device_identity_message = None;
            });
            Some(cmd_response(
                true,
                "device identity notification acknowledged",
                Some(shared.lock().status.clone()),
            ))
        }
        Data::ShutdownGracefully => {
            shared.lock().shutdown_requested = true;
            Some(cmd_response(
                true,
                "shutdown scheduled",
                Some(shared.lock().status.clone()),
            ))
        }
        // 客户端不应发送这些，忽略
        Data::Handshake { .. } | Data::CommandResponse { .. } | Data::StatusChanged(_) => None,
    }
}
