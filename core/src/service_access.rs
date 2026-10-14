//! Device-local ingress policy. LAN forwarding is outside this policy's scope.
use anyhow::{anyhow, Result};
use once_cell::sync::Lazy;
use parking_lot::Mutex;
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AccessMode {
    #[default]
    All,
    Allowlist,
    Deny,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PortRange {
    pub start: u16,
    pub end: u16,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ServiceAccessPolicy {
    pub mode: AccessMode,
    pub tcp: Vec<PortRange>,
    pub udp: Vec<PortRange>,
    pub allow_ping: bool,
}

impl ServiceAccessPolicy {
    pub fn normalized(mut self) -> Result<Self> {
        for ranges in [&mut self.tcp, &mut self.udp] {
            if ranges.len() > 256 {
                return Err(anyhow!("At most 256 port ranges per protocol"));
            }
            if ranges.iter().any(|r| r.start == 0 || r.start > r.end) {
                return Err(anyhow!("Ports must be 1–65535, with start <= end"));
            }
            ranges.sort_by_key(|r| (r.start, r.end));
            let mut merged: Vec<PortRange> = Vec::new();
            for range in ranges.drain(..) {
                if let Some(last) = merged.last_mut() {
                    if u32::from(range.start) <= u32::from(last.end) + 1 {
                        last.end = last.end.max(range.end);
                        continue;
                    }
                }
                merged.push(range);
            }
            *ranges = merged;
        }
        Ok(self)
    }
}

static APPLIED: Lazy<Mutex<Option<ServiceAccessPolicy>>> = Lazy::new(|| Mutex::new(None));

pub fn status() -> Result<serde_json::Value> {
    let applied = APPLIED.lock();
    let policy = crate::config::load_machine_config()?
        .service_access
        .normalized()?;
    Ok(serde_json::json!({ "policy": policy, "applied": applied.as_ref() == Some(&policy) }))
}

pub fn initialize(config: &crate::config::MachineConfig) -> Result<()> {
    let mut applied = APPLIED.lock();
    let policy = config.service_access.clone().normalized()?;
    if let Err(error) = apply_backend(&policy, config) {
        if let Some(previous) = applied.clone() {
            let _ = rollback_policy(&previous, config, &mut applied);
        }
        return Err(error);
    }
    *applied = Some(policy);
    Ok(())
}

pub fn save(policy: ServiceAccessPolicy) -> Result<serde_json::Value> {
    let policy = policy.normalized()?;
    let mut applied = APPLIED.lock();
    let config = crate::config::load_machine_config()?;
    let previous = applied
        .clone()
        .unwrap_or(config.service_access.clone())
        .normalized()?;
    if let Err(error) = apply_backend(&policy, &config) {
        if let Err(rollback) = rollback_policy(&previous, &config, &mut applied) {
            return Err(anyhow!(
                "Policy apply failed: {error}; rollback failed: {rollback}"
            ));
        }
        return Err(error);
    }
    if let Err(error) = crate::config::update_machine_settings(|config| {
        config.service_access = policy.clone();
        Ok(())
    }) {
        if let Err(rollback) = rollback_policy(&previous, &config, &mut applied) {
            return Err(anyhow!(
                "Policy persistence failed: {error}; rollback failed: {rollback}"
            ));
        }
        return Err(error);
    }
    *applied = Some(policy.clone());
    Ok(serde_json::json!({ "policy": policy, "applied": true }))
}

fn rollback_policy(
    previous: &ServiceAccessPolicy,
    config: &crate::config::MachineConfig,
    applied: &mut Option<ServiceAccessPolicy>,
) -> Result<()> {
    if let Err(error) = apply_backend(previous, config) {
        fail_closed(config);
        *applied = None;
        return Err(error);
    }
    *applied = Some(previous.clone());
    Ok(())
}

fn fail_closed(config: &crate::config::MachineConfig) {
    let deny = ServiceAccessPolicy {
        mode: AccessMode::Deny,
        ..Default::default()
    };
    if let Err(error) = apply_backend(&deny, config) {
        tracing::error!("Cannot install emergency service deny policy: {error}");
        if let Err(cleanup) = crate::p2p::wgvpn_flow::cleanup_stale_sessions(config) {
            tracing::error!("Emergency tunnel cleanup failed: {cleanup}");
        }
    }
}

pub fn ensure_ready() -> Result<()> {
    if APPLIED.lock().is_none() {
        return Err(anyhow!(
            "Service access policy is not ready; save the policy before connecting"
        ));
    }
    Ok(())
}

#[cfg(not(target_os = "linux"))]
fn apply_backend(
    policy: &ServiceAccessPolicy,
    config: &crate::config::MachineConfig,
) -> Result<()> {
    crate::gonc_ffi::set_service_access_policy(std::path::Path::new(&config.p2p_punch_path), policy)
}

#[cfg(target_os = "linux")]
fn apply_backend(policy: &ServiceAccessPolicy, _: &crate::config::MachineConfig) -> Result<()> {
    linux::apply(policy)
}

#[cfg(any(target_os = "linux", test))]
mod linux {
    use super::*;
    #[cfg(target_os = "linux")]
    use std::io::Write;
    #[cfg(target_os = "linux")]
    use std::process::{Command, Stdio};
    #[cfg(target_os = "linux")]
    use std::time::Duration;
    #[cfg(target_os = "linux")]
    use wait_timeout::ChildExt;
    const CHAIN: &str = "P2PREMOTE_SERVICES";

    #[cfg(target_os = "linux")]
    fn run(program: &str, args: &[&str], input: Option<&str>) -> Result<bool> {
        let mut child = Command::new(program)
            .args(args)
            .stdin(if input.is_some() {
                Stdio::piped()
            } else {
                Stdio::null()
            })
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn()?;
        if let Some(input) = input {
            child
                .stdin
                .take()
                .ok_or_else(|| anyhow!("Missing firewall stdin"))?
                .write_all(input.as_bytes())?;
        }
        match child.wait_timeout(Duration::from_secs(15))? {
            Some(status) => Ok(status.success()),
            None => {
                let _ = child.kill();
                let _ = child.wait();
                Err(anyhow!("Firewall command timed out: {program}"))
            }
        }
    }

    pub(super) fn rules(policy: &ServiceAccessPolicy, ipv6: bool) -> String {
        let mut out = format!("*filter\n:{CHAIN} - [0:0]\n-F {CHAIN}\n");
        if policy.mode == AccessMode::All {
            out.push_str(&format!("-A {CHAIN} -j RETURN\n"));
        } else {
            // The dedicated, peer-validated health listener must stay reachable.
            out.push_str(&format!(
                "-A {CHAIN} -p tcp --dport {} -j RETURN\n",
                crate::health::HEALTH_PORT
            ));
            // Only replies to locally initiated flows bypass the whitelist.
            out.push_str(&format!(
                "-A {CHAIN} -m conntrack --ctstate ESTABLISHED,RELATED --ctdir REPLY -j RETURN\n"
            ));
            if policy.mode == AccessMode::Allowlist {
                for (protocol, ranges) in [("tcp", &policy.tcp), ("udp", &policy.udp)] {
                    for range in ranges {
                        out.push_str(&format!(
                            "-A {CHAIN} -p {protocol} --dport {}:{} -j RETURN\n",
                            range.start, range.end
                        ));
                    }
                }
                if policy.allow_ping {
                    let ping = if ipv6 {
                        "ipv6-icmp --icmpv6-type echo-request"
                    } else {
                        "icmp --icmp-type echo-request"
                    };
                    out.push_str(&format!("-A {CHAIN} -p {ping} -j RETURN\n"));
                }
            }
            out.push_str(&format!("-A {CHAIN} -j DROP\n"));
        }
        out.push_str("COMMIT\n");
        out
    }

    #[cfg(target_os = "linux")]
    pub(super) fn apply(policy: &ServiceAccessPolicy) -> Result<()> {
        for (tool, restore, ipv6) in [
            ("iptables", "iptables-restore", false),
            ("ip6tables", "ip6tables-restore", true),
        ] {
            let exists = match run(tool, &["-w", "5", "-S", CHAIN], None) {
                Ok(exists) => exists,
                Err(error)
                    if policy.mode == AccessMode::All
                        && error
                            .downcast_ref::<std::io::Error>()
                            .is_some_and(|e| e.kind() == std::io::ErrorKind::NotFound) =>
                {
                    continue
                }
                Err(error) => return Err(error),
            };
            if !exists && policy.mode == AccessMode::All {
                continue;
            }
            if !exists && !run(tool, &["-w", "5", "-N", CHAIN], None)? {
                return Err(anyhow!("Cannot create service firewall chain: {tool}"));
            }
            // Populate before attaching; --noflush preserves other firewall rules.
            if !run(
                restore,
                &["--noflush", "-w", "5"],
                Some(&rules(policy, ipv6)),
            )? {
                return Err(anyhow!("Cannot apply service firewall rules: {restore}"));
            }
            // Remove every old hook, including duplicates or hooks displaced by
            // another firewall manager, then recreate one at the head of INPUT.
            while run(
                tool,
                &["-w", "5", "-C", "INPUT", "-i", "wg0", "-j", CHAIN],
                None,
            )? {
                if !run(
                    tool,
                    &["-w", "5", "-D", "INPUT", "-i", "wg0", "-j", CHAIN],
                    None,
                )? {
                    return Err(anyhow!("Cannot remove old service firewall hook: {tool}"));
                }
            }
            if !run(
                tool,
                &["-w", "5", "-I", "INPUT", "1", "-i", "wg0", "-j", CHAIN],
                None,
            )? {
                return Err(anyhow!("Cannot attach service firewall chain: {tool}"));
            }
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[cfg(target_os = "linux")]
    #[test]
    #[ignore = "requires root and an isolated network namespace"]
    fn linux_backend_installs_and_updates_only_its_own_chain() {
        use std::process::Command;
        assert_ne!(
            std::fs::read_link("/proc/self/ns/net").unwrap(),
            std::fs::read_link("/proc/1/ns/net").unwrap(),
            "Run this test with unshare --net; never modify the host firewall"
        );
        fn command(tool: &str, args: &[&str]) -> String {
            let output = Command::new(tool).args(args).output().unwrap();
            assert!(
                output.status.success(),
                "{tool}: {}",
                String::from_utf8_lossy(&output.stderr)
            );
            String::from_utf8(output.stdout).unwrap()
        }
        command("ip", &["link", "add", "wg0", "type", "dummy"]);
        command("iptables", &["-N", "UNRELATED_TEST"]);
        command("iptables", &["-A", "INPUT", "-j", "UNRELATED_TEST"]);
        let policy = ServiceAccessPolicy {
            mode: AccessMode::Allowlist,
            tcp: vec![PortRange {
                start: 3389,
                end: 3389,
            }],
            udp: vec![PortRange {
                start: 1000,
                end: 2000,
            }],
            allow_ping: true,
        };
        linux::apply(&policy).unwrap();
        // Displace and duplicate the existing hooks before updating the policy.
        for tool in ["iptables", "ip6tables"] {
            command(tool, &["-I", "INPUT", "1", "-i", "wg0", "-j", "ACCEPT"]);
            command(
                tool,
                &["-A", "INPUT", "-i", "wg0", "-j", "P2PREMOTE_SERVICES"],
            );
        }
        linux::apply(&policy).unwrap();
        for tool in ["iptables", "ip6tables"] {
            let rules = command(tool, &["-S", "P2PREMOTE_SERVICES"]);
            assert!(rules.contains("--dport 3389"));
            assert!(rules.contains("--dport 1000:2000"));
            assert!(rules.contains("--ctdir REPLY"));
            let input = command(tool, &["-S", "INPUT"]);
            let first_rule = input
                .lines()
                .find(|line| line.starts_with("-A INPUT "))
                .unwrap();
            assert_eq!(first_rule, "-A INPUT -i wg0 -j P2PREMOTE_SERVICES");
            assert_eq!(input.matches("-i wg0 -j P2PREMOTE_SERVICES").count(), 1);
        }
        linux::apply(&ServiceAccessPolicy {
            mode: AccessMode::Deny,
            ..Default::default()
        })
        .unwrap();
        for tool in ["iptables", "ip6tables"] {
            let rules = command(tool, &["-S", "P2PREMOTE_SERVICES"]);
            assert!(!rules.contains("--dport 3389"));
            assert!(rules.contains("--dport 41119"));
            assert!(rules.contains("-j DROP"));
        }
        linux::apply(&Default::default()).unwrap();
        assert!(!command("iptables", &["-S", "P2PREMOTE_SERVICES"]).contains("DROP"));
        assert!(command("iptables", &["-S", "INPUT"]).contains("-j UNRELATED_TEST"));
    }
    #[cfg(windows)]
    #[test]
    fn userspace_policy_abi_accepts_valid_and_rejects_invalid_ranges() {
        let library = std::env::current_exe()
            .unwrap()
            .parent()
            .unwrap()
            .join("p2premote-wg.dll");
        let capabilities = crate::gonc_ffi::get_wg_capabilities(&library).unwrap();
        assert_eq!(capabilities.abi_version, 3);
        let invalid = ServiceAccessPolicy {
            mode: AccessMode::Allowlist,
            tcp: vec![PortRange { start: 0, end: 100 }],
            ..Default::default()
        };
        assert!(crate::gonc_ffi::set_service_access_policy(&library, &invalid).is_err());
        crate::gonc_ffi::set_service_access_policy(
            &library,
            &ServiceAccessPolicy {
                mode: AccessMode::Deny,
                ..Default::default()
            },
        )
        .unwrap();
        crate::gonc_ffi::set_service_access_policy(&library, &Default::default()).unwrap();
    }
    #[test]
    fn linux_rules_preserve_reply_direction_and_restrict_current_connections() {
        let policy = ServiceAccessPolicy {
            mode: AccessMode::Allowlist,
            tcp: vec![PortRange {
                start: 1000,
                end: 2000,
            }],
            allow_ping: true,
            ..Default::default()
        };
        let v4 = linux::rules(&policy, false);
        assert!(v4.contains("--ctdir REPLY"));
        assert!(v4.contains("-p tcp --dport 1000:2000 -j RETURN"));
        assert!(v4.contains("-p icmp --icmp-type echo-request"));
        assert!(v4.ends_with("-A P2PREMOTE_SERVICES -j DROP\nCOMMIT\n"));
        let v6 = linux::rules(&policy, true);
        assert!(v6.contains("-p ipv6-icmp --icmpv6-type echo-request"));
        let deny = linux::rules(
            &ServiceAccessPolicy {
                mode: AccessMode::Deny,
                ..policy
            },
            false,
        );
        assert!(!deny.contains("--dport 1000:2000"));
        assert!(deny.contains(&format!("--dport {}", crate::health::HEALTH_PORT)));
        assert!(!deny.contains("echo-request"));
        let all = linux::rules(&Default::default(), false);
        assert!(!all.contains("DROP"));
    }
    #[test]
    fn ranges_are_validated_and_merged() {
        let policy = ServiceAccessPolicy {
            tcp: vec![
                PortRange {
                    start: 1001,
                    end: 2000,
                },
                PortRange {
                    start: 1000,
                    end: 1000,
                },
            ],
            ..Default::default()
        }
        .normalized()
        .unwrap();
        assert_eq!(
            policy.tcp,
            vec![PortRange {
                start: 1000,
                end: 2000
            }]
        );
        for (start, end) in [(0, 1), (20, 10)] {
            assert!(ServiceAccessPolicy {
                udp: vec![PortRange { start, end }],
                ..Default::default()
            }
            .normalized()
            .is_err());
        }
    }
    #[test]
    fn legacy_config_keeps_unrestricted_policy() {
        let policy: ServiceAccessPolicy = Default::default();
        assert_eq!(policy.mode, AccessMode::All);
        assert!(serde_json::from_str::<ServiceAccessPolicy>(
            r#"{"mode":"allowlist","tcp":[],"udp":[],"allow_ping":false}"#
        )
        .is_ok());
    }
}
