//! Default [`ToolRegistry`] factory — assembles the built-in tool set
//! (filesystem, search, shell, web, notebook, message, cron, spawn)
//! against a single [`ToolFactoryConfig`].
//!
//! Keeping this in a dedicated module means the [`AgentLoop`](crate::loop_::AgentLoop)
//! can hand callers (CLI / tests / new channels) a fully-wired registry
//! with one call, and still lets integrators drop specific tools or
//! swap backends (e.g. a real [`WebSearchBackend`]) in piecewise.

use std::path::PathBuf;
use std::sync::Arc;

use bus::MessageBus;
use config::Config;

use super::cron::CronTool;
use super::filesystem::{EditFileTool, FsTool, ListDirTool, ReadFileTool, WriteFileTool};
use super::message::MessageTool;
use super::notebook::NotebookEditTool;
use super::registry::ToolRegistry;
use super::search::GrepTool;
use super::shell::ExecTool;
use super::spawn::{SpawnCallback, SpawnTool};
use super::web::{WebFetchTool, WebSearchBackend, WebSearchTool};
use super::web_search_ddg::DuckDuckGoBackend;

/// Configuration for [`BuiltinToolSet::default_tools`].
#[derive(Clone)]
pub struct ToolFactoryConfig {
    /// Root workspace for the agent. Used for path restriction and cwd.
    pub workspace: PathBuf,
    /// Extra directories outside *workspace* the filesystem tools may touch.
    pub extra_allowed_dirs: Vec<PathBuf>,
    /// If `true`, filesystem + shell tools must stay inside *workspace*.
    pub restrict_to_workspace: bool,

    /// `exec` tool defaults.
    pub exec_timeout_secs: u64,
    /// Optional sandbox backend name (e.g. `"bwrap"`). Empty = no sandbox.
    pub exec_sandbox: String,
    pub exec_path_append: String,
    pub exec_allowed_env_keys: Vec<String>,

    /// `web_fetch` content cap (0 = default 50 000).
    pub web_max_chars: usize,
    pub web_proxy: Option<String>,

    /// Default timezone for cron expressions / `at` datetimes.
    pub default_timezone: String,
}

impl ToolFactoryConfig {
    pub fn new(workspace: PathBuf) -> Self {
        Self {
            workspace,
            extra_allowed_dirs: Vec::new(),
            restrict_to_workspace: false,
            exec_timeout_secs: 60,
            exec_sandbox: String::new(),
            exec_path_append: String::new(),
            exec_allowed_env_keys: Vec::new(),
            web_max_chars: 0,
            web_proxy: None,
            default_timezone: "UTC".into(),
        }
    }

    pub fn from_config(cfg: &Config) -> Self {
        let mut tf = Self::new(cfg.workspace_path());
        tf.restrict_to_workspace = cfg.tools.restrict_to_workspace;
        tf.exec_timeout_secs = cfg.tools.exec.timeout as u64;
        tf.exec_sandbox = cfg.tools.exec.sandbox.clone();
        tf.exec_path_append = cfg.tools.exec.path_append.clone();
        tf.exec_allowed_env_keys = cfg.tools.exec.allowed_env_keys.clone();
        tf.web_proxy = cfg.tools.web.proxy.clone();
        tf.default_timezone = cfg.agents.defaults.timezone.clone();
        tf
    }
}

/// Bundle of the registry and the context-bearing tools.
///
/// The tools that hold per-turn routing context (message / spawn / cron)
/// are returned as `Arc` so the [`AgentLoop`](crate::loop_::AgentLoop)
/// can call their `set_context` method before each turn without
/// re-registering.
pub struct BuiltinToolSet {
    pub registry: ToolRegistry,
    pub message: Arc<MessageTool>,
    pub spawn: Option<Arc<SpawnTool>>,
    pub cron: Option<Arc<CronTool>>,
}

