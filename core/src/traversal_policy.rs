//! Application policy; the gonc-compatible punch library owns actual probing.
use anyhow::{anyhow, Result};
use serde::{Deserialize, Serialize};

pub const VERSION: u8 = 2;

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
            return Err(anyhow!("unsupported_traversal_negotiation"));
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
    let valid = |value: &&NatEvidence| {
        value.network == network && matches!(value.nat_type.as_str(), "easy" | "hard" | "symm")
    };
    let left: Vec<_> = local.iter().filter(valid).collect();
    let right: Vec<_> = remote.iter().filter(valid).collect();
    if left.is_empty() || right.is_empty() {
        return false;
    }
    network.starts_with("udp")
        || (matches!(network, "tcp4" | "tcp6")
            && (left.iter().any(|v| v.nat_type == "easy")
                || right.iter().any(|v| v.nat_type == "easy")))
}

pub fn build_plan(preferences: Preferences, local: &[NatEvidence], remote: &[NatEvidence]) -> Vec<String> {
    network_order(preferences).into_iter()
        .filter(|network| eligible(network, local, remote))
        .map(str::to_string).collect()
}

/// Fresh evidence is gathered for every attempt; no cross-network cache.
pub async fn collect_evidence() -> Vec<NatEvidence> {
    let result = p2premote_punch::api::detect_nat(
        &["udp4", "udp6", "tcp4", "tcp6"], std::time::Duration::from_secs(6),
    ).await;
    let mut evidence = result.unwrap_or_default().into_iter()
        .filter(|v| matches!(v.network.as_str(), "udp4" | "udp6" | "tcp4" | "tcp6"))
        .map(|v| NatEvidence { network: v.network, nat_type: v.nat_type })
        .collect::<Vec<_>>();
    evidence.sort_by(|a, b| (&a.network, &a.nat_type).cmp(&(&b.network, &b.nat_type)));
    evidence.dedup();
    evidence.truncate(32);
    evidence
}

#[cfg(test)]
mod tests {
    use super::*;
    fn nat(network: &str, nat_type: &str) -> NatEvidence {
        NatEvidence { network: network.into(), nat_type: nat_type.into() }
    }
    #[test]
    fn tcp_requires_easy_on_the_same_transport_and_family() {
        let local = vec![nat("udp4", "easy"), nat("tcp4", "hard"), nat("tcp6", "easy")];
        let remote = vec![nat("udp4", "symm"), nat("tcp4", "symm"), nat("tcp6", "hard")];
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
                let p = Preferences { prefer_ipv6, prefer_tcp };
                assert_eq!(build_plan(p, &all, &all), network_order(p));
            }
        }
        let hard = ["udp4", "udp6", "tcp4", "tcp6"].map(|n| nat(n, "hard"));
        assert_eq!(build_plan(Preferences { prefer_ipv6: true, prefer_tcp: true }, &hard, &hard), ["udp6", "udp4"]);
        assert!(build_plan(Preferences::default(), &[], &all).is_empty());
    }
}
