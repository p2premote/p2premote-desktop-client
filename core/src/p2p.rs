use crate::auth::refresh_with_config;
use crate::config::MachineConfig;
use crate::health::HEALTH_PORT;
use crate::http::{shared_client, ApiResponse};
use crate::speed_test::{
    build_speed_test_client, extract_download_result, extract_upload_result,
    TunnelSpeedTestCommand, TunnelSpeedTestResult, SPEED_TEST_PORT, SPEED_TEST_RUN_TIMEOUT_SECS,
};
use crate::tunnel_control::{
    now_millis, TunnelControlConnection, TunnelControlMessage, TUNNEL_CONTROL_PROTOCOL_VERSION,
};
use anyhow::{anyhow, Context, Result};
use parking_lot::Mutex;
use rand::Rng;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::sync::Arc;
use std::time::{SystemTime, UNIX_EPOCH};
use tokio::net::TcpStream;
use tokio::sync::{mpsc, watch};
use tracing::{info, warn};

#[derive(Debug, Clone, Default)]
pub struct PassivePeerInfo {
    pub source_user_id: i64,
    pub source_username: String,
    pub source_email: String,
    pub source_device_name: String,
    pub source_device_alias: String,
    pub source_public_ip: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ActiveStartResult {
    pub success: bool,
    pub reused: bool,
    pub local_port: u16,
    pub rdp_address: String,
    pub message: String,
    #[serde(default)]
    pub warning: Option<String>,
}

#[derive(Debug, Clone)]
pub struct ActiveP2POpenResult {
    pub connection_id: String,
    pub log_id: i64,
    pub access_grant: String,
    pub target_rdp_port: u16,
    pub source_user_id: i64,
    pub source_username: String,
    pub source_email: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TunnelHealthEvent {
    HeartbeatSucceeded { latency_ms: Option<u32> },
    HeartbeatFailed,
    ConnectionClosed,
}

pub type TunnelHealthEventHandler = Arc<dyn Fn(TunnelHealthEvent) + Send + Sync>;

#[derive(Serialize)]
struct P2POpenRequest {
    client_job_id: String,
    source_device_id: i64,
    source_device_uuid: String,
    target_device_id: i64,
    target_device_uuid: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    connect_code: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    temporary_password: Option<String>,
}

type P2POpenResponse = ApiResponse<Option<P2POpenData>>;

#[derive(Deserialize)]
struct P2POpenData {
    connection_id: String,
    log_id: i64,
    access_grant: String,
    #[serde(default)]
    target: Option<P2POpenTarget>,
    #[serde(default)]
    source_user_id: i64,
    #[serde(default)]
    source_username: String,
    #[serde(default)]
    source_email: String,
}

#[derive(Deserialize)]
struct P2POpenTarget {
    #[serde(default)]
    service_port: u16,
}
#[derive(Serialize)]
struct NotifyP2PEndRequest {
    connection_id: String,
    log_id: i64,
    success: bool,
    source_device_id: i64,
    target_device_id: i64,
    source_nat_type: String,
    target_nat_type: String,
    error_code: String,
    error_message: String,
}
/// token 过期前的刷新阈值（秒）
const TOKEN_REFRESH_THRESHOLD_SECS: i64 = 300;

fn token_expires_soon(config: &MachineConfig, threshold_secs: i64) -> bool {
    let Some(expires_at) = config.access_token_expires_at else {
        return true;
    };
    let now = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0);
    expires_at < now + threshold_secs
}

async fn ensure_fresh_token(config: &mut MachineConfig) {
    if !token_expires_soon(config, TOKEN_REFRESH_THRESHOLD_SECS) {
        return;
    }
    match refresh_with_config(config).await {
        Ok(_) => info!("[p2p] token refreshed before p2p/start"),
        Err(err) => warn!(
            "[p2p] token refresh failed (will try with existing token): {:#}",
            err
        ),
    }
}
pub async fn open_active_p2p_job(
    config: &MachineConfig,
    client_job_id: String,
    target_device_id: i64,
    target_device_uuid: String,
    connect_code: Option<String>,
    temporary_password: Option<String>,
) -> Result<ActiveP2POpenResult> {
    let mut config = config.clone();
    ensure_fresh_token(&mut config).await;
    let token = config
        .auth_token
        .clone()
        .ok_or_else(|| anyhow!("not logged in"))?;
    let source_device_id = config
        .device_id
        .ok_or_else(|| anyhow!("device not registered"))?;
    let source_device_uuid = config
        .device_uuid
        .clone()
        .ok_or_else(|| anyhow!("device not registered"))?;
    let url = format!(
        "{}/api/v1/p2p/open",
        config.server_url.trim_end_matches('/')
    );
    let response = shared_client()
        .post(url)
        .header("Authorization", format!("Bearer {}", token))
        .json(&P2POpenRequest {
            client_job_id,
            source_device_id,
            source_device_uuid,
            target_device_id,
            target_device_uuid,
            connect_code,
            temporary_password,
        })
        .send()
        .await
        .context("p2p open request failed")?;
    let response: P2POpenResponse = response.json().await.context("invalid p2p open response")?;
    if response.code != 0 {
        return Err(anyhow!(
            response.localized_error_message(config.locale.as_deref())
        ));
    }
    let data = response
        .data
        .ok_or_else(|| anyhow!("p2p open response missing data"))?;
    info!(
        "[p2p] p2p/open source identity: user_id={}, username_present={}, email_present={}",
        data.source_user_id,
        !data.source_username.trim().is_empty(),
        !data.source_email.trim().is_empty(),
    );
    Ok(ActiveP2POpenResult {
        connection_id: data.connection_id,
        log_id: data.log_id,
        access_grant: data.access_grant,
        target_rdp_port: data
            .target
            .map(|target| target.service_port)
            .unwrap_or(3389),
        source_user_id: data.source_user_id,
        source_username: data.source_username,
        source_email: data.source_email,
    })
}
pub fn build_punch_token(prefix: &str) -> String {
    let mut rng = rand::thread_rng();
    let suffix: String = (0..8)
        .map(|_| (b'a' + rng.gen_range(0..26)) as char)
        .collect();
    format!("{}-{}", prefix, suffix)
}

pub async fn wgvpn_health_monitor_loop(
    source_device_id: i64,
    health_addr: String,
    mut stop_rx: watch::Receiver<bool>,
    mut speed_rx: mpsc::UnboundedReceiver<TunnelSpeedTestCommand>,
    speed_test_busy: Arc<std::sync::atomic::AtomicBool>,
    event_handler: TunnelHealthEventHandler,
) -> Result<()> {
    let last_success = Arc::new(std::sync::Mutex::new(std::time::Instant::now()));
    let session_last_success = last_success.clone();
    let session_event_handler = event_handler.clone();
    let session_handler: TunnelHealthEventHandler = Arc::new(move |event| {
        if matches!(event, TunnelHealthEvent::HeartbeatSucceeded { .. }) {
            *session_last_success.lock().expect("health timestamp lock") =
                std::time::Instant::now();
        }
        session_event_handler(event);
    });

    let monitor = async {
        loop {
            if *stop_rx.borrow() {
                return Ok(());
            }
            match active_health_session(
                source_device_id,
                &health_addr,
                &mut stop_rx,
                &mut speed_rx,
                &speed_test_busy,
                Some(&session_handler),
            )
            .await
            {
                Ok(()) => return Ok(()),
                Err(err) => warn!(
                    "[wgvpn-health] health session failed, retrying: address={}, error={:#}",
                    health_addr, err
                ),
            }
            tokio::select! {
                _ = stop_rx.changed() => {
                    if *stop_rx.borrow() {
                        return Ok(());
                    }
                }
                _ = tokio::time::sleep(std::time::Duration::from_secs(5)) => {}
            }
        }
    };

    let watchdog = async {
        let mut interval = tokio::time::interval(std::time::Duration::from_secs(5));
        interval.tick().await;
        loop {
            interval.tick().await;
            let elapsed = last_success
                .lock()
                .expect("health timestamp lock")
                .elapsed();
            if elapsed >= std::time::Duration::from_secs(5) {
                event_handler(TunnelHealthEvent::HeartbeatFailed);
            }
        }
    };

    tokio::select! {
        result = monitor => result,
        _ = watchdog => unreachable!("health watchdog does not terminate"),
    }
}

async fn active_health_session(
    source_device_id: i64,
    health_addr: &str,
    stop_rx: &mut watch::Receiver<bool>,
    speed_rx: &mut mpsc::UnboundedReceiver<TunnelSpeedTestCommand>,
    speed_test_busy: &Arc<std::sync::atomic::AtomicBool>,
    event_handler: Option<&TunnelHealthEventHandler>,
) -> Result<()> {
    let stream = tokio::time::timeout(
        std::time::Duration::from_secs(10),
        TcpStream::connect(health_addr),
    )
    .await
    .context("health connect timeout")?
    .with_context(|| format!("failed to connect remote health: {}", health_addr))?;

    let mut conn = TunnelControlConnection::new(stream);
    conn.send(&TunnelControlMessage::Hello {
        source_device_id,
        protocol_version: TUNNEL_CONTROL_PROTOCOL_VERSION,
    })
    .await
    .context("failed to send health hello")?;

    let ack = tokio::time::timeout(std::time::Duration::from_secs(5), conn.next())
        .await
        .context("health hello ack timeout")?
        .context("failed to read health hello ack")?;
    match ack {
        Some(TunnelControlMessage::HelloAck {
            ok: true,
            protocol_version,
            ..
        }) if protocol_version == TUNNEL_CONTROL_PROTOCOL_VERSION => {}
        Some(other) => return Err(anyhow!("unexpected health hello ack: {:?}", other)),
        None => {
            if let Some(handler) = event_handler {
                handler(TunnelHealthEvent::ConnectionClosed);
                return Ok(());
            }
            return Err(anyhow!("remote health closed before hello ack"));
        }
    };

    let mut reported_rtt_ms = None;
    let peer_virtual_ip = health_addr
        .rsplit_once(':')
        .map(|(ip, _)| ip)
        .unwrap_or(health_addr);
    loop {
        tokio::select! {
            _ = stop_rx.changed() => {
                if *stop_rx.borrow() {
                    return Ok(());
                }
            }
            request = speed_rx.recv() => {
                if let Some(request) = request {
                    // 本分支串行占用健康连接约 15-20s；健康阈值约 60s，且
                    // 共享 busy 标志会阻止本地与远端请求并发堆积。
                    let result = run_speed_test_as_client(&mut conn, peer_virtual_ip).await.map_err(|e| e.to_string());
                    speed_test_busy.store(false, std::sync::atomic::Ordering::SeqCst);
                    let _ = request.response.send(result);
                    if let Some(handler) = event_handler {
                        handler(TunnelHealthEvent::HeartbeatSucceeded { latency_ms: None });
                    }
                }
            }
            message = conn.next() => {
                match message.context("failed to read health control message")? {
                    Some(TunnelControlMessage::SpeedTestRequest { request_id }) => {
                        execute_requested_speed_test(
                            &mut conn,
                            peer_virtual_ip,
                            request_id,
                            speed_test_busy,
                        ).await?;
                        if let Some(handler) = event_handler {
                            handler(TunnelHealthEvent::HeartbeatSucceeded { latency_ms: None });
                        }
                    }
                    Some(other) => {
                        return Err(anyhow!("unexpected health control message: {:?}", other));
                    }
                    None => {
                        if let Some(handler) = event_handler {
                            handler(TunnelHealthEvent::ConnectionClosed);
                            return Ok(());
                        }
                        return Err(anyhow!("remote health closed"));
                    }
                }
            }
            _ = tokio::time::sleep(std::time::Duration::from_secs(5)) => {
                let ts = now_millis();
                let started = std::time::Instant::now();
                conn.send(&TunnelControlMessage::Ping { ts, reported_rtt_ms })
                    .await
                    .context("failed to send health ping")?;
                wait_for_health_pong(
                    &mut conn,
                    peer_virtual_ip,
                    ts,
                    speed_test_busy,
                ).await?;
                let raw_rtt_ms = (started.elapsed().as_secs_f64() * 1000.0)
                    .round()
                    .clamp(0.0, u32::MAX as f64) as u32;
                let latency_ms = raw_rtt_ms;
                reported_rtt_ms = Some(latency_ms);
                if let Some(handler) = event_handler {
                    handler(TunnelHealthEvent::HeartbeatSucceeded {
                        latency_ms: Some(latency_ms),
                    });
                }
            }
        }
    }
}

async fn wait_for_health_pong(
    conn: &mut TunnelControlConnection<TcpStream>,
    peer_virtual_ip: &str,
    expected_ts: i64,
    speed_test_busy: &Arc<std::sync::atomic::AtomicBool>,
) -> Result<()> {
    let mut deferred_speed_request = None;
    loop {
        let response = tokio::time::timeout(std::time::Duration::from_secs(5), conn.next())
            .await
            .context("health pong timeout")?
            .context("failed to read health pong")?;
        match response {
            Some(TunnelControlMessage::Pong { ts }) if ts == expected_ts => break,
            Some(TunnelControlMessage::SpeedTestRequest { request_id }) => {
                if deferred_speed_request.is_some() {
                    conn.send(&TunnelControlMessage::SpeedTestError {
                        request_id,
                        message: "speed test already in progress".to_string(),
                    })
                    .await?;
                } else {
                    deferred_speed_request = Some(request_id);
                }
            }
            Some(other) => return Err(anyhow!("unexpected health response: {:?}", other)),
            None => return Err(anyhow!("remote health closed")),
        }
    }
    if let Some(request_id) = deferred_speed_request {
        execute_requested_speed_test(conn, peer_virtual_ip, request_id, speed_test_busy).await?;
    }
    Ok(())
}

async fn execute_requested_speed_test(
    conn: &mut TunnelControlConnection<TcpStream>,
    peer_virtual_ip: &str,
    request_id: u64,
    speed_test_busy: &Arc<std::sync::atomic::AtomicBool>,
) -> Result<()> {
    if speed_test_busy.swap(true, std::sync::atomic::Ordering::SeqCst) {
        return conn
            .send(&TunnelControlMessage::SpeedTestError {
                request_id,
                message: "speed test already in progress".to_string(),
            })
            .await;
    }
    let message = match run_speed_test_as_client(conn, peer_virtual_ip).await {
        Ok(result) => TunnelControlMessage::SpeedTestResult {
            request_id,
            latency_ms: result.latency_ms,
            download_mbps: result.download_mbps,
            upload_mbps: result.upload_mbps,
            retransmits: result.retransmits,
        },
        Err(err) => TunnelControlMessage::SpeedTestError {
            request_id,
            message: err.to_string(),
        },
    };
    speed_test_busy.store(false, std::sync::atomic::Ordering::SeqCst);
    conn.send(&message)
        .await
        .context("failed to return requested speed test result")
}

/// 测速发起端的 client 流程：
/// 1. 通过控制 TCP 发 `SpeedStart`，让对端启动 riperf3 server；
/// 2. 等 `SpeedReady` 回执（对端 server 已 listen，避免 connect 撞空）；
/// 3. 5 次 `SpeedPing`/`SpeedPong` 往返测延迟（riperf3 不提供独立延迟测量）；
/// 4. 作为 riperf3 client 连 `peer_virtual_ip:SPEED_TEST_PORT`，依次跑上传和下载
///    两次独立 TCP 测试，每次 6 秒；
/// 5. 从两份 client `Report` 提取各方向结果。
///
/// 测速始终由主动端执行；被动端点击时通过健康连接下发请求。
async fn run_speed_test_as_client(
    conn: &mut TunnelControlConnection<TcpStream>,
    peer_virtual_ip: &str,
) -> Result<TunnelSpeedTestResult> {
    // ① 测延迟；上传、下载测试各自启动一个 one-off riperf3 server。
    let latency_ms = measure_speed_latency(conn).await?;
    let peer_addr: std::net::SocketAddr = format!("{peer_virtual_ip}:{SPEED_TEST_PORT}")
        .parse()
        .with_context(|| format!("invalid peer addr: {peer_virtual_ip}:{SPEED_TEST_PORT}"))?;

    let upload_report = run_speed_test_direction(conn, peer_addr, false).await?;
    let (upload_mbps, retransmits) = extract_upload_result(&upload_report);
    let download_report = run_speed_test_direction(conn, peer_addr, true).await?;
    let download_mbps = extract_download_result(&download_report);

    Ok(TunnelSpeedTestResult {
        latency_ms,
        download_mbps,
        upload_mbps,
        retransmits,
    })
}

async fn run_speed_test_direction(
    conn: &mut TunnelControlConnection<TcpStream>,
    peer_addr: std::net::SocketAddr,
    reverse: bool,
) -> Result<riperf3::Report> {
    // 通知对端启动本方向的 one-off riperf3 server。
    conn.send(&TunnelControlMessage::SpeedStart)
        .await
        .context("failed to send SpeedStart")?;

    // ② 等 SpeedReady（对端 server 已 listen）。riperf3 client 连接失败不重试，
    //    必须等此回执后才能发起 client。
    match next_speed_response(conn, std::time::Duration::from_secs(10)).await? {
        Some(TunnelControlMessage::SpeedReady) => {}
        other => return Err(anyhow!("expected SpeedReady, got {:?}", other)),
    }

    // 不自动重试：任何连接、协议或数据传输错误都保留第一次失败原因并直接返回。
    let client = build_speed_test_client(peer_addr, reverse)?;
    tokio::time::timeout(
        std::time::Duration::from_secs(SPEED_TEST_RUN_TIMEOUT_SECS),
        client.run(),
    )
    .await
    .context("riperf3 client run timed out")?
    .context("riperf3 client run failed")
}

async fn measure_speed_latency(conn: &mut TunnelControlConnection<TcpStream>) -> Result<f64> {
    // 5 次延迟探测，取平均往返时延（RTT）。
    let mut latency = std::time::Duration::ZERO;
    for _ in 0..5 {
        let nonce = rand::thread_rng().gen::<u64>();
        let started = std::time::Instant::now();
        conn.send(&TunnelControlMessage::SpeedPing { nonce })
            .await
            .context("failed to send speed ping")?;
        match next_speed_response(conn, std::time::Duration::from_secs(3)).await? {
            Some(TunnelControlMessage::SpeedPong { nonce: value }) if value == nonce => {}
            other => return Err(anyhow!("unexpected speed ping response: {:?}", other)),
        }
        latency += started.elapsed();
    }

    // latency 是 5 次 RTT 的总和；× 1000 转毫秒 / 5 得平均 RTT。
    Ok(latency.as_secs_f64() * 1000.0 / 5.0)
}

/// 测速过程中若对端又请求一次测速，立即返回 busy，随后继续等待当前操作的响应。
async fn next_speed_response(
    conn: &mut TunnelControlConnection<TcpStream>,
    timeout: std::time::Duration,
) -> Result<Option<TunnelControlMessage>> {
    loop {
        let message = tokio::time::timeout(timeout, conn.next())
            .await
            .context("speed control response timeout")?
            .context("speed control response read failed")?;
        match message {
            Some(TunnelControlMessage::SpeedTestRequest { request_id }) => {
                conn.send(&TunnelControlMessage::SpeedTestError {
                    request_id,
                    message: "speed test already in progress".to_string(),
                })
                .await?;
            }
            other => return Ok(other),
        }
    }
}

async fn notify_remote_tunnel_stop(source_device_id: i64, health_addr: &str) -> Result<()> {
    let stream = tokio::time::timeout(
        std::time::Duration::from_secs(3),
        TcpStream::connect(health_addr),
    )
    .await
    .context("remote stop health connect timeout")?
    .with_context(|| format!("failed to connect remote health: {}", health_addr))?;

    let mut conn = TunnelControlConnection::new(stream);
    conn.send(&TunnelControlMessage::Hello {
        source_device_id,
        protocol_version: TUNNEL_CONTROL_PROTOCOL_VERSION,
    })
    .await
    .context("failed to send remote stop hello")?;

    let ack = tokio::time::timeout(std::time::Duration::from_secs(3), conn.next())
        .await
        .context("remote stop hello ack timeout")?
        .context("failed to read remote stop hello ack")?;
    match ack {
        Some(TunnelControlMessage::HelloAck {
            ok: true,
            protocol_version,
            ..
        }) if protocol_version == TUNNEL_CONTROL_PROTOCOL_VERSION => {}
        Some(other) => return Err(anyhow!("unexpected remote stop hello ack: {:?}", other)),
        None => return Err(anyhow!("remote health closed before stop ack")),
    };

    conn.send(&TunnelControlMessage::Stop {
        reason: "initiator_closed".to_string(),
    })
    .await
    .context("failed to send remote stop")?;
    Ok(())
}

async fn notify_p2p_end(
    config: &MachineConfig,
    connection_id: String,
    log_id: i64,
    success: bool,
    source_device_id: i64,
    target_device_id: i64,
    source_nat_type: String,
    target_nat_type: String,
    error_code: String,
    error_message: String,
) -> Result<()> {
    let token = config
        .auth_token
        .clone()
        .ok_or_else(|| anyhow!("not logged in"))?;
    let url = format!("{}/api/v1/p2p/end", config.server_url.trim_end_matches('/'));
    let client = shared_client();
    let response = client
        .post(url)
        .header("Authorization", format!("Bearer {}", token))
        .json(&NotifyP2PEndRequest {
            connection_id,
            log_id,
            success,
            source_device_id,
            target_device_id,
            source_nat_type,
            target_nat_type,
            error_code,
            error_message,
        })
        .send()
        .await
        .context("p2p end request failed")?;

    if !response.status().is_success() {
        return Err(anyhow!("p2p end returned status {}", response.status()));
    }
    Ok(())
}

pub async fn close_active_p2p_job(
    config: &MachineConfig,
    opened: &ActiveP2POpenResult,
    target_device_id: i64,
    success: bool,
    error_code: String,
    error_message: String,
) -> Result<()> {
    let source_device_id = config
        .device_id
        .ok_or_else(|| anyhow!("device not registered"))?;
    notify_p2p_end(
        config,
        opened.connection_id.clone(),
        opened.log_id,
        success,
        source_device_id,
        target_device_id,
        String::new(),
        String::new(),
        error_code,
        error_message,
    )
    .await
}

#[cfg(test)]
mod tests {
    use super::*;
    use tokio::net::TcpListener;

