//! One business error contract shared with the native punch engine.
pub use p2premote_punch::error::{AppError, ErrorCode};

pub const CONTRACT_VERSION: u8 = p2premote_punch::error_codes::ERROR_CONTRACT_VERSION;

pub fn require_contract(version: u8) -> anyhow::Result<()> {
    if version != CONTRACT_VERSION {
        return Err(coded(
            ErrorCode::ConnectionCompatibilityClientUpgradeRequired,
            "peer error contract is unsupported; upgrade required",
        ));
    }
    Ok(())
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
    fn anyhow_context_does_not_change_business_identity() {
        let error = coded(ErrorCode::TraversalExecutionExhausted, "任意文案")
            .context("connection context")
            .context("different language");
        assert_eq!(code_of(&error), ErrorCode::TraversalExecutionExhausted);
        assert!(retry_attempt(&code_of(&error)));
        assert!(!retry_attempt(&ErrorCode::ConnectionExecutionUserCancelled));
    }
}
