//! 共享 HTTP 客户端
//!
//! 历史上每个调用点都 `build_client()` 新建一个 Client，开销大且无法复用连接池。
//! 现改为单例，全局复用。

use once_cell::sync::Lazy;
use reqwest::Client;
use serde::Deserialize;
use std::collections::BTreeMap;
use std::fmt;
use std::time::Duration;

/// 单例客户端：首次访问时构建一次，之后全局复用。
static SHARED_CLIENT: Lazy<Client> = Lazy::new(build_client);

/// 获取共享的 HTTP 客户端。
pub fn shared_client() -> Client {
    SHARED_CLIENT.clone()
}

/// 是否在本地联调时放开 TLS 证书校验。
///
/// 安全原则：日志级别（log_level）是用户可配置项，不应改变网络安全边界。
/// 仅当显式设置环境变量 `P2PREMOTE_INSECURE_TLS=1` 时才放开校验，且仅用于本地
/// 自签证书联调，生产环境不应设置。
fn insecure_tls_enabled() -> bool {
    std::env::var("P2PREMOTE_INSECURE_TLS")
        .map(|v| v == "1" || v.eq_ignore_ascii_case("true"))
        .unwrap_or(false)
}

/// 构建一个 HTTP 客户端。
///
/// 默认强制校验 TLS 证书。仅当环境变量 `P2PREMOTE_INSECURE_TLS=1` 时才放开校验，
/// 供本地联调自签证书使用——不与 log_level 耦合，避免用户调日志级别就关闭安全边界。
pub fn build_client() -> Client {
    let mut builder = Client::builder()
        .use_rustls_tls()
        .http1_only()
        .connect_timeout(Duration::from_secs(10))
        .timeout(Duration::from_secs(10))
        .pool_idle_timeout(Duration::from_secs(90))
        .tcp_keepalive(Duration::from_secs(30));
    if insecure_tls_enabled() {
        builder = builder.danger_accept_invalid_certs(true);
    }
    builder.build().expect("failed to build http client")
}

/// 后端业务响应统一包装：`{ code, msg, data }`。
///
/// `code == 0` 表示成功，非 0 表示业务错误（`msg` 为错误描述）。
/// `data` 的类型由调用方指定，通常为 `DeviceInfo` / `Vec<DeviceInfo>` /
/// `Option<XxxData>` / `Option<serde_json::Value>`。
///
/// 统一此结构，避免在 device/auth/p2p 各模块重复定义 9 份仅 data 类型不同的 envelope。
#[derive(Debug, Clone, Deserialize)]
#[serde(bound(deserialize = "T: serde::de::DeserializeOwned"))]
pub struct ApiResponse<T> {
    pub code: i32,
    #[serde(default)]
    pub msg: String,
    #[serde(default)]
    pub message_key: Option<String>,
    #[serde(default)]
    pub message_params: BTreeMap<String, serde_json::Value>,
    #[serde(default)]
    pub trace_id: Option<String>,
    pub data: T,
}

#[derive(Debug, Clone, PartialEq)]
pub struct ApiError {
    pub code: i32,
    pub message_key: Option<String>,
    pub message_params: BTreeMap<String, serde_json::Value>,
    pub fallback_message: String,
    pub trace_id: Option<String>,
}

impl ApiError {
    pub fn localized_message(&self, locale: Option<&str>) -> String {
        crate::i18n::localized_api_error(
            locale,
            self.code,
            self.message_key.as_deref(),
            &self.message_params,
            &self.fallback_message,
        )
    }
}

impl fmt::Display for ApiError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        if self.fallback_message.is_empty() {
            write!(f, "business error code: {}", self.code)
        } else {
            f.write_str(&self.fallback_message)
        }
    }
}

impl std::error::Error for ApiError {}

impl<T> ApiResponse<T> {
    /// 业务是否成功（code == 0）。
    pub fn is_ok(&self) -> bool {
        self.code == 0
    }

    /// 若成功返回 data 的引用，否则返回 msg 作为错误描述。
    pub fn into_data(self) -> Result<T, String> {
        if self.is_ok() {
            Ok(self.data)
        } else {
            Err(self.into_api_error().to_string())
        }
    }

