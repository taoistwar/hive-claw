//! Shared CLI runtime helpers.
//!
//! This module mirrors the Python `_load_runtime_config` / `_make_provider`
//! / `_migrate_cron_store` helpers from `nanobot/cli/commands.py`, plus a
//! small `LoopConfig::from_config` helper used by every command that spins up an
//! [`AgentLoop`].

use std::path::{Path, PathBuf};
use std::sync::Arc;

use agent::tools::{BuiltinToolSet, ToolFactoryConfig, ToolFactoryDeps};
use agent::{AgentLoop, LoopConfig};
use bus::MessageBus;
use config::schema::{Config, ProviderConfig};
use config::{get_config_path, paths::is_default_workspace, set_config_path};
use providers::anthropic::{AnthropicConfig, AnthropicProvider};
use providers::azure_openai::{AzureOpenAIConfig, AzureOpenAIProvider};
use providers::openai_codex::{OpenAICodexConfig, OpenAICodexProvider};
use providers::openai_compat::{OpenAICompatConfig, OpenAICompatProvider};
use providers::registry::{Backend, ProviderSpec, find_by_model, find_by_name};
use providers::{GenerationSettings, GitHubCopilotProvider, LLMProvider};
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

/// Resolve a [`ProviderSpec`] for the default model — honours the
/// `"auto"` sentinel just like Python.
pub fn resolve_spec(cfg: &Config) -> Option<&'static ProviderSpec> {
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
pub fn provider_config_for<'a>(
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

fn resolve_api_base(
    provider_spec: Option<&ProviderSpec>,
    provider_config: Option<&ProviderConfig>,
) -> Option<String> {
    if let Some(pc) = provider_config {
        if let Some(b) = &pc.api_base {
            if !b.is_empty() {
                return Some(b.clone());
            }
        }
    }
    if let Some(s) = provider_spec {
        if !s.default_api_base.is_empty() {
            return Some(s.default_api_base.to_string());
        }
    }
    None
}

/// Build the right [`LLMProvider`] for the active config (Python
/// `_make_provider`).
pub fn make_provider(cfg: &Config) -> Result<Arc<dyn LLMProvider>, String> {
    let model = cfg.agents.defaults.model.clone();
    let provider_spec = resolve_spec(cfg);
    let provider_config = provider_config_for(cfg, provider_spec);
    let backend = provider_spec
        .map(|s| s.backend)
        .unwrap_or(Backend::OpenAICompat);

    // ---- credential validation (mirror Python checks) ----
    match backend {
        Backend::AzureOpenAI => {
            let ok = provider_config
                .map(|p| {
                    !p.api_key.as_deref().unwrap_or("").is_empty()
                        && !p.api_base.as_deref().unwrap_or("").is_empty()
                })
                .unwrap_or(false);
            if !ok {
                return Err("Azure OpenAI requires api_key and api_base. \
                     Set them in ~/.nanobot/config.json under providers.azureOpenai \
                     and use `model` for the deployment name."
                    .into());
            }
        }
        Backend::OpenAICompat if !model.starts_with("bedrock/") => {
            let has_key = provider_config
                .and_then(|p| p.api_key.as_deref())
                .map(|k| !k.is_empty())
                .unwrap_or(false);
            let exempt = provider_spec
                .map(|s| s.is_oauth || s.is_local || s.is_direct)
                .unwrap_or(false);
            if !has_key && !exempt {
                let name = provider_spec.map(|s| s.name).unwrap_or("");
                return Err(format!(
                    "No API key configured for provider '{name}'. Set one in \
                     ~/.nanobot/config.json under the providers section."
                ));
            }
        }
        _ => {}
    }

    let generation = GenerationSettings::from_config(cfg);

    let provider: Arc<dyn LLMProvider> = match backend {
        Backend::OpenAICodex => {
            let mut cx_cfg = OpenAICodexConfig::default();
            cx_cfg.default_model = model;
            let p = OpenAICodexProvider::new(cx_cfg).map_err(|e| format!("openai_codex: {e}"))?;
            Arc::new(p)
        }
        Backend::GitHubCopilot => {
            let p =
                GitHubCopilotProvider::new(model).map_err(|e| format!("github_copilot: {e}"))?;
            Arc::new(p)
        }
        Backend::AzureOpenAI => {
            let pc = provider_config
                .ok_or_else(|| "Azure OpenAI provider config missing".to_string())?;
            let endpoint = pc.api_base.clone().unwrap_or_default();
            let api_key = pc.api_key.clone().unwrap_or_default();
            let cfg_az = AzureOpenAIConfig::new(endpoint, api_key, model.clone());
            let p = AzureOpenAIProvider::new(cfg_az).map_err(|e| format!("azure_openai: {e}"))?;
            Arc::new(p.with_generation(generation))
        }
        Backend::Anthropic => {
            let mut ant = AnthropicConfig::new(model.clone());
            if let Some(pc) = provider_config {
                if let Some(k) = &pc.api_key {
                    ant = ant.with_api_key(k.clone());
                }
                if let Some(headers) = &pc.extra_headers {
                    for (k, v) in headers {
                        ant = ant.with_extra_header(k, v);
                    }
                }
            }
            if let Some(base) = resolve_api_base(provider_spec, provider_config) {
                ant = ant.with_api_base(base);
            }
            Arc::new(AnthropicProvider::new(ant).with_generation(generation))
        }
        Backend::OpenAICompat => {
            let mut oc = OpenAICompatConfig::new(model.clone());
            if let Some(pc) = provider_config {
                if let Some(k) = &pc.api_key {
                    oc = oc.with_api_key(k.clone());
                }
                if let Some(headers) = &pc.extra_headers {
                    for (k, v) in headers {
                        oc = oc.with_extra_header(k, v);
                    }
                }
            }
            if let Some(s) = provider_spec {
                oc = oc.with_spec(s);
            }
            if let Some(base) = resolve_api_base(provider_spec, provider_config) {
                oc = oc.with_api_base(base);
            }
            Arc::new(OpenAICompatProvider::new(oc).with_generation(generation))
        }
    };
    Ok(provider)
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
        let provider = make_provider(cfg)?;
        let bus = Arc::new(MessageBus::new());
        let tf = ToolFactoryConfig::from_config(cfg);
        let deps = ToolFactoryDeps::new_with_cron(cron);
        let builtin: BuiltinToolSet = BuiltinToolSet::default_tools(tf, bus.clone(), deps).await;

        let lc = LoopConfig::from_config(cfg);
        let sessions = Arc::new(Mutex::new(SessionManager::new(cfg.workspace_path())));

        let message_handle = builtin.message.clone();
        let cron_handle = builtin.cron.clone();

        let agent = AgentLoop::from_builtin(bus.clone(), provider, sessions, builtin, lc);
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
    if s == "~" {
        if let Some(home) = dirs::home_dir() {
            return home;
        }
    }
    if let Some(rest) = s.strip_prefix("~/") {
        if let Some(home) = dirs::home_dir() {
            return home.join(rest);
        }
    }
    path.to_path_buf()
}
