//! 后端 message 本地化工具。
//!
//! 供 service 进程在生成面向用户的消息（隧道任务进度、错误描述、设备身份通知等）时，
//! 根据当前 `RuntimeStatus.locale` 选词。
//!
//! 设计要点：
//! - key 命名风格与前端 `src/i18n/messages/*.ts` 对齐（`tunnel.xxx`、`errors.xxx`），
//!   便于跨端 review 时核对。
//! - 占位符用 `{name}` 形式（与 vue-i18n 一致），由 [`localized_message`] 做简单替换。
//! - locale 缺失或非 `en` 时一律 fallback 到 zh-CN，保证默认可用。
//! - key 找不到时返回 key 本身，避免运行时 panic；可在测试中发现遗漏。
//!
//! 不在本模块范围：
//! - 老服务端 API 返回的 `msg` 仅作为兼容回退；新服务端错误通过
//!   `message_key + message_params` 在客户端本地化。
//! - 开发者日志（`info!/error!` 等），日志保持中文便于排查。

use std::collections::BTreeMap;

/// 当前支持的语言。locale 字符串归一化后只有两种：`zh-CN` / `en`。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Locale {
    ZhCn,
    En,
}

impl Locale {
    /// 从 locale 字符串解析。None 或未知值统一归一到 zh-CN（默认语言）。
    fn from_str(s: Option<&str>) -> Self {
        match s {
            Some("en") => Locale::En,
            _ => Locale::ZhCn,
        }
    }
}

/// 按 locale + key 取本地化文案，并用 `params` 替换 `{name}` 占位符。
///
/// # 参数
/// - `locale`: 来自 `RuntimeStatus.locale`；None 时按 zh-CN 处理。
/// - `key`: 词表键，如 `"tunnel.job.waiting_passive"`。
/// - `params`: 占位符替换对，如 `[("attempt", "3"), ("max", "30")]`。
///
/// # 返回
/// 命中词表则返回替换后的文案；key 缺失时返回 `key` 本身（便于发现遗漏，不 panic）。
///
/// # 示例
/// ```
/// use p2premote_core::i18n::localized_message;
/// // zh-CN（默认）
/// let m = localized_message(Some("zh-CN"), "tunnel.job.auto_established", &[]);
/// assert_eq!(m, "隧道已自动建立成功");
/// // en
/// let m = localized_message(Some("en"), "tunnel.job.auto_established", &[]);
/// assert_eq!(m, "Tunnel established automatically");
/// // 带占位符
/// let m = localized_message(Some("en"), "wgvpn.job.attempt_failed_retry",
///     &[("secs", "5"), ("reason", "timeout")]);
/// assert!(m.contains("timeout") && m.contains("5s"));
/// ```
pub fn localized_message(locale: Option<&str>, key: &str, params: &[(&str, &str)]) -> String {
    let loc = Locale::from_str(locale);
    let raw: &str = match loc {
        Locale::ZhCn => zh_message(key),
        Locale::En => en_message(key),
    };
    interpolate(raw, params)
}

/// Localize a structured API error. New servers provide message_key and
/// parameters; code mapping covers partial rollouts; msg remains the old-server
/// fallback and is never parsed as a protocol value.
pub fn localized_api_error(
    locale: Option<&str>,
    code: i32,
    message_key: Option<&str>,
    message_params: &BTreeMap<String, serde_json::Value>,
    fallback_message: &str,
) -> String {
    let owned_params: Vec<(String, String)> = message_params
        .iter()
        .map(|(key, value)| {
            let value = value
                .as_str()
                .map(ToOwned::to_owned)
                .unwrap_or_else(|| value.to_string());
            (key.clone(), value)
        })
        .collect();
    let params: Vec<(&str, &str)> = owned_params
        .iter()
        .map(|(key, value)| (key.as_str(), value.as_str()))
        .collect();

    if let Some(key) = message_key.filter(|key| !key.is_empty()) {
        let message = localized_message(locale, key, &params);
        if message != key {
            return message;
        }
    }
    if let Some(key) = api_error_key_for_code(code) {
        let message = localized_message(locale, key, &params);
        if message != key {
            return message;
        }
    }
    if !fallback_message.is_empty() {
        return fallback_message.to_string();
    }
    localized_message(
        locale,
        "server.request_failed",
        &[("code", &code.to_string())],
    )
}

