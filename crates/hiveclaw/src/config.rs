use std::env;
use std::net::SocketAddr;
use std::str::FromStr;

use thiserror::Error;

#[derive(Debug, Error)]
pub enum ConfigError {
    #[error("HIVECLAW_BIND_ADDR is not a valid socket address: {0}")]
    InvalidBindAddr(String),
    #[error("HIVECLAW_LOG_LEVEL is not a valid level (expected one of trace, debug, info, warn, error): {0}")]
    InvalidLogLevel(String),
}

#[derive(Debug, Clone)]
pub struct Config {
    pub bind_addr: SocketAddr,
    pub log_level: tracing::Level,
}

impl Config {
    pub fn from_env() -> Result<Self, ConfigError> {
        let bind_addr_str =
            env::var("HIVECLAW_BIND_ADDR").unwrap_or_else(|_| "127.0.0.1:8686".to_string());
        let bind_addr = SocketAddr::from_str(&bind_addr_str)
            .map_err(|_| ConfigError::InvalidBindAddr(bind_addr_str))?;

        let log_level_str = env::var("HIVECLAW_LOG_LEVEL").unwrap_or_else(|_| "info".to_string());
        let log_level = tracing::Level::from_str(&log_level_str)
            .map_err(|_| ConfigError::InvalidLogLevel(log_level_str))?;

        Ok(Config {
            bind_addr,
            log_level,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn defaults_when_env_unset() {
        std::env::remove_var("HIVECLAW_BIND_ADDR");
        std::env::remove_var("HIVECLAW_LOG_LEVEL");
        let cfg = Config::from_env().unwrap();
        assert_eq!(cfg.bind_addr.to_string(), "127.0.0.1:8686");
        assert_eq!(cfg.log_level, tracing::Level::INFO);
    }

    #[test]
    fn rejects_garbage_bind_addr() {
        std::env::set_var("HIVECLAW_BIND_ADDR", "not-a-socket");
        let err = Config::from_env().unwrap_err();
        assert!(matches!(err, ConfigError::InvalidBindAddr(_)));
        std::env::remove_var("HIVECLAW_BIND_ADDR");
    }
}
