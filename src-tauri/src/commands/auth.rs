//! 认证模块
//!
//! 认证命令统一通过 IPC 委托 service 执行。service 是认证状态和令牌的唯一管理者。

use tracing::{debug, error, info};

use super::service::send_command_responsive;
use p2premote_core::auth::{InviteInfo, UserInfo};
use p2premote_core::control::send_command;
use p2premote_core::control::Data;
use p2premote_core::service_control::stop_service;

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

    command_data(
        send_command_responsive(Data::RegisterByEmailCode {
            username,
            email,
            password,
            verification_code,
            invite_code,
        })
        .await?,
    )
}

#[tauri::command]
pub async fn send_registration_verification_code(
    email: String,
) -> Result<serde_json::Value, String> {
    command_data(
        send_command_responsive(Data::SendVerificationCode {
            email,
            purpose: "register".to_string(),
        })
        .await?,
    )
}

#[tauri::command]
pub async fn send_reset_password_verification_code(
    email: String,
) -> Result<serde_json::Value, String> {
    command_data(
        send_command_responsive(Data::SendVerificationCode {
            email,
            purpose: "reset_password".to_string(),
        })
        .await?,
    )
}

#[tauri::command]
pub async fn reset_password_by_email_code(
    email: String,
    verification_code: String,
    new_password: String,
) -> Result<serde_json::Value, String> {
    command_data(
        send_command_responsive(Data::ResetPasswordByEmailCode {
            email,
            verification_code,
            new_password,
        })
        .await?,
    )
}

/// Tauri 命令：检查是否已登录
///
/// 先查内存 AUTH_STATE（登录后立即生效）；内存为空时通过 IPC 查 service 的
/// RuntimeStatus.logged_in（不读受保护的 machine config，避免权限失败返回假阴性）。
#[tauri::command]
pub async fn is_logged_in() -> Result<bool, String> {
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
    info!("[ResumeSavedSession] saved session resumed");
    Ok(())
}

/// Tauri 命令：获取用户信息
#[tauri::command]
pub async fn get_user_info() -> Result<Option<UserInfo>, String> {
    if !is_logged_in().await? {
        return Ok(None);
    }
    fetch_user_profile().await
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
            Ok(Some(info))
        }
        Data::CommandResponse {
            ok: false, message, ..
        } => Err(message),
        other => Err(format!("unexpected service response: {other:?}")),
    }
}

fn command_data(response: Data) -> Result<serde_json::Value, String> {
    match response {
        Data::CommandResponse {
            ok: true,
            data: Some(data),
            ..
        } => Ok(data),
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
