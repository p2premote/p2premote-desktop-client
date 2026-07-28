//! WireGuard VPN 模块。
//!
//! gonc 负责外层加密 P2P UDP 数据面，WireGuard 负责内层虚拟网卡和 peer 管理。

use anyhow::{anyhow, Context, Result};
use base64::Engine;
#[cfg(target_os = "linux")]
use parking_lot::Mutex;
#[cfg(target_os = "linux")]
use std::collections::HashMap;
use std::collections::HashSet;
use std::io::Read;
use std::path::Path;
use std::path::PathBuf;
use std::process::{Child, Command, Output, Stdio};
#[cfg(target_os = "linux")]
use std::sync::OnceLock;
use std::time::Duration;
use wait_timeout::ChildExt;
use x25519_dalek::{PublicKey, StaticSecret};

const WIREGUARD_COMMAND_TIMEOUT: Duration = Duration::from_secs(15);

#[cfg(target_os = "linux")]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LinuxWireGuardBackend {
    Kernel,
    Userspace,
}

#[cfg(target_os = "linux")]
impl LinuxWireGuardBackend {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Kernel => "kernel",
            Self::Userspace => "userspace",
        }
    }
}

#[cfg(target_os = "linux")]
static LINUX_WIREGUARD_BACKEND: OnceLock<LinuxWireGuardBackend> = OnceLock::new();
#[cfg(target_os = "linux")]
static LINUX_USERSPACE_PROCESSES: once_cell::sync::Lazy<Mutex<HashMap<String, Child>>> =
    once_cell::sync::Lazy::new(|| Mutex::new(HashMap::new()));

/// 在 service 启动时探测 Linux 内核 WireGuard 能力，并固定本进程使用的后端。
/// 临时内核接口能够创建即选择 kernel；配置命令固定使用安装包内静态 `wg`，
/// 因此系统是否安装 wireguard-tools 不参与能力判断。
#[cfg(target_os = "linux")]
pub fn initialize_linux_wireguard_backend() -> LinuxWireGuardBackend {
    *LINUX_WIREGUARD_BACKEND.get_or_init(|| {
        let probe_name = format!("p2prwg{}", std::process::id());
        let probe = run_command_with_timeout(
            Command::new("ip").args(["link", "add", &probe_name, "type", "wireguard"]),
            "probe kernel WireGuard",
        );
        let interface_supported = matches!(probe, Ok(ref output) if output.status.success());
        if interface_supported {
            let _ = run_command_with_timeout(
                Command::new("ip").args(["link", "del", &probe_name]),
                "remove WireGuard probe interface",
            );
        }
        if interface_supported {
            LinuxWireGuardBackend::Kernel
        } else {
            LinuxWireGuardBackend::Userspace
        }
    })
}

#[cfg(target_os = "linux")]
fn linux_wireguard_backend() -> LinuxWireGuardBackend {
    initialize_linux_wireguard_backend()
}

fn wait_child_with_timeout(mut child: Child, label: &str) -> Result<Output> {
    // 关闭父进程持有的 stdin，避免等待以 EOF 为结束标志的子进程永久不退出。
    drop(child.stdin.take());
    let status = match child
        .wait_timeout(WIREGUARD_COMMAND_TIMEOUT)
        .with_context(|| format!("failed waiting for {}", label))?
    {
        Some(status) => status,
        None => {
            let _ = child.kill();
            let _ = child.wait();
            return Err(anyhow!(
                "{} timed out after {} seconds",
                label,
                WIREGUARD_COMMAND_TIMEOUT.as_secs()
            ));
        }
    };
    let mut stdout = Vec::new();
    let mut stderr = Vec::new();
    if let Some(mut pipe) = child.stdout.take() {
        pipe.read_to_end(&mut stdout)
            .with_context(|| format!("failed reading {} stdout", label))?;
    }
    if let Some(mut pipe) = child.stderr.take() {
        pipe.read_to_end(&mut stderr)
            .with_context(|| format!("failed reading {} stderr", label))?;
    }
    Ok(Output {
        status,
        stdout,
        stderr,
    })
}

pub(crate) fn run_command_with_timeout(command: &mut Command, label: &str) -> Result<Output> {
    command.stdout(Stdio::piped()).stderr(Stdio::piped());
    let child = command
        .spawn()
        .with_context(|| format!("failed to start {}", label))?;
    wait_child_with_timeout(child, label)
}

/// WireGuard 公钥校验：32 字节 base64 编码，固定 44 字符。
///
/// 用于校验从对端 P2P 接收到的公钥，防止格式错误导致配置生成失败。
pub fn validate_public_key(key: &str) -> Result<()> {
    let trimmed = key.trim();
    if trimmed.len() != 44 {
        return Err(anyhow!(
            "invalid public key length: expected 44, got {}",
            trimmed.len()
        ));
    }
    // WireGuard 公钥末尾固定是 '='（base64 padding）
    if !trimmed.ends_with('=') {
        return Err(anyhow!("invalid public key: must end with '='"));
    }
    // 校验是否为合法 base64
    base64::engine::general_purpose::STANDARD
        .decode(trimmed.as_bytes())
        .map_err(|e| anyhow!("invalid public key base64: {}", e))?;
    Ok(())
}

