//! Shared CLI runtime helpers.
//!
//! This module mirrors the Python `_load_runtime_config` / `_make_provider`
//! / `_migrate_cron_store` helpers from `nanobot/cli/commands.py`, plus a
//! small `LoopConfig::from_config` helper used by every command that spins up an
//! [`AgentLoop`].

use std::path::{Path, PathBuf};
use std::sync::Arc;

use agent::{AgentLoop, BuiltinToolSet, LoopConfig, ToolFactoryConfig, ToolFactoryDeps};
use bus::MessageBus;
use config::schema::Config;
use config::{get_config_path, paths::is_default_workspace, set_config_path};
use session::SessionManager;
use tokio::sync::Mutex;

/// Resolved runtime: the active [`Config`] plus the path it was loaded
/// from (or `None` when defaults were used).
pub struct Runtime {
    pub config: Config,
    pub config_path: Option<PathBuf>,
}

impl Runtime {
    /// Mirror of Python `_load_runtime_config`.
    ///
    /// * Sets the global config path when `config_path` is supplied so all
    ///   path helpers see the override.
    /// * Loads + env-var-expands the config.
    /// * Optionally rewrites `agents.defaults.workspace` from `--workspace`.
    pub fn from_config(
        config_path: Option<&Path>,
        workspace: Option<&Path>,
    ) -> Result<Runtime, String> {
        let resolved: Option<PathBuf> = match config_path {
            Some(p) => {
                let path = expand_tilde(p);
                if !path.exists() {
                    return Err(format!("Config file not found: {}", path.display()));
                }
                set_config_path(&path);
                Some(path)
            }
            None => None,
        };

        let cfg = Config::from_config(resolved.as_deref());
        let mut cfg = cfg.resolve_env_vars().map_err(|e| e.to_string())?;

        if let Some(ws) = workspace {
            cfg.agents.defaults.workspace = expand_tilde(ws).to_string_lossy().into_owned();
        }
        warn_deprecated_config_keys(resolved.as_deref());
        Ok(Runtime {
            config: cfg,
            config_path: resolved,
        })
    }
}

/// Hint users to remove obsolete keys from their config file (Python
/// `_warn_deprecated_config_keys`).
fn warn_deprecated_config_keys(config_path: Option<&Path>) {
    let path = match config_path {
        Some(p) => p.to_path_buf(),
        None => get_config_path(),
    };
    let Ok(text) = std::fs::read_to_string(&path) else {
        return;
    };
    let Ok(value) = serde_json::from_str::<serde_json::Value>(&text) else {
        return;
    };
    let has_legacy = value
        .get("agents")
        .and_then(|a| a.get("defaults"))
        .and_then(|d| d.get("memoryWindow"))
        .is_some();
    if has_legacy {
        eprintln!(
            "hint: `memoryWindow` in your config is no longer used and can be safely removed."
        );
    }
}

/// One-time migration: move legacy global cron store into the workspace
/// (Python `_migrate_cron_store`). Only fires for the default workspace.
pub fn migrate_cron_store(cfg: &Config) {
    let workspace = cfg.workspace_path();
    if !is_default_workspace(Some(&workspace)) {
        return;
    }
    let legacy = config::paths::get_cron_dir().join("jobs.json");
    let new_path = workspace.join("cron").join("jobs.json");
    if legacy.is_file() && !new_path.exists() {
        if let Some(parent) = new_path.parent() {
            let _ = std::fs::create_dir_all(parent);
        }
        let _ = std::fs::rename(&legacy, &new_path);
    }
}

/// Output of [`LoopBundle::build_agent_loop`].
pub struct LoopBundle {
    pub bus: Arc<MessageBus>,
    pub agent: AgentLoop,
    pub builtin_message: Arc<agent::tools::MessageTool>,
    pub builtin_cron: Option<Arc<agent::tools::CronTool>>,
}

impl LoopBundle {
    /// Build a fully-wired [`AgentLoop`] from `cfg` plus an optional cron
    /// service (passes the cron service through the tool factory so the
    /// `cron` tool is registered).
    pub async fn build_agent_loop(
        cfg: &Config,
        cron: Option<Arc<::cron::CronService>>,
    ) -> Result<Self, String> {
        let provider = providers::make_provider(cfg)?;
        let bus = Arc::new(MessageBus::new());
        let tf = ToolFactoryConfig::from_config(cfg);
        let deps = ToolFactoryDeps::new_with_cron(cron);
        let builtin = BuiltinToolSet::default_tools(tf, bus.clone(), deps).await;

        let lc = LoopConfig::from_config(cfg);
        let sessions = Arc::new(Mutex::new(SessionManager::new(cfg.workspace_path())));

        let message_handle = builtin.message.clone();
        let cron_handle = builtin.cron.clone();

        let mut agent = AgentLoop::from_builtin(bus.clone(), provider, sessions, builtin, lc);
        agent.set_mcp_servers(cfg.tools.mcp_servers.clone());
        Ok(Self {
            bus,
            agent,
            builtin_message: message_handle,
            builtin_cron: cron_handle,
        })
    }
}

/// Expand a leading `~` / `~/` prefix to the user's HOME directory.
pub fn expand_tilde(path: &Path) -> PathBuf {
    let s = path.to_string_lossy();
    if s == "~"
        && let Some(home) = dirs::home_dir()
    {
        return home;
    }
    if let Some(rest) = s.strip_prefix("~/")
        && let Some(home) = dirs::home_dir()
    {
        return home.join(rest);
    }
    path.to_path_buf()
}
