use serde::Deserialize;

#[derive(Debug, Clone, Deserialize)]
pub struct VersionPolicyData {
    pub latest_version: String,
    pub min_supported_version: String,
    pub download_url: String,
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
        "{}/api/v1/client/version-policy",
        server_url.trim_end_matches('/')
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
    fn evaluates_force_optional_and_current_modes() {
        let mut policy = VersionPolicyData {
            latest_version: "2.0.0".to_string(),
            min_supported_version: "1.5.0".to_string(),
            download_url: String::new(),
            release_notes: String::new(),
        };
        assert_eq!(evaluate_version_policy("1.4.0", &policy).mode, "force");
        assert_eq!(evaluate_version_policy("1.6.0", &policy).mode, "optional");
        policy.latest_version = "1.6.0".to_string();
        assert_eq!(evaluate_version_policy("1.6.0", &policy).mode, "none");
    }
}
