//! Configuration loading utilities.
//!
//! Translated from `nanobot/config/loader.py`. Highlights:
//! * A global "current config path" is stored in a `Mutex` (Python used a
//!   module-level `_current_config_path` variable).
//! * [`load_config`] runs a legacy migration pass and then falls back to the
//!   default [`Config`] when deserialization fails, matching the Python
//!   "log and continue" behavior.
//! * [`config.resolve_env_vars`] expands `${VAR}` references via a JSON
//!   round-trip. The Rust version has no reflection so we walk a
//!   `serde_json::Value` tree instead of the concrete struct.

use std::fs;
use std::io;
use std::path::{Path, PathBuf};
use std::sync::{Mutex, OnceLock};

use regex::Regex;
use serde_json::Value;
use thiserror::Error;

use crate::schema::Config;
const PKG_NAME: &str = "nanobot";
const CONFIG_FILE_NAME: &str = "config.json";

// ---------------------------------------------------------------------------
// Global config-path registry
// ---------------------------------------------------------------------------

fn current_config_path() -> &'static Mutex<Option<PathBuf>> {
    static CELL: OnceLock<Mutex<Option<PathBuf>>> = OnceLock::new();
    CELL.get_or_init(|| Mutex::new(None))
}

/// Set the current config path (used to derive the data directory).
pub fn set_config_path<P: Into<PathBuf>>(path: P) {
    let mut guard = current_config_path()
        .lock()
        .expect("config-path mutex poisoned");
    *guard = Some(path.into());
}

/// Clear the globally configured config path (primarily useful in tests).
pub fn clear_config_path() {
    let mut guard = current_config_path()
        .lock()
        .expect("config-path mutex poisoned");
    *guard = None;
}

/// Get the current configuration file path.
///
/// Returns the value set via [`set_config_path`] if present; otherwise the
/// default `~/.{PKG_NAME}/{CONFIG_FILE_NAME}`.
pub fn get_config_path() -> PathBuf {
    if let Some(p) = current_config_path()
        .lock()
        .expect("config-path mutex poisoned")
        .clone()
    {
        return p;
    }
    dirs::home_dir()
        .unwrap_or_else(|| PathBuf::from("."))
        .join(format!(".{}", PKG_NAME))
        .join(CONFIG_FILE_NAME)
}

// ---------------------------------------------------------------------------
// Errors
// ---------------------------------------------------------------------------

#[derive(Debug, Error)]
pub enum ConfigError {
    #[error("I/O error: {0}")]
    Io(#[from] io::Error),
    #[error("JSON error: {0}")]
    Json(#[from] serde_json::Error),
    #[error("Environment variable '{0}' referenced in config is not set")]
    MissingEnvVar(String),
}

// ---------------------------------------------------------------------------
// Load / Save
// ---------------------------------------------------------------------------

pub(crate) fn try_load(path: &Path) -> Result<Config, ConfigError> {
    let text = fs::read_to_string(path)?;
    let mut data: Value = serde_json::from_str(&text)?;
    migrate_config(&mut data);
    let cfg: Config = serde_json::from_value(data)?;
    Ok(cfg)
}

// ---------------------------------------------------------------------------
// ${VAR} env-var resolution
// ---------------------------------------------------------------------------
fn env_ref_regex() -> &'static Regex {
    static RE: OnceLock<Regex> = OnceLock::new();
    RE.get_or_init(|| Regex::new(r"\$\{([A-Za-z_][A-Za-z0-9_]*)\}").unwrap())
}

pub(crate) fn resolve_value(value: &mut Value) -> Result<(), ConfigError> {
    match value {
        Value::String(s) => {
            if let Some(replaced) = expand_env_string(s)? {
                *s = replaced;
            }
        }
        Value::Array(items) => {
            for item in items {
                resolve_value(item)?;
            }
        }
        Value::Object(map) => {
            for (_, v) in map.iter_mut() {
                resolve_value(v)?;
            }
        }
        _ => {}
    }
    Ok(())
}

fn expand_env_string(input: &str) -> Result<Option<String>, ConfigError> {
    let re = env_ref_regex();
    if !re.is_match(input) {
        return Ok(None);
    }
    let mut missing: Option<String> = None;
    let out = re.replace_all(input, |caps: &regex::Captures<'_>| {
        let name = &caps[1];
        match std::env::var(name) {
            Ok(v) => v,
            Err(_) => {
                if missing.is_none() {
                    missing = Some(name.to_string());
                }
                String::new()
            }
        }
    });
    if let Some(name) = missing {
        return Err(ConfigError::MissingEnvVar(name));
    }
    Ok(Some(out.into_owned()))
}

