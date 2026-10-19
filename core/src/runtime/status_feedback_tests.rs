use super::*;

fn job(active: bool, state: TunnelJobState, code: &str) -> TunnelJobStatus {
    TunnelJobStatus {
        error_code: code.into(),
        peer_device_id: 42,
        peer_device_uuid: "peer".into(),
        is_active: active,
        state,
        attempt: 2,
        max_attempts: 30,
        message: "private dependency details: token=secret".into(),
        result: None,
        tcp_retry_recommended: false,
        symmetric_nat_help_recommended: false,
        updated_at: 123,
    }
}

fn lifecycle(
    role: &str,
    state: &str,
    last_result: &str,
    code: Option<&str>,
) -> TunnelLifecycleStatus {
    serde_json::from_value(serde_json::json!({
        "peer_device_id": 42, "role": role, "state": state,
        "attempt": 2, "max_attempts": 30, "last_result": last_result,
        "error_code": code, "message": "old text", "updated_at": 123,
    }))
    .unwrap()
}

#[test]
fn relocalization_rebuilds_each_job_collection_without_losing_live_state() {
    for collection in ["active", "wgvpn_active", "wgvpn_passive"] {
        for (state, lifecycle_state, result, code) in [
            (TunnelJobState::Running, "connecting", "none", ""),
            (
                TunnelJobState::Waiting,
                "connecting",
                "none",
                "traversal.punch.timeout",
            ),
            (TunnelJobState::Succeeded, "connected", "none", ""),
            (
                TunnelJobState::Failed,
                "not_established",
                "attempt_failed",
                "wireguard.handshake.failed",
            ),
            (
                TunnelJobState::Cancelled,
                "not_established",
                "cancelled",
                "connection.execution.peer_cancelled",
            ),
        ] {
            let active = collection != "wgvpn_passive";
            let current = job(active, state, code);
            let mut status = RuntimeStatus {
                locale: Some("en".into()),
                device_identity_rebuilt: true,
                active_tunnel_jobs: if collection == "active" {
                    vec![current.clone()]
                } else {
                    vec![]
                },
                wgvpn_jobs: if collection != "active" {
                    vec![current]
                } else {
                    vec![]
                },
                tunnel_lifecycles: vec![lifecycle(
                    if active { "active" } else { "passive" },
                    lifecycle_state,
                    result,
                    Some(code),
                )],
                ..RuntimeStatus::default()
            };
            relocalize_runtime_status(&mut status);
            assert!(status
                .device_identity_message
                .as_deref()
                .unwrap()
                .contains("identity"));
            let current = if collection == "active" {
                &status.active_tunnel_jobs[0]
            } else {
                &status.wgvpn_jobs[0]
            };
            assert_eq!(current.state, state);
            assert_eq!(current.error_code, code);
            assert!(!current.message.contains("secret"));
            let lifecycle = &status.tunnel_lifecycles[0];
            assert_eq!(lifecycle.updated_at, 123);
            if state == TunnelJobState::Succeeded {
                assert_eq!(lifecycle.message.as_deref(), Some("Tunnel connected"));
            } else {
                assert_eq!(lifecycle.message.as_deref(), Some(current.message.as_str()));
            }
            if state == TunnelJobState::Waiting {
                assert!(current.message.contains(if collection == "active" {
                    "60s"
                } else {
                    "30s"
                }));
            }
        }
    }
}

#[test]
fn locale_change_does_not_reclassify_health_approval_or_disconnection_as_failure() {
    for (state, result, expected) in [
        ("connecting", "none", "Waiting for the active peer to establish the connection"),
        ("awaiting_approval", "none", "Request sent. Waiting for the other person to allow the connection"),
        ("connected", "none", "Tunnel connected"),
        ("recovering", "none", "Recovering from network issues"),
        ("not_established", "health_grace_expired", "The network did not recover, so the tunnel was closed. Click Establish tunnel to reconnect"),
        ("not_established", "user_disconnected", "Tunnel disconnected by user"),
        ("not_established", "peer_disconnected", "Tunnel disconnected"),
        ("not_established", "none", "Tunnel disconnected"),
    ] {
        let mut status = RuntimeStatus {
            locale: Some("en".into()),
            tunnel_lifecycles: vec![lifecycle("passive", state, result, None)],
            ..RuntimeStatus::default()
        };
        let before = status.tunnel_lifecycles[0].clone();
        relocalize_runtime_status(&mut status);
        let after = &status.tunnel_lifecycles[0];
        assert_eq!(after.message.as_deref(), Some(expected));
        assert_eq!(after.state, before.state);
        assert_eq!(after.last_result, before.last_result);
        assert_eq!(after.error_code, before.error_code);
    }
}

