use anyhow::{anyhow, Result};
use p2premote_core::control::{connect_with_handshake, Data};
use serde_json::{json, Value};
use std::io::Read;
use std::time::Duration;

const MAX_REQUEST_BYTES: u64 = 64 * 1024;

fn parse_request(bytes: &[u8]) -> Result<Data> {
    if bytes.len() as u64 > MAX_REQUEST_BYTES {
        return Err(anyhow!("automation request too large"));
    }
    let command: Data =
        serde_json::from_slice(bytes).map_err(|_| anyhow!("invalid automation request"))?;
    if !matches!(
        &command,
        Data::Status
            | Data::Ping
            | Data::Login { .. }
            | Data::RegisterDevice
            | Data::GetDeviceList
            | Data::RefreshTunnelStatus
            | Data::GetAutoKeepStatuses
            | Data::StartActiveTunnelJob { .. }
            | Data::StartAnonymousActiveTunnelJob { .. }
            | Data::StopActiveTunnelJob { .. }
            | Data::StopActiveTunnel { .. }
            | Data::StopTunnel { .. }
            | Data::DisconnectActiveTunnel { .. }
            | Data::ApproveInboundTunnel { .. }
            | Data::RejectInboundTunnel { .. }
            | Data::SetAutoKeep { .. }
            | Data::SaveLoginSettings { .. }
            | Data::TestTunnelSpeed { .. }
    ) {
        return Err(anyhow!("unsupported automation command"));
    }
    Ok(command)
}

fn redact(value: &mut Value) {
    match value {
        Value::Object(object) => {
            object.retain(|key, _| {
                !matches!(
                    key.to_ascii_lowercase().as_str(),
                    "password"
                        | "temporary_password"
                        | "invite_temporary_password"
                        | "access_token"
                        | "refresh_token"
                        | "token"
                        | "secret"
                        | "access_grant"
                        | "punch_token"
                        | "private_key"
                        | "preshared_key"
                        | "security_code"
                        | "web_admin_security_code"
                        | "connect_code"
                )
            });
            for child in object.values_mut() {
                redact(child);
            }
        }
        Value::Array(items) => {
            for item in items {
                redact(item);
            }
        }
        _ => {}
    }
}

fn response_value(response: Data) -> Result<Value> {
    let mut value = match response {
        Data::CommandResponse {
            ok,
            error,
            status,
            data,
            ..
        } => json!({
            "ok": ok,
            "error_code": error.map(|error| error.code)
                .unwrap_or_else(|| if ok { String::new() } else { "runtime.execution.unclassified".into() }),
            "status": status, "data": data,
        }),
        Data::StatusChanged(status) => {
            json!({"ok":true,"error_code":"","status":status,"data":null})
        }
        _ => return Err(anyhow!("unexpected automation response")),
    };
    super::sanitize_cli_value(&mut value);
    redact(&mut value);
    Ok(value)
}

pub async fn run(timeout_secs: u64) -> Result<()> {
    let result = async {
        let mut bytes = Vec::new();
        std::io::stdin()
            .take(MAX_REQUEST_BYTES + 1)
            .read_to_end(&mut bytes)?;
        let command = parse_request(&bytes)?;
        // Do not automatically start/install the service: stopped-service tests must observe failure.
        let response = tokio::time::timeout(Duration::from_secs(timeout_secs), async {
            let mut connection = connect_with_handshake().await?;
            connection.send(&command).await?;
            connection
                .next()
                .await?
                .ok_or_else(|| anyhow!("missing automation response"))
        })
        .await
        .map_err(|_| anyhow!("automation timeout"))??;
        response_value(response)
    }
    .await;
    let value = result.unwrap_or_else(|_| {
        json!({
            "ok":false,"error_code":"automation.request.failed","status":null,"data":null
        })
    });
    println!("{}", serde_json::to_string(&value)?);
    if value["ok"] == true {
        Ok(())
    } else {
        Err(anyhow!("automation request failed"))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn request_allowlist_accepts_real_commands_and_rejects_responses_and_administration() {
        for wire in [
            r#"{"t":"Status"}"#,
            r#"{"t":"Login","c":{"identifier":"test","password":"sensitive"}}"#,
            r#"{"t":"StopActiveTunnelJob","c":{"peer_device_id":42}}"#,
        ] {
            assert!(parse_request(wire.as_bytes()).is_ok());
        }
        for wire in [
            r#"{"t":"ShutdownGracefully"}"#,
            r#"{"t":"DeleteDevice","c":{"device_id":42}}"#,
            r#"{"t":"Handshake","c":{"secret":"sensitive"}}"#,
            "invalid",
            r#"{"t":"Login"}"#,
        ] {
            let error = parse_request(wire.as_bytes()).unwrap_err();
            assert!(!error.to_string().contains("sensitive"));
        }
        assert!(parse_request(&vec![b' '; MAX_REQUEST_BYTES as usize + 1]).is_err());
    }

    #[test]
    fn structured_failures_and_legacy_failures_never_become_success() {
        for (error, expected) in [
            (
                Some(p2premote_core::control::CommandError::new(
                    "connection.approval.denied",
                    false,
                )),
                "connection.approval.denied",
            ),
            (None, "runtime.execution.unclassified"),
        ] {
            let value = response_value(Data::CommandResponse {
                ok: false,
                message: "localized".into(),
                error,
                status: None,
                data: None,
            })
            .unwrap();
            assert_eq!(value["ok"], false);
            assert_eq!(value["error_code"], expected);
            assert!(value.get("message").is_none());
        }
    }

    #[test]
    fn secrets_are_removed_recursively_and_status_remains_machine_readable() {
        let mut value = json!({"password":"sensitive","nested":[{"access_token":"sensitive",
            "private_key":"sensitive","state":"connected"}],"ok":true});
        redact(&mut value);
        assert!(!value.to_string().contains("sensitive"));
        assert_eq!(value["nested"][0]["state"], "connected");
        let value = response_value(Data::StatusChanged(Default::default())).unwrap();
        assert_eq!(value["ok"], true);
        assert!(value["status"].is_object());
        assert!(value["status"].get("invite_temporary_password").is_none());
        assert!(response_value(Data::Ping).is_err());
    }

    #[test]
    fn successful_command_preserves_data_and_drops_nested_credentials() {
        let value = response_value(Data::CommandResponse {
            ok: true,
            message: "localized".into(),
            error: None,
            status: None,
            data: Some(json!({"result":42,
                "nested":[{"refresh_token":"sensitive","PASSWORD":"sensitive"}]})),
        })
        .unwrap();
        assert_eq!(value["ok"], true);
        assert_eq!(value["error_code"], "");
        assert_eq!(value["data"]["result"], 42);
        assert!(!value.to_string().contains("sensitive"));
    }
}
