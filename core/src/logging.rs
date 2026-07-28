use once_cell::sync::OnceCell;

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
