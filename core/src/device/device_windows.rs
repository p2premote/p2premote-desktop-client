//! Windows 平台的设备信息与远程访问探测。
//!
//! 注意：本模块整体以 `#[cfg(any(windows, test))]` 引入（见 device.rs），
//! 以便纯字符串辅助函数在任意平台跑单测；依赖 Windows API 的部分在
//! 函数级用 `#[cfg(windows)]` 单独控制。

use serde::Deserialize;
#[cfg(windows)]
use tracing::{debug, error};
#[cfg(windows)]
use windows_service::{
    service::{ServiceAccess, ServiceState},
    service_manager::{ServiceManager, ServiceManagerAccess},
};
#[cfg(windows)]
use winreg::enums::*;
#[cfg(windows)]
use winreg::RegKey;

use super::RemoteAccessInfo;

#[cfg(windows)]
pub(super) fn run_hidden_cmd(args: &[&str]) -> std::io::Result<std::process::Output> {
    use std::os::windows::process::CommandExt;

    const CREATE_NO_WINDOW: u32 = 0x08000000;

    let mut cmd = std::process::Command::new("cmd");
    cmd.args(args).creation_flags(CREATE_NO_WINDOW);
    cmd.output()
}

#[cfg(windows)]
pub(super) fn get_system_version() -> String {
    if let Some((caption, current_build)) = get_windows_version_from_wmi() {
        let product_name = caption.replace("Microsoft ", "");
        let product_name = normalize_windows_product_name(product_name.trim(), &current_build);
        let display_version = get_windows_registry_value("DisplayVersion");
        return format_windows_system_version(&product_name, &display_version, &current_build);
    }

    let product_name = match run_hidden_cmd(&[
        "/C",
        "reg",
        "query",
        r"HKLM\SOFTWARE\Microsoft\Windows NT\CurrentVersion",
        "/v",
        "ProductName",
    ]) {
        Ok(output) => {
            let stdout = String::from_utf8_lossy(&output.stdout);
            parse_reg_output(&stdout)
        }
        Err(_) => String::new(),
    };

    if product_name.is_empty() {
        return "Windows 10/11".to_string();
    }

    let product_name = product_name.replace("Microsoft ", "");
    let display_version = match run_hidden_cmd(&[
        "/C",
        "reg",
        "query",
        r"HKLM\SOFTWARE\Microsoft\Windows NT\CurrentVersion",
        "/v",
        "DisplayVersion",
    ]) {
        Ok(output) => parse_reg_output(&String::from_utf8_lossy(&output.stdout)),
        Err(_) => String::new(),
    };

    let current_build = match run_hidden_cmd(&[
        "/C",
        "reg",
        "query",
        r"HKLM\SOFTWARE\Microsoft\Windows NT\CurrentVersion",
        "/v",
        "CurrentBuildNumber",
    ]) {
        Ok(output) => parse_reg_output(&String::from_utf8_lossy(&output.stdout)),
        Err(_) => String::new(),
    };

    let product_name = normalize_windows_product_name(&product_name, &current_build);

    format_windows_system_version(&product_name, &display_version, &current_build)
}

#[cfg(windows)]
pub(super) fn get_windows_version_from_wmi() -> Option<(String, String)> {
    #[derive(Deserialize)]
    #[serde(rename = "Win32_OperatingSystem")]
    struct WindowsOperatingSystem {
        #[serde(rename = "Caption")]
        caption: String,
        #[serde(rename = "BuildNumber")]
        build_number: String,
    }

    let com_library = wmi::COMLibrary::new().ok()?;
    let connection = wmi::WMIConnection::new(com_library).ok()?;
    let operating_systems: Vec<WindowsOperatingSystem> = connection.query().ok()?;
    operating_systems
        .into_iter()
        .next()
        .map(|os| (os.caption, os.build_number))
}

#[cfg(windows)]
fn get_windows_registry_value(value_name: &str) -> String {
    match run_hidden_cmd(&[
        "/C",
        "reg",
        "query",
        r"HKLM\SOFTWARE\Microsoft\Windows NT\CurrentVersion",
        "/v",
        value_name,
    ]) {
        Ok(output) => parse_reg_output(&String::from_utf8_lossy(&output.stdout)),
        Err(_) => String::new(),
    }
}

pub(super) fn format_windows_system_version(
    product_name: &str,
    display_version: &str,
    current_build: &str,
) -> String {
    if !display_version.is_empty() && !current_build.is_empty() {
        return format!("{} ({}.{})", product_name, display_version, current_build);
    }
    if !display_version.is_empty() {
        return format!("{} ({})", product_name, display_version);
    }
    if !current_build.is_empty() {
        return format!("{} (Build {})", product_name, current_build);
    }
    product_name.to_string()
}

pub(super) fn normalize_windows_product_name(product_name: &str, current_build: &str) -> String {
    let current_build = current_build.trim().parse::<u32>().ok();

    if let Some(suffix) = product_name.strip_prefix("Windows 10") {
        if suffix.is_empty() || suffix.chars().next().is_some_and(char::is_whitespace) {
            if current_build.is_some_and(|build| build >= 22_000) {
                return format!("Windows 11{suffix}");
            }
            if current_build.is_none() {
                return format!("Windows 10/11{suffix}");
            }
        }
    }

    product_name.to_string()
}

#[cfg(windows)]
fn parse_reg_output(output: &str) -> String {
    for line in output.lines() {
        let line = line.trim();
        if line.contains("REG_SZ") {
            if let Some(value) = line.split("REG_SZ").nth(1) {
                return value.trim().to_string();
            }
        }
        if line.contains("REG_DWORD") {
            if let Some(value) = line.split("REG_DWORD").nth(1) {
                return value.trim().to_string();
            }
        }
    }
    String::new()
}

#[cfg(windows)]
pub(crate) fn get_rdp_port_from_registry() -> u16 {
    let hklm = RegKey::predef(HKEY_LOCAL_MACHINE);
    if let Ok(key) =
        hklm.open_subkey(r"SYSTEM\CurrentControlSet\Control\Terminal Server\WinStations\RDP-Tcp")
    {
        if let Ok(port) = key.get_value::<u32, _>("PortNumber") {
            return port as u16;
        }
    }
    3389
}

#[cfg(windows)]
pub(crate) fn is_rdp_enabled() -> bool {
    match ServiceManager::local_computer(None::<&str>, ServiceManagerAccess::CONNECT) {
        Ok(manager) => match manager.open_service("TermService", ServiceAccess::QUERY_STATUS) {
            Ok(service) => match service.query_status() {
                Ok(status) => status.current_state == ServiceState::Running,
                Err(e) => {
                    error!("Failed to query RDP service status: {}", e);
                    false
                }
            },
            Err(e) => {
                debug!("Failed to open TermService: {}", e);
                false
            }
        },
        Err(e) => {
            debug!("Failed to connect to service manager: {}", e);
            false
        }
    }
}

#[cfg(windows)]
pub(super) fn current_remote_access() -> Option<RemoteAccessInfo> {
    Some(RemoteAccessInfo {
        protocol: "rdp".to_string(),
        enabled: is_rdp_enabled(),
        port: get_rdp_port_from_registry(),
    })
}
