//! `clap`-based subcommand dispatcher and all command implementations.
//!
//! The dispatcher and all command logic live here: onboard, gateway, agent_cmd,
//! status, channels, plugins, provider login/status, and serve.
//!
//! Heavy lifting (config loading, provider construction, AgentLoop wiring)
//! is handled through the Runtime and LoopBundle types defined in this module.

use std::io::{self, Read};
use std::net::SocketAddr;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::SystemTime;

use agent::tools::MessageTool;
use agent::{AgentLoop, BuiltinToolSet, Dream, DreamConfig, LoopConfig, MemoryDream, ToolFactoryConfig, ToolFactoryDeps};
use api::{ApiAgent, ApiAnswer, ApiRequest, ApiServerConfig};
use async_trait::async_trait;
use bus::{InboundMessage, MessageBus, OutboundMessage};
use channels::base::TranscriptionSettings;
use channels::{build_one, known_channel_names, ChannelManager};
use chrono::Local;
use clap::{Parser, Subcommand};
use config::paths::get_workspace_path;
use config::schema::{Config, ProviderConfig};
use config::{get_config_path, paths::is_default_workspace, set_config_path};
use cron::{CronJob, CronPayload, CronSchedule, JobHandler, PayloadKind, ScheduleKind};
use heartbeat::{HeartbeatConfig as HbCfg, HeartbeatDecider, HeartbeatExecutor, HeartbeatNotifier, HeartbeatService, LLMHeartbeatDecider};
use log::info;
use providers::anthropic_provider::{AnthropicConfig, AnthropicProvider};
use providers::azure_openai_provider::{AzureOpenAIConfig, AzureOpenAIProvider};
use providers::bedrock_provider::{BedrockConfig, BedrockProvider};
use providers::openai_codex_provider::{OpenAICodexConfig, OpenAICodexProvider};
use providers::openai_compat_provider::{OpenAICompatConfig, OpenAICompatProvider};
use providers::registry::{Backend, ProviderSpec, find_by_model, find_by_name};
use providers::{FileTokenStorage, GenerationSettings, GitHubCopilotProvider, LLMProvider, PROVIDERS};
use providers::{ProviderBuildConfig, build_provider, env_api_base, env_api_key, env_region};
use rustyline::DefaultEditor;
use rustyline::error::ReadlineError;
use serde_json::Value;
use session::SessionManager;
use tokio::net::TcpListener;
use tokio::signal;
use tokio::sync::Mutex;
use tokio::sync::oneshot;

use crate::onboard::{self, OnboardArgs};

// ===================================================================
// AgentLoopApi — adapter that makes AgentLoop implement ApiAgent
// ===================================================================

struct AgentLoopApi {
    loop_: Arc<AgentLoop>,
}

impl AgentLoopApi {
    fn new(loop_: Arc<AgentLoop>) -> Self {
        Self { loop_ }
    }
}

#[async_trait]
impl ApiAgent for AgentLoopApi {
    async fn generate(&self, req: ApiRequest) -> Result<ApiAnswer, String> {
        let key = req.session_key();
        let sender = format!("api:{}", req.channel);
        let msg = InboundMessage {
            channel: req.channel.clone(),
            sender_id: sender.clone(),
            chat_id: req.chat_id.clone(),
            content: req.content.clone(),
            timestamp: chrono::Local::now(),
            media: Vec::new(),
            metadata: Default::default(),
            session_key_override: Some(key),
        };
        match self.loop_.process_inbound(msg).await {
            Ok(result) => Ok(ApiAnswer { content: result.final_content.unwrap_or_default() }),
            Err(e) => Err(format!("agent loop error: {e}")),
        }
    }
}

const VERSION: &str = env!("CARGO_PKG_VERSION");

// ---------------------------------------------------------------------------
// CLI types and enums
// ---------------------------------------------------------------------------

/// Top-level `nanobot` CLI.
#[derive(Parser, Debug)]
#[command(
    name = "nanobot",
    version = VERSION,
    about = "Personal AI assistant (Rust port)",
    propagate_version = true,
)]
pub struct Cli {
    /// Workspace directory. Defaults to the value in the loaded config.
    #[arg(long, global = true)]
    pub workspace: Option<PathBuf>,

    /// Path to the config file (defaults to `~/.nanobot/config.json`).
    #[arg(long, global = true)]
    pub config: Option<PathBuf>,

    #[command(subcommand)]
    pub command: Command,
}

#[derive(Subcommand, Debug)]
pub enum Command {
    /// Print the CLI version and exit.
    Version,

    /// First-time setup: create config + workspace and print next steps.
    Onboard {
        /// Overwrite an existing config with defaults.
        #[arg(long)]
        overwrite: bool,
    },

    /// Show config / workspace / model / provider credential status.
    Status,

    /// Send one prompt or open an interactive REPL.
    Agent {
        /// Single-shot message. Use `-` to read from stdin.
        #[arg(short = 'm', long)]
        message: Option<String>,

        /// Override the session id (default `cli:default`).
        #[arg(short = 's', long)]
        session: Option<String>,

        /// (reserved) render replies as Markdown — falls back to plain text.
        #[arg(long, default_value_t = false)]
        markdown: bool,

        /// Forward agent INFO logs to stderr.
        #[arg(long, default_value_t = false)]
        logs: bool,
    },

    /// Backwards-compat alias for `agent --message …`.
    Run {
        #[arg(short, long)]
        message: Option<String>,
    },

    /// Backwards-compat alias for `agent` (REPL).
    Chat,

