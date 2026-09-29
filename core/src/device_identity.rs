use crate::config::MachineConfig;
use uuid::Uuid;

pub const DEVICE_FINGERPRINT_VERSION: u32 = 1;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DeviceFingerprint {
    pub platform: &'static str,
    pub value: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CloneDetectionResult {
    Unchanged,
    BaselineCreated,
    CloneDetected,
    Unsupported,
    Unavailable(String),
}

pub fn new_device_uuid() -> String {
    Uuid::new_v4().to_string()
}

pub fn ensure_device_uuid(config: &mut MachineConfig) -> bool {
    if config
        .device_uuid
        .as_deref()
        .is_some_and(|value| !value.trim().is_empty())
    {
        return false;
    }
    config.device_uuid = Some(new_device_uuid());
    true
}

pub fn detect_clone_with(
    config: &mut MachineConfig,
    fingerprint: Result<Option<DeviceFingerprint>, String>,
) -> CloneDetectionResult {
    let fingerprint = match fingerprint {
        Ok(Some(value)) => value,
        Ok(None) => return CloneDetectionResult::Unsupported,
        Err(error) => return CloneDetectionResult::Unavailable(error),
    };

    let previous = config.device_fingerprint.as_deref().map(str::trim);
    if previous.is_none() || previous == Some("") {
        config.device_fingerprint = Some(fingerprint.value);
        config.device_fingerprint_platform = Some(fingerprint.platform.to_string());
        config.device_fingerprint_version = DEVICE_FINGERPRINT_VERSION;
        return CloneDetectionResult::BaselineCreated;
    }

    let same_scheme = config.device_fingerprint_platform.as_deref() == Some(fingerprint.platform)
        && config.device_fingerprint_version == DEVICE_FINGERPRINT_VERSION;
    if !same_scheme {
        config.device_fingerprint = Some(fingerprint.value);
        config.device_fingerprint_platform = Some(fingerprint.platform.to_string());
        config.device_fingerprint_version = DEVICE_FINGERPRINT_VERSION;
        return CloneDetectionResult::BaselineCreated;
    }
    if previous == Some(fingerprint.value.as_str()) {
        return CloneDetectionResult::Unchanged;
    }

    config.device_id = None;
    config.device_uuid = Some(new_device_uuid());
    config.device_fingerprint = Some(fingerprint.value);
    config.device_fingerprint_platform = Some(fingerprint.platform.to_string());
    config.device_fingerprint_version = DEVICE_FINGERPRINT_VERSION;
    CloneDetectionResult::CloneDetected
}

pub fn detect_clone(config: &mut MachineConfig) -> CloneDetectionResult {
    detect_clone_with(config, collect_clone_fingerprint())
}

#[cfg(windows)]
fn collect_clone_fingerprint() -> Result<Option<DeviceFingerprint>, String> {
    let value = read_windows_smbios_uuid()?;
    Ok(Some(DeviceFingerprint {
        platform: "windows-smbios",
        value,
    }))
}

#[cfg(target_os = "linux")]
fn collect_clone_fingerprint() -> Result<Option<DeviceFingerprint>, String> {
    for path in [
        "/sys/class/dmi/id/product_uuid",
        "/sys/devices/virtual/dmi/id/product_uuid",
    ] {
        if let Ok(value) = std::fs::read_to_string(path) {
            if let Some(value) = normalize_uuid(&value) {
                return Ok(Some(DeviceFingerprint {
                    platform: "linux-dmi",
                    value,
                }));
            }
        }
    }
    Err("DMI product UUID is unavailable".to_string())
}

#[cfg(not(any(windows, target_os = "linux")))]
fn collect_clone_fingerprint() -> Result<Option<DeviceFingerprint>, String> {
    Ok(None)
}

fn normalize_uuid(value: &str) -> Option<String> {
    let parsed = Uuid::parse_str(value.trim()).ok()?;
    let bytes = parsed.as_bytes();
    if bytes.iter().all(|byte| *byte == 0) || bytes.iter().all(|byte| *byte == 0xff) {
        return None;
    }
    Some(parsed.hyphenated().to_string().to_uppercase())
}

#[cfg(windows)]
fn read_windows_smbios_uuid() -> Result<String, String> {
    use windows_sys::Win32::System::SystemInformation::GetSystemFirmwareTable;

    const RSMB: u32 = u32::from_be_bytes(*b"RSMB");
    let size = unsafe { GetSystemFirmwareTable(RSMB, 0, std::ptr::null_mut(), 0) };
    if size < 8 {
        return Err("SMBIOS table is unavailable".to_string());
    }
    let mut raw = vec![0u8; size as usize];
    let read =
        unsafe { GetSystemFirmwareTable(RSMB, 0, raw.as_mut_ptr().cast(), raw.len() as u32) };
    if read != size {
        return Err("failed to read SMBIOS table".to_string());
    }
    parse_raw_smbios_uuid(&raw)
}

#[cfg(windows)]
fn parse_raw_smbios_uuid(raw: &[u8]) -> Result<String, String> {
    if raw.len() < 8 {
        return Err("SMBIOS table is truncated".to_string());
    }
    let table_len = u32::from_le_bytes(raw[4..8].try_into().unwrap()) as usize;
    let end = 8usize.saturating_add(table_len).min(raw.len());
    let table = &raw[8..end];
    let mut offset = 0usize;
    while offset + 4 <= table.len() {
        let structure_type = table[offset];
        let length = table[offset + 1] as usize;
        if length < 4 || offset + length > table.len() {
            break;
        }
        if structure_type == 1 && length >= 24 {
            let bytes: [u8; 16] = table[offset + 8..offset + 24].try_into().unwrap();
            let uuid = Uuid::from_fields(
                u32::from_le_bytes(bytes[0..4].try_into().unwrap()),
                u16::from_le_bytes(bytes[4..6].try_into().unwrap()),
                u16::from_le_bytes(bytes[6..8].try_into().unwrap()),
                &bytes[8..16].try_into().unwrap(),
            );
            return normalize_uuid(&uuid.to_string())
                .ok_or_else(|| "SMBIOS UUID is invalid".to_string());
        }
        let mut next = offset + length;
        while next + 1 < table.len() && !(table[next] == 0 && table[next + 1] == 0) {
            next += 1;
        }
        offset = next.saturating_add(2);
        if structure_type == 127 {
            break;
        }
    }
    Err("SMBIOS Type 1 UUID is unavailable".to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn creates_baseline_without_replacing_legacy_identity() {
        let mut config = MachineConfig::default();
        config.device_id = Some(10);
        config.device_uuid = Some("legacy-id".to_string());
        let result = detect_clone_with(
            &mut config,
            Ok(Some(DeviceFingerprint {
                platform: "windows-smbios",
                value: "AAAAAAAA-AAAA-AAAA-AAAA-AAAAAAAAAAAA".to_string(),
            })),
        );
        assert_eq!(result, CloneDetectionResult::BaselineCreated);
        assert_eq!(config.device_id, Some(10));
        assert_eq!(config.device_uuid.as_deref(), Some("legacy-id"));
    }

    #[test]
    fn rebuilds_identity_when_fingerprint_changes() {
        let mut config = MachineConfig::default();
        config.device_id = Some(10);
        config.device_uuid = Some("legacy-id".to_string());
        config.device_fingerprint = Some("AAAAAAAA-AAAA-AAAA-AAAA-AAAAAAAAAAAA".to_string());
        config.device_fingerprint_platform = Some("windows-smbios".to_string());
        config.device_fingerprint_version = DEVICE_FINGERPRINT_VERSION;
        let result = detect_clone_with(
            &mut config,
            Ok(Some(DeviceFingerprint {
                platform: "windows-smbios",
                value: "BBBBBBBB-BBBB-BBBB-BBBB-BBBBBBBBBBBB".to_string(),
            })),
        );
        assert_eq!(result, CloneDetectionResult::CloneDetected);
        assert_eq!(config.device_id, None);
        assert_ne!(config.device_uuid.as_deref(), Some("legacy-id"));
        assert!(Uuid::parse_str(config.device_uuid.as_deref().unwrap()).is_ok());
    }

    #[test]
    fn unsupported_platform_never_changes_identity() {
        let mut config = MachineConfig::default();
        config.device_id = Some(10);
        config.device_uuid = Some("existing".to_string());
        assert_eq!(
            detect_clone_with(&mut config, Ok(None)),
            CloneDetectionResult::Unsupported
        );
        assert_eq!(config.device_id, Some(10));
        assert_eq!(config.device_uuid.as_deref(), Some("existing"));
    }

    #[test]
    fn fingerprint_scheme_upgrade_only_replaces_baseline() {
        let mut config = MachineConfig::default();
        config.device_id = Some(10);
        config.device_uuid = Some("existing".to_string());
        config.device_fingerprint = Some("AAAAAAAA-AAAA-AAAA-AAAA-AAAAAAAAAAAA".to_string());
        config.device_fingerprint_platform = Some("windows-smbios-old".to_string());
        let result = detect_clone_with(
            &mut config,
            Ok(Some(DeviceFingerprint {
                platform: "windows-smbios",
                value: "BBBBBBBB-BBBB-BBBB-BBBB-BBBBBBBBBBBB".to_string(),
            })),
        );
        assert_eq!(result, CloneDetectionResult::BaselineCreated);
        assert_eq!(config.device_id, Some(10));
        assert_eq!(config.device_uuid.as_deref(), Some("existing"));
    }

    #[cfg(windows)]
    #[test]
    fn parses_fixed_raw_smbios_type1_uuid() {
        let mut table = vec![0u8; 24];
        table[0] = 1;
        table[1] = 24;
        table[8..24].copy_from_slice(&[
            0x56, 0x4d, 0xe9, 0x51, 0x35, 0xf7, 0x1e, 0x8d, 0xc9, 0x5e, 0x0a, 0x59, 0x56, 0x28,
            0x66, 0x3f,
        ]);
        table.extend_from_slice(&[0, 0]);
        let mut raw = vec![0, 3, 0, 0];
        raw.extend_from_slice(&(table.len() as u32).to_le_bytes());
        raw.extend_from_slice(&table);
        assert_eq!(
            parse_raw_smbios_uuid(&raw).unwrap(),
            "51E94D56-F735-8D1E-C95E-0A595628663F"
        );
    }
}
