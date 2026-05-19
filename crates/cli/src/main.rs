use std::process::ExitCode;

#[tokio::main]
async fn main() -> ExitCode {
    let _ = env_logger_try_init();
    match cli::dispatch().await {
        Ok(()) => ExitCode::SUCCESS,
        Err(e) => {
            eprintln!("error: {e}");
            ExitCode::FAILURE
        }
    }
}

/// Best-effort logger init — never fails the CLI if the env_logger
/// feature isn't pulled in (keeps the binary runnable in release mode
/// where init-by-side-effect is cheap to skip).
fn env_logger_try_init() -> Result<(), ()> {
    // Intentionally no-op: `log` calls from the agent surface through
    // any external logger the embedding process wires up. We don't want
    // to pull env_logger into the workspace just for the CLI.
    Ok(())
}
