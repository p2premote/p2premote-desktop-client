use crate::config::MachineConfig;
use serde::{Deserialize, Serialize};

/// A source device's local preference. No cloud intent or membership state.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct AutoKeepSetting {
    pub server_url: String,
    pub user_email: String,
    pub source_device_uuid: String,
    pub target_device_id: i64,
    /// Only preferences enabled while this device was the established initiator
    /// may recover a connection. Older role-less preferences stay disabled.
    #[serde(default)]
    pub active_side: bool,
    #[serde(default)]
    pub paused_remote_stop: bool,
}

impl AutoKeepSetting {
    pub fn belongs_to(&self, config: &MachineConfig) -> bool {
        self.server_url.trim_end_matches('/') == config.server_url.trim_end_matches('/')
            && config
                .user_email
                .as_deref()
                .is_some_and(|email| self.user_email.eq_ignore_ascii_case(email))
            && config.device_uuid.as_deref() == Some(self.source_device_uuid.as_str())
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct AutoKeepStatus {
    pub target_device_id: i64,
    pub enabled: bool,
    pub state: String,
    pub last_trigger: Option<String>,
    pub stop_reason: Option<String>,
    pub next_retry_at: Option<i64>,
    pub updated_at: i64,
}

impl AutoKeepStatus {
    pub fn stopped(target_device_id: i64) -> Self {
        Self {
            target_device_id,
            enabled: false,
            state: "stopped".into(),
            last_trigger: None,
            stop_reason: None,
            next_retry_at: None,
            updated_at: 0,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn local_settings_are_scoped_to_account_server_and_source_device() {
        let setting = AutoKeepSetting {
            server_url: "https://example.test/".into(),
            user_email: "A@example.test".into(),
            source_device_uuid: "source".into(),
            target_device_id: 42,
            active_side: true,
            paused_remote_stop: false,
        };
        let mut config = MachineConfig {
            server_url: "https://example.test".into(),
            user_email: Some("a@example.test".into()),
            device_uuid: Some("source".into()),
            ..Default::default()
        };
        assert!(setting.belongs_to(&config));
        config.user_email = Some("b@example.test".into());
        assert!(!setting.belongs_to(&config));
        config.user_email = Some("a@example.test".into());
        config.device_uuid = Some("other".into());
        assert!(!setting.belongs_to(&config));
        config.device_uuid = Some("source".into());
        config.server_url = "https://other.test".into();
        assert!(!setting.belongs_to(&config));
    }
}
