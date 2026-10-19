//! One application-owned protocol round at a time, over authorized WebSocket.
use crate::error::{code_of, coded, AppError, ErrorCode};
use crate::{
    gonc_ffi::{self, UdpTunnelRequest, UdpTunnelResult},
    traversal_policy::{self, NatEvidence, Negotiation},
    ws::ServiceWsClient,
};
use anyhow::Result;
use once_cell::sync::Lazy;
use parking_lot::Mutex;
use serde::{Deserialize, Serialize};
use std::{
    collections::HashMap,
    sync::{
        atomic::{AtomicBool, Ordering},
        Arc,
    },
    time::Duration,
};
use tokio::sync::{mpsc, Notify};

const LAN_PROBE_BUDGET_SECS: u64 = 1;
const LEGACY_LAN_PROBE_BUDGET_SECS: u64 = 6;

fn validate_lan_probe_budget(value: Option<u64>) -> Result<u64> {
    match value {
        None => Ok(LEGACY_LAN_PROBE_BUDGET_SECS),
        Some(value @ 1..=LEGACY_LAN_PROBE_BUDGET_SECS) => Ok(value),
        Some(_) => Err(coded(
            ErrorCode::TraversalCapabilityExchangeInvalidPayload,
            "invalid_lan_probe_budget",
        )),
    }
}

