use crate::auth_http::send_authed;
use crate::config::{load_machine_config, save_machine_config, MachineConfig};
use crate::http::{shared_client, ApiResponse};
use anyhow::{anyhow, Result};
use serde::{Deserialize, Serialize};
use std::time::{SystemTime, UNIX_EPOCH};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct UserInfo {
    pub user_id: i64,
    pub username: String,
    pub email: String,
    #[serde(default)]
    pub member_level: Option<String>,
    #[serde(default)]
    pub member_expire_time: Option<String>,
    #[serde(default)]
    pub trial_start_time: Option<String>,
    #[serde(default)]
    pub trial_expire_time: Option<String>,
    #[serde(default)]
    pub trial_used: Option<bool>,
    #[serde(default)]
    pub trial_remaining_days: Option<i32>,
    #[serde(default)]
    pub is_pro: Option<bool>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AuthBundle {
    pub access_token: String,
    pub refresh_token: String,
    pub expires_in: i64,
    pub user: UserInfo,
}

/// 邀请信息（/auth/invite 响应）
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct InviteInfo {
    pub invite_code: String,
    pub invite_link: String,
}

type AuthEnvelope = ApiResponse<Option<AuthBundle>>;

fn current_locale() -> Option<String> {
    load_machine_config().ok().and_then(|config| config.locale)
}

#[derive(Debug, Serialize)]
struct LoginRequest<'a> {
    identifier: &'a str,
    password: &'a str,
}

#[derive(Debug, Serialize)]
struct RefreshRequest<'a> {
    refresh_token: &'a str,
}

#[derive(Debug, Serialize)]
struct RegisterRequest<'a> {
    username: &'a str,
    email: &'a str,
    password: &'a str,
    verification_code: &'a str,
    #[serde(skip_serializing_if = "Option::is_none")]
    invite_code: Option<&'a str>,
}

#[derive(Debug, Serialize)]
struct VerificationCodeRequest<'a> {
    email: &'a str,
    code_type: &'a str,
}

#[derive(Debug, Serialize)]
struct ResetPasswordRequest<'a> {
    email: &'a str,
    verification_code: &'a str,
    new_password: &'a str,
}

pub async fn send_verification_code(
    server_url: &str,
    email: &str,
    code_type: &str,
) -> Result<ApiResponse<Option<serde_json::Value>>> {
    let url = format!(
        "{}/api/v1/auth/verification-code",
        server_url.trim_end_matches('/')
    );
    let response = shared_client()
        .post(url)
        .json(&VerificationCodeRequest { email, code_type })
        .send()
        .await?;
    Ok(response.json().await?)
}

pub async fn reset_password_by_email_code(
    server_url: &str,
    email: &str,
    verification_code: &str,
    new_password: &str,
) -> Result<ApiResponse<Option<serde_json::Value>>> {
    let url = format!(
        "{}/api/v1/auth/reset-password",
        server_url.trim_end_matches('/')
    );
    let response = shared_client()
        .post(url)
        .json(&ResetPasswordRequest {
            email,
            verification_code,
            new_password,
        })
        .send()
        .await?;
    Ok(response.json().await?)
}

/// 使用邮件验证码注册用户。
///
/// 桌面窗口与 Web 管理界面共用此请求实现，确保服务地址和邀请代码处理一致。
pub async fn register_by_email_code(
    server_url: &str,
    username: &str,
    email: &str,
    password: &str,
    verification_code: &str,
    invite_code: Option<&str>,
) -> Result<ApiResponse<Option<serde_json::Value>>> {
    let invite_code = invite_code.map(str::trim).filter(|value| !value.is_empty());
    let url = format!(
        "{}/api/v1/auth/register/email",
        server_url.trim_end_matches('/')
    );
    let response = shared_client()
        .post(url)
        .json(&RegisterRequest {
            username,
            email,
            password,
            verification_code,
            invite_code,
        })
        .send()
        .await?;
    Ok(response.json().await?)
}

pub async fn login_and_persist(identifier: &str, password: &str) -> Result<AuthBundle> {
    let mut config = load_machine_config()?;
    let bundle = login(&config.server_url, identifier, password).await?;
    apply_auth_bundle(&mut config, &bundle);
    save_machine_config(&config)?;
    Ok(bundle)
}