fn api_error_key_for_code(code: i32) -> Option<&'static str> {
    Some(match code {
        400 => "request.bad_request",
        401 => "request.unauthorized",
        403 => "request.forbidden",
        404 => "request.not_found",
        500 => "server.internal_error",
        503 => "server.unavailable",
        1001 => "auth.invalid_credentials",
        1002 => "auth.user_not_found",
        1003 => "auth.user_already_exists",
        1004 => "auth.invalid_token",
        1005 => "auth.token_expired",
        1006 => "auth.verification_code.invalid",
        1007 => "auth.verification_code.expired",
        1008 => "auth.verification_code.required",
        1009 => "auth.email_already_exists",
        1010 => "auth.username_already_exists",
        1011 => "auth.password.too_short",
        1012 => "auth.email.invalid",
        1013 => "auth.verification_code.type_invalid",
        1014 => "auth.verification_code.send_failed",
        1015 => "auth.password.not_match",
        1016 => "auth.captcha.required",
        1017 => "auth.captcha.invalid",
        1018 => "auth.invite_code.self",
        1019 => "auth.invite_code.invalid",
        1020 => "auth.phone_registration.unsupported",
        1051 => "device.not_found",
        1052 => "device.already_registered",
        1053 => "device.not_owned",
        1054 => "device.offline",
        1055 => "device.name.required",
        1056 => "device.id.required",
        1057 => "device.uuid.required",
        1058 => "device.connect_code.invalid",
        1059 => "device.connect_code.expired",
        1060 => "device.connect_password.required",
        1061 => "device.connect_password.wrong",
        1062 => "device.anonymous_connect.disabled",
        1101 => "p2p.start_failed",
        1102 => "p2p.hole_punch_failed",
        1103 => "p2p.timeout",
        1104 => "p2p.connection_not_found",
        1105 => "p2p.connection_refused",
        1106 => "p2p.daily_limit_reached",
        1151 => "credit.insufficient",
        1152 => "credit.deduction_failed",
        1153 => "credit.record_not_found",
        1201 => "usage.record_not_found",
        1251 => "email.send_failed",
        1252 => "email.not_configured",
        1301 => "request.parameter.missing",
        1302 => "request.parameter.invalid",
        1303 => "request.too_frequent",
        1304 => "request.ip_banned",
        1305 => "request.rate_limit_exceeded",
        2001 => "membership.required",
        2002 => "membership.trial_expired",
        2003 => "membership.trial_already_used",
        2004 => "membership.expired",
        2051 => "device.limit_reached.free",
        2052 => "device.limit_reached.pro",
        2101 => "payment.disabled",
        2102 => "payment.plan.invalid",
        _ => return None,
    })
}