#[test]
fn advice_matrix_keeps_one_relevant_suggestion_and_never_shows_it_while_retrying() {
    for locale in [None, Some("en")] {
        for active in [true, false] {
            for state in [
                TunnelJobState::Running,
                TunnelJobState::Waiting,
                TunnelJobState::Failed,
                TunnelJobState::Succeeded,
                TunnelJobState::Cancelled,
            ] {
                for tcp in [true, false] {
                    for nat in [true, false] {
                        let mut current = job(active, state, "traversal.punch.timeout");
                        current.tcp_retry_recommended = tcp;
                        current.symmetric_nat_help_recommended = nat;
                        localize_tunnel_job_feedback(locale, &mut current, 37);
                        let final_active = active && state == TunnelJobState::Failed;
                        assert_eq!(current.tcp_retry_recommended, final_active && tcp);
                        assert_eq!(
                            current.symmetric_nat_help_recommended,
                            final_active && !tcp && nat
                        );
                        assert_eq!(current.error_code, "traversal.punch.timeout");
                        assert_eq!(current.updated_at, 123);
                        assert_eq!((current.attempt, current.max_attempts), (2, 30));
                        let text = &current.message;
                        match state {
                            TunnelJobState::Running | TunnelJobState::Succeeded => {
                                assert_eq!(text, "private dependency details: token=secret")
                            }
                            TunnelJobState::Waiting => {
                                assert!(text.contains("37"));
                                assert!(
                                    !text.contains("secret")
                                        && !text.contains("TCP")
                                        && !text.contains("NAT")
                                );
                                assert!(!text.contains("请重试") && !text.contains("Retry;"));
                                if !active {
                                    assert!(
                                        text.contains(if locale.is_none() {
                                            "第 2 次"
                                        } else {
                                            "Attempt 2"
                                        }),
                                        "{text}"
                                    );
                                }
                            }
                            TunnelJobState::Failed => {
                                assert!(!text.contains("secret"));
                                if final_active && tcp {
                                    assert!(text.contains("TCP") && !text.contains("NAT"));
                                    assert!(
                                        !text.contains("更换网络")
                                            && !text.contains("different network")
                                    );
                                } else if final_active && nat {
                                    assert!(text.contains("NAT") && !text.contains("TCP"));
                                } else if !active {
                                    assert!(text.contains(if locale.is_none() {
                                        "发起方重试"
                                    } else {
                                        "initiator to retry"
                                    }));
                                } else {
                                    assert!(text.contains(if locale.is_none() {
                                        "更换网络"
                                    } else {
                                        "different network"
                                    }));
                                }
                            }
                            TunnelJobState::Cancelled => assert_eq!(
                                text,
                                if locale.is_none() {
                                    "本次连接已取消。"
                                } else {
                                    "This connection was cancelled."
                                }
                            ),
                        }
                        let once = serde_json::to_value(&current).unwrap();
                        localize_tunnel_job_feedback(locale, &mut current, 37);
                        assert_eq!(
                            serde_json::to_value(&current).unwrap(),
                            once,
                            "feedback must be idempotent"
                        );
                    }
                }
            }
        }
    }
}

