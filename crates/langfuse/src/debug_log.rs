//! Langfuse debug file logger.
//!
//! All Langfuse tracing events are written to `langfuse_debug.log`
//! in the current working directory so we can trace every step of
//! the ingest pipeline.

use std::fs::{File, OpenOptions};
use std::io::{BufWriter, Write};
use std::sync::Mutex;
use chrono::Local;

/// Global file writer for Langfuse debug logs.
static DEBUG_LOG: std::sync::LazyLock<Mutex<Option<BufWriter<File>>>> =
    std::sync::LazyLock::new(|| {
        let file = match OpenOptions::new()
            .create(true)
            .append(true)
            .open("langfuse_debug.log")
        {
            Ok(f) => {
                eprintln!("[langfuse debug] log file opened: langfuse_debug.log");
                Some(BufWriter::new(f))
            }
            Err(e) => {
                eprintln!("[langfuse debug] failed to open log file: {e}");
                None
            }
        };
        Mutex::new(file)
    });

/// Write a timestamped debug line to the Langfuse log file and stderr.
#[allow(dead_code)]
pub fn debug_log(msg: &str) {
    let ts = Local::now().format("%Y-%m-%dT%H:%M:%S%.3f").to_string();
    let line = format!("[{}] {}\n", ts, msg);

    // Always print to stderr so the user can see it immediately.
    eprint!("[lf:debug] {}", line);

    if let Ok(mut guard) = DEBUG_LOG.lock() {
        if let Some(ref mut writer) = *guard {
            let _ = writer.write_all(line.as_bytes());
            let _ = writer.flush();
        }
    }
}

/// Macro for convenience — behaves like `format!` but writes to debug log.
#[macro_export]
macro_rules! lf_debug {
    ($($arg:tt)*) => {
        $crate::debug_log::debug_log(&format!($($arg)*))
    };
}
