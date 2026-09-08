//! 认证模块
//!
//! 认证命令（login/logout/profile/invite/try_auto_login）统一通过 IPC 委托 service
//! 执行，service 是令牌的唯一管理者（持有令牌、负责持久化与续期）。UI 仅维护内存
//! 中的 AUTH_STATE 供前端展示。登录偏好也通过 IPC 委托 service 管理。

use tracing::{debug, error, info};

use super::service::send_command_responsive;
use p2premote_core::auth::{InviteInfo, UserInfo};
use p2premote_core::control::send_command;
use p2premote_core::control::Data;
use p2premote_core::service_control::stop_service;

/// 全局认证状态
static AUTH_STATE: once_cell::sync::Lazy<parking_lot::Mutex<AuthState>> =
    once_cell::sync::Lazy::new(|| parking_lot::Mutex::new(AuthState::default()));

#[derive(Default)]
pub struct AuthState {
    pub token: Option<String>,
    pub user_info: Option<UserInfo>,
}

/// service 是令牌唯一管理者，UI 不接触真实 token。
/// 该占位符用于 token.value 表示"已登录"语义，前端不应解析它。
pub const SERVICE_SESSION_PLACEHOLDER: &str = "__service_session__";

/// Tauri 命令：登录
///
/// 通过 IPC 委托 service 执行登录（service 持有令牌、负责持久化与续期）。
/// 响应**不含真实 token**，只返回 user 信息；UI 用占位符表示已登录。
#[tauri::command]
pub async fn login(identifier: String, password: String) -> Result<serde_json::Value, String> {
    debug!("Login request for identifier: {}", identifier);

    let resp = send_command_responsive(Data::Login {
        identifier: identifier.clone(),
        password,
    })
    .await?;

    let user_json = match resp {
        Data::CommandResponse {
            ok: true,
            data: Some(d),
            ..
        } => d,
        Data::CommandResponse {
            ok: false, message, ..
        } => {
            error!("[Login] service 登录失败: {}", message);
            return Err(message);
        }
        other => return Err(format!("unexpected service response: {:?}", other)),
    };

    // 从响应中提取 user info 更新内存状态（仅用于 UI 展示）
    let info = serde_json::from_value::<UserInfo>(user_json.clone())
        .map_err(|error| format!("invalid user information returned by service: {error}"))?;
    let mut auth = AUTH_STATE.lock();
    // 用占位符表示已登录，不存真实 token
    auth.token = Some(SERVICE_SESSION_PLACEHOLDER.to_string());
    auth.user_info = Some(info);

    info!("[Login] 登录成功: {}", identifier);
    // 返回结构对齐前端 LoginResponse，但 data 只含 user（不含 token）
    Ok(serde_json::json!({
        "code": 0,
        "msg": "ok",
        "data": {
            "user": user_json,
        },
    }))
}

/// Tauri 命令：登出
///
/// 委托 service 清理认证状态（service 的 Data::Logout 会彻底清除 machine config 的
/// token + 记住密码/自动登录标志、取消隧道、断开 WS）。
/// 登出即忘记此设备凭据——下次启动需重新登录。UI 仅清内存 AUTH_STATE。
#[tauri::command]
pub async fn logout() -> Result<(), String> {
    info!("Logout request");

    match send_command(Data::Logout)
        .await
        .map_err(|error| error.to_string())?
    {
        Data::CommandResponse { ok: true, .. } => {}
        Data::CommandResponse { message, .. } => return Err(message),
        other => return Err(format!("unexpected service response: {other:?}")),
    }
    stop_service().map_err(|error| error.to_string())?;

    let mut auth = AUTH_STATE.lock();
    auth.token = None;
    auth.user_info = None;

    Ok(())
}

/// Tauri 命令：使用邮件验证码注册。
#[tauri::command]
pub async fn register_by_email_code(
    username: String,
    email: String,
    password: String,
    verification_code: String,
    invite_code: Option<String>,
) -> Result<serde_json::Value, String> {
    debug!("RegisterByEmailCode request for: {}", email);

    let config = p2premote_core::config::load_machine_config()
        .map_err(|error| format!("failed to load machine config: {error}"))?;
    debug!(
        "[RegisterByEmailCode] server base_url: {}",
        config.server_url
    );
    let response = p2premote_core::auth::register_by_email_code(
        &config.server_url,
        &username,
        &email,
        &password,
        &verification_code,
        invite_code.as_deref(),
    )
    .await
    .map_err(|e| e.to_string())?;

    let message = if response.code == 0 {
        response.msg.clone()
    } else {
        response.localized_error_message(config.locale.as_deref())
    };
    Ok(serde_json::json!({
        "code": response.code,
        "msg": message,
    }))
}

