use super::*;
use approval_effects::ApprovalFuture;

#[tokio::test]
async fn approval_terminal_status_distinguishes_cancel_from_failure_and_ignores_replaced_jobs() {
    for (code, expected) in [
        (
            ErrorCode::TraversalExecutionCancelled,
            TunnelJobState::Cancelled,
        ),
        (ErrorCode::SignalSendTimeout, TunnelJobState::Failed),
    ] {
        for stale in [false, true] {
            let (shared, _script, _rx, _cancel, _cancel_rx) = fixture(None, "");
            if stale {
                shared
                    .lock()
                    .wgvpn_job_cancels
                    .get_mut(&42)
                    .unwrap()
                    .generation = 2;
            }
            publish_approval_failure(&shared, 42, false, 1, 1, 1, &coded(code.clone(), "approval ended"));
            let s = shared.lock();
            if stale {
                assert!(s.status.wgvpn_jobs.is_empty());
                assert_eq!(s.wgvpn_job_cancels[&42].generation, 2);
            } else {
                assert_eq!(s.status.wgvpn_jobs[0].state, expected);
                assert!(!s.wgvpn_job_cancels.contains_key(&42));
                assert_eq!(
                    s.status.wgvpn_jobs[0].error_code,
                    if code == ErrorCode::TraversalExecutionCancelled {
                        ErrorCode::ConnectionExecutionUserCancelled.as_str()
                    } else {
                        code.as_str()
                    }
                );
            }
        }
    }
}

struct Script {
    calls: Mutex<Vec<&'static str>>,
    fail: &'static str,
    decision: Option<PassiveApprovalDecision>,
    tx: watch::Sender<PassiveApprovalDecision>,
    shared: Arc<Mutex<SharedRuntimeState>>,
    replace: bool,
}
impl ApprovalEffects for Script {
    fn allowed(&self, allowed: bool) -> ApprovalFuture<'_, Result<()>> {
        Box::pin(async move {
            let step = if allowed { "enable" } else { "disable" };
            self.calls.lock().push(step);
            if self.fail == step {
                Err(coded(ErrorCode::WireguardConfigureFailed, step))
            } else {
                Ok(())
            }
        })
    }
    fn send(&self, message: P2PAttemptMessage) -> ApprovalFuture<'_, Result<()>> {
        Box::pin(async move {
            let step = match message {
                P2PAttemptMessage::ApprovalRequired {
                    attempt_id,
                    expires_at,
                    ..
                } => {
                    assert_eq!(attempt_id, "old");
                    assert!(expires_at > 0);
                    "required"
                }
                P2PAttemptMessage::ApprovalGranted { attempt_id, .. } => {
                    assert_eq!(attempt_id, "old");
                    "granted"
                }
                P2PAttemptMessage::ApprovalDenied { .. } => "denied",
                P2PAttemptMessage::ApprovalTimeout { .. } => "timeout",
                _ => panic!("unexpected signal"),
            };
            self.calls.lock().push(step);
            if step == "required" {
                if self.replace {
                    let mut s = self.shared.lock();
                    s.wgvpn_job_cancels.get_mut(&42).unwrap().generation = 2;
                    s.pending_inbound_approval
                        .as_mut()
                        .unwrap()
                        .status
                        .attempt_id = "new".into();
                }
                if let Some(decision) = self.decision {
                    self.tx.send(decision).unwrap();
                }
            }
            if self.fail == step {
                Err(coded(ErrorCode::SignalSendTimeout, step))
            } else {
                Ok(())
            }
        })
    }
    fn stop(&self) -> ApprovalFuture<'_, ()> {
        Box::pin(async move {
            self.calls.lock().push("stop");
        })
    }
    fn refresh(&self) {
        self.calls.lock().push("refresh");
    }
}
fn fixture(
    decision: Option<PassiveApprovalDecision>,
    fail: &'static str,
) -> (
    Arc<Mutex<SharedRuntimeState>>,
    Script,
    watch::Receiver<PassiveApprovalDecision>,
    watch::Sender<bool>,
    watch::Receiver<bool>,
) {
    let shared = Arc::new(Mutex::new(SharedRuntimeState::default()));
    let (tx, rx) = watch::channel(PassiveApprovalDecision::Pending);
    let (cancel, cancel_rx) = watch::channel(false);
    {
        let mut s = shared.lock();
        s.wgvpn_job_cancels.insert(
            42,
            WgvpnJobControl {
                generation: 1,
                cancel_tx: cancel.clone(),
            },
        );
        s.pending_inbound_approval=Some(PendingInboundApprovalRuntime {status:serde_json::from_value(serde_json::json!({"attempt_id":"old","source_user_id":9,"source_device_id":42,"requested_at":0,"expires_at":60})).unwrap(),decision_tx:tx.clone(),connection_id:"connection".into(),source_device_id:42,access_grant:"test-only".into(),ws_client:ServiceWsClient::new()});
        s.passive_p2p_attempts.insert(
            42,
            PassiveP2PAttempt {
                connection_id: "connection".into(),
                attempt_id: "old".into(),
            },
        );
        s.passive_p2p_attempts.insert(
            99,
            PassiveP2PAttempt {
                connection_id: "other".into(),
                attempt_id: "other".into(),
            },
        );
    }
    let script = Script {
        calls: Mutex::new(vec![]),
        fail,
        decision,
        tx,
        shared: shared.clone(),
        replace: false,
    };
    (shared, script, rx, cancel, cancel_rx)
}
async fn run(
    shared: &Arc<Mutex<SharedRuntimeState>>,
    script: &Script,
    rx: watch::Receiver<PassiveApprovalDecision>,
    cancel: watch::Receiver<bool>,
) -> Result<()> {
    await_approval_with(
        shared,
        42,
        "old",
        1,
        1,
        1,
        now_ts() + 60,
        rx,
        Some(cancel),
        script,
    )
    .await
}

