//! Minimal demo entry point — loads a config (if present) and runs a
//! single prompt through [`nanobot_main::Nanobot`].
//!
//! This mirrors the Python `python -m nanobot.nanobot` usage: build the
//! facade, dispatch one message, print the result.

use std::process::ExitCode;

#[tokio::main]
async fn main() -> ExitCode {
    env_logger::builder()
        .format_timestamp_millis()
        .try_init()
        .ok();

    match cli::dispatch().await {
        Ok(()) => ExitCode::SUCCESS,
        Err(e) => {
            eprintln!("error: {e}");
            ExitCode::FAILURE
        }
    }
}
