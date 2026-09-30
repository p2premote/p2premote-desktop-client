use anyhow::{Context, Result};
use once_cell::sync::Lazy;
use parking_lot::Mutex;
use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};
use sha2::{Digest, Sha256};
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};

/// Serialize configuration and runtime-state persistence inside the service process.
/// This covers the complete temp-file write and atomic replacement operation.
static CONFIG_WRITE_LOCK: Lazy<Mutex<()>> = Lazy::new(|| Mutex::new(()));

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MachineConfig {
    pub server_url: String,
    pub auth_token: Option<String>,
    pub refresh_token: Option<String>,
    pub access_token_expires_at: Option<i64>,
    pub user_email: Option<String>,
    pub device_id: Option<i64>,
    pub device_uuid: Option<String>,
    /// 桌面端克隆检测基线；移动端不采集。
    #[serde(default)]
    pub device_fingerprint: Option<String>,
    #[serde(default)]
    pub device_fingerprint_platform: Option<String>,
    #[serde(default)]
    pub device_fingerprint_version: u32,
    /// 独立 WireGuard 数据面动态库路径（Windows）。
    #[serde(default)]
    pub p2p_punch_path: String,
    /// Service 日志级别。由 service 使用，GUI 不提供自定义入口。
    #[serde(default = "default_log_level")]
    pub log_level: String,
    #[serde(default)]
    pub auto_start: bool,
    #[serde(default)]
    pub prefer_ipv6: bool,
    #[serde(default)]
    pub prefer_tcp: bool,
    /// 是否允许保留 refresh token 供后续手动或自动恢复会话。
    #[serde(default)]
    pub remember_me: bool,
    /// 是否自动登录
    #[serde(default)]
    pub auto_login: bool,
    /// wgvpn LAN 访问配置，由 service 持久化，GUI/web 只通过 IPC 读写。
    #[serde(default)]
    pub wgvpn_lan_access_enabled: bool,
    #[serde(default)]
    pub wgvpn_lan_cidrs: Vec<String>,
    /// UI 语言（"zh-CN" | "en"）。None/空 表示未设置，由前端探测系统语言后写入。
    /// service 侧仅持久化与缓存，供批次4 后端 message 按语言选词。
    #[serde(default)]
    pub locale: Option<String>,
    /// 允许访问 Web UI 的单个远端 IP。空值表示仅允许本机回环地址。
    #[serde(default)]
    pub web_admin_allowed_ip: Option<String>,
    /// Web UI 安全码。启用远端访问时必须配置；不限制复杂度。
    #[serde(default)]
    pub web_admin_security_code: Option<String>,
    /// 首次安装使用默认 Web 安全码时，进入管理界面前必须修改。
    #[serde(default)]
    pub web_admin_security_code_must_change: bool,
    /// 是否启动 Web UI 监听。默认开启，关闭后需重启 service 才会生效。
    #[serde(default = "default_webui_enabled")]
    pub webui_enabled: bool,
    /// 本机本次网络检测得到的公网 IP；service 启动时清空，防止复用陈旧网络信息。
    #[serde(default)]
    pub cached_public_ip: Option<String>,
    /// 与 cached_public_ip 配套的展示归属地。
    #[serde(default)]
    pub cached_public_ip_location: Option<String>,
    #[serde(default)]
    pub cached_public_network_checked_at: i64,
    /// 邀请协助的临时连接密码（当前服务器端生效值）。
    /// 持久化以免邀请页每次挂载都重新生成、使已分享未使用的邀请失效。
    #[serde(default)]
    pub invite_temporary_password: Option<String>,
}

/// 不属于用户配置的持久化运行时状态。
///
/// 这些字段与登录会话、设备身份和瞬态网络缓存有关，单独保存到 state.json，
/// 避免 config.json 在用户未修改配置时仍因运行状态变化而出现内容。
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(default)]
struct MachineState {
    auth_token: Option<String>,
    refresh_token: Option<String>,
    access_token_expires_at: Option<i64>,
    user_email: Option<String>,
    device_id: Option<i64>,
    device_uuid: Option<String>,
    device_fingerprint: Option<String>,
    device_fingerprint_platform: Option<String>,
    device_fingerprint_version: u32,
    cached_public_ip: Option<String>,
    cached_public_ip_location: Option<String>,
    cached_public_network_checked_at: i64,
    invite_temporary_password: Option<String>,
}

impl Default for MachineConfig {
    fn default() -> Self {
        Self {
            server_url: "https://cli.p2premote.top".to_string(),
            auth_token: None,
            refresh_token: None,
            access_token_expires_at: None,
            user_email: None,
            device_id: None,
            device_uuid: None,
            device_fingerprint: None,
            device_fingerprint_platform: None,
            device_fingerprint_version: 0,
            p2p_punch_path: default_p2p_punch_path().to_string_lossy().to_string(),
            log_level: default_log_level(),
            auto_start: false,
            prefer_ipv6: false,
            prefer_tcp: false,
            remember_me: false,
            auto_login: false,
            wgvpn_lan_access_enabled: false,
            wgvpn_lan_cidrs: Vec::new(),
            locale: None,
            web_admin_allowed_ip: None,
            web_admin_security_code: None,
            web_admin_security_code_must_change: false,
            webui_enabled: true,
            cached_public_ip: None,
            cached_public_ip_location: None,
            cached_public_network_checked_at: 0,
            invite_temporary_password: None,
        }
    }
}