/// 取中文词表。未命中返回 key 本身。
fn zh_message(key: &str) -> &str {
    match key {
        // ---- 设备身份通知（RuntimeStatus.device_identity_message）----
        "device_identity.cloned_rebuilt" => {
            "检测到当前系统由另一台设备复制，已自动创建新的设备身份。"
        }
        "device_identity.manual_rebuilding" => "设备身份已手动重建，正在重新注册。",

        // ---- 主动隧道 job（ActiveTunnelJobStatus.message）----
        "tunnel.job.waiting_passive" => "等待主动端建立连接",
        "tunnel.job.attempt_progress_secs" => "第 {attempt} 次尝试超过 {secs} 秒",
        "tunnel.job.auto_established" => "隧道已自动建立成功",
        "tunnel.job.attempt_failed_retry" => "本次建立失败：{reason}；{secs} 秒后自动重试",
        "tunnel.job.ended_not_established" => "自动建立隧道已结束，仍未建立隧道",
        "tunnel.job.ended_not_established_with_reason" => {
            "自动建立隧道已结束，仍未建立隧道：{reason}"
        }
        "tunnel.job.cancelled" => "自动建立隧道已取消",
        "tunnel.job.attempt_total_secs" => "每次最多 {secs} 秒",

        // ---- 被动隧道生命周期（TunnelLifecycleStatus.message）----
        "tunnel.lifecycle.recovering" => "网络异常重连中",
        "tunnel.lifecycle.connected" => "隧道已连接",
        "tunnel.lifecycle.health_grace_expired" => {
            "网络持续异常，隧道已自动断开。请点击“建立隧道”重新连接"
        }
        "tunnel.lifecycle.disconnected" => "隧道已断开",
        "tunnel.lifecycle.passive_disconnected" => "隧道已主动断开",

        // ---- wgvpn job（WgvpnJobStatus.message）----
        "wgvpn.job.building" => "正在建立 wgvpn 隧道，第 {attempt}/{max} 次",
        "wgvpn.job.config_load_failed" => "加载配置失败: {reason}",
        "wgvpn.job.attempt_failed_retry" => "第 {attempt} 次失败: {reason}，{secs} 秒后重试",
        "wgvpn.job.attempt_progress_secs" => "第 {attempt} 次尝试超过 {secs} 秒",
        "wgvpn.job.all_attempts_failed" => "{max} 次尝试均失败",
        "wgvpn.job.cancelled" => "已取消",

        // ---- LAN 访问校验 ----
        "errors.lan_empty_when_enabled" => "开启 LAN 访问后必须填写至少一个 LAN 网段",

        // ---- 未登录/未注册等鉴权态（runtime.rs 末尾 match）----
        "errors.invalid_or_expired_token" => "无效或已过期的令牌",
        "errors.not_logged_in" => "未登录",
        "errors.device_not_registered" => "设备未注册",

        // ---- src-tauri commands 用户面向 ----
        "errors.resolve_install_dir" => "无法解析客户端安装目录",
        "errors.binary_not_found" => "未找到 {name}。请确认安装包已包含该文件，或将其部署到 {path}",
        "errors.service_check_timeout" => "后台服务检查超时，请检查服务状态和运行日志",
        "errors.service_not_running_no_public_ip" => "后台服务未启动，无法获取公网 IP",
        "errors.open_registry_run_failed" => "打开注册表 Run 键失败: {reason}",
        "errors.write_registry_failed" => "写入注册表失败: {reason}",
        "errors.version_policy_empty" => "版本策略数据为空",
        "errors.parse_version_failed" => "解析版本信息失败",
        "errors.server_status_code" => "服务器返回状态码: {status}",
        "errors.cannot_connect_update_server" => "无法连接到更新服务器",
        "errors.no_connect_code_data" => "无连接码数据",
        "errors.hole_punch_wait_timeout" => "等待打洞响应超时",
        "info.verify_code_sent" => "验证码发送成功",

        // ---- Go API structured errors ----
        "request.bad_request" => "请求参数错误",
        "request.unauthorized" => "未授权，请重新登录",
        "request.forbidden" => "禁止访问",
        "request.not_found" => "请求的资源不存在",
        "request.parameter.missing" => "缺少必要参数",
        "request.parameter.invalid" => "参数格式错误",
        "request.too_frequent" => "操作过于频繁，请稍后再试",
        "request.rate_limit_exceeded" => "请求过于频繁，请稍后再试",
        "server.internal_error" => "服务器内部错误，请稍后再试",
        "server.unavailable" => "服务暂时不可用，请稍后再试",
        "server.unknown_error" => "未知错误",
        "server.request_failed" => "请求失败（错误码 {code}）",
        "auth.invalid_credentials" => "用户名或密码错误",
        "auth.user_not_found" => "用户不存在",
        "auth.user_already_exists" => "用户已存在",
        "auth.invalid_token" => "无效的令牌",
        "auth.token_expired" => "令牌已过期，请重新登录",
        "auth.verification_code.invalid" => "验证码错误",
        "auth.verification_code.expired" => "验证码已过期",
        "auth.verification_code.required" => "请输入验证码",
        "auth.email_already_exists" => "邮箱已被使用",
        "auth.username_already_exists" => "用户名已被使用",
        "auth.password.too_short" => "密码太短",
        "auth.email.invalid" => "邮箱格式无效",
        "auth.verification_code.type_invalid" => "验证码类型无效",
        "auth.verification_code.send_failed" => "发送验证码失败",
        "auth.password.not_match" => "两次输入的密码不一致",
        "auth.captcha.required" => "请输入图片验证码",
        "auth.captcha.invalid" => "图片验证码错误",
        "auth.invite_code.self" => "不能使用自己的邀请码",
        "auth.invite_code.invalid" => "邀请码无效",
        "auth.phone_registration.unsupported" => "暂不支持手机号注册",
        "device.not_found" => "设备不存在",
        "device.already_registered" => "设备已注册",
        "device.not_owned" => "设备不属于当前用户",
        "device.offline" => "设备不在线",
        "device.name.required" => "设备名称不能为空",
        "device.id.required" => "设备 ID 不能为空",
        "device.uuid.required" => "设备 UUID 不能为空",
        "device.connect_code.invalid" => "连接码无效",
        "device.connect_code.expired" => "连接码已过期",
        "device.connect_password.required" => "请输入连接密码",
        "device.connect_password.wrong" => "连接密码错误",
        "device.anonymous_connect.disabled" => "匿名连接未启用",
        "device.limit_reached.free" => {
            "当前 Free 用户最多可注册 {limit} 台设备，升级 Pro 后最多可注册 {pro_limit} 台"
        }
        "device.limit_reached.pro" => "当前 Pro 用户最多可注册 {limit} 台设备",
        "p2p.start_failed" => "P2P 连接启动失败",
        "p2p.hole_punch_failed" => "打洞失败",
        "p2p.timeout" => "P2P 连接超时",
        "p2p.connection_not_found" => "P2P 连接不存在",
        "p2p.connection_refused" => "P2P 连接被拒绝",
        "p2p.daily_limit_reached" => "今日 P2P 连接次数已用完，请升级 Pro",
        "credit.insufficient" => "积分不足，请充值",
        "credit.deduction_failed" => "积分扣除失败",
        "credit.record_not_found" => "积分记录不存在",
        "usage.record_not_found" => "用量记录不存在",
        "email.send_failed" => "邮件发送失败",
        "email.not_configured" => "邮件服务未配置",
        "request.ip_banned" => "当前 IP 已被禁止访问",
        "membership.required" => "请升级 Pro 后使用此功能",
        "membership.trial_expired" => "试用期已结束，请升级 Pro",
        "membership.trial_already_used" => "试用机会已使用",
        "membership.expired" => "会员已过期，请续费",
        "payment.disabled" => "支付功能暂未开启",
        "payment.plan.invalid" => "无效的订阅类型",

        // ---- P2P WebSocket structured errors ----
        "p2p.attempt.config_load_failed" => "对端加载配置失败",
        "p2p.attempt.passive_start_failed" => "对端启动隧道失败",
        "p2p.attempt.hole_punch_wait_timeout" => "等待打洞响应超时",
        "p2p.attempt.invalid_lan_cidr" => "对端 LAN 网段配置无效",
        "p2p.attempt.peer_offline" => "对端设备离线",
        "p2p.attempt.user_cancelled" => "对端已取消建立隧道",
        "p2p.attempt.wireguard_handshake_failed" => "WireGuard 握手失败",
        "p2p.attempt.wireguard_config_failed" => "WireGuard 配置失败",
        "p2p.attempt.internal_error" => "对端建立隧道失败",

        // ---- start_service_active_tunnel 的进度 message（service.rs）----
        "service.start.confirming" => "正在确认后台服务状态",
        "service.start.service_ready" => "后台服务已就绪，正在创建自动建立任务",
        "service.start.sent" => "已发送给后台服务，正在自动建立隧道",
        "service.start.job_started" => "自动建立隧道任务已启动",
        "service.start.service_failed" => "后台服务返回失败: {reason}",
        "service.start.unknown_response" => "后台服务返回了未知响应",
        "service.start.ipc_failed" => "后台服务通信失败: {reason}",
        "service.start_invite.sent" => "已发送给后台服务，正在校验邀请信息并自动建立隧道",

        _ => key,
    }
}

