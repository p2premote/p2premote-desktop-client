pub mod auth;
pub mod auth_http;
pub mod bytes_codec;
pub mod config;
pub mod control;
pub mod device;
pub mod device_identity;
pub mod health;
pub mod http;
pub mod i18n;
pub mod invite;
pub mod logging;
#[cfg(target_os = "macos")]
pub mod macos_service_management;
pub mod p2p;
pub mod runtime;
pub mod service_control;
pub mod speed_test; // 基于 riperf3 的双向隧道测速（发起端 client / 对端 server）
pub mod subnet_router;
pub mod tunnel_control;
pub mod tunnel_view;
pub mod update;
pub mod ws;
pub mod wol;

pub mod gonc_ffi;
pub mod wgvpn;
pub mod wgvpn_exchange;
