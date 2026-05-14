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

    if let Err(e) = ui::app::run(cfg) {
        eprintln!("hivegui: ui crashed: {e:#}");
        return ExitCode::from(1);
    }

    ExitCode::SUCCESS
}