/// 生成 WireGuard 密钥对。Linux 内核后端调用系统 `wg`；用户态后端和
/// Windows 直接使用 Rust X25519 实现，不依赖外部 WireGuard 工具。
/// 返回 (private_key, public_key)，均为 base64 字符串。
pub fn generate_keypair(wg_exe: &str) -> Result<(String, String)> {
    #[cfg(not(target_os = "linux"))]
    {
        let _ = wg_exe;
        generate_userspace_keypair()
    }

    #[cfg(target_os = "linux")]
    if linux_wireguard_backend() == LinuxWireGuardBackend::Userspace {
        return generate_userspace_keypair();
    }

    #[cfg(target_os = "linux")]
    {
        // 1. 生成私钥
        let priv_output = run_command_with_timeout(
            Command::new(wg_exe).arg("genkey"),
            &format!("{} genkey", wg_exe),
        )?;
        if !priv_output.status.success() {
            return Err(anyhow!(
                "wg genkey failed: {}",
                String::from_utf8_lossy(&priv_output.stderr)
            ));
        }
        let private_key = String::from_utf8_lossy(&priv_output.stdout)
            .trim()
            .to_string();

        // 2. 用私钥派生公钥
        let mut child = Command::new(wg_exe)
            .arg("pubkey")
            .stdin(std::process::Stdio::piped())
            .stdout(std::process::Stdio::piped())
            .stderr(std::process::Stdio::piped())
            .spawn()
            .with_context(|| format!("failed to spawn '{} pubkey'", wg_exe))?;
        {
            use std::io::Write;
            if let Some(stdin) = child.stdin.as_mut() {
                stdin
                    .write_all(private_key.as_bytes())
                    .context("failed to feed private key to pubkey")?;
            }
        }
        let output = wait_child_with_timeout(child, &format!("{} pubkey", wg_exe))?;
        if !output.status.success() {
            return Err(anyhow!(
                "wg pubkey failed: {}",
                String::from_utf8_lossy(&output.stderr)
            ));
        }
        let public_key = String::from_utf8_lossy(&output.stdout).trim().to_string();
        validate_public_key(&public_key)?;
        Ok((private_key, public_key))
    }
}

fn generate_userspace_keypair() -> Result<(String, String)> {
    let private = StaticSecret::random_from_rng(rand::rngs::OsRng);
    let public = PublicKey::from(&private);
    Ok((
        base64::engine::general_purpose::STANDARD.encode(private.to_bytes()),
        base64::engine::general_purpose::STANDARD.encode(public.as_bytes()),
    ))
}

/// 私钥落盘（仅 SYSTEM+Administrators 可读，依赖父目录 ACL）。
pub fn write_private_key(path: &Path, private_key: &str) -> Result<()> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)
            .with_context(|| format!("failed to create private key dir: {}", parent.display()))?;
    }
    std::fs::write(path, private_key)
        .with_context(|| format!("failed to write private key: {}", path.display()))
}

/// 读取私钥（trim 首尾空白）。
pub fn read_private_key(path: &Path) -> Result<String> {
    let content = std::fs::read_to_string(path)
        .with_context(|| format!("failed to read private key: {}", path.display()))?;
    Ok(content.trim().to_string())
}

// ============ 配置文件生成 ============

/// 为 WireGuard + gonc 双层 UDP 封装预留足够空间，避免外层 IP 分片。
pub const WGVPN_MTU: u16 = 1280;

/// WireGuard 多 Peer 配置生成上下文（builder 模式）。
pub struct WgConfigBuilder {
    private_key: Option<String>,
    address: Option<String>,
    listen_port: Option<u16>,
    mtu: u16,
    peers: Vec<PeerConfig>,
}

impl WgConfigBuilder {
    pub fn new() -> Self {
        Self {
            private_key: None,
            address: None,
            listen_port: None,
            mtu: WGVPN_MTU,
            peers: Vec::new(),
        }
    }

    pub fn private_key(mut self, k: impl Into<String>) -> Self {
        self.private_key = Some(k.into());
        self
    }
    pub fn address(mut self, a: impl Into<String>) -> Self {
        self.address = Some(a.into());
        self
    }
    pub fn listen_port(mut self, p: u16) -> Self {
        self.listen_port = Some(p);
        self
    }
    /// 设置 peers 列表（覆盖式）。调用后 render() 走多 peer 路径。
    pub fn peers(mut self, peers: Vec<PeerConfig>) -> Self {
        self.peers = peers;
        self
    }

    /// 链式追加单个 peer。
    pub fn add_peer(mut self, peer: PeerConfig) -> Self {
        self.peers.push(peer);
        self
    }

    /// 构建主动端配置（含 Endpoint + PersistentKeepalive）。
    pub fn build_active(self) -> WgConfig {
        WgConfig { inner: self }
    }
}

impl Default for WgConfigBuilder {
    fn default() -> Self {
        Self::new()
    }
}

/// 已确定的 WireGuard 配置（含角色信息）。
pub struct WgConfig {
    inner: WgConfigBuilder,
}

