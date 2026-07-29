//! 认证模块
//!
//! 认证命令（login/logout/profile/invite/try_auto_login）统一通过 IPC 委托 service
//! 执行，service 是令牌的唯一管理者（持有令牌、负责持久化与续期）。UI 仅维护内存
//! 中的 AUTH_STATE 供前端展示。登录偏好也通过 IPC 委托 service 管理。

use serde::{Deserialize, Serialize};
use tracing::{debug, error, info};

use super::service::send_command_responsive;
use crate::http::PublicHttpClient;
use p2premote_core::control::send_command;
use p2premote_core::control::Data;
use p2premote_core::http::ApiResponse;
use p2premote_core::service_control::stop_service;

/// 用户信息（UI 展示用，与 core::auth::UserInfo 对齐）
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct UserInfo {
    pub user_id: i64,
    pub username: String,
    pub email: String,
    #[serde(default)]
    pub member_level: Option<String>, // free, pro
    #[serde(default)]
    pub member_expire_time: Option<String>, // RFC3339
    #[serde(default)]
    pub trial_start_time: Option<String>, // RFC3339
    #[serde(default)]
    pub trial_used: Option<bool>,
    #[serde(default)]
    pub trial_remaining_days: Option<i32>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct InviteInfo {
    pub invite_code: String,
    pub invite_link: String,
}

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
    if let Ok(info) = serde_json::from_value::<UserInfo>(user_json.clone()) {
        let mut auth = AUTH_STATE.lock();
        // 用占位符表示已登录，不存真实 token
        auth.token = Some(SERVICE_SESSION_PLACEHOLDER.to_string());
        auth.user_info = Some(info);
    }

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

    let _ = send_command(Data::Logout).await;
    let _ = stop_service();

    let mut auth = AUTH_STATE.lock();
    auth.token = None;
    auth.user_info = None;

    Ok(())
}

/// Tauri 命令：注册（无需邮箱验证码，使用客户端本地图片验证码）
#[tauri::command]
pub async fn register_no_verify(
    username: String,
    email: String,
    password: String,
    invite_code: Option<String>,
) -> Result<serde_json::Value, String> {
    debug!("RegisterNoVerify request for: {}", email);

    #[derive(Serialize)]
    struct RegisterNoVerifyRequest {
        username: String,
        email: String,
        password: String,
        #[serde(skip_serializing_if = "Option::is_none")]
        invite_code: Option<String>,
    }

    let http = PublicHttpClient::new();
    debug!("[RegisterNoVerify] server base_url: {}", http.base_url());

    let request = RegisterNoVerifyRequest {
        username: username.clone(),
        email: email.clone(),
        password,
        invite_code: invite_code
            .map(|value| value.trim().to_string())
            .filter(|value| !value.is_empty()),
    };

    // Debug: 打印发送的 JSON
    let json_body = serde_json::to_string(&request).map_err(|e| e.to_string())?;
    info!(
        "[RegisterNoVerify] POST /api/v1/auth/register/email body: {}",
        json_body
    );

    let response: ApiResponse<Option<serde_json::Value>> = http
        .post_json("/api/v1/auth/register/email", &request)
        .await
        .map_err(|e| e.to_string())?;

    let locale = p2premote_core::config::load_machine_config()
        .ok()
        .and_then(|config| config.locale);
    let message = if response.code == 0 {
        response.msg.clone()
    } else {
        response.localized_error_message(locale.as_deref())
    };
    Ok(serde_json::json!({
        "code": response.code,
        "msg": message,
    }))
}

/// Tauri 命令：检查是否已登录
///
/// 先查内存 AUTH_STATE（登录后立即生效）；内存为空时通过 IPC 查 service 的
/// RuntimeStatus.logged_in（不读受保护的 machine config，避免权限失败返回假阴性）。
#[tauri::command]
pub async fn is_logged_in() -> bool {
    // 先检查内存状态（确保 MutexGuard 不跨 await）
    let in_memory = { AUTH_STATE.lock().token.is_some() };
    if in_memory {
        return true;
    }

    // 内存为空则查 service 运行时状态
    match send_command_responsive(Data::Status).await {
        Ok(Data::CommandResponse {
            status: Some(s), ..
        }) => s.logged_in,
        _ => false,
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
    let resp = match send_command_responsive(Data::TryAutoLogin).await {
        Ok(r) => r,
        Err(e) => {
            error!("[TryAutoLogin] IPC 失败: {}", e);
            return Ok(String::new());
        }
    };

    let Data::CommandResponse {
        ok: true,
        data: Some(d),
        ..
    } = resp
    else {
        return Ok(String::new());
    };

    let logged_in = d
        .get("logged_in")
        .and_then(|v| v.as_bool())
        .unwrap_or(false);
    if !logged_in {
        return Ok(String::new());
    }

    // 同步 user info 到内存状态
    if let Some(user) = d.get("user") {
        if let Ok(info) = serde_json::from_value::<UserInfo>(user.clone()) {
            let mut auth = AUTH_STATE.lock();
            auth.token = Some(SERVICE_SESSION_PLACEHOLDER.to_string());
            auth.user_info = Some(info);
        }
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
        .unwrap_or(false)
    {
        return Err("saved session was not resumed".to_string());
    }
    if let Some(user) = d.get("user") {
        if let Ok(info) = serde_json::from_value::<UserInfo>(user.clone()) {
            let mut auth = AUTH_STATE.lock();
            auth.token = Some(SERVICE_SESSION_PLACEHOLDER.to_string());
            auth.user_info = Some(info);
        }
    }
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
            if let Ok(info) = serde_json::from_value::<UserInfo>(d.clone()) {
                let mut auth = AUTH_STATE.lock();
                auth.user_info = Some(info.clone());
                Ok(Some(info))
            } else {
                Ok(None)
            }
        }
        Data::CommandResponse {
            ok: false, message, ..
        } => Err(message),
        _ => Ok(None),
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
            ok: false, message, ..
        } => Err(message),
        _ => Ok(None),
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
        _ => Ok(None),
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
