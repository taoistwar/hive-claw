use std::env;
use std::path::PathBuf;
use std::str::FromStr;

use directories::ProjectDirs;
use thiserror::Error;
use url::Url;

#[derive(Debug, Error)]
pub enum ConfigError {
    #[error("HIVECLAW_URL is not a valid URL: {0}")]
    InvalidHiveclawUrl(String),
    #[error("HIVEGUI_LOG_LEVEL is not a valid level (expected one of trace, debug, info, warn, error): {0}")]
    InvalidLogLevel(String),
    #[error("could not resolve a per-user data directory for HiveGUI logs")]
    NoLogDir,
}

#[derive(Debug, Clone)]
pub struct Config {
    pub hiveclaw_url: Url,
    pub log_level: tracing::Level,
    pub log_dir: PathBuf,
    pub headless: bool,
}

impl Config {
    pub fn from_env() -> Result<Self, ConfigError> {
        let url_str =
            env::var("HIVECLAW_URL").unwrap_or_else(|_| "http://127.0.0.1:8686".to_string());
        let hiveclaw_url =
            Url::parse(&url_str).map_err(|_| ConfigError::InvalidHiveclawUrl(url_str))?;

        let level_str = env::var("HIVEGUI_LOG_LEVEL").unwrap_or_else(|_| "info".to_string());
        let log_level = tracing::Level::from_str(&level_str)
            .map_err(|_| ConfigError::InvalidLogLevel(level_str))?;

        let log_dir = match env::var("HIVEGUI_LOG_DIR") {
            Ok(p) => PathBuf::from(p),
            Err(_) => default_log_dir().ok_or(ConfigError::NoLogDir)?,
        };

        let headless = matches!(
            env::var("HIVEGUI_HEADLESS").as_deref(),
            Ok("1") | Ok("true")
        );

        Ok(Config {
            hiveclaw_url,
            log_level,
            log_dir,
            headless,
        })
    }
}

fn default_log_dir() -> Option<PathBuf> {
    // Per the quickstart documentation, the canonical Linux log path is
    // `$XDG_DATA_HOME/hivegui/logs`. The `directories` crate's `ProjectDirs`
    // honours XDG on Linux and produces `Library/Application Support/hivegui`
    // on macOS when initialised with empty qualifier/organization.
    ProjectDirs::from("", "", "hivegui").map(|p| p.data_local_dir().join("logs"))
}