impl WgConfig {
    /// 渲染为 wg0.conf 文本内容。
    ///
    /// 每个 Peer 自带 endpoint、allowed IP 和 keepalive；空 Peer 列表属于无效配置。
    pub fn render(&self) -> Result<String> {
        let private_key = self
            .inner
            .private_key
            .as_ref()
            .ok_or_else(|| anyhow!("missing private_key"))?;
        let address = self
            .inner
            .address
            .as_ref()
            .ok_or_else(|| anyhow!("missing address"))?;

        let mut out = String::new();
        out.push_str("[Interface]\n");
        out.push_str(&format!("PrivateKey = {}\n", private_key));
        out.push_str(&format!("Address = {}\n", address));
        out.push_str(&format!("MTU = {}\n", self.inner.mtu));
        // listen_port 可选（多 peer 模式下首次建接口的 minimal conf 也可能不带）
        if let Some(lp) = self.inner.listen_port {
            out.push_str(&format!("ListenPort = {}\n", lp));
        }
        out.push('\n');

        if self.inner.peers.is_empty() {
            return Err(anyhow!("missing WireGuard peer"));
        }
        for peer in &self.inner.peers {
            out.push_str("[Peer]\n");
            out.push_str(&format!("PublicKey = {}\n", peer.public_key));
            if let Some(ep) = &peer.endpoint {
                out.push_str(&format!("Endpoint = {}\n", ep));
            }
            if let Some(ka) = peer.keepalive {
                out.push_str(&format!("PersistentKeepalive = {}\n", ka));
            }
            if !peer.allowed_ips.is_empty() {
                out.push_str(&format!("AllowedIPs = {}\n", peer.allowed_ips.join(", ")));
            }
            out.push('\n');
        }
        Ok(out)
    }

    /// 写入指定路径。
    pub fn write_to(&self, path: &Path) -> Result<()> {
        let text = self.render()?;
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)
                .with_context(|| format!("failed to create conf dir: {}", parent.display()))?;
        }
        std::fs::write(path, text)
            .with_context(|| format!("failed to write conf: {}", path.display()))
    }
}

// ============ 隧道启停 ============

/// 隧道句柄（停止时用）。
pub struct TunnelHandle {
    /// 隧道名（如 "wg0"，由 conf 文件名派生）。
    pub tunnel_name: String,
    /// conf 文件路径（停止后清理）。
    pub conf_path: PathBuf,
}

/// 从 conf 路径派生隧道名（去掉 .conf 扩展名）。
pub fn tunnel_name_from_conf_path(conf_path: &Path) -> String {
    conf_path
        .file_stem()
        .and_then(|s| s.to_str())
        .unwrap_or("wg0")
        .to_string()
}

/// 在 Linux 上创建并启动 WireGuard 接口。Windows 主流程使用进程内用户态 WG，
/// 不通过此函数启动外部 WireGuard 服务。
pub fn start_tunnel(wireguard_exe: &str, wg_cli: &str, conf_path: &Path) -> Result<TunnelHandle> {
    let conf_abs = if conf_path.is_absolute() {
        conf_path.to_path_buf()
    } else {
        std::env::current_dir().unwrap_or_default().join(conf_path)
    };
    if !conf_abs.exists() {
        return Err(anyhow!("conf file not found: {}", conf_abs.display()));
    }

    let tunnel_name = tunnel_name_from_conf_path(&conf_abs);
    #[cfg(not(windows))]
    {
        return start_tunnel_linux(wireguard_exe, wg_cli, &tunnel_name, &conf_abs);
    }

    #[cfg(windows)]
    {
        let _ = (wireguard_exe, wg_cli, tunnel_name);
        Err(anyhow!(
            "external WireGuard tunnel service is not supported on Windows"
        ))
    }
}

/// 卸载并停止 WireGuard 隧道服务。
pub fn stop_tunnel(wireguard_exe: &str, tunnel_name: &str) -> Result<()> {
    #[cfg(not(windows))]
    {
        let _ = wireguard_exe;
        return stop_tunnel_linux(tunnel_name);
    }

    #[cfg(windows)]
    {
        let _ = (wireguard_exe, tunnel_name);
        Ok(())
    }
}

