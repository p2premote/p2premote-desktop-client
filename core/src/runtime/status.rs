//! Runtime status mutations shared by service tasks.

use super::{
    classify_tunnel_error_code, now_ts, SharedRuntimeState, ACTIVE_TUNNEL_JOB_ATTEMPT_TOTAL_SECS,
    ACTIVE_TUNNEL_JOB_BACKOFF_SECS, WGVPN_JOB_BACKOFF_SECS,
};
use crate::control::{
    TunnelJobState, TunnelJobStatus, RuntimeStatus, TunnelLastResult,
    TunnelLifecycleRole, TunnelLifecycleState, TunnelLifecycleStatus,
};
use crate::i18n::localized_message;
use parking_lot::Mutex;
use std::sync::Arc;

/// Rebuild all service-owned display messages after the UI language changes.
///
/// RuntimeStatus is also consumed by the CLI and Web UI, so these messages are
/// localized in the service rather than translated by a single frontend.  A
/// locale switch must therefore update the existing snapshot as well as future
/// state transitions.
pub(super) fn relocalize_runtime_status(status: &mut RuntimeStatus) {
    let locale = status.locale.clone();

    if status.device_identity_rebuilt {
        status.device_identity_message = Some(localized_message(
            locale.as_deref(),
            "device_identity.cloned_rebuilt",
            &[],
        ));
    }

    for job in &mut status.active_tunnel_jobs {
        job.message = match job.state {
            TunnelJobState::Running => localized_message(
                locale.as_deref(),
                "tunnel.job.attempt_total_secs",
                &[("secs", &ACTIVE_TUNNEL_JOB_ATTEMPT_TOTAL_SECS.to_string())],
            ),
            TunnelJobState::Waiting => localized_message(
                locale.as_deref(),
                "tunnel.job.attempt_failed_retry",
                &[
                    (
                        "reason",
                        &localized_message(
                            locale.as_deref(),
                            "errors.hole_punch_wait_timeout",
                            &[],
                        ),
                    ),
                    ("secs", &ACTIVE_TUNNEL_JOB_BACKOFF_SECS.to_string()),
                ],
            ),
            TunnelJobState::Succeeded => {
                localized_message(locale.as_deref(), "tunnel.job.auto_established", &[])
            }
            TunnelJobState::Failed => {
                localized_message(locale.as_deref(), "tunnel.job.ended_not_established", &[])
            }
            TunnelJobState::Cancelled => {
                localized_message(locale.as_deref(), "tunnel.job.cancelled", &[])
            }
        };
    }

    for job in &mut status.wgvpn_jobs {
        job.message = match job.state {
            TunnelJobState::Running => localized_message(
                locale.as_deref(),
                "wgvpn.job.building",
                &[
                    ("attempt", &job.attempt.to_string()),
                    ("max", &job.max_attempts.to_string()),
                ],
            ),
            TunnelJobState::Waiting => localized_message(
                locale.as_deref(),
                "wgvpn.job.attempt_failed_retry",
                &[
                    ("attempt", &job.attempt.to_string()),
                    (
                        "reason",
                        &localized_message(
                            locale.as_deref(),
                            "errors.hole_punch_wait_timeout",
                            &[],
                        ),
                    ),
                    ("secs", &WGVPN_JOB_BACKOFF_SECS.to_string()),
                ],
            ),
            TunnelJobState::Succeeded => {
                localized_message(locale.as_deref(), "tunnel.lifecycle.connected", &[])
            }
            TunnelJobState::Failed => localized_message(
                locale.as_deref(),
                "wgvpn.job.all_attempts_failed",
                &[("max", &job.max_attempts.to_string())],
            ),
            TunnelJobState::Cancelled => {
                localized_message(locale.as_deref(), "wgvpn.job.cancelled", &[])
            }
        };
    }

    let job_messages: Vec<(i64, TunnelLifecycleRole, String)> = status
        .active_tunnel_jobs
        .iter()
        .map(|job| {
            (
                job.peer_device_id,
                TunnelLifecycleRole::Active,
                job.message.clone(),
            )
        })
        .chain(status.wgvpn_jobs.iter().map(|job| {
            (
                job.peer_device_id,
                if job.is_active {
                    TunnelLifecycleRole::Active
                } else {
                    TunnelLifecycleRole::Passive
                },
                job.message.clone(),
            )
        }))
        .collect();

    for lifecycle in &mut status.tunnel_lifecycles {
        lifecycle.message = Some(match lifecycle.state {
            TunnelLifecycleState::Connecting => job_messages
                .iter()
                .find(|(device_id, role, _)| {
                    *device_id == lifecycle.peer_device_id && *role == lifecycle.role
                })
                .map(|(_, _, message)| message.clone())
                .unwrap_or_else(|| {
                    localized_message(locale.as_deref(), "tunnel.job.waiting_passive", &[])
                }),
            TunnelLifecycleState::AwaitingApproval => {
                localized_message(locale.as_deref(), "tunnel.lifecycle.awaiting_approval", &[])
            }
            TunnelLifecycleState::Connected => {
                localized_message(locale.as_deref(), "tunnel.lifecycle.connected", &[])
            }
            TunnelLifecycleState::Recovering => {
                localized_message(locale.as_deref(), "tunnel.lifecycle.recovering", &[])
            }
            TunnelLifecycleState::NotEstablished => match lifecycle.last_result {
                TunnelLastResult::HealthGraceExpired => localized_message(
                    locale.as_deref(),
                    "tunnel.lifecycle.health_grace_expired",
                    &[],
                ),
                TunnelLastResult::UserDisconnected => localized_message(
                    locale.as_deref(),
                    "tunnel.lifecycle.passive_disconnected",
                    &[],
                ),
                TunnelLastResult::Cancelled => {
                    localized_message(locale.as_deref(), "tunnel.job.cancelled", &[])
                }
                TunnelLastResult::AttemptFailed => {
                    localized_message(locale.as_deref(), "tunnel.job.ended_not_established", &[])
                }
                TunnelLastResult::PeerDisconnected | TunnelLastResult::None => {
                    localized_message(locale.as_deref(), "tunnel.lifecycle.disconnected", &[])
                }
            },
        });
    }
}