fn negotiate_lan_probe_budget(local: Option<u64>, remote: Option<u64>) -> Result<u64> {
    Ok(validate_lan_probe_budget(local)?.max(validate_lan_probe_budget(remote)?))
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RoundError {
    error_code: ErrorCode,
    message: String,
}

impl RoundError {
    fn from_error(error: &anyhow::Error) -> Self {
        Self {
            error_code: code_of(error),
            message: error.to_string(),
        }
    }
    fn into_error(self) -> anyhow::Error {
        AppError::new(self.error_code, self.message).into()
    }
}

// Both endpoints finish Close/Closed before applying the same round policy.
fn choose_round_error(
    local: Option<anyhow::Error>,
    peer: Option<RoundError>,
) -> Option<anyhow::Error> {
    let peer = peer.map(RoundError::into_error);
    match (local, peer) {
        (Some(local), Some(peer))
            if code_of(&local).retry_class() == "next_network_or_attempt"
                && code_of(&peer).retry_class() != "next_network_or_attempt" =>
        {
            Some(peer)
        }
        (Some(local), _) => Some(local),
        (None, peer) => peer,
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum Frame {
    Capabilities {
        evidence: Vec<NatEvidence>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        ipv6_available: Option<bool>,
        #[serde(default)]
        ipv6_gate_supported: bool,
        /// Missing on legacy peers, which keep the original 6-second LAN round.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        lan_probe_budget_secs: Option<u64>,
    },
    Plan {
        networks: Vec<String>,
    },
    PlanAck {
        networks: Vec<String>,
    },
    Prepare {
        round: u8,
        network: String,
        mode: String,
        token: String,
        timeout_secs: u64,
    },
    Ready {
        round: u8,
    },
    Result {
        round: u8,
        ok: bool,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        error: Option<RoundError>,
    },
    Commit {
        round: u8,
    },
    Committed {
        round: u8,
    },
    Close {
        round: u8,
    },
    Closed {
        round: u8,
    },
}

impl Frame {
    fn round(&self) -> u8 {
        match self {
            Self::Prepare { round, .. }
            | Self::Ready { round }
            | Self::Result { round, .. }
            | Self::Commit { round }
            | Self::Committed { round }
            | Self::Close { round }
            | Self::Closed { round } => *round,
            _ => 0,
        }
    }
    fn stage(&self) -> u8 {
        match self {
            Self::Capabilities { .. } => 0,
            Self::Plan { .. } => 1,
            Self::PlanAck { .. } => 2,
            Self::Prepare { .. } => 3,
            Self::Ready { .. } => 4,
            Self::Result { .. } => 5,
            Self::Commit { .. } | Self::Close { .. } => 6,
            Self::Committed { .. } | Self::Closed { .. } => 7,
        }
    }
}

static SESSIONS: Lazy<Mutex<HashMap<String, Arc<Session>>>> =
    Lazy::new(|| Mutex::new(HashMap::new()));

/// Negotiated plan records, keyed by attempt ID, for the /api/v1/p2p/end
/// report. Entries are consumed by take_plan when the job reports its result;
/// the registry is bounded so a crash between punch and report cannot grow it
/// without limit.
static PLANS: Lazy<Mutex<HashMap<String, (std::time::Instant, PlanRecord)>>> =
    Lazy::new(|| Mutex::new(HashMap::new()));

#[derive(Default, Clone)]
struct PlanRecord {
    proposed: String,
    acknowledged: String,
    local_nat: String,
    remote_nat: String,
    network: String,
    attempted_networks: Vec<String>,
}

/// Negotiation facts consumed by the /api/v1/p2p/end report. Empty strings
/// mean the attempt never reached plan negotiation (legacy peer or early
/// failure).
#[derive(Default, Clone)]
pub struct NegotiatedReport {
    pub plan: String,
    pub selection: String,
    pub local_nat: String,
    pub remote_nat: String,
    pub network: String,
    pub attempted_networks: Vec<String>,
}

/// Preliminary udp4 NAT classification from exchanged evidence; used as the
/// fallback NAT source when the actual punch call failed before producing a
/// diagnostic result.
pub fn udp4_nat(evidence: &[NatEvidence]) -> String {
    evidence
        .iter()
        .find(|v| v.network == "udp4" && matches!(v.nat_type.as_str(), "easy" | "hard" | "symm"))
        .map(|v| v.nat_type.clone())
        .unwrap_or_default()
}

fn plan_entry<'a>(
    plans: &'a mut HashMap<String, (std::time::Instant, PlanRecord)>,
    attempt: &str,
) -> &'a mut PlanRecord {
    if plans.len() >= 512 && !plans.contains_key(attempt) {
        if let Some(oldest) = plans
            .iter()
            .min_by_key(|(_, (created, _))| *created)
            .map(|(key, _)| key.clone())
        {
            tracing::warn!(attempt = %oldest, "traversal plan registry full; evicting oldest report");
            plans.remove(&oldest);
        }
    }
    &mut plans
        .entry(attempt.to_string())
        .or_insert_with(|| (std::time::Instant::now(), PlanRecord::default()))
        .1
}

fn record_plan(attempt: &str, proposed: Option<String>, acknowledged: Option<String>) {
    let mut plans = PLANS.lock();
    let entry = plan_entry(&mut plans, attempt);
    if let Some(value) = proposed {
        entry.proposed = value;
    }
    if let Some(value) = acknowledged {
        entry.acknowledged = value;
    }
}

fn record_evidence_nat(attempt: &str, local: String, remote: String) {
    let mut plans = PLANS.lock();
    let entry = plan_entry(&mut plans, attempt);
    if !local.is_empty() {
        entry.local_nat = local;
    }
    if !remote.is_empty() {
        entry.remote_nat = remote;
    }
}

pub fn peek_plan(attempt: &str) -> NegotiatedReport {
    PLANS
        .lock()
        .get(attempt)
        .map(|(_, record)| NegotiatedReport {
            plan: record.proposed.clone(),
            selection: record.acknowledged.clone(),
            local_nat: record.local_nat.clone(),
            remote_nat: record.remote_nat.clone(),
            network: record.network.clone(),
            attempted_networks: record.attempted_networks.clone(),
        })
        .unwrap_or_default()
}

fn record_native(attempt: &str, result: &UdpTunnelResult) {
    record_evidence_nat(
        attempt,
        result.local_nat_type.clone(),
        result.remote_nat_type.clone(),
    );
    if !result.network.is_empty() {
        plan_entry(&mut PLANS.lock(), attempt).network = result.network.clone();
    }
}

/// Take the negotiated plan and evidence NAT for the server-side p2p/end
/// report; consumes the record.
pub fn take_plan(attempt: &str) -> NegotiatedReport {
    match PLANS.lock().remove(attempt) {
        Some((_, record)) => NegotiatedReport {
            plan: record.proposed,
            selection: record.acknowledged,
            local_nat: record.local_nat,
            remote_nat: record.remote_nat,
            network: record.network,
            attempted_networks: record.attempted_networks,
        },
        None => NegotiatedReport::default(),
    }
}

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
    signal: Option<Arc<p2premote_punch::easyp2p::signal::ExternalSignalSession>>,
    ws_generation: u64,
}

