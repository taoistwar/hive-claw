use std::process::ExitCode;

use hivegui::config::AppIdentity;
use hivegui::startup;
use hivegui::ui::app;

/// Full HiveGUI desktop client entry point. All startup boilerplate
/// (config parsing, logging, Tokio runtime, exit-code mapping) lives in
/// [`hivegui::startup::launch`]; this binary only selects its own
/// [`AppIdentity`] — and therefore its own data folder and log file — and
/// which GPUI application to run.
fn main() -> ExitCode {
    startup::launch(AppIdentity::HIVEGUI, |cfg| app::run(cfg))
}