#[tauri::command]
pub async fn send_registration_verification_code(
    email: String,
) -> Result<serde_json::Value, String> {
    let config = p2premote_core::config::load_machine_config()
        .map_err(|error| format!("failed to load machine config: {error}"))?;
    let response =
        p2premote_core::auth::send_verification_code(&config.server_url, &email, "register")
            .await
            .map_err(|e| e.to_string())?;
    if response.code != 0 {
        return Err(response.localized_error_message(config.locale.as_deref()));
    }
    Ok(serde_json::json!({ "code": response.code, "msg": response.msg }))
}

#[tauri::command]
pub async fn send_reset_password_verification_code(
    email: String,
) -> Result<serde_json::Value, String> {
    let config = p2premote_core::config::load_machine_config()
        .map_err(|error| format!("failed to load machine config: {error}"))?;
    let response =
        p2premote_core::auth::send_verification_code(&config.server_url, &email, "reset_password")
            .await
            .map_err(|e| e.to_string())?;
    if response.code != 0 {
        return Err(response.localized_error_message(config.locale.as_deref()));
    }
    Ok(serde_json::json!({ "code": response.code, "msg": response.msg }))
}

#[tauri::command]
pub async fn reset_password_by_email_code(
    email: String,
    verification_code: String,
    new_password: String,
) -> Result<serde_json::Value, String> {
    let config = p2premote_core::config::load_machine_config()
        .map_err(|error| format!("failed to load machine config: {error}"))?;
    let response = p2premote_core::auth::reset_password_by_email_code(
        &config.server_url,
        &email,
        &verification_code,
        &new_password,
    )
    .await
    .map_err(|e| e.to_string())?;
    if response.code != 0 {
        return Err(response.localized_error_message(config.locale.as_deref()));
    }
    Ok(serde_json::json!({ "code": response.code, "msg": response.msg }))
}

/// Tauri 命令：检查是否已登录
///
/// 先查内存 AUTH_STATE（登录后立即生效）；内存为空时通过 IPC 查 service 的
/// RuntimeStatus.logged_in（不读受保护的 machine config，避免权限失败返回假阴性）。
#[tauri::command]
pub async fn is_logged_in() -> Result<bool, String> {
    // 先检查内存状态（确保 MutexGuard 不跨 await）
    let in_memory = { AUTH_STATE.lock().token.is_some() };
    if in_memory {
        return Ok(true);
    }

    // 内存为空则查 service 运行时状态
    match send_command_responsive(Data::Status).await {
        Ok(Data::CommandResponse {
            status: Some(s), ..
        }) => Ok(s.logged_in),
        Ok(other) => Err(format!("unexpected service response: {other:?}")),
        Err(error) => Err(error),
    }
}

/// Tauri 命令：尝试自动登录
///
/// 委托 service 执行：若 remember_me/auto_login 开启，service 用 refresh token
/// 恢复会话。
/// 返回 SERVICE_SESSION_PLACEHOLDER（已登录）或空串（未登录），不返回真实 token。
#[tauri::command]
pub async fn try_auto_login() -> Result<String, String> {
    debug!("[TryAutoLogin] 委托 service 自动登录");
    let resp = send_command_responsive(Data::TryAutoLogin)
        .await
        .map_err(|error| {
            error!("[TryAutoLogin] IPC 失败: {}", error);
            error
        })?;

    let d = match resp {
        Data::CommandResponse {
            ok: true,
            data: Some(data),
            ..
        } => data,
        Data::CommandResponse {
            ok: false, message, ..
        } => return Err(message),
        other => return Err(format!("unexpected service response: {other:?}")),
    };

    let logged_in = d
        .get("logged_in")
        .and_then(|v| v.as_bool())
        .ok_or_else(|| "auto-login response missing boolean field: logged_in".to_string())?;
    if !logged_in {
        return Ok(String::new());
    }

    // 同步 user info 到内存状态
    let user = d
        .get("user")
        .ok_or_else(|| "auto-login response missing user information".to_string())?;
    let info = serde_json::from_value::<UserInfo>(user.clone())
        .map_err(|error| format!("invalid user information returned by service: {error}"))?;
    let mut auth = AUTH_STATE.lock();
    auth.token = Some(SERVICE_SESSION_PLACEHOLDER.to_string());
    auth.user_info = Some(info);
    info!("[TryAutoLogin] 自动登录成功");
    Ok(SERVICE_SESSION_PLACEHOLDER.to_string())
}

