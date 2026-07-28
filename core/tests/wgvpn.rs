//! wgvpn 模块集成测试

use p2premote_core::wgvpn;
use std::path::PathBuf;

#[test]
fn validate_public_key_format_accepts_valid_base64() {
    // WireGuard 公钥是 32 字节 base64 编码，固定 44 字符（含 1 个 = 填充）
    let valid_pubkey = "MDEyMzQ1Njc4OWFiY2RlZjAxMjM0NTY3ODlhYmNkZWY=";
    assert!(wgvpn::validate_public_key(valid_pubkey).is_ok());
}

#[test]
fn validate_public_key_format_rejects_empty() {
    assert!(wgvpn::validate_public_key("").is_err());
}

#[test]
fn validate_public_key_format_rejects_garbage() {
    assert!(wgvpn::validate_public_key("not a valid key!!!").is_err());
}

#[test]
fn private_key_file_roundtrip() {
    // 写私钥 → 读私钥，内容一致
    let tmp = std::env::temp_dir().join(format!("wgvpn-test-{}.key", std::process::id()));
    let expected = "aGkK5l+5l+5l+5l+5l+5l+5l+5l+5l+5l+5l+5l+5l+5l+5l+5l+5l+5=";
    wgvpn::write_private_key(&tmp, expected).unwrap();
    let loaded = wgvpn::read_private_key(&tmp).unwrap();
    assert_eq!(loaded, expected);
    let _ = std::fs::remove_file(&tmp);
}

#[test]
fn tunnel_name_from_conf_path_strips_extension() {
    // wg0.conf → wg0
    let path = PathBuf::from("C:/ProgramData/p2premote/wgvpn/wg0.conf");
    assert_eq!(wgvpn::tunnel_name_from_conf_path(&path), "wg0");
}

#[test]
fn tunnel_name_handles_no_extension() {
    let path = PathBuf::from("/etc/wireguard/wg1");
    assert_eq!(wgvpn::tunnel_name_from_conf_path(&path), "wg1");
}