    /// Start the OpenAI-compatible HTTP API server.
    Serve {
        /// Bind socket address. Overrides config when set.
        #[arg(long)]
        bind: Option<SocketAddr>,

        /// Bind host. Overrides config when set.
        #[arg(long)]
        host: Option<String>,

        /// Bind port. Overrides config when set.
        #[arg(long)]
        port: Option<u16>,

        /// Per-request timeout (seconds). Overrides config when set.
        #[arg(long)]
        timeout: Option<f32>,

        /// Model name advertised by `/v1/models`.
        #[arg(long)]
        model_name: Option<String>,

        /// Enable INFO-level log forwarding.
        #[arg(long, default_value_t = false)]
        verbose: bool,
    },

    /// Long-running gateway: cron + heartbeat + bus loop + /health.
    Gateway {
        #[arg(long)]
        host: Option<String>,
        #[arg(long)]
        port: Option<u16>,
        /// Enable DEBUG-level log forwarding.
        #[arg(long, default_value_t = false)]
        verbose: bool,
    },

    /// Channel adapter management (status / login).
    Channels {
        #[command(subcommand)]
        sub: ChannelsCommand,
    },

    /// Channel plugin discovery (built-in + configured).
    Plugins {
        #[command(subcommand)]
        sub: PluginsCommand,
    },

    /// Interactive provider login flows.
    Provider {
        #[command(subcommand)]
        sub: ProviderCommand,
    },
}

#[derive(Subcommand, Debug)]
pub enum ProviderCommand {
    /// Trigger an interactive login.
    Login {
        /// `github-copilot` or `openai-codex`.
        name: String,
    },
    /// Print login status for the given provider.
    Status { name: String },
}

#[derive(Subcommand, Debug)]
pub enum ChannelsCommand {
    /// List configured channels and whether they're enabled.
    Status,
    /// Trigger an interactive channel login (e.g. WeChat QR scan).
    Login {
        /// Channel name (e.g. `weixin`, `qq`).
        name: String,
        /// Force re-login even if a saved session exists.
        #[arg(short = 'f', long)]
        force: bool,
    },
}

#[derive(Subcommand, Debug)]
pub enum PluginsCommand {
    /// List configured channel plugins.
    List,
}

// ---------------------------------------------------------------------------
// Runtime types
// ---------------------------------------------------------------------------

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

// ---------------------------------------------------------------------------
// LoopBundle
// ---------------------------------------------------------------------------

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

        let agent = AgentLoop::from_builtin(bus.clone(), provider, sessions, builtin, lc);
        Ok(Self {
            bus,
            agent,
            builtin_message: message_handle,
            builtin_cron: cron_handle,
        })
    }
}

// ---------------------------------------------------------------------------
// ProviderChoice
// ---------------------------------------------------------------------------

/// Which backend the CLI should instantiate.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ProviderChoice {
    OpenAI,
    Anthropic,
    Azure,
    Bedrock,
    GithubCopilot,
    Custom,
}

impl ProviderChoice {
    pub fn parse(raw: &str) -> Option<Self> {
        match raw.to_ascii_lowercase().as_str() {
            "openai" => Some(Self::OpenAI),
            "anthropic" | "claude" => Some(Self::Anthropic),
            "azure" | "azure_openai" | "azure-openai" => Some(Self::Azure),
            "bedrock" | "aws_bedrock" | "aws-bedrock" => Some(Self::Bedrock),
            "github_copilot" | "github-copilot" | "copilot" => Some(Self::GithubCopilot),
            "custom" | "openai_compat" | "openai-compat" => Some(Self::Custom),
            _ => None,
        }
    }
}

/// Build a provider from environment variables.
///
/// Selection precedence:
/// * explicit `choice`
/// * `PROVIDER` env var
/// * OpenAI (API_KEY=`OPENAI_API_KEY`)
///
/// The caller can override the default model with the `model` argument
/// (usually from `--model` on the CLI).
pub fn default_provider(
    choice: Option<ProviderChoice>,
    model: Option<String>,
) -> Result<Arc<dyn LLMProvider>, String> {
    let choice = choice
        .or_else(|| std::env::var("PROVIDER").ok().and_then(|s| ProviderChoice::parse(&s)))
        .unwrap_or(ProviderChoice::OpenAI);

    match choice {
        ProviderChoice::OpenAI => {
            let model = model
                .or_else(|| std::env::var("MODEL").ok())
                .unwrap_or_else(|| "gpt-4o-mini".into());
            let api_key = std::env::var("OPENAI_API_KEY")
                .or_else(|_| std::env::var("API_KEY"))
                .ok();
            let cfg = ProviderBuildConfig {
                model,
                api_key,
                api_base: std::env::var("OPENAI_API_BASE").ok(),
                extra_headers: None,
                extra_body: None,
                region: None,
                profile: None,
            };
            build_provider(Backend::OpenAICompat, cfg)
        }
        ProviderChoice::Custom => {
            let model = model
                .or_else(|| std::env::var("MODEL").ok())
                .unwrap_or_else(|| "gpt-4o-mini".into());
            let api_key = std::env::var("OPENAI_API_KEY")
                .or_else(|_| std::env::var("API_KEY"))
                .ok();
            let api_base = std::env::var("API_BASE")
                .or_else(|_| std::env::var("OPENAI_API_BASE"))
                .ok();
            let cfg = ProviderBuildConfig {
                model,
                api_key,
                api_base,
                extra_headers: None,
                extra_body: None,
                region: None,
                profile: None,
            };
            build_provider(Backend::OpenAICompat, cfg)
        }
        ProviderChoice::Anthropic => {
            let api_key = std::env::var("ANTHROPIC_API_KEY")
                .map_err(|_| "ANTHROPIC_API_KEY is not set".to_string())?;
            let model = model.unwrap_or_else(|| "claude-3-5-sonnet-latest".into());
            let cfg = ProviderBuildConfig {
                model,
                api_key: Some(api_key),
                api_base: None,
                extra_headers: None,
                extra_body: None,
                region: None,
                profile: None,
            };
            build_provider(Backend::Anthropic, cfg)
        }
        ProviderChoice::Azure => {
            let api_key = std::env::var("AZURE_OPENAI_API_KEY")
                .map_err(|_| "AZURE_OPENAI_API_KEY is not set".to_string())?;
            let endpoint = std::env::var("AZURE_OPENAI_ENDPOINT")
                .map_err(|_| "AZURE_OPENAI_ENDPOINT is not set".to_string())?;
            let deployment = std::env::var("AZURE_OPENAI_DEPLOYMENT")
                .or_else(|_| std::env::var("AZURE_OPENAI_MODEL"))
                .map_err(|_| "AZURE_OPENAI_DEPLOYMENT is not set".to_string())?;
            let cfg = ProviderBuildConfig {
                model: deployment,
                api_key: Some(api_key),
                api_base: Some(endpoint),
                extra_headers: None,
                extra_body: None,
                region: None,
                profile: None,
            };
            build_provider(Backend::AzureOpenAI, cfg)
        }
        ProviderChoice::Bedrock => {
            let model = model.unwrap_or_else(|| {
                "bedrock/global.anthropic.claude-opus-4-7".into()
            });
            let cfg = ProviderBuildConfig {
                model,
                api_key: env_api_key("AWS_ACCESS_KEY_ID"),
                api_base: env_api_base("BEDROCK_API_BASE"),
                extra_headers: None,
                extra_body: None,
                region: env_region(),
                profile: None,
            };
            build_provider(Backend::Bedrock, cfg)
        }
        ProviderChoice::GithubCopilot => {
            let model = model.unwrap_or_else(|| "github-copilot/gpt-4.1".into());
            let cfg = ProviderBuildConfig {
                model,
                api_key: None,
                api_base: None,
                extra_headers: None,
                extra_body: None,
                region: None,
                profile: None,
            };
            build_provider(Backend::GitHubCopilot, cfg)
        }
    }
}