/// 取英文词表。未命中回退到中文词表（避免英文缺失时显示 raw key）。
fn en_message(key: &str) -> &str {
    match key {
        // ---- 设备身份通知 ----
        "device_identity.cloned_rebuilt" => "Detected this system was cloned from another device. A new device identity has been created automatically.",
        "device_identity.manual_rebuilding" => "Device identity has been rebuilt manually, re-registering.",

        // ---- 主动隧道 job ----
        "tunnel.job.waiting_passive" => "Waiting for the active peer to establish the connection",
        "tunnel.job.attempt_progress_secs" => "Attempt {attempt} exceeded {secs}s",
        "tunnel.job.auto_established" => "Tunnel established automatically",
        "tunnel.job.attempt_failed_retry" => "Establishment failed: {reason}; retrying in {secs}s",
        "tunnel.job.ended_not_established" => "Auto-establishment ended; tunnel still not established",
        "tunnel.job.ended_not_established_with_reason" => "Auto-establishment ended; tunnel still not established: {reason}",
        "tunnel.job.cancelled" => "Auto-establishment cancelled",
        "tunnel.job.attempt_total_secs" => "Up to {secs}s per attempt",

        // ---- 被动隧道生命周期 ----
        "tunnel.lifecycle.recovering" => "Recovering from network issues",
        "tunnel.lifecycle.connected" => "Tunnel connected",
        "tunnel.lifecycle.health_grace_expired" => "The network did not recover, so the tunnel was closed. Click Establish tunnel to reconnect",
        "tunnel.lifecycle.disconnected" => "Tunnel disconnected",
        "tunnel.lifecycle.passive_disconnected" => "Tunnel disconnected by user",

        // ---- wgvpn job ----
        "wgvpn.job.building" => "Establishing wgvpn tunnel, attempt {attempt}/{max}",
        "wgvpn.job.config_load_failed" => "Failed to load config: {reason}",
        "wgvpn.job.attempt_failed_retry" => "Attempt {attempt} failed: {reason}; retrying in {secs}s",
        "wgvpn.job.attempt_progress_secs" => "Attempt {attempt} exceeded {secs}s",
        "wgvpn.job.all_attempts_failed" => "All {max} attempts failed",
        "wgvpn.job.cancelled" => "Cancelled",

        // ---- LAN 访问校验 ----
        "errors.lan_empty_when_enabled" => "At least one LAN subnet is required when LAN access is enabled",

        // ---- 鉴权态 ----
        "errors.invalid_or_expired_token" => "Invalid or expired token",
        "errors.not_logged_in" => "Not logged in",
        "errors.device_not_registered" => "Device not registered",

        // ---- src-tauri commands 用户面向 ----
        "errors.resolve_install_dir" => "Unable to resolve the client installation directory",
        "errors.binary_not_found" => "{name} not found. Make sure the installer includes it, or deploy it to {path}",
        "errors.service_check_timeout" => "Background service check timed out. Please verify the service status and logs.",
        "errors.service_not_running_no_public_ip" => "Background service is not running; cannot obtain the public IP",
        "errors.open_registry_run_failed" => "Failed to open the registry Run key: {reason}",
        "errors.write_registry_failed" => "Failed to write to the registry: {reason}",
        "errors.version_policy_empty" => "Version policy data is empty",
        "errors.parse_version_failed" => "Failed to parse version information",
        "errors.server_status_code" => "Server returned status code: {status}",
        "errors.cannot_connect_update_server" => "Unable to connect to the update server",
        "errors.no_connect_code_data" => "No connect-code data",
        "errors.hole_punch_wait_timeout" => "Timed out waiting for hole-punch response",
        "info.verify_code_sent" => "Verification code sent",

        // ---- Go API structured errors ----
        "request.bad_request" => "Invalid request parameters",
        "request.unauthorized" => "Unauthorized. Please sign in again.",
        "request.forbidden" => "Access forbidden",
        "request.not_found" => "The requested resource was not found",
        "request.parameter.missing" => "Required parameter missing",
        "request.parameter.invalid" => "Invalid parameter format",
        "request.too_frequent" => "Too many attempts. Please try again later.",
        "request.rate_limit_exceeded" => "Too many requests. Please try again later.",
        "server.internal_error" => "Server error. Please try again later.",
        "server.unavailable" => "Service temporarily unavailable. Please try again later.",
        "server.unknown_error" => "Unknown error",
        "server.request_failed" => "Request failed (error code {code})",
        "auth.invalid_credentials" => "Incorrect username or password",
        "auth.user_not_found" => "User not found",
        "auth.user_already_exists" => "User already exists",
        "auth.invalid_token" => "Invalid token",
        "auth.token_expired" => "Session expired. Please sign in again.",
        "auth.verification_code.invalid" => "Incorrect verification code",
        "auth.verification_code.expired" => "Verification code expired",
        "auth.verification_code.required" => "Verification code required",
        "auth.email_already_exists" => "Email address already in use",
        "auth.username_already_exists" => "Username already in use",
        "auth.password.too_short" => "Password is too short",
        "auth.email.invalid" => "Invalid email address",
        "auth.verification_code.type_invalid" => "Invalid verification-code type",
        "auth.verification_code.send_failed" => "Failed to send verification code",
        "auth.password.not_match" => "Passwords do not match",
        "auth.captcha.required" => "Image verification code required",
        "auth.captcha.invalid" => "Incorrect image verification code",
        "auth.invite_code.self" => "You cannot use your own invitation code",
        "auth.invite_code.invalid" => "Invalid invitation code",
        "auth.phone_registration.unsupported" => "Phone registration is not supported",
        "device.not_found" => "Device not found",
        "device.already_registered" => "Device already registered",
        "device.not_owned" => "This device does not belong to the current user",
        "device.offline" => "Device is offline",
        "device.name.required" => "Device name is required",
        "device.id.required" => "Device ID is required",
        "device.uuid.required" => "Device UUID is required",
        "device.connect_code.invalid" => "Invalid connection code",
        "device.connect_code.expired" => "Connection code expired",
        "device.connect_password.required" => "Connection password required",
        "device.connect_password.wrong" => "Incorrect connection password",
        "device.anonymous_connect.disabled" => "Anonymous connections are disabled",
        "device.limit_reached.free" => "Free users can register up to {limit} devices. Upgrade to Pro for up to {pro_limit}.",
        "device.limit_reached.pro" => "Pro users can register up to {limit} devices.",
        "p2p.start_failed" => "Failed to start the P2P connection",
        "p2p.hole_punch_failed" => "Hole punching failed",
        "p2p.timeout" => "P2P connection timed out",
        "p2p.connection_not_found" => "P2P connection not found",
        "p2p.connection_refused" => "P2P connection refused",
        "p2p.daily_limit_reached" => "Daily P2P connection limit reached. Upgrade to Pro.",
        "credit.insufficient" => "Insufficient credits. Please top up.",
        "credit.deduction_failed" => "Failed to deduct credits",
        "credit.record_not_found" => "Credit record not found",
        "usage.record_not_found" => "Usage record not found",
        "email.send_failed" => "Failed to send email",
        "email.not_configured" => "Email service is not configured",
        "request.ip_banned" => "This IP address is blocked",
        "membership.required" => "Upgrade to Pro to use this feature",
        "membership.trial_expired" => "Trial expired. Upgrade to Pro.",
        "membership.trial_already_used" => "The trial has already been used",
        "membership.expired" => "Membership expired. Please renew.",
        "payment.disabled" => "Payments are currently unavailable",
        "payment.plan.invalid" => "Invalid subscription plan",

        // ---- P2P WebSocket structured errors ----
        "p2p.attempt.config_load_failed" => "The peer failed to load its configuration",
        "p2p.attempt.passive_start_failed" => "The peer failed to start the tunnel",
        "p2p.attempt.hole_punch_wait_timeout" => "Timed out waiting for hole-punch response",
        "p2p.attempt.invalid_lan_cidr" => "The peer has an invalid LAN subnet configuration",
        "p2p.attempt.peer_offline" => "The peer device is offline",
        "p2p.attempt.user_cancelled" => "The peer cancelled tunnel establishment",
        "p2p.attempt.wireguard_handshake_failed" => "WireGuard handshake failed",
        "p2p.attempt.wireguard_config_failed" => "WireGuard configuration failed",
        "p2p.attempt.internal_error" => "The peer failed to establish the tunnel",

        // ---- start_service_active_tunnel 进度 ----
        "service.start.confirming" => "Confirming background service status",
        "service.start.service_ready" => "Background service ready; creating auto-establish task",
        "service.start.sent" => "Sent to the background service; auto-establishing the tunnel",
        "service.start.job_started" => "Auto-establish task started",
        "service.start.service_failed" => "Background service returned failure: {reason}",
        "service.start.unknown_response" => "Background service returned an unknown response",
        "service.start.ipc_failed" => "Background service communication failed: {reason}",
        "service.start_invite.sent" => "Sent to the background service; verifying invite and auto-establishing the tunnel",

        _ => zh_message(key), // 英文缺失时回退中文，优于 raw key
    }
}

