use super::*;
#[cfg(windows)]
use std::os::windows::process::CommandExt;
use std::{
    path::{Path, PathBuf},
    process::{Command, Stdio},
};

/// RustDeskTiny owns its listener and defaults to this direct-access port.
/// A future per-device preference may override the connection target only; p2pRemote must never
/// write the value into RustDeskTiny's configuration or manage its host lifecycle.
pub(super) const DEFAULT_RUSTDESK_TINY_PORT: u16 = 21121;

#[cfg(windows)]
const DETACHED_PROCESS: u32 = 0x00000008;
#[cfg(windows)]
const CREATE_NEW_PROCESS_GROUP: u32 = 0x00000200;
#[cfg(windows)]
const CREATE_BREAKAWAY_FROM_JOB: u32 = 0x01000000;

pub(super) async fn start_active_desktop_session(
    _shared: &Arc<Mutex<SharedRuntimeState>>,
    peer_device_id: i64,
    rustdesk_tiny_port: Option<u16>,
) -> Result<serde_json::Value> {
    let session = require_wgvpn_session(peer_device_id)?;
    if !session.is_active {
        return Err(anyhow!("desktop_controller_requires_active_tunnel"));
    }
    if !cfg!(windows) {
        return Err(anyhow!(
            "desktop_platform_unsupported: open RustDeskTiny manually and connect to the peer virtual IP"
        ));
    }

    let port = rustdesk_tiny_port.unwrap_or(DEFAULT_RUSTDESK_TINY_PORT);
    if port == 0 {
        return Err(anyhow!("desktop_port_invalid"));
    }
    let address = format!("{}:{}", session.peer_virtual_ip, port);
    let executable = resolve_desktop_executable()?;
    let address_for_process = address.clone();
    tokio::task::spawn_blocking(move || launch_controller(&executable, &address_for_process))
        .await
        .map_err(|error| anyhow!("desktop_controller_start_task_failed: {error}"))??;

    Ok(serde_json::json!({
        "state": "launched",
        "window": "native",
        "address": address,
        "port": port,
    }))
}

fn launch_controller(executable: &Path, address: &str) -> Result<()> {
    let mut command = Command::new(executable);
    command
        .args(["--connect", address])
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null());
    #[cfg(windows)]
    command.creation_flags(DETACHED_PROCESS | CREATE_NEW_PROCESS_GROUP | CREATE_BREAKAWAY_FROM_JOB);
    let child = command
        .spawn()
        .with_context(|| format!("start RustDeskTiny controller at {address}"))?;
    info!(
        pid = child.id(),
        address, "[Desktop] RustDeskTiny controller launched"
    );
    Ok(())
}

fn require_wgvpn_session(peer_device_id: i64) -> Result<crate::p2p::wgvpn_flow::WgVpnSession> {
    wgvpn_flow::snapshot_sessions()
        .into_iter()
        .find(|session| session.peer_device_id == peer_device_id)
        .ok_or_else(|| anyhow!("desktop_wgvpn_session_not_found"))
}

fn resolve_desktop_executable() -> Result<PathBuf> {
    if let Some(path) = std::env::var_os("RUSTDESK_TINY_PATH") {
        return validate_desktop_path(PathBuf::from(path));
    }
    #[cfg(windows)]
    {
        let resources_dir = std::env::current_exe()
            .ok()
            .and_then(|path| path.parent().map(Path::to_path_buf))
            .ok_or_else(|| anyhow!("engine_path_resolution_failed"))?;
        let engine_dir = resources_dir.join("RustDeskTiny");
        for executable_name in ["RustDeskTiny.exe", "RustDeskTinyLegacy.exe"] {
            let candidate = engine_dir.join(executable_name);
            if candidate.is_file() {
                return Ok(candidate);
            }
        }
        Err(anyhow!(
            "desktop_executable_not_found: expected RustDeskTiny.exe or RustDeskTinyLegacy.exe under {}",
            engine_dir.display()
        ))
    }
    #[cfg(not(windows))]
    Err(anyhow!("desktop_platform_unsupported"))
}

fn validate_desktop_path(path: PathBuf) -> Result<PathBuf> {
    if path.is_file() {
        Ok(path)
    } else {
        Err(anyhow!("desktop_executable_not_found: {}", path.display()))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_port_matches_rustdesk_tiny_direct_access() {
        assert_eq!(DEFAULT_RUSTDESK_TINY_PORT, 21121);
    }
}
