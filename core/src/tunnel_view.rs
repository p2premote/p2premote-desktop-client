use crate::control::RuntimeStatus;

/// Cross-surface tunnel snapshot. GUI/WebUI merge these collections for display;
/// CLI emits the same source collections without dropping pending or failed work.
pub fn tunnel_status_view(status: &RuntimeStatus) -> serde_json::Value {
    serde_json::json!({
        "sessions": status.wgvpn_sessions,
        "wgvpn_jobs": status.wgvpn_jobs,
        "active_tunnel_jobs": status.active_tunnel_jobs,
        "lifecycles": status.tunnel_lifecycles,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn empty_status_keeps_all_gui_status_collections() {
        assert_eq!(
            tunnel_status_view(&RuntimeStatus::default()),
            serde_json::json!({
                "sessions": [],
                "wgvpn_jobs": [],
                "active_tunnel_jobs": [],
                "lifecycles": [],
            })
        );
    }
}
