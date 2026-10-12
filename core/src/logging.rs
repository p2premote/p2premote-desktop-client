use flate2::{write::DeflateEncoder, Compression};
use once_cell::sync::{Lazy, OnceCell};
use serde::Serialize;
use std::fs::{self, File, OpenOptions};
use std::io::{self, Cursor, Seek, Write};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use tracing_subscriber::fmt::MakeWriter;

const MAX_ROLLED_LOG_FILES: usize = 3;
pub const AUDIT_LOG_FILE_NAME: &str = "p2premote-audit.log";

/// Build a portable default name for an exported log archive.
pub fn default_log_archive_name() -> String {
    let now = time::OffsetDateTime::now_local().unwrap_or_else(|_| time::OffsetDateTime::now_utc());
    format!(
        "p2premote-logs-{:04}{:02}{:02}-{:02}{:02}{:02}.zip",
        now.year(),
        u8::from(now.month()),
        now.day(),
        now.hour(),
        now.minute(),
        now.second()
    )
}

/// Compress all regular files in the application's log directory into `destination`.
///
/// The archive is first written beside the destination and then moved into place, so
/// a failed export never leaves a partially written ZIP at the user-selected path.
pub fn export_logs(destination: impl AsRef<Path>) -> io::Result<usize> {
    export_logs_from(&crate::config::machine_log_dir(), destination.as_ref())
}

fn export_logs_from(log_dir: &Path, destination: &Path) -> io::Result<usize> {
    let parent = destination
        .parent()
        .filter(|path| !path.as_os_str().is_empty())
        .unwrap_or(Path::new("."));
    fs::create_dir_all(parent)?;

    let temp_name = format!(
        ".{}.{}.tmp",
        destination
            .file_name()
            .unwrap_or_default()
            .to_string_lossy(),
        std::process::id()
    );
    let temp_path = parent.join(temp_name);
    let result = (|| {
        let file = OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&temp_path)?;
        let (file, count) = write_logs_zip(log_dir, file, &[destination, &temp_path])?;
        file.sync_all()?;

        if destination.exists() {
            fs::remove_file(destination)?;
        }
        fs::rename(&temp_path, destination)?;
        Ok(count)
    })();
    if result.is_err() {
        let _ = fs::remove_file(&temp_path);
    }
    result
}

/// Build a ZIP response for the authenticated Web management download endpoint.
pub fn build_logs_zip() -> io::Result<(Vec<u8>, usize)> {
    let (cursor, count) = write_logs_zip(
        &crate::config::machine_log_dir(),
        Cursor::new(Vec::new()),
        &[],
    )?;
    Ok((cursor.into_inner(), count))
}