// ---------------------------------------------------------------------------
// Legacy migrations
// ---------------------------------------------------------------------------

/// Mutate the raw JSON value in place to migrate older config layouts.
///
/// Mirrors Python's `_migrate_config`:
/// * `tools.exec.restrictToWorkspace` → `tools.restrictToWorkspace`
/// * `tools.myEnabled` / `tools.mySet` → `tools.my.{enable, allowSet}`
pub fn migrate_config(data: &mut Value) {
    let Some(root) = data.as_object_mut() else {
        return;
    };
    let Some(tools) = root.get_mut("tools").and_then(|v| v.as_object_mut()) else {
        return;
    };

    // exec.restrictToWorkspace → tools.restrictToWorkspace
    let exec_restrict = tools
        .get_mut("exec")
        .and_then(|v| v.as_object_mut())
        .and_then(|m| m.remove("restrictToWorkspace"));
    if let Some(val) = exec_restrict {
        tools.entry("restrictToWorkspace").or_insert(val);
    }

    // tools.myEnabled / tools.mySet → tools.my.{enable, allowSet}
    let popped_enabled = tools.remove("myEnabled");
    let popped_set = tools.remove("mySet");
    if popped_enabled.is_some() || popped_set.is_some() {
        let my_entry = tools
            .entry("my")
            .or_insert_with(|| Value::Object(Default::default()));
        if !my_entry.is_object() {
            *my_entry = Value::Object(Default::default());
        }
        let my_obj = my_entry.as_object_mut().expect("my is object");
        if let Some(v) = popped_enabled {
            my_obj.entry("enable").or_insert(v);
        }
        if let Some(v) = popped_set {
            my_obj.entry("allowSet").or_insert(v);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn migration_moves_exec_restrict_and_my_flags() {
        let mut v: Value = serde_json::json!({
            "tools": {
                "exec": {"restrictToWorkspace": true, "timeout": 10},
                "myEnabled": false,
                "mySet": true
            }
        });
        migrate_config(&mut v);
        let tools = v.get("tools").unwrap();
        assert_eq!(tools.get("restrictToWorkspace"), Some(&Value::Bool(true)));
        assert_eq!(
            tools.get("my").and_then(|m| m.get("enable")),
            Some(&Value::Bool(false))
        );
        assert_eq!(
            tools.get("my").and_then(|m| m.get("allowSet")),
            Some(&Value::Bool(true))
        );
        assert!(tools.get("myEnabled").is_none());
        assert!(tools.get("mySet").is_none());
        assert!(
            tools
                .get("exec")
                .unwrap()
                .get("restrictToWorkspace")
                .is_none()
        );
    }

    #[test]
    fn env_var_expansion() {
        unsafe {
            std::env::set_var("NANOBOT_TEST_KEY", "secret");
        }
        let mut cfg = Config::default();
        cfg.providers.openai.api_key = Some("${NANOBOT_TEST_KEY}".into());
        let resolved = cfg.resolve_env_vars().unwrap();
        assert_eq!(resolved.providers.openai.api_key.as_deref(), Some("secret"));
    }

    #[test]
    fn env_var_missing_is_error() {
        let mut cfg = Config::default();
        cfg.providers.openai.api_key = Some("${__NANOBOT_UNSET_VAR__}".into());
        let err = cfg.resolve_env_vars().unwrap_err();
        assert!(matches!(err, ConfigError::MissingEnvVar(_)));
    }
}
