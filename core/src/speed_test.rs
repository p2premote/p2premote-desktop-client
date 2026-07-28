//! 基于 riperf3 的隧道测速（主动端 client / 被动端 server）。
//!
//! 架构选择依据真机验证：wgvpn userspace 后端下，被动端没有 TUN 接口，
//! 无法主动连主动端；但**主动端→被动端方向可达**（与 health TCP 同路）。
//! 因此：
//! - **被动端**扮演 riperf3 server，绑 `0.0.0.0:SPEED_TEST_PORT`；
//! - **主动端**（点击测速的设备）扮演 client，连 `peer_virtual_ip:port`；
//! - 上传与下载分别跑一次 TCP 测试，每个方向持续 6 秒。
//!
//! 控制面（协商 + 延迟探测）走 `tunnel_control` 上的健康 TCP 长连接。

use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};
use std::net::SocketAddr;
use tokio::sync::oneshot;

/// 单次测速时长（秒）。iperf3 官方推荐区间，统计样本足够稳定。
pub const SPEED_TEST_DURATION_SECS: u32 = 6;

/// riperf3 server / client 约定的测速端口。
/// 与 `HEALTH_PORT`(48082 TCP 健康面)、WebUI(48083) 错开。
pub const SPEED_TEST_PORT: u16 = 48084;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TunnelSpeedTestResult {
    /// 延迟（毫秒）。由健康长连接上的 `SpeedPing`/`SpeedPong` 往返测得，
    /// riperf3 本身不提供独立的 ping/RTT 测量。
    pub latency_ms: f64,
    /// 对端 → 本端的吞吐（Mbps）= client 在反向（server→client）recv 实测。
    pub download_mbps: f64,
    /// 本端 → 对端的吞吐（Mbps）= client 在正向（client→server）send 实测。
    /// TCP 拥塞控制下，应用层 send 速率 ≈ 网络实际传输速率。
    pub upload_mbps: f64,
    /// TCP 重传次数（forward + reverse 合计）。反映链路丢包情况，比 UDP
    /// 丢包率更直观（内核自动重传，用户无感）。`None` 表示未统计。
    pub retransmits: Option<i64>,
}

pub struct TunnelSpeedTestCommand {
    pub response: oneshot::Sender<Result<TunnelSpeedTestResult, String>>,
    /// 测速并发去重标志：runtime 入口抢占式 swap(true)，测速结束后 store(false)。
    pub busy_flag: std::sync::Arc<std::sync::atomic::AtomicBool>,
}

/// 被动端：构建 riperf3 server。
///
/// 绑 `0.0.0.0:SPEED_TEST_PORT`——被动端在 wgvpn userspace 后端下没有
/// TUN 接口，无法绑隧道 IP；绑 0.0.0.0 让 wgvpn 用户态转发能送达流量
/// （与 health server 0.0.0.0:48082 同理）。
///
/// 调用方应在 `tokio::spawn` 里启动 `run_once`，listen 就绪后回 `SpeedReady`，
/// 然后等待一次测试完成。
pub fn build_speed_test_server() -> Result<riperf3::Server> {
    riperf3::ServerBuilder::new()
        .port(Some(SPEED_TEST_PORT))
        .one_off(true)
        .json_output(true)
        .bind_address("0.0.0.0")
        .build()
        .context("failed to build riperf3 server")
}

/// 主动端：构建 riperf3 client，连被动端 server。
///
/// - `peer_addr`：被动端在隧道内的 IP + 测速端口（由主动端的 peer_virtual_ip
///   构造，主动端有 TUN 接口，路由可达）；
/// - **TCP 协议**：p2premote 主场景是远程桌面（可靠流），TCP 吞吐对用户
///   最有参考价值。且 TCP 的内核拥塞控制会自动探测可用带宽，避免 UDP
///   无限灌包导致的自我拥塞（实测 UDP 在 wgvpn userspace 后端下吞吐反被
///   压垮到 ~12 Mbps，丢包率虚高至 99.7%）；
/// - `reverse = false` 测上传（client → server）；`reverse = true` 测下载
///   （server → client）；两次测试互不抢占链路；
/// - `get_server_output(true)` 让上传结果可优先使用 server 的实际接收速率。
pub fn build_speed_test_client(peer_addr: SocketAddr, reverse: bool) -> Result<riperf3::Client> {
    let host = peer_addr.ip().to_string();
    riperf3::ClientBuilder::new(&host)
        .port(Some(peer_addr.port()))
        .protocol(riperf3::TransportProtocol::Tcp)
        .reverse(reverse)
        .duration(SPEED_TEST_DURATION_SECS)
        .json_output(true)
        .get_server_output(true)
        .build()
        .context("riperf3 client build failed")
}

