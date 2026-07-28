use futures_util::{SinkExt, StreamExt};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::sync::Arc;
use tokio::sync::{mpsc, oneshot, Mutex as TokioMutex};
use tokio::time::{interval, Duration, Instant};
use tokio_tungstenite::{
    connect_async,
    tungstenite::{client::IntoClientRequest, Message},
};
use tracing::{debug, error, info, warn};

#[derive(Debug, Clone)]
pub enum WsSendType {
    P2PReady {
        punch_token: String,
        public_ip: String,
        public_port: i64,
        target_device_id: i64,
    },
    P2PNotify {
        connection_id: String,
        target_device_id: i64,
        message_id: String,
        access_grant: String,
        data: String,
    },
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type", content = "payload")]
pub enum WsMessage {
    #[serde(rename = "p2p_start")]
    P2PStart {
        #[serde(default)]
        source_user_id: i64,
        #[serde(default)]
        source_username: String,
        #[serde(default)]
        source_email: String,
        source_device_id: i64,
        #[serde(default)]
        source_device_name: String,
        #[serde(default)]
        source_device_alias: String,
        #[serde(default)]
        source_public_ip: String,
        punch_token: String,
        timestamp: Option<String>,
    },
    #[serde(rename = "p2p_ready")]
    P2PReady {
        punch_token: String,
        public_ip: String,
        public_port: i64,
        #[serde(default)]
        target_user_id: i64,
        #[serde(default)]
        target_device_id: i64,
    },
    #[serde(rename = "p2p_notify")]
    P2PNotify {
        connection_id: String,
        #[serde(default)]
        source_device_id: i64,
        #[serde(default)]
        target_device_id: i64,
        message_id: String,
        access_grant: String,
        data: String,
    },
    #[serde(rename = "p2p_notify_ack")]
    P2PNotifyAck {
        message_id: String,
        accepted: bool,
        #[serde(default)]
        error: Option<serde_json::Value>,
    },
    #[serde(rename = "temp_password_consumed")]
    TempPasswordConsumed { device_id: i64 },
}

#[derive(Debug, Clone)]
pub enum WsEvent {
    Connected,
    Disconnected,
    P2PStart {
        source_user_id: i64,
        source_username: String,
        source_email: String,
        source_device_id: i64,
        source_device_name: String,
        source_device_alias: String,
        source_public_ip: String,
        punch_token: String,
    },
    P2PNotify {
        connection_id: String,
        source_device_id: i64,
        message_id: String,
        access_grant: String,
        data: String,
    },
    TempPasswordConsumed {
        device_id: i64,
    },
}

const WS_PING_INTERVAL_SECS: u64 = 20;
const WS_PONG_TIMEOUT_SECS: u64 = 10;
const WS_MAX_CONSECUTIVE_FAILURES: u8 = 3;

#[derive(Debug, Default)]
struct HeartbeatTracker {
    next_sequence: u64,
    waiting_sequence: Option<u64>,
    pong_deadline: Option<Instant>,
    consecutive_failures: u8,
    confirmed: bool,
}

impl HeartbeatTracker {
    fn begin_ping(&mut self, now: Instant) -> Option<u64> {
        if self.waiting_sequence.is_some() {
            return None;
        }
        self.next_sequence = self.next_sequence.wrapping_add(1);
        self.waiting_sequence = Some(self.next_sequence);
        self.pong_deadline = Some(now + Duration::from_secs(WS_PONG_TIMEOUT_SECS));
        Some(self.next_sequence)
    }

    fn accept_pong(&mut self, payload: &[u8]) -> Option<bool> {
        let Ok(bytes) = <[u8; 8]>::try_from(payload) else {
            return None;
        };
        let sequence = u64::from_be_bytes(bytes);
        if self.waiting_sequence != Some(sequence) {
            return None;
        }
        self.waiting_sequence = None;
        self.pong_deadline = None;
        self.consecutive_failures = 0;
        let first_confirmation = !self.confirmed;
        self.confirmed = true;
        Some(first_confirmation)
    }

