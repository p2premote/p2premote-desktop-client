use super::connection_failure_message;

// User-facing contracts, grouped by meaning. Test each code in each group, not
// just one representative: aliases must not silently fall back to "unknown".
#[test]
fn all_registered_connection_errors_have_the_expected_explanation_and_advice() {
    let cases: &[(&[&str], &str, &str, &str, bool)] = &[
        (
            &["connection.authorize.unauthenticated"],
            "登录状态已失效。",
            "Your login session has expired.",
            "Sign in again",
            false,
        ),
        (
            &["connection.authorize.forbidden"],
            "本次连接没有获得授权。",
            "This connection is not authorized.",
            "grant access",
            false,
        ),
        (
            &["connection.authorize.daily_limit_reached"],
            "已达到当前连接次数限制。",
            "The connection limit has been reached.",
            "connection limit information",
            false,
        ),
        (
            &["connection.peer_prepare.offline"],
            "对端设备当前离线。",
            "The peer device is offline.",
            "peer client is running",
            true,
        ),
        (
            &["connection.peer_prepare.timeout"],
            "对端未响应。",
            "The peer did not respond.",
            "peer client is running",
            true,
        ),
        (
            &["connection.peer_prepare.rejected"],
            "对端未能准备连接。",
            "The peer could not prepare the connection.",
            "peer client is running",
            true,
        ),
        (
            &["connection.approval.denied"],
            "本次连接请求未获允许。",
            "The connection request was not approved.",
            "confirm access approval",
            true,
        ),
        (
            &["connection.approval.timeout"],
            "等待连接确认超时。",
            "Connection approval timed out.",
            "confirm access approval",
            true,
        ),
        (
            &[
                "connection.approval.busy",
                "connection.approval.request_missing",
            ],
            "本次连接确认未完成。",
            "Connection approval was not completed.",
            "confirm access approval",
            true,
        ),
        (
            &[
                "connection.compatibility.client_upgrade_required",
                "signal.transport_select.peer_upgrade_required",
            ],
            "客户端版本不满足连接要求。",
            "A client version does not meet the connection requirements.",
            "updated to supported versions",
            false,
        ),
        (
            &[
                "connection.authorize.timeout",
                "connection.authorize.unavailable",
                "signal.send.timeout",
                "signal.send.disconnected",
                "signal.send.rejected",
                "signal.send.queue_full",
                "signal.receive.closed",
                "signal.receive.queue_full",
            ],
            "连接协调服务暂时不可用。",
            "The connection coordination service is temporarily unavailable.",
            "Check your network",
            true,
        ),
        (
            &[
                "traversal.key_exchange.timeout",
                "traversal.capability_exchange.timeout",
                "traversal.plan_negotiation.timeout",
                "traversal.round_sync.timeout",
                "traversal.round_sync.peer_closed",
                "traversal.address_exchange.timeout",
            ],
            "连接协商超时。",
            "Connection negotiation timed out.",
            "peer client is running",
            true,
        ),
        (
            &[
                "traversal.nat_detection.timeout",
                "traversal.nat_detection.no_result",
                "traversal.nat_detection.io_failed",
            ],
            "网络检测未能完成。",
            "Network detection could not be completed.",
            "Check your network",
            true,
        ),
        (
            &[
                "traversal.candidate_selection.no_common_network",
                "traversal.candidate_selection.no_usable_candidate",
                "traversal.punch.timeout",
                "traversal.punch.io_failed",
                "traversal.punch.failed",
                "traversal.lan_probe.timeout",
                "traversal.lan_probe.failed",
                "traversal.execution.exhausted",
            ],
            "本次未能建立直连。",
            "A direct connection could not be established this time.",
            "different network",
            true,
        ),
        (
            &["traversal.punch.ipv6_unavailable"],
            "本次 IPv6 连接方式不可用。",
            "IPv6 is unavailable for this connection attempt.",
            "adjust connection preferences",
            true,
        ),
        (
            &["wireguard.configure.permission_denied"],
            "连接服务缺少必要权限。",
            "A connection service lacks the required permissions.",
            "service permissions",
            false,
        ),
        (
            &[
                "wireguard.configure.backend_unavailable",
                "traversal.local_forward.invalid_address",
                "traversal.local_forward.io_failed",
                "native.jni.failed",
            ],
            "必要的连接组件不可用。",
            "A required connection component is unavailable.",
            "repair the client services",
            false,
        ),
        (
            &[
                "wireguard.configure.invalid_config",
                "wireguard.configure.failed",
                "wireguard.handshake.timeout",
                "wireguard.handshake.failed",
            ],
            "加密隧道未能就绪。",
            "The encrypted tunnel could not become ready.",
            "repair the client services",
            false,
        ),
        (
            &["runtime.config.load_failed"],
            "客户端配置未能加载。",
            "The client configuration could not be loaded.",
            "repair the client services",
            false,
        ),
        (
            &["runtime.device.not_registered"],
            "设备尚未完成注册。",
            "Device registration is incomplete.",
            "device registration",
            false,
        ),
        (
            &["connection.route_validation.overlap"],
            "共享网段与现有网络冲突。",
            "A shared subnet conflicts with an existing network.",
            "adjust the shared subnets",
            false,
        ),
        (
            &["connection.route_validation.invalid_cidr"],
            "共享网段配置无效。",
            "The shared subnet configuration is invalid.",
            "adjust the shared subnets",
            false,
        ),
        (
            &["connection.execution.network_changed"],
            "连接期间网络发生变化。",
            "The network changed while connecting.",
            "network is stable",
            true,
        ),
        (
            &["connection.execution.deadline_exceeded"],
            "连接未在规定时间内完成。",
            "The connection did not complete within the time limit.",
            "Check your network",
            true,
        ),
        (
            &[
                "signal.transport_select.invalid_selection",
                "signal.receive.invalid_frame",
                "signal.receive.context_mismatch",
                "signal.receive.stale_frame",
                "signal.receive.payload_too_large",
                "traversal.key_exchange.invalid_payload",
                "traversal.key_exchange.crypto_failed",
                "traversal.capability_exchange.invalid_payload",
                "traversal.plan_negotiation.mismatch",
                "traversal.plan_negotiation.invalid_payload",
                "traversal.round_sync.invalid_sequence",
                "traversal.address_exchange.invalid_payload",
                "traversal.address_exchange.crypto_failed",
                "native.decode.invalid_payload",
                "native.encode.failed",
            ],
            "双方连接协议协商失败。",
            "Connection protocol negotiation failed.",
            "updated to supported versions",
            false,
        ),
        (
            &[
                "connection.authorize.invalid_request",
                "native.input.invalid_argument",
            ],
            "连接请求参数无效。",
            "The connection request parameters are invalid.",
            "target device and connection settings",
            false,
        ),
    ];
    let mut seen = std::collections::HashSet::new();
    for &(codes, zh_reason, en_reason, en_advice, passive_advice) in cases {
        for &code in codes {
            assert!(seen.insert(code), "duplicate case: {code}");
            assert!(
                crate::error::ErrorCode::parse(code).is_known(),
                "typo in code: {code}"
            );
            for active in [true, false] {
                for (locale, reason) in [("zh-CN", zh_reason), ("en", en_reason)] {
                    assert_eq!(
                        connection_failure_message(Some(locale), code, active, false),
                        reason,
                        "{code}/{locale}/{active}"
                    );
                    let full = connection_failure_message(Some(locale), code, active, true);
                    assert!(
                        full.starts_with(reason) && full.len() > reason.len(),
                        "{code}: {full}"
                    );
                    assert!(
                        !full.contains("connection.failure.")
                            && !full.contains("connection.advice."),
                        "{code}: {full}"
                    );
                    assert!(
                        !full.contains(code),
                        "known errors should not expose internal codes: {full}"
                    );
                    if locale == "en" {
                        assert!(full.is_ascii(), "missing English translation: {full}");
                        assert!(
                            full.contains(if !active && passive_advice {
                                "ask the initiator to retry"
                            } else {
                                en_advice
                            }),
                            "{code}: {full}"
                        );
                    } else if !active && passive_advice {
                        assert!(full.contains("再由发起方重试"), "{code}: {full}");
                    }
                }
            }
        }
    }
}