#[test]
fn all_cancellation_codes_keep_their_identity_through_job_and_lifecycle_projection() {
    for code in [
        "connection.execution.user_cancelled",
        "connection.execution.peer_cancelled",
        "traversal.execution.cancelled",
        "signal.receive.cancelled",
    ] {
        for state in [TunnelJobState::Failed, TunnelJobState::Cancelled] {
            for active in [true, false] {
                let mut current = job(active, state, code);
                current.tcp_retry_recommended = true;
                current.symmetric_nat_help_recommended = true;
                localize_tunnel_job_feedback(None, &mut current, 60);
                assert_eq!(current.state, TunnelJobState::Cancelled);
                assert_eq!(current.error_code, code);
                assert!(!current.tcp_retry_recommended && !current.symmetric_nat_help_recommended);
                assert!(!current.message.contains("失败") && !current.message.contains("重试"));
                let (state, result, projected_code) =
                    tunnel_job_lifecycle_projection(current.state, code);
                assert_eq!(state, TunnelLifecycleState::NotEstablished);
                assert_eq!(result, TunnelLastResult::Cancelled);
                assert_eq!(projected_code.as_deref(), Some(code));
            }
        }
    }
    let (_, _, code) = tunnel_job_lifecycle_projection(TunnelJobState::Cancelled, "");
    assert_eq!(code.as_deref(), Some("connection.execution.user_cancelled"));
}

#[test]
fn projection_preserves_unknown_codes_and_uses_an_explicit_fallback_when_missing() {
    for state in [TunnelJobState::Waiting, TunnelJobState::Failed] {
        let (lifecycle_state, result, code) = tunnel_job_lifecycle_projection(state, "");
        assert_eq!(code.as_deref(), Some("runtime.execution.unclassified"));
        assert_eq!(
            lifecycle_state,
            if state == TunnelJobState::Waiting {
                TunnelLifecycleState::Connecting
            } else {
                TunnelLifecycleState::NotEstablished
            }
        );
        assert_eq!(
            result,
            if state == TunnelJobState::Waiting {
                TunnelLastResult::None
            } else {
                TunnelLastResult::AttemptFailed
            }
        );
        let (_, _, code) = tunnel_job_lifecycle_projection(state, "future.connection.failure");
        assert_eq!(code.as_deref(), Some("future.connection.failure"));
    }
    for state in [TunnelJobState::Running, TunnelJobState::Succeeded] {
        let (_, result, code) = tunnel_job_lifecycle_projection(state, "old.failure.code");
        assert_eq!(result, TunnelLastResult::None);
        assert!(
            code.is_none(),
            "old error codes must not leak into a new or successful connection"
        );
    }
}

#[test]
fn connecting_lifecycle_never_displays_another_device_or_roles_progress() {
    let mut other_device = job(
        true,
        TunnelJobState::Waiting,
        "connection.peer_prepare.timeout",
    );
    other_device.peer_device_id = 7;
    let mut status = RuntimeStatus {
        locale: Some("en".into()),
        active_tunnel_jobs: vec![other_device],
        wgvpn_jobs: vec![job(
            false,
            TunnelJobState::Waiting,
            "connection.approval.timeout",
        )],
        tunnel_lifecycles: vec![lifecycle("active", "connecting", "none", None)],
        ..RuntimeStatus::default()
    };
    relocalize_runtime_status(&mut status);
    assert_eq!(
        status.tunnel_lifecycles[0].message.as_deref(),
        Some("Waiting for the active peer to establish the connection")
    );
}

#[test]
fn relocalization_preserves_both_roles_and_selected_advice_without_mutating_error_codes() {
    let mut active = job(true, TunnelJobState::Failed, "traversal.punch.timeout");
    active.tcp_retry_recommended = true;
    active.symmetric_nat_help_recommended = true;
    let mut status = RuntimeStatus {
        active_tunnel_jobs: vec![active],
        wgvpn_jobs: vec![job(
            false,
            TunnelJobState::Failed,
            "wireguard.configure.permission_denied",
        )],
        tunnel_lifecycles: vec![
            lifecycle(
                "active",
                "not_established",
                "attempt_failed",
                Some("traversal.punch.timeout"),
            ),
            lifecycle(
                "passive",
                "not_established",
                "attempt_failed",
                Some("wireguard.configure.permission_denied"),
            ),
        ],
        ..RuntimeStatus::default()
    };
    for locale in ["zh-CN", "en", "zh-CN"] {
        status.locale = Some(locale.into());
        relocalize_runtime_status(&mut status);
        assert_eq!(
            status.tunnel_lifecycles[0].message.as_deref(),
            Some(status.active_tunnel_jobs[0].message.as_str())
        );
        assert_eq!(
            status.tunnel_lifecycles[1].message.as_deref(),
            Some(status.wgvpn_jobs[0].message.as_str())
        );
        assert!(status.active_tunnel_jobs[0].message.contains("TCP"));
        assert!(!status.active_tunnel_jobs[0].message.contains("NAT"));
        assert!(status.wgvpn_jobs[0].message.contains(if locale == "en" {
            "required permissions"
        } else {
            "必要权限"
        }));
        assert_eq!(
            status.active_tunnel_jobs[0].error_code,
            "traversal.punch.timeout"
        );
    }
}

