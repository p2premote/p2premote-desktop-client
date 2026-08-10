use once_cell::sync::{Lazy, OnceCell};
use serde::Serialize;
use std::fs::{self, File, OpenOptions};
use std::io::{self, Write};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use tracing_subscriber::fmt::MakeWriter;

const MAX_ROLLED_LOG_FILES: usize = 3;
pub const AUDIT_LOG_FILE_NAME: &str = "p2premote-audit.log";

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
