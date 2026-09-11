//! Shared process startup for HiveGUI entry points.
//!
//! Both the full desktop binary (`main.rs`) and the focused
//! prompt-engineering binary (`bin/ngy_prompt_studio.rs`) need the same
//! configuration parsing, logging bootstrap, Tokio runtime creation, and
//! exit-code mapping. [`launch`] centralizes that boilerplate so the
//! binaries stay thin and cannot drift in their startup behavior; each binary
//! only passes its own [`AppIdentity`], which selects the independent data
//! folder and rotating log file name.

use std::{
    env,
    process::{ExitCode, id},
};

use crate::config::{AppIdentity, Config};
use tracing::info;

fn log_startup_details(app: AppIdentity, cfg: &Config) {
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
        app = app.data_folder(),
        data_root = %app.data_root().display(),
        log_file = app.log_file(),
        version = crate::version::version(),
        pid = id(),
        executable = %executable,
        current_dir = %current_dir,
        headless = cfg.headless,
        log_level = %cfg.log_level,
        log_dir = %cfg.log_dir.display(),
    );
}

fn log_startup_error<E>(app: AppIdentity, stage: &str, error: &E, cfg: Option<&Config>)
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

    eprintln!("{} startup failed: {stage}", app.data_folder());
    eprintln!("  pid: {}", id());
    eprintln!("  executable: {executable}");
    eprintln!("  current_dir: {current_dir}");
    eprintln!("  app: {}", app.data_folder());
    eprintln!("  data_root: {}", app.data_root().display());
    eprintln!("  log_file: {}", app.log_file());
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

/// Parse configuration, initialize logging and a Tokio runtime, then run the
/// GPUI application produced by `run_app`. Every startup failure is mapped to
/// the documented exit code and reported to stderr; success returns
/// [`ExitCode::SUCCESS`].
///
/// `app` selects the per-user data folder and the rotating log file name; the
/// two HiveGUI binaries pass different identities so they never share a data
/// root or a log file, and every startup diagnostic (stderr report, `"HiveGUI
/// starting"` event) names that identity instead of a hard-coded product name.
///
/// `run_app` is expected to open its own Store/LlmStore via
/// [`crate::ui::app::bootstrap_stores`] and block on the GPUI event loop.
pub fn launch<F>(app: AppIdentity, run_app: F) -> ExitCode
where
    F: FnOnce(Config) -> anyhow::Result<()>,
{
    let cfg = match Config::from_env(app) {
        Ok(c) => c,
        Err(e) => {
            log_startup_error(app, "invalid configuration", &e, None);
            return ExitCode::from(2);
        }
    };

    let _guard = match crate::logging::init(cfg.log_level, &cfg.log_dir, app.log_file()) {
        Ok(g) => g,
        Err(e) => {
            log_startup_error(app, "init logging", &e, Some(&cfg));
            return ExitCode::from(1);
        }
    };

    log_startup_details(app, &cfg);

    if cfg.headless {
        // Test-only short-circuit: exit cleanly before opening the GPUI
        // window. The smoke test (T016) sets HIVEGUI_HEADLESS=1 to verify
        // that the binary links and reaches the run point without a display.
        info!(message = "HiveGUI headless mode: exiting before window open");
        return ExitCode::SUCCESS;
    }

    // Stand up a Tokio runtime BEFORE entering the GPUI event loop so async
    // HTTP calls dispatched from view handlers can await reqwest futures.
    // A multi-thread runtime lets SSE streams progress in the background
    // while the GPUI main thread renders.
    let rt = match tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .worker_threads(2)
        .thread_name("hivegui-http")
        .build()
    {
        Ok(rt) => rt,
        Err(e) => {
            log_startup_error(app, "create tokio runtime", &e, Some(&cfg));
            return ExitCode::from(1);
        }
    };
    let _guard_rt = rt.enter();

    if let Err(e) = run_app(cfg) {
        log_startup_error(app, "ui run", &e, None);
        return ExitCode::from(1);
    }

    ExitCode::SUCCESS
}
