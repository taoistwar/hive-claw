use std::env;
use std::path::PathBuf;
use std::str::FromStr;

use thiserror::Error;

/// Product identity of one HiveGUI desktop binary.
///
/// HiveGUI ships two desktop-local entry points: the full desktop application
/// (`bin/hivegui`) and the focused prompt-engineering application
/// (`bin/ngy_prompt_studio`). Each one owns an independent per-user data
/// folder **and** an independent rotating log file name, so the two processes
/// can run side by side: a shared data root would put both of them behind the
/// same exclusive Store owner lock (`{root}/datasources.db.lock`), and a shared
/// log file would interleave their log lines.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct AppIdentity {
    /// Folder name under the per-user local data directory.
    data_folder: &'static str,
    /// Rotating log file name, written as `{log_file}.YYYY-MM-DD` under
    /// `{data_root}/logs`.
    log_file: &'static str,
}

impl AppIdentity {
    /// Full desktop HiveGUI application (`bin/hivegui`).
    pub const HIVEGUI: Self = Self {
        data_folder: "hivegui",
        log_file: "hivegui.log",
    };

    /// Focused prompt-engineering application (`bin/ngy_prompt_studio`).
    pub const NGY_PROMPT_STUDIO: Self = Self {
        data_folder: "ngy_prompt_studio",
        log_file: "ngy_prompt_studio.log",
    };

    /// Folder name under the per-user local data directory.
    pub fn data_folder(self) -> &'static str {
        self.data_folder
    }

    /// Rotating log file name.
    pub fn log_file(self) -> &'static str {
        self.log_file
    }

    /// Per-user data root. Every durable artifact this application owns lives
    /// under it: `datasources.db`, `datasources.db.lock`, `encryption.key`,
    /// snapshots, the `plugins` tree, and `logs`.
    pub fn data_root(self) -> PathBuf {
        dirs::data_local_dir()
            .unwrap_or_else(|| dirs::home_dir().unwrap_or_default())
            .join(self.data_folder)
    }

    /// Directory holding this application's rotating log files.
    pub fn log_dir(self) -> PathBuf {
        self.data_root().join("logs")
    }
}

#[derive(Debug, Error)]
pub enum ConfigError {
    #[error(
        "HIVEGUI_LOG_LEVEL is not a valid level (expected one of trace, debug, info, warn, error): {0}"
    )]
    InvalidLogLevel(String),
}

#[derive(Debug, Clone)]
pub struct Config {
    pub log_level: tracing::Level,
    pub log_dir: PathBuf,
    pub headless: bool,
}

impl Config {
    /// Resolve the process configuration for `app` from the environment.
    ///
    /// `HIVEGUI_LOG_LEVEL` and `HIVEGUI_HEADLESS` are shared by both entry
    /// points. `HIVEGUI_LOG_DIR` still overrides the log directory when set;
    /// otherwise logs go to `{app data root}/logs` so each application keeps
    /// its own log file.
    pub fn from_env(app: AppIdentity) -> Result<Self, ConfigError> {
        let level_str = env::var("HIVEGUI_LOG_LEVEL").unwrap_or_else(|_| "info".to_string());
        let log_level = tracing::Level::from_str(&level_str)
            .map_err(|_| ConfigError::InvalidLogLevel(level_str))?;

        let log_dir = match env::var("HIVEGUI_LOG_DIR") {
            Ok(p) => PathBuf::from(p),
            Err(_) => app.log_dir(),
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

#[cfg(test)]
mod tests {
    use super::*;

    /// Each HiveGUI binary must resolve to its own data folder and its own
    /// rotating log file, otherwise the two applications contend for one Store
    /// owner lock and write into one log file.
    #[test]
    fn app_identities_are_isolated() {
        let desktop = AppIdentity::HIVEGUI;
        let studio = AppIdentity::NGY_PROMPT_STUDIO;

        assert_eq!(desktop.data_folder(), "hivegui");
        assert_eq!(studio.data_folder(), "ngy_prompt_studio");
        assert_eq!(desktop.log_file(), "hivegui.log");
        assert_eq!(studio.log_file(), "ngy_prompt_studio.log");

        assert_ne!(desktop.data_root(), studio.data_root());
        assert_ne!(desktop.log_dir(), studio.log_dir());
        assert_eq!(studio.log_dir(), studio.data_root().join("logs"));
        assert_eq!(
            studio
                .data_root()
                .file_name()
                .and_then(|name| name.to_str()),
            Some("ngy_prompt_studio")
        );
    }
}