/// Update the status snapshot and broadcast it to persistent IPC clients.
pub(super) fn update_status(
    shared: &Arc<Mutex<SharedRuntimeState>>,
    f: impl FnOnce(&mut RuntimeStatus),
) {
    let mut state = shared.lock();
    f(&mut state.status);
    if let Some(tx) = &state.status_tx {
        let _ = tx.send(state.status.clone());
    }
}

/// Read the UI locale before generating a user-facing status message.
/// This keeps localization outside a status-locking closure.
pub(super) fn current_locale(shared: &Arc<Mutex<SharedRuntimeState>>) -> Option<String> {
    shared.lock().status.locale.clone()
}

/// Merge an incoming lifecycle projection without losing identity or the first
/// successful connection timestamp from an earlier update.
pub(super) fn upsert_tunnel_lifecycle(
    status: &mut RuntimeStatus,
    mut lifecycle: TunnelLifecycleStatus,
) {
    if let Some(existing) = status
        .tunnel_lifecycles
        .iter_mut()
        .find(|item| item.peer_device_id == lifecycle.peer_device_id && item.role == lifecycle.role)
    {
        if lifecycle.source_username.is_empty() {
            lifecycle.source_username = existing.source_username.clone();
        }
        if lifecycle.source_email.is_empty() {
            lifecycle.source_email = existing.source_email.clone();
        }
        if lifecycle.source_user_id == 0 {
            lifecycle.source_user_id = existing.source_user_id;
        }
        if lifecycle.peer_device_name.is_empty() {
            lifecycle.peer_device_name = existing.peer_device_name.clone();
        }
        if lifecycle.peer_device_alias.is_empty() {
            lifecycle.peer_device_alias = existing.peer_device_alias.clone();
        }
        if lifecycle.peer_public_ip.is_empty() {
            lifecycle.peer_public_ip = existing.peer_public_ip.clone();
        }
        if lifecycle.connected_at.is_none() {
            lifecycle.connected_at = existing.connected_at;
        }
        if lifecycle.state == TunnelLifecycleState::Connected
            && existing.state != TunnelLifecycleState::Connected
            && lifecycle.connected_at.is_none()
        {
            lifecycle.connected_at = Some(now_ts());
        }
        *existing = lifecycle;
    } else {
        if lifecycle.state == TunnelLifecycleState::Connected && lifecycle.connected_at.is_none() {
            lifecycle.connected_at = Some(now_ts());
        }
        status.tunnel_lifecycles.push(lifecycle);
    }
}

