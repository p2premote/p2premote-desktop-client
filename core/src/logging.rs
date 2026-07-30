use once_cell::sync::OnceCell;
use std::fs::{self, File, OpenOptions};
use std::io::{self, Write};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use tracing_subscriber::fmt::MakeWriter;

const MAX_ROLLED_LOG_FILES: usize = 3;

struct DailyLogFile {
    file: File,
    date: String,
    stem: String,
    parent: PathBuf,
}

impl DailyLogFile {
    fn new(log_dir: &Path, stem: &str) -> io::Result<Self> {
        let date = today();
        fs::create_dir_all(log_dir)?;
        let file = OpenOptions::new()
            .create(true)
            .append(true)
            .open(log_dir.join(format!("{stem}.log")))?;
        Ok(Self {
            file,
            date,
            stem: stem.to_string(),
            parent: log_dir.to_path_buf(),
        })
    }

    fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
        let current = today();
        if current != self.date {
            self.date = current;
            self.file = OpenOptions::new()
                .create(true)
                .append(true)
                .open(self.parent.join(format!("{}-{}.log", self.stem, self.date)))?;
            self.cleanup_rolled_logs();
        }
        self.file.write(buf)
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
        self.inner.lock().unwrap().file.flush()
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
