use once_cell::sync::OnceCell;
use std::fs::{self, File, OpenOptions};
use std::io::{self, Write};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use tracing_subscriber::fmt::MakeWriter;

const MAX_ROLLED_LOG_FILES: usize = 3;

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