    fn check_timeout(&mut self, now: Instant) -> Option<u8> {
        if self.pong_deadline.is_none_or(|deadline| now < deadline) {
            return None;
        }
        self.waiting_sequence = None;
        self.pong_deadline = None;
        self.consecutive_failures = self.consecutive_failures.saturating_add(1);
        Some(self.consecutive_failures)
    }
}

#[derive(Clone)]
pub struct ServiceWsClient {
    connected: Arc<TokioMutex<bool>>,
    task_handle: Arc<TokioMutex<Option<tokio::task::JoinHandle<()>>>>,
    send_channel: Arc<TokioMutex<Option<mpsc::UnboundedSender<WsSendType>>>>,
    p2p_notify_acks: Arc<TokioMutex<HashMap<String, oneshot::Sender<Result<(), String>>>>>,
}

impl ServiceWsClient {
    pub fn new() -> Self {
        Self {
            connected: Arc::new(TokioMutex::new(false)),
            task_handle: Arc::new(TokioMutex::new(None)),
            send_channel: Arc::new(TokioMutex::new(None)),
            p2p_notify_acks: Arc::new(TokioMutex::new(HashMap::new())),
        }
    }

    pub async fn connect(
        &self,
        server_url: &str,
        token: &str,
        device_id: i64,
        device_uuid: &str,
        event_tx: mpsc::UnboundedSender<WsEvent>,
    ) -> Result<(), String> {
        let ws_url = format!(
            "{}/ws?device_id={}&device_uuid={}",
            server_url
                .replace("https://", "wss://")
                .replace("http://", "ws://"),
            device_id,
            device_uuid
        );

        let mut request = ws_url
            .into_client_request()
            .map_err(|e| format!("WebSocket request build failed: {}", e))?;
        let auth_value = format!("Bearer {}", token)
            .parse()
            .map_err(|e| format!("invalid Authorization header: {}", e))?;
        request.headers_mut().insert("Authorization", auth_value);

        let (ws_stream, _) = connect_async(request)
            .await
            .map_err(|e| format!("WebSocket connection failed: {}", e))?;

        *self.connected.lock().await = true;
        let (send_tx, mut send_rx) = mpsc::unbounded_channel::<WsSendType>();
        *self.send_channel.lock().await = Some(send_tx);

        let connected = self.connected.clone();
        let p2p_notify_acks = self.p2p_notify_acks.clone();
        let task = tokio::spawn(async move {
            let mut ping_ticker = interval(Duration::from_secs(WS_PING_INTERVAL_SECS));
            let mut timeout_ticker = interval(Duration::from_secs(1));
            let mut heartbeat = HeartbeatTracker::default();
            let (mut write, mut read) = ws_stream.split();

            loop {
                tokio::select! {
                    biased;
                    _ = ping_ticker.tick() => {
                        if let Some(sequence) = heartbeat.begin_ping(Instant::now()) {
                            if let Err(err) = write.send(Message::Ping(sequence.to_be_bytes().to_vec().into())).await {
                                error!("[ServiceWS] send protocol ping failed: {}", err);
                                *connected.lock().await = false;
                                break;
                            }
                            debug!("[ServiceWS] protocol ping sent: sequence={}", sequence);
                        }
                    }
                    Some(send_msg) = send_rx.recv() => {
                        match send_msg {
                            WsSendType::P2PReady { punch_token, public_ip, public_port, target_device_id } => {
                                let ws_msg = WsMessage::P2PReady {
                                    punch_token,
                                    public_ip,
                                    public_port,
                                    target_user_id: 0,
                                    target_device_id,
                                };
                                if let Ok(json) = serde_json::to_string(&ws_msg) {
                                    if let Err(err) = write.send(Message::Text(json.into())).await {
                                        error!("[ServiceWS] send p2p_ready failed: {}", err);
                                    }
                                }
                            }
                            WsSendType::P2PNotify { connection_id, target_device_id, message_id, access_grant, data } => {
                                let ws_msg = WsMessage::P2PNotify {
                                    connection_id,
                                    source_device_id: 0,
                                    target_device_id,
                                    message_id,
                                    access_grant,
                                    data,
                                };
                                if let Ok(json) = serde_json::to_string(&ws_msg) {
                                    if let Err(err) = write.send(Message::Text(json.into())).await {
                                        error!("[ServiceWS] send p2p_notify failed: {}", err);
                                    }
                                }
                            }
                        }
                    }
                    msg = read.next() => {
                        match msg {
                            Some(Ok(Message::Text(text))) => {
                                let text = text.to_string();
                                let frame_len = text.len();
                                let newline_count = text.bytes().filter(|byte| *byte == b'\n').count();
                                let message = match serde_json::from_str::<WsMessage>(&text) {
                                    Ok(message) => message,
                                    Err(err) => {
                                        warn!(
                                            "[ServiceWS] invalid text frame: bytes={}, newlines={}, error={}",
                                            frame_len,
                                            newline_count,
                                            err
                                        );
                                        continue;
                                    }
                                };
                                match message {
                                    WsMessage::P2PStart {
                                        source_user_id,
                                        source_username,
                                        source_email,
                                        source_device_id,
                                        source_device_name,
                                        source_device_alias,
                                        source_public_ip,
                                        punch_token,
                                        ..
                                    } => {
                                        info!(
                                            "[ServiceWS] p2p_start frame received: source_device_id={}, bytes={}, newlines={}",
                                            source_device_id,
                                            frame_len,
                                            newline_count
                                        );
                                        let _ = event_tx.send(WsEvent::P2PStart {
                                            source_user_id,
                                            source_username,
                                            source_email,
                                            source_device_id,
                                            source_device_name,
                                            source_device_alias,
                                            source_public_ip,
                                            punch_token,
                                        });
                                    }
                                    WsMessage::TempPasswordConsumed { device_id } => {
                                        let _ = event_tx.send(WsEvent::TempPasswordConsumed { device_id });
                                    }
                                    WsMessage::P2PNotify { connection_id, source_device_id, message_id, access_grant, data, .. } => {
                                        let _ = event_tx.send(WsEvent::P2PNotify {
                                            connection_id,
                                            source_device_id,
                                            message_id,
                                            access_grant,
                                            data,
                                        });
                                    }
                                    WsMessage::P2PNotifyAck { message_id, accepted, error } => {
                                        if let Some(waiter) = p2p_notify_acks.lock().await.remove(&message_id) {
                                            let result = if accepted {
                                                Ok(())
                                            } else {
                                                let code = error
                                                    .as_ref()
                                                    .and_then(|value| value.get("code"))
                                                    .and_then(|value| value.as_str())
                                                    .unwrap_or("notify_rejected");
                                                Err(code.to_string())
                                            };
                                            let _ = waiter.send(result);
                                        }
                                        if !accepted {
                                            warn!("[ServiceWS] p2p_notify rejected: message_id={}, error={:?}", message_id, error);
                                        }
                                    }
                                    _ => {}
                                }
                            }
                            Some(Ok(Message::Pong(payload))) => {
                                match heartbeat.accept_pong(&payload) {
                                    Some(true) => {
                                        info!("[ServiceWS] first protocol pong received, connection confirmed");
                                        let _ = event_tx.send(WsEvent::Connected);
                                    }
                                    Some(false) => debug!("[ServiceWS] protocol pong acknowledged"),
                                    None => debug!("[ServiceWS] ignored unmatched protocol pong"),
                                }
                            }
                            Some(Ok(Message::Close(_))) | None => {
                                info!("[ServiceWS] connection closed by peer or stream ended");
                                *connected.lock().await = false;
                                break;
                            }
                            Some(Err(err)) => {
                                error!("[ServiceWS] read message failed: {}", err);
                                *connected.lock().await = false;
                                break;
                            }
                            Some(Ok(other)) => {
                                debug!("[ServiceWS] non-text frame received: {:?}", other);
                            }
                        }
                    }
                    _ = timeout_ticker.tick() => {
                        if let Some(missed) = heartbeat.check_timeout(Instant::now()) {
                            warn!(
                                "[ServiceWS] protocol pong missed: {}/{}",
                                missed,
                                WS_MAX_CONSECUTIVE_FAILURES
                            );
                            if missed >= WS_MAX_CONSECUTIVE_FAILURES {
                                warn!("[ServiceWS] heartbeat failed 3 consecutive times, reconnecting");
                                *connected.lock().await = false;
                                break;
                            }
                        }
                    }
                }
            }

            let _ = event_tx.send(WsEvent::Disconnected);
            info!("[ServiceWS] message loop exited");
        });

        *self.task_handle.lock().await = Some(task);
        Ok(())
    }