#[cfg(not(windows))]
fn start_tunnel_linux(
    wireguard_exe: &str,
    wg_cli: &str,
    tunnel_name: &str,
    conf_path: &Path,
) -> Result<TunnelHandle> {
    let conf_text = std::fs::read_to_string(conf_path)
        .with_context(|| format!("failed to read wg conf: {}", conf_path.display()))?;
    let address = parse_interface_address(&conf_text)
        .ok_or_else(|| anyhow!("wg conf missing Interface Address"))?;
    let mtu = parse_interface_mtu(&conf_text).unwrap_or(WGVPN_MTU);
    let _ = run_command_with_timeout(
        Command::new("ip").args(["link", "del", tunnel_name]),
        "ip link del",
    );
    match linux_wireguard_backend() {
        LinuxWireGuardBackend::Kernel => {
            let wg_conf_text = strip_wg_quick_only_lines(&conf_text);
            let setconf_path = conf_path.with_extension("setconf");
            std::fs::write(&setconf_path, wg_conf_text).with_context(|| {
                format!("failed to write wg setconf: {}", setconf_path.display())
            })?;
            run_linux_command("ip", &["link", "add", tunnel_name, "type", "wireguard"])?;
            let setconf_result = run_linux_command(
                wg_cli,
                &["setconf", tunnel_name, &setconf_path.to_string_lossy()],
            );
            let _ = std::fs::remove_file(&setconf_path);
            if let Err(err) = setconf_result {
                let _ = run_command_with_timeout(
                    Command::new("ip").args(["link", "del", tunnel_name]),
                    "remove failed WireGuard interface",
                );
                return Err(err);
            }
        }
        LinuxWireGuardBackend::Userspace => {
            start_linux_userspace_wireguard(wireguard_exe, tunnel_name, conf_path, &conf_text)?;
        }
    }
    let configure_result = (|| {
        run_linux_command("ip", &["address", "add", &address, "dev", tunnel_name])?;
        run_linux_command(
            "ip",
            &["link", "set", "mtu", &mtu.to_string(), "dev", tunnel_name],
        )?;
        run_linux_command("ip", &["link", "set", "up", "dev", tunnel_name])
    })();
    if let Err(err) = configure_result {
        let _ = stop_tunnel_linux(tunnel_name);
        return Err(err);
    }

    tracing::info!(
        "[wgvpn] linux tunnel installed: name={}, backend={}, conf={}",
        tunnel_name,
        linux_wireguard_backend().as_str(),
        conf_path.display()
    );
    Ok(TunnelHandle {
        tunnel_name: tunnel_name.to_string(),
        conf_path: conf_path.to_path_buf(),
    })
}

#[cfg(not(windows))]
fn stop_tunnel_linux(tunnel_name: &str) -> Result<()> {
    if linux_wireguard_backend() == LinuxWireGuardBackend::Userspace {
        stop_linux_userspace_wireguard_process(tunnel_name);
    }
    let output = run_command_with_timeout(
        Command::new("ip").args(["link", "del", tunnel_name]),
        "ip link del",
    )?;
    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        if !stderr.contains("Cannot find device") {
            return Err(anyhow!("ip link del failed: {}", stderr));
        }
    }
    tracing::info!("[wgvpn] linux tunnel uninstalled: name={}", tunnel_name);
    if linux_wireguard_backend() == LinuxWireGuardBackend::Userspace {
        let _ = std::fs::remove_file(linux_uapi_socket_path(tunnel_name));
    }
    Ok(())
}

#[cfg(target_os = "linux")]
fn start_linux_userspace_wireguard(
    wireguard_go: &str,
    tunnel_name: &str,
    conf_path: &Path,
    conf_text: &str,
) -> Result<()> {
    if !Path::new(wireguard_go).is_file() {
        return Err(anyhow!(
            "kernel WireGuard is unavailable and userspace backend was not found: {}",
            wireguard_go
        ));
    }
    let request = wg_quick_conf_to_uapi(conf_text).with_context(|| {
        format!(
            "invalid userspace WireGuard config: {}",
            conf_path.display()
        )
    })?;
    let socket_path = linux_uapi_socket_path(tunnel_name);
    stop_linux_userspace_wireguard_process(tunnel_name);
    let _ = std::fs::remove_file(&socket_path);
    let mut child = Command::new(wireguard_go)
        .arg(tunnel_name)
        // 某些发行版的内核模块存在但当前命名空间/权限无法创建接口；此时仍应
        // 允许经过明确能力探测后选择的用户态后端启动。
        .env("WG_I_PREFER_BUGGY_USERSPACE_TO_POLISHED_KMOD", "1")
        .stdin(Stdio::null())
        .stdout(Stdio::inherit())
        .stderr(Stdio::inherit())
        .spawn()
        .with_context(|| format!("failed to start userspace WireGuard: {}", wireguard_go))?;
    let deadline = std::time::Instant::now() + Duration::from_secs(5);
    while !socket_path.exists() {
        if let Some(status) = child
            .try_wait()
            .context("failed to inspect wireguard-go process")?
        {
            return Err(anyhow!(
                "wireguard-go exited before UAPI was ready: {}",
                status
            ));
        }
        if std::time::Instant::now() >= deadline {
            let _ = child.kill();
            let _ = child.wait();
            return Err(anyhow!(
                "wireguard-go UAPI socket was not created: {}",
                socket_path.display()
            ));
        }
        std::thread::sleep(Duration::from_millis(50));
    }
    if let Err(err) = linux_uapi_set(tunnel_name, &request) {
        let _ = child.kill();
        let _ = child.wait();
        let _ = std::fs::remove_file(&socket_path);
        return Err(err);
    }
    LINUX_USERSPACE_PROCESSES
        .lock()
        .insert(tunnel_name.to_string(), child);
    Ok(())
}

#[cfg(target_os = "linux")]
fn stop_linux_userspace_wireguard_process(tunnel_name: &str) {
    let Some(mut child) = LINUX_USERSPACE_PROCESSES.lock().remove(tunnel_name) else {
        return;
    };
    let _ = child.kill();
    let _ = child.wait();
}