/// 统一的 job 状态 → lifecycle 投影（审计 O-1：合并主动/被动两轨的重复映射）。
/// Waiting 统一携带 hole_punch_wait_timeout：两轨的重试原因本就是同一个
/// 打洞等待超时（i18n 共用 errors.hole_punch_wait_timeout）。
pub(super) fn tunnel_job_lifecycle_projection(
    state: TunnelJobState,
    message: &str,
) -> (TunnelLifecycleState, TunnelLastResult, Option<String>) {
    match state {
        TunnelJobState::Running => (TunnelLifecycleState::Connecting, TunnelLastResult::None, None),
        TunnelJobState::Waiting => (
            TunnelLifecycleState::Connecting,
            TunnelLastResult::None,
            Some("hole_punch_wait_timeout".to_string()),
        ),
        TunnelJobState::Succeeded => (TunnelLifecycleState::Connected, TunnelLastResult::None, None),
        TunnelJobState::Failed => (
            TunnelLifecycleState::NotEstablished,
            TunnelLastResult::AttemptFailed,
            Some(classify_tunnel_error_code(message).to_string()),
        ),
        TunnelJobState::Cancelled => (
            TunnelLifecycleState::NotEstablished,
            TunnelLastResult::Cancelled,
            Some("user_cancelled".to_string()),
        ),
    }
}

pub(super) fn update_active_tunnel_job_status(
    shared: &Arc<Mutex<SharedRuntimeState>>,
    status: TunnelJobStatus,
) {
    let mut state = shared.lock();
    if let Some(existing) = state
        .status
        .active_tunnel_jobs
        .iter_mut()
        .find(|item| item.peer_device_id == status.peer_device_id)
    {
        *existing = status.clone();
    } else {
        state.status.active_tunnel_jobs.push(status.clone());
    }
    let (lifecycle_state, last_result, error_code) =
        tunnel_job_lifecycle_projection(status.state, &status.message);
    let stage = match status.state {
        TunnelJobState::Running => Some("hole_punch_wait".to_string()),
        TunnelJobState::Waiting => Some("retry_wait".to_string()),
        _ => None,
    };
    upsert_tunnel_lifecycle(
        &mut state.status,
        TunnelLifecycleStatus {
            peer_device_id: status.peer_device_id,
            source_user_id: 0,
            source_username: String::new(),
            source_email: String::new(),
            peer_device_name: String::new(),
            peer_device_alias: String::new(),
            peer_public_ip: String::new(),
            role: TunnelLifecycleRole::Active,
            state: lifecycle_state,
            attempt: status.attempt,
            max_attempts: status.max_attempts,
            stage,
            virtual_ip: None,
            peer_virtual_ip: None,
            last_result,
            error_code,
            message: Some(status.message.clone()),
            health_failures: 0,
            health_grace_deadline: None,
            connected_at: None,
            updated_at: status.updated_at,
        },
    );
    if let Some(tx) = &state.status_tx {
        let _ = tx.send(state.status.clone());
    }
}

pub(super) fn clear_active_tunnel_job_status(
    shared: &Arc<Mutex<SharedRuntimeState>>,
    peer_device_id: i64,
) {
    let mut state = shared.lock();
    state
        .status
        .active_tunnel_jobs
        .retain(|item| item.peer_device_id != peer_device_id);
    if let Some(tx) = &state.status_tx {
        let _ = tx.send(state.status.clone());
    }
}