// ---------------------------------------------------------------------------
// Command argument types
// ---------------------------------------------------------------------------

/// Args for `nanobot agent`.
#[derive(Debug, Default, Clone)]
pub struct AgentArgs {
    /// Single-shot message. When `None`, drops into a REPL.
    pub message: Option<String>,
    /// Override the session ID. Defaults to `cli:default`.
    pub session: Option<String>,
    /// Override the workspace from the config.
    pub workspace: Option<PathBuf>,
    /// Override the config path.
    pub config: Option<PathBuf>,
    /// Reserved — no Markdown rendering in the Rust REPL yet.
    pub markdown: bool,
    /// Enable `INFO`-level log forwarding to stderr.
    pub logs: bool,
}

/// Args for `nanobot gateway`.
#[derive(Debug, Default, Clone)]
pub struct GatewayArgs {
    pub workspace: Option<PathBuf>,
    pub config: Option<PathBuf>,
    pub host: Option<String>,
    pub port: Option<u16>,
    pub verbose: bool,
}

// ---------------------------------------------------------------------------
// Dispatch
// ---------------------------------------------------------------------------

/// Parse argv + dispatch to the matching handler.
pub async fn dispatch() -> Result<(), String> {
    let cli = Cli::parse();
    let cfg_path = cli.config.clone();
    let ws_path = cli.workspace.clone();

    match cli.command {
        Command::Version => {
            println!("nanobot {VERSION}");
            Ok(())
        }
        Command::Onboard { overwrite } => {
            onboard::run(OnboardArgs {
                workspace: ws_path,
                config: cfg_path,
                overwrite,
            })
            .await
        }
        Command::Status => status_cmd(ws_path, cfg_path).await,
        Command::Agent {
            message,
            session,
            markdown,
            logs,
        } => {
            agent_run(AgentArgs {
                message,
                session,
                workspace: ws_path,
                config: cfg_path,
                markdown,
                logs,
            })
            .await
        }
        Command::Run { message } => {
            agent_run(AgentArgs {
                message: Some(message.unwrap_or_else(|| "-".into())),
                workspace: ws_path,
                config: cfg_path,
                ..Default::default()
            })
            .await
        }
        Command::Chat => {
            agent_run(AgentArgs {
                workspace: ws_path,
                config: cfg_path,
                ..Default::default()
            })
            .await
        }
        Command::Serve {
            bind,
            host,
            port,
            timeout,
            model_name,
            verbose,
        } => {
            serve(
                ws_path, cfg_path, bind, host, port, timeout, model_name, verbose,
            )
            .await
        }
        Command::Gateway { host, port, verbose } => {
            gateway_run(GatewayArgs {
                workspace: ws_path,
                config: cfg_path,
                host,
                port,
                verbose,
            })
            .await
        }
        Command::Channels { sub } => match sub {
            ChannelsCommand::Status => channels_status_cmd(cfg_path).await,
            ChannelsCommand::Login { name, force } => {
                channels_login_cmd(name, force, cfg_path).await
            }
        },
        Command::Plugins { sub } => match sub {
            PluginsCommand::List => plugins_list_cmd(cfg_path).await,
        },
        Command::Provider { sub } => match sub {
            ProviderCommand::Login { name } => provider_login(&name).await,
            ProviderCommand::Status { name } => provider_status(&name),
        },
    }
}

// ---------------------------------------------------------------------------
// serve
// ---------------------------------------------------------------------------

