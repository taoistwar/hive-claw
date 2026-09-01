use std::env;
use std::path::PathBuf;
use std::str::FromStr;

use directories::ProjectDirs;
use thiserror::Error;

#[derive(Debug, Error)]
pub enum ConfigError {
    #[error(
        "HIVEGUI_LOG_LEVEL is not a valid level (expected one of trace, debug, info, warn, error): {0}"
    )]
    InvalidLogLevel(String),
    #[error("could not resolve a per-user data directory for HiveGUI logs")]
    NoLogDir,
}

#[derive(Debug, Clone)]
pub struct Config {
    pub log_level: tracing::Level,
    pub log_dir: PathBuf,
    pub headless: bool,
}

impl Config {
    pub fn from_env() -> Result<Self, ConfigError> {
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
            log_level,
            log_dir,
            headless,
        })
    }

    /// Test-only constructor that bypasses `from_env`. The returned
    /// configuration points at a non-existent log directory; production
    /// code uses `from_env` instead.
    pub fn for_test() -> Self {
        Self {
            log_level: tracing::Level::INFO,
            log_dir: PathBuf::from("/tmp/hivegui-test-logs"),
            headless: true,
        }
    }
}

impl Default for Config {
    fn default() -> Self {
        Self::for_test()
    }
}

fn default_log_dir() -> Option<PathBuf> {
    ProjectDirs::from("", "", "hivegui").map(|p| p.data_local_dir().join("logs"))
}