/// 从独立上传测试的 client 端 `Report` 提取吞吐 + TCP 重传。
///
/// 字段选择经过真机验证校准：不能用 client.sum_sent 作为 upload——它统计
/// client 调用 send() 灌入 socket 的速率（含被内核 send buffer 吸收但
/// 尚未实际送达对端的部分）。在 wgvpn userspace 后端下，client send 很快
/// 但 server 实际 recv 远少（实测 client.sum_sent 200 Mbps vs server 真实
/// recv ~20 Mbps）。
///
/// 正确的字段来源（都是**接收方物理 read 实测**）：
/// - **upload**（client→server，forward）：server 是接收方，读
///   `server_output_json.end.sum_received.bits_per_second`（通过
///   `--get-server-output` 回传）。server 没回传时退化为 client.sum_sent
///   作为兜底（已知可能虚高）。
/// TCP 无 jitter/丢包率概念（流式协议，内核自动重传）。retransmits 反映
/// 链路质量，但 Windows 不支持 TCP_INFO，值恒为 -1，仅 Linux 有效。
pub fn extract_upload_result(report: &riperf3::Report) -> (f64, Option<i64>) {
    let end = &report.end;
    // upload：优先用 server 端实测（server 是 forward 接收方），从
    // server_output_json 解析；缺失时退化为 client.sum_sent（可能虚高）。
    let server_stats = parse_server_stats(report.server_output_json.as_ref());
    let upload_mbps = server_stats
        .received_bps
        .or_else(|| end.sum_sent.as_ref().map(|s| s.bits_per_second));
    (bps_to_mbps(upload_mbps), server_stats.retransmits)
}

/// 从独立下载测试的 client 端 `Report` 提取吞吐。
/// reverse 模式下 server 发送、client 接收，`sum_received` 是本端实测值。
pub fn extract_download_result(report: &riperf3::Report) -> f64 {
    bps_to_mbps(report.end.sum_received.as_ref().map(|s| s.bits_per_second))
}

/// server 端 forward 接收方的统计（通过 `--get-server-output` 回传）。
#[derive(Default)]
struct ServerStats {
    /// server 在 forward（client→server）方向的 recv 速率，对应真实 upload 带宽。
    received_bps: Option<f64>,
    /// server 在 forward 方向接收的重传数（链路质量指标，仅 Linux 有效）。
    retransmits: Option<i64>,
}

/// 从 server Report 的 JSON 解析 forward 接收方统计。
/// `server_value` 为 None 时返回默认值（server 未回传报告）。
/// 用 `serde_json::Value` 零散访问，因为 Report 只实现 Serialize。
fn parse_server_stats(server_value: Option<&serde_json::Value>) -> ServerStats {
    let Some(server_value) = server_value else {
        return ServerStats::default();
    };
    let end = server_value.get("end");
    let sum_received = end.and_then(|e| e.get("sum_received"));
    // retransmits 在 sender 视角字段（sum_sent_bidir_reverse = server reverse 发送）。
    let retransmits = end
        .and_then(|e| e.get("sum_sent_bidir_reverse"))
        .and_then(|s| s.get("retransmits"))
        .and_then(|v| v.as_i64())
        // Windows 不支持 TCP_INFO，riperf3 返回 -1 表示"不可用"，转成 None。
        .filter(|&r| r >= 0);
    ServerStats {
        received_bps: sum_received
            .and_then(|s| s.get("bits_per_second"))
            .and_then(|v| v.as_f64()),
        retransmits,
    }
}

/// 内部：把 bits_per_second 转 Mbps。`None` 视为 0。
fn bps_to_mbps(bps: Option<f64>) -> f64 {
    bps.unwrap_or(0.0) / 1_000_000.0
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn bps_to_mbps_converts_correctly() {
        assert!((bps_to_mbps(Some(1_000_000.0)) - 1.0).abs() < f64::EPSILON);
        assert!((bps_to_mbps(Some(50_000_000.0)) - 50.0).abs() < f64::EPSILON);
        assert!(bps_to_mbps(None).abs() < f64::EPSILON);
    }

    #[test]
    fn each_speed_test_direction_lasts_six_seconds() {
        assert_eq!(SPEED_TEST_DURATION_SECS, 6);
    }

    #[test]
    fn speed_test_port_does_not_collide_with_health_or_webui() {
        assert_ne!(SPEED_TEST_PORT, 48082, "must not collide with HEALTH_PORT");
        assert_ne!(SPEED_TEST_PORT, 48083, "must not collide with WebUI port");
    }

    #[test]
    fn parse_server_stats_reads_forward_receiver_metrics() {
        // 模拟 server 端 bidir 报告：forward 接收在 sum_received，
        // reverse 发送的重传在 sum_sent_bidir_reverse.retransmits。
        let server_json = serde_json::json!({
            "end": {
                "sum_received": {
                    "bits_per_second": 50000000.0
                },
                "sum_sent_bidir_reverse": {
                    "bits_per_second": 30000000.0,
                    "retransmits": 12
                }
            }
        });
        let stats = parse_server_stats(Some(&server_json));
        // received_bps = 50 Mbps（server 实际接收，对应真实 upload 带宽）
        assert!((stats.received_bps.unwrap() - 50_000_000.0).abs() < f64::EPSILON);
        assert_eq!(stats.retransmits, Some(12));
    }

    #[test]
    fn parse_server_stats_treats_unsupported_retransmits_as_none() {
        // Windows 不支持 TCP_INFO，riperf3 返回 -1，应转为 None。
        let server_json = serde_json::json!({
            "end": {
                "sum_received": {"bits_per_second": 50000000.0},
                "sum_sent_bidir_reverse": {"retransmits": -1}
            }
        });
        let stats = parse_server_stats(Some(&server_json));
        assert!(stats.retransmits.is_none());
    }

    #[test]
    fn parse_server_stats_handles_missing_report() {
        // server 未回传报告（server_output_json 为 None）。
        let stats = parse_server_stats(None);
        assert!(stats.received_bps.is_none());
        assert!(stats.retransmits.is_none());
    }
}
