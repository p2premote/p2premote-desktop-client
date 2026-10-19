use super::*;
use effects::JobFuture;
use std::collections::VecDeque;
use tokio::sync::broadcast;

enum Outcome {
    Error(ErrorCode),
    Success,
    Unsuccessful,
    WaitSuccess,
    Panic,
}
struct Script {
    outcomes: Mutex<VecDeque<Outcome>>,
    calls: Mutex<Vec<String>>,
    reports: Mutex<Vec<JobReport>>,
    release: tokio::sync::Notify,
    cancel_on_success: Option<watch::Sender<bool>>,
}
fn success() -> ActiveStartResult {
    ActiveStartResult {
        success: true,
        rdp_address: "100.99.71.3:3389".into(),
        remote_address: "100.99.71.3:3389".into(),
        remote_protocol: "rdp".into(),
        network: "udp4".into(),
        source_nat_type: "easy".into(),
        target_nat_type: "hard".into(),
        message: "established".into(),
        warning: None,
    }
}
impl ActiveJobEffects for Script {
    fn attempt(&self, n: u8, id: String, _: String) -> JobFuture<'_, Result<ActiveStartResult>> {
        Box::pin(async move {
            assert!(!id.is_empty());
            self.calls.lock().push(format!("attempt:{n}"));
            let outcome = self
                .outcomes
                .lock()
                .pop_front()
                .expect("unexpected extra attempt");
            match outcome {
                Outcome::Error(code) => Err(coded(code, "scripted failure")),
                Outcome::Panic => panic!("scripted task failure"),
                Outcome::Unsuccessful => {
                    let mut r = success();
                    r.success = false;
                    r.message = "not established".into();
                    Ok(r)
                }
                Outcome::Success => {
                    if let Some(tx) = &self.cancel_on_success {
                        tx.send(true).unwrap();
                    }
                    Ok(success())
                }
                Outcome::WaitSuccess => {
                    self.release.notified().await;
                    self.calls.lock().push("drained".into());
                    Ok(success())
                }
            }
        })
    }
    fn cancel(&self, _: String, reason: &'static str) -> JobFuture<'_, ()> {
        Box::pin(async move {
            self.calls.lock().push(format!("cancel:{reason}"));
            self.release.notify_one();
        })
    }
    fn stop(&self) -> JobFuture<'_, ()> {
        Box::pin(async move {
            self.calls.lock().push("stop".into());
        })
    }
    fn report(&self, r: JobReport) -> JobFuture<'_, ()> {
        Box::pin(async move {
            self.calls.lock().push("report".into());
            self.reports.lock().push(r);
        })
    }
    fn established(&self) {
        self.calls.lock().push("established".into());
    }
}
fn script(outcomes: Vec<Outcome>) -> Arc<Script> {
    Arc::new(Script {
        outcomes: Mutex::new(outcomes.into()),
        calls: Mutex::new(vec![]),
        reports: Mutex::new(vec![]),
        release: tokio::sync::Notify::new(),
        cancel_on_success: None,
    })
}
fn state() -> Arc<Mutex<SharedRuntimeState>> {
    Arc::new(Mutex::new(SharedRuntimeState::default()))
}
async fn run(
    shared: Arc<Mutex<SharedRuntimeState>>,
    rx: watch::Receiver<bool>,
    backend: Arc<Script>,
) {
    run_active_job(
        shared,
        rx,
        42,
        "peer".into(),
        MachineConfig::default(),
        backend,
    )
    .await;
}
fn terminal(shared: &Arc<Mutex<SharedRuntimeState>>) -> TunnelJobStatus {
    shared.lock().status.active_tunnel_jobs[0].clone()
}

#[tokio::test(start_paused = true)]
async fn retryable_failures_exhaust_exactly_thirty_attempts_and_report_once() {
    let shared = state();
    let (tx, rx) = watch::channel(false);
    let backend = script(
        (0..ACTIVE_TUNNEL_JOB_MAX_ATTEMPTS)
            .map(|_| Outcome::Error(ErrorCode::TraversalPunchTimeout))
            .collect(),
    );
    let (events, mut statuses) = broadcast::channel(128);
    shared.lock().status_tx = Some(events);
    run(shared.clone(), rx, backend.clone()).await;
    drop(tx);
    let status = terminal(&shared);
    assert_eq!(status.state, TunnelJobState::Failed);
    assert_eq!(status.attempt, 30);
    assert_eq!(status.error_code, ErrorCode::TraversalPunchTimeout.as_str());
    let reports = backend.reports.lock();
    assert_eq!(reports.len(), 1);
    assert!(!reports[0].success);
    assert_eq!(reports[0].error_code, status.error_code);
    assert!(!reports[0].attempt_id.is_empty());
    assert_eq!(
        backend
            .calls
            .lock()
            .iter()
            .filter(|s| s.starts_with("attempt:"))
            .count(),
        30
    );
    let mut waiting = 0;
    while let Ok(s) = statuses.try_recv() {
        if s.active_tunnel_jobs[0].state == TunnelJobState::Waiting {
            waiting += 1;
        }
    }
    assert_eq!(waiting, 29);
}

