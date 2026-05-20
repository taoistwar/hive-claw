//! High-level programmatic interface to nanobot.
//!
//! Port of `nanobot/nanobot/nanobot.py`. Exposes [`Nanobot`], a thin
//! facade that loads a config file, wires up a provider, tool registry,
//! message bus, and session manager, and drives a single [`AgentLoop`].
//!
//! Usage:
//!
//! ```ignore
//! use nanobot_main::Nanobot;
//!
//! # async fn demo() -> Result<(), Box<dyn std::error::Error>> {
//! let bot = Nanobot::from_config(None, None).await?;
//! let result = bot.run("Summarize this repo", "sdk:default").await?;
//! println!("{}", result.content);
//! # Ok(())
//! # }
//! ```

use std::path::{Path, PathBuf};
use std::sync::Arc;

use chrono::Local;
use serde_json::Value;
use thiserror::Error;
use tokio::sync::Mutex;

use agent::tools::{BuiltinToolSet, ToolFactoryConfig, ToolFactoryDeps};
use agent::{AgentLoop, LoopConfig};
use bus::{InboundMessage, MessageBus};
use config::ConfigError;
use config::schema::{Config, ProviderConfig, ProviderRetryMode};
use providers::anthropic::{AnthropicConfig, AnthropicProvider};
use providers::azure_openai::{AzureOpenAIConfig, AzureOpenAIProvider};
use providers::openai_codex::{OpenAICodexConfig, OpenAICodexProvider};
use providers::openai_compat::{OpenAICompatConfig, OpenAICompatProvider};
use providers::registry::{Backend, ProviderSpec, find_by_model, find_by_name};
use providers::{GenerationSettings, GitHubCopilotProvider, LLMProvider, RetryMode};
use session::SessionManager;

/// Result of a single agent run (mirrors Python `RunResult`).
#[derive(Debug, Clone, Default)]
pub struct RunResult {
    content: String,
    tools_used: Vec<String>,
    messages: Vec<Value>,
}

impl RunResult {
    pub fn new(content: String, tools_used: Vec<String>, messages: Vec<Value>) -> Self {
        RunResult {
            content,
            tools_used,
            messages,
        }
    }
    pub fn content(&self) -> &str {
        &self.content
    }
    pub fn tools_used(&self) -> &[String] {
        &self.tools_used
    }
    pub fn messages(&self) -> &[Value] {
        &self.messages
    }
}

/// Errors surfaced by the [`Nanobot`] facade.
#[derive(Debug, Error)]
pub enum NanobotError {
    #[error("config not found: {0}")]
    ConfigNotFound(PathBuf),

    #[error(transparent)]
    Config(#[from] ConfigError),

    #[error("provider setup failed: {0}")]
    Provider(String),

    #[error("agent run failed: {0}")]
    Run(String),
}

/// Programmatic facade for running the nanobot agent.
pub struct Nanobot {
    loop_: Arc<AgentLoop>,
    default_channel: String,
    default_chat_id: String,
}

impl Nanobot {
    /// Build a [`Nanobot`] from a config file.
    ///
    /// * `config_path` — path to `config.json`. Defaults to
    ///   `~/.nanobot/config.json` when `None`.
    /// * `workspace` — override the workspace directory from the config.
    pub async fn from_config(
        config_path: Option<&Path>,
        workspace: Option<&Path>,
    ) -> Result<Self, NanobotError> {
        let resolved_path: Option<PathBuf> = match config_path {
            Some(p) => {
                let expanded = expand_path(p);
                if !expanded.exists() {
                    return Err(NanobotError::ConfigNotFound(expanded));
                }
                Some(expanded)
            }
            None => None,
        };

        let mut cfg = Config::from_config(resolved_path.as_deref());
        cfg = cfg.resolve_env_vars()?;

        if let Some(ws) = workspace {
            cfg.agents.defaults.workspace = expand_path(ws).to_string_lossy().into_owned();
        }

        let provider = make_provider(&cfg)?;
        let workspace_path = cfg.workspace_path();

        let bus = Arc::new(MessageBus::new());
        let tf_config = ToolFactoryConfig::from_config(&cfg);
        let builtin =
            BuiltinToolSet::default_tools(tf_config, bus.clone(), ToolFactoryDeps::default()).await;

        let defaults = &cfg.agents.defaults;
        let mut loop_cfg = LoopConfig::new(workspace_path.clone());
        loop_cfg.model = Some(defaults.model.clone());
        loop_cfg.max_iterations = defaults.max_tool_iterations;
        loop_cfg.max_tool_result_chars = defaults.max_tool_result_chars as usize;
        loop_cfg.context_window_tokens = Some(defaults.context_window_tokens);
        loop_cfg.timezone = Some(defaults.timezone.clone());
        loop_cfg.disabled_skills = defaults.disabled_skills.clone();
        loop_cfg.provider_retry_mode = match defaults.provider_retry_mode {
            ProviderRetryMode::Standard => RetryMode::Standard,
            ProviderRetryMode::Persistent => RetryMode::Persistent,
        };

        let sessions = Arc::new(Mutex::new(SessionManager::new(workspace_path)));
        let loop_ = AgentLoop::from_builtin(bus, provider, sessions, builtin, loop_cfg);

        Ok(Self {
            loop_: Arc::new(loop_),
            default_channel: "sdk".into(),
            default_chat_id: "default".into(),
        })
    }

    /// Run the agent once and return the result.
    ///
    /// * `message` — user message to process.
    /// * `session_key` — session identifier. Different keys get
    ///   independent history. Use `"sdk:default"` for a single shared
    ///   conversation, matching the Python default.
    pub async fn run(&self, message: &str, session_key: &str) -> Result<RunResult, NanobotError> {
        let (channel, chat_id) = split_session_key(session_key)
            .unwrap_or_else(|| (self.default_channel.clone(), self.default_chat_id.clone()));

        let inbound = InboundMessage {
            channel,
            sender_id: "sdk".into(),
            chat_id,
            content: message.to_string(),
            timestamp: Local::now(),
            media: Vec::new(),
            metadata: Default::default(),
            session_key_override: Some(session_key.to_string()),
        };

        let result = self
            .loop_
            .process_inbound(inbound)
            .await
            .map_err(NanobotError::Run)?;

        Ok(RunResult {
            content: result.final_content.unwrap_or_default(),
            tools_used: result.tools_used,
            messages: result.messages,
        })
    }

    /// Borrow the inner [`AgentLoop`] for advanced use (custom hooks,
    /// command prefilters, etc.).
    pub fn loop_ref(&self) -> &AgentLoop {
        &self.loop_
    }
}
