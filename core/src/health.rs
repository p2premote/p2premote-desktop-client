//! Tunnel health server owned by the background service.

use std::collections::HashMap;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex as StdMutex};

use crate::tunnel_control::{
    TunnelControlConnection, TunnelControlMessage, TUNNEL_CONTROL_PROTOCOL_VERSION,
};
use anyhow::{Context, Result};
use tokio::net::{TcpListener, TcpStream};
use tokio::sync::watch;
use tracing::{debug, info, warn};

pub const HEALTH_PORT: u16 = 48082;

static NEXT_HEALTH_CONNECTION_GENERATION: AtomicU64 = AtomicU64::new(1);

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PassiveHealthEvent {
    Connected,
    HeartbeatSucceeded { latency_ms: Option<u32> },
    Disconnected { reason: &'static str },
}

pub type HealthDisconnectHandler = Arc<dyn Fn(i64, u64, PassiveHealthEvent) + Send + Sync>;

/// peer_device_id → 该连接的停止信号发送器。
///
/// 被动端主动断开隧道时，通过此表通知对应的连接 task 退出，
/// 从而 drop `TcpStream` 触发 TCP FIN 立即送达主动端。
pub type PeerStopRegistry = Arc<StdMutex<HashMap<i64, (u64, watch::Sender<bool>)>>>;

pub struct HealthServerHandle {
    stop_tx: watch::Sender<bool>,
    peer_stops: PeerStopRegistry,
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
        if let Some((_generation, tx)) = self
            .peer_stops
            .lock()
            .ok()
            .and_then(|mut m| m.remove(&peer_device_id))
        {
            let _ = tx.send(true);
        }
    }
}

pub async fn spawn_health_server(
    on_disconnect: HealthDisconnectHandler,
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
    let peer_stops: PeerStopRegistry = Arc::new(StdMutex::new(HashMap::new()));
    let task = tokio::spawn({
        let peer_stops = peer_stops.clone();
        async move {
            health_server_loop(listener, stop_rx, peer_stops, on_disconnect).await;
        }
    });

    Ok(HealthServerHandle {
        stop_tx,
        peer_stops,
        task,
    })
}

