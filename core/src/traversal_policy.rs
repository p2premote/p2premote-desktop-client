//! Application policy; the gonc-compatible punch library owns actual probing.
use crate::error::{coded, ErrorCode};
use anyhow::Result;
use serde::{Deserialize, Serialize};

pub const VERSION: u8 = 2;

/// Public IPv6 traversal needs a global unicast source, not a LAN-only address.
pub fn usable_ipv6(ip: std::net::Ipv6Addr) -> bool {
    let bytes = ip.octets();
    bytes[0] & 0xe0 == 0x20 && bytes[..4] != [0x20, 0x01, 0x0d, 0xb8]
}

/// Failure to enumerate addresses means unknown, never confirmed absent.
pub fn ipv6_available() -> Option<bool> {
    use network_interface::{Addr, NetworkInterface, NetworkInterfaceConfig};
    let interfaces = NetworkInterface::show().ok()?;
    Some(interfaces.into_iter().any(|interface| {
        let name = interface.name.to_ascii_lowercase();
        if ["loopback", "wg", "tun", "tap", "tailscale"]
            .iter()
            .any(|prefix| name.starts_with(prefix))
        {
            return false;
        }
        interface
            .addr
            .into_iter()
            .any(|addr| matches!(addr, Addr::V6(v6) if usable_ipv6(v6.ip)))
    }))
}

pub fn tcp_retry_recommended(
    prefer_tcp: bool,
    code: &crate::error::ErrorCode,
    attempted_networks: &[String],
) -> bool {
    prefer_tcp
        && (matches!(code, crate::error::ErrorCode::TraversalExecutionExhausted)
            || code.retry_class() == "next_network_or_attempt")
        && attempted_networks
            .iter()
            .any(|network| matches!(network.as_str(), "tcp4" | "tcp6"))
}

