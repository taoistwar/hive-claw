use std::process::ExitCode;

use hiveclaw::{config::Config, http, logging, version};
use tokio::net::TcpListener;
use tokio::signal;
use tracing::info;

fn main() -> ExitCode {
    let cfg = match Config::from_env() {
        Ok(c) => c,
        Err(e) => {
            eprintln!("hiveclaw: invalid configuration: {e}");
            return ExitCode::from(2);
        }
    };

    logging::init(cfg.log_level);

    let rt = match tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()
    {
        Ok(rt) => rt,
        Err(e) => {
            eprintln!("hiveclaw: could not start Tokio runtime: {e}");
            return ExitCode::from(1);
        }
    };

    match rt.block_on(run(cfg)) {
        Ok(()) => ExitCode::SUCCESS,
        Err(e) => {
            eprintln!("hiveclaw: runtime error: {e:#}");
            ExitCode::from(1)
        }
    }
}

async fn run(cfg: Config) -> anyhow::Result<()> {
    let listener = TcpListener::bind(cfg.bind_addr).await?;
    let local_addr = listener.local_addr()?;

    info!(
        message = "HiveClaw listening",
        bind_addr = %local_addr,
        version = version::version(),
    );

    let app = http::router();
    axum::serve(listener, app)
        .with_graceful_shutdown(shutdown_signal())
        .await?;

    info!(message = "HiveClaw stopped");
    Ok(())
}

async fn shutdown_signal() {
    let ctrl_c = async {
        let _ = signal::ctrl_c().await;
    };

    #[cfg(unix)]
    let terminate = async {
        if let Ok(mut sig) = signal::unix::signal(signal::unix::SignalKind::terminate()) {
            sig.recv().await;
        }
    };

    #[cfg(not(unix))]
    let terminate = std::future::pending::<()>();

    tokio::select! {
        _ = ctrl_c => {},
        _ = terminate => {},
    }
}
