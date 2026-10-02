use serde::Deserialize;

#[derive(Debug, Clone, Deserialize)]
pub struct VersionPolicyData {
    pub latest_version: String,
    pub min_supported_version: String,
    #[serde(default)]
    pub release_notes: String,
}

#[derive(Debug, Deserialize)]
struct VersionPolicyResponse {
    data: Option<VersionPolicyData>,
}

#[derive(Debug)]
pub enum VersionPolicyError {
    Request(String),
    Status(reqwest::StatusCode),
    Decode(String),
    Empty,
}

impl VersionPolicyError {
    pub fn localized_message(&self, locale: Option<&str>) -> String {
        match self {
            Self::Status(status) => crate::i18n::localized_message(
                locale,
                "errors.server_status_code",
                &[("status", status.as_str())],
            ),
            Self::Decode(_) => {
                crate::i18n::localized_message(locale, "errors.parse_version_failed", &[])
            }
            Self::Empty => {
                crate::i18n::localized_message(locale, "errors.version_policy_empty", &[])
            }
            Self::Request(_) => {
                crate::i18n::localized_message(locale, "errors.cannot_connect_update_server", &[])
            }
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct VersionEvaluation {
    pub mode: &'static str,
    pub has_update: bool,
    pub force_update: bool,
}

pub async fn fetch_version_policy(
    server_url: &str,
) -> Result<VersionPolicyData, VersionPolicyError> {
    let url = format!(
        "{}/api/v1/client/version-policy?target={}",
        server_url.trim_end_matches('/'),
        client_update_target()
    );
    let response = crate::http::shared_client()
        .get(url)
        .send()
        .await
        .map_err(|error| VersionPolicyError::Request(error.to_string()))?;
    if !response.status().is_success() {
        return Err(VersionPolicyError::Status(response.status()));
    }
    response
        .json::<VersionPolicyResponse>()
        .await
        .map_err(|error| VersionPolicyError::Decode(error.to_string()))?
        .data
        .ok_or(VersionPolicyError::Empty)
}

/// Returns the version-policy target used by this installation. Package
/// builds bake their target in at compile time via P2PREMOTE_RELEASE_TARGET
/// (windows-win7-x64, linux-gui-x64), container packaging overrides the
/// baked/inferred target at runtime with P2PREMOTE_UPDATE_TARGET, and plain
/// headless builds fall back to the host triple.
pub fn client_update_target() -> String {
    if let Ok(target) = std::env::var("P2PREMOTE_UPDATE_TARGET") {
        if is_supported_update_target(&target) {
            return target;
        }
    }
    if let Some(target) = option_env!("P2PREMOTE_RELEASE_TARGET") {
        if is_supported_update_target(target) {
            return target.to_string();
        }
    }

    match (std::env::consts::OS, std::env::consts::ARCH) {
        ("windows", _) => "windows",
        ("macos", _) => "macos-universal",
        ("linux", "aarch64") => "linux-headless-aarch64",
        ("linux", _) => "linux-headless-x64",
        _ => "windows",
    }
    .to_string()
}

/// Must stay aligned with the server's model.SupportedClientTargets; unknown
/// values are rejected so a stale build falls back to the inferred target
/// instead of querying a policy row that does not exist.
fn is_supported_update_target(target: &str) -> bool {
    matches!(
        target,
        "windows"
            | "windows-win7-x64"
            | "linux-gui-x64"
            | "linux-headless-x64"
            | "linux-headless-aarch64"
            | "linux-docker-x64"
            | "linux-docker-aarch64"
            | "macos-universal"
            | "android"
    )
}

pub fn evaluate_version_policy(current: &str, policy: &VersionPolicyData) -> VersionEvaluation {
    let force_update = is_version_less(current, &policy.min_supported_version);
    let has_update = is_version_less(current, &policy.latest_version);
    let mode = if force_update {
        "force"
    } else if has_update {
        "optional"
    } else {
        "none"
    };
    VersionEvaluation {
        mode,
        has_update,
        force_update,
    }
}

/// Compares dotted version strings. Semantics must stay in sync with the
/// Android client's ClientVersionPolicy.compare/numericParts
/// (app/src/main/java/top/p2premote/android/ClientVersionPolicy.java): any
/// number of segments, missing segments default to 0, and segments that
/// fail to parse (or overflow) are treated as 0. Changing either side
/// requires updating the other.
pub fn is_version_less(current: &str, target: &str) -> bool {
    let current = version_parts(current);
    let target = version_parts(target);
    for index in 0..current.len().max(target.len()) {
        let current_part = *current.get(index).unwrap_or(&0);
        let target_part = *target.get(index).unwrap_or(&0);
        if current_part != target_part {
            return current_part < target_part;
        }
    }
    false
}

/// Splits a version into numeric segments; see is_version_less for the
/// cross-platform contract with the Android client's ClientVersionPolicy.
fn version_parts(version: &str) -> Vec<u64> {
    version
        .trim()
        .trim_start_matches(['v', 'V'])
        .split('.')
        .map(|part| {
            let digits = part
                .chars()
                .take_while(|char| char.is_ascii_digit())
                .collect::<String>();
            digits.parse::<u64>().unwrap_or(0)
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn compares_prefixed_and_build_suffixed_versions() {
        assert!(!is_version_less("1.6.4-cd8712", "v1.6.4"));
        assert!(is_version_less("v1.6.4-cd8712", "1.6.5"));
        assert!(is_version_less("1.6", "1.6.1"));
    }

    #[test]
    fn selects_a_supported_update_target() {
        assert!(is_supported_update_target(&client_update_target()));
    }

    #[test]
    fn baked_release_target_wins_over_host_inference() {
        // The None arm is a no-op on builds without a baked target (plain
        // host builds); package CI compiles with P2PREMOTE_RELEASE_TARGET set,
        // where this guards the runtime-env-over-baked-target precedence.
        if let Some(target) = option_env!("P2PREMOTE_RELEASE_TARGET") {
            assert_eq!(client_update_target(), target);
        }
    }

    #[test]
    fn supported_targets_match_server_policy_targets() {
        for target in [
            "windows",
            "windows-win7-x64",
            "linux-gui-x64",
            "linux-headless-x64",
            "linux-headless-aarch64",
            "linux-docker-x64",
            "linux-docker-aarch64",
            "macos-universal",
            "android",
        ] {
            assert!(is_supported_update_target(target), "missing {target}");
        }
        assert!(!is_supported_update_target("linux-gui-aarch64"));
        assert!(!is_supported_update_target(""));
    }

    #[test]
    fn evaluates_force_optional_and_current_modes() {
        let mut policy = VersionPolicyData {
            latest_version: "2.0.0".to_string(),
            min_supported_version: "1.5.0".to_string(),
            release_notes: String::new(),
        };
        assert_eq!(evaluate_version_policy("1.4.0", &policy).mode, "force");
        assert_eq!(evaluate_version_policy("1.6.0", &policy).mode, "optional");
        policy.latest_version = "1.6.0".to_string();
        assert_eq!(evaluate_version_policy("1.6.0", &policy).mode, "none");
    }

    #[test]
    fn localizes_version_policy_errors() {
        let error = VersionPolicyError::Status(reqwest::StatusCode::BAD_GATEWAY);
        assert_eq!(
            error.localized_message(Some("en")),
            "Server returned status code: 502"
        );
        assert_eq!(
            VersionPolicyError::Empty.localized_message(Some("zh-CN")),
            "版本策略数据为空"
        );
    }
}