    pub fn into_data_structured(self) -> Result<T, ApiError> {
        if self.is_ok() {
            Ok(self.data)
        } else {
            Err(self.into_api_error())
        }
    }

    pub fn localized_error_message(&self, locale: Option<&str>) -> String {
        ApiError {
            code: self.code,
            message_key: self.message_key.clone(),
            message_params: self.message_params.clone(),
            fallback_message: self.msg.clone(),
            trace_id: self.trace_id.clone(),
        }
        .localized_message(locale)
    }

    fn into_api_error(self) -> ApiError {
        ApiError {
            code: self.code,
            message_key: self.message_key,
            message_params: self.message_params,
            fallback_message: self.msg,
            trace_id: self.trace_id,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[derive(Debug, Deserialize, PartialEq)]
    struct Payload {
        name: String,
    }

    #[test]
    fn api_response_deserialize_success() {
        let json = r#"{"code":0,"msg":"ok","data":{"name":"hello"}}"#;
        let resp: ApiResponse<Payload> = serde_json::from_str(json).unwrap();
        assert!(resp.is_ok());
        assert_eq!(resp.data.name, "hello");
    }

    #[test]
    fn api_response_deserialize_error() {
        let json = r#"{"code":1001,"msg":"参数错误","data":null}"#;
        let resp: ApiResponse<Option<Payload>> = serde_json::from_str(json).unwrap();
        assert!(!resp.is_ok());
        assert_eq!(resp.into_data().unwrap_err(), "参数错误");
    }

    #[test]
    fn api_response_optional_data_omitted() {
        // data 字段缺失时，Option<T> 自动为 None（无需 #[serde(default)]）
        let json = r#"{"code":0,"msg":"ok"}"#;
        let resp: ApiResponse<Option<Payload>> = serde_json::from_str(json).unwrap();
        assert!(resp.is_ok());
        assert!(resp.data.is_none());
    }

    #[test]
    fn api_response_msg_default_when_missing() {
        let json = r#"{"code":0,"data":{"name":"x"}}"#;
        let resp: ApiResponse<Payload> = serde_json::from_str(json).unwrap();
        assert_eq!(resp.msg, "");
    }

    #[test]
    fn api_response_into_data_error_without_msg() {
        let json = r#"{"code":500,"data":null}"#;
        let resp: ApiResponse<Option<serde_json::Value>> = serde_json::from_str(json).unwrap();
        // msg 缺失（默认空串），错误描述回退到 code
        assert_eq!(resp.into_data().unwrap_err(), "business error code: 500");
    }

    #[test]
    fn structured_error_is_localized_instead_of_using_chinese_fallback() {
        let json = r#"{
            "code":2051,
            "msg":"当前 Free 用户最多可注册 3 台设备",
            "message_key":"device.limit_reached.free",
            "message_params":{"limit":3,"pro_limit":10},
            "trace_id":"trace-123",
            "data":null
        }"#;
        let resp: ApiResponse<Option<Payload>> = serde_json::from_str(json).unwrap();
        assert_eq!(
            resp.localized_error_message(Some("en")),
            "Free users can register up to 3 devices. Upgrade to Pro for up to 10."
        );
        let error = resp.into_data_structured().unwrap_err();
        assert_eq!(error.trace_id.as_deref(), Some("trace-123"));
    }

    #[test]
    fn old_server_error_falls_back_to_msg_when_code_is_unknown() {
        let json = r#"{"code":9999,"msg":"legacy message","data":null}"#;
        let resp: ApiResponse<Option<Payload>> = serde_json::from_str(json).unwrap();
        assert_eq!(resp.localized_error_message(Some("en")), "legacy message");
    }

    #[test]
    fn known_code_localizes_partial_rollout_without_message_key() {
        let json = r#"{"code":1054,"msg":"设备不在线","data":null}"#;
        let resp: ApiResponse<Option<Payload>> = serde_json::from_str(json).unwrap();
        assert_eq!(
            resp.localized_error_message(Some("en")),
            "Device is offline"
        );
    }
}