#[allow(clippy::too_many_arguments)]
async fn serve(
    workspace: Option<PathBuf>,
    config: Option<PathBuf>,
    bind: Option<SocketAddr>,
    host: Option<String>,
    port: Option<u16>,
    timeout: Option<f32>,
    model_name: Option<String>,
    verbose: bool,
) -> Result<(), String> {
    if verbose {
        let _ = env_logger::Builder::from_default_env()
            .filter_level(log::LevelFilter::Info)
            .try_init();
    }

    let Runtime { config: cfg, .. } =
        Runtime::from_config(config.as_deref(), workspace.as_deref())?;

    let bind: SocketAddr = if let Some(b) = bind {
        b
    } else {
        let h = host.unwrap_or_else(|| cfg.api.host.clone());
        let p = port.unwrap_or(cfg.api.port);
        format!("{h}:{p}")
            .parse()
            .map_err(|e: std::net::AddrParseError| format!("invalid bind: {e}"))?
    };
    if bind.ip().to_string() == "0.0.0.0" {
        eprintln!("warning: serving on 0.0.0.0 exposes the API to the local network.");
    }

    let bundle = LoopBundle::build_agent_loop(&cfg, None).await?;
    let adapter: Arc<dyn ApiAgent> = Arc::new(AgentLoopApi::new(Arc::new(bundle.agent)));

    let secs = timeout.unwrap_or(cfg.api.timeout).max(1.0);
    let server_cfg = ApiServerConfig::new(bind, config::get_media_dir(None))
        .with_model_name(model_name.unwrap_or_else(|| cfg.agents.defaults.model.clone()))
        .with_request_timeout(std::time::Duration::from_secs_f32(secs));
    info!("serving OpenAI-compatible API on http://{bind}");

    api::serve(adapter, server_cfg)
        .await
        .map_err(|e| e.to_string())
}

// ---------------------------------------------------------------------------
// gateway
// ---------------------------------------------------------------------------

async fn gateway_run(args: GatewayArgs) -> Result<(), String> {
    let log_level = if args.verbose {
        log::LevelFilter::Debug
    } else {
        log::LevelFilter::Info
    };
    let _ = env_logger::Builder::from_default_env()
        .filter_level(log_level)
        .try_init();

    let Runtime { config: cfg, .. } =
        Runtime::from_config(args.config.as_deref(), args.workspace.as_deref())?;

    if config::paths::is_default_workspace(Some(cfg.workspace_path())) {
        migrate_cron_store(&cfg);
    }

    let host = args.host.unwrap_or_else(|| cfg.gateway.host.clone());
    let port = args.port.unwrap_or(cfg.gateway.port);
    let bind = format!("{host}:{port}");

    if host == "0.0.0.0" {
        eprintln!(
            "warning: binding 0.0.0.0 exposes the gateway to the local network. \
             Prefer 127.0.0.1 unless you know what you're doing."
        );
    }

    let cron_path = cfg.workspace_path().join("cron").join("jobs.json");
    let cron_svc = Arc::new(::cron::CronService::new(cron_path, None));

    let bundle = LoopBundle::build_agent_loop(&cfg, Some(cron_svc.clone())).await?;
    let agent = Arc::new(bundle.agent);
    let bus = bundle.bus.clone();

    let dream_provider = providers::make_provider(&cfg)?;
    let dream_model = cfg.agents.defaults.model.clone();

    let dream_cfg = &cfg.agents.defaults.dream;
    let dream_config = DreamConfig {
        max_batch_size: dream_cfg.max_batch_size as usize,
        max_iterations: dream_cfg.max_iterations,
        max_tool_result_chars: cfg.agents.defaults.max_tool_result_chars as usize,
        annotate_line_ages: dream_cfg.annotate_line_ages,
    };

    let dream = Arc::new(MemoryDream::new(
        cfg.workspace_path(),
        dream_provider,
        dream_model,
        dream_config,
    ));
    if let Err(e) = dream.initialize().await {
        eprintln!("warning: dream initialization failed: {e}");
    }

    let store_path = cfg.workspace_path().join("cron").join("jobs.json");
    let cron_handler: Arc<dyn JobHandler> = Arc::new(CronAgentHandler {
        agent: agent.clone(),
        dream: dream.clone(),
    });
    let cron_svc = Arc::new(::cron::CronService::new(store_path, Some(cron_handler)));
    cron_svc.start().await;

    register_dream_job(&cron_svc, &cfg).await;

    let channel_mgr: Option<Arc<ChannelManager>> =
        match ChannelManager::new(Arc::new(cfg.clone()), (*bus).clone()) {
            Ok(mgr) => {
                mgr.clone().start_all().await;
                let names = mgr.enabled_channels().await;
                if !names.is_empty() {
                    println!("channels enabled: {}", names.join(", "));
                }
                Some(mgr)
            }
            Err(e) => {
                eprintln!("warning: channel manager init failed: {e}");
                None
            }
        };

    let hb_cfg = HbCfg::new(cfg.workspace_path(), cfg.agents.defaults.model.clone())
        .with_interval_s(cfg.gateway.heartbeat.interval_s as u64)
        .with_enabled(cfg.gateway.heartbeat.enabled)
        .with_timezone(Some(cfg.agents.defaults.timezone.clone()));

    let heartbeat_provider = providers::make_provider(&cfg)?;
    let decider: Arc<dyn HeartbeatDecider> = Arc::new(LLMHeartbeatDecider::new(
        heartbeat_provider,
        cfg.agents.defaults.model.clone(),
    ));

    let executor: Arc<dyn HeartbeatExecutor> = if let Some(ref mgr) = channel_mgr {
        Arc::new(GatewayAgentExecutor {
            agent: agent.clone(),
            channel_mgr: mgr.clone(),
            keep_recent_messages: cfg.gateway.heartbeat.keep_recent_messages as usize,
        })
    } else {
        Arc::new(GatewayAgentExecutor {
            agent: agent.clone(),
            channel_mgr: ChannelManager::new(Arc::new(cfg.clone()), (*bus).clone())
                .map_err(|e| format!("channel manager: {e}"))?,
            keep_recent_messages: cfg.gateway.heartbeat.keep_recent_messages as usize,
        })
    };

    let notifier: Option<Arc<dyn HeartbeatNotifier>> = if let Some(ref mgr) = channel_mgr {
        Some(Arc::new(HeartbeatChannelNotifier {
            bus: bus.clone(),
            channel_mgr: mgr.clone(),
        }))
    } else {
        None
    };

    let hb = Arc::new(HeartbeatService::new(
        hb_cfg,
        decider,
        Some(executor),
        notifier,
        None,
    ));
    hb.clone().start().await;

    let listener = TcpListener::bind(&bind)
        .await
        .map_err(|e| format!("bind {bind}: {e}"))?;
    let started_at = SystemTime::now();
    let model = cfg.agents.defaults.model.clone();
    let (shutdown_tx, mut shutdown_rx) = oneshot::channel::<()>();
    let server = tokio::spawn(async move {
        loop {
            tokio::select! {
                _ = &mut shutdown_rx => break,
                accept = listener.accept() => {
                    let (mut stream, _) = match accept {
                        Ok(v) => v,
                        Err(e) => {
                            log::warn!("gateway accept error: {e}");
                            continue;
                        }
                    };
                    let model = model.clone();
                    let started_at = started_at;
                    tokio::spawn(async move {
                        let _ = handle_health_request(&mut stream, &model, started_at).await;
                    });
                }
            }
        }
    });

    let agent_for_loop = agent.clone();
    let agent_task = tokio::spawn(async move { agent_for_loop.run().await });

    println!("nanobot gateway listening on http://{bind} (health endpoint)");
    println!("workspace: {}", cfg.workspace_path().display());
    let channel_count = match &channel_mgr {
        Some(mgr) => mgr.enabled_channels().await.len(),
        None => 0,
    };
    println!(
        "cron: enabled  heartbeat: {}  channels: {channel_count}",
        if cfg.gateway.heartbeat.enabled {
            "enabled"
        } else {
            "disabled"
        }
    );

    let _ = signal::ctrl_c().await;
    println!("\nshutting down…");

    if let Some(mgr) = &channel_mgr {
        mgr.stop_all().await;
    }
    let _ = shutdown_tx.send(());
    cron_svc.stop().await;
    hb.stop();
    server.abort();
    agent_task.abort();

    let flushed = agent.flush_sessions();
    if flushed > 0 {
        log::info!("Shutdown: flushed {} session(s) to disk", flushed);
    }

    Ok(())
}