/// 旧进程留下的公网网络信息不能跨 service 重启复用。
pub fn clear_cached_public_network_info() -> Result<()> {
    let mut config = load_machine_config()?;
    if config.cached_public_ip.is_some() || config.cached_public_ip_location.is_some() {
        config.cached_public_ip = None;
        config.cached_public_ip_location = None;
        config.cached_public_network_checked_at = 0;
        save_machine_config(&config)?;
    }
    Ok(())
}

pub fn platform_executable_name(base: &str) -> String {
    #[cfg(windows)]
    {
        format!("{}.exe", base)
    }

    #[cfg(not(windows))]
    {
        base.to_string()
    }
}

fn default_log_level() -> String {
    "info".to_string()
}

fn default_webui_enabled() -> bool {
    true
}

pub fn default_service_binary_name() -> &'static str {
    #[cfg(windows)]
    {
        "p2premote-service.exe"
    }

    #[cfg(not(windows))]
    {
        "p2premote-service"
    }
}

#[cfg(target_os = "linux")]
pub fn linux_install_root_dir() -> PathBuf {
    if let Ok(root) = std::env::var("P2PREMOTE_INSTALL_ROOT") {
        let path = PathBuf::from(root);
        if !path.as_os_str().is_empty() {
            return path;
        }
    }

    let exe_dir = std::env::current_exe()
        .ok()
        .and_then(|p| p.parent().map(|d| d.to_path_buf()))
        .unwrap_or_else(|| PathBuf::from("."));

    if exe_dir
        .file_name()
        .and_then(|name| name.to_str())
        .map(|name| name == "resources")
        .unwrap_or(false)
    {
        return exe_dir.parent().map(|p| p.to_path_buf()).unwrap_or(exe_dir);
    }

    exe_dir
}

#[cfg(target_os = "linux")]
pub fn linux_resources_dir() -> PathBuf {
    linux_install_root_dir().join("resources")
}

#[cfg(target_os = "linux")]
pub fn linux_run_dir() -> PathBuf {
    linux_install_root_dir().join("run")
}

#[cfg(target_os = "macos")]
pub fn macos_install_root_dir() -> PathBuf {
    std::env::var("P2PREMOTE_INSTALL_ROOT")
        .ok()
        .map(PathBuf::from)
        .filter(|path| !path.as_os_str().is_empty())
        .unwrap_or_else(|| PathBuf::from("/Library/Application Support/p2pRemote"))
}

#[cfg(target_os = "macos")]
pub fn macos_resources_dir() -> PathBuf {
    // Both the GUI and the LaunchDaemon execute from the signed `.app`
    // bundle. Keep code and dylibs inside that immutable bundle; only mutable
    // data/log/runtime files live under /Library/Application Support.
    if let Ok(executable) = std::env::current_exe() {
        for ancestor in executable.ancestors() {
            if ancestor.file_name().and_then(|name| name.to_str()) == Some("Contents") {
                return ancestor.join("Resources").join("resources");
            }
        }
    }
    macos_install_root_dir().join("resources")
}

pub fn platform_run_dir() -> PathBuf {
    #[cfg(target_os = "linux")]
    {
        return linux_run_dir();
    }
    #[cfg(target_os = "macos")]
    {
        return macos_install_root_dir().join("run");
    }
    #[cfg(windows)]
    {
        return install_root_dir().join("run");
    }
    #[allow(unreachable_code)]
    PathBuf::from("run")
}

pub fn default_wireguard_binary_name() -> &'static str {
    #[cfg(windows)]
    {
        "wireguard.exe"
    }

    #[cfg(target_os = "linux")]
    {
        "wireguard-go"
    }

    #[cfg(target_os = "macos")]
    {
        "wireguard-go"
    }

    #[cfg(all(not(windows), not(target_os = "linux"), not(target_os = "macos")))]
    {
        "wireguard"
    }
}

pub fn default_wireguard_path() -> PathBuf {
    #[cfg(target_os = "linux")]
    {
        return linux_resources_dir().join(default_wireguard_binary_name());
    }

    #[cfg(windows)]
    {
        return install_root_dir()
            .join("resources")
            .join(default_wireguard_binary_name());
    }

    #[cfg(target_os = "macos")]
    {
        return macos_resources_dir().join(default_wireguard_binary_name());
    }

    #[cfg(all(not(target_os = "linux"), not(target_os = "macos"), not(windows)))]
    {
        PathBuf::from(default_wireguard_binary_name())
    }
}

