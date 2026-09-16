//! Tunnel health server owned by the background service.

use std::collections::HashMap;
use std::net::IpAddr;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex as StdMutex};

use crate::speed_test::TunnelSpeedTestResult;
use crate::tunnel_control::{
    protocol_version_supported, TunnelControlConnection, TunnelControlMessage,
    TUNNEL_CONTROL_PROTOCOL_VERSION,
};
use anyhow::{anyhow, Context, Result};
use tokio::net::{TcpListener, TcpStream};
use tokio::sync::{mpsc, oneshot, watch};
use tracing::{debug, info, warn};

pub const HEALTH_PORT: u16 = 48082;
const PEER_SESSION_LOOKUP_RETRIES: usize = 5;
const PEER_SESSION_LOOKUP_RETRY_DELAY_MS: u64 = 100;

static NEXT_HEALTH_CONNECTION_GENERATION: AtomicU64 = AtomicU64::new(1);
static NEXT_SPEED_TEST_REQUEST_ID: AtomicU64 = AtomicU64::new(1);

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PassiveHealthEvent {
    Connected,
    HeartbeatSucceeded { latency_ms: Option<u32> },
    Disconnected { reason: &'static str },
}

pub type HealthDisconnectHandler = Arc<dyn Fn(i64, u64, PassiveHealthEvent) + Send + Sync>;
pub type PeerIdentityValidator = Arc<dyn Fn(i64, IpAddr) -> Result<(), String> + Send + Sync>;

/// peer_device_id → 该连接的停止信号发送器。
///
/// 被动端主动断开隧道时，通过此表通知对应的连接 task 退出，
/// 从而 drop `TcpStream` 触发 TCP FIN 立即送达主动端。
struct PassiveSpeedTestCommand {
    response: oneshot::Sender<Result<TunnelSpeedTestResult, String>>,
}

struct PeerHealthControl {
    generation: u64,
    stop_tx: watch::Sender<bool>,
    speed_tx: mpsc::UnboundedSender<PassiveSpeedTestCommand>,
}

type PeerHealthRegistry = Arc<StdMutex<HashMap<i64, PeerHealthControl>>>;

pub struct HealthServerHandle {
    stop_tx: watch::Sender<bool>,
    peer_controls: PeerHealthRegistry,
    task: tokio::task::JoinHandle<()>,
}

impl HealthServerHandle {
    /// 全局停止：仅 service 退出时调用。消费 self。
    pub fn stop(self) {
        let _ = self.stop_tx.send(true);
        self.task.abort();
    }

    /// 方案 A 核心：主动关闭指定 peer 的 health TCP 连接。
    ///
    /// 发送 stop 信号后，对应的 `health_server_connection_loop` task 会 break 退出，
    /// drop `TcpStream` 触发 TCP FIN 立即送达主动端，主动端 `conn.next()` 返回
    /// `None` → `ConnectionClosed` → 立即清理（无宽限期）。
    ///
    /// peer 不存在时静默（幂等，重复调用无害）。
    pub fn close_peer_connection(&self, peer_device_id: i64) {
        if let Some(control) = self
            .peer_controls
            .lock()
            .ok()
            .and_then(|mut m| m.remove(&peer_device_id))
        {
            let _ = control.stop_tx.send(true);
        }
    }

    /// 通过主动端已经建立的健康长连接请求测速。测速执行端始终是主动端；
    /// 返回值已转换为被动端视角的上传/下载方向。
    pub async fn request_peer_speed_test(
        &self,
        peer_device_id: i64,
    ) -> Result<TunnelSpeedTestResult> {
        let speed_tx = self
            .peer_controls
            .lock()
            .map_err(|_| anyhow!("health connection registry lock poisoned"))?
            .get(&peer_device_id)
            .map(|control| control.speed_tx.clone())
            .ok_or_else(|| anyhow!("health control connection is unavailable"))?;
        let (response_tx, response_rx) = oneshot::channel();
        speed_tx
            .send(PassiveSpeedTestCommand {
                response: response_tx,
            })
            .map_err(|_| anyhow!("health control connection is unavailable"))?;
        tokio::time::timeout(std::time::Duration::from_secs(40), response_rx)
            .await
            .context("tunnel speed test timed out")?
            .context("health control connection closed")?
            .map_err(anyhow::Error::msg)
    }
}

pub async fn spawn_health_server(
    on_disconnect: HealthDisconnectHandler,
    validate_peer: PeerIdentityValidator,
) -> Result<HealthServerHandle> {
    info!("[Health] binding on 0.0.0.0:{}", HEALTH_PORT);
    let listener = TcpListener::bind(format!("0.0.0.0:{}", HEALTH_PORT))
        .await
        .with_context(|| format!("failed to start health server on port {}", HEALTH_PORT))?;
    match listener.local_addr() {
        Ok(addr) => info!("[Health] started on {}", addr),
        Err(_) => info!("[Health] started on port {}", HEALTH_PORT),
    }

    let (stop_tx, stop_rx) = watch::channel(false);
    let peer_controls: PeerHealthRegistry = Arc::new(StdMutex::new(HashMap::new()));
    let task = tokio::spawn({
        let peer_controls = peer_controls.clone();
        async move {
            health_server_loop(
                listener,
                stop_rx,
                peer_controls,
                on_disconnect,
                validate_peer,
            )
            .await;
        }
    });

    Ok(HealthServerHandle {
        stop_tx,
        peer_controls,
        task,
    })
}

async fn health_server_loop(
    listener: TcpListener,
    mut stop_rx: watch::Receiver<bool>,
    peer_controls: PeerHealthRegistry,
    on_disconnect: HealthDisconnectHandler,
    validate_peer: PeerIdentityValidator,
) {
    loop {
        tokio::select! {
            changed = stop_rx.changed() => {
                if changed.is_ok() && *stop_rx.borrow() {
                    break;
                }
            }
            accepted = listener.accept() => {
                match accepted {
                    Ok((stream, addr)) => {
                        debug!("[Health] connection accepted from {}", addr);
                        let handler = on_disconnect.clone();
                        let peer_controls = peer_controls.clone();
                        let validate_peer = validate_peer.clone();
                        tokio::spawn(async move {
                            health_server_connection_loop(handler, peer_controls, stream, validate_peer).await;
                        });
                    }
                    Err(err) => {
                        warn!("[Health] accept failed: {}", err);
                    }
                }
            }
        }
    }
    info!("[Health] stopped");
}

async fn health_server_connection_loop(
    on_disconnect: HealthDisconnectHandler,
    peer_controls: PeerHealthRegistry,
    stream: TcpStream,
    validate_peer: PeerIdentityValidator,
) {
    let connection_generation = NEXT_HEALTH_CONNECTION_GENERATION.fetch_add(1, Ordering::Relaxed);
    let peer = stream.peer_addr().ok();
    debug!("[Health] connection loop started, peer={:?}", peer);
    let mut conn = TunnelControlConnection::new(stream);

    let (source_device_id, negotiated_protocol_version) =
        match tokio::time::timeout(std::time::Duration::from_secs(5), conn.next()).await {
            Ok(Ok(Some(TunnelControlMessage::Hello {
                source_device_id,
                protocol_version,
            }))) if protocol_version_supported(protocol_version) => {
                (source_device_id, protocol_version)
            }
            Ok(Ok(Some(TunnelControlMessage::Hello {
                protocol_version, ..
            }))) => {
                warn!(
                    "[Health] unsupported protocol version from {:?}: {}",
                    peer, protocol_version
                );
                let _ = conn
                    .send(&TunnelControlMessage::HelloAck {
                        ok: false,
                        protocol_version: TUNNEL_CONTROL_PROTOCOL_VERSION,
                        message: "unsupported protocol version".to_string(),
                    })
                    .await;
                return;
            }
            Ok(Ok(Some(other))) => {
                warn!("[Health] expected Hello from {:?}, got {:?}", peer, other);
                return;
            }
            Ok(Ok(None)) => {
                warn!("[Health] connection closed before Hello, peer={:?}", peer);
                return;
            }
            Ok(Err(err)) => {
                warn!("[Health] read Hello failed from {:?}: {}", peer, err);
                return;
            }
            Err(_) => {
                warn!("[Health] Hello timeout from peer={:?}", peer);
                return;
            }
        };

    let Some(peer_ip) = peer.map(|address| address.ip()) else {
        warn!("[Health] tunnel_peer_identity_mismatch: peer address unavailable");
        return;
    };
    let mut validation_result = validate_peer(source_device_id, peer_ip);
    for _ in 0..PEER_SESSION_LOOKUP_RETRIES {
        let Err(error) = &validation_result else {
            break;
        };
        if error != "wgvpn session not found" {
            break;
        }
        tokio::time::sleep(std::time::Duration::from_millis(
            PEER_SESSION_LOOKUP_RETRY_DELAY_MS,
        ))
        .await;
        validation_result = validate_peer(source_device_id, peer_ip);
    }
    if let Err(error) = validation_result {
        warn!(
            "[Health] tunnel_peer_identity_mismatch: source_device_id={}, peer_ip={}, error={}",
            source_device_id, peer_ip, error
        );
        let _ = conn
            .send(&TunnelControlMessage::HelloAck {
                ok: false,
                protocol_version: TUNNEL_CONTROL_PROTOCOL_VERSION,
                message: format!("tunnel_peer_identity_mismatch: {error}"),
            })
            .await;
        return;
    }

    if conn
        .send(&TunnelControlMessage::HelloAck {
            ok: true,
            // Echo the accepted version so strict v3/v4/v5 clients can use the common
            // health/heartbeat/speed subset while this endpoint advertises v3 itself.
            protocol_version: negotiated_protocol_version,
            message: "ok".to_string(),
        })
        .await
        .is_err()
    {
        warn!(
            "[Health] send HelloAck failed, source_device_id={}, peer={:?}",
            source_device_id, peer
        );
        return;
    }

    // 健康连接恢复时通知上层取消被动会话的断线宽限清理。
    on_disconnect(
        source_device_id,
        connection_generation,
        PassiveHealthEvent::Connected,
    );

    // 注册 per-peer 停止信号：被动端主动断开隧道时，stop_wgvpn_job 通过
    // HealthServerHandle.close_peer_connection 发送 true，本 loop 收到后 break，
    // drop `TcpStream` 触发 TCP FIN 立即送达主动端（conn.next() → None）。
    let (peer_stop_tx, mut peer_stop_rx) = watch::channel(false);
    let (speed_request_tx, mut speed_request_rx) = mpsc::unbounded_channel();
    let replaced_peer_control = peer_controls.lock().ok().and_then(|mut registry| {
        registry.insert(
            source_device_id,
            PeerHealthControl {
                generation: connection_generation,
                stop_tx: peer_stop_tx,
                speed_tx: speed_request_tx,
            },
        )
    });
    if let Some(previous) = replaced_peer_control {
        // 同一设备建立了新一代连接。显式关闭旧连接，避免旧 sender 被覆盖后
        // receiver 永久以 Err 立即返回并形成 busy loop。
        let _ = previous.stop_tx.send(true);
    }

    // 清理守卫：无论从哪个分支退出，都从 registry 移除自己的 sender，避免泄漏。
    // 用闭包封装保证所有 return/break 路径都执行清理。
    let cleanup_registry = || {
        if let Ok(mut registry) = peer_controls.lock() {
            // 旧连接可能已被同 device_id 的新连接替换；只清理自己的代次，
            // 避免旧 task 退出时误删新连接的 sender。
            let owns_entry = registry
                .get(&source_device_id)
                .is_some_and(|control| control.generation == connection_generation);
            if owns_entry {
                registry.remove(&source_device_id);
            }
        }
    };

    let mut speed_server_task = None;
    let mut pending_speed_test = None;
    loop {
        tokio::select! {
            // 被动端主动断开：直接 break，drop TcpStream 触发 TCP FIN。
            // 不发任何消息——主动端 conn.next() 返回 None 即触发 ConnectionClosed。
            changed = peer_stop_rx.changed() => {
                match changed {
                    Ok(()) if *peer_stop_rx.borrow() => {
                        info!(
                            "[Health] passive initiated close, source_device_id={}",
                            source_device_id
                        );
                        break;
                    }
                    Ok(()) => {}
                    Err(_) => {
                        // 所有 sender 被释放后 changed() 会永久立即返回 Err。
                        // 必须退出，否则该 select 循环会占满一个 CPU 核。
                        debug!(
                            "[Health] peer stop channel closed, source_device_id={}",
                            source_device_id
                        );
                        break;
                    }
                }
            }
            request = speed_request_rx.recv() => {
                let Some(request) = request else {
                    break;
                };
                if pending_speed_test.is_some() {
                    let _ = request.response.send(Err("speed test already in progress".to_string()));
                    continue;
                }
                let request_id = NEXT_SPEED_TEST_REQUEST_ID.fetch_add(1, Ordering::Relaxed);
                if let Err(err) = conn
                    .send(&TunnelControlMessage::SpeedTestRequest { request_id })
                    .await
                {
                    let _ = request.response.send(Err(err.to_string()));
                    on_disconnect(
                        source_device_id,
                        connection_generation,
                        PassiveHealthEvent::Disconnected { reason: "write_failed" },
                    );
                    break;
                }
                pending_speed_test = Some((request_id, request.response));
            }
            msg_result = tokio::time::timeout(std::time::Duration::from_secs(60), conn.next()) => {
                // 主动端每 5 秒发送一次 Ping；60 秒没有心跳后进入网络异常。
                let message = match msg_result {
                    Ok(Ok(Some(message))) => message,
                    Ok(Ok(None)) => {
                        info!(
                            "[Health] peer disconnected, source_device_id={}",
                            source_device_id
                        );
                        on_disconnect(
                            source_device_id,
                            connection_generation,
                            PassiveHealthEvent::Disconnected { reason: "peer_disconnected" },
                        );
                        break;
                    }
                    Ok(Err(err)) => {
                        warn!(
                            "[Health] read heartbeat failed, source_device_id={}, error={}",
                            source_device_id, err
                        );
                        on_disconnect(
                            source_device_id,
                            connection_generation,
                            PassiveHealthEvent::Disconnected { reason: "read_failed" },
                        );
                        break;
                    }
                    Err(_) => {
                        warn!(
                            "[Health] heartbeat timeout, source_device_id={}",
                            source_device_id
                        );
                        on_disconnect(
                            source_device_id,
                            connection_generation,
                            PassiveHealthEvent::Disconnected { reason: "heartbeat_timeout" },
                        );
                        break;
                    }
                };

                match message {
                    TunnelControlMessage::Ping { ts, reported_rtt_ms } => {
                        on_disconnect(
                            source_device_id,
                            connection_generation,
                            PassiveHealthEvent::HeartbeatSucceeded {
                                latency_ms: reported_rtt_ms,
                            },
                        );
                        if conn.send(&TunnelControlMessage::Pong { ts }).await.is_err() {
                            warn!(
                                "[Health] write pong failed, source_device_id={}",
                                source_device_id
                            );
                            on_disconnect(
                                source_device_id,
                                connection_generation,
                                PassiveHealthEvent::Disconnected { reason: "write_failed" },
                            );
                            break;
                        }
                    }
                    message @ (TunnelControlMessage::SpeedPing { .. } | TunnelControlMessage::SpeedStart) => {
                        match handle_speed_control_message(
                            &mut conn,
                            message,
                            source_device_id,
                        )
                        .await
                        {
                            Ok(Some(task)) => {
                                replace_speed_server_task(&mut speed_server_task, task).await;
                            }
                            Ok(None) => {}
                            Err(err) => {
                                if let Some((_request_id, response)) = pending_speed_test.take() {
                                    let _ = response.send(Err(err.to_string()));
                                }
                                warn!(
                                    "[SpeedTest] health control request failed, source_device_id={}, error={:#}",
                                    source_device_id, err
                                );
                                break;
                            }
                        }
                    }
                    TunnelControlMessage::SpeedTestResult {
                        request_id,
                        latency_ms,
                        download_mbps,
                        upload_mbps,
                        retransmits,
                    } => {
                        match pending_speed_test.take() {
                            Some((pending_id, response)) if pending_id == request_id => {
                                // 主动端报告的是主动端视角；转换成当前被动端视角。
                                let _ = response.send(Ok(TunnelSpeedTestResult {
                                    latency_ms,
                                    download_mbps: upload_mbps,
                                    upload_mbps: download_mbps,
                                    retransmits,
                                }));
                            }
                            Some(pending) => {
                                pending_speed_test = Some(pending);
                                warn!(
                                    "[SpeedTest] unexpected result id, source_device_id={}, request_id={}",
                                    source_device_id, request_id
                                );
                            }
                            None => warn!(
                                "[SpeedTest] result without request, source_device_id={}, request_id={}",
                                source_device_id, request_id
                            ),
                        }
                    }
                    TunnelControlMessage::SpeedTestError { request_id, message } => {
                        match pending_speed_test.take() {
                            Some((pending_id, response)) if pending_id == request_id => {
                                let _ = response.send(Err(message));
                            }
                            Some(pending) => {
                                pending_speed_test = Some(pending);
                                warn!(
                                    "[SpeedTest] unexpected error id, source_device_id={}, request_id={}",
                                    source_device_id, request_id
                                );
                            }
                            None => warn!(
                                "[SpeedTest] error without request, source_device_id={}, request_id={}",
                                source_device_id, request_id
                            ),
                        }
                    }
                    TunnelControlMessage::Stop { reason } => {
                        info!(
                            "[Health] remote requested tunnel stop, source_device_id={}, reason={}",
                            source_device_id, reason
                        );
                        on_disconnect(
                            source_device_id,
                            connection_generation,
                            PassiveHealthEvent::Disconnected { reason: "remote_stop" },
                        );
                        break;
                    }
                    other => {
                        warn!(
                            "[Health] unexpected tunnel message, source_device_id={}, message={:?}",
                            source_device_id, other
                        );
                    }
                }
            }
        }
    }

    if let Some((_request_id, response)) = pending_speed_test.take() {
        let _ = response.send(Err("health control connection closed".to_string()));
    }
    stop_speed_server_task(&mut speed_server_task).await;
    cleanup_registry();
}

async fn handle_speed_control_message(
    conn: &mut TunnelControlConnection<TcpStream>,
    message: TunnelControlMessage,
    source_device_id: i64,
) -> Result<Option<tokio::task::JoinHandle<()>>> {
    match message {
        TunnelControlMessage::SpeedPing { nonce } => {
            conn.send(&TunnelControlMessage::SpeedPong { nonce })
                .await?;
            Ok(None)
        }
        TunnelControlMessage::SpeedStart => {
            // 被动端临时作为 riperf3 server；主动端通过被动端虚拟 IP连接。
            let server_lease = crate::speed_test::acquire_speed_test_server_lease().await?;
            let server = crate::speed_test::build_speed_test_server()
                .context("failed to build speed test server")?;
            let server_task = tokio::spawn(async move {
                let _server_lease = server_lease;
                match tokio::time::timeout(
                    std::time::Duration::from_secs(crate::speed_test::SPEED_TEST_RUN_TIMEOUT_SECS),
                    server.run_once(),
                )
                .await
                {
                    Ok(Ok(_report)) => {}
                    Ok(Err(err)) => warn!("[SpeedTest] server run_once failed: {:#}", err),
                    Err(_) => warn!(
                        "[SpeedTest] server timed out after {}s",
                        crate::speed_test::SPEED_TEST_RUN_TIMEOUT_SECS
                    ),
                }
            });
            // riperf3 没有暴露 bind-ready 回调；先让 run_once 在独立任务中实际
            // 进入 listen，再给控制端 Ready。若任务已提前退出则明确报错。
            tokio::task::yield_now().await;
            tokio::time::sleep(std::time::Duration::from_millis(200)).await;
            if server_task.is_finished() {
                let _ = server_task.await;
                return Err(anyhow::anyhow!("speed test server exited before ready"));
            }
            info!(
                "[SpeedTest] server listening on 0.0.0.0:{}, requested_by={}",
                crate::speed_test::SPEED_TEST_PORT,
                source_device_id
            );
            if let Err(err) = conn.send(&TunnelControlMessage::SpeedReady).await {
                server_task.abort();
                let _ = server_task.await;
                return Err(err.context("failed to send speed ready"));
            }
            Ok(Some(server_task))
        }
        other => Err(anyhow::anyhow!(
            "unexpected speed control message: {:?}",
            other
        )),
    }
}

async fn replace_speed_server_task(
    current: &mut Option<tokio::task::JoinHandle<()>>,
    next: tokio::task::JoinHandle<()>,
) {
    stop_speed_server_task(current).await;
    *current = Some(next);
}

async fn stop_speed_server_task(current: &mut Option<tokio::task::JoinHandle<()>>) {
    if let Some(task) = current.take() {
        if !task.is_finished() {
            task.abort();
        }
        let _ = task.await;
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tokio::net::{TcpListener, TcpStream};
    use tokio::sync::mpsc;

    fn test_peer_validator() -> PeerIdentityValidator {
        Arc::new(|_, _| Ok(()))
    }

    #[tokio::test]
    async fn health_connection_rejects_tunnel_peer_identity_mismatch() {
        let listener = TcpListener::bind("127.0.0.1:0")
            .await
            .expect("bind listener");
        let addr = listener.local_addr().expect("listener address");
        tokio::spawn(async move {
            let (stream, _) = listener.accept().await.expect("accept stream");
            health_server_connection_loop(
                Arc::new(|_, _, _| {}),
                Arc::new(StdMutex::new(HashMap::new())),
                stream,
                Arc::new(|_, _| Err("virtual ip does not match session".to_string())),
            )
            .await;
        });
        let stream = TcpStream::connect(addr).await.expect("connect");
        let mut conn = TunnelControlConnection::new(stream);
        conn.send(&TunnelControlMessage::Hello {
            source_device_id: 42,
            protocol_version: TUNNEL_CONTROL_PROTOCOL_VERSION,
        })
        .await
        .expect("send hello");
        match conn.next().await.expect("read ack") {
            Some(TunnelControlMessage::HelloAck {
                ok: false, message, ..
            }) => {
                assert!(message.contains("tunnel_peer_identity_mismatch"));
            }
            other => panic!("expected rejected hello ack, got {other:?}"),
        }
    }

    async fn spawn_test_health_connection() -> (
        std::net::SocketAddr,
        mpsc::UnboundedReceiver<(i64, PassiveHealthEvent)>,
    ) {
        let listener = TcpListener::bind("127.0.0.1:0")
            .await
            .expect("bind test listener");
        let addr = listener.local_addr().expect("test listener addr");
        let (tx, rx) = mpsc::unbounded_channel();
        tokio::spawn(async move {
            let (stream, _) = listener.accept().await.expect("accept test stream");
            health_server_connection_loop(
                Arc::new(move |source_device_id, _generation, event| {
                    let _ = tx.send((source_device_id, event));
                }),
                Arc::new(StdMutex::new(HashMap::new())),
                stream,
                test_peer_validator(),
            )
            .await;
        });
        (addr, rx)
    }

    #[tokio::test]
    async fn health_connection_replies_hello_ok_and_pong() {
        let (addr, mut rx) = spawn_test_health_connection().await;
        let stream = TcpStream::connect(addr)
            .await
            .expect("connect health server");
        let mut conn = TunnelControlConnection::new(stream);

        conn.send(&TunnelControlMessage::Hello {
            source_device_id: 123,
            protocol_version: TUNNEL_CONTROL_PROTOCOL_VERSION,
        })
        .await
        .expect("write hello");
        assert_eq!(
            conn.next().await.expect("read hello ack"),
            Some(TunnelControlMessage::HelloAck {
                ok: true,
                protocol_version: TUNNEL_CONTROL_PROTOCOL_VERSION,
                message: "ok".to_string(),
            })
        );
        assert_eq!(rx.recv().await, Some((123, PassiveHealthEvent::Connected)));

        conn.send(&TunnelControlMessage::Ping {
            ts: 100,
            reported_rtt_ms: Some(37),
        })
        .await
        .expect("write ping");
        assert_eq!(
            conn.next().await.expect("read pong"),
            Some(TunnelControlMessage::Pong { ts: 100 })
        );
        assert_eq!(
            rx.recv().await,
            Some((
                123,
                PassiveHealthEvent::HeartbeatSucceeded {
                    latency_ms: Some(37)
                }
            ))
        );
    }

    #[tokio::test]
    async fn passive_speed_request_uses_health_connection_and_reverses_directions() {
        let listener = TcpListener::bind("127.0.0.1:0")
            .await
            .expect("bind test listener");
        let addr = listener.local_addr().expect("test listener addr");
        let peer_stops: PeerHealthRegistry = Arc::new(StdMutex::new(HashMap::new()));
        let peer_stops_for_task = peer_stops.clone();
        tokio::spawn(async move {
            let (stream, _) = listener.accept().await.expect("accept test stream");
            health_server_connection_loop(
                Arc::new(|_, _, _| {}),
                peer_stops_for_task,
                stream,
                test_peer_validator(),
            )
            .await;
        });
        let stream = TcpStream::connect(addr)
            .await
            .expect("connect health server");
        let mut conn = TunnelControlConnection::new(stream);

        conn.send(&TunnelControlMessage::Hello {
            source_device_id: 321,
            protocol_version: TUNNEL_CONTROL_PROTOCOL_VERSION,
        })
        .await
        .expect("write hello");
        assert!(matches!(
            conn.next().await.expect("read hello ack"),
            Some(TunnelControlMessage::HelloAck { ok: true, .. })
        ));

        let speed_tx = loop {
            if let Some(speed_tx) = peer_stops
                .lock()
                .expect("lock registry")
                .get(&321)
                .map(|control| control.speed_tx.clone())
            {
                break speed_tx;
            }
            tokio::task::yield_now().await;
        };
        let (response_tx, response_rx) = oneshot::channel();
        speed_tx
            .send(PassiveSpeedTestCommand {
                response: response_tx,
            })
            .expect("send passive speed request");
        let request_id = match conn.next().await.expect("read speed request") {
            Some(TunnelControlMessage::SpeedTestRequest { request_id }) => request_id,
            other => panic!("unexpected speed request: {:?}", other),
        };
        conn.send(&TunnelControlMessage::SpeedTestResult {
            request_id,
            latency_ms: 10.0,
            download_mbps: 20.0,
            upload_mbps: 30.0,
            retransmits: Some(1),
        })
        .await
        .expect("write speed result");

        let result = response_rx
            .await
            .expect("receive passive result")
            .expect("speed test succeeds");
        assert_eq!(result.download_mbps, 30.0);
        assert_eq!(result.upload_mbps, 20.0);
        assert_eq!(result.retransmits, Some(1));
    }

    #[tokio::test]
    async fn stopping_control_connection_task_releases_speed_server_lease() {
        let lease = crate::speed_test::acquire_speed_test_server_lease()
            .await
            .expect("acquire first lease");
        let task = tokio::spawn(async move {
            let _lease = lease;
            std::future::pending::<()>().await;
        });
        let mut current = Some(task);

        stop_speed_server_task(&mut current).await;
        assert!(current.is_none());

        let second = tokio::time::timeout(
            std::time::Duration::from_secs(1),
            crate::speed_test::acquire_speed_test_server_lease(),
        )
        .await
        .expect("lease should be released after control cleanup")
        .expect("acquire second lease");
        drop(second);
    }

    #[tokio::test]
    async fn health_connection_reports_peer_disconnected() {
        let (addr, mut rx) = spawn_test_health_connection().await;
        let stream = TcpStream::connect(addr)
            .await
            .expect("connect health server");
        let mut conn = TunnelControlConnection::new(stream);

        conn.send(&TunnelControlMessage::Hello {
            source_device_id: 456,
            protocol_version: TUNNEL_CONTROL_PROTOCOL_VERSION,
        })
        .await
        .expect("write hello");
        assert!(matches!(
            conn.next().await.expect("read hello ack"),
            Some(TunnelControlMessage::HelloAck { ok: true, .. })
        ));

        assert_eq!(rx.recv().await, Some((456, PassiveHealthEvent::Connected)));

        drop(conn);

        let (source_device_id, event) =
            tokio::time::timeout(std::time::Duration::from_secs(2), rx.recv())
                .await
                .expect("wait disconnect event")
                .expect("disconnect payload");
        assert_eq!(source_device_id, 456);
        assert_eq!(
            event,
            PassiveHealthEvent::Disconnected {
                reason: "peer_disconnected"
            }
        );
    }

    #[tokio::test]
    async fn health_connection_reports_remote_stop() {
        let (addr, mut rx) = spawn_test_health_connection().await;
        let stream = TcpStream::connect(addr)
            .await
            .expect("connect health server");
        let mut conn = TunnelControlConnection::new(stream);

        conn.send(&TunnelControlMessage::Hello {
            source_device_id: 789,
            protocol_version: TUNNEL_CONTROL_PROTOCOL_VERSION,
        })
        .await
        .expect("write hello");
        assert!(matches!(
            conn.next().await.expect("read hello ack"),
            Some(TunnelControlMessage::HelloAck { ok: true, .. })
        ));

        assert_eq!(rx.recv().await, Some((789, PassiveHealthEvent::Connected)));

        conn.send(&TunnelControlMessage::Stop {
            reason: "user_closed".to_string(),
        })
        .await
        .expect("write stop");

        let (source_device_id, event) =
            tokio::time::timeout(std::time::Duration::from_secs(2), rx.recv())
                .await
                .expect("wait stop event")
                .expect("stop payload");
        assert_eq!(source_device_id, 789);
        assert_eq!(
            event,
            PassiveHealthEvent::Disconnected {
                reason: "remote_stop"
            }
        );
    }

    #[tokio::test]
    async fn health_connection_rejects_unsupported_protocol_version() {
        let (addr, _rx) = spawn_test_health_connection().await;
        let stream = TcpStream::connect(addr)
            .await
            .expect("connect health server");
        let mut conn = TunnelControlConnection::new(stream);

        // 兼容范围内（3..=MAX_COMPAT）的版本应被接受，超出上界才拒绝。
        conn.send(&TunnelControlMessage::Hello {
            source_device_id: 111,
            protocol_version: crate::tunnel_control::TUNNEL_CONTROL_PROTOCOL_MAX_COMPAT + 1,
        })
        .await
        .expect("write hello");

        assert_eq!(
            conn.next().await.expect("read hello ack"),
            Some(TunnelControlMessage::HelloAck {
                ok: false,
                protocol_version: TUNNEL_CONTROL_PROTOCOL_VERSION,
                message: "unsupported protocol version".to_string(),
            })
        );
    }

    /// 验证方案 A 核心：外部通过 PeerHealthRegistry 发送 stop 信号后，
    /// 连接 task 退出并 drop TcpStream，主动端 conn.next() 返回 None。
    ///
    /// 这模拟了被动端点击"断开"时 stop_wgvpn_job 调用 close_peer_connection 的场景：
    /// 被动端 health server 主动关闭 TCP 连接 → TCP FIN → 主动端立即感知。
    #[tokio::test]
    async fn health_connection_closes_on_passive_stop_signal() {
        // 这个测试需要从外部触发 per-peer stop，所以直接用 PeerHealthRegistry。
        let listener = TcpListener::bind("127.0.0.1:0")
            .await
            .expect("bind test listener");
        let addr = listener.local_addr().expect("test listener addr");
        let (tx, mut rx) = mpsc::unbounded_channel();
        let peer_stops: PeerHealthRegistry = Arc::new(StdMutex::new(HashMap::new()));
        let peer_stops_for_task = peer_stops.clone();

        tokio::spawn(async move {
            let (stream, _) = listener.accept().await.expect("accept test stream");
            health_server_connection_loop(
                Arc::new(move |source_device_id, _generation, event| {
                    let _ = tx.send((source_device_id, event));
                }),
                peer_stops_for_task,
                stream,
                test_peer_validator(),
            )
            .await;
        });

        let stream = TcpStream::connect(addr)
            .await
            .expect("connect health server");
        let mut conn = TunnelControlConnection::new(stream);

        conn.send(&TunnelControlMessage::Hello {
            source_device_id: 222,
            protocol_version: TUNNEL_CONTROL_PROTOCOL_VERSION,
        })
        .await
        .expect("write hello");
        assert!(matches!(
            conn.next().await.expect("read hello ack"),
            Some(TunnelControlMessage::HelloAck { ok: true, .. })
        ));

        // 等待 connected 回调（确认 per-peer sender 已注册）
        assert_eq!(rx.recv().await, Some((222, PassiveHealthEvent::Connected)));

        // 模拟 stop_wgvpn_job 调用 close_peer_connection：
        // 取出 sender 并发送 stop 信号。
        let control = peer_stops
            .lock()
            .expect("lock registry")
            .remove(&222)
            .expect("peer stop sender registered");
        control.stop_tx.send(true).expect("send stop signal");

        // 被动端连接 task 收到信号后 break，drop TcpStream 发送 TCP FIN。
        // 主动端 conn.next() 应返回 None（不返回任何消息、不报错）。
        let result = tokio::time::timeout(std::time::Duration::from_secs(2), conn.next())
            .await
            .expect("conn.next should return promptly after peer stop");
        assert_eq!(result.expect("read should succeed"), None);

        // 方案 A 核心验证：被动端主动关闭后，主动端 conn.next() 返回 None
        // （TCP FIN 生效）。被动端 connection_loop 收到 stop 信号直接 break，
        // 不会触发 on_disconnect 回调——但即使触发了也无害（cleanup 幂等），
        // 这里只验证 TCP FIN 的确定性传递，不约束 rx 行为。
    }

    #[tokio::test]
    async fn newer_health_connection_replaces_and_closes_previous_generation() {
        let listener = TcpListener::bind("127.0.0.1:0")
            .await
            .expect("bind test listener");
        let addr = listener.local_addr().expect("test listener addr");
        let peer_stops: PeerHealthRegistry = Arc::new(StdMutex::new(HashMap::new()));

        let peer_stops_for_task = peer_stops.clone();
        tokio::spawn(async move {
            for _ in 0..2 {
                let (stream, _) = listener.accept().await.expect("accept test stream");
                let peer_stops = peer_stops_for_task.clone();
                tokio::spawn(async move {
                    health_server_connection_loop(
                        Arc::new(|_, _, _| {}),
                        peer_stops,
                        stream,
                        test_peer_validator(),
                    )
                    .await;
                });
            }
        });

        let first_stream = TcpStream::connect(addr)
            .await
            .expect("connect first health client");
        let mut first = TunnelControlConnection::new(first_stream);
        first
            .send(&TunnelControlMessage::Hello {
                source_device_id: 333,
                protocol_version: TUNNEL_CONTROL_PROTOCOL_VERSION,
            })
            .await
            .expect("write first hello");
        assert!(matches!(
            first.next().await.expect("read first hello ack"),
            Some(TunnelControlMessage::HelloAck { ok: true, .. })
        ));

        let second_stream = TcpStream::connect(addr)
            .await
            .expect("connect replacement health client");
        let mut second = TunnelControlConnection::new(second_stream);
        second
            .send(&TunnelControlMessage::Hello {
                source_device_id: 333,
                protocol_version: TUNNEL_CONTROL_PROTOCOL_VERSION,
            })
            .await
            .expect("write replacement hello");
        assert!(matches!(
            second.next().await.expect("read replacement hello ack"),
            Some(TunnelControlMessage::HelloAck { ok: true, .. })
        ));

        let first_result = tokio::time::timeout(std::time::Duration::from_secs(2), first.next())
            .await
            .expect("previous connection should close promptly");
        assert_eq!(first_result.expect("previous connection read"), None);

        // 旧连接 cleanup 不能删除新一代 sender；新连接仍应正常响应。
        assert!(peer_stops.lock().expect("lock registry").contains_key(&333));
        second
            .send(&TunnelControlMessage::Ping {
                ts: 333,
                reported_rtt_ms: None,
            })
            .await
            .expect("write ping on replacement connection");
        assert_eq!(
            second.next().await.expect("read replacement pong"),
            Some(TunnelControlMessage::Pong { ts: 333 })
        );
    }
}