    #[tokio::test]
    async fn active_health_session_sends_hello_and_ping() {
        let listener = TcpListener::bind("127.0.0.1:0")
            .await
            .expect("bind fake health server");
        let addr = listener.local_addr().expect("health addr");
        let server = tokio::spawn(async move {
            let (stream, _) = listener.accept().await.expect("accept health client");
            let mut conn = TunnelControlConnection::new(stream);
            assert_eq!(
                conn.next().await.expect("read hello"),
                Some(TunnelControlMessage::Hello {
                    source_device_id: 42,
                    protocol_version: TUNNEL_CONTROL_PROTOCOL_VERSION,
                })
            );
            conn.send(&TunnelControlMessage::HelloAck {
                ok: true,
                protocol_version: TUNNEL_CONTROL_PROTOCOL_VERSION,
                message: "ok".to_string(),
            })
            .await
            .expect("send hello ack");
            let ping = conn.next().await.expect("read ping");
            let ts = match ping {
                Some(TunnelControlMessage::Ping {
                    ts,
                    reported_rtt_ms: None,
                }) => ts,
                other => panic!("expected ping, got {:?}", other),
            };
            conn.send(&TunnelControlMessage::Pong { ts })
                .await
                .expect("send pong");
        });

        let (stop_tx, mut stop_rx) = watch::channel(false);
        let (_speed_tx, mut speed_rx) = mpsc::unbounded_channel();
        let speed_test_busy = Arc::new(std::sync::atomic::AtomicBool::new(false));
        let session = tokio::spawn(async move {
            active_health_session(
                42,
                &addr.to_string(),
                &mut stop_rx,
                &mut speed_rx,
                &speed_test_busy,
                None,
            )
            .await
        });
        server.await.expect("server task");
        stop_tx.send(true).expect("stop active health session");
        if let Err(err) = session.await.expect("session task") {
            assert!(err.to_string().contains("remote health closed"));
        }
    }

