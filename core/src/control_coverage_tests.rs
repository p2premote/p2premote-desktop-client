use super::*;

#[test]
fn legacy_errors_keep_priority_and_retry_semantics_for_every_supported_spelling() {
    let cases: &[(&[&str], &str, bool)] = &[
        (
            &[
                "saved session expired",
                "saved session invalid",
                "SAVED SESSION INVALID timeout",
            ],
            "AUTH_SAVED_SESSION_EXPIRED",
            false,
        ),
        (
            &[
                "invalid token",
                "invalid credential",
                "unauthorized",
                "session expired",
            ],
            "AUTH_SESSION_EXPIRED",
            false,
        ),
        (
            &[
                "permission denied",
                "access denied",
                "authorization check failed",
            ],
            "SERVICE_PERMISSION_DENIED",
            false,
        ),
        (
            &[
                "service not installed",
                "service installation is incomplete",
            ],
            "SERVICE_NOT_INSTALLED",
            false,
        ),
        (&["service disabled"], "SERVICE_DISABLED", false),
        (
            &[
                "service not ready",
                "service unavailable",
                "no persistent connection",
            ],
            "SERVICE_NOT_READY",
            true,
        ),
        (
            &["timed out", "timeout", "请求超时"],
            "NETWORK_TIMEOUT",
            true,
        ),
        (
            &[
                "DNS failed",
                "name or service not known",
                "failed to lookup address",
                "域名解析失败",
            ],
            "NETWORK_DNS_FAILED",
            true,
        ),
        (
            &[
                "offline",
                "network is unreachable",
                "not connected to a network",
                "网络不可用",
                "设备不在线",
            ],
            "NETWORK_OFFLINE",
            true,
        ),
        (
            &[
                "connection refused",
                "connection reset",
                "failed to connect",
                "error sending request",
                "无法连接服务器",
                "网络请求失败",
            ],
            "NETWORK_REQUEST_FAILED",
            true,
        ),
    ];
    for (messages, code, retryable) in cases {
        for message in *messages {
            let error = CommandError::from_legacy_message(message).expect(message);
            assert_eq!(error.code, *code, "{message}");
            assert_eq!(error.retryable, *retryable, "{message}");
            let wrapped = anyhow::anyhow!(message.to_string());
            assert_eq!(CommandError::from_error(&wrapped).unwrap().code, *code);
        }
    }
    for message in [
        "saved file expired",
        "disabled option",
        "unavailable",
        "not installed",
        "plain failure",
        "",
    ] {
        assert!(
            CommandError::from_legacy_message(message).is_none(),
            "must not over-classify {message}"
        );
    }
}

#[test]
fn structured_errors_survive_wrapping_and_unknown_diagnostics_use_explicit_fallbacks() {
    use crate::error::{coded, ErrorCode};
    let error =
        coded(ErrorCode::SignalSendTimeout, "arbitrary translated text").context("outer context");
    let command = CommandError::from_error(&error).unwrap();
    assert_eq!(command.code, "signal.send.timeout");
    assert!(command.retryable);
    assert_eq!(
        CommandError::from_network_message("new diagnostic").code,
        "SERVER_UNAVAILABLE"
    );
    assert!(CommandError::from_network_message("new diagnostic").retryable);
    assert_eq!(
        CommandError::from_service_status_error("access denied").code,
        "SERVICE_PERMISSION_DENIED"
    );
    assert!(!CommandError::from_service_status_error("access denied").retryable);
    assert_eq!(
        CommandError::from_service_status_error("new diagnostic").code,
        "SERVICE_NOT_READY"
    );
    assert!(CommandError::from_error(&anyhow::anyhow!("new diagnostic")).is_none());
}
