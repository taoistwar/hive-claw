//! Runtime path helpers derived from the active config context.
//!
//! Translated from `nanobot/config/paths.py`. Each helper mirrors the Python
//! function of the same name. Directories are created on access via
//! `ensure_dir` (equivalent to `nanobot.utils.helpers.ensure_dir`).

use std::io;
use std::path::{Path, PathBuf};

use crate::loader::get_config_path;

/// Create the directory (and parents) if missing and return the path.
///
/// Mirrors `nanobot.utils.helpers.ensure_dir`. Errors from the filesystem
/// are logged rather than propagated to match the Python helper's
/// best-effort behavior; the returned path is always the requested one.
pub fn ensure_dir<P: AsRef<Path>>(path: P) -> PathBuf {
    let p = path.as_ref().to_path_buf();
    if let Err(err) = std::fs::create_dir_all(&p) {
        if err.kind() != io::ErrorKind::AlreadyExists {
            log::warn!("Failed to create directory {:?}: {}", p, err);
        }
    }
    p
}

/// Expand a leading `~` or `~/` prefix using the current user's home dir.
fn expand_tilde(raw: &str) -> PathBuf {
    if raw == "~" {
        return dirs::home_dir().unwrap_or_else(|| PathBuf::from("~"));
    }
    if let Some(rest) = raw.strip_prefix("~/") {
        if let Some(home) = dirs::home_dir() {
            return home.join(rest);
        }
    }
    PathBuf::from(raw)
}

fn nanobot_home() -> PathBuf {
    dirs::home_dir()
        .unwrap_or_else(|| PathBuf::from("."))
        .join(".nanobot")
}

/// Instance-level runtime data directory (parent of the active config file).
pub fn get_data_dir() -> PathBuf {
    let cfg = get_config_path();
    let parent = cfg
        .parent()
        .map(Path::to_path_buf)
        .unwrap_or_else(|| PathBuf::from("."));
    ensure_dir(parent)
}

/// A named runtime subdirectory under the instance data dir.
pub fn get_runtime_subdir(name: &str) -> PathBuf {
    ensure_dir(get_data_dir().join(name))
}

/// Media directory, optionally namespaced per channel.
pub fn get_media_dir(channel: Option<&str>) -> PathBuf {
    let base = get_runtime_subdir("media");
    match channel {
        Some(ch) if !ch.is_empty() => ensure_dir(base.join(ch)),
        _ => base,
    }
}

/// Cron storage directory.
pub fn get_cron_dir() -> PathBuf {
    get_runtime_subdir("cron")
}

/// Logs directory.
pub fn get_logs_dir() -> PathBuf {
    get_runtime_subdir("logs")
}

/// WebUI-only persisted display threads directory.
pub fn get_webui_dir() -> PathBuf {
    get_runtime_subdir("webui")
}

/// Resolve and ensure the agent workspace path.
///
/// Passing `None` returns the default `~/.nanobot/workspace`.
pub fn get_workspace_path(workspace: Option<&str>) -> PathBuf {
    let path = match workspace {
        Some(w) if !w.is_empty() => expand_tilde(w),
        _ => nanobot_home().join("workspace"),
    };
    ensure_dir(path)
}

/// Whether the given workspace resolves to nanobot's default workspace path.
pub fn is_default_workspace<P: AsRef<Path>>(workspace: Option<P>) -> bool {
    let current = match workspace {
        Some(p) => {
            let s = p.as_ref().to_string_lossy().to_string();
            expand_tilde(&s)
        }
        None => nanobot_home().join("workspace"),
    };
    let default = nanobot_home().join("workspace");
    // Best-effort canonicalization — fall back to raw comparison if either
    // path does not exist yet.
    let a = std::fs::canonicalize(&current).unwrap_or(current);
    let b = std::fs::canonicalize(&default).unwrap_or(default);
    a == b
}

/// Shared CLI history file path.
pub fn get_cli_history_path() -> PathBuf {
    nanobot_home().join("history").join("cli_history")
}

/// Shared WhatsApp bridge installation directory.
pub fn get_bridge_install_dir() -> PathBuf {
    nanobot_home().join("bridge")
}

/// Legacy global session directory used for migration fallback.
pub fn get_legacy_sessions_dir() -> PathBuf {
    nanobot_home().join("sessions")
}