    pub async fn disconnect(&self) {
        *self.connected.lock().await = false;
        *self.send_channel.lock().await = None;
        if let Some(handle) = self.task_handle.lock().await.take() {
            handle.abort();
        }
    }

    pub async fn is_connected(&self) -> bool {
        *self.connected.lock().await
    }

    pub async fn send_p2p_ready(
        &self,
        punch_token: String,
        public_ip: String,
        public_port: i64,
        target_device_id: i64,
    ) -> Result<(), String> {
        let channel = self.send_channel.lock().await;
        if let Some(ref tx) = *channel {
            tx.send(WsSendType::P2PReady {
                punch_token,
                public_ip,
                public_port,
                target_device_id,
            })
            .map_err(|e| format!("failed to queue p2p_ready: {}", e))?;
            Ok(())
        } else {
            Err("websocket not connected".to_string())
        }
    }

    pub async fn send_p2p_notify(
        &self,
        connection_id: String,
        target_device_id: i64,
        message_id: String,
        access_grant: String,
        data: String,
    ) -> Result<(), String> {
        let (ack_tx, ack_rx) = oneshot::channel();
        self.p2p_notify_acks
            .lock()
            .await
            .insert(message_id.clone(), ack_tx);
        let channel = self.send_channel.lock().await;
        let send_result = match channel.as_ref() {
            Some(tx) => tx
                .send(WsSendType::P2PNotify {
                    connection_id,
                    target_device_id,
                    message_id: message_id.clone(),
                    access_grant,
                    data,
                })
                .map_err(|err| format!("failed to queue p2p_notify: {}", err)),
            None => Err("websocket not connected".to_string()),
        };
        drop(channel);
        if let Err(err) = send_result {
            self.p2p_notify_acks.lock().await.remove(&message_id);
            return Err(err);
        }
        match tokio::time::timeout(Duration::from_secs(5), ack_rx).await {
            Ok(Ok(result)) => result,
            Ok(Err(_)) => Err("p2p_notify_ack_cancelled".to_string()),
            Err(_) => {
                self.p2p_notify_acks.lock().await.remove(&message_id);
                Err("p2p_notify_ack_timeout".to_string())
            }
        }
    }
}

impl Default for ServiceWsClient {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn heartbeat_requires_three_consecutive_timeouts() {
        let mut tracker = HeartbeatTracker::default();
        let mut now = Instant::now();