#[cfg(target_os = "linux")]
fn linux_uapi_socket_path(tunnel_name: &str) -> PathBuf {
    PathBuf::from("/var/run/wireguard").join(format!("{}.sock", tunnel_name))
}

#[cfg(target_os = "linux")]
fn linux_uapi_request(tunnel_name: &str, request: &str) -> Result<String> {
    use std::io::{BufRead as _, BufReader, Write as _};
    use std::os::unix::net::UnixStream;

    let path = linux_uapi_socket_path(tunnel_name);
    let mut stream = UnixStream::connect(&path)
        .with_context(|| format!("connect WireGuard UAPI socket: {}", path.display()))?;
    stream
        .set_read_timeout(Some(Duration::from_secs(3)))
        .context("set WireGuard UAPI read timeout")?;
    stream
        .write_all(request.as_bytes())
        .context("write WireGuard UAPI request")?;
    let mut response = String::new();
    let mut reader = BufReader::new(stream);
    loop {
        let mut line = String::new();
        if reader
            .read_line(&mut line)
            .context("read WireGuard UAPI response")?
            == 0
        {
            break;
        }
        let done = line == "\n";
        response.push_str(&line);
        if done {
            break;
        }
    }
    let errno = response
        .lines()
        .find_map(|line| line.strip_prefix("errno="))
        .unwrap_or("0");
    if errno != "0" {
        return Err(anyhow!("WireGuard UAPI returned errno={}", errno));
    }
    Ok(response)
}

#[cfg(target_os = "linux")]
fn linux_uapi_set(tunnel_name: &str, body: &str) -> Result<()> {
    let request = format!("set=1\n{}\n\n", body.trim_end());
    linux_uapi_request(tunnel_name, &request).map(|_| ())
}

#[cfg(target_os = "linux")]
fn linux_uapi_get(tunnel_name: &str) -> Result<Vec<LinuxUapiPeer>> {
    let response = linux_uapi_request(tunnel_name, "get=1\n\n")?;
    parse_linux_uapi_peers(&response)
}

#[cfg(target_os = "linux")]
#[derive(Debug, Default, Clone, PartialEq, Eq)]
struct LinuxUapiPeer {
    public_key: String,
    allowed_ips: Vec<String>,
    last_handshake_time_sec: u64,
    rx_bytes: u64,
    tx_bytes: u64,
}

#[cfg(target_os = "linux")]
fn parse_linux_uapi_peers(response: &str) -> Result<Vec<LinuxUapiPeer>> {
    let mut peers = Vec::new();
    let mut current: Option<LinuxUapiPeer> = None;
    for line in response.lines() {
        let Some((key, value)) = line.split_once('=') else {
            continue;
        };
        if key == "public_key" {
            if let Some(peer) = current.take() {
                peers.push(peer);
            }
            current = Some(LinuxUapiPeer {
                public_key: hex_key_to_base64(value)?,
                ..Default::default()
            });
            continue;
        }
        let Some(peer) = current.as_mut() else {
            continue;
        };
        match key {
            "allowed_ip" => peer.allowed_ips.push(value.to_string()),
            "last_handshake_time_sec" => {
                peer.last_handshake_time_sec = value.parse().unwrap_or_default()
            }
            "rx_bytes" => peer.rx_bytes = value.parse().unwrap_or_default(),
            "tx_bytes" => peer.tx_bytes = value.parse().unwrap_or_default(),
            _ => {}
        }
    }
    if let Some(peer) = current {
        peers.push(peer);
    }
    Ok(peers)
}

#[cfg(target_os = "linux")]
fn base64_key_to_hex(value: &str) -> Result<String> {
    let bytes = base64::engine::general_purpose::STANDARD
        .decode(value.trim())
        .context("decode WireGuard key")?;
    if bytes.len() != 32 {
        return Err(anyhow!("WireGuard key must contain 32 bytes"));
    }
    Ok(bytes.iter().map(|b| format!("{:02x}", b)).collect())
}

#[cfg(target_os = "linux")]
fn hex_key_to_base64(value: &str) -> Result<String> {
    if value.len() != 64 {
        return Err(anyhow!("WireGuard hex key must contain 64 digits"));
    }
    let bytes = (0..value.len())
        .step_by(2)
        .map(|index| u8::from_str_radix(&value[index..index + 2], 16))
        .collect::<std::result::Result<Vec<_>, _>>()
        .context("decode WireGuard hex key")?;
    Ok(base64::engine::general_purpose::STANDARD.encode(bytes))
}

