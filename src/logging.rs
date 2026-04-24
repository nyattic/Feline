use anyhow::Result;
use tracing_appender::non_blocking::WorkerGuard;
use tracing_appender::rolling::{RollingFileAppender, Rotation};
use tracing_subscriber::{EnvFilter, fmt, layer::SubscriberExt, util::SubscriberInitExt};

use crate::util::exe_dir;

pub fn init() -> Result<WorkerGuard> {
    let log_dir = exe_dir().join("log");
    std::fs::create_dir_all(&log_dir)?;

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