// ---------------------------------------------------------------------------
// Cron handler
// ---------------------------------------------------------------------------

struct CronAgentHandler {
    agent: Arc<agent::AgentLoop>,
    dream: Arc<MemoryDream>,
}

#[async_trait]
impl JobHandler for CronAgentHandler {
    async fn on_job(&self, job: &CronJob) -> Result<Option<String>, String> {
        if job.id == "system::dream" {
            let did_work = self.dream.run().await;
            return Ok(Some(if did_work {
                "dream: completed".into()
            } else {
                "dream: nothing to process".into()
            }));
        }

        let channel = job
            .payload
            .channel
            .clone()
            .unwrap_or_else(|| "cron".to_string());
        let chat_id = job.payload.to.clone().unwrap_or_else(|| job.id.clone());
        let inbound = InboundMessage {
            channel: channel.clone(),
            sender_id: "cron".into(),
            chat_id: chat_id.clone(),
            content: job.payload.message.clone(),
            timestamp: Local::now(),
            media: Vec::new(),
            metadata: Default::default(),
            session_key_override: None,
        };

        let response = self
            .agent
            .process_inbound(inbound)
            .await
            .map(|r| r.final_content)?;

        if job.payload.deliver
            && !job
                .payload
                .to
                .as_ref()
                .map(|s| s.is_empty())
                .unwrap_or(true)
        {
            if let Some(ref content) = response {
                if !content.is_empty() {
                    let outbound = OutboundMessage {
                        channel,
                        chat_id,
                        content: content.clone(),
                        reply_to: None,
                        media: Vec::new(),
                        metadata: Default::default(),
                    };
                    self.agent.bus().publish_outbound(outbound).await;
                }
            }
        }

        Ok(response)
    }
}

// ---------------------------------------------------------------------------
// Heartbeat plug-ins
// ---------------------------------------------------------------------------

fn pick_heartbeat_target(
    channel_mgr: &Option<Arc<channels::ChannelManager>>,
) -> Option<(String, String)> {
    match channel_mgr {
        Some(mgr) => {
            let enabled_channels = tokio::task::block_in_place(|| {
                tokio::runtime::Handle::current().block_on(mgr.enabled_channels())
            });
            let enabled: std::collections::HashSet<String> = enabled_channels.into_iter().collect();

            for channel in &enabled {
                if channel != "cli" && channel != "system" {
                    return Some((channel.clone(), "default".into()));
                }
            }

            None
        }
        None => None,
    }
}

struct HeartbeatChannelNotifier {
    bus: Arc<bus::MessageBus>,
    channel_mgr: Arc<channels::ChannelManager>,
}

#[async_trait]
impl HeartbeatNotifier for HeartbeatChannelNotifier {
    async fn notify(&self, text: &str) {
        if let Some((channel, chat_id)) = pick_heartbeat_target(&Some(self.channel_mgr.clone())) {
            if channel == "cli" {
                return;
            }
            let outbound = OutboundMessage {
                channel,
                chat_id,
                content: text.to_string(),
                reply_to: None,
                media: Vec::new(),
                metadata: Default::default(),
            };
            self.bus.publish_outbound(outbound).await;
        }
    }
}

struct GatewayAgentExecutor {
    agent: Arc<agent::AgentLoop>,
    channel_mgr: Arc<channels::ChannelManager>,
    keep_recent_messages: usize,
}

