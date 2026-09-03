//! Linux 平台的设备信息与远程访问探测。
//!
//! 注意：本模块整体以 `#[cfg(any(target_os = "linux", test))]` 引入（见 device.rs），
//! 以便 `/etc/os-release` 解析的纯函数在任意平台跑单测；依赖 Linux 环境的
//! 部分在函数级用 `#[cfg(target_os = "linux")]` 单独控制。
//!
//! 远程访问只探测本地监听端口（GNOME Remote Desktop / xrdp 的 RDP 3389，
//! VNC 5900+），不修改用户的远程桌面配置。

#[cfg(target_os = "linux")]
use super::RemoteAccessInfo;

#[cfg(target_os = "linux")]
pub(super) fn get_system_version() -> String {
    std::fs::read_to_string("/etc/os-release")
        .ok()
        .and_then(|contents| parse_linux_system_version(&contents))
        .unwrap_or_else(|| "Linux".to_string())
}

pub(super) fn parse_linux_system_version(os_release: &str) -> Option<String> {
    fn value_for<'a>(contents: &'a str, key: &str) -> Option<&'a str> {
        contents.lines().find_map(|line| {
            let line = line.trim();
            if line.is_empty() || line.starts_with('#') {
                return None;
            }
            let (candidate, value) = line.split_once('=')?;
            (candidate == key).then(|| value.trim().trim_matches(|c| c == '\"' || c == '\''))
        })
    }

    let name = value_for(os_release, "NAME")
        .or_else(|| value_for(os_release, "ID"))?
        .trim();
    if name.is_empty() {
        return None;
    }

    let version = value_for(os_release, "VERSION_ID")
        .map(str::trim)
        .filter(|version| !version.is_empty());

    Some(match version {
        Some(version) => format!("{} {}", name, version),
        None => name.to_string(),
    })
}

#[cfg(target_os = "linux")]
pub(crate) fn get_rdp_port_from_registry() -> u16 {
    3389
}

#[cfg(target_os = "linux")]
pub(crate) fn is_rdp_enabled() -> bool {
    super::is_local_port_open(3389)
}

#[cfg(target_os = "linux")]
pub(super) fn current_remote_access() -> Option<RemoteAccessInfo> {
    if super::is_local_port_open(3389) {
        return Some(RemoteAccessInfo {
            protocol: "rdp".to_string(),
            enabled: true,
            port: 3389,
        });
    }
    for port in [5900, 5901] {
        if super::is_local_port_open(port) {
            return Some(RemoteAccessInfo {
                protocol: "vnc".to_string(),
                enabled: true,
                port,
            });
        }
    }
    Some(RemoteAccessInfo {
        protocol: "rdp".to_string(),
        enabled: false,
        port: 3389,
    })
}