    #[tokio::test]
    async fn active_health_session_defers_speed_request_until_pong_then_returns_error() {
        let listener = TcpListener::bind("127.0.0.1:0")
            .await
            .expect("bind fake health server");
        let addr = listener.local_addr().expect("health addr");
        let server = tokio::spawn(async move {
            let (stream, _) = listener.accept().await.expect("accept health client");
            let mut conn = TunnelControlConnection::new(stream);
            assert!(matches!(
                conn.next().await.expect("read hello"),
                Some(TunnelControlMessage::Hello { .. })
            ));
            conn.send(&TunnelControlMessage::HelloAck {
                ok: true,
                protocol_version: TUNNEL_CONTROL_PROTOCOL_VERSION,
                message: "ok".to_string(),
            })
            .await
            .expect("send hello ack");

            let ts = match conn.next().await.expect("read ping") {
                Some(TunnelControlMessage::Ping { ts, .. }) => ts,
                other => panic!("expected ping, got {:?}", other),
            };
            // 请求先于 Pong 到达，主动端必须先消费 Pong，再开始 SpeedPing。
            conn.send(&TunnelControlMessage::SpeedTestRequest { request_id: 9 })
                .await
                .expect("send speed request");
            conn.send(&TunnelControlMessage::Pong { ts })
                .await
                .expect("send pong");
            for _ in 0..5 {
                let nonce = match conn.next().await.expect("read speed ping") {
                    Some(TunnelControlMessage::SpeedPing { nonce }) => nonce,
                    other => panic!("expected speed ping, got {:?}", other),
                };
                conn.send(&TunnelControlMessage::SpeedPong { nonce })
                    .await
                    .expect("send speed pong");
            }
            assert_eq!(
                conn.next().await.expect("read speed start"),
                Some(TunnelControlMessage::SpeedStart)
            );
            conn.send(&TunnelControlMessage::Stop {
                reason: "force test error".to_string(),
            })
            .await
            .expect("send unexpected response");
            assert!(matches!(
                conn.next().await.expect("read returned speed error"),
                Some(TunnelControlMessage::SpeedTestError { request_id: 9, .. })
            ));
        });

        let (_stop_tx, mut stop_rx) = watch::channel(false);
        let (_speed_tx, mut speed_rx) = mpsc::unbounded_channel();
        let speed_test_busy = Arc::new(std::sync::atomic::AtomicBool::new(false));
        let session = tokio::spawn(async move {
            active_health_session(
                42,
                &addr.to_string(),
                &mut stop_rx,
                &mut speed_rx,
                &speed_test_busy,
                None,
            )
            .await
        });
        server.await.expect("server task");
        assert!(session.await.expect("session task").is_err());
    }

