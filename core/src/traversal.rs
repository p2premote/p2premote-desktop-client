//! One application-owned protocol round at a time, over authorized WebSocket.
use crate::{gonc_ffi::{self, UdpTunnelRequest, UdpTunnelResult}, traversal_policy::{self, NatEvidence, Negotiation}, ws::ServiceWsClient};
use anyhow::{anyhow, Result};
use once_cell::sync::Lazy;
use parking_lot::Mutex;
use serde::{Deserialize, Serialize};
use std::{collections::HashMap, sync::{Arc, atomic::{AtomicBool, Ordering}}, time::Duration};
use tokio::sync::{mpsc, Notify};

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum Frame {
    Capabilities { evidence: Vec<NatEvidence> },
    Plan { networks: Vec<String> },
    PlanAck { networks: Vec<String> },
    Prepare { round: u8, network: String, mode: String, token: String, timeout_secs: u64 },
    Ready { round: u8 },
    Result { round: u8, ok: bool },
    Commit { round: u8 },
    Committed { round: u8 },
    Close { round: u8 },
    Closed { round: u8 },
}

impl Frame {
    fn round(&self) -> u8 {
        match self {
            Self::Prepare { round, .. } | Self::Ready { round } | Self::Result { round, .. }
            | Self::Commit { round } | Self::Committed { round } | Self::Close { round } | Self::Closed { round } => *round,
            _ => 0,
        }
    }
    fn stage(&self) -> u8 {
        match self {
            Self::Capabilities { .. } => 0, Self::Plan { .. } => 1, Self::PlanAck { .. } => 2,
            Self::Prepare { .. } => 3, Self::Ready { .. } => 4, Self::Result { .. } => 5,
            Self::Commit { .. } | Self::Close { .. } => 6,
            Self::Committed { .. } | Self::Closed { .. } => 7,
        }
    }
}

static SESSIONS: Lazy<Mutex<HashMap<String, Arc<Session>>>> = Lazy::new(|| Mutex::new(HashMap::new()));

pub struct Session {
    attempt_id: String,
    connection_id: String,
    peer: i64,
    grant: String,
    ws: ServiceWsClient,
    active: bool,
    negotiation: Negotiation,
    tx: mpsc::Sender<Frame>,
    rx: tokio::sync::Mutex<mpsc::Receiver<Frame>>,
    cancelled: AtomicBool,
    wake: Notify,
}

pub fn register(token: String, attempt_id: String, connection_id: String, peer: i64,
    grant: String, ws: ServiceWsClient, active: bool, negotiation: Negotiation) -> Result<()> {
    negotiation.validate()?;
    let (tx, rx) = mpsc::channel(64);
    let mut sessions = SESSIONS.lock();
    if sessions.values().any(|s| s.attempt_id == attempt_id && s.connection_id == connection_id) {
        return Err(anyhow!("duplicate_traversal_attempt"));
    }
    sessions.insert(token, Arc::new(Session { attempt_id, connection_id, peer, grant, ws, active,
        negotiation, tx, rx: tokio::sync::Mutex::new(rx), cancelled: AtomicBool::new(false), wake: Notify::new() }));
    Ok(())
}

pub fn lookup(token: &str) -> Option<Arc<Session>> { SESSIONS.lock().get(token).cloned() }
pub fn remove(token: &str) { SESSIONS.lock().remove(token); }
pub fn cancel(attempt: &str) {
    for session in SESSIONS.lock().values().filter(|s| s.attempt_id == attempt) {
        session.cancelled.store(true, Ordering::Release);
        session.wake.notify_one();
    }
}
pub fn deliver(connection: &str, peer: i64, attempt: &str, frame: Frame) -> Result<()> {
    let sessions = SESSIONS.lock();
    let session = sessions.values().find(|s| s.connection_id == connection && s.peer == peer && s.attempt_id == attempt)
        .ok_or_else(|| anyhow!("stale_traversal_frame"))?;
    session.tx.try_send(frame).map_err(|_| anyhow!("traversal_inbox_full"))
}

/// Own registry cleanup even when key exchange or platform setup fails.
pub struct Registration(pub String);
impl Drop for Registration { fn drop(&mut self) { remove(&self.0); } }

struct PendingTunnel(Option<UdpTunnelResult>);
impl Drop for PendingTunnel {
    fn drop(&mut self) {
        if let Some(t) = &self.0 { gonc_ffi::stop_udp_tunnel_native(&t.handle_id); }
    }
}

impl Session {
    fn check(&self) -> Result<()> {
        if self.cancelled.load(Ordering::Acquire) { Err(anyhow!("traversal_cancelled")) } else { Ok(()) }
    }
    async fn send(&self, frame: Frame) -> Result<()> {
        self.check()?;
        self.ws.send_p2p_notify(self.connection_id.clone(), self.peer, uuid::Uuid::new_v4().to_string(),
            self.grant.clone(), serde_json::json!({"type":"traversal", "attempt_id":self.attempt_id, "frame":frame}).to_string())
            .await.map_err(|e| anyhow!("traversal_signal_failed: {e}"))
    }
    async fn wait(&self, round: u8, stage: u8) -> Result<Frame> {
        let mut rx = self.rx.lock().await;
        tokio::time::timeout(Duration::from_secs(12), async {
            loop {
                self.check()?;
                let frame = tokio::select! {
                    f = rx.recv() => f.ok_or_else(|| anyhow!("traversal_signal_closed"))?,
                    _ = self.wake.notified() => { self.check()?; continue; }
                };
                if frame.round() < round || (frame.round() == round && frame.stage() < stage) { continue; }
                if frame.round() != round || frame.stage() != stage { return Err(anyhow!("invalid_traversal_sequence")); }
                return Ok(frame);
            }
        }).await.map_err(|_| anyhow!("traversal_signal_timeout"))?
    }