pub fn register(
    token: String,
    attempt_id: String,
    connection_id: String,
    peer: i64,
    grant: String,
    ws: ServiceWsClient,
    active: bool,
    negotiation: Negotiation,
) -> Result<()> {
    register_with_signal(
        token,
        attempt_id,
        connection_id,
        peer,
        grant,
        ws,
        active,
        negotiation,
        None,
    )
}

pub fn register_with_signal(
    token: String,
    attempt_id: String,
    connection_id: String,
    peer: i64,
    grant: String,
    ws: ServiceWsClient,
    active: bool,
    negotiation: Negotiation,
    signal_local_device: Option<i64>,
) -> Result<()> {
    negotiation.validate()?;
    let ws_generation = ws.generation();
    let signal = match signal_local_device {
        Some(local) => {
            let session = crate::punch_signal::create_session(
                ws.clone(),
                connection_id.clone(),
                attempt_id.clone(),
                local,
                peer,
                grant.clone(),
            )?;
            session
                .register_call(
                    "wg-config",
                    &format!("{token}-kx"),
                    Duration::from_secs(120),
                )
                .map_err(anyhow::Error::new)?;
            Some(session)
        }
        None => None,
    };
    let (tx, rx) = mpsc::channel(64);
    let mut sessions = SESSIONS.lock();
    if sessions
        .values()
        .any(|s| s.attempt_id == attempt_id && s.connection_id == connection_id)
    {
        return Err(coded(
            ErrorCode::SignalReceiveContextMismatch,
            "duplicate_traversal_attempt",
        ));
    }
    sessions.insert(
        token,
        Arc::new(Session {
            attempt_id,
            connection_id,
            peer,
            grant,
            ws,
            active,
            negotiation,
            tx,
            rx: tokio::sync::Mutex::new(rx),
            cancelled: AtomicBool::new(false),
            wake: Notify::new(),
            signal,
            ws_generation,
        }),
    );
    Ok(())
}

pub fn lookup(token: &str) -> Option<Arc<Session>> {
    SESSIONS.lock().get(token).cloned()
}
pub fn remove(token: &str) {
    if let Some(session) = SESSIONS.lock().remove(token) {
        session.cancel_signal();
    }
}
pub fn cancel_token(token: &str) {
    if let Some(session) = lookup(token) {
        session.cancelled.store(true, Ordering::Release);
        session.wake.notify_one();
        session.cancel_signal();
    }
}
pub fn cancel(attempt: &str) {
    for session in SESSIONS.lock().values().filter(|s| s.attempt_id == attempt) {
        session.cancelled.store(true, Ordering::Release);
        session.wake.notify_one();
        session.cancel_signal();
    }
}
pub fn cancel_peer(peer: i64) {
    for session in SESSIONS.lock().values().filter(|s| s.peer == peer) {
        session.cancelled.store(true, Ordering::Release);
        session.wake.notify_one();
        session.cancel_signal();
    }
}

pub fn cancel_ws_sessions() {
    for session in SESSIONS.lock().values().filter(|s| s.signal.is_some()) {
        session.cancelled.store(true, Ordering::Release);
        session.wake.notify_one();
        session.cancel_signal();
    }
}

pub fn deliver_signal(
    connection: &str,
    peer: i64,
    generation: u64,
    frame: p2premote_punch::easyp2p::signal::SignalFrame,
) -> Result<()> {
    let session = SESSIONS
        .lock()
        .values()
        .find(|s| {
            s.connection_id == connection
                && s.peer == peer
                && s.attempt_id == frame.attempt_id
                && s.ws_generation == generation
        })
        .cloned()
        .ok_or_else(|| coded(ErrorCode::SignalReceiveStaleFrame, "stale signal frame"))?;
    let signal = session.signal.as_ref().ok_or_else(|| {
        coded(
            ErrorCode::SignalReceiveContextMismatch,
            "signal transport mismatch",
        )
    })?;
    signal
        .deliver(connection, peer, frame)
        .map_err(anyhow::Error::new)
}
pub fn deliver(connection: &str, peer: i64, attempt: &str, frame: Frame) -> Result<()> {
    let sessions = SESSIONS.lock();
    let session = sessions
        .values()
        .find(|s| s.connection_id == connection && s.peer == peer && s.attempt_id == attempt)
        .ok_or_else(|| coded(ErrorCode::SignalReceiveStaleFrame, "stale traversal frame"))?;
    session.tx.try_send(frame).map_err(|error| {
        AppError::with_source(
            ErrorCode::SignalReceiveQueueFull,
            "traversal inbox full",
            error,
        )
        .into()
    })
}

