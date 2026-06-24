use tracing::Level;
use tracing_subscriber::{fmt, prelude::*, EnvFilter};

pub fn init(level: Level) {
    let filter = EnvFilter::try_from_env("HIVECLAW_LOG_LEVEL")
        .unwrap_or_else(|_| EnvFilter::new(level.to_string()));

    let json_layer = fmt::layer()
        .json()
        .with_current_span(true)
        .with_span_list(false)
        .with_target(true)
        .with_writer(std::io::stderr);

    let _ = tracing_subscriber::registry()
        .with(filter)
        .with(json_layer)
        .try_init();
}
