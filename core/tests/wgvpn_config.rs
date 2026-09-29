use p2premote_core::wgvpn;

const VALID_PUBKEY: &str = "MDEyMzQ1Njc4OWFiY2RlZjAxMjM0NTY3ODlhYmNkZWY=";

// 与 core::p2p::wgvpn_flow 的网段保持一致（CGNAT 100.99.71.0/24）
const ACTIVE_IP_CIDR: &str = "100.99.71.2/24";
const PASSIVE_IP_CIDR: &str = "100.99.71.1/24";
const WGVPN_CIDR: &str = "100.99.71.0/24";

#[test]
fn active_config_contains_required_fields() {
    let cfg = wgvpn::WgConfigBuilder::new()
        .private_key("priv-aaa")
        .address(PASSIVE_IP_CIDR)
        .listen_port(51820)
        .add_peer(wgvpn::PeerConfig {
            public_key: VALID_PUBKEY.to_string(),
            endpoint: Some("127.0.0.1:51821".to_string()),
            allowed_ips: vec![WGVPN_CIDR.to_string()],
            keepalive: Some(25),
        })
        .build_active();

    let text = cfg.render().unwrap();
    assert!(text.contains("[Interface]"));
    assert!(text.contains("PrivateKey = priv-aaa"));
    assert!(text.contains(&format!("Address = {}", PASSIVE_IP_CIDR)));
    assert!(text.contains("MTU = 1280"));
    assert!(text.contains("ListenPort = 51820"));
    assert!(text.contains("[Peer]"));
    assert!(text.contains("Endpoint = 127.0.0.1:51821"));
    assert!(text.contains("PersistentKeepalive = 25"));
    assert!(text.contains(&format!("AllowedIPs = {}", WGVPN_CIDR)));
}

#[test]
fn passive_config_omits_endpoint_and_keepalive() {
    // 渲染能力测试：endpoint/keepalive 为 None 时不输出对应行
    // （生产被动端 peer 均带显式 Endpoint，见 passive_side_peers_carry_explicit_endpoints）
    let cfg = wgvpn::WgConfigBuilder::new()
        .private_key("priv-bbb")
        .address(ACTIVE_IP_CIDR)
        .listen_port(51820)
        .add_peer(wgvpn::PeerConfig {
            public_key: VALID_PUBKEY.to_string(),
            endpoint: None,
            allowed_ips: vec![WGVPN_CIDR.to_string()],
            keepalive: None,
        })
        .build_active();

    let text = cfg.render().unwrap();
    assert!(text.contains(&format!("Address = {}", ACTIVE_IP_CIDR)));
    assert!(text.contains("MTU = 1280"));
    assert!(!text.contains("Endpoint ="), "被动端不应包含 Endpoint");
    assert!(
        !text.contains("PersistentKeepalive"),
        "被动端不应包含 Keepalive"
    );
}

#[test]
fn render_throws_on_missing_peer() {
    let cfg = wgvpn::WgConfigBuilder::new()
        .private_key("priv")
        .address(PASSIVE_IP_CIDR)
        .listen_port(51820)
        .build_active();
    assert!(cfg.render().is_err(), "缺 peer 应报错");
}

// ============ 多 peer 配置生成测试 ============

#[test]
fn multi_peer_config_renders_all_peers() {
    // 场景：主动端连 3 个被动端，每端一个 [Peer]
    let pubkey_b = "BBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBB=";
    let pubkey_c = "CCCCCCCCCCCCCCCCCCCCCCCCCCCCCCCCCCCCCCCCCCC=";
    let pubkey_d = "DDDDDDDDDDDDDDDDDDDDDDDDDDDDDDDDDDDDDDDDDDD=";

    let cfg = wgvpn::WgConfigBuilder::new()
        .private_key("priv-multi")
        .address("100.99.71.5/24")
        .listen_port(51820)
        .add_peer(wgvpn::PeerConfig::active(
            pubkey_b,
            "127.0.0.1:51821",
            "100.99.71.1",
        ))
        .add_peer(wgvpn::PeerConfig::active(
            pubkey_c,
            "127.0.0.1:51822",
            "100.99.71.3",
        ))
        .add_peer(wgvpn::PeerConfig::active(
            pubkey_d,
            "127.0.0.1:51823",
            "100.99.71.4",
        ))
        .build_active();

    let text = cfg.render().unwrap();
    // 应有 3 个 [Peer] 段
    assert_eq!(text.matches("[Peer]").count(), 3, "应渲染 3 个 [Peer] 段");
    // 每个 peer 都有自己的 Endpoint 和 AllowedIPs
    assert!(text.contains(&format!("PublicKey = {}", pubkey_b)));
    assert!(text.contains("Endpoint = 127.0.0.1:51821"));
    assert!(text.contains("AllowedIPs = 100.99.71.1/32"));

    assert!(text.contains(&format!("PublicKey = {}", pubkey_c)));
    assert!(text.contains("Endpoint = 127.0.0.1:51822"));
    assert!(text.contains("AllowedIPs = 100.99.71.3/32"));

    assert!(text.contains(&format!("PublicKey = {}", pubkey_d)));
    assert!(text.contains("Endpoint = 127.0.0.1:51823"));
    assert!(text.contains("AllowedIPs = 100.99.71.4/32"));
}

#[test]
fn passive_side_peers_carry_explicit_endpoints() {
    // 生产现实：被动端的每个 peer 也用 PeerConfig::active 构造——对端是
    // gonc 本地转发端口，需要显式 Endpoint 才能发起/保持连接
    // （wgvpn_flow.rs 被动路径与主动路径同样走 active）。
    let pubkey_first = "AAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAA=";
    let pubkey_second = "PPPPPPPPPPPPPPPPPPPPPPPPPPPPPPPPPPPPPPPPPPP=";

    let cfg = wgvpn::WgConfigBuilder::new()
        .private_key("priv-passive")
        .address(PASSIVE_IP_CIDR)
        .listen_port(51820)
        .add_peer(wgvpn::PeerConfig::active(
            pubkey_first,
            "127.0.0.1:32000",
            "100.99.71.2",
        ))
        .add_peer(wgvpn::PeerConfig::active(
            pubkey_second,
            "127.0.0.1:32001",
            "100.99.71.5",
        ))
        .build_active();

    let text = cfg.render().unwrap();
    // 被动端两个 [Peer] 段，每个都带 Endpoint 与 Keepalive
    assert_eq!(text.matches("[Peer]").count(), 2);
    assert!(text.contains("Endpoint = 127.0.0.1:32000"));
    assert!(text.contains("Endpoint = 127.0.0.1:32001"));
    assert_eq!(text.matches("PersistentKeepalive = 25").count(), 2);
}