        for expected in 1..=WS_MAX_CONSECUTIVE_FAILURES {
            assert!(tracker.begin_ping(now).is_some());
            now += Duration::from_secs(WS_PONG_TIMEOUT_SECS + 1);
            assert_eq!(tracker.check_timeout(now), Some(expected));
        }
        assert_eq!(tracker.consecutive_failures, WS_MAX_CONSECUTIVE_FAILURES);
    }

    #[test]
    fn matching_pong_confirms_and_resets_failures() {
        let mut tracker = HeartbeatTracker::default();
        let now = Instant::now();
        let first = tracker.begin_ping(now).unwrap();
        assert_eq!(tracker.accept_pong(&first.to_be_bytes()), Some(true));

        tracker.consecutive_failures = 2;
        let second = tracker.begin_ping(now).unwrap();
        assert_eq!(tracker.accept_pong(&second.to_be_bytes()), Some(false));
        assert_eq!(tracker.consecutive_failures, 0);
    }

    #[test]
    fn unmatched_pong_does_not_reset_failure_state() {
        let mut tracker = HeartbeatTracker::default();
        let sequence = tracker.begin_ping(Instant::now()).unwrap();
        tracker.consecutive_failures = 2;

        assert_eq!(tracker.accept_pong(&(sequence + 1).to_be_bytes()), None);
        assert_eq!(tracker.waiting_sequence, Some(sequence));
        assert_eq!(tracker.consecutive_failures, 2);
    }
}