async fn health_server_loop(
    listener: TcpListener,
    mut stop_rx: watch::Receiver<bool>,
    peer_stops: PeerStopRegistry,
    on_disconnect: HealthDisconnectHandler,
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
                        info!("[Health] connection accepted from {}", addr);
                        let handler = on_disconnect.clone();
                        let peer_stops = peer_stops.clone();
                        tokio::spawn(async move {
                            health_server_connection_loop(handler, peer_stops, stream).await;
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
    peer_stops: PeerStopRegistry,
    stream: TcpStream,
) {
    let connection_generation = NEXT_HEALTH_CONNECTION_GENERATION.fetch_add(1, Ordering::Relaxed);
    let peer = stream.peer_addr().ok();
    debug!("[Health] connection loop started, peer={:?}", peer);
    let mut conn = TunnelControlConnection::new(stream);

    let source_device_id =
        match tokio::time::timeout(std::time::Duration::from_secs(5), conn.next()).await {
            Ok(Ok(Some(TunnelControlMessage::Hello {
                source_device_id,
                protocol_version,
            }))) if protocol_version == TUNNEL_CONTROL_PROTOCOL_VERSION => source_device_id,
            Ok(Ok(Some(TunnelControlMessage::Hello {
                source_device_id: _,
                protocol_version,
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

    if conn
        .send(&TunnelControlMessage::HelloAck {
            ok: true,
            protocol_version: TUNNEL_CONTROL_PROTOCOL_VERSION,
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
    let replaced_peer_stop = peer_stops.lock().ok().and_then(|mut registry| {
        registry.insert(source_device_id, (connection_generation, peer_stop_tx))
    });
    if let Some((_previous_generation, previous_tx)) = replaced_peer_stop {
        // 同一设备建立了新一代连接。显式关闭旧连接，避免旧 sender 被覆盖后
        // receiver 永久以 Err 立即返回并形成 busy loop。
        let _ = previous_tx.send(true);
    }

    // 清理守卫：无论从哪个分支退出，都从 registry 移除自己的 sender，避免泄漏。
    // 用闭包封装保证所有 return/break 路径都执行清理。
    let cleanup_registry = || {
        if let Ok(mut registry) = peer_stops.lock() {
            // 旧连接可能已被同 device_id 的新连接替换；只清理自己的代次，
            // 避免旧 task 退出时误删新连接的 sender。
            let owns_entry = registry
                .get(&source_device_id)
                .is_some_and(|(generation, _)| *generation == connection_generation);
            if owns_entry {
                registry.remove(&source_device_id);
            }
        }
    };

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
                    TunnelControlMessage::SpeedPing { nonce } => {
                        if conn
                            .send(&TunnelControlMessage::SpeedPong { nonce })
                            .await
                            .is_err()
                        {
                            break;
                        }
                    }
                    TunnelControlMessage::SpeedStart => {
                        // 被动端作为 riperf3 server（被主动端 client 连）。
                        // 架构依据：wgvpn userspace 后端只有"主动端→被动端"方向可达，
                        // 所以被动端做 server、主动端做 client。
                        //
                        // 必须在独立 task 里跑 run_once：每个方向测速持续约 8 秒（6s 数据 + 握手），
                        // 若同步等待会占用 health loop，期间无法应答 Ping。
                        // 测试时长由两端各自的 SPEED_TEST_DURATION_SECS 常量决定。
                        let server = match crate::speed_test::build_speed_test_server() {
                            Ok(s) => s,
                            Err(err) => {
                                warn!(
                                    "[SpeedTest] passive server build failed: {:#}, source_device_id={}",
                                    err, source_device_id
                                );
                                break;
                            }
                        };
                        // 先 spawn run_once 让 server 开始 listen，再回 SpeedReady。
                        // 时序关键：riperf3 client 连接失败不重试，必须等 listen 就绪。
                        let (ready_tx, ready_rx) = tokio::sync::oneshot::channel::<()>();
                        tokio::spawn(async move {
                            // riperf3 0.8.0 的 run_once 内部自己 tcp_listen，无法在 run 前
                            // 拿到 listen 就绪信号。给一个短暂 yield 让 listen 启动，然后
                            // 通知主动端。run_once 会阻塞到服务完一个 client。
                            tokio::task::yield_now().await;
                            tokio::time::sleep(std::time::Duration::from_millis(200)).await;
                            let _ = ready_tx.send(());
                            if let Err(err) = server.run_once().await {
                                warn!("[SpeedTest] passive server run_once failed: {:#}", err);
                            }
                        });
                        // 等 ready 信号（理论上立即就绪，加超时兜底）。
                        match tokio::time::timeout(std::time::Duration::from_secs(3), ready_rx).await {
                            Ok(Ok(())) => {}
                            _ => {
                                warn!(
                                    "[SpeedTest] passive server ready signal timeout, source_device_id={}",
                                    source_device_id
                                );
                            }
                        }
                        info!(
                            "[SpeedTest] passive server listening on 0.0.0.0:{}",
                            crate::speed_test::SPEED_TEST_PORT
                        );
                        if conn.send(&TunnelControlMessage::SpeedReady).await.is_err() {
                            break;
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

    cleanup_registry();
}

#[cfg(test)]
mod tests {
    use super::*;
    use tokio::net::{TcpListener, TcpStream};
    use tokio::sync::mpsc;

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

        conn.send(&TunnelControlMessage::Hello {
            source_device_id: 111,
            protocol_version: TUNNEL_CONTROL_PROTOCOL_VERSION + 1,
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

    /// 验证方案 A 核心：外部通过 PeerStopRegistry 发送 stop 信号后，
    /// 连接 task 退出并 drop TcpStream，主动端 conn.next() 返回 None。
    ///
    /// 这模拟了被动端点击"断开"时 stop_wgvpn_job 调用 close_peer_connection 的场景：
    /// 被动端 health server 主动关闭 TCP 连接 → TCP FIN → 主动端立即感知。
    #[tokio::test]
    async fn health_connection_closes_on_passive_stop_signal() {
        // 这个测试需要从外部触发 per-peer stop，所以直接用 PeerStopRegistry。
        let listener = TcpListener::bind("127.0.0.1:0")
            .await
            .expect("bind test listener");
        let addr = listener.local_addr().expect("test listener addr");
        let (tx, mut rx) = mpsc::unbounded_channel();
        let peer_stops: PeerStopRegistry = Arc::new(StdMutex::new(HashMap::new()));
        let peer_stops_for_task = peer_stops.clone();

        tokio::spawn(async move {
            let (stream, _) = listener.accept().await.expect("accept test stream");
            health_server_connection_loop(
                Arc::new(move |source_device_id, _generation, event| {
                    let _ = tx.send((source_device_id, event));
                }),
                peer_stops_for_task,
                stream,
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
        let (_generation, stop_tx) = peer_stops
            .lock()
            .expect("lock registry")
            .remove(&222)
            .expect("peer stop sender registered");
        stop_tx.send(true).expect("send stop signal");

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
        let peer_stops: PeerStopRegistry = Arc::new(StdMutex::new(HashMap::new()));

        let peer_stops_for_task = peer_stops.clone();
        tokio::spawn(async move {
            for _ in 0..2 {
                let (stream, _) = listener.accept().await.expect("accept test stream");
                let peer_stops = peer_stops_for_task.clone();
                tokio::spawn(async move {
                    health_server_connection_loop(Arc::new(|_, _, _| {}), peer_stops, stream).await;
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
