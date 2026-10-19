use super::*;
use std::collections::VecDeque;

struct NoWire;
impl p2premote_punch::easyp2p::signal::WireSender for NoWire {
    fn send(
        &self,
        _: String,
        _: std::time::Instant,
    ) -> p2premote_punch::easyp2p::signal::SendFuture<'_> {
        Box::pin(async { panic!("unit test must not use network") })
    }
}

#[tokio::test]
async fn bound_signal_calls_close_on_success_and_preserve_cancellation_instead_of_native_timeout() {
    use p2premote_punch::easyp2p::signal::{ExternalSignalSession, SessionIdentity};
    for active in [false, true] {
        for failed in [false, true] {
            let mut s = scripted_session(active, successful_frames(active, Some(1)));
            let signal = ExternalSignalSession::new(
                SessionIdentity {
                    connection_id: s.connection_id.clone(),
                    attempt_id: s.attempt_id.clone(),
                    local_device_id: 1,
                    peer_device_id: s.peer,
                },
                Arc::new(NoWire),
            )
            .unwrap();
            signal
                .register_call("wg-config", "config-token-000000", Duration::from_secs(120))
                .unwrap();
            s.signal = Some(signal.clone());
            let mut backend = ScriptedBackend::new(
                &s,
                if failed {
                    vec![Err(coded(
                        ErrorCode::TraversalPunchTimeout,
                        "native timed out",
                    ))]
                } else {
                    vec![native_ok("bound")]
                },
            );
            backend.cancel_signal = failed;
            let result = s
                .start_with_backend(
                    &UdpTunnelRequest::wgvpn("test-token", "active", 41118),
                    &backend,
                )
                .await;
            if failed {
                assert_eq!(
                    code_of(&result.unwrap_err()),
                    ErrorCode::SignalReceiveCancelled
                );
            } else {
                assert_eq!(result.unwrap().handle_id, "bound");
            }
            assert!(signal.lookup_call("wg-config").is_none());
            assert!(signal.lookup_call("network/1").is_none());
            assert_eq!(backend.requests.lock().len(), 1);
            assert!(backend.stopped.lock().is_empty());
            take_plan(&s.attempt_id);
        }
    }
}

#[tokio::test]
async fn legacy_ipv6_round_is_coordinated_but_never_invokes_native_without_a_source() {
    for active in [false, true] {
        let mut frames = vec![
            Frame::Capabilities {
                evidence: vec![],
                ipv6_available: None,
                ipv6_gate_supported: false,
                lan_probe_budget_secs: Some(1),
            },
            if active {
                Frame::PlanAck {
                    networks: vec!["udp4".into(), "udp6".into()],
                }
            } else {
                Frame::Plan {
                    networks: vec!["udp4".into(), "udp6".into()],
                }
            },
        ];
        for round in 1..=3 {
            let mode = if round == 1 { "lan" } else { "internet" };
            let mut p = prepare(round, mode, if round == 1 { 1 } else { 30 });
            if round == 3 {
                if let Frame::Prepare { network, .. } = &mut p {
                    *network = "udp6".into();
                }
            }
            frames.extend([
                if active { Frame::Ready { round } } else { p },
                Frame::Result {
                    round,
                    ok: false,
                    error: None,
                },
                if active {
                    Frame::Closed { round }
                } else {
                    Frame::Close { round }
                },
            ]);
        }
        let s = scripted_session(active, frames);
        let backend = ScriptedBackend::new(
            &s,
            vec![
                Err(coded(ErrorCode::TraversalLanProbeTimeout, "lan")),
                Err(coded(ErrorCode::TraversalPunchTimeout, "ipv4")),
            ],
        );
        let error = s
            .start_with_backend(
                &UdpTunnelRequest::wgvpn("test-token", "active", 41118),
                &backend,
            )
            .await
            .unwrap_err();
        assert_eq!(code_of(&error), ErrorCode::TraversalPunchTimeout);
        assert_eq!(backend.requests.lock().len(), 2);
        assert!(backend.requests.lock().iter().all(|r| r.network == "udp4"));
        assert!(backend.sent.lock().iter().any(|f| matches!(
            f,
            Frame::Result {
                round: 3,
                ok: false,
                error: None
            }
        )));
        assert_eq!(take_plan(&s.attempt_id).attempted_networks, vec!["udp4"]);
    }
}

