use std::path::Path;

use tracing::Level;
use tracing_appender::non_blocking::WorkerGuard;
use tracing_subscriber::{fmt, prelude::*, EnvFilter};

/// Initialise structured logging to both a rotating JSON-lines file under
/// `log_dir` and stderr when a TTY is attached. The returned `WorkerGuard`
/// MUST be held for the lifetime of the process; dropping it flushes and
/// closes the file appender.
pub fn init(level: Level, log_dir: &Path) -> std::io::Result<WorkerGuard> {
    std::fs::create_dir_all(log_dir)?;

    let file_appender = tracing_appender::rolling::daily(log_dir, "hivegui.log");
    let (file_writer, guard) = tracing_appender::non_blocking(file_appender);

    let filter = EnvFilter::try_from_env("HIVEGUI_LOG_LEVEL")
        .unwrap_or_else(|_| EnvFilter::new(level.to_string()));

    let file_layer = fmt::layer()
        .json()
        .with_current_span(true)
        .with_span_list(false)
        .with_target(true)
        .with_writer(file_writer);

    let stderr_layer = fmt::layer()
        .json()
        .with_current_span(true)
        .with_span_list(false)
        .with_target(true)
        .with_writer(std::io::stderr);

    let _ = tracing_subscriber::registry()
        .with(filter)
        .with(file_layer)
        .with(stderr_layer)
        .try_init();

    Ok(guard)
}