/// Own registry cleanup even when key exchange or platform setup fails.
pub struct Registration(pub String);
impl Drop for Registration {
    fn drop(&mut self) {
        remove(&self.0);
    }
}

type TraversalFuture<'a, T> =
    std::pin::Pin<Box<dyn std::future::Future<Output = Result<T>> + Send + 'a>>;

trait TraversalBackend: Send + Sync {
    fn ipv6_available(&self) -> Option<bool>;
    fn evidence(&self, ipv6: Option<bool>) -> TraversalFuture<'_, Vec<NatEvidence>>;
    fn send(&self, frame: Frame) -> TraversalFuture<'_, ()>;
    fn punch<'a>(
        &'a self,
        request: &'a UdpTunnelRequest,
        call: Option<Arc<p2premote_punch::easyp2p::signal::SignalCall>>,
    ) -> TraversalFuture<'a, UdpTunnelResult>;
    fn stop(&self, handle: &str);
}

struct LiveTraversal<'a>(&'a Session);
impl TraversalBackend for LiveTraversal<'_> {
    fn ipv6_available(&self) -> Option<bool> {
        traversal_policy::ipv6_available()
    }
    fn evidence(&self, ipv6: Option<bool>) -> TraversalFuture<'_, Vec<NatEvidence>> {
        Box::pin(async move { Ok(traversal_policy::collect_evidence(ipv6).await) })
    }
    fn send(&self, frame: Frame) -> TraversalFuture<'_, ()> {
        Box::pin(self.0.send(frame))
    }
    fn punch<'a>(
        &'a self,
        request: &'a UdpTunnelRequest,
        call: Option<Arc<p2premote_punch::easyp2p::signal::SignalCall>>,
    ) -> TraversalFuture<'a, UdpTunnelResult> {
        Box::pin(gonc_ffi::start_udp_tunnel_native_with_signal(request, call))
    }
    fn stop(&self, handle: &str) {
        gonc_ffi::stop_udp_tunnel_native(handle);
    }
}

struct PendingTunnel<'a>(Option<UdpTunnelResult>, &'a dyn TraversalBackend);
impl PendingTunnel<'_> {
    fn close(&mut self) {
        if let Some(tunnel) = self.0.take() {
            self.1.stop(&tunnel.handle_id);
        }
    }
}
impl Drop for PendingTunnel<'_> {
    fn drop(&mut self) {
        self.close();
    }
}

impl Session {
    fn cancel_signal(&self) {
        if let Some(signal) = &self.signal {
            signal.close();
        }
    }

    pub fn config_call(&self) -> Result<Option<Arc<p2premote_punch::easyp2p::signal::SignalCall>>> {
        self.check()?;
        self.signal
            .as_ref()
            .map(|s| {
                s.lookup_call("wg-config").ok_or_else(|| {
                    coded(
                        ErrorCode::SignalReceiveContextMismatch,
                        "signal_context_mismatch",
                    )
                })
            })
            .transpose()
    }