struct ScriptedBackend<'a> {
    session: &'a Session,
    outcomes: Mutex<VecDeque<Result<UdpTunnelResult>>>,
    requests: Mutex<Vec<UdpTunnelRequest>>,
    sent: Mutex<Vec<Frame>>,
    stopped: Mutex<Vec<String>>,
    fail_send: Option<usize>,
    cancel_after_punch: bool,
    cancel_signal: bool,
}
impl<'a> ScriptedBackend<'a> {
    fn new(session: &'a Session, outcomes: Vec<Result<UdpTunnelResult>>) -> Self {
        Self {
            session,
            outcomes: Mutex::new(outcomes.into()),
            requests: Mutex::new(Vec::new()),
            sent: Mutex::new(Vec::new()),
            stopped: Mutex::new(Vec::new()),
            fail_send: None,
            cancel_after_punch: false,
            cancel_signal: false,
        }
    }
}
impl TraversalBackend for ScriptedBackend<'_> {
    fn ipv6_available(&self) -> Option<bool> {
        Some(false)
    }
    fn evidence(&self, _: Option<bool>) -> TraversalFuture<'_, Vec<NatEvidence>> {
        Box::pin(async { Ok(Vec::new()) })
    }
    fn send(&self, frame: Frame) -> TraversalFuture<'_, ()> {
        Box::pin(async move {
            let mut sent = self.sent.lock();
            sent.push(frame);
            if self.fail_send == Some(sent.len()) {
                return Err(coded(ErrorCode::SignalSendTimeout, "scripted send failure"));
            }
            Ok(())
        })
    }
    fn punch<'a>(
        &'a self,
        request: &'a UdpTunnelRequest,
        call: Option<Arc<p2premote_punch::easyp2p::signal::SignalCall>>,
    ) -> TraversalFuture<'a, UdpTunnelResult> {
        Box::pin(async move {
            self.requests.lock().push(request.clone());
            if self.session.signal.is_some() {
                let call = call.expect("registered native call missing");
                assert!(call.matches_uid(&request.token));
                if self.cancel_signal {
                    call.cancel();
                }
            }
            if self.cancel_after_punch {
                self.session.cancelled.store(true, Ordering::Release);
            }
            self.outcomes
                .lock()
                .pop_front()
                .expect("unexpected native call")
        })
    }
    fn stop(&self, handle: &str) {
        self.stopped.lock().push(handle.into());
    }
}
fn native_ok(handle: &str) -> Result<UdpTunnelResult> {
    Ok(serde_json::from_value(
        serde_json::json!({"ok":true,"handle_id":handle,"message":"","network":"udp4"}),
    )
    .unwrap())
}
fn capability(budget: Option<u64>) -> Frame {
    Frame::Capabilities {
        evidence: vec![],
        ipv6_available: Some(false),
        ipv6_gate_supported: true,
        lan_probe_budget_secs: budget,
    }
}
fn prepare(round: u8, mode: &str, timeout_secs: u64) -> Frame {
    Frame::Prepare {
        round,
        network: "udp4".into(),
        mode: mode.into(),
        token: "0123456789abcdef".into(),
        timeout_secs,
    }
}
fn scripted_session(active: bool, frames: Vec<Frame>) -> Session {
    let mut s = session();
    s.attempt_id = uuid::Uuid::new_v4().to_string();
    s.active = active;
    for frame in frames {
        s.tx.try_send(frame).unwrap();
    }
    s
}
fn successful_frames(active: bool, budget: Option<u64>) -> Vec<Frame> {
    vec![
        capability(budget),
        if active {
            Frame::PlanAck {
                networks: vec!["udp4".into()],
            }
        } else {
            Frame::Plan {
                networks: vec!["udp4".into()],
            }
        },
        if active {
            Frame::Ready { round: 1 }
        } else {
            prepare(1, "lan", budget.unwrap_or(6))
        },
        Frame::Result {
            round: 1,
            ok: true,
            error: None,
        },
        if active {
            Frame::Committed { round: 1 }
        } else {
            Frame::Commit { round: 1 }
        },
    ]
}