/// wg.exe（命令行工具）默认二进制名。
pub fn default_wg_binary_name() -> &'static str {
    #[cfg(windows)]
    {
        "wg.exe"
    }

    #[cfg(not(windows))]
    {
        "wg"
    }
}

/// WireGuard CLI 默认路径。Linux 固定使用安装包内的静态二进制，不依赖系统
/// wireguard-tools 或 glibc。
pub fn default_wg_path() -> PathBuf {
    #[cfg(target_os = "linux")]
    {
        return linux_resources_dir().join(default_wg_binary_name());
    }

    #[cfg(windows)]
    {
        return install_root_dir()
            .join("resources")
            .join(default_wg_binary_name());
    }

    #[cfg(target_os = "macos")]
    {
        return macos_resources_dir().join(default_wg_binary_name());
    }

    #[cfg(all(not(target_os = "linux"), not(target_os = "macos"), not(windows)))]
    {
        PathBuf::from(default_wg_binary_name())
    }
}

pub fn default_p2p_punch_binary_name() -> &'static str {
    #[cfg(windows)]
    {
        "p2premote-wg.dll"
    }

    #[cfg(target_os = "linux")]
    {
        "libp2premote-punch.a"
    }

    // macOS 使用与 Windows 同源的 p2premote-wg-ffi（wgonly）数据面动态库。
    #[cfg(all(not(windows), not(target_os = "linux")))]
    {
        "libp2premote-wg.dylib"
    }
}

pub fn default_p2p_punch_path() -> PathBuf {
    #[cfg(target_os = "linux")]
    {
        return linux_resources_dir().join(default_p2p_punch_binary_name());
    }

    #[cfg(windows)]
    {
        return install_root_dir()
            .join("resources")
            .join(default_p2p_punch_binary_name());
    }

    #[cfg(target_os = "macos")]
    {
        return macos_resources_dir().join(default_p2p_punch_binary_name());
    }

    #[cfg(all(not(target_os = "linux"), not(target_os = "macos"), not(windows)))]
    {
        PathBuf::from(default_p2p_punch_binary_name())
    }
}

pub fn default_service_path() -> PathBuf {
    #[cfg(target_os = "linux")]
    {
        return linux_resources_dir().join(default_service_binary_name());
    }

    #[cfg(target_os = "macos")]
    {
        return macos_resources_dir().join(default_service_binary_name());
    }

    #[cfg(all(not(target_os = "linux"), not(target_os = "macos")))]
    {
        PathBuf::from(default_service_binary_name())
    }
}

/// Windows 安装根目录。
///
/// Tauri 主程序位于安装根目录，service / cli 位于 `resources` 子目录。
/// 因此不能直接使用 `current_exe().parent()/data`，否则 UI 会写 `$INSTDIR/data`，
/// service 会写 `$INSTDIR/resources/data`，导致登录态和配置分裂。
#[cfg(windows)]
pub fn install_root_dir() -> PathBuf {
    let exe_dir = std::env::current_exe()
        .ok()
        .and_then(|p| p.parent().map(|d| d.to_path_buf()))
        .unwrap_or_else(|| PathBuf::from("."));

    // The Windows SCM may normalize the service image path to an 8.3 short
    // path (for example `RESOUR~1`).  Do not append another `resources`
    // component in that case: the service and bundled DLLs already live in
    // this directory.  Checking for the bundled service/DLL also covers
    // localized or otherwise non-standard short directory aliases.
    let is_resources_dir = exe_dir
        .file_name()
        .and_then(|name| name.to_str())
        .map(|name| name.eq_ignore_ascii_case("resources"))
        .unwrap_or(false)
        || (exe_dir.join("p2premote-service.exe").is_file()
            && exe_dir.join("p2premote-wg.dll").is_file());

    if is_resources_dir {
        return exe_dir.parent().map(|p| p.to_path_buf()).unwrap_or(exe_dir);
    }

    exe_dir
}

/// 运行时数据根目录。
///
/// Windows: 安装根目录下 `data` 子目录。只支持管理员安装/使用，不做多用户隔离。
/// Linux: 安装根目录下 `data` 子目录，对齐 Windows 的单 prefix 布局。
pub fn install_data_dir() -> PathBuf {
    #[cfg(windows)]
    {
        return install_root_dir().join("data");
    }

    #[cfg(target_os = "linux")]
    {
        linux_install_root_dir().join("data")
    }

    #[cfg(target_os = "macos")]
    {
        macos_install_root_dir().join("data")
    }
}

/// 运行时配置目录（machine config）。
pub fn machine_config_dir() -> PathBuf {
    install_data_dir()
}

/// 运行时日志目录。
pub fn machine_log_dir() -> PathBuf {
    // 允许通过环境变量覆盖（调试/运维用）
    if let Ok(log_dir) = std::env::var("P2PREMOTE_SERVICE_LOG_DIR") {
        let path = PathBuf::from(log_dir);
        if !path.as_os_str().is_empty() {
            return path;
        }
    }
    #[cfg(windows)]
    {
        return install_data_dir().join("logs");
    }

    #[cfg(target_os = "linux")]
    {
        linux_install_root_dir().join("logs")
    }
    #[cfg(target_os = "macos")]
    {
        macos_install_root_dir().join("logs")
    }
}

