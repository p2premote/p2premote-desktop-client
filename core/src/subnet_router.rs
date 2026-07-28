use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq, Default)]
#[serde(rename_all = "snake_case")]
pub enum LanMode {
    #[default]
    Disabled,
    KernelSnat,
    UserspaceSnat,
}

pub fn desired_lan_mode(exposed_lan_cidrs: &[String]) -> LanMode {
    if exposed_lan_cidrs.is_empty() {
        LanMode::Disabled
    } else if cfg!(target_os = "linux") {
        LanMode::KernelSnat
    } else {
        LanMode::UserspaceSnat
    }
}

pub fn lan_mode_from_backend_label(label: &str) -> Option<LanMode> {
    match label {
        "" => None,
        "disabled" => Some(LanMode::Disabled),
        "kernel_snat" => Some(LanMode::KernelSnat),
        "userspace_snat" => Some(LanMode::UserspaceSnat),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn lan_mode_defaults_to_disabled_without_cidrs() {
        assert_eq!(desired_lan_mode(&[]), LanMode::Disabled);
        let expected = if cfg!(target_os = "linux") {
            LanMode::KernelSnat
        } else {
            LanMode::UserspaceSnat
        };
        assert_eq!(desired_lan_mode(&["192.168.10.0/24".into()]), expected);
        assert_eq!(
            lan_mode_from_backend_label("kernel_snat"),
            Some(LanMode::KernelSnat)
        );
    }
}