pub fn symmetric_nat_help_recommended(error_code: &str, local_nat: &str, remote_nat: &str) -> bool {
    matches!(
        crate::error::ErrorCode::parse(error_code),
        crate::error::ErrorCode::TraversalExecutionExhausted
            | crate::error::ErrorCode::TraversalPunchTimeout
            | crate::error::ErrorCode::TraversalPunchFailed
            | crate::error::ErrorCode::TraversalCandidateSelectionNoUsableCandidate
    ) && (local_nat.eq_ignore_ascii_case("symm") || remote_nat.eq_ignore_ascii_case("symm"))
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct Preferences {
    pub prefer_ipv6: bool,
    pub prefer_tcp: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Negotiation {
    pub version: u8,
    pub preferences: Preferences,
}

impl Negotiation {
    pub fn validate(&self) -> Result<()> {
        if self.version != VERSION {
            return Err(coded(
                ErrorCode::TraversalCapabilityExchangeInvalidPayload,
                format!("unsupported_traversal_negotiation"),
            ));
        }
        Ok(())
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct NatEvidence {
    pub network: String,
    pub nat_type: String,
}

pub fn network_order(preferences: Preferences) -> [&'static str; 4] {
    match (preferences.prefer_ipv6, preferences.prefer_tcp) {
        (false, false) => ["udp4", "udp6", "tcp4", "tcp6"],
        (true, false) => ["udp6", "udp4", "tcp6", "tcp4"],
        (false, true) => ["tcp4", "tcp6", "udp4", "udp6"],
        (true, true) => ["tcp6", "tcp4", "udp6", "udp4"],
    }
}

pub fn eligible(network: &str, local: &[NatEvidence], remote: &[NatEvidence]) -> bool {
    // Both v2 implementations support these UDP transports. A failed STUN
    // preflight is not evidence that the library cannot punch with them.
    if matches!(network, "udp4" | "udp6") {
        return true;
    }
    let valid = |value: &&NatEvidence| {
        value.network == network && matches!(value.nat_type.as_str(), "easy" | "hard" | "symm")
    };
    let left: Vec<_> = local.iter().filter(valid).collect();
    let right: Vec<_> = remote.iter().filter(valid).collect();
    if left.is_empty() || right.is_empty() {
        return false;
    }
    matches!(network, "tcp4" | "tcp6")
        && (left.iter().any(|v| v.nat_type == "easy") || right.iter().any(|v| v.nat_type == "easy"))
}

pub fn build_plan_with_family(
    preferences: Preferences,
    local: &[NatEvidence],
    remote: &[NatEvidence],
    local_ipv6: Option<bool>,
    remote_ipv6: Option<bool>,
) -> Vec<String> {
    network_order(preferences)
        .into_iter()
        .filter(|network| {
            !network.ends_with('6') || (local_ipv6 != Some(false) && remote_ipv6 != Some(false))
        })
        .filter(|network| eligible(network, local, remote))
        .map(str::to_string)
        .collect()
}

/// Fresh evidence is gathered for every attempt; no cross-network cache.
pub async fn collect_evidence(ipv6: Option<bool>) -> Vec<NatEvidence> {
    let networks: &[&str] = if ipv6 == Some(false) {
        &["udp4", "tcp4"]
    } else {
        &["udp4", "udp6", "tcp4", "tcp6"]
    };
    let result =
        p2premote_punch::api::detect_nat(networks, std::time::Duration::from_secs(6)).await;
    let mut evidence = result
        .unwrap_or_default()
        .into_iter()
        .filter(|v| matches!(v.network.as_str(), "udp4" | "udp6" | "tcp4" | "tcp6"))
        .map(|v| NatEvidence {
            network: v.network,
            nat_type: v.nat_type,
        })
        .collect::<Vec<_>>();
    evidence.sort_by(|a, b| (&a.network, &a.nat_type).cmp(&(&b.network, &b.nat_type)));
    evidence.dedup();
    evidence.truncate(32);
    evidence
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn shared_protocol_vectors_match_android() {
        #[derive(Deserialize)]
        struct PlanCase {
            name: String,
            preferences: Preferences,
            local: String,
            remote: String,
            local_ipv6: Option<bool>,
            remote_ipv6: Option<bool>,
            expected: Vec<String>,
        }
        #[derive(Deserialize)]
        struct Vectors {
            version: u8,
            evidence_sets: std::collections::HashMap<String, Vec<NatEvidence>>,
            plan_cases: Vec<PlanCase>,
        }
        let vectors: Vectors = serde_json::from_str(include_str!(
            "../../../p2premote-punch-rs/testdata/traversal-policy-v2.json"
        ))
        .unwrap();
        assert_eq!(vectors.version, VERSION);
        for case in vectors.plan_cases {
            assert_eq!(
                build_plan_with_family(
                    case.preferences,
                    &vectors.evidence_sets[&case.local],
                    &vectors.evidence_sets[&case.remote],
                    case.local_ipv6,
                    case.remote_ipv6
                ),
                case.expected,
                "{}",
                case.name
            );
        }
    }
    fn nat(network: &str, nat_type: &str) -> NatEvidence {
        NatEvidence {
            network: network.into(),
            nat_type: nat_type.into(),
        }
    }
    #[test]
    fn tcp_requires_easy_on_the_same_transport_and_family() {
        let local = vec![
            nat("udp4", "easy"),
            nat("tcp4", "hard"),
            nat("tcp6", "easy"),
        ];
        let remote = vec![
            nat("udp4", "symm"),
            nat("tcp4", "symm"),
            nat("tcp6", "hard"),
        ];
        assert!(!eligible("tcp4", &local, &remote));
        assert!(eligible("tcp6", &local, &remote));
        assert!(eligible("tcp6", &remote, &local));
        assert!(eligible("udp4", &local, &remote));
        assert!(!eligible("tcp6", &local, &[nat("tcp6", "unknown")]));
    }
    #[test]
    fn every_preference_combination_filters_before_sorting() {
        let all = ["udp4", "udp6", "tcp4", "tcp6"].map(|n| nat(n, "easy"));
        for prefer_ipv6 in [false, true] {
            for prefer_tcp in [false, true] {
                let p = Preferences {
                    prefer_ipv6,
                    prefer_tcp,
                };
                assert_eq!(
                    build_plan_with_family(p, &all, &all, None, None),
                    network_order(p)
                );
            }
        }
        let hard = ["udp4", "udp6", "tcp4", "tcp6"].map(|n| nat(n, "hard"));
        assert_eq!(
            build_plan_with_family(
                Preferences {
                    prefer_ipv6: true,
                    prefer_tcp: true
                },
                &hard,
                &hard,
                None,
                None
            ),
            ["udp6", "udp4"]
        );
        assert_eq!(
            build_plan_with_family(Preferences::default(), &[], &all, None, None),
            ["udp4", "udp6"]
        );
    }
    #[test]
    fn failed_nat_preflight_keeps_udp_fallback_but_never_grants_tcp() {
        let unknown = [nat("tcp4", "unknown"), nat("tcp6", "unknown")];
        let all = ["udp4", "udp6", "tcp4", "tcp6"].map(|n| nat(n, "easy"));
        let preferences = Preferences {
            prefer_ipv6: true,
            prefer_tcp: true,
        };
        assert_eq!(
            build_plan_with_family(preferences, &[], &[], None, None),
            ["udp6", "udp4"]
        );
        assert_eq!(
            build_plan_with_family(preferences, &unknown, &all, None, None),
            ["udp6", "udp4"]
        );
        assert_eq!(
            build_plan_with_family(preferences, &all, &unknown, None, None),
            ["udp6", "udp4"]
        );
    }
    #[test]
    fn either_endpoint_without_ipv6_excludes_both_ipv6_transports() {
        let all = ["udp4", "udp6", "tcp4", "tcp6"].map(|n| nat(n, "easy"));
        for prefer_ipv6 in [false, true] {
            for prefer_tcp in [false, true] {
                let preferences = Preferences {
                    prefer_ipv6,
                    prefer_tcp,
                };
                for other in [None, Some(false), Some(true)] {
                    for (local, remote) in [(Some(false), other), (other, Some(false))] {
                        let plan = build_plan_with_family(preferences, &all, &all, local, remote);
                        assert!(plan.iter().all(|network| !network.ends_with('6')));
                        assert!(plan.contains(&"udp4".to_string()));
                        assert!(plan.contains(&"tcp4".to_string()));
                    }
                }
            }
        }
        assert_eq!(
            build_plan_with_family(
                Preferences {
                    prefer_ipv6: true,
                    prefer_tcp: true
                },
                &[],
                &[],
                None,
                Some(true)
            ),
            ["udp6", "udp4"]
        );
    }
    #[test]
    fn ipv6_address_check_excludes_local_and_documentation_addresses() {
        for address in ["::", "::1", "fe80::1", "fd00::1", "ff02::1", "2001:db8::1"] {
            assert!(!usable_ipv6(address.parse().unwrap()), "{address}");
        }
        assert!(usable_ipv6("2001:4860:4860::8888".parse().unwrap()));
    }
    #[test]
    fn tcp_hint_uses_code_and_attempted_networks() {
        use crate::error::ErrorCode;
        let networks = vec!["tcp4".to_owned(), "udp4".to_owned()];
        assert!(tcp_retry_recommended(
            true,
            &ErrorCode::TraversalPunchTimeout,
            &networks
        ));
        assert!(!tcp_retry_recommended(
            true,
            &ErrorCode::TraversalLocalForwardIoFailed,
            &networks
        ));
        assert!(tcp_retry_recommended(
            true,
            &ErrorCode::TraversalExecutionExhausted,
            &networks
        ));
        assert!(!tcp_retry_recommended(
            false,
            &ErrorCode::TraversalExecutionExhausted,
            &networks
        ));
        assert!(!tcp_retry_recommended(
            true,
            &ErrorCode::ConnectionExecutionUserCancelled,
            &networks
        ));
        assert!(!tcp_retry_recommended(
            true,
            &ErrorCode::TraversalExecutionExhausted,
            &["udp4".into()]
        ));
    }
    #[test]
    fn symmetric_nat_help_requires_network_punch_failure() {
        assert!(symmetric_nat_help_recommended(
            "traversal.punch.timeout",
            "symm",
            "easy"
        ));
        assert!(symmetric_nat_help_recommended(
            "traversal.punch.timeout",
            "hard",
            "SYMM"
        ));
        assert!(!symmetric_nat_help_recommended(
            "runtime.execution.unclassified",
            "symm",
            "symm"
        ));
        assert!(!symmetric_nat_help_recommended(
            "traversal.punch.timeout",
            "hard",
            "easy"
        ));
    }
}
