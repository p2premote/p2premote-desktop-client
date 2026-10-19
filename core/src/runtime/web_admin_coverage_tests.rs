use super::*;

#[cfg(not(target_os = "linux"))]
#[tokio::test]
async fn auto_start_save_failure_rolls_back_and_keeps_original_error_even_if_rollback_fails() {
    use std::cell::RefCell;
    for enabled in [false, true] {
        for fail in ["", "set", "save", "rollback"] {
            let calls = RefCell::new(vec![]);
            let result = update_auto_start_with(
                enabled,
                |value| {
                    calls.borrow_mut().push(format!("set:{value}"));
                    if fail == "set" || (fail == "rollback" && value != enabled) {
                        Err(anyhow::anyhow!("system operation rejected"))
                    } else {
                        Ok(())
                    }
                },
                async {
                    calls.borrow_mut().push("save".into());
                    if fail == "save" || fail == "rollback" {
                        Err(crate::control::CommandFailure::Structured(
                            crate::control::CommandError::new("SAVE_FAILED", false),
                        ))
                    } else {
                        Ok(Data::Ping)
                    }
                },
            )
            .await;
            let mut expected = vec![format!("set:{enabled}")];
            if fail != "set" {
                expected.push("save".into());
            }
            if fail == "save" || fail == "rollback" {
                expected.push(format!("set:{}", !enabled));
            }
            assert_eq!(*calls.borrow(), expected);
            if fail.is_empty() {
                assert_eq!(result.unwrap(), serde_json::Value::Null);
            } else {
                assert_eq!(
                    result.unwrap_err().to_string(),
                    if fail == "set" {
                        "system operation rejected"
                    } else {
                        "SAVE_FAILED"
                    }
                );
            }
        }
    }
}

#[tokio::test]
async fn command_validation_rejects_incomplete_payloads_before_dispatch() {
    let state = web_state(None);
    let cases: &[(&str, &[(&str, serde_json::Value)])] = &[
        ("set_locale", &[("locale", serde_json::json!("zh-CN"))]),
        ("set_auto_start", &[("enabled", serde_json::json!(true))]),
        (
            "register_by_email_code",
            &[
                ("username", serde_json::json!("u")),
                ("email", serde_json::json!("u@example.invalid")),
                ("password", serde_json::json!("test-only")),
                ("verification_code", serde_json::json!("123456")),
            ],
        ),
        (
            "send_registration_verification_code",
            &[("email", serde_json::json!("u@example.invalid"))],
        ),
        (
            "send_reset_password_verification_code",
            &[("email", serde_json::json!("u@example.invalid"))],
        ),
        (
            "reset_password_by_email_code",
            &[
                ("email", serde_json::json!("u@example.invalid")),
                ("verification_code", serde_json::json!("123456")),
                ("new_password", serde_json::json!("test-only")),
            ],
        ),
        (
            "login",
            &[
                ("identifier", serde_json::json!("u")),
                ("password", serde_json::json!("test-only")),
            ],
        ),
        (
            "save_login_settings",
            &[("identifier", serde_json::json!("u"))],
        ),
        ("parse_invite_info", &[("input", serde_json::json!("bad"))]),
        (
            "update_device_alias",
            &[
                ("device_id", serde_json::json!(42)),
                ("alias", serde_json::json!("unit")),
            ],
        ),
        (
            "set_device_password",
            &[
                ("device_id", serde_json::json!(42)),
                ("password", serde_json::json!("test-only")),
            ],
        ),
        (
            "start_service_active_tunnel",
            &[
                ("peer_device_id", serde_json::json!(42)),
                ("peer_device_uuid", serde_json::json!("unit")),
            ],
        ),
        (
            "start_service_anonymous_active_tunnel",
            &[
                ("connect_code", serde_json::json!("unit")),
                ("temporary_password", serde_json::json!("test-only")),
            ],
        ),
        ("delete_device", &[("device_id", serde_json::json!(42))]),
        (
            "generate_connect_code",
            &[("device_id", serde_json::json!(42))],
        ),
        (
            "stop_active_tunnel_job",
            &[("peer_device_id", serde_json::json!(42))],
        ),
        (
            "approve_inbound_tunnel",
            &[("attempt_id", serde_json::json!("unit"))],
        ),
        (
            "reject_inbound_tunnel",
            &[("attempt_id", serde_json::json!("unit"))],
        ),
        (
            "stop_service_active_tunnel",
            &[("peer_device_id", serde_json::json!(42))],
        ),
        (
            "disconnect_active_tunnel",
            &[("peer_device_id", serde_json::json!(42))],
        ),
        (
            "test_tunnel_speed",
            &[("peer_device_id", serde_json::json!(42))],
        ),
        (
            "stop_service_tunnel",
            &[("source_device_id", serde_json::json!(42))],
        ),
    ];
    for (command, fields) in cases {
        let mut args = serde_json::Map::new();
        // Deliberately never send a complete request: no persistence, network or SCM operation.
        for (name, value) in *fields {
            let error = handle_web_command(command, args.clone().into(), &state)
                .await
                .unwrap_err();
            assert!(
                error.to_string().contains(name),
                "{command}/{name}: {error}"
            );
            args.insert((*name).into(), value.clone());
        }
    }
    for command in [
        "save_connection_preferences",
        "save_service_access_policy",
        "save_wgvpn_lan_access_config",
        "set_auto_keep",
    ] {
        let error = handle_web_command(command, serde_json::json!({"unexpected": true}), &state)
            .await
            .unwrap_err();
        assert_eq!(error.to_string(), "INVALID_ARGUMENT");
    }
    assert_eq!(
        handle_web_command(
            "check_required_client_files",
            serde_json::Value::Null,
            &state
        )
        .await
        .unwrap(),
        serde_json::json!([])
    );
    assert_eq!(
        handle_web_command("listen_service_events", serde_json::Value::Null, &state)
            .await
            .unwrap(),
        serde_json::json!(true)
    );
    assert_eq!(
        handle_web_command("is_logged_in", serde_json::Value::Null, &state)
            .await
            .unwrap(),
        serde_json::json!(false)
    );
    assert!(state.shared.lock().status.active_tunnel_jobs.is_empty());
}
use axum::http::HeaderValue;

