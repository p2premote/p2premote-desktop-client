use anyhow::{anyhow, Result};
use network_interface::{Addr, NetworkInterface, NetworkInterfaceConfig};
use serde::{Deserialize, Serialize};
use std::collections::HashSet;
use std::net::{Ipv4Addr, SocketAddrV4};
use tokio::net::UdpSocket;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct WOLInterface {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub mac: Option<String>,
    pub ipv4: String,
    pub prefix_len: u8,
    #[serde(default)]
    pub priority: i32,
}
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct WOLCapability {
    pub version: u8,
    pub can_relay: bool,
    pub interfaces: Vec<WOLInterface>,
}

pub fn collect_capability() -> Option<WOLCapability> {
    let mut interfaces = Vec::new();
    for iface in NetworkInterface::show().ok()? {
        let name = iface.name.to_ascii_lowercase();
        if [
            "loopback",
            "docker",
            "veth",
            "vmnet",
            "virtualbox",
            "utun",
            "wg",
            "tailscale",
            "tun",
            "tap",
        ]
        .iter()
        .any(|x| name.contains(x))
        {
            continue;
        }
        let mac = iface.mac_addr.as_deref().and_then(normalize_mac);
        for addr in iface.addr {
            if let Addr::V4(v4) = addr {
                if v4.ip.is_loopback() || v4.ip.is_link_local() {
                    continue;
                };
                let Some(netmask) = v4.netmask else { continue };
                let prefix = netmask.octets().iter().map(|b| b.count_ones()).sum::<u32>() as u8;
                if prefix == 0 {
                    continue;
                };
                interfaces.push(WOLInterface {
                    mac: mac.clone(),
                    ipv4: v4.ip.to_string(),
                    prefix_len: prefix,
                    priority: if interfaces.is_empty() { 100 } else { 0 },
                });
            }
        }
    }
    if interfaces.is_empty() {
        None
    } else {
        Some(WOLCapability {
            version: 1,
            can_relay: true,
            interfaces,
        })
    }
}

pub fn normalize_mac(value: &str) -> Option<String> {
    let hex: String = value
        .chars()
        .filter(|c| c.is_ascii_hexdigit())
        .map(|c| c.to_ascii_lowercase())
        .collect();
    if hex.len() != 12 || hex == "000000000000" {
        return None;
    };
    let bytes: Vec<u8> = (0..6)
        .filter_map(|i| u8::from_str_radix(&hex[i * 2..i * 2 + 2], 16).ok())
        .collect();
    if bytes.len() != 6 || bytes[0] & 1 != 0 {
        return None;
    };
    Some(
        bytes
            .iter()
            .map(|b| format!("{b:02x}"))
            .collect::<Vec<_>>()
            .join(":"),
    )
}
pub fn magic_packet(mac: &str) -> Result<Vec<u8>> {
    let normalized = normalize_mac(mac).ok_or_else(|| anyhow!("invalid_mac"))?;
    let bytes = normalized
        .split(':')
        .map(|x| u8::from_str_radix(x, 16))
        .collect::<std::result::Result<Vec<_>, _>>()?;
    let mut packet = vec![0xff; 6];
    for _ in 0..16 {
        packet.extend_from_slice(&bytes)
    }
    Ok(packet)
}
fn broadcast(ip: Ipv4Addr, prefix: u8) -> Result<Ipv4Addr> {
    if prefix == 0 || prefix > 32 {
        return Err(anyhow!("invalid_prefix"));
    };
    let raw = u32::from(ip);
    let mask = u32::MAX.checked_shl((32 - prefix) as u32).unwrap_or(0);
    Ok(Ipv4Addr::from(raw | !mask))
}

fn source_ipv4_for_target(target: Ipv4Addr, prefix: u8) -> Result<Ipv4Addr> {
    let target_network = u32::from(target)
        & u32::from(Ipv4Addr::from(
            u32::MAX.checked_shl((32 - prefix) as u32).unwrap_or(0),
        ));
    for iface in NetworkInterface::show()? {
        for addr in iface.addr {
            if let Addr::V4(v4) = addr {
                if !v4.ip.is_loopback()
                    && !v4.ip.is_link_local()
                    && (u32::from(v4.ip)
                        & u32::from(Ipv4Addr::from(
                            u32::MAX.checked_shl((32 - prefix) as u32).unwrap_or(0),
                        )))
                        == target_network
                {
                    return Ok(v4.ip);
                }
            }
        }
    }
    Err(anyhow!("no_matching_interface"))
}

pub async fn send_magic_packets(macs: &[String], target_ipv4: &str, prefix_len: u8) -> Result<()> {
    if macs.is_empty() || macs.len() > 8 {
        return Err(anyhow!("invalid_macs"));
    };
    let ip: Ipv4Addr = target_ipv4.parse()?;
    let directed = broadcast(ip, prefix_len)?;
    let source = source_ipv4_for_target(ip, prefix_len)?;
    let socket = UdpSocket::bind(SocketAddrV4::new(source, 0)).await?;
    socket.set_broadcast(true)?;
    let destinations = [directed, Ipv4Addr::BROADCAST];
    let mut sent = 0;
    let mut unique = HashSet::new();
    for mac in macs {
        if !unique.insert(mac) {
            continue;
        };
        let packet = magic_packet(mac)?;
        for dest in destinations {
            for _ in 0..3 {
                socket.send_to(&packet, SocketAddrV4::new(dest, 9)).await?;
                sent += 1
            }
        }
    }
    if sent == 0 {
        Err(anyhow!("nothing_sent"))
    } else {
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn packet_shape() {
        let p = magic_packet("00:11:22:33:44:55").unwrap();
        assert_eq!(p.len(), 102);
        assert_eq!(&p[6..12], &[0, 17, 34, 51, 68, 85]);
    }
    #[test]
    fn broadcast_address() {
        assert_eq!(
            broadcast("192.168.7.3".parse().unwrap(), 24).unwrap(),
            "192.168.7.255".parse::<Ipv4Addr>().unwrap()
        );
    }
}