#[tokio::test]
async fn real_round_flow_commits_both_roles_and_preserves_legacy_budget() {
    for active in [false, true] {
        for budget in [Some(1), None] {
            let s = scripted_session(active, successful_frames(active, budget));
            let backend = ScriptedBackend::new(&s, vec![native_ok("owned-by-caller")]);
            let result = s
                .start_with_backend(
                    &UdpTunnelRequest::wgvpn("test-token", "active", 41118),
                    &backend,
                )
                .await
                .unwrap();
            assert_eq!(result.handle_id, "owned-by-caller");
            assert!(
                backend.stopped.lock().is_empty(),
                "successful handle must transfer to caller"
            );
            let requests = backend.requests.lock();
            assert_eq!(requests.len(), 1);
            assert_eq!(requests[0].traversal_mode, "lan");
            assert_eq!(requests[0].timeout_secs, budget.unwrap_or(6));
            assert!(backend
                .sent
                .lock()
                .iter()
                .any(|f| matches!(f, Frame::Commit { .. } | Frame::Committed { .. })));
            take_plan(&s.attempt_id);
        }
    }
}

#[tokio::test]
async fn real_round_flow_falls_back_only_after_close_handshake() {
    for active in [false, true] {
        let mut frames = successful_frames(active, Some(1));
        frames.truncate(3);
        frames.extend([
            Frame::Result {
                round: 1,
                ok: false,
                error: None,
            },
            if active {
                Frame::Closed { round: 1 }
            } else {
                Frame::Close { round: 1 }
            },
            if active {
                Frame::Ready { round: 2 }
            } else {
                prepare(2, "internet", 30)
            },
            Frame::Result {
                round: 2,
                ok: true,
                error: None,
            },
            if active {
                Frame::Committed { round: 2 }
            } else {
                Frame::Commit { round: 2 }
            },
        ]);
        let s = scripted_session(active, frames);
        let backend = ScriptedBackend::new(
            &s,
            vec![
                Err(coded(ErrorCode::TraversalLanProbeTimeout, "LAN expired")),
                native_ok("internet"),
            ],
        );
        assert_eq!(
            s.start_with_backend(
                &UdpTunnelRequest::wgvpn("test-token", "active", 41118),
                &backend
            )
            .await
            .unwrap()
            .handle_id,
            "internet"
        );
        let requests = backend.requests.lock();
        assert_eq!(
            requests
                .iter()
                .map(|r| r.traversal_mode.as_str())
                .collect::<Vec<_>>(),
            vec!["lan", "internet"]
        );
        assert_eq!(requests[1].timeout_secs, 30);
        let sent = backend.sent.lock();
        let closed = sent
            .iter()
            .position(|f| matches!(f, Frame::Close { round: 1 } | Frame::Closed { round: 1 }))
            .unwrap();
        let second = sent
            .iter()
            .position(|f| {
                matches!(
                    f,
                    Frame::Prepare { round: 2, .. } | Frame::Ready { round: 2 }
                )
            })
            .unwrap();
        assert!(closed < second);
        drop(sent);
        assert_eq!(take_plan(&s.attempt_id).attempted_networks, vec!["udp4"]);
    }
}

#[tokio::test]
async fn single_sided_success_closes_handle_exactly_once() {
    for active in [false, true] {
        let mut frames = successful_frames(active, Some(1));
        frames[3] = Frame::Result {
            round: 1,
            ok: false,
            error: Some(RoundError {
                error_code: ErrorCode::SignalSendTimeout,
                message: "peer signal failed".into(),
            }),
        };
        frames[4] = if active {
            Frame::Closed { round: 1 }
        } else {
            Frame::Close { round: 1 }
        };
        let s = scripted_session(active, frames);
        let backend = ScriptedBackend::new(&s, vec![native_ok("must-close")]);
        let error = s
            .start_with_backend(
                &UdpTunnelRequest::wgvpn("test-token", "active", 41118),
                &backend,
            )
            .await
            .unwrap_err();
        assert_eq!(code_of(&error), ErrorCode::SignalSendTimeout);
        assert_eq!(*backend.stopped.lock(), vec!["must-close"]);
        assert_eq!(backend.requests.lock().len(), 1);
        take_plan(&s.attempt_id);
    }
}

