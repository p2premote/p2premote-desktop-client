use super::*;

trait PeerSetupOps {
    fn check_port(&self) -> Result<()>;
    fn start_userspace(&self) -> Result<gonc_ffi::WindowsWgPeerResult>;
    fn wait_userspace(&self, handle: &str) -> Result<()>;
    fn stop_userspace(&self, handle: &str);
    fn write_config(&self) -> Result<()>;
    fn start_kernel(&self) -> Result<wgvpn::TunnelHandle>;
    fn add_peer(&self) -> Result<()>;
    fn wait_kernel(&self) -> Result<()>;
    fn remove_peer(&self);
}

pub(super) struct LivePeerSetup<'a> {
    pub library: &'a Path,
    pub request: gonc_ffi::StartWindowsWgPeerRequest,
    pub conf_path: &'a Path,
    pub wg_cli: &'a str,
    pub peer: &'a PeerConfig,
    pub private_key: &'a str,
    pub local_ip: &'a str,
}

impl PeerSetupOps for LivePeerSetup<'_> {
    fn check_port(&self) -> Result<()> {
        ensure_wg_listen_port_available(WGVPN_LISTEN_PORT)
    }
    fn start_userspace(&self) -> Result<gonc_ffi::WindowsWgPeerResult> {
        gonc_ffi::start_userspace_wg_peer(self.library, &self.request)
    }
    fn wait_userspace(&self, handle: &str) -> Result<()> {
        wait_for_windows_wg_peer(self.library, handle, WIREGUARD_HANDSHAKE_TIMEOUT)
    }
    fn stop_userspace(&self, handle: &str) {
        let _ = gonc_ffi::stop_userspace_wg_peer(self.library, handle);
    }
    fn write_config(&self) -> Result<()> {
        WgConfigBuilder::new()
            .private_key(self.private_key)
            .address(format!("{}/24", self.local_ip))
            .listen_port(WGVPN_LISTEN_PORT)
            .add_peer(self.peer.clone())
            .build_active()
            .write_to(self.conf_path)
    }
    fn start_kernel(&self) -> Result<wgvpn::TunnelHandle> {
        start_tunnel_and_wait(self.conf_path)
    }
    fn add_peer(&self) -> Result<()> {
        wgvpn::add_peer(self.wg_cli, WG_TUNNEL_NAME, self.peer)
    }
    fn wait_kernel(&self) -> Result<()> {
        wait_for_handshake_with_peer(
            self.wg_cli,
            WG_TUNNEL_NAME,
            &self.peer.public_key,
            WIREGUARD_HANDSHAKE_TIMEOUT,
            current_epoch_secs(),
        )
    }
    fn remove_peer(&self) {
        let _ = wgvpn::remove_peer(self.wg_cli, WG_TUNNEL_NAME, &self.peer.public_key);
    }
}

// Keep all platform branches executable with in-memory effects in unit tests.
pub(super) fn configure_peer(
    ops: &LivePeerSetup<'_>,
    userspace: bool,
    existed: bool,
) -> Result<(wgvpn::TunnelHandle, Option<gonc_ffi::WindowsWgPeerResult>)> {
    configure_peer_with(ops, userspace, existed, ops.conf_path)
}

fn configure_peer_with(
    ops: &dyn PeerSetupOps,
    userspace: bool,
    existed: bool,
    conf_path: &Path,
) -> Result<(wgvpn::TunnelHandle, Option<gonc_ffi::WindowsWgPeerResult>)> {
    if userspace {
        let result = ops.start_userspace()?;
        if let Err(error) = ops.wait_userspace(&result.handle_id) {
            ops.stop_userspace(&result.handle_id);
            return Err(error);
        }
        Ok((
            wgvpn::TunnelHandle {
                tunnel_name: String::new(),
                conf_path: conf_path.into(),
            },
            Some(result),
        ))
    } else if !existed {
        ops.check_port()?;
        ops.write_config()?;
        ops.start_kernel().map(|handle| (handle, None))
    } else {
        ops.add_peer()?;
        if let Err(error) = ops.wait_kernel() {
            ops.remove_peer();
            return Err(error);
        }
        Ok((
            wgvpn::TunnelHandle {
                tunnel_name: WG_TUNNEL_NAME.into(),
                conf_path: conf_path.into(),
            },
            None,
        ))
    }
}

pub(super) fn finish_setup<T>(result: Result<T>, cleanup: impl FnOnce()) -> Result<T> {
    result.map_err(|error| {
        cleanup();
        classified(
            error,
            ErrorCode::WireguardConfigureFailed,
            "WireGuard operation failed",
        )
    })
}

pub(super) fn finish_ip_exchange<T>(
    result: Result<T>,
    reserved: Option<u32>,
    release: impl FnOnce(u32),
) -> Result<T> {
    result.map_err(|error| {
        if let Some(ip) = reserved {
            release(ip);
        }
        error
    })
}

pub(super) fn start_kernel_with(
    start: impl FnOnce() -> Result<wgvpn::TunnelHandle>,
    wait: impl FnOnce(&wgvpn::TunnelHandle) -> Result<()>,
    stop: impl FnOnce(&wgvpn::TunnelHandle),
) -> Result<wgvpn::TunnelHandle> {
    let handle = start()?;
    if let Err(error) = wait(&handle) {
        stop(&handle);
        return Err(classified(
            error,
            ErrorCode::WireguardConfigureFailed,
            "WireGuard operation failed",
        ));
    }
    Ok(handle)
}

#[cfg(test)]
#[path = "wgvpn_setup_tests.rs"]
mod tests;
