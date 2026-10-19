use super::*;
use std::{future::Future, pin::Pin};
pub(super) type ApprovalFuture<'a, T> = Pin<Box<dyn Future<Output = T> + Send + 'a>>;
pub(super) trait ApprovalEffects: Send + Sync {
    fn allowed(&self, allowed: bool) -> ApprovalFuture<'_, Result<()>>;
    fn send(&self, message: P2PAttemptMessage) -> ApprovalFuture<'_, Result<()>>;
    fn stop(&self) -> ApprovalFuture<'_, ()>;
    fn refresh(&self);
}
pub(super) struct LiveApproval<'a> {
    pub shared: &'a Arc<Mutex<SharedRuntimeState>>,
    pub config: &'a crate::config::MachineConfig,
    pub peer: i64,
    pub ws: ServiceWsClient,
    pub connection: String,
    pub source: i64,
    pub grant: String,
}
impl ApprovalEffects for LiveApproval<'_> {
    fn allowed(&self, allowed: bool) -> ApprovalFuture<'_, Result<()>> {
        Box::pin(wgvpn_flow::set_wgvpn_allowed(
            self.config,
            self.peer,
            allowed,
        ))
    }
    fn send(&self, message: P2PAttemptMessage) -> ApprovalFuture<'_, Result<()>> {
        Box::pin(async move {
            send_p2p_attempt_message(
                &self.ws,
                self.connection.clone(),
                self.source,
                self.grant.clone(),
                message,
            )
            .await
        })
    }
    fn stop(&self) -> ApprovalFuture<'_, ()> {
        Box::pin(async move {
            let _ = wgvpn_flow::stop_wgvpn(self.config, self.peer).await;
        })
    }
    fn refresh(&self) {
        refresh_wgvpn_sessions(self.shared);
    }
}