pub fn machine_config_path() -> PathBuf {
    machine_config_dir().join("config.json")
}

pub fn machine_state_path() -> PathBuf {
    machine_config_dir().join("state.json")
}

/// 安装包自带的全量配置基线。该文件只读；用户只在 data/config.json 中保存覆盖项。
pub fn default_machine_config_path() -> PathBuf {
    #[cfg(target_os = "linux")]
    {
        return linux_resources_dir().join(".p2premote_default.json");
    }

    #[cfg(windows)]
    {
        return install_root_dir()
            .join("resources")
            .join(".p2premote_default.json");
    }

    #[cfg(target_os = "macos")]
    {
        return macos_resources_dir().join(".p2premote_default.json");
    }

    #[cfg(all(not(target_os = "linux"), not(target_os = "macos"), not(windows)))]
    {
        PathBuf::from(".p2premote_default.json")
    }
}

/// 进程内只设置一次权限
static DIR_PERMISSIONS_SET: AtomicBool = AtomicBool::new(false);

pub fn ensure_machine_dirs() -> Result<()> {
    let config_dir = machine_config_dir();
    #[cfg(target_os = "linux")]
    {
        fs::create_dir_all(linux_resources_dir())
            .context("failed to create Linux resources dir")?;
        fs::create_dir_all(linux_run_dir()).context("failed to create Linux run dir")?;
    }
    #[cfg(target_os = "macos")]
    {
        fs::create_dir_all(macos_resources_dir())
            .context("failed to create macOS resources dir")?;
        fs::create_dir_all(platform_run_dir()).context("failed to create macOS run dir")?;
    }
    fs::create_dir_all(&config_dir).context("failed to create machine config dir")?;
    fs::create_dir_all(machine_log_dir()).context("failed to create machine log dir")?;
    if !DIR_PERMISSIONS_SET.swap(true, Ordering::SeqCst) {
        configure_machine_data_dir_permissions(&config_dir);
        #[cfg(target_os = "macos")]
        configure_macos_log_dir_permissions();
    }
    Ok(())
}

/// macOS admin 组的 gid。root 后台服务据此把 machine logs 目录与 IPC 套接字
/// 共享给 GUI 进程（本地 GUI 用户默认属于 admin 组）。
///
/// SAFETY: getgrnam 返回指向静态缓冲的指针，非线程安全；仅在进程启动路径
/// （服务/GUI 初始化阶段，单线程时）调用。
#[cfg(target_os = "macos")]
pub(crate) fn macos_admin_group_gid() -> Option<u32> {
    unsafe {
        let name = b"admin\0";
        let group = libc::getgrnam(name.as_ptr().cast());
        if group.is_null() {
            None
        } else {
            Some((*group).gr_gid)
        }
    }
}

/// macOS：machine logs 目录由 root 后台服务与普通用户 GUI 共写。
/// root 创建的目录默认 root:wheel 0755，GUI 无法写入日志；收敛为 admin 组
/// 可写（0770）。GUI（p2premote-*.log）与服务（p2premote-service-*.log）
/// 的日志文件名不同，互不冲突。
#[cfg(target_os = "macos")]
fn configure_macos_log_dir_permissions() {
    use std::os::unix::fs::PermissionsExt;

    let dir = machine_log_dir();
    match macos_admin_group_gid() {
        Some(gid) => {
            if let Err(error) = std::os::unix::fs::chown(&dir, None, Some(gid)) {
                tracing::warn!("[config] failed to set machine log dir group: {error}");
            }
        }
        None => tracing::warn!("[config] failed to resolve admin group gid"),
    }
    if let Err(error) = fs::set_permissions(&dir, fs::Permissions::from_mode(0o770)) {
        tracing::warn!("[config] failed to set machine log dir permissions: {error}");
    }
}

