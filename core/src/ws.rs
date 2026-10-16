use crate::error::{AppError, ErrorCode};
use futures_util::{SinkExt, StreamExt};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::sync::{
    atomic::{AtomicU64, Ordering},
    Arc,
};
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
    WOLResult {
        request_id: String,
        success: bool,
        code: String,
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
    #[serde(rename = "wol_request")]
    WOLRequest {
        request_id: String,
        macs: Vec<String>,
        target_ipv4: String,
        prefix_len: u8,
    },
    #[serde(rename = "wol_result")]
    WOLResult {
        request_id: String,
        success: bool,
        code: String,
    },
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
    WOLRequest {
        request_id: String,
        macs: Vec<String>,
        target_ipv4: String,
        prefix_len: u8,
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
        if self.pong_deadline.map_or(true, |deadline| now < deadline) {
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
    connection_generation: Arc<AtomicU64>,
    task_handle: Arc<TokioMutex<Option<tokio::task::JoinHandle<()>>>>,
    send_channel: Arc<TokioMutex<Option<(u64, mpsc::UnboundedSender<WsSendType>)>>>,
    p2p_notify_acks: Arc<TokioMutex<HashMap<String, PendingNotifyAck>>>,
}

struct PendingNotifyAck {
    generation: u64,
    sender: oneshot::Sender<Result<(), AppError>>,
}

impl ServiceWsClient {
    pub fn new() -> Self {
        Self {
            connected: Arc::new(TokioMutex::new(false)),
            connection_generation: Arc::new(AtomicU64::new(0)),
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

        let generation = self
            .connection_generation
            .fetch_add(1, Ordering::AcqRel)
            .wrapping_add(1);
        *self.connected.lock().await = true;
        let (send_tx, mut send_rx) = mpsc::unbounded_channel::<WsSendType>();
        *self.send_channel.lock().await = Some((generation, send_tx));

        let connected = self.connected.clone();
        let connection_generation = self.connection_generation.clone();
        let send_channel = self.send_channel.clone();
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
                                        break;
                                    }
                                }
                            }
                            WsSendType::P2PNotify { connection_id, target_device_id, message_id, access_grant, data } => {
                                let ws_msg = WsMessage::P2PNotify {
                                    connection_id,
                                    source_device_id: 0,
                                    target_device_id,
                                    message_id: message_id.clone(),
                                    access_grant,
                                    data,
                                };
                                if let Ok(json) = serde_json::to_string(&ws_msg) {
                                    if let Err(err) = write.send(Message::Text(json.into())).await {
                                        error!("[ServiceWS] send p2p_notify failed: {}", err);
                                        fail_pending_notify_ack(
                                            &p2p_notify_acks,
                                            &message_id,
                                            generation,
                                            format!("p2p_notify_write_failed: {err}"),
                                        )
                                        .await;
                                        break;
                                    }
                                }
                            }
                            WsSendType::WOLResult {
                                request_id,
                                success,
                                code,
                            } => {
                                let ws_msg = WsMessage::WOLResult {
                                    request_id,
                                    success,
                                    code,
                                };
                                if let Ok(json) = serde_json::to_string(&ws_msg) {
                                    if let Err(err) = write.send(Message::Text(json.into())).await {
                                        error!("[ServiceWS] send wol_result failed: {}", err);
                                        break;
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
                                        let waiter = {
                                            let mut waiters = p2p_notify_acks.lock().await;
                                            if waiters.get(&message_id).is_some_and(|pending| pending.generation == generation) {
                                                waiters.remove(&message_id)
                                            } else {
                                                None
                                            }
                                        };
                                        if let Some(waiter) = waiter {
                                            let result = if accepted {
                                                Ok(())
                                            } else {
                                                let code = error
                                                    .as_ref()
                                                    .and_then(|value| value.get("code"))
                                                    .and_then(|value| value.as_str())
                                                    .unwrap_or("notify_rejected");
                                                Err(AppError::new(ErrorCode::SignalSendRejected, code))
                                            };
                                            let _ = waiter.sender.send(result);
                                        }
                                        if !accepted {
                                            warn!("[ServiceWS] p2p_notify rejected: message_id={}, error={:?}", message_id, error);
                                        }
                                    }
                                    WsMessage::WOLRequest { request_id, macs, target_ipv4, prefix_len } => { let _=event_tx.send(WsEvent::WOLRequest{request_id,macs,target_ipv4,prefix_len}); }
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
                                debug!("[ServiceWS] connection closed by peer or stream ended");
                                break;
                            }
                            Some(Err(err)) => {
                                error!("[ServiceWS] read message failed: {}", err);
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
                                break;
                            }
                        }
                    }
                }
            }

            let was_current = cleanup_connection_generation(
                &connected,
                &connection_generation,
                &send_channel,
                &p2p_notify_acks,
                generation,
                "websocket_disconnected",
            )
            .await;
            if was_current {
                let _ = event_tx.send(WsEvent::Disconnected);
            }
            debug!("[ServiceWS] message loop exited");
        });

        *self.task_handle.lock().await = Some(task);
        Ok(())
    }

    pub async fn disconnect(&self) {
        let generation = self.connection_generation.load(Ordering::Acquire);
        if let Some(handle) = self.task_handle.lock().await.take() {
            handle.abort();
            let _ = handle.await;
        }
        cleanup_connection_generation(
            &self.connected,
            &self.connection_generation,
            &self.send_channel,
            &self.p2p_notify_acks,
            generation,
            "websocket_disconnected",
        )
        .await;
    }

    pub async fn is_connected(&self) -> bool {
        *self.connected.lock().await
    }

    pub fn generation(&self) -> u64 {
        self.connection_generation.load(Ordering::Acquire)
    }

    pub async fn send_p2p_ready(
        &self,
        punch_token: String,
        public_ip: String,
        public_port: i64,
        target_device_id: i64,
    ) -> Result<(), String> {
        let channel = self.send_channel.lock().await;
        if let Some((_, tx)) = channel.as_ref() {
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

    pub async fn send_wol_result(
        &self,
        request_id: String,
        success: bool,
        code: String,
    ) -> Result<(), String> {
        self.send_channel
            .lock()
            .await
            .as_ref()
            .ok_or_else(|| "websocket not connected".to_string())?
            .1
            .send(WsSendType::WOLResult {
                request_id,
                success,
                code,
            })
            .map_err(|e| e.to_string())
    }

    pub async fn send_p2p_notify(
        &self,
        connection_id: String,
        target_device_id: i64,
        message_id: String,
        access_grant: String,
        data: String,
    ) -> Result<(), AppError> {
        self.send_p2p_notify_for_generation(
            connection_id,
            target_device_id,
            message_id,
            access_grant,
            data,
            self.generation(),
            std::time::Instant::now() + Duration::from_secs(5),
        )
        .await
    }

    pub async fn send_p2p_notify_for_generation(
        &self,
        connection_id: String,
        target_device_id: i64,
        message_id: String,
        access_grant: String,
        data: String,
        expected_generation: u64,
        deadline: std::time::Instant,
    ) -> Result<(), AppError> {
        let budget = deadline
            .saturating_duration_since(std::time::Instant::now())
            .min(Duration::from_secs(5));
        if budget.is_zero() {
            return Err(AppError::new(
                ErrorCode::SignalSendTimeout,
                "signal deadline exceeded",
            ));
        }
        let (ack_tx, ack_rx) = oneshot::channel();
        let generation = self.connection_generation.load(Ordering::Acquire);
        if generation == 0 || generation != expected_generation || !*self.connected.lock().await {
            return Err(AppError::new(
                ErrorCode::SignalSendDisconnected,
                "websocket not connected",
            ));
        }
        let mut waiters = self.p2p_notify_acks.lock().await;
        if waiters.contains_key(&message_id) {
            return Err(AppError::new(
                ErrorCode::SignalReceiveContextMismatch,
                "duplicate notify message ID",
            ));
        }
        waiters.insert(
            message_id.clone(),
            PendingNotifyAck {
                generation,
                sender: ack_tx,
            },
        );
        drop(waiters);
        let _registration = NotifyAckRegistration {
            waiters: self.p2p_notify_acks.clone(),
            message_id: message_id.clone(),
            generation,
        };
        let channel = self.send_channel.lock().await;
        let send_result = match channel.as_ref() {
            Some((active_generation, tx)) if *active_generation == generation => tx
                .send(WsSendType::P2PNotify {
                    connection_id,
                    target_device_id,
                    message_id: message_id.clone(),
                    access_grant,
                    data,
                })
                .map_err(|err| {
                    AppError::with_source(
                        ErrorCode::SignalSendDisconnected,
                        "failed to queue signal",
                        err,
                    )
                }),
            _ => Err(AppError::new(
                ErrorCode::SignalSendDisconnected,
                "websocket not connected",
            )),
        };
        drop(channel);
        if let Err(err) = send_result {
            remove_pending_notify_ack(&self.p2p_notify_acks, &message_id, generation).await;
            return Err(err);
        }
        match tokio::time::timeout(budget, ack_rx).await {
            Ok(Ok(result)) => result,
            Ok(Err(_)) => Err(AppError::new(
                ErrorCode::SignalReceiveCancelled,
                "signal acknowledgement cancelled",
            )),
            Err(_) => {
                remove_pending_notify_ack(&self.p2p_notify_acks, &message_id, generation).await;
                Err(AppError::new(
                    ErrorCode::SignalSendTimeout,
                    "signal acknowledgement timed out",
                ))
            }
        }
    }
}

struct NotifyAckRegistration {
    waiters: Arc<TokioMutex<HashMap<String, PendingNotifyAck>>>,
    message_id: String,
    generation: u64,
}
impl Drop for NotifyAckRegistration {
    fn drop(&mut self) {
        if let Ok(mut waiters) = self.waiters.try_lock() {
            if waiters.get(&self.message_id).map(|w| w.generation) == Some(self.generation) {
                waiters.remove(&self.message_id);
            }
        } else if let Ok(runtime) = tokio::runtime::Handle::try_current() {
            let waiters = self.waiters.clone();
            let message_id = self.message_id.clone();
            let generation = self.generation;
            runtime.spawn(async move {
                remove_pending_notify_ack(&waiters, &message_id, generation).await;
            });
        }
    }
}

async fn remove_pending_notify_ack(
    waiters: &Arc<TokioMutex<HashMap<String, PendingNotifyAck>>>,
    message_id: &str,
    generation: u64,
) -> Option<PendingNotifyAck> {
    let mut waiters = waiters.lock().await;
    if waiters
        .get(message_id)
        .is_some_and(|pending| pending.generation == generation)
    {
        waiters.remove(message_id)
    } else {
        None
    }
}

async fn fail_pending_notify_ack(
    waiters: &Arc<TokioMutex<HashMap<String, PendingNotifyAck>>>,
    message_id: &str,
    generation: u64,
    reason: String,
) {
    if let Some(waiter) = remove_pending_notify_ack(waiters, message_id, generation).await {
        let _ = waiter.sender.send(Err(AppError::new(
            ErrorCode::SignalSendDisconnected,
            reason,
        )));
    }
}

async fn fail_pending_notify_acks_for_generation(
    waiters: &Arc<TokioMutex<HashMap<String, PendingNotifyAck>>>,
    generation: u64,
    reason: &str,
) {
    let pending = {
        let mut waiters = waiters.lock().await;
        let message_ids = waiters
            .iter()
            .filter_map(|(message_id, pending)| {
                (pending.generation == generation).then(|| message_id.clone())
            })
            .collect::<Vec<_>>();
        message_ids
            .into_iter()
            .filter_map(|message_id| waiters.remove(&message_id))
            .collect::<Vec<_>>()
    };
    for waiter in pending {
        let _ = waiter.sender.send(Err(AppError::new(
            ErrorCode::SignalSendDisconnected,
            reason,
        )));
    }
}

async fn cleanup_connection_generation(
    connected: &Arc<TokioMutex<bool>>,
    current_generation: &Arc<AtomicU64>,
    send_channel: &Arc<TokioMutex<Option<(u64, mpsc::UnboundedSender<WsSendType>)>>>,
    waiters: &Arc<TokioMutex<HashMap<String, PendingNotifyAck>>>,
    generation: u64,
    reason: &str,
) -> bool {
    fail_pending_notify_acks_for_generation(waiters, generation, reason).await;
    if current_generation.load(Ordering::Acquire) != generation {
        return false;
    }
    let mut connected = connected.lock().await;
    // The generation may change while cleanup is waiting for the state lock.
    // Recheck under the lock so an old connection cannot mark a newly connected
    // generation as disconnected.
    if current_generation.load(Ordering::Acquire) != generation {
        return false;
    }
    *connected = false;
    drop(connected);
    let mut channel = send_channel.lock().await;
    if channel
        .as_ref()
        .is_some_and(|(active_generation, _)| *active_generation == generation)
    {
        *channel = None;
    }
    true
}

impl Default for ServiceWsClient {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn pending_ack(generation: u64) -> (PendingNotifyAck, oneshot::Receiver<Result<(), AppError>>) {
        let (sender, receiver) = oneshot::channel();
        (PendingNotifyAck { generation, sender }, receiver)
    }

    #[tokio::test]
    async fn cancelling_signal_send_removes_ack_waiter() {
        let client = ServiceWsClient::new();
        client.connection_generation.store(1, Ordering::Release);
        *client.connected.lock().await = true;
        let (sender, mut outgoing) = mpsc::unbounded_channel();
        *client.send_channel.lock().await = Some((1, sender));
        let task = tokio::spawn({
            let client = client.clone();
            async move {
                client
                    .send_p2p_notify_for_generation(
                        "test".into(),
                        2,
                        "message".into(),
                        "synthetic".into(),
                        "{}".into(),
                        1,
                        std::time::Instant::now() + Duration::from_secs(30),
                    )
                    .await
            }
        });
        outgoing.recv().await.unwrap();
        assert_eq!(client.p2p_notify_acks.lock().await.len(), 1);
        task.abort();
        let _ = task.await;
        assert!(client.p2p_notify_acks.lock().await.is_empty());
    }

    #[tokio::test]
    async fn signal_from_old_socket_generation_never_enters_new_sender() {
        let client = ServiceWsClient::new();
        client.connection_generation.store(2, Ordering::Release);
        *client.connected.lock().await = true;
        let (sender, mut outgoing) = mpsc::unbounded_channel();
        *client.send_channel.lock().await = Some((2, sender));
        let result = client
            .send_p2p_notify_for_generation(
                "test".into(),
                2,
                "message".into(),
                "synthetic".into(),
                "{}".into(),
                1,
                std::time::Instant::now() + Duration::from_secs(5),
            )
            .await;
        assert!(result.is_err());
        assert!(outgoing.try_recv().is_err());
        assert!(client.p2p_notify_acks.lock().await.is_empty());
    }

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

    #[tokio::test]
    async fn disconnect_fails_only_waiters_from_its_generation() {
        let waiters = Arc::new(TokioMutex::new(HashMap::new()));
        let (old, old_rx) = pending_ack(4);
        let (current, mut current_rx) = pending_ack(5);
        {
            let mut pending = waiters.lock().await;
            pending.insert("old".to_string(), old);
            pending.insert("current".to_string(), current);
        }

        fail_pending_notify_acks_for_generation(&waiters, 4, "websocket_disconnected").await;

        assert_eq!(
            old_rx.await.unwrap().unwrap_err().code(),
            ErrorCode::SignalSendDisconnected
        );
        assert!(matches!(
            current_rx.try_recv(),
            Err(tokio::sync::oneshot::error::TryRecvError::Empty)
        ));
        assert_eq!(waiters.lock().await.len(), 1);
    }

    #[tokio::test]
    async fn stale_connection_cleanup_preserves_current_connection() {
        let connected = Arc::new(TokioMutex::new(true));
        let generation = Arc::new(AtomicU64::new(8));
        let (send_tx, _send_rx) = mpsc::unbounded_channel();
        let send_channel = Arc::new(TokioMutex::new(Some((8, send_tx))));
        let waiters = Arc::new(TokioMutex::new(HashMap::new()));

        let was_current = cleanup_connection_generation(
            &connected,
            &generation,
            &send_channel,
            &waiters,
            7,
            "websocket_disconnected",
        )
        .await;

        assert!(!was_current);
        assert!(*connected.lock().await);
        assert_eq!(send_channel.lock().await.as_ref().unwrap().0, 8);
    }

    #[tokio::test]
    async fn generation_change_while_cleanup_waits_preserves_connected_state() {
        let connected = Arc::new(TokioMutex::new(true));
        let connected_guard = connected.lock().await;
        let generation = Arc::new(AtomicU64::new(10));
        let (send_tx, _send_rx) = mpsc::unbounded_channel();
        let send_channel = Arc::new(TokioMutex::new(Some((11, send_tx))));
        let waiters = Arc::new(TokioMutex::new(HashMap::new()));

        let cleanup = tokio::spawn({
            let connected = connected.clone();
            let generation = generation.clone();
            let send_channel = send_channel.clone();
            let waiters = waiters.clone();
            async move {
                cleanup_connection_generation(
                    &connected,
                    &generation,
                    &send_channel,
                    &waiters,
                    10,
                    "websocket_disconnected",
                )
                .await
            }
        });

        tokio::task::yield_now().await;
        generation.store(11, Ordering::Release);
        drop(connected_guard);

        assert!(!cleanup.await.unwrap());
        assert!(*connected.lock().await);
        assert_eq!(send_channel.lock().await.as_ref().unwrap().0, 11);
    }

    #[tokio::test]
    async fn current_connection_cleanup_clears_sender_and_waiters() {
        let connected = Arc::new(TokioMutex::new(true));
        let generation = Arc::new(AtomicU64::new(9));
        let (send_tx, _send_rx) = mpsc::unbounded_channel();
        let send_channel = Arc::new(TokioMutex::new(Some((9, send_tx))));
        let waiters = Arc::new(TokioMutex::new(HashMap::new()));
        let (pending, receiver) = pending_ack(9);
        waiters.lock().await.insert("pending".to_string(), pending);

        let was_current = cleanup_connection_generation(
            &connected,
            &generation,
            &send_channel,
            &waiters,
            9,
            "websocket_disconnected",
        )
        .await;

        assert!(was_current);
        assert!(!*connected.lock().await);
        assert!(send_channel.lock().await.is_none());
        assert!(waiters.lock().await.is_empty());
        assert_eq!(
            receiver.await.unwrap().unwrap_err().code(),
            ErrorCode::SignalSendDisconnected
        );
    }
}