    #[tokio::test]
    async fn active_health_session_rejects_remote_request_while_local_speed_is_busy() {
        let listener = TcpListener::bind("127.0.0.1:0")
            .await
            .expect("bind fake health server");
        let addr = listener.local_addr().expect("health addr");
        let server = tokio::spawn(async move {
            let (stream, _) = listener.accept().await.expect("accept health client");
            let mut conn = TunnelControlConnection::new(stream);
            assert!(matches!(
                conn.next().await.expect("read hello"),
                Some(TunnelControlMessage::Hello { .. })
            ));
            conn.send(&TunnelControlMessage::HelloAck {
                ok: true,
                protocol_version: TUNNEL_CONTROL_PROTOCOL_VERSION,
                message: "ok".to_string(),
            })
            .await
            .expect("send hello ack");
            conn.send(&TunnelControlMessage::SpeedTestRequest { request_id: 10 })
                .await
                .expect("send speed request");
            assert_eq!(
                conn.next().await.expect("read busy response"),
                Some(TunnelControlMessage::SpeedTestError {
                    request_id: 10,
                    message: "speed test already in progress".to_string(),
                })
            );
        });

        let (_stop_tx, mut stop_rx) = watch::channel(false);
        let (_speed_tx, mut speed_rx) = mpsc::unbounded_channel();
        let speed_test_busy = Arc::new(std::sync::atomic::AtomicBool::new(true));
        let session_busy = speed_test_busy.clone();
        let session = tokio::spawn(async move {
            active_health_session(
                42,
                &addr.to_string(),
                &mut stop_rx,
                &mut speed_rx,
                &session_busy,
                None,
            )
            .await
        });
        server.await.expect("server task");
        assert!(session.await.expect("session task").is_err());
        assert!(speed_test_busy.load(std::sync::atomic::Ordering::SeqCst));
    }