/// 初始化机器级数据目录的访问权限。
///
/// Windows 服务以 SYSTEM 身份写入 machine config 和运行日志，而桌面 GUI 以
/// 普通用户身份启动，也需要在该目录下创建日志、访问运行期数据。因此保留
/// SYSTEM/Administrators 的完全控制，同时给内置 Users 组授予可继承的修改权限。
fn configure_machine_data_dir_permissions(dir: &Path) {
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        const CREATE_NO_WINDOW: u32 = 0x08000000;

        let dir_str = match dir.to_str() {
            Some(s) => s,
            None => return,
        };

        let output = std::process::Command::new("icacls")
            .args([
                dir_str,
                "/inheritance:r",
                "/grant:r",
                "SYSTEM:(OI)(CI)F",
                "/grant:r",
                "Administrators:(OI)(CI)F",
                "/grant:r",
                "*S-1-5-32-545:(OI)(CI)M",
            ])
            .creation_flags(CREATE_NO_WINDOW)
            .output();

        match output {
            Ok(out) if out.status.success() => {
                tracing::info!("[config] configured machine data directory permissions: {}", dir_str);
            }
            Ok(out) => {
                // 非 admin 进程会失败，service 以 SYSTEM 运行时下次启动会修复
                tracing::debug!(
                    "[config] failed to configure directory permissions (non-admin?): {}",
                    String::from_utf8_lossy(&out.stderr)
                );
            }
            Err(e) => {
                tracing::debug!("[config] failed to run icacls: {}", e);
            }
        }
    }

    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        // 仅 service 所属用户可访问（与 Windows 下"仅 SYSTEM+Admins"语义一致）。
        // machine config 含认证 token，不能全局可读写（旧版 0o777 是安全隐患）。
        if let Err(e) = fs::set_permissions(dir, fs::Permissions::from_mode(0o700)) {
            tracing::warn!("[config] chmod 700 failed: {}", e);
        }
    }
}

pub fn load_machine_config() -> Result<MachineConfig> {
    let defaults = load_default_machine_config()?;
    let path = machine_config_path();
    if !path.exists() {
        return Ok(apply_machine_state(defaults, load_machine_state()?));
    }

    let content = fs::read_to_string(&path)
        .with_context(|| format!("failed to read machine config: {}", path.display()))?;
    let mut overrides = parse_config_value(&content)
        .with_context(|| format!("failed to parse machine config: {}", path.display()))?;
    let legacy_state = extract_state_fields(&mut overrides);
    let state_path = machine_state_path();
    let state = if state_path.exists() {
        load_machine_state()?
    } else if let Some(legacy_state) = legacy_state.as_ref() {
        let state: MachineState = serde_json::from_value(legacy_state.clone())
            .context("invalid legacy runtime state in config.json")?;
        save_machine_state(&state)?;
        state
    } else {
        MachineState::default()
    };
    if legacy_state.is_some() {
        // 仅移除已迁移的状态字段，保留旧全量 config.json 的其余字段，避免在此处
        // 混入默认配置差异化迁移。
        atomic_write_json(&path, &overrides)?;
    }
    let config = merge_machine_config(defaults, overrides)?;
    Ok(apply_machine_state(config, state))
}

/// 配置文件允许 JSONC 注释和尾随逗号，便于用户直接维护 `config.json`。
fn parse_machine_config(content: &str) -> Result<MachineConfig> {
    json5::from_str::<MachineConfig>(content).context("invalid JSONC configuration")
}

fn parse_config_value(content: &str) -> Result<Value> {
    let value = json5::from_str::<Value>(content).context("invalid JSONC configuration")?;
    if !value.is_object() {
        anyhow::bail!("configuration root must be an object");
    }
    Ok(value)
}

fn load_default_machine_config() -> Result<MachineConfig> {
    const EMBEDDED_DEFAULTS: &str =
        include_str!("../../src-tauri/resources/.p2premote_default.json");

    let path = default_machine_config_path();
    let content = if path.exists() {
        fs::read_to_string(&path)
            .with_context(|| format!("failed to read default machine config: {}", path.display()))?
    } else {
        EMBEDDED_DEFAULTS.to_string()
    };
    let mut config = parse_machine_config(&content)
        .with_context(|| format!("failed to parse default machine config: {}", path.display()))?;
    // 安装路径随平台和部署目录变化，默认文件中的空值表示使用本机解析出的资源路径。
    if config.p2p_punch_path.trim().is_empty() {
        config.p2p_punch_path = default_p2p_punch_path().to_string_lossy().to_string();
    }
    Ok(config)
}

fn merge_machine_config(defaults: MachineConfig, overrides: Value) -> Result<MachineConfig> {
    let mut merged =
        serde_json::to_value(defaults).context("failed to serialize default config")?;
    let default_object = merged
        .as_object_mut()
        .context("default configuration must be an object")?;
    for (key, value) in overrides
        .as_object()
        .context("configuration root must be an object")?
    {
        default_object.insert(key.clone(), value.clone());
    }
    serde_json::from_value(merged).context("invalid configuration override")
}

fn config_overrides(config: &MachineConfig, defaults: &MachineConfig) -> Result<Value> {
    let current = serde_json::to_value(config).context("failed to serialize machine config")?;
    let default_values = serde_json::to_value(defaults).context("failed to serialize defaults")?;
    let current = current
        .as_object()
        .context("machine configuration must be an object")?;
    let default_values = default_values
        .as_object()
        .context("default configuration must be an object")?;
    let mut overrides = Map::new();
    for (key, value) in current {
        if is_machine_state_field(key) {
            continue;
        }
        if default_values.get(key) != Some(value) {
            overrides.insert(key.clone(), value.clone());
        }
    }
    Ok(Value::Object(overrides))
}