fn write_logs_zip<W: Write + Seek>(
    log_dir: &Path,
    writer: W,
    excluded: &[&Path],
) -> io::Result<(W, usize)> {
    let mut writer = writer;
    let mut entries = fs::read_dir(log_dir)?
        .filter_map(Result::ok)
        .filter(|entry| {
            entry
                .file_type()
                .map(|kind| kind.is_file())
                .unwrap_or(false)
        })
        .filter(|entry| entry.path().extension().is_some_and(|ext| ext == "log"))
        .filter(|entry| !excluded.iter().any(|path| entry.path() == *path))
        .collect::<Vec<_>>();
    entries.sort_by_key(|entry| entry.file_name());

    struct CentralEntry {
        name: Vec<u8>,
        crc32: u32,
        compressed_size: u32,
        uncompressed_size: u32,
        offset: u32,
    }
    let mut central_entries = Vec::new();
    for entry in entries {
        let name = entry
            .file_name()
            .to_string_lossy()
            .into_owned()
            .into_bytes();
        let contents = fs::read(entry.path())?;
        let uncompressed_size = u32::try_from(contents.len())
            .map_err(|_| io::Error::other("a log file is too large for ZIP32"))?;
        let mut encoder = DeflateEncoder::new(Vec::new(), Compression::default());
        encoder.write_all(&contents)?;
        let compressed = encoder.finish()?;
        let compressed_size = u32::try_from(compressed.len())
            .map_err(|_| io::Error::other("a compressed log file is too large for ZIP32"))?;
        let offset = u32::try_from(writer.stream_position()?)
            .map_err(|_| io::Error::other("log archive is too large for ZIP32"))?;
        let crc32 = crc32(&contents);

        write_u32(&mut writer, 0x0403_4b50)?;
        write_u16(&mut writer, 20)?;
        write_u16(&mut writer, 0x0800)?; // UTF-8 file name
        write_u16(&mut writer, 8)?; // deflate
        write_u16(&mut writer, 0)?;
        write_u16(&mut writer, 0)?;
        write_u32(&mut writer, crc32)?;
        write_u32(&mut writer, compressed_size)?;
        write_u32(&mut writer, uncompressed_size)?;
        write_u16(&mut writer, name.len() as u16)?;
        write_u16(&mut writer, 0)?;
        writer.write_all(&name)?;
        writer.write_all(&compressed)?;
        central_entries.push(CentralEntry {
            name,
            crc32,
            compressed_size,
            uncompressed_size,
            offset,
        });
    }

    let central_offset = writer.stream_position()?;
    for entry in &central_entries {
        write_u32(&mut writer, 0x0201_4b50)?;
        write_u16(&mut writer, 20)?;
        write_u16(&mut writer, 20)?;
        write_u16(&mut writer, 0x0800)?;
        write_u16(&mut writer, 8)?;
        write_u16(&mut writer, 0)?;
        write_u16(&mut writer, 0)?;
        write_u32(&mut writer, entry.crc32)?;
        write_u32(&mut writer, entry.compressed_size)?;
        write_u32(&mut writer, entry.uncompressed_size)?;
        write_u16(&mut writer, entry.name.len() as u16)?;
        write_u16(&mut writer, 0)?;
        write_u16(&mut writer, 0)?;
        write_u16(&mut writer, 0)?;
        write_u16(&mut writer, 0)?;
        write_u32(&mut writer, 0)?;
        write_u32(&mut writer, entry.offset)?;
        writer.write_all(&entry.name)?;
    }
    let central_size = writer.stream_position()? - central_offset;
    let count = u16::try_from(central_entries.len())
        .map_err(|_| io::Error::other("too many files for ZIP32"))?;
    write_u32(&mut writer, 0x0605_4b50)?;
    write_u16(&mut writer, 0)?;
    write_u16(&mut writer, 0)?;
    write_u16(&mut writer, count)?;
    write_u16(&mut writer, count)?;
    write_u32(&mut writer, central_size as u32)?;
    write_u32(&mut writer, central_offset as u32)?;
    write_u16(&mut writer, 0)?;
    Ok((writer, central_entries.len()))
}

fn write_u16(writer: &mut impl Write, value: u16) -> io::Result<()> {
    writer.write_all(&value.to_le_bytes())
}

fn write_u32(writer: &mut impl Write, value: u32) -> io::Result<()> {
    writer.write_all(&value.to_le_bytes())
}

fn crc32(bytes: &[u8]) -> u32 {
    let mut crc = !0_u32;
    for &byte in bytes {
        crc ^= u32::from(byte);
        for _ in 0..8 {
            crc = (crc >> 1) ^ (0xedb8_8320 & (0_u32.wrapping_sub(crc & 1)));
        }
    }
    !crc
}

static AUDIT_LOG_WRITE_LOCK: Lazy<Mutex<()>> = Lazy::new(|| Mutex::new(()));

#[derive(Debug, Clone, Copy, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum TunnelAuditAction {
    Established,
    Disconnected,
}

#[derive(Debug, Clone, Copy, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum TunnelAuditRole {
    Active,
    Passive,
}

#[derive(Debug, Serialize)]
pub struct TunnelAuditEvent {
    pub action: TunnelAuditAction,
    pub role: TunnelAuditRole,
    pub local_device_id: Option<i64>,
    pub peer_device_id: i64,
    pub local_virtual_ip: String,
    pub peer_virtual_ip: String,
    pub exposed_lan_cidrs: Vec<String>,
}

#[derive(Serialize)]
struct TunnelAuditLine<'a> {
    timestamp: String,
    event: TunnelAuditAction,
    tunnel_type: &'static str,
    role: TunnelAuditRole,
    local_device_id: Option<i64>,
    peer_device_id: i64,
    local_virtual_ip: &'a str,
    peer_virtual_ip: &'a str,
    exposed_lan_cidrs: &'a [String],
}

/// Append one successful tunnel lifecycle transition to the standalone audit log.
///
/// Audit logging is deliberately best-effort and never participates in tunnel
/// rollback: a filesystem failure is reported to the service log, but is not
/// returned to the caller.
pub fn record_tunnel_audit(event: TunnelAuditEvent) {
    let action = event.action;
    let peer_device_id = event.peer_device_id;
    if let Err(err) = append_tunnel_audit(&crate::config::machine_log_dir(), &event) {
        tracing::warn!(
            action = ?action,
            peer_device_id,
            error = %err,
            "failed to append tunnel audit event"
        );
    }
}

