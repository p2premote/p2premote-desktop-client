use crate::auth_http::send_authed;
use crate::config::MachineConfig;
use crate::http::{shared_client, ApiResponse};
use anyhow::{anyhow, Result};
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct AutoKeepStatus {
    pub target_device_id: i64,
    #[serde(default)]
    pub enabled: bool,
    #[serde(default = "default_stopped")]
    pub state: String,
    #[serde(default)]
    pub revision: i64,
    #[serde(default)]
    pub eligible: bool,
    #[serde(default)]
    pub ineligible_reason: Option<String>,
    #[serde(default)]
    pub last_trigger: Option<String>,
    #[serde(default)]
    pub stop_reason: Option<String>,
    #[serde(default)]
    pub error_code: Option<String>,
    #[serde(default)]
    pub next_retry_at: Option<i64>,
    #[serde(default)]
    pub updated_at: i64,
}

fn default_stopped() -> String {
    "stopped".to_string()
}

#[derive(Serialize)]
struct SetAutoKeepRequest {
    source_device_id: i64,
    enabled: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    expected_revision: Option<i64>,
}

type AutoKeepEnvelope = ApiResponse<Option<AutoKeepStatus>>;
type AutoKeepListEnvelope = ApiResponse<Option<Vec<AutoKeepStatus>>>;

pub async fn list_auto_keep(
    config: &mut MachineConfig,
    source_device_id: i64,
) -> Result<Vec<AutoKeepStatus>> {
    let url = format!(
        "{}/api/v1/devices/tunnel-auto-keep?source_device_id={}",
        config.server_url.trim_end_matches('/'),
        source_device_id
    );
    let client = shared_client();
    let response = send_authed(config, |token| {
        client
            .get(&url)
            .header("Authorization", format!("Bearer {}", token))
    })
    .await?;
    let envelope: AutoKeepListEnvelope = serde_json::from_str(&response.body)
        .map_err(|error| anyhow!("invalid auto keep list response: {}", error))?;
    if envelope.code != 0 {
        return Err(anyhow!(
            envelope.localized_error_message(config.locale.as_deref())
        ));
    }
    Ok(envelope.data.unwrap_or_default())
}

pub async fn set_auto_keep(
    config: &mut MachineConfig,
    source_device_id: i64,
    target_device_id: i64,
    enabled: bool,
    expected_revision: Option<i64>,
) -> Result<AutoKeepStatus> {
    let url = format!(
        "{}/api/v1/devices/{}/tunnel-auto-keep",
        config.server_url.trim_end_matches('/'),
        target_device_id
    );
    let request = SetAutoKeepRequest {
        source_device_id,
        enabled,
        expected_revision,
    };
    let client = shared_client();
    let response = send_authed(config, |token| {
        client
            .put(&url)
            .header("Authorization", format!("Bearer {}", token))
            .json(&request)
    })
    .await?;
    let envelope: AutoKeepEnvelope = serde_json::from_str(&response.body)
        .map_err(|error| anyhow!("invalid auto keep response: {}", error))?;
    if envelope.code != 0 {
        return Err(anyhow!(
            envelope.localized_error_message(config.locale.as_deref())
        ));
    }
    envelope
        .data
        .ok_or_else(|| anyhow!("auto keep response missing data"))
}
