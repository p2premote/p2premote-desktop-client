//! macOS 平台的设备信息与远程访问探测（Screen Sharing / VNC）。

use std::net::{SocketAddr, TcpStream};
use std::time::Duration;

use super::RemoteAccessInfo;

pub(super) fn get_system_version() -> String {
    let product_version = std::process::Command::new("/usr/bin/sw_vers")
        .arg("-productVersion")
        .output()
        .ok()
        .filter(|output| output.status.success())
        .map(|output| String::from_utf8_lossy(&output.stdout).trim().to_string())
        .filter(|value| !value.is_empty());
    let build_version = std::process::Command::new("/usr/bin/sw_vers")
        .arg("-buildVersion")
        .output()
        .ok()
        .filter(|output| output.status.success())
        .map(|output| String::from_utf8_lossy(&output.stdout).trim().to_string())
        .filter(|value| !value.is_empty());

    match (product_version, build_version) {
        (Some(product), Some(build)) => format!("macOS {product} ({build})"),
        (Some(product), None) => format!("macOS {product}"),
        _ => "macOS".to_string(),
    }
}

pub(crate) fn get_rdp_port_from_registry() -> u16 {
    // This legacy field is still carried in the P2P-ready message for one
    // compatibility cycle.  On macOS it represents Screen Sharing/VNC.
    5900
}

pub(super) fn is_macos_screen_sharing_enabled() -> bool {
    super::is_local_port_open(5900)
}

pub(super) fn current_remote_access() -> Option<RemoteAccessInfo> {
    Some(RemoteAccessInfo {
        protocol: "vnc".to_string(),
        enabled: is_macos_screen_sharing_enabled(),
        port: 5900,
    })
}