    #[tokio::test]
    async fn notify_remote_tunnel_stop_sends_stop_message() {
        let listener = TcpListener::bind("127.0.0.1:0")
            .await
            .expect("bind fake health server");
        let addr = listener.local_addr().expect("health addr");
        let server = tokio::spawn(async move {
            let (stream, _) = listener.accept().await.expect("accept health client");
            let mut conn = TunnelControlConnection::new(stream);
            assert_eq!(
                conn.next().await.expect("read hello"),
                Some(TunnelControlMessage::Hello {
                    source_device_id: 77,
                    protocol_version: TUNNEL_CONTROL_PROTOCOL_VERSION,
                })
            );
            conn.send(&TunnelControlMessage::HelloAck {
                ok: true,
                protocol_version: TUNNEL_CONTROL_PROTOCOL_VERSION,
                message: "ok".to_string(),
            })
            .await
            .expect("send hello ack");
            assert_eq!(
                conn.next().await.expect("read stop"),
                Some(TunnelControlMessage::Stop {
                    reason: "initiator_closed".to_string(),
                })
            );
        });

        notify_remote_tunnel_stop(77, &addr.to_string())
            .await
            .expect("notify remote stop");
        server.await.expect("server task");
    }
}

// ============ wgvpn 模块 ============
//
// gonc 负责外层加密 P2P UDP 数据面，WireGuard 负责内层 VPN。
// 服务端不新增 API，客户端通过 gonc FFI 交换 WireGuard 公钥/IP 并建立本地 UDP tunnel。

pub mod wgvpn_flow;