    pub async fn start(&self, template: &UdpTunnelRequest) -> Result<UdpTunnelResult> {
        self.check()?;
        let evidence = traversal_policy::collect_evidence().await;
        self.send(Frame::Capabilities { evidence: evidence.clone() }).await?;
        let remote = match self.wait(0, 0).await? {
            Frame::Capabilities { evidence } if evidence.len() <= 32 => evidence,
            _ => return Err(anyhow!("invalid_traversal_capabilities")),
        };
        let networks = traversal_policy::build_plan(self.negotiation.preferences, &evidence, &remote);
        if self.active {
            self.send(Frame::Plan { networks: networks.clone() }).await?;
            match self.wait(0, 2).await? {
                Frame::PlanAck { networks: accepted } if accepted == networks => {},
                _ => return Err(anyhow!("traversal_plan_mismatch")),
            }
        } else {
            match self.wait(0, 1).await? {
                Frame::Plan { networks: offered } if offered == networks => {},
                _ => return Err(anyhow!("traversal_plan_mismatch")),
            }
            self.send(Frame::PlanAck { networks: networks.clone() }).await?;
        }
        // LAN eligibility is independent of public NAT classification.
        let lan = if self.negotiation.preferences.prefer_tcp { "tcp4" } else { "udp4" };
        let rounds = std::iter::once((lan.to_string(), "lan", 6u64)).chain(networks.into_iter()
            .map(|n| { let seconds = if n.starts_with("tcp") { 10 } else { 30 }; (n, "internet", seconds) }));
        for (index, (network, mode, timeout_secs)) in rounds.enumerate() {
            self.check()?;
            let round = (index + 1) as u8;
            let mut req = template.clone();
            req.network = network.clone(); req.traversal_mode = mode.into(); req.timeout_secs = timeout_secs;
            if self.active {
                req.token = uuid::Uuid::new_v4().to_string();
                self.send(Frame::Prepare { round, network, mode: mode.into(), token: req.token.clone(), timeout_secs }).await?;
                if !matches!(self.wait(round, 4).await?, Frame::Ready { .. }) { return Err(anyhow!("invalid_traversal_ready")); }
            } else {
                match self.wait(round, 3).await? {
                    Frame::Prepare { network: n, mode: m, token, timeout_secs: seconds, .. }
                        if n == network && m == mode && seconds == timeout_secs && token.len() >= 16 && token.len() <= 128 => req.token = token,
                    _ => return Err(anyhow!("invalid_traversal_round")),
                }
                self.send(Frame::Ready { round }).await?;
            }
            // Never abandon this call: cancellation must drain a late handle.
            let result = gonc_ffi::start_udp_tunnel_native(&req).await;
            let mut owned = PendingTunnel(result.ok());
            self.check()?;
            self.send(Frame::Result { round, ok: owned.0.is_some() }).await?;
            let peer_ok = match self.wait(round, 5).await? {
                Frame::Result { ok, .. } => ok,
                _ => return Err(anyhow!("invalid_traversal_result")),
            };
            let success = peer_ok && owned.0.is_some();
            if self.active {
                self.send(if success { Frame::Commit { round } } else { Frame::Close { round } }).await?;
                let ack = self.wait(round, 7).await?;
                if success && matches!(ack, Frame::Committed { .. }) { return Ok(owned.0.take().unwrap()); }
                if success || !matches!(ack, Frame::Closed { .. }) { return Err(anyhow!("invalid_traversal_commit")); }
            } else {
                let decision = self.wait(round, 6).await?;
                if success && matches!(decision, Frame::Commit { .. }) {
                    self.send(Frame::Committed { round }).await?;
                    return Ok(owned.0.take().unwrap());
                }
                if !matches!(decision, Frame::Close { .. }) { return Err(anyhow!("invalid_traversal_commit")); }
                drop(owned);
                self.send(Frame::Closed { round }).await?;
                continue;
            }
            drop(owned);
        }
        Err(anyhow!("punch_exhausted"))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    fn session() -> Session {
        let (tx, rx) = mpsc::channel(64);
        Session { attempt_id: "test".into(), connection_id: "authorized".into(), peer: 7,
            grant: String::new(), ws: ServiceWsClient::new(), active: true,
            negotiation: Negotiation { version: 2, preferences: Default::default() },
            tx, rx: tokio::sync::Mutex::new(rx), cancelled: AtomicBool::new(false), wake: Notify::new() }
    }
    #[tokio::test]
    async fn discards_duplicates_but_rejects_future_rounds() {
        let s = session();
        s.tx.try_send(Frame::Result { round: 1, ok: false }).unwrap();
        s.tx.try_send(Frame::Prepare { round: 2, network: "udp4".into(), mode: "internet".into(), token: "secret".into(), timeout_secs: 30 }).unwrap();
        assert!(matches!(s.wait(2, 3).await.unwrap(), Frame::Prepare { round: 2, .. }));
        s.tx.try_send(Frame::Ready { round: 4 }).unwrap();
        assert!(s.wait(3, 4).await.is_err());
    }
    #[tokio::test]
    async fn cancelled_session_cannot_advance() {
        let s = session();
        s.cancelled.store(true, Ordering::Release);
        assert!(s.wait(1, 4).await.is_err());
    }
}
