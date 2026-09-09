use std::{env, process, process::ExitCode};

use hivegui::{config::Config, logging, ui, version};
use tracing::info;

fn log_startup_details(cfg: &Config) {
    let executable = env::current_exe()
        .ok()
        .and_then(|p| p.into_os_string().into_string().ok())
        .unwrap_or_else(|| "<unavailable>".to_string());
    let current_dir = env::current_dir()
        .ok()
        .and_then(|p| p.into_os_string().into_string().ok())
        .unwrap_or_else(|| "<unavailable>".to_string());
    info!(
        message = "HiveGUI starting",
        version = version::version(),
        pid = process::id(),
        executable = %executable,
        current_dir = %current_dir,
        headless = cfg.headless,
        log_level = %cfg.log_level,
        log_dir = %cfg.log_dir.display(),
        app_mode = "desktop",
    );
}

fn log_startup_error<E>(stage: &str, error: &E, cfg: Option<&Config>)
where
    E: std::fmt::Display + std::fmt::Debug,
{
    let executable = env::current_exe()
        .ok()
        .and_then(|p| p.into_os_string().into_string().ok())
        .unwrap_or_else(|| "<unavailable>".to_string());
    let current_dir = env::current_dir()
        .ok()
        .and_then(|p| p.into_os_string().into_string().ok())
        .unwrap_or_else(|| "<unavailable>".to_string());

    eprintln!("hivegui startup failed: {stage}");
    eprintln!("  pid: {}", process::id());
    eprintln!("  executable: {executable}");
    eprintln!("  current_dir: {current_dir}");
    eprintln!(
        "  RUST_LOG: {}",
        env::var("RUST_LOG").unwrap_or_else(|_| "<unset>".to_string())
    );
    eprintln!(
        "  HIVEGUI_LOG_LEVEL set: {}",
        env::var("HIVEGUI_LOG_LEVEL").is_ok()
    );
    eprintln!(
        "  HIVEGUI_HEADLESS set: {}",
        env::var("HIVEGUI_HEADLESS").is_ok()
    );
    eprintln!("  error: {error}");
    eprintln!("  error(debug): {error:#?}");

    if let Some(cfg) = cfg {
        eprintln!("  config:");
        eprintln!("    headless: {}", cfg.headless);
        eprintln!("    log_level: {}", cfg.log_level);
        eprintln!("    log_dir: {}", cfg.log_dir.display());
    }

    eprintln!("  hint: set RUST_BACKTRACE=1 and re-run for full backtrace.");
}

fn main() -> ExitCode {
    use std::process::ExitCode;

    let cfg = match Config::from_env() {
        Ok(c) => c,
        Err(e) => {
            log_startup_error("invalid configuration", &e, None);
            return ExitCode::from(2);
        }
    };

    let _guard = match logging::init(cfg.log_level, &cfg.log_dir) {
        Ok(g) => g,
        Err(e) => {
            log_startup_error("init logging", &e, Some(&cfg));
            return ExitCode::from(1);
        }
    };

    log_startup_details(&cfg);

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
            log_startup_error("create tokio runtime", &e, Some(&cfg));
            return ExitCode::from(1);
        }
    };
    let _guard_rt = rt.enter();

    if let Err(e) = ui::app::run(cfg) {
        log_startup_error("ui run", &e, None);
        return ExitCode::from(1);
    }

    ExitCode::SUCCESS
}