fn web_state(code: Option<&str>) -> WebAdminState {
    WebAdminState {
        shared: Arc::new(Mutex::new(SharedRuntimeState::default())),
        wake: Arc::new(Notify::new()),
        security: Arc::new(
            WebSecurityState::from_values(Some("192.0.2.1"), code.or(Some("unit-code"))).unwrap(),
        ),
        listener_revision: watch::channel(0).0,
    }
}
fn origin() -> HeaderMap {
    let mut h = HeaderMap::new();
    h.insert(
        header::ORIGIN,
        HeaderValue::from_static("http://192.0.2.1:48083"),
    );
    h.insert(header::HOST, HeaderValue::from_static("192.0.2.1:48083"));
    h
}
fn peer() -> ConnectInfo<SocketAddr> {
    ConnectInfo("192.0.2.1:12345".parse().unwrap())
}

#[tokio::test]
async fn unlock_rate_limit_expires_and_success_rotates_source_bound_session() {
    let state = web_state(Some("unit-code"));
    let response = web_auth_unlock(
        peer(),
        HeaderMap::new(),
        State(state.clone()),
        Json(WebUnlockRequest {
            security_code: "unit-code".into(),
        }),
    )
    .await;
    assert_eq!(response.status(), StatusCode::FORBIDDEN);
    for _ in 0..WEB_UNLOCK_FAILURE_LIMIT {
        assert_eq!(
            web_auth_unlock(
                peer(),
                origin(),
                State(state.clone()),
                Json(WebUnlockRequest {
                    security_code: "wrong".into()
                })
            )
            .await
            .status(),
            StatusCode::UNAUTHORIZED
        );
    }
    assert_eq!(
        web_auth_unlock(
            peer(),
            origin(),
            State(state.clone()),
            Json(WebUnlockRequest {
                security_code: "unit-code".into()
            })
        )
        .await
        .status(),
        StatusCode::TOO_MANY_REQUESTS
    );
    state
        .security
        .failures
        .lock()
        .get_mut(&peer().0.ip())
        .unwrap()
        .locked_until = Some(Instant::now() - Duration::from_secs(1));
    let response = web_auth_unlock(
        peer(),
        origin(),
        State(state.clone()),
        Json(WebUnlockRequest {
            security_code: " unit-code ".into(),
        }),
    )
    .await;
    assert_eq!(response.status(), StatusCode::OK);
    assert!(response.headers().contains_key(header::SET_COOKIE));
    assert!(state.security.failures.lock().is_empty());
    let first = state
        .security
        .session
        .lock()
        .as_ref()
        .unwrap()
        .token
        .clone();
    assert!(state
        .security
        .session_token_valid(&first, peer().0.ip(), false));
    assert!(!state
        .security
        .session_token_valid(&first, "192.0.2.2".parse().unwrap(), false));
    assert_eq!(
        web_auth_unlock(
            peer(),
            origin(),
            State(state.clone()),
            Json(WebUnlockRequest {
                security_code: "unit-code".into()
            })
        )
        .await
        .status(),
        StatusCode::OK
    );
    assert!(!state
        .security
        .session_token_valid(&first, peer().0.ip(), false));
    *state.security.security_code_hash.lock() = None;
    assert_eq!(
        web_auth_unlock(
            peer(),
            origin(),
            State(state.clone()),
            Json(WebUnlockRequest {
                security_code: String::new()
            })
        )
        .await
        .status(),
        StatusCode::OK
    );
}