#[cfg(target_os = "linux")]
fn wg_quick_conf_to_uapi(conf: &str) -> Result<String> {
    let mut output = vec!["replace_peers=true".to_string()];
    let mut section = "";
    for line in conf.lines() {
        let line = line.trim();
        if line.is_empty() || line.starts_with('#') {
            continue;
        }
        if line.starts_with('[') && line.ends_with(']') {
            section = &line[1..line.len() - 1];
            continue;
        }
        let Some((key, value)) = line.split_once('=') else {
            continue;
        };
        let key = key.trim();
        let value = value.trim();
        match (section, key) {
            ("Interface", "PrivateKey") => {
                output.push(format!("private_key={}", base64_key_to_hex(value)?))
            }
            ("Interface", "ListenPort") => output.push(format!("listen_port={}", value)),
            ("Peer", "PublicKey") => {
                output.push(format!("public_key={}", base64_key_to_hex(value)?));
                output.push("replace_allowed_ips=true".to_string());
            }
            ("Peer", "Endpoint") => output.push(format!("endpoint={}", value)),
            ("Peer", "AllowedIPs") => {
                output.extend(
                    value
                        .split(',')
                        .map(str::trim)
                        .filter(|entry| !entry.is_empty())
                        .map(|entry| format!("allowed_ip={}", entry)),
                );
            }
            ("Peer", "PersistentKeepalive") => {
                output.push(format!("persistent_keepalive_interval={}", value))
            }
            _ => {}
        }
    }
    Ok(output.join("\n"))
}

#[cfg(not(windows))]
fn run_linux_command(cmd: &str, args: &[&str]) -> Result<()> {
    let output = run_command_with_timeout(
        Command::new(cmd).args(args),
        &format!("{} {}", cmd, args.join(" ")),
    )?;
    if !output.status.success() {
        return Err(anyhow!(
            "{} {} failed: {}",
            cmd,
            args.join(" "),
            String::from_utf8_lossy(&output.stderr)
        ));
    }
    Ok(())
}

#[cfg(not(windows))]
fn parse_interface_address(conf: &str) -> Option<String> {
    conf.lines().find_map(|line| {
        let (key, value) = line.split_once('=')?;
        if key.trim().eq_ignore_ascii_case("Address") {
            Some(value.trim().to_string())
        } else {
            None
        }
    })
}

#[cfg(not(windows))]
fn parse_interface_mtu(conf: &str) -> Option<u16> {
    conf.lines().find_map(|line| {
        let (key, value) = line.split_once('=')?;
        if key.trim().eq_ignore_ascii_case("MTU") {
            value.trim().parse().ok()
        } else {
            None
        }
    })
}

#[cfg(not(windows))]
fn strip_wg_quick_only_lines(conf: &str) -> String {
    conf.lines()
        .filter(|line| {
            let key = line.split_once('=').map(|(k, _)| k.trim());
            !matches!(key, Some(k) if k.eq_ignore_ascii_case("Address") || k.eq_ignore_ascii_case("MTU"))
        })
        .collect::<Vec<_>>()
        .join("\n")
}

// ============ 多 peer 动态管理（wg set） ============
//
// Linux 单 wg0 接口多 Peer 模式下，首次连接创建接口，后续通过 `wg set`
// 热增删 Peer，不影响其他连接。Windows 由进程内用户态 WG 管理 Peer。

/// 单个 peer 的配置（多 peer 模型的核心数据结构）。
#[derive(Debug, Clone)]
pub struct PeerConfig {
    /// 对端 WireGuard 公钥（base64）。
    pub public_key: String,
    /// 对端 endpoint。`None` 表示被动端不写 Endpoint（从握手包学习对端地址）。
    pub endpoint: Option<String>,
    /// 路由到该 peer 的 CIDR 列表（通常是 `<peer_virtual_ip>/32`）。
    pub allowed_ips: Vec<String>,
    /// Keepalive 间隔（秒）。`None` 表示不写。
    pub keepalive: Option<u16>,
}

impl PeerConfig {
    /// 构造主动端 peer（含 Endpoint + Keepalive 25s）。
    pub fn active(
        public_key: impl Into<String>,
        endpoint: impl Into<String>,
        peer_ip: &str,
    ) -> Self {
        Self {
            public_key: public_key.into(),
            endpoint: Some(endpoint.into()),
            allowed_ips: vec![format!("{}/32", peer_ip)],
            keepalive: Some(25),
        }
    }

    /// 构造被动端 peer（无 Endpoint、无 Keepalive，等对端连入）。
    pub fn passive(public_key: impl Into<String>, peer_ip: &str) -> Self {
        Self {
            public_key: public_key.into(),
            endpoint: None,
            allowed_ips: vec![format!("{}/32", peer_ip)],
            keepalive: None,
        }
    }
}

/// 查询 wg 接口是否已存在。
///
/// 调用 `wg show <tunnel_name>`，成功（退出码 0）即认为接口存在。
pub fn tunnel_exists(wg_cli: &str, tunnel_name: &str) -> bool {
    #[cfg(target_os = "linux")]
    if linux_wireguard_backend() == LinuxWireGuardBackend::Userspace {
        return linux_uapi_get(tunnel_name).is_ok();
    }

    let output = run_command_with_timeout(
        Command::new(wg_cli).args(["show", tunnel_name]),
        "wg show tunnel",
    );
    matches!(output, Ok(o) if o.status.success())
}

