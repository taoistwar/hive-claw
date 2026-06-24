use std::process::ExitCode;

use hivegui::{config::Config, logging, ui, version};
use tracing::info;

fn main() -> ExitCode {
    let cfg = match Config::from_env() {
        Ok(c) => c,
        Err(e) => {
            eprintln!("hivegui: invalid configuration: {e}");
            return ExitCode::from(2);
        }
    };

    let _guard = match logging::init(cfg.log_level, &cfg.log_dir) {
        Ok(g) => g,
        Err(e) => {
            eprintln!("hivegui: could not initialise logging: {e}");
            return ExitCode::from(1);
        }
    };

    info!(
        message = "HiveGUI starting",
        version = version::version(),
        hiveclaw_url = %cfg.hiveclaw_url,
        log_dir = %cfg.log_dir.display(),
    );

    if cfg.headless {
        // Test-only short-circuit: exit cleanly before opening the gpui
        // window. The smoke test (T016) sets HIVEGUI_HEADLESS=1 to verify
        // that the binary links and reaches the run point without
        // requiring a display server.
        info!(message = "HiveGUI headless mode: exiting before window open");
        return ExitCode::SUCCESS;
    }

    // T078: stand up a Tokio runtime BEFORE entering the gpui event loop
    // so async HTTP calls dispatched from view handlers (via gpui's
    // AsyncApp::spawn) can await reqwest futures, which require a Tokio
    // context. We use a multi-thread runtime so SSE streams can make
    // progress in the background while the gpui main thread renders.
    let rt = match tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .worker_threads(2)
        .thread_name("hivegui-http")
        .build()
    {
        Ok(rt) => rt,
        Err(e) => {
            eprintln!("hivegui: could not start Tokio runtime: {e}");
            return ExitCode::from(1);
        }
    };
    let _guard_rt = rt.enter();

    if let Err(e) = ui::app::run(cfg) {
        eprintln!("hivegui: ui crashed: {e:#}");
        return ExitCode::from(1);
    }

    ExitCode::SUCCESS
}