    fn check(&self) -> Result<()> {
        if self.cancelled.load(Ordering::Acquire) {
            Err(coded(
                ErrorCode::TraversalExecutionCancelled,
                "traversal_cancelled",
            ))
        } else {
            Ok(())
        }
    }
    async fn send(&self, frame: Frame) -> Result<()> {
        self.check()?;
        self.ws.send_p2p_notify(self.connection_id.clone(), self.peer, uuid::Uuid::new_v4().to_string(),
            self.grant.clone(), serde_json::json!({"type":"traversal", "attempt_id":self.attempt_id, "frame":frame}).to_string())
            .await.map_err(anyhow::Error::new)
    }
    async fn wait(&self, round: u8, stage: u8) -> Result<Frame> {
        self.wait_for(round, stage, 12).await
    }
    async fn wait_for(&self, round: u8, stage: u8, seconds: u64) -> Result<Frame> {
        let mut rx = self.rx.lock().await;
        tokio::time::timeout(Duration::from_secs(seconds), async {
            loop {
                self.check()?;
                let frame = tokio::select! {
                    f = rx.recv() => f.ok_or_else(|| coded(ErrorCode::SignalReceiveClosed, "traversal_signal_closed"))?,
                    _ = self.wake.notified() => { self.check()?; continue; }
                };
                if frame.round() < round || (frame.round() == round && frame.stage() < stage) {
                    continue;
                }
                if frame.round() != round || frame.stage() != stage {
                    return Err(coded(ErrorCode::TraversalRoundSyncInvalidSequence, "invalid_traversal_sequence"));
                }
                return Ok(frame);
            }
        })
        .await
        .map_err(|_| coded(match stage {
            0 => ErrorCode::TraversalCapabilityExchangeTimeout,
            1 | 2 => ErrorCode::TraversalPlanNegotiationTimeout,
            _ => ErrorCode::TraversalRoundSyncTimeout,
        }, "traversal frame wait timed out"))?
    }

    pub async fn start(&self, template: &UdpTunnelRequest) -> Result<UdpTunnelResult> {
        self.start_with_backend(template, &LiveTraversal(self))
            .await
    }

    async fn send_with(&self, backend: &dyn TraversalBackend, frame: Frame) -> Result<()> {
        self.check()?;
        backend.send(frame).await
    }

