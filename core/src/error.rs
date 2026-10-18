//! One business error contract shared with the native punch engine.
pub use p2premote_punch::error::{AppError, ErrorCode};

pub const CONTRACT_VERSION: u8 = p2premote_punch::error_codes::ERROR_CONTRACT_VERSION;

pub fn require_contract(version: u8) -> anyhow::Result<()> {
    // 1.14 is transitional: absent declarations decode as zero for legacy peers.
    if version != 0 && version != CONTRACT_VERSION {
        return Err(coded(
            ErrorCode::ConnectionCompatibilityClientUpgradeRequired,
            "peer error contract is unsupported; upgrade required",
        ));
    }
    Ok(())
}

/// Wire-only adapter for pre-1.15 peers. Internal control flow uses canonical codes.
pub fn legacy_code(code: &str) -> &str {
    match code {
        "hole_punch_wait_timeout" | "punch_exhausted" | "traversal_signal_timeout" => "traversal.execution.exhausted",
        "peer_prepare_timeout" => "connection.peer_prepare.timeout",
        "peer_offline" => "connection.peer_prepare.offline",
        "peer_cancelled" => "connection.execution.peer_cancelled",
        "user_cancelled" => "connection.execution.user_cancelled",
        "invalid_lan_cidr" => "connection.route_validation.invalid_cidr",
        "config_load_failed" => "runtime.config.load_failed",
        "wireguard_config_failed" => "wireguard.configure.failed",
        "wireguard_handshake_failed" => "wireguard.handshake.failed",
        "approval_denied" => "connection.approval.denied",
        "approval_timeout" => "connection.approval.timeout",
        "approval_request_missing" => "connection.approval.request_missing",
        "approval_busy" => "connection.approval.busy",
        _ => "runtime.execution.unclassified",
    }
}

pub fn coded(code: ErrorCode, message: impl Into<String>) -> anyhow::Error {
    AppError::new(code, message).into()
}

pub fn code_of(error: &anyhow::Error) -> ErrorCode {
    if let Some(coded) = error.downcast_ref::<AppError>() {
        return coded.code();
    }
    error
        .chain()
        .find_map(|cause| cause.downcast_ref::<AppError>().map(AppError::code))
        .unwrap_or(ErrorCode::RuntimeExecutionUnclassified)
}

/// Classify an uncoded dependency failure at its owning operation boundary.
pub fn classified(error: anyhow::Error, fallback: ErrorCode, message: &str) -> anyhow::Error {
    if code_of(&error) != ErrorCode::RuntimeExecutionUnclassified {
        return error.context(message.to_owned());
    }
    AppError::with_boxed_source(fallback, message, error.into_boxed_dyn_error()).into()
}

pub fn retry_attempt(code: &ErrorCode) -> bool {
    matches!(
        code.retry_class(),
        "next_attempt" | "next_network_or_attempt"
    )
}

pub fn deserialize_code<'de, D: serde::Deserializer<'de>>(
    deserializer: D,
) -> Result<String, D::Error> {
    let code = <String as serde::Deserialize>::deserialize(deserializer)?;
    if !p2premote_punch::error::valid_code(&code) {
        return Err(serde::de::Error::custom("invalid structured error code"));
    }
    Ok(code)
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn transitional_contract_checks_every_wire_version() {
        for version in 0..=u8::MAX {
            let result = require_contract(version);
            if version == 0 || version == CONTRACT_VERSION {
                assert!(result.is_ok(), "version {version}");
            } else {
                assert_eq!(code_of(&result.unwrap_err()), ErrorCode::ConnectionCompatibilityClientUpgradeRequired);
            }
        }
    }

    #[test]
    fn classification_preserves_typed_errors_and_classifies_dependency_errors() {
        let original = coded(ErrorCode::ConnectionApprovalDenied, "original");
        let wrapped = classified(original, ErrorCode::SignalSendTimeout, "outer");
        assert_eq!(code_of(&wrapped), ErrorCode::ConnectionApprovalDenied);
        let dependency = anyhow::anyhow!("dependency failed");
        assert_eq!(code_of(&dependency), ErrorCode::RuntimeExecutionUnclassified);
        let wrapped = classified(dependency, ErrorCode::SignalSendTimeout, "send failed");
        assert_eq!(code_of(&wrapped), ErrorCode::SignalSendTimeout);
        assert!(format!("{wrapped:#}").contains("dependency failed"));
    }
    #[test]
    fn anyhow_context_does_not_change_business_identity() {
        let error = coded(ErrorCode::TraversalExecutionExhausted, "任意文案")
            .context("connection context")
            .context("different language");
        assert_eq!(code_of(&error), ErrorCode::TraversalExecutionExhausted);
        assert!(retry_attempt(&code_of(&error)));
        assert!(!retry_attempt(&ErrorCode::ConnectionExecutionUserCancelled));
    }
}
