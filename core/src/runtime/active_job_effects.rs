use super::*;
use std::{future::Future, pin::Pin};

pub(super) type JobFuture<'a, T> = Pin<Box<dyn Future<Output = T> + Send + 'a>>;

pub(super) struct JobReport {
    pub attempt_id: String,
    pub success: bool,
    pub source_nat: String,
    pub target_nat: String,
    pub network: String,
    pub elapsed_secs: u64,
    pub error_code: String,
    pub message: String,
}

pub(super) trait ActiveJobEffects: Send + Sync {
    fn attempt(
        &self,
        number: u8,
        id: String,
        token: String,
    ) -> JobFuture<'_, Result<ActiveStartResult>>;
    fn cancel(&self, id: String, reason: &'static str) -> JobFuture<'_, ()>;
    fn stop(&self) -> JobFuture<'_, ()>;
    fn report(&self, report: JobReport) -> JobFuture<'_, ()>;
    fn established(&self);
}

pub(super) struct LiveActiveJob {
    pub shared: Arc<Mutex<SharedRuntimeState>>,
    pub ws: ServiceWsClient,
    pub config: MachineConfig,
    pub opened: crate::p2p::ActiveP2POpenResult,
    pub peer: i64,
    pub client_job_id: String,
}

impl ActiveJobEffects for LiveActiveJob {
    fn attempt(
        &self,
        number: u8,
        id: String,
        token: String,
    ) -> JobFuture<'_, Result<ActiveStartResult>> {
        Box::pin(async move {
            start_wgvpn_active_with_notify(
                &self.shared,
                &self.ws,
                &self.config,
                self.peer,
                &self.client_job_id,
                &self.opened,
                number,
                id,
                token,
            )
            .await
        })
    }
    fn cancel(&self, id: String, reason: &'static str) -> JobFuture<'_, ()> {
        Box::pin(async move {
            crate::traversal::cancel(&id);
            let _ = send_p2p_attempt_message(
                &self.ws,
                self.opened.connection_id.clone(),
                self.peer,
                self.opened.access_grant.clone(),
                P2PAttemptMessage::AttemptCancel {
                    protocol_version: 1,
                    attempt_id: id,
                    reason: reason.into(),
                },
            )
            .await;
        })
    }
    fn stop(&self) -> JobFuture<'_, ()> {
        Box::pin(async move {
            let _ = wgvpn_flow::stop_wgvpn(&self.config, self.peer).await;
        })
    }
    fn report(&self, report: JobReport) -> JobFuture<'_, ()> {
        Box::pin(async move {
            let _ = crate::p2p::close_active_p2p_job_with_nat(
                &self.config,
                &self.opened,
                self.peer,
                &report.attempt_id,
                report.success,
                report.source_nat,
                report.target_nat,
                report.network,
                report.elapsed_secs,
                report.error_code,
                report.message,
            )
            .await;
        })
    }
    fn established(&self) {
        let session_info = wgvpn_flow::snapshot_sessions()
            .into_iter()
            .find(|s| s.peer_device_id == self.peer && s.is_active)
            .map(|s| (s.peer_virtual_ip, s.peer_health_port));
        self.shared
            .lock()
            .wgvpn_health_runtime
            .insert(self.peer, WgvpnHealthRuntime::default());
        refresh_wgvpn_sessions(&self.shared);
        if let Some((ip, port)) = session_info.filter(|(ip, _)| !ip.is_empty()) {
            spawn_wgvpn_health_monitor(self.shared.clone(), self.peer, ip, port);
        } else {
            mark_wgvpn_health_degraded(self.shared.clone(), self.peer, "missing_peer_virtual_ip");
        }
    }
}