/// 将 `{name}` 占位符替换为 params 中对应的值。
/// 占位符缺失对应的 param 时保持原样（不做处理），避免误删信息。
fn interpolate(template: &str, params: &[(&str, &str)]) -> String {
    let mut out = template.to_string();
    for (name, value) in params {
        let placeholder = format!("{{{}}}", name);
        if out.contains(&placeholder) {
            out = out.replace(&placeholder, value);
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn locale_default_is_zh() {
        assert_eq!(Locale::from_str(None), Locale::ZhCn);
        assert_eq!(Locale::from_str(Some("")), Locale::ZhCn);
        assert_eq!(Locale::from_str(Some("zh-CN")), Locale::ZhCn);
        assert_eq!(Locale::from_str(Some("fr")), Locale::ZhCn);
        assert_eq!(Locale::from_str(Some("en")), Locale::En);
    }

    #[test]
    fn zh_en_basic() {
        assert_eq!(
            localized_message(Some("zh-CN"), "tunnel.job.auto_established", &[]),
            "隧道已自动建立成功"
        );
        assert_eq!(
            localized_message(Some("en"), "tunnel.job.auto_established", &[]),
            "Tunnel established automatically"
        );
        assert_eq!(
            localized_message(Some("en"), "tunnel.lifecycle.passive_disconnected", &[],),
            "Tunnel disconnected by user"
        );
    }

    #[test]
    fn none_locale_falls_back_to_zh() {
        assert_eq!(
            localized_message(None, "tunnel.job.cancelled", &[]),
            "自动建立隧道已取消"
        );
    }

    #[test]
    fn interpolation_replaces_placeholders() {
        let m = localized_message(
            Some("zh-CN"),
            "tunnel.job.attempt_failed_retry",
            &[("reason", "打洞失败"), ("secs", "5")],
        );
        assert_eq!(m, "本次建立失败：打洞失败；5 秒后自动重试");
    }

    #[test]
    fn interpolation_en() {
        let m = localized_message(
            Some("en"),
            "wgvpn.job.building",
            &[("attempt", "3"), ("max", "30")],
        );
        assert_eq!(m, "Establishing wgvpn tunnel, attempt 3/30");
    }

    #[test]
    fn missing_param_kept_as_placeholder() {
        // 缺少 secs 参数时占位符原样保留，不崩
        let m = localized_message(
            Some("zh-CN"),
            "tunnel.job.attempt_failed_retry",
            &[("reason", "x")],
        );
        assert_eq!(m, "本次建立失败：x；{secs} 秒后自动重试");
    }

    #[test]
    fn unknown_key_returns_key() {
        assert_eq!(
            localized_message(Some("zh-CN"), "no.such.key", &[]),
            "no.such.key"
        );
        assert_eq!(
            localized_message(Some("en"), "no.such.key", &[]),
            "no.such.key"
        );
    }

    #[test]
    fn all_keys_have_en_translation() {
        // 收集 zh 词表所有 key，断言英文表都有对应翻译（未回退到中文）。
        // 这是防止批次5 之前英文遗漏的护栏；若某 key 故意无英文，加到例外清单。
        let samples = [
            "device_identity.cloned_rebuilt",
            "device_identity.manual_rebuilding",
            "tunnel.job.waiting_passive",
            "tunnel.job.attempt_progress_secs",
            "tunnel.job.auto_established",
            "tunnel.job.attempt_failed_retry",
            "tunnel.job.ended_not_established",
            "tunnel.job.ended_not_established_with_reason",
            "tunnel.job.cancelled",
            "tunnel.job.attempt_total_secs",
            "tunnel.lifecycle.recovering",
            "tunnel.lifecycle.connected",
            "tunnel.lifecycle.health_grace_expired",
            "tunnel.lifecycle.disconnected",
            "tunnel.lifecycle.passive_disconnected",
            "wgvpn.job.building",
            "wgvpn.job.config_load_failed",
            "wgvpn.job.attempt_failed_retry",
            "wgvpn.job.attempt_progress_secs",
            "wgvpn.job.all_attempts_failed",
            "wgvpn.job.cancelled",
            "errors.lan_empty_when_enabled",
            "errors.invalid_or_expired_token",
            "errors.not_logged_in",
            "errors.device_not_registered",
            "errors.resolve_install_dir",
            "errors.binary_not_found",
            "errors.service_check_timeout",
            "errors.service_not_running_no_public_ip",
            "errors.open_registry_run_failed",
            "errors.write_registry_failed",
            "errors.version_policy_empty",
            "errors.parse_version_failed",
            "errors.server_status_code",
            "errors.cannot_connect_update_server",
            "errors.no_connect_code_data",
            "info.verify_code_sent",
            "service.start.confirming",
            "service.start.service_ready",
            "service.start.sent",
            "service.start.job_started",
            "service.start.service_failed",
            "service.start.unknown_response",
            "service.start.ipc_failed",
            "service.start_invite.sent",
        ];
        for key in samples {
            let zh = zh_message(key);
            let en = en_message(key);
            assert_ne!(
                zh, en,
                "key `{key}` 的英文翻译缺失或与中文相同（回退到了中文词表）",
            );
        }
    }
}