#[tokio::test(start_paused = true)]
async fn approval_grant_deny_and_timeout_have_exact_effect_order_and_cleanup() {
    for (decision, expected, code) in [
        (
            Some(PassiveApprovalDecision::Granted),
            vec!["disable", "refresh", "required", "enable", "granted"],
            None,
        ),
        (
            Some(PassiveApprovalDecision::Denied),
            vec!["disable", "refresh", "required", "denied", "stop"],
            Some(ErrorCode::ConnectionApprovalDenied),
        ),
        (
            None,
            vec!["disable", "refresh", "required", "timeout", "stop"],
            Some(ErrorCode::ConnectionApprovalTimeout),
        ),
    ] {
        let (shared, script, rx, _cancel, cancel_rx) = fixture(decision, "");
        let result = run(&shared, &script, rx, cancel_rx).await;
        assert_eq!(result.as_ref().err().map(code_of), code);
        assert_eq!(*script.calls.lock(), expected);
        let s = shared.lock();
        assert!(s.pending_inbound_approval.is_none());
        assert!(!s.passive_p2p_attempts.contains_key(&42));
        assert!(s.passive_p2p_attempts.contains_key(&99));
        assert_eq!(s.status.wgvpn_jobs[0].state, TunnelJobState::Waiting);
    }
}

#[tokio::test(start_paused = true)]
async fn permission_or_signal_failure_stops_once_and_preserves_the_original_error() {
    for fail in [
        "disable", "required", "enable", "granted", "denied", "timeout",
    ] {
        let decision = if fail == "timeout" {
            None
        } else if fail == "denied" {
            Some(PassiveApprovalDecision::Denied)
        } else {
            Some(PassiveApprovalDecision::Granted)
        };
        let (shared, script, rx, _cancel, cancel_rx) = fixture(decision, fail);
        let error = run(&shared, &script, rx, cancel_rx).await.unwrap_err();
        let expected = match fail {
            "disable" | "enable" => ErrorCode::WireguardConfigureFailed,
            "denied" => ErrorCode::ConnectionApprovalDenied,
            "timeout" => ErrorCode::ConnectionApprovalTimeout,
            _ => ErrorCode::SignalSendTimeout,
        };
        assert_eq!(code_of(&error), expected);
        assert_eq!(
            script.calls.lock().iter().filter(|x| **x == "stop").count(),
            1
        );
        assert!(shared.lock().pending_inbound_approval.is_none());
        if fail == "disable" || fail == "required" {
            assert!(!script.calls.lock().contains(&"enable"));
        }
    }
}

#[tokio::test(start_paused = true)]
async fn cancel_while_waiting_wakes_immediately_and_late_approval_cannot_enable_peer() {
    let (shared, script, rx, cancel, cancel_rx) = fixture(None, "");
    let script = Arc::new(script);
    let shared_task = shared.clone();
    let backend = script.clone();
    let task = tokio::spawn(async move { run(&shared_task, &backend, rx, cancel_rx).await });
    while !script.calls.lock().contains(&"required") {
        tokio::task::yield_now().await;
    }
    cancel.send(true).unwrap();
    let error = task.await.unwrap().unwrap_err();
    assert_eq!(code_of(&error), ErrorCode::TraversalExecutionCancelled);
    let _ = script.tx.send(PassiveApprovalDecision::Granted);
    assert!(!script.calls.lock().contains(&"enable"));
    assert_eq!(
        script.calls.lock().iter().filter(|x| **x == "stop").count(),
        1
    );
    assert!(shared.lock().pending_inbound_approval.is_none());
}

#[tokio::test(start_paused = true)]
async fn replaced_generation_rejects_old_grant_and_preserves_new_request_and_tunnel() {
    let (shared, mut script, rx, _cancel, cancel_rx) =
        fixture(Some(PassiveApprovalDecision::Granted), "");
    script.replace = true;
    let error = run(&shared, &script, rx, cancel_rx).await.unwrap_err();
    assert_eq!(code_of(&error), ErrorCode::TraversalExecutionCancelled);
    assert!(!script.calls.lock().contains(&"enable"));
    assert!(!script.calls.lock().contains(&"stop"));
    assert_eq!(
        shared
            .lock()
            .pending_inbound_approval
            .as_ref()
            .unwrap()
            .status
            .attempt_id,
        "new"
    );
    assert_eq!(shared.lock().wgvpn_job_cancels[&42].generation, 2);
}