fn append_tunnel_audit(log_dir: &Path, event: &TunnelAuditEvent) -> io::Result<()> {
    let _guard = AUDIT_LOG_WRITE_LOCK
        .lock()
        .map_err(|_| io::Error::other("audit log write lock is poisoned"))?;
    fs::create_dir_all(log_dir)?;

    let record = TunnelAuditLine {
        timestamp: audit_timestamp(),
        event: event.action,
        tunnel_type: "wgvpn",
        role: event.role,
        local_device_id: event.local_device_id,
        peer_device_id: event.peer_device_id,
        local_virtual_ip: &event.local_virtual_ip,
        peer_virtual_ip: &event.peer_virtual_ip,
        exposed_lan_cidrs: &event.exposed_lan_cidrs,
    };
    let mut line = serde_json::to_vec(&record).map_err(io::Error::other)?;
    line.push(b'\n');

    let mut file = OpenOptions::new()
        .create(true)
        .append(true)
        .open(log_dir.join(AUDIT_LOG_FILE_NAME))?;
    file.write_all(&line)?;
    file.sync_data()
}

fn audit_timestamp() -> String {
    let now = time::OffsetDateTime::now_local().unwrap_or_else(|_| time::OffsetDateTime::now_utc());
    let offset_seconds = now.offset().whole_seconds();
    let offset_sign = if offset_seconds < 0 { '-' } else { '+' };
    let offset_seconds = offset_seconds.abs();
    format!(
        "{:04}-{:02}-{:02}T{:02}:{:02}:{:02}.{:03}{}{:02}:{:02}",
        now.year(),
        u8::from(now.month()),
        now.day(),
        now.hour(),
        now.minute(),
        now.second(),
        now.nanosecond() / 1_000_000,
        offset_sign,
        offset_seconds / 3_600,
        (offset_seconds % 3_600) / 60,
    )
}

struct DailyLogFile {
    // Keep the handle optional so Windows can close it before the current log
    // is renamed. Renaming an open file is not reliable on Windows.
    file: Option<File>,
    date: String,
    stem: String,
    parent: PathBuf,
}

impl DailyLogFile {
    fn new(log_dir: &Path, stem: &str) -> io::Result<Self> {
        Self::new_for_date(log_dir, stem, today())
    }

    fn new_for_date(log_dir: &Path, stem: &str, date: String) -> io::Result<Self> {
        fs::create_dir_all(log_dir)?;
        let file = Self::open_current_file(log_dir, stem)?;
        Ok(Self {
            file: Some(file),
            date,
            stem: stem.to_string(),
            parent: log_dir.to_path_buf(),
        })
    }

    fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
        self.write_for_date(buf, today())
    }

    fn write_for_date(&mut self, buf: &[u8], current: String) -> io::Result<usize> {
        if current != self.date {
            self.rotate(current)?;
        }
        self.file_mut()?.write(buf)
    }

    fn rotate(&mut self, current: String) -> io::Result<()> {
        // A previous rotation may have archived the old file successfully but
        // failed to create the new current file. Retry opening it here so a
        // transient filesystem error does not permanently disable logging.
        if self.file.is_none() {
            self.file = Some(Self::open_current_file(&self.parent, &self.stem)?);
        }
        let mut old_file = self
            .file
            .take()
            .ok_or_else(|| io::Error::other("current log file is not open"))?;
        if let Err(err) = old_file.flush() {
            self.file = Some(old_file);
            return Err(err);
        }
        // Explicitly close the handle before rename. This is required on
        // Windows; replacing the path while retaining the old handle would
        // leave subsequent writes attached to the archived file.
        drop(old_file);

        if let Err(err) = self.archive_current_log() {
            self.file = Self::open_current_file(&self.parent, &self.stem).ok();
            return Err(err);
        }

        match Self::open_current_file(&self.parent, &self.stem) {
            Ok(file) => {
                self.file = Some(file);
                self.date = current;
                self.cleanup_rolled_logs();
                Ok(())
            }
            Err(err) => Err(err),
        }
    }

    fn archive_current_log(&self) -> io::Result<()> {
        let current_path = self.parent.join(format!("{}.log", self.stem));
        if !current_path.exists() {
            return Ok(());
        }
        let archived_path = self.parent.join(format!("{}-{}.log", self.stem, self.date));
        if archived_path.exists() {
            fs::remove_file(&archived_path)?;
        }
        fs::rename(current_path, archived_path)
    }

    fn open_current_file(log_dir: &Path, stem: &str) -> io::Result<File> {
        OpenOptions::new()
            .create(true)
            .append(true)
            .open(log_dir.join(format!("{stem}.log")))
    }

    fn file_mut(&mut self) -> io::Result<&mut File> {
        self.file
            .as_mut()
            .ok_or_else(|| io::Error::other("current log file is not open"))
    }

    fn cleanup_rolled_logs(&self) {
        let prefix = format!("{}-", self.stem);
        let Ok(entries) = fs::read_dir(&self.parent) else {
            return;
        };
        let mut logs = entries
            .filter_map(Result::ok)
            .filter_map(|entry| {
                let path = entry.path();
                let name = path.file_name()?.to_string_lossy().to_string();
                (name.starts_with(&prefix) && name.ends_with(".log")).then_some((name, path))
            })
            .collect::<Vec<_>>();
        logs.sort_by(|a, b| b.0.cmp(&a.0));
        for (_, path) in logs.into_iter().skip(MAX_ROLLED_LOG_FILES) {
            let _ = fs::remove_file(path);
        }
    }
}