pub fn save_machine_config(config: &MachineConfig) -> Result<()> {
    ensure_machine_dirs()?;
    let defaults = load_default_machine_config()?;
    let overrides = config_overrides(config, &defaults)?;
    atomic_write_json(&machine_config_path(), &overrides)?;
    save_machine_state(&machine_state_from_config(config))
}

fn is_machine_state_field(key: &str) -> bool {
    matches!(
        key,
        "auth_token"
            | "refresh_token"
            | "access_token_expires_at"
            | "user_email"
            | "device_id"
            | "device_uuid"
            | "device_fingerprint"
            | "device_fingerprint_platform"
            | "device_fingerprint_version"
            | "cached_public_ip"
            | "cached_public_ip_location"
            | "cached_public_network_checked_at"
            | "invite_temporary_password"
    )
}

fn extract_state_fields(overrides: &mut Value) -> Option<Value> {
    let values = overrides.as_object_mut()?;
    let mut state = Map::new();
    let keys: Vec<String> = values
        .keys()
        .filter(|key| is_machine_state_field(key))
        .cloned()
        .collect();
    for key in keys {
        if let Some(value) = values.remove(&key) {
            state.insert(key, value);
        }
    }
    (!state.is_empty()).then_some(Value::Object(state))
}

fn load_machine_state() -> Result<MachineState> {
    let path = machine_state_path();
    if !path.exists() {
        return Ok(MachineState::default());
    }
    let content = fs::read_to_string(&path)
        .with_context(|| format!("failed to read machine state: {}", path.display()))?;
    json5::from_str(&content)
        .context("invalid JSONC state configuration")
        .with_context(|| format!("failed to parse machine state: {}", path.display()))
}

fn save_machine_state(state: &MachineState) -> Result<()> {
    atomic_write_json(&machine_state_path(), state)
}

fn machine_state_from_config(config: &MachineConfig) -> MachineState {
    MachineState {
        auth_token: config.auth_token.clone(),
        refresh_token: config.refresh_token.clone(),
        access_token_expires_at: config.access_token_expires_at,
        user_email: config.user_email.clone(),
        device_id: config.device_id,
        device_uuid: config.device_uuid.clone(),
        device_fingerprint: config.device_fingerprint.clone(),
        device_fingerprint_platform: config.device_fingerprint_platform.clone(),
        device_fingerprint_version: config.device_fingerprint_version,
        cached_public_ip: config.cached_public_ip.clone(),
        cached_public_ip_location: config.cached_public_ip_location.clone(),
        cached_public_network_checked_at: config.cached_public_network_checked_at,
        invite_temporary_password: config.invite_temporary_password.clone(),
    }
}

fn apply_machine_state(mut config: MachineConfig, state: MachineState) -> MachineConfig {
    config.auth_token = state.auth_token;
    config.refresh_token = state.refresh_token;
    config.access_token_expires_at = state.access_token_expires_at;
    config.user_email = state.user_email;
    config.device_id = state.device_id;
    config.device_uuid = state.device_uuid;
    config.device_fingerprint = state.device_fingerprint;
    config.device_fingerprint_platform = state.device_fingerprint_platform;
    config.device_fingerprint_version = state.device_fingerprint_version;
    config.cached_public_ip = state.cached_public_ip;
    config.cached_public_ip_location = state.cached_public_ip_location;
    config.cached_public_network_checked_at = state.cached_public_network_checked_at;
    config.invite_temporary_password = state.invite_temporary_password;
    config
}

/// 清除凭据：token/refresh/expires/user_email/device 绑定。
///
/// 用于登出——token 必须清除，否则"登出"后仍可凭 refresh token 免密恢复会话。
/// remember_me/auto_login 是用户对登录方式的偏好而非凭据，保留：
/// 结束会话和清除偏好是两件事，静默重置偏好会让"记住密码/自动登录"
/// 在用户无感知时失效；取消偏好只能由用户在登录页手动取消勾选。
pub fn clear_machine_credentials(config: &mut MachineConfig) {
    config.auth_token = None;
    config.refresh_token = None;
    config.access_token_expires_at = None;
    config.user_email = None;
    config.device_id = None;
}

pub fn ensure_machine_config() -> Result<MachineConfig> {
    ensure_machine_dirs()?;
    Ok(load_machine_config().unwrap_or_default())
}

/// 从机器指纹派生 IPC control secret，不存盘
pub fn derive_control_secret() -> String {
    let fingerprint = get_machine_fingerprint();
    let mut hasher = Sha256::new();
    hasher.update(b"p2premote-ipc-secret:");
    hasher.update(fingerprint.as_bytes());
    let digest = hasher.finalize();
    digest
        .iter()
        .map(|b| format!("{:02x}", b))
        .take(32)
        .collect()
}