#[tokio::test]
async fn security_code_change_rejects_unauthorized_missing_wrong_weak_and_unchanged_values_before_persistence(
) {
    let state = web_state(Some("unit-code"));
    let request = || WebChangeSecurityCodeRequest {
        current_security_code: None,
        new_security_code: "new-code".into(),
    };
    assert_eq!(
        web_auth_change_security_code(
            peer(),
            HeaderMap::new(),
            State(state.clone()),
            Json(request())
        )
        .await
        .status(),
        StatusCode::FORBIDDEN
    );
    assert_eq!(
        web_auth_change_security_code(peer(), origin(), State(state.clone()), Json(request()))
            .await
            .status(),
        StatusCode::UNAUTHORIZED
    );
    let now = Instant::now();
    state.security.replace_session(WebSession {
        token: "valid".into(),
        source_ip: peer().0.ip(),
        created_at: now,
        last_active_at: now,
    });
    let mut headers = origin();
    headers.insert(
        header::COOKIE,
        HeaderValue::from_str(&format!("{WEB_SESSION_COOKIE}=valid")).unwrap(),
    );
    assert_eq!(
        web_auth_change_security_code(
            peer(),
            headers.clone(),
            State(state.clone()),
            Json(request())
        )
        .await
        .status(),
        StatusCode::BAD_REQUEST
    );
    assert_eq!(
        web_auth_change_security_code(
            peer(),
            headers.clone(),
            State(state.clone()),
            Json(WebChangeSecurityCodeRequest {
                current_security_code: Some("wrong".into()),
                new_security_code: "new-code".into()
            })
        )
        .await
        .status(),
        StatusCode::UNAUTHORIZED
    );
    for new in [
        "x".to_string(),
        "x".repeat(257),
        "0000".into(),
        "unit-code".into(),
    ] {
        assert_eq!(
            web_auth_change_security_code(
                peer(),
                headers.clone(),
                State(state.clone()),
                Json(WebChangeSecurityCodeRequest {
                    current_security_code: Some("unit-code".into()),
                    new_security_code: new
                })
            )
            .await
            .status(),
            StatusCode::BAD_REQUEST
        );
    }
    assert_eq!(
        *state.security.security_code_hash.lock(),
        Some(hash_security_code("unit-code"))
    );
    assert!(state
        .security
        .session_token_valid("valid", peer().0.ip(), false));
}