#[tokio::test(start_paused = true)]
async fn fatal_failure_stops_immediately_and_successful_retry_transfers_ownership() {
    for succeeds in [false, true] {
        let shared = state();
        let (_tx, rx) = watch::channel(false);
        let backend = if succeeds {
            script(vec![
                Outcome::Error(ErrorCode::TraversalPunchTimeout),
                Outcome::Success,
            ])
        } else {
            script(vec![Outcome::Error(ErrorCode::ConnectionApprovalDenied)])
        };
        run(shared.clone(), rx, backend.clone()).await;
        let status = terminal(&shared);
        assert_eq!(
            status.state,
            if succeeds {
                TunnelJobState::Succeeded
            } else {
                TunnelJobState::Failed
            }
        );
        assert_eq!(status.attempt, if succeeds { 2 } else { 1 });
        let reports = backend.reports.lock();
        assert_eq!(reports.len(), 1);
        assert_eq!(reports[0].success, succeeds);
        if succeeds {
            assert_eq!(reports[0].source_nat, "easy");
            assert_eq!(reports[0].target_nat, "hard");
            assert_eq!(reports[0].network, "udp4");
            assert!(status.result.unwrap().success);
        }
        assert_eq!(
            backend
                .calls
                .lock()
                .iter()
                .filter(|s| *s == "established")
                .count(),
            usize::from(succeeds)
        );
        assert!(!backend.calls.lock().iter().any(|s| s == "stop"));
    }
}

#[tokio::test(start_paused = true)]
async fn cancellation_before_start_has_no_attempt_and_waiting_cancel_never_retries() {
    for before in [true, false] {
        let shared = state();
        let (tx, rx) = watch::channel(before);
        let backend = script(if before {
            vec![]
        } else {
            vec![Outcome::Error(ErrorCode::TraversalPunchTimeout)]
        });
        let (events, mut statuses) = broadcast::channel(16);
        shared.lock().status_tx = Some(events);
        let task = tokio::spawn(run(shared.clone(), rx, backend.clone()));
        if !before {
            while statuses.recv().await.unwrap().active_tunnel_jobs[0].state
                != TunnelJobState::Waiting
            {}
            tx.send(true).unwrap();
        }
        task.await.unwrap();
        assert_eq!(terminal(&shared).state, TunnelJobState::Cancelled);
        assert_eq!(backend.reports.lock().len(), 1);
        assert_eq!(
            backend.reports.lock()[0].error_code,
            "connection.execution.user_cancelled"
        );
        assert_eq!(
            backend
                .calls
                .lock()
                .iter()
                .filter(|s| s.starts_with("attempt:"))
                .count(),
            usize::from(!before)
        );
    }
}

#[tokio::test(start_paused = true)]
async fn cancel_or_deadline_drains_late_success_before_cleanup_and_never_publishes_it() {
    for cancelled in [true, false] {
        let shared = state();
        let (tx, rx) = watch::channel(false);
        let backend = script(if cancelled {
            vec![Outcome::WaitSuccess]
        } else {
            vec![
                Outcome::WaitSuccess,
                Outcome::Error(ErrorCode::ConnectionApprovalDenied),
            ]
        });
        let task = tokio::spawn(run(shared.clone(), rx, backend.clone()));
        while backend.calls.lock().is_empty() {
            tokio::task::yield_now().await;
        }
        if cancelled {
            tx.send(true).unwrap();
        }
        task.await.unwrap();
        let calls = backend.calls.lock();
        let drained = calls.iter().position(|x| x == "drained").unwrap();
        let stopped = calls.iter().position(|x| x == "stop").unwrap();
        let reported = calls.iter().position(|x| x == "report").unwrap();
        assert!(drained < stopped && stopped < reported);
        assert!(!calls.iter().any(|x| x == "established"));
        assert_eq!(calls.iter().filter(|x| *x == "stop").count(), 1);
        assert_eq!(
            terminal(&shared).state,
            if cancelled {
                TunnelJobState::Cancelled
            } else {
                TunnelJobState::Failed
            }
        );
        assert!(!backend.reports.lock()[0].success);
    }
}

#[tokio::test(start_paused = true)]
async fn completion_racing_cancel_is_never_reported_as_success() {
    let shared = state();
    let (tx, rx) = watch::channel(false);
    let mut backend = script(vec![Outcome::Success]);
    Arc::get_mut(&mut backend).unwrap().cancel_on_success = Some(tx);
    run(shared.clone(), rx, backend.clone()).await;
    assert_eq!(terminal(&shared).state, TunnelJobState::Cancelled);
    assert!(!backend.reports.lock()[0].success);
    assert!(!backend.calls.lock().iter().any(|x| x == "established"));
    assert_eq!(
        backend.calls.lock().iter().filter(|x| *x == "stop").count(),
        1
    );
}

#[tokio::test(start_paused = true)]
async fn unsuccessful_native_result_and_join_failure_preserve_failure_reporting() {
    for outcome in [Outcome::Unsuccessful, Outcome::Panic] {
        let shared = state();
        let (_tx, rx) = watch::channel(false);
        let backend = script(vec![
            outcome,
            Outcome::Error(ErrorCode::ConnectionApprovalDenied),
        ]);
        run(shared.clone(), rx, backend.clone()).await;
        assert_eq!(terminal(&shared).state, TunnelJobState::Failed);
        assert_eq!(backend.reports.lock().len(), 1);
        assert!(!backend.reports.lock()[0].success);
    }
}

#[test]
fn old_job_slot_cannot_remove_a_replacement_slot() {
    let shared = state();
    let (old, _old_rx) = watch::channel(false);
    let (new, _new_rx) = watch::channel(false);
    shared
        .lock()
        .active_tunnel_job_cancels
        .insert(42, old.clone());
    let slot = ActiveTunnelJobSlot {
        shared: shared.clone(),
        peer_device_id: 42,
        owner: Some(old),
    };
    shared
        .lock()
        .active_tunnel_job_cancels
        .insert(42, new.clone());
    drop(slot);
    assert!(shared.lock().active_tunnel_job_cancels[&42].same_channel(&new));
    drop(ActiveTunnelJobSlot {
        shared: shared.clone(),
        peer_device_id: 42,
        owner: Some(new),
    });
    assert!(!shared.lock().active_tunnel_job_cancels.contains_key(&42));
}
