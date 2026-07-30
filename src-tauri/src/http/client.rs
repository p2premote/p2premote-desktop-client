use p2premote_core::http;
use reqwest::Client;
use tracing::{debug, error, trace};

const DEFAULT_SERVER_URL: &str = "https://cli.p2premote.top";

/// 不持认证 token 的公开 HTTP 客户端，服务于未登录接口（注册/验证码/重置密码）、
/// 版本检查、匿名连接、公网 IP 检测等。鉴权请求已统一下沉到 service（走 IPC）。
///
/// 复用 core::http 的共享 Client（含 TLS 策略、连接池），避免跨 crate 重复实现
/// 导致安全策略漂移。
#[derive(Clone)]
pub struct PublicHttpClient {
    client: Client,
    base_url: String,
}

impl PublicHttpClient {
    pub fn new() -> Self {
        Self {
            client: http::shared_client(),
            base_url: DEFAULT_SERVER_URL.to_string(),
        }
    }

    pub fn base_url(&self) -> &str {
        &self.base_url
    }

    pub async fn post_json<T: serde::de::DeserializeOwned, B: serde::Serialize>(
        &self,
        path: &str,
        body: &B,
    ) -> Result<T, String> {
        let url = format!("{}{}", self.base_url, path);
        let json_body = serde_json::to_string(body).map_err(|e| e.to_string())?;

        debug!("[PublicHttpClient] >>> POST {}", url);
        trace!("[PublicHttpClient] >>> Request body: {}", json_body);

        let resp = self
            .client
            .post(&url)
            .json(body)
            .send()
            .await
            .map_err(|e| e.to_string())?;

        let status = resp.status();
        let resp_body = resp.text().await.map_err(|e| e.to_string())?;

        debug!("[PublicHttpClient] <<< POST {} status={}", url, status);
        trace!("[PublicHttpClient] <<< Response body: {}", resp_body);

        let result = serde_json::from_str::<T>(&resp_body).map_err(|e| {
            error!("[PublicHttpClient] JSON parse error: {}", e);
            e.to_string()
        })?;

        Ok(result)
    }
}

impl Default for PublicHttpClient {
    fn default() -> Self {
        Self::new()
    }
}