pub async fn login(server_url: &str, identifier: &str, password: &str) -> Result<AuthBundle> {
    let client = shared_client();
    let url = format!("{}/api/v1/auth/login", server_url.trim_end_matches('/'));
    let response = client
        .post(url)
        .json(&LoginRequest {
            identifier,
            password,
        })
        .send()
        .await?;

    let envelope: AuthEnvelope = response.json().await?;
    if envelope.code != 0 {
        return Err(anyhow!(
            envelope.localized_error_message(current_locale().as_deref())
        ));
    }
    envelope
        .data
        .ok_or_else(|| anyhow!("login response missing data"))
}

pub async fn refresh_with_config(config: &mut MachineConfig) -> Result<AuthBundle> {
    let refresh_token = config
        .refresh_token
        .clone()
        .ok_or_else(|| anyhow!("missing refresh token"))?;
    let bundle = refresh(&config.server_url, refresh_token.as_str()).await?;
    apply_auth_bundle(config, &bundle);
    save_machine_config(config)?;
    Ok(bundle)
}

pub async fn refresh(server_url: &str, refresh_token: &str) -> Result<AuthBundle> {
    let client = shared_client();
    let url = format!("{}/api/v1/auth/refresh", server_url.trim_end_matches('/'));
    let response = client
        .post(url)
        .json(&RefreshRequest { refresh_token })
        .send()
        .await?;

    let envelope: AuthEnvelope = response.json().await?;
    if envelope.code != 0 {
        return Err(anyhow!(
            envelope.localized_error_message(current_locale().as_deref())
        ));
    }
    envelope
        .data
        .ok_or_else(|| anyhow!("refresh response missing data"))
}

pub fn apply_auth_bundle(config: &mut MachineConfig, bundle: &AuthBundle) {
    let now = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|duration| duration.as_secs() as i64)
        .unwrap_or_default();
    config.auth_token = Some(bundle.access_token.clone());
    config.refresh_token = Some(bundle.refresh_token.clone());
    config.access_token_expires_at = Some(now + bundle.expires_in);
    config.user_email = Some(bundle.user.email.clone());
}

// ---- 带令牌的认证信息查询（走 send_authed 自动刷新）----

/// /auth/profile 响应数据
type ProfileEnvelope = ApiResponse<Option<ProfileData>>;
#[derive(Deserialize)]
struct ProfileData {
    user: UserInfo,
}

/// /auth/invite 响应数据
type InviteEnvelope = ApiResponse<Option<InviteInfo>>;

/// 获取当前用户资料（GET /auth/profile）。
///
/// 走 send_authed，遇令牌失效自动刷新重试一次。
pub async fn fetch_profile(config: &mut MachineConfig) -> Result<UserInfo> {
    let url = format!(
        "{}/api/v1/auth/profile",
        config.server_url.trim_end_matches('/')
    );
    let client = shared_client();
    let resp = send_authed(config, |token| {
        client
            .get(&url)
            .header("Authorization", format!("Bearer {}", token))
    })
    .await?;
    let envelope: ProfileEnvelope = serde_json::from_str(&resp.body)
        .map_err(|e| anyhow!("invalid profile response body={}: {}", resp.body, e))?;
    if envelope.code != 0 {
        return Err(anyhow!(
            envelope.localized_error_message(config.locale.as_deref())
        ));
    }
    envelope
        .data
        .map(|d| d.user)
        .ok_or_else(|| anyhow!("profile response missing data"))
}

/// 获取邀请信息（GET /auth/invite）。
///
/// 走 send_authed，遇令牌失效自动刷新重试一次。
pub async fn fetch_invite_info(config: &mut MachineConfig) -> Result<InviteInfo> {
    let url = format!(
        "{}/api/v1/auth/invite",
        config.server_url.trim_end_matches('/')
    );
    let client = shared_client();
    let resp = send_authed(config, |token| {
        client
            .get(&url)
            .header("Authorization", format!("Bearer {}", token))
    })
    .await?;
    let envelope: InviteEnvelope = serde_json::from_str(&resp.body)
        .map_err(|e| anyhow!("invalid invite response body={}: {}", resp.body, e))?;
    if envelope.code != 0 {
        return Err(anyhow!(
            envelope.localized_error_message(config.locale.as_deref())
        ));
    }
    envelope
        .data
        .ok_or_else(|| anyhow!("invite response missing data"))
}
