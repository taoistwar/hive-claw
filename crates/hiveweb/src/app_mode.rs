//! Process-wide application mode.

use std::sync::OnceLock;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AppMode {
    Production,
    Development,
    Test,
}

pub static APP_MODE: OnceLock<AppMode> = OnceLock::new();

impl AppMode {
    pub fn is_production(self) -> bool {
        self == Self::Production
    }

    pub fn should_register_builtins(self) -> bool {
        !self.is_production()
    }
}

/// Initialize the process-wide mode. The CLI value takes precedence over `APP_ENV`.
pub fn init(cli_mode: Option<&str>) -> AppMode {
    let mode = resolve(cli_mode, std::env::var("APP_ENV").ok().as_deref());
    *APP_MODE.get_or_init(|| mode)
}

/// Return the initialized mode, defaulting to development for library/test callers.
pub fn get() -> AppMode {
    *APP_MODE.get_or_init(|| resolve(None, std::env::var("APP_ENV").ok().as_deref()))
}

fn resolve(cli_mode: Option<&str>, env_mode: Option<&str>) -> AppMode {
    cli_mode
        .and_then(parse)
        .or_else(|| env_mode.and_then(parse))
        .unwrap_or(AppMode::Development)
}

fn parse(value: &str) -> Option<AppMode> {
    match value.trim().to_ascii_lowercase().as_str() {
        "prod" | "production" => Some(AppMode::Production),
        "dev" | "development" => Some(AppMode::Development),
        "test" | "testing" => Some(AppMode::Test),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::{AppMode, resolve};

    #[test]
    fn cli_mode_takes_precedence() {
        assert_eq!(resolve(Some("test"), Some("production")), AppMode::Test);
    }

    #[test]
    fn resolves_all_three_modes() {
        assert_eq!(resolve(None, Some("prod")), AppMode::Production);
        assert_eq!(resolve(None, Some("development")), AppMode::Development);
        assert_eq!(resolve(None, Some("testing")), AppMode::Test);
    }

    #[test]
    fn defaults_to_development() {
        assert_eq!(resolve(None, None), AppMode::Development);
    }
}
