/// Utilities for redirecting `log` crate messages to a custom handler.
///
/// In Rust, the `log` crate provides a facade over a globally-registered
/// logger (typically `env_logger` or `tracing-subscriber`). This module
/// offers a lightweight bridge that:
/// - Implements `log::Log` to capture and format log records
/// - Prefixes messages with `[lib_name]` for easy identification
/// - Provides best-effort global logger registration

use std::io::{Write, stderr};
use std::sync::atomic::{AtomicBool, Ordering};

use log::{Log, Metadata, Record, LevelFilter, SetLoggerError};

/// Route `log` crate records into a custom handler with consistent formatting.
///
/// The bridge prefixes every message with `[lib_name]` and writes to stderr.
/// It filters records so that only logs originating from the configured
/// `lib_name` (matching `Record::target()`) are forwarded, mirroring the
/// Python behaviour of `logging.getLogger(name)`.
pub struct LoguruBridge {
    lib_name: String,
    level_filter: LevelFilter,
    registered: AtomicBool,
}

impl LoguruBridge {
    /// Create a new bridge for the given library / module prefix.
    ///
    /// When `level` is `None` the bridge accepts all levels and defers
    /// filtering to the downstream logger (matches the Python default).
    pub fn new(lib_name: &str, level: Option<&str>) -> Self {
        let level_filter = level
            .and_then(|l| match l {
                "debug" | "DEBUG" => Some(LevelFilter::Debug),
                "info" | "INFO" => Some(LevelFilter::Info),
                "warn" | "warning" | "WARN" | "WARNING" => Some(LevelFilter::Warn),
                "error" | "ERROR" => Some(LevelFilter::Error),
                "trace" | "TRACE" => Some(LevelFilter::Trace),
                "off" | "OFF" => Some(LevelFilter::Off),
                _ => None,
            })
            .unwrap_or(LevelFilter::Trace);

        Self {
            lib_name: lib_name.to_string(),
            level_filter,
            registered: AtomicBool::new(false),
        }
    }

    /// Register the bridge handler with the `log` crate.
    ///
    /// This is a best-effort operation: if a logger is already registered,
    /// the call is a silent no-op (returns `Err` but does not panic).
    /// Subsequent calls after the first successful registration are no-ops.
    pub fn register(&self) -> Result<(), SetLoggerError> {
        if self.registered.swap(true, Ordering::SeqCst) {
            return Ok(());
        }

        let boxed = Box::new(Self {
            lib_name: self.lib_name.clone(),
            level_filter: self.level_filter,
            registered: AtomicBool::new(true),
        });

        log::set_boxed_logger(boxed).map(|()| {
            log::set_max_level(self.level_filter);
        })
    }

    /// Returns `true` if the record's target matches this bridge's lib prefix.
    fn matches_target(&self, record: &Record) -> bool {
        let target = record.target();
        target == self.lib_name || target.starts_with(&format!("{}::", self.lib_name)) || target.starts_with(&format!("{}.", self.lib_name))
    }
}

impl Log for LoguruBridge {
    fn enabled(&self, metadata: &Metadata) -> bool {
        metadata.level() <= self.level_filter
    }

    fn log(&self, record: &Record) {
        if !self.enabled(record.metadata()) {
            return;
        }

        if !self.matches_target(record) {
            return;
        }

        let mut stderr = stderr();
        let _ = writeln!(
            stderr,
            "[{level}] [{lib}] {message}",
            level = record.level(),
            lib = self.lib_name,
            message = record.args(),
        );
    }

    fn flush(&self) {
        let _ = stderr().flush();
    }
}

/// Redirect `log` crate messages from a specific module prefix into a custom handler.
///
/// Creates a [`LoguruBridge`] for `name` and attempts to register it as the
/// global logger. If a logger is already registered the call silently returns,
/// leaving the existing logger in place.
///
/// When `level` is `None`, the bridge accepts all levels — the downstream
/// logger's own level controls visibility.
///
/// # Example
///
/// ```no_run
/// use utils::redirect_lib_logging;
///
/// redirect_lib_logging("my_lib", None);
///
/// log::info!("hello from my_lib"); // => [INFO] [my_lib] hello from my_lib
/// ```
pub fn redirect_lib_logging(name: &str, level: Option<&str>) {
    let bridge = LoguruBridge::new(name, level);
    let _ = bridge.register();
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn level_parsing() {
        assert_eq!(LoguruBridge::new("test", Some("debug")).level_filter, LevelFilter::Debug);
        assert_eq!(LoguruBridge::new("test", Some("INFO")).level_filter, LevelFilter::Info);
        assert_eq!(LoguruBridge::new("test", Some("warning")).level_filter, LevelFilter::Warn);
        assert_eq!(LoguruBridge::new("test", Some("error")).level_filter, LevelFilter::Error);
        assert_eq!(LoguruBridge::new("test", None).level_filter, LevelFilter::Trace);
        assert_eq!(LoguruBridge::new("test", Some("invalid")).level_filter, LevelFilter::Trace);
    }

    #[test]
    fn target_matching_exact() {
        let bridge = LoguruBridge::new("my_lib", None);
        let metadata = Metadata::builder()
            .level(LevelFilter::Info.to_level().unwrap())
            .target("my_lib")
            .build();
        assert!(bridge.matches_target(&Record::builder().metadata(metadata).args(format_args!("")).build()));
    }

    #[test]
    fn target_matching_nested() {
        let bridge = LoguruBridge::new("my_lib", None);
        let metadata = Metadata::builder()
            .level(LevelFilter::Info.to_level().unwrap())
            .target("my_lib::sub::module")
            .build();
        assert!(bridge.matches_target(&Record::builder().metadata(metadata).args(format_args!("")).build()));
    }

    #[test]
    fn target_no_match() {
        let bridge = LoguruBridge::new("my_lib", None);
        let metadata = Metadata::builder()
            .level(LevelFilter::Info.to_level().unwrap())
            .target("other_lib::something")
            .build();
        assert!(!bridge.matches_target(&Record::builder().metadata(metadata).args(format_args!("")).build()));
    }

    #[test]
    fn enabled_respects_level() {
        let bridge = LoguruBridge::new("test", Some("warn"));
        let debug_meta = Metadata::builder()
            .level(log::Level::Debug)
            .target("test")
            .build();
        let warn_meta = Metadata::builder()
            .level(log::Level::Warn)
            .target("test")
            .build();
        assert!(!bridge.enabled(&debug_meta));
        assert!(bridge.enabled(&warn_meta));
    }
}
