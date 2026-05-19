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

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

fn expand_path(path: &Path) -> PathBuf {
    let s = path.to_string_lossy();
    if let Some(rest) = s.strip_prefix("~/") {
        if let Some(home) = dirs_home() {
            return home.join(rest);
        }
    } else if s == "~" {
        if let Some(home) = dirs_home() {
            return home;
        }
    }
    path.to_path_buf()
}

fn dirs_home() -> Option<PathBuf> {
    std::env::var_os("HOME").map(PathBuf::from)
}

fn split_session_key(key: &str) -> Option<(String, String)> {
    let (a, b) = key.split_once(':')?;
    if a.is_empty() || b.is_empty() {
        return None;
    }
    Some((a.to_string(), b.to_string()))
}

// ---------------------------------------------------------------------------
// Provider construction (mirrors Python `_make_provider`).
// ---------------------------------------------------------------------------

/// Resolve a [`ProviderSpec`] for the default model, honouring the
/// `"auto"` sentinel the same way Python does.
fn resolve_spec(cfg: &Config) -> Option<&'static ProviderSpec> {
    let defaults = &cfg.agents.defaults;
    let configured = defaults.provider.trim();
    if !configured.is_empty() && configured != "auto" {
        if let Some(spec) = find_by_name(configured) {
            return Some(spec);
        }
    }
    find_by_model(&defaults.model)
}

/// Pull the [`ProviderConfig`] entry matching *spec* from the root config.
fn provider_config_for<'a>(
    cfg: &'a Config,
    spec: Option<&ProviderSpec>,
) -> Option<&'a ProviderConfig> {
    let spec = spec?;
    let p = &cfg.providers;
    Some(match spec.name {
        "custom" => &p.custom,
        "azure_openai" => &p.azure_openai,
        "anthropic" => &p.anthropic,
        "openai" => &p.openai,
        "openrouter" => &p.openrouter,
        "deepseek" => &p.deepseek,
        "groq" => &p.groq,
        "zhipu" => &p.zhipu,
        "dashscope" => &p.dashscope,
        "vllm" => &p.vllm,
        "ollama" => &p.ollama,
        "lm_studio" => &p.lm_studio,
        "ovms" => &p.ovms,
        "gemini" => &p.gemini,
        "moonshot" => &p.moonshot,
        "minimax" => &p.minimax,
        "minimax_anthropic" => &p.minimax_anthropic,
        "mistral" => &p.mistral,
        "stepfun" => &p.stepfun,
        "xiaomi_mimo" => &p.xiaomi_mimo,
        "aihubmix" => &p.aihubmix,
        "siliconflow" => &p.siliconflow,
        "volcengine" => &p.volcengine,
        "volcengine_coding_plan" => &p.volcengine_coding_plan,
        "byteplus" => &p.byteplus,
        "byteplus_coding_plan" => &p.byteplus_coding_plan,
        "openai_codex" => &p.openai_codex,
        "github_copilot" => &p.github_copilot,
        "qianfan" => &p.qianfan,
        _ => return None,
    })
}

fn resolve_api_base(spec: Option<&ProviderSpec>, p: Option<&ProviderConfig>) -> Option<String> {
    if let Some(pc) = p {
        if let Some(b) = &pc.api_base {
            if !b.is_empty() {
                return Some(b.clone());
            }
        }
    }
    if let Some(s) = spec {
        if !s.default_api_base.is_empty() {
            return Some(s.default_api_base.to_string());
        }
    }
    None
}

fn make_provider(cfg: &Config) -> Result<Arc<dyn LLMProvider>, NanobotError> {
    let model = cfg.agents.defaults.model.clone();
    let spec = resolve_spec(cfg);
    let pconf = provider_config_for(cfg, spec);
    let backend = spec.map(|s| s.backend).unwrap_or(Backend::OpenAICompat);

    // ---- credential validation (mirror Python checks) ----
    match backend {
        Backend::AzureOpenAI => {
            let ok = pconf
                .map(|p| {
                    !p.api_key.as_deref().unwrap_or("").is_empty()
                        && !p.api_base.as_deref().unwrap_or("").is_empty()
                })
                .unwrap_or(false);
            if !ok {
                return Err(NanobotError::Provider(
                    "Azure OpenAI requires api_key and api_base in config.".into(),
                ));
            }
        }
        Backend::OpenAICompat if !model.starts_with("bedrock/") => {
            let has_key = pconf
                .and_then(|p| p.api_key.as_deref())
                .map(|k| !k.is_empty())
                .unwrap_or(false);
            let exempt = spec
                .map(|s| s.is_oauth || s.is_local || s.is_direct)
                .unwrap_or(false);
            if !has_key && !exempt {
                let name = spec.map(|s| s.name).unwrap_or("");
                return Err(NanobotError::Provider(format!(
                    "No API key configured for provider '{name}'."
                )));
            }
        }
        _ => {}
    }

    let generation = GenerationSettings::from_config(cfg);

    // ---- build backend ----
    let provider: Arc<dyn LLMProvider> = match backend {
        Backend::OpenAICodex => {
            let mut cx_cfg = OpenAICodexConfig::default();
            cx_cfg.default_model = model;
            let p = OpenAICodexProvider::new(cx_cfg)
                .map_err(|e| NanobotError::Provider(format!("openai_codex: {e}")))?;
            Arc::new(p)
        }
        Backend::GitHubCopilot => {
            let p = GitHubCopilotProvider::new(model)
                .map_err(|e| NanobotError::Provider(format!("github_copilot: {e}")))?;
            Arc::new(p)
        }
        Backend::AzureOpenAI => {
            let pc = pconf.ok_or_else(|| {
                NanobotError::Provider("Azure OpenAI provider config missing".into())
            })?;
            let endpoint = pc.api_base.clone().unwrap_or_default();
            let api_key = pc.api_key.clone().unwrap_or_default();
            let cfg_az = AzureOpenAIConfig::new(endpoint, api_key, model.clone());
            let p = AzureOpenAIProvider::new(cfg_az)
                .map_err(|e| NanobotError::Provider(format!("azure_openai: {e}")))?;
            Arc::new(p.with_generation(generation))
        }
        Backend::Anthropic => {
            let mut ant = AnthropicConfig::new(model.clone());
            if let Some(pc) = pconf {
                if let Some(k) = &pc.api_key {
                    ant = ant.with_api_key(k.clone());
                }
                if let Some(headers) = &pc.extra_headers {
                    for (k, v) in headers {
                        ant = ant.with_extra_header(k, v);
                    }
                }
            }
            if let Some(base) = resolve_api_base(spec, pconf) {
                ant = ant.with_api_base(base);
            }
            Arc::new(AnthropicProvider::new(ant).with_generation(generation))
        }
        Backend::OpenAICompat => {
            let mut oc = OpenAICompatConfig::new(model.clone());
            if let Some(pc) = pconf {
                if let Some(k) = &pc.api_key {
                    oc = oc.with_api_key(k.clone());
                }
                if let Some(headers) = &pc.extra_headers {
                    for (k, v) in headers {
                        oc = oc.with_extra_header(k, v);
                    }
                }
            }
            if let Some(s) = spec {
                oc = oc.with_spec(s);
            }
            if let Some(base) = resolve_api_base(spec, pconf) {
                oc = oc.with_api_base(base);
            }
            Arc::new(OpenAICompatProvider::new(oc).with_generation(generation))
        }
    };
    Ok(provider)
}
