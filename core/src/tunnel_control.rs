//! Tunnel health/control protocol.
//!
//! Wire format: BytesCodec length-prefixed frame + JSON TunnelControlMessage.

use crate::bytes_codec::BytesCodec;
use anyhow::{anyhow, Context, Result};
use bytes::Bytes;
use futures_util::{SinkExt, StreamExt};
use serde::{Deserialize, Serialize};
use tokio_util::codec::Framed;

pub const TUNNEL_CONTROL_PROTOCOL_VERSION: u16 = 3;

/// 最低可互连的协议版本。
pub const TUNNEL_CONTROL_PROTOCOL_MIN_COMPAT: u16 = 3;

/// 对端 Hello/HelloAck 的版本是否可接受：不强制与本端相等，只要求不低于
/// 下限。不设上限——版本号升级但消息实际兼容时不应被拒；将来出现真正的
/// 破坏性变更时提高 MIN_COMPAT 即可。旧端对未知消息 tag 的解析失败只会
/// 断开该条控制连接，不影响既有消息互通。
pub fn protocol_version_supported(version: u16) -> bool {
    version >= TUNNEL_CONTROL_PROTOCOL_MIN_COMPAT
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "t", content = "c")]
pub enum TunnelControlMessage {
    Hello {
        source_device_id: i64,
        protocol_version: u16,
    },
    HelloAck {
        ok: bool,
        protocol_version: u16,
        message: String,
    },
    Ping {
        ts: i64,
        /// 主动端上一次测得并用于展示的隧道 RTT。首个样本前为空；
        /// 被动端只消费该值，不使用两端系统时间推算延迟。
        #[serde(default, skip_serializing_if = "Option::is_none")]
        reported_rtt_ms: Option<u32>,
    },
    Pong {
        ts: i64,
    },
    SpeedPing {
        nonce: u64,
    },
    SpeedPong {
        nonce: u64,
    },
    /// 主动端→被动端：通知被动端启动 riperf3 server。
    /// 端口固定为 `speed_test::SPEED_TEST_PORT`，被动端 server 绑 `0.0.0.0`，
    /// 主动端通过 peer_virtual_ip 连接（依赖 wgvpn 主动→被动方向可达）。
    /// 测试时长由两端各自的 `SPEED_TEST_DURATION_SECS` 常量决定，无需协议协商。
    SpeedStart,
    /// 被动端→主动端：riperf3 server 已 listen，可以连接。
    /// riperf3 client 连接失败不重试，必须等此回执后才发起 client。
    SpeedReady,
    /// 被动端→主动端：要求主动端在现有健康连接上执行测速。
    SpeedTestRequest {
        request_id: u64,
    },
    /// 主动端→被动端：返回主动端视角的测速结果；被动端展示时交换上下行。
    SpeedTestResult {
        request_id: u64,
        latency_ms: f64,
        download_mbps: f64,
        upload_mbps: f64,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        retransmits: Option<i64>,
    },
    /// 主动端→被动端：测速失败，保留第一次错误且不重试。
    SpeedTestError {
        request_id: u64,
        message: String,
    },
    Stop {
        reason: String,
    },
}

pub struct TunnelControlConnection<T> {
    inner: Framed<T, BytesCodec>,
}

impl<T> TunnelControlConnection<T>
where
    T: tokio::io::AsyncRead + tokio::io::AsyncWrite + Unpin,
{
    pub fn new(stream: T) -> Self {
        Self {
            inner: Framed::new(stream, BytesCodec::new()),
        }
    }

    pub async fn send(&mut self, message: &TunnelControlMessage) -> Result<()> {
        let json = serde_json::to_vec(message).context("failed to serialize tunnel message")?;
        self.inner
            .send(Bytes::from(json))
            .await
            .map_err(|err| anyhow!("tunnel message send failed: {}", err))
    }

    pub async fn next(&mut self) -> Result<Option<TunnelControlMessage>> {
        match self.inner.next().await {
            Some(Ok(bytes)) => {
                let message = serde_json::from_slice(&bytes)
                    .context("failed to decode tunnel message json")?;
                Ok(Some(message))
            }
            Some(Err(err)) => Err(anyhow!("tunnel message read failed: {}", err)),
            None => Ok(None),
        }
    }

    /// Flush pending frames and gracefully close the underlying TCP write side.
    /// Callers that use connection closure as a control signal must await this
    /// before tearing down the tunnel that carries the TCP stream.
    pub async fn close(&mut self) -> Result<()> {
        self.inner
            .close()
            .await
            .map_err(|err| anyhow!("tunnel connection close failed: {}", err))
    }
}

pub fn now_millis() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|duration| duration.as_millis() as i64)
        .unwrap_or_default()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tunnel_control_message_json_roundtrip() {
        let message = TunnelControlMessage::Hello {
            source_device_id: 42,
            protocol_version: TUNNEL_CONTROL_PROTOCOL_VERSION,
        };
        let json = serde_json::to_vec(&message).expect("serialize message");
        let decoded: TunnelControlMessage = serde_json::from_slice(&json).expect("decode message");
        assert_eq!(decoded, message);
    }

    #[test]
    fn speed_control_message_json_roundtrip() {
        let message = TunnelControlMessage::SpeedStart;
        let json = serde_json::to_vec(&message).expect("serialize speed message");
        let decoded: TunnelControlMessage =
            serde_json::from_slice(&json).expect("decode speed message");
        assert_eq!(decoded, message);
        // SpeedReady 是无字段变体，单独验证
        let ready = TunnelControlMessage::SpeedReady;
        let ready_json = serde_json::to_vec(&ready).expect("serialize ready");
        let ready_decoded: TunnelControlMessage =
            serde_json::from_slice(&ready_json).expect("decode ready");
        assert_eq!(ready_decoded, ready);

        let result = TunnelControlMessage::SpeedTestResult {
            request_id: 7,
            latency_ms: 12.5,
            download_mbps: 80.0,
            upload_mbps: 40.0,
            retransmits: Some(2),
        };
        let result_json = serde_json::to_vec(&result).expect("serialize speed result");
        let result_decoded: TunnelControlMessage =
            serde_json::from_slice(&result_json).expect("decode speed result");
        assert_eq!(result_decoded, result);
    }

    #[tokio::test]
    async fn tunnel_control_connection_roundtrip() {
        let (client, server) = tokio::io::duplex(4096);
        let mut client_conn = TunnelControlConnection::new(client);
        let mut server_conn = TunnelControlConnection::new(server);

        let message = TunnelControlMessage::Ping {
            ts: 123,
            reported_rtt_ms: Some(42),
        };
        client_conn.send(&message).await.expect("send message");
        let decoded = server_conn
            .next()
            .await
            .expect("read message")
            .expect("message exists");
        assert_eq!(decoded, message);
    }

    #[test]
    fn legacy_ping_without_reported_rtt_is_accepted() {
        let decoded: TunnelControlMessage =
            serde_json::from_str(r#"{"t":"Ping","c":{"ts":123}}"#).expect("decode legacy ping");
        assert_eq!(
            decoded,
            TunnelControlMessage::Ping {
                ts: 123,
                reported_rtt_ms: None,
            }
        );
    }
}
