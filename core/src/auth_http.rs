//! 带令牌自动刷新的统一鉴权请求封装（service 端用）
//!
//! service 持有 machine config 中的 access/refresh token，是令牌的唯一管理者。
//! 所有 service handler 调服务器都应走这里的 [`send_authed`]：
//! - 发请求前不主动刷新（避免每次请求都查过期）；
//! - 若响应表明令牌失效（HTTP 401 或业务 code 为 401），自动刷新一次并用新令牌重试一次；
//! - 只重试一次，refresh 本身失败则直接返回错误，不会死循环。

use crate::auth::refresh_with_config;
use crate::config::MachineConfig;
use anyhow::{anyhow, Result};
use reqwest::{RequestBuilder, StatusCode};
use serde::Deserialize;

/// 业务响应中代表"令牌失效"的错误码。
const AUTH_FAILURE_CODES: &[i32] = &[401];

/// 发送鉴权请求的产物：HTTP 状态码与完整 body 文本。
pub struct AuthedResponse {
    pub status: StatusCode,
    pub body: String,
}

/// 发送一次带鉴权的请求，若令牌失效则自动刷新并重试一次。
///
/// `build` 接收当前 token，返回构造好的 RequestBuilder（调用方负责 method、url、body，
/// **并需自行注入 `Authorization: Bearer <token>` 头**）。本函数只负责发送、判定令牌
/// 失效、刷新重试，不代为添加 Authorization 头。
///
/// 判定令牌失效的条件：HTTP 401，或响应 body 是 JSON 且其中的 `code` 字段命中
/// [`AUTH_FAILURE_CODES`]。命中则刷新一次令牌并用新令牌重试一次。
pub async fn send_authed<F>(config: &mut MachineConfig, build: F) -> Result<AuthedResponse>
where
    F: Fn(&str) -> RequestBuilder,
{
    let token = config
        .auth_token
        .clone()
        .ok_or_else(|| anyhow!("not logged in"))?;

    let (status, body) = do_send(&build, &token).await?;
    if !is_auth_failure(status, &body) {
        return Ok(AuthedResponse { status, body });
    }

    // 令牌失效：刷新一次后用新 token 重试
    tracing::debug!("[auth_http] 响应表明令牌失效，刷新令牌后重试一次");
    refresh_with_config(config).await?;
    let new_token = config
        .auth_token
        .clone()
        .ok_or_else(|| anyhow!("token missing after refresh"))?;
    let (status, body) = do_send(&build, &new_token).await?;
    Ok(AuthedResponse { status, body })
}

async fn do_send<F>(build: &F, token: &str) -> Result<(StatusCode, String)>
where
    F: Fn(&str) -> RequestBuilder,
{
    let resp = build(token).send().await?;
    let status = resp.status();
    let body = resp.text().await?;
    Ok((status, body))
}

/// 判定是否为"令牌失效"：HTTP 401，或 body 中 JSON code 命中鉴权错误码集合。
fn is_auth_failure(status: StatusCode, body: &str) -> bool {
    if status == StatusCode::UNAUTHORIZED {
        return true;
    }
    // 服务器可能用 HTTP 200 + body {"code":401,...} 表示令牌失效
    #[derive(Deserialize)]
    struct CodeOnly {
        #[serde(default)]
        code: Option<i32>,
    }
    let Ok(parsed) = serde_json::from_str::<CodeOnly>(body) else {
        return false;
    };
    parsed
        .code
        .map(|c| AUTH_FAILURE_CODES.contains(&c))
        .unwrap_or(false)
}