#[tokio::test]
async fn cancellation_after_native_completion_drains_late_handle() {
    let s = scripted_session(true, successful_frames(true, Some(1)));
    let mut backend = ScriptedBackend::new(&s, vec![native_ok("late-handle")]);
    backend.cancel_after_punch = true;
    let error = s
        .start_with_backend(
            &UdpTunnelRequest::wgvpn("test-token", "active", 41118),
            &backend,
        )
        .await
        .unwrap_err();
    assert_eq!(code_of(&error), ErrorCode::TraversalExecutionCancelled);
    assert_eq!(*backend.stopped.lock(), vec!["late-handle"]);
    assert!(!backend
        .sent
        .lock()
        .iter()
        .any(|f| matches!(f, Frame::Result { .. } | Frame::Commit { .. })));
    take_plan(&s.attempt_id);
}

#[tokio::test]
async fn signal_send_failures_never_leak_native_handle() {
    for fail_send in 1..=5 {
        let s = scripted_session(true, successful_frames(true, Some(1)));
        let mut backend = ScriptedBackend::new(&s, vec![native_ok("on-send-failure")]);
        backend.fail_send = Some(fail_send);
        let error = s
            .start_with_backend(
                &UdpTunnelRequest::wgvpn("test-token", "active", 41118),
                &backend,
            )
            .await
            .unwrap_err();
        assert_eq!(code_of(&error), ErrorCode::SignalSendTimeout);
        if fail_send <= 3 {
            assert!(backend.requests.lock().is_empty());
            assert!(backend.stopped.lock().is_empty());
        } else {
            assert_eq!(*backend.stopped.lock(), vec!["on-send-failure"]);
        }
        take_plan(&s.attempt_id);
    }
}

#[tokio::test]
async fn invalid_negotiation_is_rejected_before_any_native_call() {
    for active in [false, true] {
        for variant in 0..5 {
            let mut frames = successful_frames(active, Some(1));
            match variant {
                0 => frames[0] = capability(Some(0)),
                1 => {
                    frames[0] = Frame::Capabilities {
                        evidence: vec![
                            NatEvidence {
                                network: "udp4".into(),
                                nat_type: "easy".into()
                            };
                            33
                        ],
                        ipv6_available: None,
                        ipv6_gate_supported: false,
                        lan_probe_budget_secs: Some(1),
                    }
                }
                2 => {
                    frames[1] = if active {
                        Frame::PlanAck {
                            networks: vec!["tcp4".into()],
                        }
                    } else {
                        Frame::Plan {
                            networks: vec!["tcp4".into()],
                        }
                    }
                }
                3 if !active => frames[2] = prepare(1, "internet", 1),
                4 if !active => {
                    let mut f = prepare(1, "lan", 1);
                    if let Frame::Prepare { token, .. } = &mut f {
                        *token = "short".into();
                    }
                    frames[2] = f;
                }
                _ => continue,
            }
            let s = scripted_session(active, frames);
            let backend = ScriptedBackend::new(&s, vec![]);
            let error = s
                .start_with_backend(
                    &UdpTunnelRequest::wgvpn("test-token", "active", 41118),
                    &backend,
                )
                .await
                .unwrap_err();
            let expected = if variant < 2 {
                ErrorCode::TraversalCapabilityExchangeInvalidPayload
            } else if variant == 2 {
                ErrorCode::TraversalPlanNegotiationMismatch
            } else {
                ErrorCode::TraversalRoundSyncInvalidSequence
            };
            assert_eq!(code_of(&error), expected);
            assert!(backend.requests.lock().is_empty());
            take_plan(&s.attempt_id);
        }
    }
}

#[tokio::test]
async fn invalid_result_or_commit_releases_handle_for_both_roles() {
    for active in [false, true] {
        for invalid_result in [false, true] {
            let mut frames = successful_frames(active, Some(1));
            if invalid_result {
                frames[3] = Frame::Result {
                    round: 1,
                    ok: true,
                    error: Some(RoundError {
                        error_code: ErrorCode::TraversalPunchFailed,
                        message: "contradictory success".into(),
                    }),
                };
            } else {
                if !active {
                    frames[3] = Frame::Result {
                        round: 1,
                        ok: false,
                        error: None,
                    };
                }
                frames[4] = if active {
                    Frame::Closed { round: 1 }
                } else {
                    Frame::Commit { round: 1 }
                };
            }
            let s = scripted_session(active, frames);
            let backend = ScriptedBackend::new(&s, vec![native_ok("invalid-commit")]);
            let error = s
                .start_with_backend(
                    &UdpTunnelRequest::wgvpn("test-token", "active", 41118),
                    &backend,
                )
                .await
                .unwrap_err();
            assert_eq!(
                code_of(&error),
                ErrorCode::TraversalRoundSyncInvalidSequence
            );
            assert_eq!(*backend.stopped.lock(), vec!["invalid-commit"]);
            take_plan(&s.attempt_id);
        }
    }
}