#[async_trait]
impl HeartbeatExecutor for GatewayAgentExecutor {
    async fn execute(&self, tasks: &str) -> Option<String> {
        let (channel, chat_id) = pick_heartbeat_target(&Some(self.channel_mgr.clone()))
            .unwrap_or_else(|| ("heartbeat".into(), "heartbeat".into()));

        let inbound = InboundMessage {
            channel,
            sender_id: "heartbeat".into(),
            chat_id,
            content: tasks.to_string(),
            timestamp: Local::now(),
            media: Vec::new(),
            metadata: Default::default(),
            session_key_override: Some("heartbeat:default".into()),
        };

        let result = self
            .agent
            .process_inbound(inbound)
            .await
            .ok()
            .and_then(|r| r.final_content);

        if self.keep_recent_messages > 0 {
            self.agent
                .retain_heartbeat_session(self.keep_recent_messages);
        }

        result
    }
}

// ---------------------------------------------------------------------------
// /health handler
// ---------------------------------------------------------------------------

async fn handle_health_request(
    stream: &mut tokio::net::TcpStream,
    model: &str,
    started_at: SystemTime,
) -> std::io::Result<()> {
    use tokio::io::{AsyncReadExt, AsyncWriteExt};
    let mut buf = [0u8; 1024];
    let _ = stream.read(&mut buf).await?;

    let uptime_s = SystemTime::now()
        .duration_since(started_at)
        .map(|d| d.as_secs())
        .unwrap_or(0);
    let body = format!("{{\"status\":\"ok\",\"model\":\"{model}\",\"uptime_s\":{uptime_s}}}");
    let resp = format!(
        "HTTP/1.1 200 OK\r\n\
         Content-Type: application/json\r\n\
         Content-Length: {len}\r\n\
         Connection: close\r\n\
         \r\n\
         {body}",
        len = body.len(),
        body = body,
    );
    stream.write_all(resp.as_bytes()).await?;
    stream.shutdown().await
}

// ---------------------------------------------------------------------------
// Dream system job registration
// ---------------------------------------------------------------------------

async fn register_dream_job(svc: &Arc<::cron::CronService>, cfg: &config::Config) {
    let interval_h = cfg.agents.defaults.dream.interval_h.max(1);
    let interval_ms: i64 = (interval_h as i64).saturating_mul(3_600_000);
    let job = CronJob {
        id: "system::dream".into(),
        name: "Dream".into(),
        enabled: true,
        schedule: CronSchedule {
            kind: ScheduleKind::Every,
            every_ms: Some(interval_ms),
            ..Default::default()
        },
        payload: CronPayload {
            kind: PayloadKind::SystemEvent,
            message: "[dream]".into(),
            deliver: false,
            channel: None,
            to: None,
        },
        state: Default::default(),
        created_at_ms: 0,
        updated_at_ms: 0,
        delete_after_run: false,
    };
    svc.register_system_job(job).await;
}

// ---------------------------------------------------------------------------
// agent command
// ---------------------------------------------------------------------------

async fn agent_run(args: AgentArgs) -> Result<(), String> {
    if args.logs {
        let _ = env_logger::Builder::from_default_env()
            .filter_level(log::LevelFilter::Info)
            .try_init();
    }

    let Runtime { config: cfg, .. } =
        Runtime::from_config(args.config.as_deref(), args.workspace.as_deref())?;
    let bundle = LoopBundle::build_agent_loop(&cfg, None).await?;
    let agent = Arc::new(bundle.agent);

    let session = args
        .session
        .clone()
        .unwrap_or_else(|| "cli:default".to_string());
    let (channel, chat_id) = split_session(&session);

    if let Some(message) = args.message {
        return one_shot(&agent, &channel, &chat_id, &session, &message).await;
    }

    repl(agent, channel, chat_id, session).await
}

async fn one_shot(
    agent: &Arc<agent::AgentLoop>,
    channel: &str,
    chat_id: &str,
    session_key: &str,
    message: &str,
) -> Result<(), String> {
    let body = if message == "-" {
        let mut buf = String::new();
        io::stdin()
            .read_to_string(&mut buf)
            .map_err(|e| e.to_string())?;
        buf
    } else {
        message.to_string()
    };

    let inbound = InboundMessage {
        channel: channel.into(),
        sender_id: "cli".into(),
        chat_id: chat_id.into(),
        content: body,
        timestamp: Local::now(),
        media: Vec::new(),
        metadata: Default::default(),
        session_key_override: Some(session_key.into()),
    };
    let result = agent.process_inbound(inbound).await?;
    if let Some(text) = result.final_content {
        println!("{text}");
    }
    Ok(())
}

async fn repl(
    agent: Arc<agent::AgentLoop>,
    channel: String,
    chat_id: String,
    session_key: String,
) -> Result<(), String> {
    let mut rl = DefaultEditor::new().map_err(|e| e.to_string())?;
    let history = config::paths::get_cli_history_path();
    if let Some(parent) = history.parent() {
        let _ = std::fs::create_dir_all(parent);
    }
    let _ = rl.load_history(&history);

    println!("nanobot agent — session: {session_key}");
    println!("Type a message and press Enter. /exit to quit.");

    loop {
        match rl.readline("» ") {
            Ok(line) => {
                let trimmed = line.trim();
                if trimmed.is_empty() {
                    continue;
                }
                if matches!(trimmed, "/exit" | "/quit" | ":q") {
                    break;
                }
                let _ = rl.add_history_entry(line.as_str());
                let inbound = InboundMessage {
                    channel: channel.clone(),
                    sender_id: "cli".into(),
                    chat_id: chat_id.clone(),
                    content: trimmed.to_string(),
                    timestamp: Local::now(),
                    media: Vec::new(),
                    metadata: Default::default(),
                    session_key_override: Some(session_key.clone()),
                };
                match agent.process_inbound(inbound).await {
                    Ok(result) => {
                        if let Some(text) = result.final_content {
                            println!("{text}");
                        }
                    }
                    Err(e) => eprintln!("error: {e}"),
                }
            }
            Err(ReadlineError::Interrupted) | Err(ReadlineError::Eof) => break,
            Err(e) => return Err(e.to_string()),
        }
    }
    let _ = rl.save_history(&history);
    Ok(())
}