/// 跨平台机器指纹，仅用于派生本机 IPC control secret。
///
/// Windows 读 HKLM MachineGuid（SYSTEM 与普通用户均可读，与进程身份无关），
/// 因此 service 与 UI 能算出同一指纹。
pub fn get_machine_fingerprint() -> String {
    #[cfg(windows)]
    {
        use winreg::enums::HKEY_LOCAL_MACHINE;
        use winreg::RegKey;
        let hklm = RegKey::predef(HKEY_LOCAL_MACHINE);
        if let Ok(key) = hklm.open_subkey(r"SOFTWARE\Microsoft\Cryptography") {
            if let Ok(guid) = key.get_value::<String, _>("MachineGuid") {
                return guid.trim().to_string();
            }
        }
    }

    #[cfg(target_os = "linux")]
    {
        if let Ok(content) = std::fs::read_to_string("/etc/machine-id") {
            let id = content.trim().to_string();
            if !id.is_empty() {
                return id;
            }
        }
        if let Ok(content) = std::fs::read_to_string("/var/lib/dbus/machine-id") {
            let id = content.trim().to_string();
            if !id.is_empty() {
                return id;
            }
        }
    }

    #[cfg(target_os = "macos")]
    {
        if let Ok(output) = std::process::Command::new("/usr/sbin/ioreg")
            .args(["-rd1", "-c", "IOPlatformExpertDevice"])
            .output()
        {
            if output.status.success() {
                if let Some(uuid) = parse_ioplatform_uuid(&String::from_utf8_lossy(&output.stdout))
                {
                    return uuid;
                }
            }
        }
    }

    hostname::get()
        .map(|h| h.to_string_lossy().to_string())
        .unwrap_or_else(|_| "unknown".to_string())
}

#[cfg(any(target_os = "macos", test))]
fn parse_ioplatform_uuid(output: &str) -> Option<String> {
    output.lines().find_map(|line| {
        let (key, value) = line.split_once('=')?;
        if !key.contains("\"IOPlatformUUID\"") {
            return None;
        }
        let value = value.trim().trim_matches('"').trim();
        (!value.is_empty()).then(|| value.to_string())
    })
}

pub fn atomic_write_json<T: Serialize>(path: &Path, value: &T) -> Result<()> {
    let _write_guard = CONFIG_WRITE_LOCK.lock();

    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)
            .with_context(|| format!("failed to create directory: {}", parent.display()))?;
    }

    let json = serde_json::to_vec_pretty(value).context("failed to serialize json")?;
    let tmp_suffix = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_nanos())
        .unwrap_or_default();
    let file_name = path
        .file_name()
        .map(|name| name.to_string_lossy())
        .unwrap_or_else(|| "config.json".into());
    let tmp_path = path.with_file_name(format!(
        "{}.{}.{}.tmp",
        file_name,
        std::process::id(),
        tmp_suffix
    ));
    fs::write(&tmp_path, &json)
        .with_context(|| format!("failed to write temp file: {}", tmp_path.display()))?;

    if let Err(error) = replace_file_atomically(&tmp_path, path) {
        tracing::error!(
            target = %path.display(),
            temp = %tmp_path.display(),
            error = %error,
            "[config] atomic config replacement failed; temporary file retained"
        );
        return Err(error).with_context(|| {
            format!(
                "failed to atomically replace {} with {}; temporary file retained",
                path.display(),
                tmp_path.display()
            )
        });
    }

    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        // 仅 service 所属用户可读写（machine config 含认证 token）
        let _ = fs::set_permissions(path, fs::Permissions::from_mode(0o600));
    }
    Ok(())
}

#[cfg(windows)]
fn replace_file_atomically(source: &Path, target: &Path) -> std::io::Result<()> {
    use std::os::windows::ffi::OsStrExt;
    use windows_sys::Win32::Storage::FileSystem::{
        MoveFileExW, MOVEFILE_REPLACE_EXISTING, MOVEFILE_WRITE_THROUGH,
    };

    let source_wide: Vec<u16> = source.as_os_str().encode_wide().chain(Some(0)).collect();
    let target_wide: Vec<u16> = target.as_os_str().encode_wide().chain(Some(0)).collect();
    let result = unsafe {
        MoveFileExW(
            source_wide.as_ptr(),
            target_wide.as_ptr(),
            MOVEFILE_REPLACE_EXISTING | MOVEFILE_WRITE_THROUGH,
        )
    };
    if result == 0 {
        Err(std::io::Error::last_os_error())
    } else {
        Ok(())
    }
}

#[cfg(not(windows))]
fn replace_file_atomically(source: &Path, target: &Path) -> std::io::Result<()> {
    fs::rename(source, target)
}

#[cfg(test)]
mod wgvpn_tests {
    use super::*;