#[tokio::test]
async fn exhausted_rounds_preserve_last_cause_and_close_both_rounds() {
    for active in [false, true] {
        let mut frames = successful_frames(active, Some(1));
        frames.truncate(3);
        frames.extend([
            Frame::Result {
                round: 1,
                ok: false,
                error: None,
            },
            if active {
                Frame::Closed { round: 1 }
            } else {
                Frame::Close { round: 1 }
            },
            if active {
                Frame::Ready { round: 2 }
            } else {
                prepare(2, "internet", 30)
            },
            Frame::Result {
                round: 2,
                ok: false,
                error: None,
            },
            if active {
                Frame::Closed { round: 2 }
            } else {
                Frame::Close { round: 2 }
            },
        ]);
        let s = scripted_session(active, frames);
        let backend = ScriptedBackend::new(
            &s,
            vec![
                Err(coded(ErrorCode::TraversalLanProbeTimeout, "LAN")),
                Err(coded(ErrorCode::TraversalPunchTimeout, "public timeout")),
            ],
        );
        let error = s
            .start_with_backend(
                &UdpTunnelRequest::wgvpn("test-token", "active", 41118),
                &backend,
            )
            .await
            .unwrap_err();
        assert_eq!(code_of(&error), ErrorCode::TraversalPunchTimeout);
        assert!(error.to_string().contains("rounds exhausted"));
        assert_eq!(backend.requests.lock().len(), 2);
        assert!(backend.stopped.lock().is_empty());
        assert_eq!(
            backend
                .sent
                .lock()
                .iter()
                .filter(|f| matches!(f, Frame::Close { .. } | Frame::Closed { .. }))
                .count(),
            2
        );
        take_plan(&s.attempt_id);
    }
}