fn split_session(key: &str) -> (String, String) {
    if let Some((a, b)) = key.split_once(':') {
        if !a.is_empty() && !b.is_empty() {
            return (a.to_string(), b.to_string());
        }
    }
    ("cli".into(), key.to_string())
}

// ---------------------------------------------------------------------------
// status commands
// ---------------------------------------------------------------------------

async fn status_cmd(workspace: Option<PathBuf>, config: Option<PathBuf>) -> Result<(), String> {
    let Runtime {
        config: cfg,
        config_path,
    } = Runtime::from_config(config.as_deref(), workspace.as_deref())?;

    let workspace = get_workspace_path(Some(&cfg.agents.defaults.workspace));
    let cfg_disp = config_path
        .map(|p| p.display().to_string())
        .unwrap_or_else(|| config::get_config_path().display().to_string());

    println!("nanobot status");
    println!("──────────────");
    println!("Config:    {cfg_disp}");
    println!("Workspace: {}", workspace.display());
    println!("Model:     {}", cfg.agents.defaults.model);
    println!(
        "Provider:  {}",
        match cfg.agents.defaults.provider.as_str() {
            "" | "auto" => "auto".to_string(),
            other => other.to_string(),
        }
    );
    println!();
    println!("Provider credentials:");
    for spec in PROVIDERS {
        if spec.name == "custom" {
            continue;
        }
        let pc = providers::provider_config_for(&cfg, Some(spec));
        let has_key = pc
            .and_then(|p| p.api_key.as_deref())
            .map(|k| !k.is_empty())
            .unwrap_or(false);
        let env_present = !spec.env_key.is_empty()
            && std::env::var(spec.env_key)
                .map(|v| !v.is_empty())
                .unwrap_or(false);
        let mark = if has_key {
            "✓ configured"
        } else if env_present {
            "✓ env"
        } else if spec.is_oauth {
            "  OAuth (run `nanobot provider login`)"
        } else if spec.is_local {
            "  local"
        } else if spec.is_direct {
            "  direct"
        } else {
            "  not set"
        };
        println!("  {:<22} {}", spec.label(), mark);
    }

    if let Some(spec) = providers::resolve_spec(&cfg) {
        println!();
        println!("Selected provider for this model: {}", spec.label());
    } else {
        println!();
        println!(
            "Selected provider for this model: <none auto-detected>; will fall back to OpenAI-compat."
        );
    }
    Ok(())
}

// ---------------------------------------------------------------------------
// channels commands
// ---------------------------------------------------------------------------

async fn channels_status_cmd(config: Option<PathBuf>) -> Result<(), String> {
    let Runtime { config: cfg, .. } = Runtime::from_config(config.as_deref(), None)?;
    println!("Channels");
    println!("────────");
    let configured = list_channels(&cfg);
    if configured.is_empty() {
        println!("  (none configured)");
    } else {
        let builtin = known_channel_names();
        for (name, enabled) in configured {
            let kind = if builtin.iter().any(|n| *n == name) {
                "builtin"
            } else {
                "plugin"
            };
            let mark = if enabled { "enabled " } else { "disabled" };
            println!("  {mark}  {name:<10} ({kind})");
        }
    }
    Ok(())
}

async fn channels_login_cmd(
    name: String,
    force: bool,
    config: Option<PathBuf>,
) -> Result<(), String> {
    let Runtime { config: cfg, .. } = Runtime::from_config(config.as_deref(), None)?;

    let section = cfg
        .channels
        .extras
        .get(&name)
        .cloned()
        .unwrap_or_else(|| Value::Object(Default::default()));

    let bus = MessageBus::new();
    let transcription = TranscriptionSettings::default();
    let entry = build_one(&name, section, bus, transcription)?;

    println!("{} login starting…", entry.display_name);
    let channel = entry.channel;
    let success = Arc::clone(&channel)
        .login(force)
        .await
        .map_err(|e| format!("{name} login failed: {e}"))?;

    if success {
        println!("{} login complete.", entry.display_name);
        Ok(())
    } else {
        Err(format!(
            "{} login was cancelled or failed",
            entry.display_name
        ))
    }
}

// ---------------------------------------------------------------------------
// plugins commands
// ---------------------------------------------------------------------------