impl BuiltinToolSet {
    /// Build the default [`ToolRegistry`] populated with the built-in tools.
    pub async fn default_tools(
        config: ToolFactoryConfig,
        bus: Arc<MessageBus>,
        deps: ToolFactoryDeps,
    ) -> BuiltinToolSet {
        let registry = ToolRegistry::new();
        let allowed_dir = if config.restrict_to_workspace {
            Some(config.workspace.clone())
        } else {
            None
        };
        let fs = FsTool::new(
            Some(config.workspace.clone()),
            allowed_dir.clone(),
            config.extra_allowed_dirs.clone(),
        );

        // Filesystem
        registry.register(Arc::new(ReadFileTool(fs.clone()))).await;
        registry.register(Arc::new(WriteFileTool(fs.clone()))).await;
        registry.register(Arc::new(EditFileTool(fs.clone()))).await;
        registry.register(Arc::new(ListDirTool(fs.clone()))).await;

        // Search
        registry.register(Arc::new(GrepTool(fs.clone()))).await;

        // Shell / exec
        let exec = ExecTool::new()
            .with_working_dir(config.workspace.clone())
            .with_timeout_secs(config.exec_timeout_secs)
            .with_sandbox(config.exec_sandbox.clone())
            .with_path_append(config.exec_path_append.clone())
            .with_allowed_env_keys(config.exec_allowed_env_keys.clone())
            .with_restrict_to_workspace(config.restrict_to_workspace);
        registry.register(Arc::new(exec)).await;

        // Web
        registry
            .register(Arc::new(WebFetchTool::new(
                config.web_max_chars,
                config.web_proxy.clone(),
            )))
            .await;

        // Web Search
        // Default to DuckDuckGo when no backend is explicitly wired —
        // matches Python's behaviour (DDG is the fallback for every provider).
        let search_backend = deps
            .web_search
            .clone()
            .unwrap_or_else(|| Arc::new(DuckDuckGoBackend::new()));
        registry
            .register(Arc::new(WebSearchTool::new(search_backend)))
            .await;

        // Notebook
        registry
            .register(Arc::new(NotebookEditTool(fs.clone())))
            .await;

        // Message — always wired because it's the only way to deliver files.
        let message = Arc::new(MessageTool::new(
            bus,
            config.workspace.clone(),
            config.restrict_to_workspace,
        ));
        registry.register(message.clone()).await;

        // Spawn (requires a subagent factory callback)
        let spawn = if let Some(cb) = deps.spawn_callback {
            let tool = Arc::new(SpawnTool::new(cb));
            registry.register(tool.clone()).await;
            Some(tool)
        } else {
            None
        };

        // Cron (requires a CronService)
        let cron = if let Some(svc) = deps.cron_service {
            let tool = Arc::new(CronTool::new(svc, config.default_timezone.clone()));
            registry.register(tool.clone()).await;
            Some(tool)
        } else {
            None
        };

        BuiltinToolSet {
            registry,
            message,
            spawn,
            cron,
        }
    }
}

/// Optional plug-ins that the factory cannot construct itself.
#[derive(Default)]
pub struct ToolFactoryDeps {
    /// Concrete search backend (DDG / Jina / Kagi / ...). When `None`, a
    /// no-op backend is registered and calls return an informative error.
    pub web_search: Option<Arc<dyn WebSearchBackend>>,
    /// Spawn callback — wire in a live [`SubagentManager`](crate::subagent::SubagentManager)
    /// when subagents should be available.
    pub spawn_callback: Option<SpawnCallback>,
    /// Cron service. When `None`, the `cron` tool is omitted.
    pub cron_service: Option<Arc<::cron::service::CronService>>,
}

impl ToolFactoryDeps {
    pub fn new_with_cron(cron: Option<Arc<::cron::CronService>>) -> Self {
        Self {
            web_search: None,
            spawn_callback: None,
            cron_service: cron,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn registers_filesystem_and_search_tools() {
        let bus = Arc::new(MessageBus::new());
        let cfg = ToolFactoryConfig::new(std::env::temp_dir());
        let tools = BuiltinToolSet::default_tools(cfg, bus, ToolFactoryDeps::default()).await;
        let names = tools.registry.tool_names().await;
        for expected in [
            "read_file",
            "write_file",
            "edit_file",
            "list_dir",
            "grep",
            "exec",
            "web_fetch",
            "web_search",
            "notebook_edit",
            "message",
        ] {
            assert!(names.contains(&expected.to_string()), "missing {expected}");
        }
        // Without spawn_callback / cron_service, these are absent.
        assert!(!names.contains(&"spawn".to_string()));
        assert!(!names.contains(&"cron".to_string()));
    }
}