#[tokio::test]
async fn cancelled_before_start_has_no_external_effects() {
    let s = scripted_session(true, vec![]);
    s.cancelled.store(true, Ordering::Release);
    let backend = ScriptedBackend::new(&s, vec![]);
    let error = s
        .start_with_backend(
            &UdpTunnelRequest::wgvpn("test-token", "active", 41118),
            &backend,
        )
        .await
        .unwrap_err();
    assert_eq!(code_of(&error), ErrorCode::TraversalExecutionCancelled);
    assert!(backend.sent.lock().is_empty());
    assert!(backend.requests.lock().is_empty());
    assert!(backend.stopped.lock().is_empty());
}
#[test]
fn shared_round_errors_preserve_wire_identity_and_policy() {
    let cases: serde_json::Value = serde_json::from_str(include_str!(
        "../../../p2premote-punch-rs/testdata/traversal-round-errors-v1.json"
    ))
    .unwrap();
    for row in cases["cases"].as_array().unwrap() {
        let local = row["local"]
            .as_str()
            .map(|code| coded(ErrorCode::parse(code), "local detail"));
        let peer = row["peer"].as_str().map(|code| RoundError {
            error_code: ErrorCode::parse(code),
            message: "peer detail".into(),
        });
        let frame = Frame::Result {
            round: 3,
            ok: false,
            error: peer,
        };
        let encoded = serde_json::to_string(&frame).unwrap();
        let Frame::Result { error, .. } = serde_json::from_str(&encoded).unwrap() else {
            unreachable!()
        };
        let selected = choose_round_error(local, error);
        match row["selected"].as_str() {
            Some(expected) => {
                let selected = selected.unwrap();
                assert_eq!(code_of(&selected).as_str(), expected, "{}", row["name"]);
                assert_eq!(
                    code_of(&selected).retry_class() == "next_network_or_attempt",
                    row["advance"].as_bool().unwrap()
                );
                let wrapped = selected.context("all rounds finished");
                assert_eq!(code_of(&wrapped).as_str(), expected);
            }
            None => assert!(selected.is_none()),
        }
    }
}
#[test]
fn shared_lan_budget_vectors_match_android() {
    #[derive(Deserialize)]
    struct BudgetCase {
        local: Option<u64>,
        remote: Option<u64>,
        expected: Option<u64>,
    }
    #[derive(Deserialize)]
    struct Vectors {
        budget_cases: Vec<BudgetCase>,
    }
    let vectors: Vectors = serde_json::from_str(include_str!(
        "../../../p2premote-punch-rs/testdata/traversal-policy-v2.json"
    ))
    .unwrap();
    for case in vectors.budget_cases {
        match case.expected {
            Some(expected) => assert_eq!(
                negotiate_lan_probe_budget(case.local, case.remote).unwrap(),
                expected
            ),
            None => assert!(negotiate_lan_probe_budget(case.local, case.remote).is_err()),
        }
    }
}
fn session() -> Session {
    let (tx, rx) = mpsc::channel(64);
    Session {
        attempt_id: "test".into(),
        connection_id: "authorized".into(),
        peer: 7,
        grant: String::new(),
        ws: ServiceWsClient::new(),
        active: true,
        negotiation: Negotiation {
            version: 2,
            preferences: Default::default(),
        },
        tx,
        rx: tokio::sync::Mutex::new(rx),
        cancelled: AtomicBool::new(false),
        wake: Notify::new(),
        signal: None,
        ws_generation: 0,
    }
}
#[tokio::test]
async fn discards_duplicates_but_rejects_future_rounds() {
    let s = session();
    s.tx.try_send(Frame::Result {
        round: 1,
        ok: false,
        error: None,
    })
    .unwrap();
    s.tx.try_send(Frame::Prepare {
        round: 2,
        network: "udp4".into(),
        mode: "internet".into(),
        token: "secret".into(),
        timeout_secs: 30,
    })
    .unwrap();
    assert!(matches!(
        s.wait(2, 3).await.unwrap(),
        Frame::Prepare { round: 2, .. }
    ));
    s.tx.try_send(Frame::Ready { round: 4 }).unwrap();
    assert_eq!(
        code_of(&s.wait(3, 4).await.unwrap_err()),
        ErrorCode::TraversalRoundSyncInvalidSequence
    );
}
#[tokio::test]
async fn cancelled_session_cannot_advance() {
    let s = session();
    s.cancelled.store(true, Ordering::Release);
    assert_eq!(
        code_of(&s.wait(1, 4).await.unwrap_err()),
        ErrorCode::TraversalExecutionCancelled
    );
}
#[tokio::test]
async fn empty_wait_uses_stage_specific_timeout_codes() {
    for (stage, expected) in [
        (0, ErrorCode::TraversalCapabilityExchangeTimeout),
        (1, ErrorCode::TraversalPlanNegotiationTimeout),
        (2, ErrorCode::TraversalPlanNegotiationTimeout),
        (3, ErrorCode::TraversalRoundSyncTimeout),
        (4, ErrorCode::TraversalRoundSyncTimeout),
        (5, ErrorCode::TraversalRoundSyncTimeout),
    ] {
        let s = session();
        assert_eq!(
            code_of(&s.wait_for(1, stage, 0).await.unwrap_err()),
            expected
        );
    }
}
#[tokio::test]
async fn closed_signal_is_distinct_from_timeout_and_cancel() {
    let s = session();
    s.rx.lock().await.close();
    assert_eq!(
        code_of(&s.wait_for(1, 4, 1).await.unwrap_err()),
        ErrorCode::SignalReceiveClosed
    );
}
#[tokio::test]
async fn same_round_old_stage_is_discarded_and_future_stage_is_rejected() {
    let s = session();
    s.tx.try_send(Frame::Ready { round: 1 }).unwrap();
    s.tx.try_send(Frame::Result {
        round: 1,
        ok: false,
        error: None,
    })
    .unwrap();
    assert!(matches!(
        s.wait_for(1, 5, 1).await.unwrap(),
        Frame::Result { round: 1, .. }
    ));
    s.tx.try_send(Frame::Result {
        round: 1,
        ok: true,
        error: None,
    })
    .unwrap();
    assert_eq!(
        code_of(&s.wait_for(1, 4, 1).await.unwrap_err()),
        ErrorCode::TraversalRoundSyncInvalidSequence
    );
}
#[test]
fn udp4_evidence_extraction_skips_invalid_types() {
    use crate::traversal_policy::NatEvidence;
    let nat = |network: &str, nat_type: &str| NatEvidence {
        network: network.into(),
        nat_type: nat_type.into(),
    };
    assert_eq!(udp4_nat(&[nat("udp4", "easy")]), "easy");
    assert_eq!(
        udp4_nat(&[nat("udp6", "easy"), nat("udp4", "symm")]),
        "symm"
    );
    assert_eq!(udp4_nat(&[nat("udp4", "unknown")]), "");
    assert_eq!(udp4_nat(&[]), "");
}