/// 动态添加 peer 到已存在的 wg 接口（不重启接口）。
///
/// 等价命令：
/// ```text
/// wg set <tunnel> peer <pubkey> [endpoint <ep>] allowed-ips <ips...> [persistent-keepalive <ka>]
/// ```
pub fn add_peer(wg_cli: &str, tunnel_name: &str, peer: &PeerConfig) -> Result<()> {
    #[cfg(target_os = "linux")]
    if linux_wireguard_backend() == LinuxWireGuardBackend::Userspace {
        let mut body = vec![format!(
            "public_key={}",
            base64_key_to_hex(&peer.public_key)?
        )];
        if let Some(endpoint) = &peer.endpoint {
            body.push(format!("endpoint={}", endpoint));
        }
        body.push("replace_allowed_ips=true".to_string());
        body.extend(
            peer.allowed_ips
                .iter()
                .map(|allowed_ip| format!("allowed_ip={}", allowed_ip)),
        );
        if let Some(keepalive) = peer.keepalive {
            body.push(format!("persistent_keepalive_interval={}", keepalive));
        }
        linux_uapi_set(tunnel_name, &body.join("\n"))?;
        tracing::info!(
            "[wgvpn] userspace peer added: tunnel={}, pubkey={}..",
            tunnel_name,
            peer.public_key.get(..8).unwrap_or(&peer.public_key)
        );
        return Ok(());
    }

    let mut args: Vec<String> = vec!["set".into(), tunnel_name.into()];
    args.push("peer".into());
    args.push(peer.public_key.clone());
    if let Some(ep) = &peer.endpoint {
        args.push("endpoint".into());
        args.push(ep.clone());
    }
    if !peer.allowed_ips.is_empty() {
        args.push("allowed-ips".into());
        args.push(peer.allowed_ips.join(","));
    }
    if let Some(ka) = peer.keepalive {
        args.push("persistent-keepalive".into());
        args.push(ka.to_string());
    }

    let output = run_command_with_timeout(Command::new(wg_cli).args(&args), "wg set peer")?;
    if !output.status.success() {
        return Err(anyhow!(
            "wg set peer failed: {}",
            String::from_utf8_lossy(&output.stderr)
        ));
    }
    tracing::info!(
        "[wgvpn] peer added: tunnel={}, pubkey={}.., endpoint={:?}",
        tunnel_name,
        &peer.public_key.get(..8).unwrap_or(&peer.public_key),
        peer.endpoint
    );
    Ok(())
}

/// 动态移除 peer（不重启接口）。
///
/// 等价命令：`wg set <tunnel> peer <pubkey> remove`
pub fn remove_peer(wg_cli: &str, tunnel_name: &str, peer_public_key: &str) -> Result<()> {
    #[cfg(target_os = "linux")]
    if linux_wireguard_backend() == LinuxWireGuardBackend::Userspace {
        return linux_uapi_set(
            tunnel_name,
            &format!(
                "public_key={}\nremove=true",
                base64_key_to_hex(peer_public_key)?
            ),
        );
    }

    let output = run_command_with_timeout(
        Command::new(wg_cli).args(["set", tunnel_name, "peer", peer_public_key, "remove"]),
        "wg set peer remove",
    )?;
    if !output.status.success() {
        // peer 不存在视为成功（幂等）
        let stderr = String::from_utf8_lossy(&output.stderr);
        if !stderr.contains("No such peer") && !stderr.contains("does not exist") {
            return Err(anyhow!("wg set peer remove failed: {}", stderr));
        }
    }
    tracing::info!(
        "[wgvpn] peer removed: tunnel={}, pubkey={}..",
        tunnel_name,
        peer_public_key.get(..8).unwrap_or(peer_public_key)
    );
    Ok(())
}

/// 查询 wg 接口的 peer 数量（跨进程的真实状态）。
///
/// 基于 `wg show <tunnel> peers` 输出行数（每行一个 pubkey）。
pub fn peer_count(wg_cli: &str, tunnel_name: &str) -> usize {
    #[cfg(target_os = "linux")]
    if linux_wireguard_backend() == LinuxWireGuardBackend::Userspace {
        return linux_uapi_get(tunnel_name)
            .map(|peers| peers.len())
            .unwrap_or_default();
    }

    let Ok(output) = run_command_with_timeout(
        Command::new(wg_cli).args(["show", tunnel_name, "peers"]),
        "wg show peers",
    ) else {
        return 0;
    };
    let stdout = String::from_utf8_lossy(&output.stdout);
    stdout.lines().filter(|l| !l.trim().is_empty()).count()
}

/// 查询指定 WireGuard peer 的累计接收/发送字节数。
///
/// `wg show <tunnel> transfer` 每行格式为：
/// `<peer_pubkey>\t<received_bytes>\t<transmitted_bytes>`。
pub fn peer_transfer_bytes(
    wg_cli: &str,
    tunnel_name: &str,
    peer_public_key: &str,
) -> Option<(u64, u64)> {
    #[cfg(target_os = "linux")]
    if linux_wireguard_backend() == LinuxWireGuardBackend::Userspace {
        return linux_uapi_get(tunnel_name)
            .ok()?
            .into_iter()
            .find(|peer| peer.public_key == peer_public_key)
            .map(|peer| (peer.rx_bytes, peer.tx_bytes));
    }

    let output = run_command_with_timeout(
        Command::new(wg_cli).args(["show", tunnel_name, "transfer"]),
        "wg show transfer",
    )
    .ok()?;
    if !output.status.success() {
        return None;
    }
    let stdout = String::from_utf8_lossy(&output.stdout);
    stdout.lines().find_map(|line| {
        let mut fields = line.split_whitespace();
        if fields.next()? != peer_public_key {
            return None;
        }
        let received = fields.next()?.parse().ok()?;
        let transmitted = fields.next()?.parse().ok()?;
        Some((received, transmitted))
    })
}