#[test]
fn every_cancellation_stays_neutral_or_preserves_the_known_initiator() {
    for (code, zh, en) in [
        (
            "connection.execution.user_cancelled",
            "本次连接已由你取消。",
            "You cancelled this connection.",
        ),
        (
            "connection.execution.peer_cancelled",
            "对端已取消本次连接。",
            "The peer cancelled this connection.",
        ),
        (
            "traversal.execution.cancelled",
            "本次连接已取消。",
            "This connection was cancelled.",
        ),
        (
            "signal.receive.cancelled",
            "本次连接已取消。",
            "This connection was cancelled.",
        ),
    ] {
        for active in [true, false] {
            for advice in [true, false] {
                assert_eq!(connection_failure_message(None, code, active, advice), zh);
                assert_eq!(
                    connection_failure_message(Some("en"), code, active, advice),
                    en
                );
            }
        }
    }
}

#[test]
fn unclassified_errors_preserve_only_valid_codes_and_never_guess_a_root_cause() {
    // Reporting failures are not evidence that NAT traversal failed.
    for code in [
        "connection.authorize.invalid_response",
        "report.submit.timeout",
        "report.submit.unavailable",
        "report.submit.unauthorized",
        "report.submit.invalid_payload",
        "report.submit.conflict",
        "report.persist.failed",
        "runtime.execution.unclassified",
        "runtime.execution.panicked",
        "future.connection.new_reason",
    ] {
        assert!(p2premote_punch::error::valid_code(code));
        for locale in [None, Some("en")] {
            for active in [true, false] {
                let full = connection_failure_message(locale, code, active, true);
                assert!(full.contains(code));
                assert!(
                    full.contains(if locale.is_none() {
                        "联系支持并提供错误码"
                    } else {
                        "contact support with the error code"
                    }),
                    "{full}"
                );
                assert!(!full.contains("NAT") && !full.contains("运营商"));
                let reason = connection_failure_message(locale, code, active, false);
                assert!(reason.contains(code) && reason.len() < full.len());
            }
        }
    }
    for code in [
        "",
        "INVALID",
        "a.b",
        "future.secret\nhttps://private.example/token",
        "future.<script>.failure",
        "future.connection.reason_with_超长",
    ] {
        assert!(!p2premote_punch::error::valid_code(code));
        for locale in [None, Some("en")] {
            let message = connection_failure_message(locale, code, true, true);
            assert!(!message.contains("https://") && !message.contains("<script>"));
            assert!(!message.contains("错误码：") && !message.contains("Error code:"));
        }
    }
}

#[test]
fn unsupported_locales_use_the_same_chinese_explanation() {
    let expected =
        connection_failure_message(Some("zh-CN"), "wireguard.handshake.failed", true, true);
    for locale in [None, Some(""), Some("fr"), Some("en-US")] {
        assert_eq!(
            connection_failure_message(locale, "wireguard.handshake.failed", true, true),
            expected
        );
    }
}
