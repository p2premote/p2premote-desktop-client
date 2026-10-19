use super::*;
use std::cell::RefCell;

#[test]
fn failed_exchange_releases_a_reserved_ip_but_success_transfers_it_to_setup() {
    for reserved in [None, Some(123)] {
        for succeeds in [false, true] {
            let released = RefCell::new(vec![]);
            let result = finish_ip_exchange(
                if succeeds {
                    Ok("payload")
                } else {
                    Err(coded(ErrorCode::SignalSendTimeout, "exchange reply failed"))
                },
                reserved,
                |ip| released.borrow_mut().push(ip),
            );
            if succeeds {
                assert_eq!(result.unwrap(), "payload");
                assert!(released.borrow().is_empty());
            } else {
                assert_eq!(
                    crate::error::code_of(&result.unwrap_err()),
                    ErrorCode::SignalSendTimeout
                );
                assert_eq!(*released.borrow(), reserved.into_iter().collect::<Vec<_>>());
            }
        }
    }
}

struct Ops {
    fail: &'static str,
    calls: RefCell<Vec<String>>,
}
impl Ops {
    fn call(&self, step: &str) -> Result<()> {
        self.calls.borrow_mut().push(step.into());
        if self.fail == step {
            Err(coded(
                if step.starts_with("wait") {
                    ErrorCode::WireguardHandshakeTimeout
                } else {
                    ErrorCode::WireguardConfigureFailed
                },
                step,
            ))
        } else {
            Ok(())
        }
    }
}
impl PeerSetupOps for Ops {
    fn check_port(&self) -> Result<()> {
        self.call("port")
    }
    fn start_userspace(&self) -> Result<gonc_ffi::WindowsWgPeerResult> {
        self.call("start_userspace")?;
        Ok(
            serde_json::from_value(serde_json::json!({"ok":true,"handle_id":"native-peer"}))
                .unwrap(),
        )
    }
    fn wait_userspace(&self, handle: &str) -> Result<()> {
        assert_eq!(handle, "native-peer");
        self.call("wait_userspace")
    }
    fn stop_userspace(&self, handle: &str) {
        assert_eq!(handle, "native-peer");
        self.call("stop_userspace").unwrap();
    }
    fn write_config(&self) -> Result<()> {
        self.call("write")
    }
    fn start_kernel(&self) -> Result<wgvpn::TunnelHandle> {
        self.call("start_kernel")?;
        Ok(wgvpn::TunnelHandle {
            tunnel_name: "wg0".into(),
            conf_path: "unit.conf".into(),
        })
    }
    fn add_peer(&self) -> Result<()> {
        self.call("add")
    }
    fn wait_kernel(&self) -> Result<()> {
        self.call("wait_kernel")
    }
    fn remove_peer(&self) {
        self.call("remove").unwrap();
    }
}

#[test]
fn each_setup_failure_rolls_back_only_acquired_resources_and_preserves_code() {
    for (userspace, existed, fail, expected) in [
        (true, false, "start_userspace", vec!["start_userspace"]),
        (
            true,
            false,
            "wait_userspace",
            vec!["start_userspace", "wait_userspace", "stop_userspace"],
        ),
        (false, false, "port", vec!["port"]),
        (false, false, "write", vec!["port", "write"]),
        (
            false,
            false,
            "start_kernel",
            vec!["port", "write", "start_kernel"],
        ),
        (false, true, "add", vec!["add"]),
        (
            false,
            true,
            "wait_kernel",
            vec!["add", "wait_kernel", "remove"],
        ),
    ] {
        for passive in [false, true] {
            let ops = Ops {
                fail,
                calls: RefCell::new(vec![]),
            };
            let cleanup = RefCell::new(vec![]);
            let result = finish_setup(
                configure_peer_with(&ops, userspace, existed, Path::new("unit.conf")),
                || {
                    if passive {
                        cleanup.borrow_mut().push("release_ip");
                    }
                    cleanup.borrow_mut().push("stop_udp");
                },
            );
            let error = match result {
                Err(error) => error,
                Ok(_) => panic!("failure unexpectedly succeeded"),
            };
            assert_eq!(
                crate::error::code_of(&error),
                if fail.starts_with("wait") {
                    ErrorCode::WireguardHandshakeTimeout
                } else {
                    ErrorCode::WireguardConfigureFailed
                }
            );
            assert!(format!("{error:#}").contains(fail));
            assert_eq!(*ops.calls.borrow(), expected);
            assert_eq!(
                *cleanup.borrow(),
                if passive {
                    vec!["release_ip", "stop_udp"]
                } else {
                    vec!["stop_udp"]
                }
            );
        }
    }
}

#[test]
fn successful_setup_transfers_ownership_without_cleanup_for_all_platform_paths() {
    for (userspace, existed, expected) in [
        (true, false, vec!["start_userspace", "wait_userspace"]),
        (true, true, vec!["start_userspace", "wait_userspace"]),
        (false, false, vec!["port", "write", "start_kernel"]),
        (false, true, vec!["add", "wait_kernel"]),
    ] {
        let ops = Ops {
            fail: "",
            calls: RefCell::new(vec![]),
        };
        let (handle, native) = finish_setup(
            configure_peer_with(&ops, userspace, existed, Path::new("unit.conf")),
            || panic!("successful resource was cleaned up"),
        )
        .unwrap();
        assert_eq!(handle.conf_path, Path::new("unit.conf"));
        assert_eq!(handle.tunnel_name, if userspace { "" } else { "wg0" });
        assert_eq!(
            native.map(|x| x.handle_id),
            if userspace {
                Some("native-peer".into())
            } else {
                None
            }
        );
        assert_eq!(*ops.calls.borrow(), expected);
    }
}

#[test]
fn kernel_start_and_handshake_failures_clean_only_a_started_interface() {
    for fail in ["", "start", "wait"] {
        let calls = RefCell::new(vec![]);
        let result = start_kernel_with(
            || {
                calls.borrow_mut().push("start");
                if fail == "start" {
                    return Err(coded(ErrorCode::WireguardConfigureFailed, "start failed"));
                }
                Ok(wgvpn::TunnelHandle {
                    tunnel_name: "wg0".into(),
                    conf_path: "unit.conf".into(),
                })
            },
            |h| {
                assert_eq!(h.tunnel_name, "wg0");
                calls.borrow_mut().push("wait");
                if fail == "wait" {
                    Err(coded(ErrorCode::WireguardHandshakeTimeout, "no handshake"))
                } else {
                    Ok(())
                }
            },
            |h| {
                assert_eq!(h.tunnel_name, "wg0");
                calls.borrow_mut().push("stop");
            },
        );
        match fail {
            "start" => {
                assert_eq!(
                    crate::error::code_of(&result.err().unwrap()),
                    ErrorCode::WireguardConfigureFailed
                );
                assert_eq!(*calls.borrow(), vec!["start"]);
            }
            "wait" => {
                assert_eq!(
                    crate::error::code_of(&result.err().unwrap()),
                    ErrorCode::WireguardHandshakeTimeout
                );
                assert_eq!(*calls.borrow(), vec!["start", "wait", "stop"]);
            }
            _ => {
                assert_eq!(result.unwrap().tunnel_name, "wg0");
                assert_eq!(*calls.borrow(), vec!["start", "wait"]);
            }
        }
    }
}