#[derive(Clone)]
pub struct DailyLogWriter {
    inner: Arc<Mutex<DailyLogFile>>,
}

pub struct DailyLogLine {
    inner: Arc<Mutex<DailyLogFile>>,
}

impl DailyLogWriter {
    pub fn new(log_dir: impl AsRef<Path>, stem: &str) -> io::Result<Self> {
        Ok(Self {
            inner: Arc::new(Mutex::new(DailyLogFile::new(log_dir.as_ref(), stem)?)),
        })
    }
}

impl Write for DailyLogLine {
    fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
        self.inner.lock().unwrap().write(buf)
    }

    fn flush(&mut self) -> io::Result<()> {
        self.inner.lock().unwrap().file_mut()?.flush()
    }
}

impl<'a> MakeWriter<'a> for DailyLogWriter {
    type Writer = DailyLogLine;

    fn make_writer(&'a self) -> Self::Writer {
        DailyLogLine {
            inner: self.inner.clone(),
        }
    }
}

fn today() -> String {
    let now = time::OffsetDateTime::now_local().unwrap_or_else(|_| time::OffsetDateTime::now_utc());
    format!(
        "{:04}-{:02}-{:02}",
        now.year(),
        u8::from(now.month()),
        now.day()
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    fn test_dir(name: &str) -> PathBuf {
        std::env::temp_dir().join(format!(
            "p2premote-logging-{name}-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ))
    }

    #[test]
    fn rotation_archives_old_day_and_reopens_current_file() {
        let dir = test_dir("rotate");
        let mut log =
            DailyLogFile::new_for_date(&dir, "service", "2026-07-31".to_string()).unwrap();
        log.write_for_date(b"old day\n", "2026-07-31".to_string())
            .unwrap();
        log.write_for_date(b"new day\n", "2026-08-01".to_string())
            .unwrap();
        log.file_mut().unwrap().flush().unwrap();

        assert_eq!(
            fs::read_to_string(dir.join("service-2026-07-31.log")).unwrap(),
            "old day\n"
        );
        assert_eq!(
            fs::read_to_string(dir.join("service.log")).unwrap(),
            "new day\n"
        );
        fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn rotation_replaces_existing_archive_and_reopens_current_file() {
        let dir = test_dir("replace");
        fs::create_dir_all(&dir).unwrap();
        fs::write(dir.join("service-2026-07-31.log"), b"earlier\n").unwrap();
        let mut log =
            DailyLogFile::new_for_date(&dir, "service", "2026-07-31".to_string()).unwrap();
        log.write_for_date(b"later\n", "2026-07-31".to_string())
            .unwrap();
        log.write_for_date(b"today\n", "2026-08-01".to_string())
            .unwrap();
        log.file_mut().unwrap().flush().unwrap();

        assert_eq!(
            fs::read_to_string(dir.join("service-2026-07-31.log")).unwrap(),
            "later\n"
        );
        assert_eq!(
            fs::read_to_string(dir.join("service.log")).unwrap(),
            "today\n"
        );
        fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn cleanup_keeps_current_log_and_three_newest_archives() {
        let dir = test_dir("cleanup");
        let log = DailyLogFile::new_for_date(&dir, "service", "2026-08-01".to_string()).unwrap();
        for day in 1..=5 {
            fs::write(
                dir.join(format!("service-2026-07-{day:02}.log")),
                day.to_string(),
            )
            .unwrap();
        }

        log.cleanup_rolled_logs();

        assert!(dir.join("service.log").exists());
        assert!(!dir.join("service-2026-07-01.log").exists());
        assert!(!dir.join("service-2026-07-02.log").exists());
        assert!(dir.join("service-2026-07-03.log").exists());
        assert!(dir.join("service-2026-07-04.log").exists());
        assert!(dir.join("service-2026-07-05.log").exists());
        fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn audit_log_appends_json_lines_without_rotation() {
        let dir = test_dir("audit");
        let established = TunnelAuditEvent {
            action: TunnelAuditAction::Established,
            role: TunnelAuditRole::Active,
            local_device_id: Some(41),
            peer_device_id: 59,
            local_virtual_ip: "100.99.71.2".to_string(),
            peer_virtual_ip: "100.99.71.1".to_string(),
            exposed_lan_cidrs: vec!["192.168.10.0/24".to_string()],
        };
        let disconnected = TunnelAuditEvent {
            action: TunnelAuditAction::Disconnected,
            role: TunnelAuditRole::Active,
            local_device_id: Some(41),
            peer_device_id: 59,
            local_virtual_ip: "100.99.71.2".to_string(),
            peer_virtual_ip: "100.99.71.1".to_string(),
            exposed_lan_cidrs: vec!["192.168.10.0/24".to_string()],
        };

        append_tunnel_audit(&dir, &established).unwrap();
        append_tunnel_audit(&dir, &disconnected).unwrap();

        let content = fs::read_to_string(dir.join(AUDIT_LOG_FILE_NAME)).unwrap();
        let records = content
            .lines()
            .map(|line| serde_json::from_str::<serde_json::Value>(line).unwrap())
            .collect::<Vec<_>>();
        assert_eq!(records.len(), 2);
        assert_eq!(records[0]["event"], "established");
        assert_eq!(records[1]["event"], "disconnected");
        assert_eq!(records[0]["tunnel_type"], "wgvpn");
        assert_eq!(records[0]["role"], "active");
        assert_eq!(records[0]["local_device_id"], 41);
        assert_eq!(records[0]["peer_device_id"], 59);
        assert_eq!(records[0]["local_virtual_ip"], "100.99.71.2");
        assert_eq!(records[0]["peer_virtual_ip"], "100.99.71.1");
        assert_eq!(records[0]["exposed_lan_cidrs"][0], "192.168.10.0/24");
        assert!(records[0]["timestamp"].as_str().unwrap().contains('T'));
        assert_eq!(fs::read_dir(&dir).unwrap().count(), 1);
        fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn export_logs_creates_zip_with_all_log_files() {
        let dir = test_dir("export");
        fs::create_dir_all(&dir).unwrap();
        fs::write(dir.join("p2premote.log"), b"gui log\n").unwrap();
        fs::write(dir.join("service.log"), b"service log\n").unwrap();
        let output_dir = test_dir("export-output");
        let output = output_dir.join("logs.zip");

        assert_eq!(export_logs_from(&dir, &output).unwrap(), 2);
        let archive = fs::read(&output).unwrap();
        assert!(archive.starts_with(&0x0403_4b50_u32.to_le_bytes()));
        assert!(archive.windows(13).any(|bytes| bytes == b"p2premote.log"));
        assert!(archive.windows(11).any(|bytes| bytes == b"service.log"));
        assert!(archive
            .windows(4)
            .any(|bytes| bytes == 0x0605_4b50_u32.to_le_bytes()));

        fs::remove_dir_all(dir).unwrap();
        fs::remove_dir_all(output_dir).unwrap();
    }

    #[test]
    fn zip_crc32_matches_standard_test_vector() {
        assert_eq!(crc32(b"123456789"), 0xcbf4_3926);
    }
}

type LogLevelReloader = dyn Fn(&str) -> Result<(), String> + Send + Sync + 'static;

static LOG_LEVEL_RELOADER: OnceCell<Box<LogLevelReloader>> = OnceCell::new();

pub fn register_log_level_reloader(
    reloader: impl Fn(&str) -> Result<(), String> + Send + Sync + 'static,
) {
    let _ = LOG_LEVEL_RELOADER.set(Box::new(reloader));
}

pub fn reload_log_level(level: &str) -> Result<(), String> {
    let reloader = LOG_LEVEL_RELOADER
        .get()
        .ok_or_else(|| "service logging reload is not initialized".to_string())?;
    reloader(level)
}