/// Tauri 命令：手动使用 service 已保存的 refresh token 恢复会话。
///
/// 登录页的圆点占位符只表示已有可恢复会话；密码本身从不返回 UI。
#[tauri::command]
pub async fn resume_saved_session(auto_login: bool) -> Result<(), String> {
    let resp = send_command_responsive(Data::ResumeSavedSession { auto_login }).await?;
    let d = match resp {
        Data::CommandResponse {
            ok: true,
            data: Some(d),
            ..
        } => d,
        Data::CommandResponse { message, .. } => return Err(message),
        other => return Err(format!("unexpected service response: {:?}", other)),
    };

    if !d
        .get("logged_in")
        .and_then(|v| v.as_bool())
        .ok_or_else(|| "saved-session response missing boolean field: logged_in".to_string())?
    {
        return Err("saved session was not resumed".to_string());
    }
    let user = d
        .get("user")
        .ok_or_else(|| "saved-session response missing user information".to_string())?;
    let info = serde_json::from_value::<UserInfo>(user.clone())
        .map_err(|error| format!("invalid user information returned by service: {error}"))?;
    let mut auth = AUTH_STATE.lock();
    auth.token = Some(SERVICE_SESSION_PLACEHOLDER.to_string());
    auth.user_info = Some(info);
    info!("[ResumeSavedSession] saved session resumed");
    Ok(())
}

/// Tauri 命令：获取用户信息
#[tauri::command]
pub fn get_user_info() -> Option<UserInfo> {
    let auth = AUTH_STATE.lock();
    auth.user_info.clone()
}

#[tauri::command]
pub async fn fetch_user_profile() -> Result<Option<UserInfo>, String> {
    debug!("[Profile] 通过 service 获取用户资料");
    let resp = send_command_responsive(Data::GetUserProfile).await?;
    match resp {
        Data::CommandResponse {
            ok: true,
            data: Some(d),
            ..
        } => {
            let info = serde_json::from_value::<UserInfo>(d)
                .map_err(|error| format!("invalid user profile returned by service: {error}"))?;
            let mut auth = AUTH_STATE.lock();
            auth.user_info = Some(info.clone());
            Ok(Some(info))
        }
        Data::CommandResponse {
            ok: false, message, ..
        } => Err(message),
        other => Err(format!("unexpected service response: {other:?}")),
    }
}

#[tauri::command]
pub async fn get_invite_info() -> Result<Option<InviteInfo>, String> {
    debug!("[Invite] 通过 service 获取邀请信息");
    let resp = send_command_responsive(Data::GetInviteInfo).await?;
    match resp {
        Data::CommandResponse {
            ok: true,
            data: Some(d),
            ..
        } => serde_json::from_value::<InviteInfo>(d.clone())
            .map(Some)
            .map_err(|e| e.to_string()),
        Data::CommandResponse {
            ok: true,
            data: None,
            ..
        } => Ok(None),
        Data::CommandResponse {
            ok: false, message, ..
        } => Err(message),
        other => Err(format!("unexpected service response: {other:?}")),
    }
}

/// Tauri 命令：获取保存的登录标识与自动登录偏好。
#[tauri::command]
pub async fn get_saved_login() -> Result<Option<serde_json::Value>, String> {
    let resp = send_command_responsive(Data::GetSavedLogin).await?;
    match resp {
        Data::CommandResponse {
            ok: true,
            data: Some(d),
            ..
        } if !d.is_null() => {
            if !d.is_object() {
                return Err("invalid saved login response".to_string());
            }
            Ok(Some(d))
        }
        Data::CommandResponse {
            ok: true,
            data: Some(d),
            ..
        } if d.is_null() => Ok(None),
        Data::CommandResponse {
            ok: true,
            data: None,
            ..
        } => Ok(None),
        Data::CommandResponse {
            ok: false, message, ..
        } => Err(message),
        other => Err(format!("unexpected service response: {other:?}")),
    }
}

/// Tauri 命令：保存登录设置
///
/// machine config 受保护，UI 通过 IPC 委托 service 保存 token 会话偏好。
#[tauri::command]
pub async fn save_login_settings(
    identifier: String,
    remember_me: bool,
    auto_login: bool,
) -> Result<(), String> {
    let resp = send_command_responsive(Data::SaveLoginSettings {
        identifier,
        remember_me,
        auto_login,
    })
    .await?;
    match resp {
        Data::CommandResponse { ok: true, .. } => Ok(()),
        Data::CommandResponse {
            ok: false, message, ..
        } => Err(message),
        other => Err(format!("unexpected service response: {:?}", other)),
    }
}