#[tokio::test]
async fn auth_status_logout_and_invoke_enforce_origin_and_session_without_system_actions() {
    let state = web_state(Some("unit-code"));
    let mut headers = origin();
    let now = Instant::now();
    assert!(
        !web_auth_status(peer(), headers.clone(), State(state.clone()))
            .await
            .0
            .authenticated
    );
    state.security.replace_session(WebSession {
        token: "valid".into(),
        source_ip: peer().0.ip(),
        created_at: now,
        last_active_at: now,
    });
    headers.insert(
        header::COOKIE,
        HeaderValue::from_str(&format!("{WEB_SESSION_COOKIE}=valid")).unwrap(),
    );
    assert!(
        web_auth_status(peer(), headers.clone(), State(state.clone()))
            .await
            .0
            .authenticated
    );
    for (h, command, expected) in [
        (
            HeaderMap::new(),
            "listen_service_events",
            StatusCode::FORBIDDEN,
        ),
        (origin(), "listen_service_events", StatusCode::UNAUTHORIZED),
        (headers.clone(), "listen_service_events", StatusCode::OK),
        (
            headers.clone(),
            "unknown-command",
            StatusCode::INTERNAL_SERVER_ERROR,
        ),
    ] {
        let response = web_invoke(
            Path(command.into()),
            peer(),
            h,
            State(state.clone()),
            Json(serde_json::json!({})),
        )
        .await
        .into_response();
        assert_eq!(response.status(), expected);
    }
    assert_eq!(
        web_auth_logout(peer(), HeaderMap::new(), State(state.clone()))
            .await
            .status(),
        StatusCode::FORBIDDEN
    );
    let logged_out = web_auth_logout(peer(), headers, State(state.clone())).await;
    assert_eq!(logged_out.status(), StatusCode::OK);
    assert!(logged_out.headers()[header::SET_COOKIE]
        .to_str()
        .unwrap()
        .contains("Max-Age=0"));
    assert!(state.security.session.lock().is_none());
    assert_eq!(
        web_auth_logout(peer(), origin(), State(state.clone()))
            .await
            .status(),
        StatusCode::OK
    );
    *state.security.security_code_hash.lock() = None;
    assert!(
        web_auth_status(peer(), origin(), State(state.clone()))
            .await
            .0
            .authenticated
    );
    assert!(web_health().await.0["ok"].as_bool().unwrap());
}

#[test]
fn sessions_require_cookie_token_source_and_both_expiration_limits() {
    let security = WebSecurityState::from_values(None, Some("unit-code")).unwrap();
    let ip: IpAddr = "192.0.2.1".parse().unwrap();
    let mut headers = HeaderMap::new();
    assert!(!security.session_valid(&headers, ip, false));
    headers.insert(header::COOKIE, HeaderValue::from_static("other=value"));
    assert!(!security.session_valid(&headers, ip, false));
    headers.insert(header::COOKIE, HeaderValue::from_bytes(&[0xff]).unwrap());
    assert!(session_cookie(&headers).is_none());
    headers.insert(
        header::COOKIE,
        HeaderValue::from_str(&format!("other=x; {WEB_SESSION_COOKIE}=valid; last=y")).unwrap(),
    );
    assert_eq!(session_cookie(&headers), Some("valid"));
    assert!(!security.session_valid(&headers, ip, false));
    let now = Instant::now();
    security.replace_session(WebSession {
        token: "valid".into(),
        source_ip: ip,
        created_at: now,
        last_active_at: now - Duration::from_secs(1),
    });
    assert!(!security.session_token_valid("wrong", ip, false));
    assert!(!security.session_token_valid("valid", "192.0.2.2".parse().unwrap(), false));
    let old = security.session.lock().as_ref().unwrap().last_active_at;
    assert!(security.session_valid(&headers, ip, false));
    assert_eq!(
        security.session.lock().as_ref().unwrap().last_active_at,
        old
    );
    assert!(security.session_valid(&headers, "::ffff:192.0.2.1".parse().unwrap(), true));
    assert!(security.session.lock().as_ref().unwrap().last_active_at > old);
    security.session.lock().as_mut().unwrap().created_at =
        now - WEB_SESSION_MAX_AGE - Duration::from_secs(1);
    assert!(!security.session_valid(&headers, ip, true));
    assert!(security.session.lock().is_none());
}

#[test]
fn revocation_only_removes_the_matching_session_and_notifies_watchers() {
    let security = WebSecurityState::localhost_only();
    let now = Instant::now();
    security.remove_session("absent");
    security.replace_session(WebSession {
        token: "current".into(),
        source_ip: "127.0.0.1".parse().unwrap(),
        created_at: now,
        last_active_at: now,
    });
    let revision = *security.session_revision.borrow();
    security.remove_session("stale");
    assert_eq!(*security.session_revision.borrow(), revision);
    assert!(security.session.lock().is_some());
    security.remove_session("current");
    assert!(security.session.lock().is_none());
    assert_eq!(*security.session_revision.borrow(), revision + 1);
    security.clear_session();
    assert_eq!(*security.session_revision.borrow(), revision + 2);
}