    #[test]
    fn concurrent_atomic_writes_always_leave_valid_json() {
        let dir = std::env::temp_dir().join(format!(
            "p2premote-config-write-test-{}",
            std::process::id()
        ));
        fs::create_dir_all(&dir).unwrap();
        let path = dir.join("config.json");

        let writers: Vec<_> = (0..16)
            .map(|value| {
                let path = path.clone();
                std::thread::spawn(move || {
                    atomic_write_json(&path, &serde_json::json!({ "value": value })).unwrap();
                })
            })
            .collect();
        for writer in writers {
            writer.join().unwrap();
        }

        let persisted: Value = serde_json::from_slice(&fs::read(&path).unwrap()).unwrap();
        assert!(persisted.get("value").and_then(Value::as_i64).is_some());
        fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn parses_macos_ioplatform_uuid() {
        let output = r#"    | "IOPlatformUUID" = "01234567-89AB-CDEF-0123-456789ABCDEF""#;
        assert_eq!(
            parse_ioplatform_uuid(output).as_deref(),
            Some("01234567-89AB-CDEF-0123-456789ABCDEF")
        );
    }

    #[test]
    fn default_config_has_bundled_punch_path() {
        let cfg = MachineConfig::default();
        assert!(cfg.webui_enabled);
        assert!(!cfg.p2p_punch_path.is_empty(), "p2p_punch_path 不能为空");
    }

    #[test]
    fn machine_config_accepts_jsonc_comments_and_trailing_commas() {
        let json = serde_json::to_string_pretty(&MachineConfig::default()).unwrap();
        let jsonc = format!(
            "// 用户可维护的服务配置\n{}",
            json.replacen("\n}", ",\n  // 保留尾随逗号\n}", 1)
        );
        let config = parse_machine_config(&jsonc).unwrap();
        assert!(config.webui_enabled);
    }

    #[test]
    fn config_overrides_only_include_values_different_from_defaults() {
        let defaults = MachineConfig::default();
        let mut config = defaults.clone();
        config.auto_start = true;
        config.webui_enabled = false;

        let overrides = config_overrides(&config, &defaults).unwrap();
        assert_eq!(
            overrides,
            serde_json::json!({
                "auto_start": true,
                "webui_enabled": false,
            })
        );
    }

    #[test]
    fn config_overrides_exclude_runtime_state() {
        let defaults = MachineConfig::default();
        let mut config = defaults.clone();
        config.auto_start = true;
        config.auth_token = Some("access-token".to_string());
        config.device_id = Some(42);
        config.cached_public_ip = Some("203.0.113.8".to_string());

        assert_eq!(
            config_overrides(&config, &defaults).unwrap(),
            serde_json::json!({ "auto_start": true })
        );
    }

    #[test]
    fn legacy_config_state_is_extracted_without_touching_user_overrides() {
        let mut overrides = serde_json::json!({
            "auto_start": true,
            "auth_token": "access-token",
            "device_id": 42,
        });

        let state = extract_state_fields(&mut overrides).unwrap();
        assert_eq!(overrides, serde_json::json!({ "auto_start": true }));
        assert_eq!(
            state,
            serde_json::json!({
                "auth_token": "access-token",
                "device_id": 42,
            })
        );
    }

    #[test]
    fn runtime_state_is_applied_after_configuration_overrides() {
        let config = apply_machine_state(
            MachineConfig::default(),
            MachineState {
                auth_token: Some("access-token".to_string()),
                device_uuid: Some("device-uuid".to_string()),
                ..MachineState::default()
            },
        );

        assert_eq!(config.auth_token.as_deref(), Some("access-token"));
        assert_eq!(config.device_uuid.as_deref(), Some("device-uuid"));
    }

    #[test]
    fn config_overrides_merge_on_top_of_defaults() {
        let defaults = MachineConfig::default();
        let config = merge_machine_config(
            defaults.clone(),
            serde_json::json!({
                "server_url": "https://example.test",
                "webui_enabled": false,
            }),
        )
        .unwrap();

        assert_eq!(config.server_url, "https://example.test");
        assert!(!config.webui_enabled);
        assert_eq!(config.log_level, defaults.log_level);
    }

    #[test]
    fn bundled_default_config_parses_as_jsonc() {
        let defaults = load_default_machine_config().unwrap();
        assert!(defaults.webui_enabled);
        assert!(!defaults.p2p_punch_path.is_empty());
    }

    #[test]
    fn default_wg_path_returns_exe_on_windows() {
        let path = default_wg_path();
        #[cfg(windows)]
        assert!(path.to_string_lossy().ends_with("wg.exe"));
        #[cfg(not(windows))]
        assert!(path.to_string_lossy().ends_with("wg"));
    }

    #[test]
    fn default_wireguard_path_returns_exe_on_windows() {
        let path = default_wireguard_path();
        #[cfg(windows)]
        assert!(path.to_string_lossy().ends_with("wireguard.exe"));
        #[cfg(not(windows))]
        assert!(path.to_string_lossy().ends_with("wireguard"));
    }

    #[test]
    fn default_p2p_punch_path_returns_dynamic_library_name() {
        let path = default_p2p_punch_path();
        #[cfg(windows)]
        assert!(path.to_string_lossy().ends_with("p2premote-wg.dll"));
        #[cfg(target_os = "linux")]
        assert!(path.to_string_lossy().ends_with("libp2premote-punch.a"));
        #[cfg(target_os = "macos")]
        assert!(path.to_string_lossy().ends_with("libp2premote-wg.dylib"));
    }
}
