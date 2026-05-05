use std::time::{Duration, SystemTime};

use anyhow::Result;
use tracing_appender::non_blocking::WorkerGuard;
use tracing_appender::rolling::{RollingFileAppender, Rotation};
use tracing_subscriber::{EnvFilter, fmt, layer::SubscriberExt, util::SubscriberInitExt};

use feline_core::util;

const LOG_RETENTION: Duration = Duration::from_secs(60 * 60 * 24 * 14);

pub fn init() -> Result<WorkerGuard> {
    let log_dir = util::log_dir();
    std::fs::create_dir_all(&log_dir)?;
    sweep_old_logs(&log_dir, LOG_RETENTION);

    let file_appender = RollingFileAppender::builder()
        .rotation(Rotation::DAILY)
        .filename_prefix("app")
        .filename_suffix("log")
        .build(&log_dir)?;

    let (non_blocking, guard) = tracing_appender::non_blocking(file_appender);

    let env_filter = EnvFilter::try_from_default_env()
        .unwrap_or_else(|_| EnvFilter::new("info,feline=debug,hyper=warn,reqwest=warn"));

    let file_layer = fmt::layer()
        .with_writer(non_blocking)
        .with_ansi(false)
        .with_target(true)
        .with_line_number(true);

    let console_layer = fmt::layer().with_target(false).with_line_number(false);

    tracing_subscriber::registry()
        .with(env_filter)
        .with(file_layer)
        .with(console_layer)
        .init();

    tracing::info!("logging initialized, log dir: {}", log_dir.display());
    Ok(guard)
}

fn sweep_old_logs(log_dir: &std::path::Path, retention: Duration) {
    let cutoff = match SystemTime::now().checked_sub(retention) {
        Some(t) => t,
        None => return,
    };
    let entries = match std::fs::read_dir(log_dir) {
        Ok(e) => e,
        Err(_) => return,
    };
    for entry in entries.flatten() {
        let Ok(name) = entry.file_name().into_string() else {
            continue;
        };
        if !(name.starts_with("app.") && name.ends_with(".log")) {
            continue;
        }
        let modified = entry.metadata().and_then(|m| m.modified()).ok();
        if let Some(modified) = modified
            && modified < cutoff
        {
            let _ = std::fs::remove_file(entry.path());
        }
    }
}