async fn plugins_list_cmd(config: Option<PathBuf>) -> Result<(), String> {
    let Runtime { config: cfg, .. } = Runtime::from_config(config.as_deref(), None)?;
    let builtin = known_channel_names();
    let configured = list_channels(&cfg);

    let mut all: Vec<(String, bool, &'static str)> = Vec::new();
    for name in &builtin {
        let enabled = configured
            .iter()
            .find(|(n, _)| n == *name)
            .map(|(_, e)| *e)
            .unwrap_or(false);
        all.push(((*name).to_string(), enabled, "builtin"));
    }
    for (name, enabled) in &configured {
        if !builtin.iter().any(|n| n == name) {
            all.push((name.clone(), *enabled, "plugin"));
        }
    }
    all.sort_by(|a, b| a.0.cmp(&b.0));

    println!("Channel plugins");
    println!("───────────────");
    if all.is_empty() {
        println!("  (none)");
    } else {
        for (name, enabled, kind) in all {
            let mark = if enabled { "[on] " } else { "[off]" };
            println!("  {mark}  {name:<12} {kind}");
        }
    }
    Ok(())
}

// ---------------------------------------------------------------------------
// provider login / status
// ---------------------------------------------------------------------------

async fn provider_login(name: &str) -> Result<(), String> {
    match name.to_ascii_lowercase().as_str() {
        "github-copilot" | "github_copilot" | "copilot" => {
            let token = providers::login_github_copilot(|m| println!("{m}"))
                .await
                .map_err(|e| format!("github copilot login failed: {e}"))?;
            println!(
                "github copilot: login ok (account_id={})",
                token.account_id.as_deref().unwrap_or("?")
            );
            Ok(())
        }
        "openai-codex" | "openai_codex" | "codex" => {
            Err(
                "openai-codex login is not yet implemented in the Rust build. \
                 Run `codex login` (Node.js) and the Rust provider will pick \
                 up the credential from `~/.codex/auth.json`."
                    .into(),
            )
        }
        other => Err(format!("unknown provider: {other}")),
    }
}

fn provider_status(name: &str) -> Result<(), String> {
    match name.to_ascii_lowercase().as_str() {
        "github-copilot" | "github_copilot" | "copilot" => {
            match providers::get_github_copilot_login_status() {
                Some(tok) => println!(
                    "github copilot: logged in (account_id={})",
                    tok.account_id.as_deref().unwrap_or("?")
                ),
                None => println!("github copilot: not logged in"),
            }
            Ok(())
        }
        "openai-codex" | "openai_codex" | "codex" => {
            let storage = FileTokenStorage::new(
                providers::openai_codex_provider::TOKEN_FILENAME,
                providers::openai_codex_provider::TOKEN_APP_NAME,
                true,
            );
            match storage.load() {
                Some(tok) => println!(
                    "openai codex: logged in (account_id={})",
                    tok.account_id.as_deref().unwrap_or("?")
                ),
                None => println!("openai codex: not logged in"),
            }
            Ok(())
        }
        other => Err(format!("unknown provider: {other}")),
    }
}

// ---------------------------------------------------------------------------
// helpers
// ---------------------------------------------------------------------------

fn list_channels(cfg: &Config) -> Vec<(String, bool)> {
    let mut out: Vec<(String, bool)> = Vec::new();
    for (name, value) in cfg.channels.extras.iter() {
        let enabled = value
            .as_object()
            .and_then(|o| o.get("enabled"))
            .map(value_truthy)
            .unwrap_or(false);
        out.push((name.clone(), enabled));
    }
    out.sort_by(|a, b| a.0.cmp(&b.0));
    out
}

fn value_truthy(v: &Value) -> bool {
    match v {
        Value::Bool(b) => *b,
        Value::Number(n) => n.as_i64().map(|i| i != 0).unwrap_or(false),
        Value::String(s) => !s.is_empty() && s != "false" && s != "0",
        _ => false,
    }
}

// ---------------------------------------------------------------------------
// tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use clap::Parser;

    #[test]
    fn parse_version() {
        let cli = Cli::try_parse_from(["nanobot", "version"]).unwrap();
        assert!(matches!(cli.command, Command::Version));
    }

    #[test]
    fn parse_agent_with_message() {
        let cli = Cli::try_parse_from(["nanobot", "agent", "-m", "hello"]).unwrap();
        match cli.command {
            Command::Agent { message, .. } => assert_eq!(message.as_deref(), Some("hello")),
            _ => panic!("wrong subcommand"),
        }
    }

    #[test]
    fn parse_serve_no_bind_uses_config() {
        let cli = Cli::try_parse_from(["nanobot", "serve"]).unwrap();
        match cli.command {
            Command::Serve {
                bind, port, host, ..
            } => {
                assert!(bind.is_none());
                assert!(port.is_none());
                assert!(host.is_none());
            }
            _ => panic!("wrong subcommand"),
        }
    }

    #[test]
    fn parse_provider_login_codex() {
        let cli = Cli::try_parse_from(["nanobot", "provider", "login", "openai-codex"]).unwrap();
        match cli.command {
            Command::Provider {
                sub: ProviderCommand::Login { name },
            } => assert_eq!(name, "openai-codex"),
            _ => panic!("wrong subcommand"),
        }
    }

    #[test]
    fn parse_gateway_default() {
        let cli = Cli::try_parse_from(["nanobot", "gateway"]).unwrap();
        assert!(matches!(cli.command, Command::Gateway { .. }));
    }

    #[test]
    fn parse_channels_status() {
        let cli = Cli::try_parse_from(["nanobot", "channels", "status"]).unwrap();
        match cli.command {
            Command::Channels {
                sub: ChannelsCommand::Status,
            } => {}
            _ => panic!("wrong subcommand"),
        }
    }

    #[test]
    fn parse_plugins_list() {
        let cli = Cli::try_parse_from(["nanobot", "plugins", "list"]).unwrap();
        match cli.command {
            Command::Plugins {
                sub: PluginsCommand::List,
            } => {}
            _ => panic!("wrong subcommand"),
        }
    }

    #[test]
    fn parse_global_config() {
        let cli = Cli::try_parse_from(["nanobot", "--config", "/tmp/cfg.json", "status"]).unwrap();
        assert_eq!(
            cli.config.as_deref(),
            Some(std::path::Path::new("/tmp/cfg.json"))
        );
    }

    #[test]
    fn parse_choice_aliases() {
        assert_eq!(ProviderChoice::parse("openai"), Some(ProviderChoice::OpenAI));
        assert_eq!(ProviderChoice::parse("Claude"), Some(ProviderChoice::Anthropic));
        assert_eq!(ProviderChoice::parse("azure-openai"), Some(ProviderChoice::Azure));
        assert_eq!(ProviderChoice::parse("bedrock"), Some(ProviderChoice::Bedrock));
        assert_eq!(ProviderChoice::parse("aws-bedrock"), Some(ProviderChoice::Bedrock));
        assert_eq!(
            ProviderChoice::parse("github-copilot"),
            Some(ProviderChoice::GithubCopilot)
        );
        assert_eq!(ProviderChoice::parse("custom"), Some(ProviderChoice::Custom));
        assert_eq!(ProviderChoice::parse("nope"), None);
    }

    #[test]
    fn find_bedrock_spec() {
        let spec = find_by_name("bedrock");
        assert!(spec.is_some());
        assert_eq!(spec.unwrap().backend, Backend::Bedrock);
    }
}