/// 返回与 `wg show <tunnel> latest-handshakes` 相同的文本格式。
pub fn peer_latest_handshakes(wg_cli: &str, tunnel_name: &str) -> Result<String> {
    #[cfg(target_os = "linux")]
    if linux_wireguard_backend() == LinuxWireGuardBackend::Userspace {
        let peers = linux_uapi_get(tunnel_name)?;
        return Ok(peers
            .into_iter()
            .map(|peer| format!("{}\t{}", peer.public_key, peer.last_handshake_time_sec))
            .collect::<Vec<_>>()
            .join("\n"));
    }

    let output = run_command_with_timeout(
        Command::new(wg_cli).args(["show", tunnel_name, "latest-handshakes"]),
        "wg show latest-handshakes",
    )?;
    if !output.status.success() {
        return Err(anyhow!(
            "wg show latest-handshakes failed: {}",
            String::from_utf8_lossy(&output.stderr)
        ));
    }
    Ok(String::from_utf8_lossy(&output.stdout).into_owned())
}

/// 查询 wg 接口上所有 peer 的 AllowedIPs（跨进程的真实状态）。
///
/// 解析 `wg show <tunnel> allowed-ips` 输出，每行格式：
/// ```text
/// <peer_pubkey>\t<ip1>/32\t<ip2>/32
/// ```
/// 返回所有 AllowedIPs 的集合（CIDR 形式，如 "100.99.71.2/32"）。
pub fn list_peer_allowed_ips(wg_cli: &str, tunnel_name: &str) -> HashSet<String> {
    #[cfg(target_os = "linux")]
    if linux_wireguard_backend() == LinuxWireGuardBackend::Userspace {
        return linux_uapi_get(tunnel_name)
            .map(|peers| {
                peers
                    .into_iter()
                    .flat_map(|peer| peer.allowed_ips)
                    .collect()
            })
            .unwrap_or_default();
    }

    let mut used = HashSet::new();
    let Ok(output) = run_command_with_timeout(
        Command::new(wg_cli).args(["show", tunnel_name, "allowed-ips"]),
        "wg show allowed-ips",
    ) else {
        return used;
    };
    let stdout = String::from_utf8_lossy(&output.stdout);
    for line in stdout.lines() {
        // 每行：pubkey\tip1/32\tip2/32 ...
        for field in line.split_whitespace().skip(1) {
            if !field.is_empty() {
                used.insert(field.to_string());
            }
        }
    }
    used
}

#[cfg(all(test, target_os = "linux"))]
mod linux_userspace_tests {
    use super::*;

    const PRIVATE_KEY: &str = "OALuNaYV0FT2VSvO+1I0Gg8JgXvL0D2vC0WI5X9rIEE=";
    const PUBLIC_KEY: &str = "DKjrK6pv6DfnYkTQqh1C3pQj74JQEwcoCavwP9bR0CM=";

    #[test]
    fn converts_wg_quick_config_to_uapi() {
        let conf = format!(
            "[Interface]\nPrivateKey = {PRIVATE_KEY}\nAddress = 100.99.71.2/24\nListenPort = 51820\nMTU = 1280\n\n[Peer]\nPublicKey = {PUBLIC_KEY}\nEndpoint = 127.0.0.1:50000\nAllowedIPs = 100.99.71.3/32, 192.168.1.0/24\nPersistentKeepalive = 25\n"
        );
        let uapi = wg_quick_conf_to_uapi(&conf).unwrap();
        assert!(uapi.contains("replace_peers=true"));
        assert!(uapi.contains("private_key="));
        assert!(uapi.contains("listen_port=51820"));
        assert!(uapi.contains("public_key="));
        assert!(uapi.contains("allowed_ip=100.99.71.3/32"));
        assert!(uapi.contains("allowed_ip=192.168.1.0/24"));
        assert!(uapi.contains("persistent_keepalive_interval=25"));
        assert!(!uapi.contains("Address"));
        assert!(!uapi.contains("MTU"));
    }

    #[test]
    fn parses_userspace_peer_status() {
        let key_hex = base64_key_to_hex(PUBLIC_KEY).unwrap();
        let response = format!(
            "private_key={}\nlisten_port=51820\npublic_key={}\nallowed_ip=100.99.71.3/32\nlast_handshake_time_sec=123\nrx_bytes=456\ntx_bytes=789\nerrno=0\n\n",
            base64_key_to_hex(PRIVATE_KEY).unwrap(),
            key_hex
        );
        let peers = parse_linux_uapi_peers(&response).unwrap();
        assert_eq!(peers.len(), 1);
        assert_eq!(peers[0].public_key, PUBLIC_KEY);
        assert_eq!(peers[0].allowed_ips, ["100.99.71.3/32"]);
        assert_eq!(peers[0].last_handshake_time_sec, 123);
        assert_eq!((peers[0].rx_bytes, peers[0].tx_bytes), (456, 789));
    }
}
