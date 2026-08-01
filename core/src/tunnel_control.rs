//! Tunnel health/control protocol.
//!
//! Wire format: BytesCodec length-prefixed frame + JSON TunnelControlMessage.

use crate::bytes_codec::BytesCodec;
use anyhow::{anyhow, Context, Result};
use bytes::Bytes;
use futures_util::{SinkExt, StreamExt};
use serde::{Deserialize, Serialize};
use tokio_util::codec::Framed;

pub const TUNNEL_CONTROL_PROTOCOL_VERSION: u16 = 2;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
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