#[test]
fn evidence_written_before_plan_survives_report_collection() {
    let attempt = uuid::Uuid::new_v4().to_string();
    record_evidence_nat(&attempt, "easy".into(), "symm".into());
    record_plan(&attempt, Some("udp4".into()), Some("udp4".into()));
    let report = take_plan(&attempt);
    assert_eq!(report.local_nat, "easy");
    assert_eq!(report.remote_nat, "symm");
    assert_eq!(report.plan, "udp4");
    assert_eq!(report.selection, "udp4");
    assert_eq!(take_plan(&attempt).plan, "");
}

#[test]
fn report_capacity_evicts_only_the_oldest_new_entry() {
    let now = std::time::Instant::now();
    let mut reports = HashMap::new();
    for index in 0..512 {
        reports.insert(
            format!("attempt-{index}"),
            (now + Duration::from_secs(index), PlanRecord::default()),
        );
    }
    plan_entry(&mut reports, "attempt-511").proposed = "udp4".into();
    assert_eq!(reports.len(), 512);
    plan_entry(&mut reports, "new").local_nat = "easy".into();
    assert_eq!(reports.len(), 512);
    assert!(!reports.contains_key("attempt-0"));
    assert_eq!(reports["attempt-511"].1.proposed, "udp4");
}
#[test]
fn capability_address_family_is_optional_and_boolean() {
    let frame: Frame = serde_json::from_str(r#"{"kind":"capabilities","evidence":[]}"#).unwrap();
    assert!(matches!(
        frame,
        Frame::Capabilities {
            ipv6_available: None,
            lan_probe_budget_secs: None,
            ..
        }
    ));
    let frame: Frame =
        serde_json::from_str(r#"{"kind":"capabilities","evidence":[],"ipv6_available":false}"#)
            .unwrap();
    assert!(matches!(
        frame,
        Frame::Capabilities {
            ipv6_available: Some(false),
            ..
        }
    ));
    assert!(serde_json::from_str::<Frame>(
        r#"{"kind":"capabilities","evidence":[],"ipv6_available":"false"}"#
    )
    .is_err());
}
#[test]
fn lan_probe_failures_advance_but_signal_failures_end_the_attempt() {
    for code in [
        ErrorCode::TraversalLanProbeTimeout,
        ErrorCode::TraversalLanProbeFailed,
    ] {
        let error = choose_round_error(Some(coded(code.clone(), "LAN round ended")), None).unwrap();
        assert_eq!(code_of(&error).retry_class(), "next_network_or_attempt");
        let error = choose_round_error(
            Some(coded(code, "LAN round ended")),
            Some(RoundError {
                error_code: ErrorCode::SignalSendTimeout,
                message: "delivery failed".into(),
            }),
        )
        .unwrap();
        assert_eq!(code_of(&error), ErrorCode::SignalSendTimeout);
        assert_ne!(code_of(&error).retry_class(), "next_network_or_attempt");
    }
}

#[test]
fn lan_probe_budget_is_short_only_when_both_peers_support_it() {
    assert_eq!(negotiate_lan_probe_budget(Some(1), Some(1)).unwrap(), 1);
    assert_eq!(
        negotiate_lan_probe_budget(Some(1), None).unwrap(),
        LEGACY_LAN_PROBE_BUDGET_SECS
    );
    assert_eq!(
        negotiate_lan_probe_budget(None, Some(1)).unwrap(),
        LEGACY_LAN_PROBE_BUDGET_SECS
    );
    assert_eq!(negotiate_lan_probe_budget(Some(1), Some(2)).unwrap(), 2);
}
#[test]
fn lan_probe_budget_rejects_out_of_range_values() {
    assert!(negotiate_lan_probe_budget(Some(0), Some(1)).is_err());
    assert!(negotiate_lan_probe_budget(Some(1), Some(7)).is_err());
}