#[test]
fn first_trust_rejects_non_unicast_sources_and_releases_expired_claims() {
    let security = WebSecurityState::localhost_only();
    for source in ["0.0.0.0", "224.0.0.1", "::", "ff02::1", "::1"] {
        assert!(!security.claim_first_trust_bootstrap_with(source.parse().unwrap(), true));
    }
    let first: IpAddr = "192.0.2.1".parse().unwrap();
    let second: IpAddr = "192.0.2.2".parse().unwrap();
    assert!(security.claim_first_trust_bootstrap_with(first, true));
    *security.bootstrap_trusted_ip.lock() = Some((
        first,
        Instant::now() - FIRST_TRUST_CLAIM_IDLE - Duration::from_secs(1),
    ));
    assert!(!security.first_trust_trusted(first));
    assert!(security.claim_first_trust_bootstrap_with(second, true));
    assert!(!security.first_trust_trusted(first));
    assert!(security.first_trust_trusted(second));
}

#[test]
fn access_configuration_and_hash_boundaries_fail_closed() {
    assert!(WebSecurityState::from_values(None, Some(&"x".repeat(256))).is_ok());
    assert!(WebSecurityState::from_values(None, Some(&"x".repeat(257))).is_err());
    assert!(WebSecurityState::from_values(Some("192.0.2.1"), Some("  ")).is_err());
    assert!(WebSecurityState::from_values(Some(" "), Some(" "))
        .unwrap()
        .first_trust_bootstrap_window_open());
    assert!(parse_persisted_security_code(&format!("sha256:{}", "g".repeat(64))).is_err());
    assert_eq!(
        parse_persisted_security_code(&format!("sha256:{}", "A0".repeat(32))).unwrap(),
        [0xa0; 32]
    );
    for cidr in [
        "127.0.0.0/8",
        "224.0.0.0/4",
        "255.255.255.255/32",
        "192.0.2.1/33",
        "2001:db8::/64",
    ] {
        assert!(parse_allowed_cidr(cidr).is_err(), "{cidr}");
    }
    let security = WebSecurityState::from_values(Some("192.0.2.0/24"), Some("code")).unwrap();
    assert!(!security.source_allowed("2001:db8::1".parse().unwrap()));
    assert!(security.source_allowed("::1".parse().unwrap()));
}

#[test]
fn web_argument_boundaries_do_not_coerce_wrong_types() {
    let args = serde_json::json!({"snake":"value","empty":"","number":42,"flag":false,"items":[" a ",null,42,"", "b"]});
    assert_eq!(arg_string(&args, &["camel", "snake"]).unwrap(), "value");
    assert!(arg_string(&args, &["number"]).is_err());
    assert!(arg_string(&args, &["missing"]).is_err());
    assert!(arg_optional_string(&args, &["empty"]).is_none());
    assert!(arg_optional_string(&args, &["number"]).is_none());
    assert_eq!(arg_optional_string(&args, &["snake"]), Some("value".into()));
    assert_eq!(arg_i64(&args, &["number"]).unwrap(), 42);
    assert!(arg_i64(&args, &["snake"]).is_err());
    assert_eq!(arg_bool(&args, &["flag"]), Some(false));
    assert_eq!(arg_bool(&args, &["number"]), None);
    assert_eq!(
        arg_string_array(&args, &["items"]),
        Some(vec!["a".into(), "b".into()])
    );
    assert_eq!(arg_string_array(&args, &["number"]), None);
}

#[test]
fn browser_origins_reject_missing_malformed_or_unmatched_authority() {
    for origin in [
        None,
        Some("not a URI"),
        Some("ftp://example.test"),
        Some("http:/missing-authority"),
        Some("http://example.test:48083"),
    ] {
        let mut headers = HeaderMap::new();
        if let Some(origin) = origin {
            headers.insert(header::ORIGIN, HeaderValue::from_str(origin).unwrap());
        }
        assert!(!web_origin_allowed(&headers));
        assert!(!web_first_trust_origin_allowed(&headers));
    }
    let mut headers = HeaderMap::new();
    headers.insert(
        header::ORIGIN,
        HeaderValue::from_static("http://example.test:48083"),
    );
    headers.insert(header::HOST, HeaderValue::from_bytes(&[0xff]).unwrap());
    assert!(!web_origin_allowed(&headers));
    headers.insert(header::HOST, HeaderValue::from_static("example.test:48083"));
    assert!(web_first_trust_origin_allowed(&headers));
    headers.insert(header::HOST, HeaderValue::from_static("other.test:48083"));
    assert!(!web_origin_allowed(&headers));
}