    async fn start_with_backend(
        &self,
        template: &UdpTunnelRequest,
        backend: &dyn TraversalBackend,
    ) -> Result<UdpTunnelResult> {
        self.check()?;
        if let Some(signal) = &self.signal {
            signal.close_call("wg-config");
        }
        let ipv6_available = backend.ipv6_available();
        let evidence = backend.evidence(ipv6_available).await?;
        record_evidence_nat(&self.attempt_id, udp4_nat(&evidence), String::new());
        self.send_with(
            backend,
            Frame::Capabilities {
                evidence: evidence.clone(),
                ipv6_available,
                ipv6_gate_supported: true,
                lan_probe_budget_secs: Some(LAN_PROBE_BUDGET_SECS),
            },
        )
        .await?;
        let (remote, remote_ipv6, remote_gate, remote_lan_budget) = match self.wait(0, 0).await? {
            Frame::Capabilities {
                evidence,
                ipv6_available,
                ipv6_gate_supported,
                lan_probe_budget_secs,
            } if evidence.len() <= 32 => (
                evidence,
                ipv6_available,
                ipv6_gate_supported,
                lan_probe_budget_secs,
            ),
            _ => {
                return Err(coded(
                    ErrorCode::TraversalCapabilityExchangeInvalidPayload,
                    "invalid_traversal_capabilities",
                ))
            }
        };
        let lan_probe_budget_secs =
            negotiate_lan_probe_budget(Some(LAN_PROBE_BUDGET_SECS), remote_lan_budget)?;
        tracing::info!(
            peer = self.peer,
            local_budget_secs = LAN_PROBE_BUDGET_SECS,
            remote_budget_secs = remote_lan_budget.unwrap_or(LEGACY_LAN_PROBE_BUDGET_SECS),
            negotiated_budget_secs = lan_probe_budget_secs,
            "LAN probe budget negotiated"
        );
        let networks = traversal_policy::build_plan_with_family(
            self.negotiation.preferences,
            &evidence,
            &remote,
            if remote_gate { ipv6_available } else { None },
            if remote_gate { remote_ipv6 } else { None },
        );
        record_evidence_nat(&self.attempt_id, udp4_nat(&evidence), udp4_nat(&remote));
        if self.active {
            tracing::info!(peer = self.peer, networks = ?networks, "traversal plan proposed to peer");
            record_plan(&self.attempt_id, Some(networks.join(",")), None);
            self.send_with(
                backend,
                Frame::Plan {
                    networks: networks.clone(),
                },
            )
            .await?;
            let accepted = match self.wait(0, 2).await? {
                Frame::PlanAck { networks } => networks,
                _ => {
                    return Err(coded(
                        ErrorCode::TraversalPlanNegotiationMismatch,
                        "traversal_plan_mismatch",
                    ))
                }
            };
            tracing::info!(peer = self.peer, networks = ?accepted, "traversal plan selection acknowledged by peer");
            record_plan(&self.attempt_id, None, Some(accepted.join(",")));
            if accepted != networks {
                return Err(coded(
                    ErrorCode::TraversalPlanNegotiationMismatch,
                    "traversal_plan_mismatch",
                ));
            }
        } else {
            let offered = match self.wait(0, 1).await? {
                Frame::Plan { networks } => networks,
                _ => {
                    return Err(coded(
                        ErrorCode::TraversalPlanNegotiationMismatch,
                        "traversal_plan_mismatch",
                    ))
                }
            };
            tracing::info!(peer = self.peer, proposed = ?offered, selected = ?networks,
                "traversal plan received from initiator; responding with independently recomputed selection");
            record_plan(&self.attempt_id, Some(offered.join(",")), None);
            if offered != networks {
                return Err(coded(
                    ErrorCode::TraversalPlanNegotiationMismatch,
                    "traversal_plan_mismatch",
                ));
            }
            record_plan(&self.attempt_id, None, Some(networks.join(",")));
            self.send_with(
                backend,
                Frame::PlanAck {
                    networks: networks.clone(),
                },
            )
            .await?;
        }
        // LAN eligibility is independent of public NAT classification.
        let rounds = std::iter::once(("udp4".to_string(), "lan", lan_probe_budget_secs))
            .chain(networks.into_iter().map(|n| (n, "internet", 30u64)));
        let mut attempted_networks = Vec::new();
        let mut last_error: Option<anyhow::Error> = None;
        for (index, (network, mode, timeout_secs)) in rounds.enumerate() {
            self.check()?;
            let round = (index + 1) as u8;
            let mut req = template.clone();
            req.network = network.clone();
            req.traversal_mode = mode.into();
            req.timeout_secs = timeout_secs;
            if self.active {
                req.token = uuid::Uuid::new_v4().to_string();
                if let Some(signal) = &self.signal {
                    signal
                        .register_call(
                            &format!("network/{round}"),
                            &req.token,
                            Duration::from_secs(timeout_secs + 12),
                        )
                        .map_err(anyhow::Error::new)?;
                }
                self.send_with(
                    backend,
                    Frame::Prepare {
                        round,
                        network,
                        mode: mode.into(),
                        token: req.token.clone(),
                        timeout_secs,
                    },
                )
                .await?;
                if !matches!(self.wait(round, 4).await?, Frame::Ready { .. }) {
                    return Err(coded(
                        ErrorCode::TraversalRoundSyncInvalidSequence,
                        "invalid_traversal_ready",
                    ));
                }
            } else {
                match self.wait(round, 3).await? {
                    Frame::Prepare {
                        network: n,
                        mode: m,
                        token,
                        timeout_secs: seconds,
                        ..
                    } if n == network
                        && m == mode
                        && seconds == timeout_secs
                        && token.len() >= 16
                        && token.len() <= 128 =>
                    {
                        req.token = token;
                        if let Some(signal) = &self.signal {
                            signal
                                .register_call(
                                    &format!("network/{round}"),
                                    &req.token,
                                    Duration::from_secs(timeout_secs + 12),
                                )
                                .map_err(anyhow::Error::new)?;
                        }
                    }
                    _ => {
                        return Err(coded(
                            ErrorCode::TraversalRoundSyncInvalidSequence,
                            "invalid_traversal_round",
                        ))
                    }
                }
                self.send_with(backend, Frame::Ready { round }).await?;
            }
            // Never abandon this call: cancellation must drain a late handle.
            // Older v2 peers still require their original plan and round IDs.
            // Keep that handshake, but never invoke native IPv6 without a source.
            let skip_ipv6 = req.network.ends_with('6') && ipv6_available == Some(false);
            tracing::info!(peer = self.peer, round, network = %req.network, mode = %req.traversal_mode,
                timeout_secs, skipped = skip_ipv6, "application traversal round start");
            let (result, signal_failure) = if skip_ipv6 {
                (
                    Err(coded(
                        ErrorCode::TraversalPunchIpv6Unavailable,
                        "ipv6_unavailable",
                    )),
                    None,
                )
            } else {
                if mode == "internet" {
                    attempted_networks.push(req.network.clone());
                    plan_entry(&mut PLANS.lock(), &self.attempt_id)
                        .attempted_networks
                        .push(req.network.clone());
                }
                let call = self
                    .signal
                    .as_ref()
                    .map(|s| {
                        s.lookup_call(&format!("network/{round}")).ok_or_else(|| {
                            coded(
                                ErrorCode::SignalReceiveContextMismatch,
                                "signal_context_mismatch",
                            )
                        })
                    })
                    .transpose()?;
                let result = backend.punch(&req, call.clone()).await;
                let signal_failure = result
                    .as_ref()
                    .err()
                    .and_then(|_| call.as_ref().and_then(|call| call.failure()));
                (result, signal_failure)
            };
            if let Some(signal) = &self.signal {
                signal.close_call(&format!("network/{round}"));
            }
            match &result {
                Ok(tunnel) => record_native(&self.attempt_id, tunnel),
                Err(error) => {
                    if let Some(failure) = error.downcast_ref::<gonc_ffi::UdpTunnelFailure>() {
                        record_native(&self.attempt_id, &failure.result);
                    }
                }
            }
            if let Some(error) = signal_failure {
                return Err(anyhow::Error::new(error));
            }
            tracing::info!(peer = self.peer, round, network = %req.network, mode = %req.traversal_mode,
                success = result.is_ok(), "application traversal round completed");
            let (tunnel, local_error) = match result {
                Ok(tunnel) => (Some(tunnel), None),
                Err(error) => (None, if skip_ipv6 { None } else { Some(error) }),
            };
            let mut owned = PendingTunnel(tunnel, backend);
            self.check()?;
            self.send_with(
                backend,
                Frame::Result {
                    round,
                    ok: owned.0.is_some(),
                    error: local_error.as_ref().map(RoundError::from_error),
                },
            )
            .await?;
            let (peer_ok, peer_error) = match self.wait_for(round, 5, timeout_secs + 12).await? {
                Frame::Result { ok, error, .. } if !ok || error.is_none() => (ok, error),
                _ => {
                    return Err(coded(
                        ErrorCode::TraversalRoundSyncInvalidSequence,
                        "invalid_traversal_result",
                    ))
                }
            };
            let success = peer_ok && owned.0.is_some();
            if self.active {
                self.send_with(
                    backend,
                    if success {
                        Frame::Commit { round }
                    } else {
                        Frame::Close { round }
                    },
                )
                .await?;
                let ack = self.wait(round, 7).await?;
                if success && matches!(ack, Frame::Committed { .. }) {
                    return Ok(owned.0.take().unwrap());
                }
                if success || !matches!(ack, Frame::Closed { .. }) {
                    return Err(coded(
                        ErrorCode::TraversalRoundSyncInvalidSequence,
                        "invalid_traversal_commit",
                    ));
                }
            } else {
                let decision = self.wait(round, 6).await?;
                if success && matches!(decision, Frame::Commit { .. }) {
                    self.send_with(backend, Frame::Committed { round }).await?;
                    return Ok(owned.0.take().unwrap());
                }
                if !matches!(decision, Frame::Close { .. }) {
                    return Err(coded(
                        ErrorCode::TraversalRoundSyncInvalidSequence,
                        "invalid_traversal_commit",
                    ));
                }
                owned.close();
                self.send_with(backend, Frame::Closed { round }).await?;
            }
            drop(owned);
            if let Some(error) = choose_round_error(local_error, peer_error) {
                if code_of(&error).retry_class() != "next_network_or_attempt" {
                    return Err(error);
                }
                last_error = Some(error);
            }
        }
        Err(last_error
            .map(|error| error.context("punch rounds exhausted"))
            .unwrap_or_else(|| {
                coded(
                    ErrorCode::TraversalExecutionExhausted,
                    format!("punch rounds exhausted: {}", attempted_networks.join(",")),
                )
            }))
    }
}

#[cfg(test)]
#[path = "traversal_tests.rs"]
mod tests;
