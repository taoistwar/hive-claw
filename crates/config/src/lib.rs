//! Configuration module for nanobot (Rust port of `nanobot.config`).

pub mod loader;
pub mod paths;
pub mod schema;

pub use loader::{ConfigError, clear_config_path, get_config_path, set_config_path};
pub use paths::{
    ensure_dir, get_bridge_install_dir, get_cli_history_path, get_cron_dir, get_data_dir,
    get_legacy_sessions_dir, get_logs_dir, get_media_dir, get_runtime_subdir, get_workspace_path,
    is_default_workspace,
};
pub use schema::Config;