#[test]
fn failed_lifecycle_does_not_reuse_an_unrelated_job_message() {
    // Same device is insufficient: role, error identity and terminal state must match.
    for mismatch in ["device", "role", "code", "state", "missing"] {
        let mut candidate = job(true, TunnelJobState::Failed, "traversal.punch.timeout");
        match mismatch {
            "device" => candidate.peer_device_id = 7,
            "role" => candidate.is_active = false,
            "code" => candidate.error_code = "connection.approval.denied".into(),
            "state" => candidate.state = TunnelJobState::Waiting,
            _ => {}
        }
        let mut status = RuntimeStatus {
            locale: Some("en".into()),
            wgvpn_jobs: if mismatch == "missing" {
                vec![]
            } else {
                vec![candidate]
            },
            tunnel_lifecycles: vec![lifecycle(
                "active",
                "not_established",
                "attempt_failed",
                Some("traversal.punch.timeout"),
            )],
            ..RuntimeStatus::default()
        };
        relocalize_runtime_status(&mut status);
        let text = status.tunnel_lifecycles[0].message.as_deref().unwrap();
        assert!(
            text.contains("direct connection") && text.contains("different network"),
            "{mismatch}: {text}"
        );
        assert!(
            !text.contains("approval") && !text.contains("retrying in") && !text.contains("secret")
        );
    }
}

#[test]
fn lifecycle_only_failures_and_cancellations_keep_safe_fallbacks() {
    for locale in [None, Some("en")] {
        let mut status = RuntimeStatus {
            locale: locale.map(str::to_string),
            tunnel_lifecycles: vec![
                lifecycle("active", "not_established", "attempt_failed", None),
                lifecycle(
                    "passive",
                    "not_established",
                    "attempt_failed",
                    Some("future.connection.failure"),
                ),
                lifecycle(
                    "active",
                    "not_established",
                    "cancelled",
                    Some("connection.execution.user_cancelled"),
                ),
                lifecycle(
                    "passive",
                    "not_established",
                    "cancelled",
                    Some("connection.execution.peer_cancelled"),
                ),
                lifecycle("active", "not_established", "cancelled", None),
            ],
            ..RuntimeStatus::default()
        };
        relocalize_runtime_status(&mut status);
        assert!(status.tunnel_lifecycles[0]
            .message
            .as_deref()
            .unwrap()
            .contains(if locale.is_none() {
                "暂时无法确定原因"
            } else {
                "could not be determined"
            }));
        assert!(status.tunnel_lifecycles[1]
            .message
            .as_deref()
            .unwrap()
            .contains("future.connection.failure"));
        for item in &status.tunnel_lifecycles[2..] {
            let text = item.message.as_deref().unwrap();
            assert!(text.contains(if locale.is_none() {
                "取消"
            } else {
                "cancelled"
            }));
            assert!(!text.contains("重试") && !text.contains("retry"));
        }
        assert!(status.tunnel_lifecycles[2]
            .message
            .as_deref()
            .unwrap()
            .contains(if locale.is_none() { "由你" } else { "You" }));
        assert!(status.tunnel_lifecycles[3]
            .message
            .as_deref()
            .unwrap()
            .contains(if locale.is_none() { "对端" } else { "peer" }));
        assert_eq!(
            status.tunnel_lifecycles[1].error_code.as_deref(),
            Some("future.connection.failure")
        );
    }
}
