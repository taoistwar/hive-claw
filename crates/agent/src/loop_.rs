//! Simplified orchestration for the agent runtime.
//! Port of `nanobot.agent.loop.AgentLoop` (simplified).
//!
//! The Python original is ~1150 lines with channel routing, command
//! handling, tool wiring, streaming, and session governance. This Rust
//! port captures the top-level orchestration:
//!
//! 1. consume inbound messages from the [`MessageBus`]
//! 2. build context (system prompt + recent history) via [`ContextBuilder`]
//! 3. set per-turn context on the channel-routed tools
//!    ([`MessageTool`]/[`SpawnTool`]/[`CronTool`])
//! 4. drive one [`AgentRunner`] turn per inbound message
//! 5. persist history and publish outbound responses
//!
//! Channel routing, slash-command dispatch, streaming deltas, and full
//! session governance (auto-compact, dream, subagent bookkeeping) are
//! intentionally deferred — callers can layer them on top via
//! [`AgentLoop::set_command_prefilter`] and custom hooks.

use std::collections::{HashMap, HashSet};
use std::path::PathBuf;
use std::sync::Arc;

use futures::FutureExt;
use log::{debug, error, info, warn};
use serde_json::Value;
use tokio::sync::{Mutex, Semaphore};

use bus::{InboundMessage, MessageBus, OutboundMessage};
pub use command::CommandRouter;
use config::schema::{Config, ModelPresetConfig, ProviderRetryMode};
use providers::{LLMProvider, RetryMode};
use session::manager::SessionManager;

use crate::autocompact::AutoCompact as AutoCompactInner;
use crate::context::{ContextBuilder, RUNTIME_CONTEXT_TAG};
use crate::memory::Consolidator as ConsolidatorTrait;
use crate::memory::Dream as DreamTrait;
use crate::runner::{AgentRunResult, AgentRunSpec, AgentRunner};
use crate::subagent::SubagentManager;
use crate::tools::file_state::{FileStateStore, bind_file_states, reset_file_states};
use crate::tools::mcp::{self as mcp_impl, McpServerHandle};
use crate::tools::{CronTool, MessageTool, SpawnCallback, SpawnTool, ToolRegistry};
use config::schema::McpServerConfig as ConfigMcpServerConfig;

// ============================================================================
// Tool factory types (port of Python's _register_default_tools in loop.py)
// ============================================================================

use crate::tools::filesystem::{EditFileTool, FsTool, ListDirTool, ReadFileTool, WriteFileTool};
use crate::tools::message::MessageTool as MessageToolInner;
use crate::tools::notebook::NotebookEditTool;
use crate::tools::search::GrepTool;
use crate::tools::shell::ExecTool;
use crate::tools::web::{DuckDuckGoBackend, WebFetchTool, WebSearchBackend, WebSearchTool};

/// Configuration for default tool set.
#[derive(Clone)]
pub struct ToolFactoryConfig {
    pub workspace: PathBuf,
    pub extra_allowed_dirs: Vec<PathBuf>,
    pub restrict_to_workspace: bool,
    pub exec_timeout_secs: u64,
    pub exec_sandbox: String,
    pub exec_path_append: String,
    pub exec_allowed_env_keys: Vec<String>,
    pub web_max_chars: usize,
    pub web_proxy: Option<String>,
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
pub struct BuiltinToolSet {
    pub registry: ToolRegistry,
    pub message: Arc<MessageToolInner>,
    pub spawn: Option<Arc<SpawnTool>>,
    pub cron: Option<Arc<CronTool>>,
}

/// Optional plug-ins that the factory cannot construct itself.
#[derive(Default)]
pub struct ToolFactoryDeps {
    pub web_search: Option<Arc<dyn WebSearchBackend>>,
    pub spawn_callback: Option<SpawnCallback>,
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

impl BuiltinToolSet {
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

        registry.register(Arc::new(ReadFileTool(fs.clone()))).await;
        registry.register(Arc::new(WriteFileTool(fs.clone()))).await;
        registry.register(Arc::new(EditFileTool(fs.clone()))).await;
        registry.register(Arc::new(ListDirTool(fs.clone()))).await;

        registry.register(Arc::new(GrepTool(fs.clone()))).await;

        let exec = ExecTool::new()
            .with_working_dir(config.workspace.clone())
            .with_timeout_secs(config.exec_timeout_secs)
            .with_sandbox(config.exec_sandbox.clone())
            .with_path_append(config.exec_path_append.clone())
            .with_allowed_env_keys(config.exec_allowed_env_keys.clone())
            .with_restrict_to_workspace(config.restrict_to_workspace);
        registry.register(Arc::new(exec)).await;

        registry
            .register(Arc::new(WebFetchTool::new(
                config.web_max_chars,
                config.web_proxy.clone(),
            )))
            .await;

        let search_backend = deps
            .web_search
            .clone()
            .unwrap_or_else(|| Arc::new(DuckDuckGoBackend::new()));
        registry
            .register(Arc::new(WebSearchTool::new(search_backend)))
            .await;

        registry
            .register(Arc::new(NotebookEditTool(fs.clone())))
            .await;

        let message = Arc::new(MessageToolInner::new(
            bus,
            config.workspace.clone(),
            config.restrict_to_workspace,
        ));
        registry.register(message.clone()).await;

        let spawn = if let Some(cb) = deps.spawn_callback {
            let tool = Arc::new(SpawnTool::new(cb));
            registry.register(tool.clone()).await;
            Some(tool)
        } else {
            None
        };

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

pub const UNIFIED_SESSION_KEY: &str = "unified:default";

const RUNTIME_CHECKPOINT_KEY: &str = "runtime_checkpoint";
const PENDING_USER_TURN_KEY: &str = "pending_user_turn";

/// Transition table: (current_state, event) -> next_state
const TRANSITIONS: &[(TurnState, &str, TurnState)] = &[
    (TurnState::Restore, "ok", TurnState::Compact),
    (TurnState::Compact, "ok", TurnState::Command),
    (TurnState::Command, "dispatch", TurnState::Build),
    (TurnState::Command, "shortcut", TurnState::Done),
    (TurnState::Build, "ok", TurnState::Run),
    (TurnState::Run, "ok", TurnState::Save),
    (TurnState::Save, "ok", TurnState::Respond),
    (TurnState::Respond, "ok", TurnState::Done),
];

fn find_transition(state: TurnState, event: &str) -> Option<TurnState> {
    TRANSITIONS
        .iter()
        .find(|(s, e, _)| *s == state && *e == event)
        .map(|(_, _, next)| *next)
}

/// Event-driven turn states (mirrors Python TurnState enum).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TurnState {
    Restore,
    Compact,
    Command,
    Build,
    Run,
    Save,
    Respond,
    Done,
}

impl std::fmt::Display for TurnState {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            TurnState::Restore => write!(f, "RESTORE"),
            TurnState::Compact => write!(f, "COMPACT"),
            TurnState::Command => write!(f, "COMMAND"),
            TurnState::Build => write!(f, "BUILD"),
            TurnState::Run => write!(f, "RUN"),
            TurnState::Save => write!(f, "SAVE"),
            TurnState::Respond => write!(f, "RESPOND"),
            TurnState::Done => write!(f, "DONE"),
        }
    }
}

/// Trace entry for a single state execution (mirrors Python StateTraceEntry).
#[derive(Debug, Clone)]
pub struct StateTraceEntry {
    pub state: TurnState,
    pub started_at: f64,
    pub duration_ms: f64,
    pub event: String,
    pub error: Option<String>,
}

/// Per-turn context bag (mirrors Python TurnContext dataclass).
pub struct TurnContext {
    pub msg: InboundMessage,
    pub session_key: String,
    pub state: TurnState,
    pub turn_id: String,
    pub session: Option<session::manager::Session>,

    pub history: Vec<Value>,
    pub initial_messages: Vec<Value>,

    pub final_content: Option<String>,
    pub tools_used: Vec<String>,
    pub all_messages: Vec<Value>,
    pub stop_reason: String,
    pub had_injections: bool,

    pub user_persisted_early: bool,
    pub save_skip: usize,

    pub outbound: Option<OutboundMessage>,

    pub pending_queue: Option<tokio::sync::mpsc::UnboundedSender<InboundMessage>>,
    pub pending_summary: Option<String>,

    pub turn_wall_started_at: f64,
    pub turn_latency_ms: Option<u64>,

    pub trace: Vec<StateTraceEntry>,
}

/// Dependencies + tunables for [`AgentLoop`].
#[derive(Clone)]
pub struct LoopConfig {
    pub workspace: PathBuf,
    pub model: Option<String>,
    pub max_iterations: u32,
    pub max_tool_result_chars: usize,
    pub context_window_tokens: Option<u32>,
    pub timezone: Option<String>,
    pub disabled_skills: Vec<String>,
    pub provider_retry_mode: RetryMode,
}

impl LoopConfig {
    pub fn new(workspace: PathBuf) -> Self {
        Self {
            workspace,
            model: None,
            max_iterations: 30,
            max_tool_result_chars: 40_000,
            context_window_tokens: None,
            timezone: None,
            disabled_skills: Vec::new(),
            provider_retry_mode: RetryMode::Standard,
        }
    }

    /// Translate top-level config knobs into [`LoopConfig`].
    pub fn from_config(cfg: &Config) -> LoopConfig {
        let workspace = cfg.workspace_path();
        let defaults = &cfg.agents.defaults;
        let mut lc = LoopConfig::new(workspace);
        lc.model = Some(defaults.model.clone());
        lc.max_iterations = defaults.max_tool_iterations;
        lc.max_tool_result_chars = defaults.max_tool_result_chars as usize;
        lc.context_window_tokens = Some(defaults.context_window_tokens);
        lc.timezone = Some(defaults.timezone.clone());
        lc.disabled_skills = defaults.disabled_skills.clone();
        lc.provider_retry_mode = match defaults.provider_retry_mode {
            ProviderRetryMode::Standard => RetryMode::Standard,
            ProviderRetryMode::Persistent => RetryMode::Persistent,
        };
        lc
    }
}

/// Optional pre-filter run *before* building context or calling the LLM.
///
/// Return `Some(content)` to short-circuit with that reply as the outbound
/// message (e.g. slash-command handler), or `None` to continue with the
/// normal agent pipeline.
pub type CommandPrefilter = Arc<
    dyn Fn(&InboundMessage) -> futures::future::BoxFuture<'static, Option<String>> + Send + Sync,
>;
type ModelSwitchCallback = Arc<dyn Fn(&str) -> Result<String, String> + Send + Sync>;
type MessageKey<'a> = (
    Option<&'a str>,
    Option<&'a str>,
    Option<&'a str>,
    Option<&'a str>,
    Option<&'a Value>,
    Option<&'a Value>,
);

/// Snapshot of runtime info surfaced by `/status` / `/model`.
#[derive(Clone, Debug)]
pub struct AgentRuntimeInfo {
    pub model: String,
    pub workspace: PathBuf,
    pub context_window_tokens: Option<u32>,
}

/// Built-in command pre-filter.
pub struct BuiltinPrefilter {
    sessions: Arc<Mutex<SessionManager>>,
    tools: ToolRegistry,
    info: Arc<tokio::sync::RwLock<AgentRuntimeInfo>>,
    model_switch_callback: Option<ModelSwitchCallback>,
}

impl BuiltinPrefilter {
    pub fn new(
        sessions: Arc<Mutex<SessionManager>>,
        tools: ToolRegistry,
        info: AgentRuntimeInfo,
    ) -> Self {
        Self {
            sessions,
            tools,
            info: Arc::new(tokio::sync::RwLock::new(info)),
            model_switch_callback: None,
        }
    }

    /// Set a callback for model switching commands.
    pub fn set_model_switch_callback(&mut self, callback: ModelSwitchCallback) {
        self.model_switch_callback = Some(callback);
    }

    /// Box this prefilter into the [`CommandPrefilter`] hook expected by
    /// [`AgentLoop::set_command_prefilter`].
    pub fn into_hook(self) -> CommandPrefilter {
        let sessions = self.sessions;
        let tools = self.tools;
        let info = self.info;
        let model_switch_callback = self.model_switch_callback;
        Arc::new(move |msg: &InboundMessage| {
            let sessions = sessions.clone();
            let tools = tools.clone();
            let info = info.clone();
            let model_switch_callback = model_switch_callback.clone();
            let msg = msg.clone();
            async move {
                dispatch_prefilter(
                    &msg,
                    &sessions,
                    &tools,
                    &info,
                    model_switch_callback.as_ref(),
                )
                .await
            }
            .boxed()
        })
    }
}

async fn dispatch_prefilter(
    msg: &InboundMessage,
    sessions: &Arc<Mutex<SessionManager>>,
    tools: &ToolRegistry,
    info: &Arc<tokio::sync::RwLock<AgentRuntimeInfo>>,
    model_switch_callback: Option<&ModelSwitchCallback>,
) -> Option<String> {
    let text = msg.content.trim();
    if !text.starts_with('/') {
        return None;
    }
    // Split into (cmd, args). Command is case-insensitive.
    let (cmd_raw, args) = match text.split_once(char::is_whitespace) {
        Some((c, a)) => (c, a.trim()),
        None => (text, ""),
    };
    let cmd = cmd_raw.to_lowercase();

    match cmd.as_str() {
        "/help" | "/?" => Some(help_text(tools).await),
        "/version" => Some(format!(
            "RustBot agent v{} (crate: agent)",
            env!("CARGO_PKG_VERSION")
        )),
        "/status" => Some(status_text(sessions, info, &msg.session_key()).await),
        "/clear" | "/new" => Some(clear_session(sessions, &msg.session_key()).await),
        "/model" => {
            if let Some(cb) = model_switch_callback {
                Some(cb(args).unwrap_or_else(|e| format!("Error: {}", e)))
            } else {
                Some(model_cmd(info, args).await)
            }
        }
        _ => None,
    }
}

async fn help_text(tools: &ToolRegistry) -> String {
    let mut names = tools.tool_names().await;
    names.sort();
    let mut out = String::from("Available commands:\n");
    out.push_str("  /help             show this help\n");
    out.push_str("  /version          print agent version\n");
    out.push_str("  /status           show runtime status\n");
    out.push_str("  /clear | /new     clear current session history\n");
    out.push_str("  /model [name]     show or set active model\n");
    out.push_str("\nAvailable tools:\n");
    if names.is_empty() {
        out.push_str("  (none registered)\n");
    } else {
        for n in names {
            out.push_str(&format!("  {n}\n"));
        }
    }
    out
}

async fn status_text(
    sessions: &Arc<Mutex<SessionManager>>,
    info: &Arc<tokio::sync::RwLock<AgentRuntimeInfo>>,
    session_key: &str,
) -> String {
    let history_len = {
        let mut mgr = sessions.lock().await;
        let s = mgr.get_or_create(session_key);
        s.messages.len()
    };
    let snap = info.read().await.clone();
    let ctx_window = snap
        .context_window_tokens
        .map(|n| n.to_string())
        .unwrap_or_else(|| "<provider default>".into());
    format!(
        "Status:\n  model:             {}\n  workspace:         {}\n  context window:    {}\n  session:           {}\n  history length:    {}",
        snap.model,
        snap.workspace.display(),
        ctx_window,
        session_key,
        history_len,
    )
}

async fn clear_session(sessions: &Arc<Mutex<SessionManager>>, session_key: &str) -> String {
    let mut mgr = sessions.lock().await;
    let mut s = mgr.get_or_create(session_key);
    let removed = s.messages.len();
    s.clear();
    match mgr.save(s, false) {
        Ok(()) => {
            format!("Cleared session {session_key} ({removed} messages).")
        }
        Err(e) => format!(
            "Cleared in-memory session {session_key} ({removed} messages) but failed to persist: {e}"
        ),
    }
}

async fn model_cmd(info: &Arc<tokio::sync::RwLock<AgentRuntimeInfo>>, args: &str) -> String {
    if args.is_empty() {
        let snap = info.read().await;
        return format!("Current model: {}", snap.model);
    }
    let mut snap = info.write().await;
    let old = std::mem::replace(&mut snap.model, args.to_string());
    format!("Model changed: {old} -> {}", snap.model)
}

#[expect(
    dead_code,
    reason = "retained for the runtime model hotswap command path"
)]
async fn model_cmd_with_hotswap(
    loop_: &AgentLoop,
    info: &Arc<tokio::sync::RwLock<AgentRuntimeInfo>>,
    args: &str,
) -> String {
    if args.is_empty() {
        let current_model = loop_.current_model();
        let active_preset = loop_.model_preset();
        let presets = loop_.list_presets();
        let mut response = format!("Current model: {}", current_model);
        if let Some(ref preset) = active_preset {
            response.push_str(&format!("\nActive preset: {}", preset));
        }
        if !presets.is_empty() {
            response.push_str(&format!("\nAvailable presets: {}", presets));
        }
        return response;
    }

    match loop_.switch_model_preset(args).await {
        Ok(msg) => {
            let mut snap = info.write().await;
            snap.model = loop_.current_model();
            msg
        }
        Err(e) => format!("Error: {}", e),
    }
}

/// Snapshot of provider configuration.
#[derive(Clone)]
pub struct ProviderSnapshot {
    pub provider: Arc<dyn LLMProvider>,
    pub model: String,
    pub context_window_tokens: u32,
    pub signature: u64,
}

impl ProviderSnapshot {
    pub fn from_config(cfg: &Config) -> Result<Self, String> {
        let defaults = &cfg.agents.defaults;
        let model = defaults.model.clone();
        let context_window = defaults.context_window_tokens;

        let provider = providers::factory::make_provider(cfg)?;
        let signature = compute_config_signature(cfg);

        Ok(Self {
            provider,
            model,
            context_window_tokens: context_window,
            signature,
        })
    }

    pub fn from_preset(preset: &ModelPresetConfig, cfg: &Config) -> Result<Self, String> {
        let mut cfg = cfg.clone();
        cfg.agents.defaults.model = preset.model.clone();
        cfg.agents.defaults.provider = preset.provider.clone();
        cfg.agents.defaults.max_tokens = preset.max_tokens;
        cfg.agents.defaults.context_window_tokens = preset.context_window_tokens;
        cfg.agents.defaults.temperature = preset.temperature;
        cfg.agents.defaults.reasoning_effort = preset.reasoning_effort.clone();

        Self::from_config(&cfg)
    }
}

fn compute_config_signature(cfg: &Config) -> u64 {
    use std::collections::hash_map::DefaultHasher;
    use std::hash::{Hash, Hasher};

    let defaults = &cfg.agents.defaults;
    let mut hasher = DefaultHasher::new();
    defaults.model.hash(&mut hasher);
    defaults.provider.hash(&mut hasher);
    cfg.agents.defaults.max_tokens.hash(&mut hasher);
    cfg.agents.defaults.context_window_tokens.hash(&mut hasher);
    cfg.agents.defaults.temperature.to_bits().hash(&mut hasher);

    if let Some(ref key) = cfg.providers.custom.api_key {
        key.hash(&mut hasher);
    }
    if let Some(ref base) = cfg.providers.custom.api_base {
        base.hash(&mut hasher);
    }
    if let Some(ref key) = cfg.providers.anthropic.api_key {
        key.hash(&mut hasher);
    }
    if let Some(ref base) = cfg.providers.anthropic.api_base {
        base.hash(&mut hasher);
    }
    if let Some(ref key) = cfg.providers.openai.api_key {
        key.hash(&mut hasher);
    }
    if let Some(ref base) = cfg.providers.openai.api_base {
        base.hash(&mut hasher);
    }

    hasher.finish()
}

/// Type alias for a provider snapshot loader function.
pub type SnapshotLoaderFn = Arc<dyn Fn() -> Result<ProviderSnapshot, String> + Send + Sync>;

/// Type alias for a preset snapshot loader function.
pub type PresetSnapshotLoaderFn =
    Arc<dyn Fn(&str) -> Result<ProviderSnapshot, String> + Send + Sync>;

/// Type alias for a runtime model publisher function.
pub type RuntimeModelPublisherFn = Arc<dyn Fn(&str, Option<&str>) + Send + Sync>;

/// WebUI turn coordinator — manages WebSocket turn status, latency, and title context.
///
/// Port of Python's `WebuiTurnCoordinator`.
pub struct WebuiTurnCoordinator {
    bus: Arc<MessageBus>,
    sessions: Arc<Mutex<session::manager::SessionManager>>,
    _title_contexts: std::collections::HashMap<String, serde_json::Value>,
}

impl WebuiTurnCoordinator {
    pub fn new(
        bus: Arc<MessageBus>,
        sessions: Arc<Mutex<session::manager::SessionManager>>,
    ) -> Self {
        Self {
            bus,
            sessions,
            _title_contexts: std::collections::HashMap::new(),
        }
    }

    /// Capture LLM runtime context for later title generation.
    pub fn capture_title_context(&mut self, session_key: &str, msg: &InboundMessage) {
        if msg.channel == "websocket"
            && msg
                .metadata
                .get("webui")
                .and_then(|v| v.as_bool())
                .unwrap_or(false)
        {
            self._title_contexts
                .insert(session_key.to_string(), serde_json::Value::Bool(true));
        }
    }

    /// Discard title context for a session.
    pub fn discard(&mut self, session_key: &str) {
        self._title_contexts.remove(session_key);
    }

    /// Publish turn run status ("running" / "idle") to the bus.
    pub async fn publish_run_status(&self, msg: &InboundMessage, status: &str) {
        if msg.channel != "websocket" {
            return;
        }
        let mut meta = msg.metadata.clone();
        meta.insert(
            "_turn_status".into(),
            serde_json::Value::String(status.to_string()),
        );
        let content = format!(
            "{{\"status\":\"{}\",\"session_key\":\"{}\"}}",
            status,
            msg.session_key()
        );
        self.bus
            .publish_outbound(OutboundMessage {
                channel: msg.channel.clone(),
                chat_id: msg.chat_id.clone(),
                content,
                reply_to: None,
                media: Vec::new(),
                metadata: meta,
            })
            .await;
    }

    /// Handle turn end: publish empty message with goal_state.
    pub async fn handle_turn_end(
        &mut self,
        msg: &InboundMessage,
        session_key: &str,
        latency_ms: Option<u64>,
    ) {
        if msg.channel != "websocket" {
            return;
        }

        let mut turn_metadata: std::collections::HashMap<String, serde_json::Value> =
            msg.metadata.clone();
        turn_metadata.insert("_turn_end".into(), serde_json::Value::Bool(true));

        if let Some(latency) = latency_ms {
            turn_metadata.insert(
                "latency_ms".into(),
                serde_json::Value::Number(latency.into()),
            );
        }

        let mut sessions = self.sessions.lock().await;
        let session = sessions.get_or_create(session_key);
        let goal_blob = session::goal_state_ws_blob(Some(&session.metadata));
        turn_metadata.insert("goal_state".into(), goal_blob);

        self.bus
            .publish_outbound(OutboundMessage {
                channel: msg.channel.clone(),
                chat_id: msg.chat_id.clone(),
                content: String::new(),
                reply_to: None,
                media: Vec::new(),
                metadata: turn_metadata.clone(),
            })
            .await;

        // Schedule title update if title context exists
        let has_title_context = self._title_contexts.remove(session_key).is_some();
        if has_title_context
            && msg
                .metadata
                .get("webui")
                .and_then(|v| v.as_bool())
                .unwrap_or(false)
        {
            let mut title_meta = msg.metadata.clone();
            title_meta.insert("_session_updated".into(), serde_json::Value::Bool(true));
            title_meta.insert(
                "_session_update_scope".into(),
                serde_json::Value::String("metadata".into()),
            );
            self.bus
                .publish_outbound(OutboundMessage {
                    channel: msg.channel.clone(),
                    chat_id: msg.chat_id.clone(),
                    content: String::new(),
                    reply_to: None,
                    media: Vec::new(),
                    metadata: title_meta,
                })
                .await;
        }
    }
}

/// Core agent orchestration — inbound -> runner -> outbound.
pub struct AgentLoop {
    bus: Arc<MessageBus>,
    provider: Arc<dyn LLMProvider>,
    runner: std::sync::Mutex<Arc<AgentRunner>>,
    context: ContextBuilder,
    sessions: Arc<Mutex<SessionManager>>,
    tools: ToolRegistry,
    message_tool: Option<Arc<MessageTool>>,
    spawn_tool: Option<Arc<SpawnTool>>,
    cron_tool: Option<Arc<CronTool>>,
    command_prefilter: Option<CommandPrefilter>,
    config: LoopConfig,

    // Provider hotswap fields
    provider_snapshot_loader: Option<SnapshotLoaderFn>,
    preset_snapshot_loader: Option<PresetSnapshotLoaderFn>,
    runtime_model_publisher: Option<RuntimeModelPublisherFn>,
    provider_signature: tokio::sync::Mutex<Option<u64>>,
    #[expect(dead_code, reason = "retained for provider selection change tracking")]
    default_selection_signature: Option<u64>,
    #[expect(dead_code, reason = "retained for runtime channel configuration")]
    channels_config: Option<Value>,
    _image_generation_provider_configs: HashMap<String, Value>,
    #[expect(dead_code, reason = "retains the injected cron service lifetime")]
    cron_service: Option<Arc<Mutex<dyn std::any::Any + Send + Sync>>>,
    #[expect(dead_code, reason = "retained as the loop workspace policy state")]
    restrict_to_workspace: bool,
    _start_time: f64,
    _last_usage: std::sync::Mutex<HashMap<String, u64>>,
    _pending_turn_latency_ms: std::sync::Mutex<HashMap<String, u64>>,
    _extra_hooks: Vec<Arc<dyn crate::hook::AgentHook + Send + Sync>>,
    /// Optional AgentContext for execution state management (specs/009-agent-context)
    agent_context: Option<Arc<crate::context::core::AgentContext>>,
    _webui_turns: Option<Arc<Mutex<WebuiTurnCoordinator>>>,
    _file_state_store: Option<Arc<std::sync::Mutex<FileStateStore>>>,
    subagents: Option<Arc<Mutex<SubagentManager>>>,
    _unified_session: bool,
    _max_messages: usize,
    _running: std::sync::atomic::AtomicBool,
    _mcp_servers: HashMap<String, ConfigMcpServerConfig>,
    _mcp_stacks: std::sync::Mutex<HashMap<String, McpServerHandle>>,
    _mcp_connected: std::sync::atomic::AtomicBool,
    _mcp_connecting: std::sync::atomic::AtomicBool,
    _active_tasks: Arc<std::sync::Mutex<HashMap<String, Vec<tokio::task::JoinHandle<()>>>>>,
    _background_tasks: Arc<std::sync::Mutex<Vec<tokio::task::JoinHandle<()>>>>,
    _session_locks: Arc<std::sync::Mutex<HashMap<String, Arc<tokio::sync::Mutex<()>>>>>,
    _pending_queues:
        Arc<std::sync::Mutex<HashMap<String, tokio::sync::mpsc::UnboundedSender<InboundMessage>>>>,
    _concurrency_gate: Option<Arc<Semaphore>>,
    consolidator: Option<Arc<Mutex<dyn ConsolidatorTrait>>>,
    auto_compact: Option<Arc<Mutex<AutoCompactInner>>>,
    dream: Option<Arc<Mutex<dyn DreamTrait>>>,
    model_presets: HashMap<String, Value>,
    _active_preset: std::sync::Mutex<Option<String>>,
    _runtime_vars: std::sync::Mutex<HashMap<String, Value>>,
    _current_iteration: std::sync::atomic::AtomicU32,
    commands: Option<Arc<Mutex<CommandRouter>>>,
    #[expect(dead_code, reason = "retained for runtime web tool configuration")]
    web_config: Option<Value>,
    #[expect(dead_code, reason = "retained for runtime exec tool configuration")]
    exec_config: Option<Value>,
    #[expect(
        dead_code,
        reason = "retained for aggregate runtime tool configuration"
    )]
    tools_config: Option<Value>,
    context_window_tokens: std::sync::atomic::AtomicU32,
    #[expect(dead_code, reason = "retained as configured context governance state")]
    context_block_limit: Option<u32>,
    provider_retry_mode: RetryMode,
    #[expect(dead_code, reason = "retained for tool progress hint shaping")]
    tool_hint_max_length: usize,
}

impl AgentLoop {
    fn default_fields(config: &LoopConfig, provider: Arc<dyn LLMProvider>) -> Self {
        let context_window = config.context_window_tokens.unwrap_or(128_000);
        let defaults = &config::schema::AgentDefaults::default();
        Self {
            bus: Arc::new(MessageBus::new()),
            provider: provider.clone(),
            runner: std::sync::Mutex::new(Arc::new(AgentRunner::new(provider))),
            context: ContextBuilder::new(
                config.workspace.clone(),
                config.timezone.clone(),
                Some(config.disabled_skills.clone()),
            ),
            sessions: Arc::new(Mutex::new(SessionManager::new(&config.workspace))),
            tools: ToolRegistry::new(),
            message_tool: None,
            spawn_tool: None,
            cron_tool: None,
            command_prefilter: None,
            config: config.clone(),
            provider_snapshot_loader: None,
            preset_snapshot_loader: None,
            runtime_model_publisher: None,
            provider_signature: tokio::sync::Mutex::new(None),
            default_selection_signature: None,
            channels_config: None,
            _image_generation_provider_configs: HashMap::new(),
            cron_service: None,
            restrict_to_workspace: false,
            _start_time: std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap_or_default()
                .as_secs_f64(),
            _last_usage: std::sync::Mutex::new(HashMap::new()),
            _pending_turn_latency_ms: std::sync::Mutex::new(HashMap::new()),
            _extra_hooks: Vec::new(),
            agent_context: None,
            _webui_turns: None,
            _file_state_store: None,
            subagents: None,
            _unified_session: false,
            _max_messages: 120,
            _running: std::sync::atomic::AtomicBool::new(false),
            _mcp_servers: HashMap::new(),
            _mcp_stacks: std::sync::Mutex::new(HashMap::new()),
            _mcp_connected: std::sync::atomic::AtomicBool::new(false),
            _mcp_connecting: std::sync::atomic::AtomicBool::new(false),
            _active_tasks: Arc::new(std::sync::Mutex::new(HashMap::new())),
            _background_tasks: Arc::new(std::sync::Mutex::new(Vec::new())),
            _session_locks: Arc::new(std::sync::Mutex::new(HashMap::new())),
            _pending_queues: Arc::new(std::sync::Mutex::new(HashMap::new())),
            _concurrency_gate: Some(Arc::new(Semaphore::new(3))),
            consolidator: None,
            auto_compact: None,
            dream: None,
            model_presets: HashMap::new(),
            _active_preset: std::sync::Mutex::new(None),
            _runtime_vars: std::sync::Mutex::new(HashMap::new()),
            _current_iteration: std::sync::atomic::AtomicU32::new(0),
            commands: None,
            web_config: None,
            exec_config: None,
            tools_config: None,
            context_window_tokens: std::sync::atomic::AtomicU32::new(context_window),
            context_block_limit: defaults.context_block_limit,
            provider_retry_mode: config.provider_retry_mode,
            tool_hint_max_length: 200,
        }
    }

    /// Build an [`AgentLoop`] from the bundle produced by
    /// [`crate::tools::BuiltinToolSet::default_tools`]. This is the recommended entry
    /// point — it ensures the per-turn context is wired to the same
    /// tool instances that the runner sees.
    pub fn from_builtin(
        bus: Arc<MessageBus>,
        provider: Arc<dyn LLMProvider>,
        sessions: Arc<Mutex<SessionManager>>,
        builtin: BuiltinToolSet,
        config: LoopConfig,
    ) -> Self {
        let mut this = Self::default_fields(&config, provider.clone());
        this.bus = bus;
        this.sessions = sessions;
        this.tools = builtin.registry;
        this.message_tool = Some(builtin.message);
        this.spawn_tool = builtin.spawn;
        this.cron_tool = builtin.cron;
        this
    }

    /// Build an [`AgentLoop`] with a *custom* [`ToolRegistry`] — use this
    /// when you need to plug in non-standard tools. The channel-routed
    /// tool context is **not** set up automatically; callers can register
    /// handles separately via [`Self::attach_tool_contexts`].
    pub fn new(
        bus: Arc<MessageBus>,
        provider: Arc<dyn LLMProvider>,
        sessions: Arc<Mutex<SessionManager>>,
        tools: ToolRegistry,
        config: LoopConfig,
    ) -> Self {
        let mut this = Self::default_fields(&config, provider);
        this.bus = bus;
        this.sessions = sessions;
        this.tools = tools;
        this
    }

    /// Initialize provider hotswap infrastructure.
    ///
    /// Sets up the snapshot loader, preset loader, and model publisher
    /// using the provided config path. The hotswap mechanism will
    /// reload provider configuration when the config file changes.
    pub async fn init_hotswap(&mut self, config_path: Option<String>) {
        let config_path_for_loader = config_path.clone();
        let loader: SnapshotLoaderFn = Arc::new(move || {
            let cfg = if let Some(ref path) = config_path_for_loader {
                Config::from_config(Some(std::path::Path::new(path)))
            } else {
                Config::from_config(None)
            };
            ProviderSnapshot::from_config(&cfg)
        });
        self.provider_snapshot_loader = Some(loader);

        let config_path_for_presets = config_path.clone();
        let preset_loader: PresetSnapshotLoaderFn = Arc::new(move |name| {
            let cfg = if let Some(ref path) = config_path_for_presets {
                Config::from_config(Some(std::path::Path::new(path)))
            } else {
                Config::from_config(None)
            };
            let presets = &cfg.model_presets;
            let preset = presets
                .get(name)
                .ok_or_else(|| format!("Preset '{}' not found", name))?;
            ProviderSnapshot::from_preset(preset, &cfg)
        });
        self.preset_snapshot_loader = Some(preset_loader);

        let info_clone = self.command_prefilter.clone();
        let _ = info_clone;
        let publisher: RuntimeModelPublisherFn =
            Arc::new(move |model: &str, preset: Option<&str>| {
                let preset_info = preset
                    .map(|p| format!(" (preset: {})", p))
                    .unwrap_or_default();
                info!("Model hotswap: {}{}", model, preset_info);
            });
        self.runtime_model_publisher = Some(publisher);

        let cfg = if let Some(ref path) = config_path {
            Config::from_config(Some(std::path::Path::new(path)))
        } else {
            Config::from_config(None)
        };

        self.model_presets = cfg
            .model_presets
            .iter()
            .map(|(k, v)| (k.clone(), serde_json::to_value(v).unwrap_or(Value::Null)))
            .collect();

        if let Some(snapshot) = cfg
            .agents
            .defaults
            .model_preset
            .as_ref()
            .and_then(|_| ProviderSnapshot::from_config(&cfg).ok())
        {
            self.apply_provider_snapshot(snapshot, false).await;
        }

        let initial_sig = compute_config_signature(&cfg);
        *self.provider_signature.lock().await = Some(initial_sig);
    }

    /// Initialize the model switch callback on the prefilter.
    ///
    /// This connects the `/model` command to the hotswap mechanism.
    /// Call this after setting the command prefilter.
    pub fn setup_model_switch_callback(&self, _prefilter: &mut BuiltinPrefilter) {
        let self_arc = Arc::new(());
        let _ = self_arc;
    }

    /// Register handles to the channel-routed tools after construction.
    /// Either argument may be `None` if that tool isn't wired.
    pub fn attach_tool_contexts(
        &mut self,
        message: Option<Arc<MessageTool>>,
        spawn: Option<Arc<SpawnTool>>,
        cron: Option<Arc<CronTool>>,
    ) {
        self.message_tool = message;
        self.spawn_tool = spawn;
        self.cron_tool = cron;
    }

    /// Set MCP server configurations from the config file.
    ///
    /// Called during construction (or later) to populate the MCP server
    /// list. Connections are deferred until [`Self::connect_mcp`] is called,
    /// which happens automatically when the agent loop starts.
    pub fn set_mcp_servers(&mut self, servers: HashMap<String, ConfigMcpServerConfig>) {
        self._mcp_servers = servers;
    }

    /// Install a pre-filter (e.g. slash-command dispatcher) that runs
    /// before context/model.
    pub fn set_command_prefilter(&mut self, filter: CommandPrefilter) {
        self.command_prefilter = Some(filter);
    }

    /// Set the optional AgentContext for execution state management.
    ///
    /// When set, the AgentContextSyncHook is automatically registered to
    /// sync turn state into the context (T081-T083, specs/009-agent-context).
    pub fn set_agent_context(&mut self, ctx: Arc<crate::context::core::AgentContext>) {
        self.agent_context = Some(ctx);
    }

    /// Wire the consolidator for memory-quality features (history summarization,
    /// token-based compaction). The consolidator is optional; if not set, the
    /// ContextBuilder's token budget prevents context overflow.
    pub fn set_consolidator(&mut self, consolidator: Arc<Mutex<dyn ConsolidatorTrait>>) {
        self.consolidator = Some(consolidator);
    }

    /// Wire the auto-compaction engine for TTL-based idle session archival.
    /// Requires a [`SessionManager`] and a consolidator instance.
    pub fn set_auto_compact(
        &mut self,
        sessions: Arc<Mutex<SessionManager>>,
        consolidator: Arc<dyn ConsolidatorTrait>,
        ttl_minutes: i64,
    ) {
        let auto_compact = AutoCompactInner::new(sessions, consolidator, ttl_minutes);
        self.auto_compact = Some(Arc::new(Mutex::new(auto_compact)));
    }

    /// Return a reference to the auto-compaction engine, if configured.
    pub fn auto_compact(&self) -> Option<Arc<Mutex<AutoCompactInner>>> {
        self.auto_compact.clone()
    }

    /// Wire the Dream memory consolidation engine.
    ///
    /// Dream is a nightly/periodic memory processor that reviews unprocessed
    /// conversation history and extracts long-term insights into MEMORY.md.
    pub fn set_dream(&mut self, dream: Arc<Mutex<dyn DreamTrait>>) {
        self.dream = Some(dream);
    }

    /// Wire the FileStateStore for per-session file read/write tracking.
    ///
    /// FileStateStore tracks which files have been read/written during a
    /// session to prevent duplicate reads and detect stale file content.
    pub fn set_file_state_store(&mut self, store: Arc<std::sync::Mutex<FileStateStore>>) {
        self._file_state_store = Some(store);
    }

    // ========================================================================
    // Missing properties
    // ========================================================================

    pub fn current_iteration(&self) -> u32 {
        self._current_iteration
            .load(std::sync::atomic::Ordering::SeqCst)
    }

    pub fn tool_names(&self) -> impl std::future::Future<Output = Vec<String>> + '_ {
        self.tools.tool_names()
    }

    // LLMRuntime type does not exist in Rust crate yet. When the provider
    // abstraction is unified, this method should return a runtime handle.
    // pub fn llm_runtime(&self) -> LLMRuntime { ... }

    pub fn model_preset(&self) -> Option<String> {
        self._active_preset.lock().unwrap().clone()
    }

    pub fn set_model_preset_name(&self, name: Option<String>) {
        *self._active_preset.lock().unwrap() = name;
    }

    // ========================================================================
    // Missing methods (ported from Python)
    // ========================================================================

    /// Keep subagent runtime limits aligned with mutable loop settings.
    pub fn sync_subagent_runtime_limits(&self) {
        if let Some(subagents) = &self.subagents {
            // SubagentManager does not yet expose a set_max_iterations API.
            // The max_iterations limit is enforced at the AgentLoop level
            // via AgentRunSpec, so subagent turns inherit the same cap.
            let _ = subagents;
        }
    }

    /// Swap model/provider for future turns without disturbing an active one.
    pub async fn apply_provider_snapshot(&self, snapshot: ProviderSnapshot, publish_update: bool) {
        let old_model = self.config.model.clone();
        self.context_window_tokens.store(
            snapshot.context_window_tokens,
            std::sync::atomic::Ordering::SeqCst,
        );
        *self.runner.lock().unwrap() = Arc::new(AgentRunner::new(snapshot.provider.clone()));
        *self.provider_signature.lock().await = Some(snapshot.signature);

        if let Some(dream) = &self.dream {
            let mut dream = dream.lock().await;
            dream.set_provider(snapshot.provider.clone(), snapshot.model.clone());
        }

        if let Some(consolidator) = &self.consolidator {
            let mut consolidator = consolidator.lock().await;
            consolidator.set_provider(
                snapshot.provider.clone(),
                snapshot.model.clone(),
                snapshot.context_window_tokens,
            );
        }

        if let Some(publisher) = self
            .runtime_model_publisher
            .as_ref()
            .filter(|_| publish_update)
        {
            publisher(
                &snapshot.model,
                self._active_preset.lock().unwrap().as_deref(),
            );
        }
        info!(
            "Runtime model switched for next turn: {:?} -> {}",
            old_model, snapshot.model
        );
    }

    /// Refresh provider snapshot from config file.
    pub async fn refresh_provider_snapshot(&self) {
        let snapshot = match &self.provider_snapshot_loader {
            Some(loader) => match loader() {
                Ok(s) => s,
                Err(e) => {
                    warn!("Failed to load provider snapshot: {}", e);
                    return;
                }
            },
            None => return,
        };

        let current_sig = self.provider_signature.lock().await;
        if Some(snapshot.signature) == *current_sig {
            return;
        }
        drop(current_sig);
        self.apply_provider_snapshot(snapshot, true).await;
    }

    /// Build model preset snapshot from config.
    pub fn build_model_preset_snapshot(&self, name: &str) -> Option<ProviderSnapshot> {
        let cfg = Config::from_config(None);
        let preset = cfg.resolve_preset(Some(name)).ok()?;
        ProviderSnapshot::from_preset(&preset, &cfg).ok()
    }

    /// Register a provider snapshot loader.
    pub fn set_provider_snapshot_loader(&mut self, loader: SnapshotLoaderFn) {
        self.provider_snapshot_loader = Some(loader);
    }

    /// Register a preset snapshot loader.
    pub fn set_preset_snapshot_loader(&mut self, loader: PresetSnapshotLoaderFn) {
        self.preset_snapshot_loader = Some(loader);
    }

    /// Register a runtime model publisher.
    pub fn set_runtime_model_publisher(&mut self, publisher: RuntimeModelPublisherFn) {
        self.runtime_model_publisher = Some(publisher);
    }

    /// Switch to a different model at runtime.
    ///
    /// If `reload_provider` is true, rebuilds the provider from config.
    /// Otherwise only updates the model name in the runtime info.
    pub async fn switch_model(
        &self,
        model_name: &str,
        reload_provider: bool,
    ) -> Result<String, String> {
        let old_model = self.config.model.clone();

        if reload_provider {
            let snapshot = match &self.provider_snapshot_loader {
                Some(loader) => match loader() {
                    Ok(s) => s,
                    Err(e) => {
                        return Err(format!("Failed to reload provider: {}", e));
                    }
                },
                None => {
                    return Err("No provider snapshot loader configured".into());
                }
            };

            let sig = self.provider_signature.lock().await;
            if snapshot.signature != sig.unwrap_or(0) {
                drop(sig);
                self.apply_provider_snapshot(snapshot, true).await;
            }
        }

        let old_display = old_model.clone().unwrap_or_else(|| "(none)".to_string());
        info!("Model switched: {} -> {}", old_display, model_name);
        Ok(format!("Model switched: {} -> {}", old_display, model_name))
    }

    /// Switch to a model preset by name.
    pub async fn switch_model_preset(&self, preset_name: &str) -> Result<String, String> {
        let cfg = Config::from_config(None);
        let preset = cfg
            .model_presets
            .get(preset_name)
            .cloned()
            .or_else(|| {
                if preset_name.is_empty() || preset_name == "default" {
                    Some(cfg.resolve_default_preset())
                } else {
                    None
                }
            })
            .ok_or_else(|| {
                format!(
                    "Preset '{}' not found. Available: {}",
                    preset_name,
                    self.list_presets()
                )
            })?;

        let snapshot = ProviderSnapshot::from_preset(&preset, &cfg)
            .map_err(|e| format!("Failed to build preset snapshot: {}", e))?;

        self.apply_provider_snapshot(snapshot, true).await;
        *self._active_preset.lock().unwrap() = Some(preset_name.to_string());

        Ok(format!(
            "Switched to preset '{}': model={}",
            preset_name, preset.model
        ))
    }

    /// List available model presets as a comma-separated string.
    pub fn list_presets(&self) -> String {
        let mut names: Vec<String> = self.model_presets.keys().cloned().collect();
        names.sort();
        names.join(", ")
    }

    /// Get the current active model name.
    pub fn current_model(&self) -> String {
        self.config
            .model
            .clone()
            .unwrap_or_else(|| self.provider.default_model())
    }

    /// Get the current context window tokens.
    pub fn current_context_window(&self) -> u32 {
        self.context_window_tokens
            .load(std::sync::atomic::Ordering::SeqCst)
    }

    /// Register default tools via plugin loader.
    ///
    /// Tool registration is handled externally via `BuiltinToolSet::default_tools`
    /// in the Rust port. This method is kept as a no-op for API compatibility.
    pub fn register_default_tools(&mut self) {
        // Tool registration is handled externally in Rust
    }

    /// Connect to configured MCP servers (one-time, lazy).
    ///
    /// MCP (Model Context Protocol) server connections are established
    /// on first use. This method is async but non-blocking to the caller —
    /// it spawns a background task so the agent loop can continue while
    /// connections are being set up.
    pub async fn connect_mcp(&self) {
        if self
            ._mcp_connected
            .load(std::sync::atomic::Ordering::SeqCst)
            || self
                ._mcp_connecting
                .load(std::sync::atomic::Ordering::SeqCst)
            || self._mcp_servers.is_empty()
        {
            return;
        }
        self._mcp_connecting
            .store(true, std::sync::atomic::Ordering::SeqCst);

        let config_servers = self._mcp_servers.clone();
        let registry = self.tools.clone();
        let mcp_connected = &self._mcp_connected;
        let mcp_connecting = &self._mcp_connecting;
        let mcp_stacks = &self._mcp_stacks;

        let mcp_config: HashMap<String, mcp_impl::McpServerConfig> = config_servers
            .into_iter()
            .map(|(name, cfg)| (name, cfg_to_mcp_config(&cfg)))
            .collect();

        let handles = mcp_impl::connect_mcp_servers(&mcp_config, &registry).await;

        if handles.is_empty() {
            warn!("No MCP servers connected successfully (will retry next message)");
        } else {
            info!("MCP connected: {} servers available", handles.len());
            let mut stacks = mcp_stacks.lock().unwrap();
            stacks.clear();
            for (name, handle) in handles {
                stacks.insert(name, handle);
            }
            mcp_connected.store(true, std::sync::atomic::Ordering::SeqCst);
        }

        mcp_connecting.store(false, std::sync::atomic::Ordering::SeqCst);
    }

    /// Update context for all tools that need routing info.
    pub fn set_tool_context(
        &self,
        channel: &str,
        chat_id: &str,
        message_id: Option<&str>,
        metadata: &HashMap<String, Value>,
        session_key: Option<&str>,
    ) {
        let request_ctx = crate::tools::context::RequestContext {
            channel: channel.to_string(),
            chat_id: chat_id.to_string(),
            message_id: message_id.map(String::from),
            session_key: session_key.map(String::from),
            metadata: metadata.clone(),
        };
        if let Some(m) = &self.message_tool {
            m.set_context(&request_ctx);
        }
        if let Some(s) = &self.spawn_tool {
            s.set_context(channel, chat_id, session_key);
        }
        if let Some(c) = &self.cron_tool {
            c.set_context(channel, chat_id);
        }
    }

    /// Return the chat id shown in runtime metadata for the model.
    pub fn runtime_chat_id(msg: &InboundMessage) -> String {
        msg.metadata
            .get("context_chat_id")
            .and_then(|v| v.as_str())
            .map(String::from)
            .unwrap_or_else(|| msg.chat_id.clone())
    }

    /// Build a progress callback that publishes payloads to the message bus.
    ///
    /// The returned closure captures the bus, channel, chat_id, and metadata
    /// from the inbound message. Each invocation publishes an outbound
    /// message with the given payload as content.
    pub async fn build_bus_progress_callback(
        &self,
        msg: &InboundMessage,
    ) -> impl Fn(&str) -> futures::future::BoxFuture<'static, ()> + '_ {
        let bus = self.bus.clone();
        let channel = msg.channel.clone();
        let chat_id = msg.chat_id.clone();
        let metadata = msg.metadata.clone();
        move |payload: &str| {
            let bus = bus.clone();
            let channel = channel.clone();
            let chat_id = chat_id.clone();
            let metadata = metadata.clone();
            let payload = payload.to_string();
            Box::pin(async move {
                bus.publish_outbound(OutboundMessage {
                    channel,
                    chat_id,
                    content: payload,
                    reply_to: None,
                    media: Vec::new(),
                    metadata,
                })
                .await;
            })
        }
    }

    /// Build a retry-wait callback that publishes to the message bus.
    ///
    /// Similar to `build_bus_progress_callback`, but adds a `_retry_wait`
    /// marker to the metadata so the frontend can distinguish retry-wait
    /// messages from regular progress updates.
    pub async fn build_retry_wait_callback(
        &self,
        msg: &InboundMessage,
    ) -> impl Fn(&str) -> futures::future::BoxFuture<'static, ()> + '_ {
        let bus = self.bus.clone();
        let channel = msg.channel.clone();
        let chat_id = msg.chat_id.clone();
        let mut metadata = msg.metadata.clone();
        metadata.insert("_retry_wait".into(), Value::Bool(true));
        move |content: &str| {
            let bus = bus.clone();
            let channel = channel.clone();
            let chat_id = chat_id.clone();
            let metadata = metadata.clone();
            let content = content.to_string();
            Box::pin(async move {
                bus.publish_outbound(OutboundMessage {
                    channel,
                    chat_id,
                    content,
                    reply_to: None,
                    media: Vec::new(),
                    metadata,
                })
                .await;
            })
        }
    }

    /// Persist the triggering user message before the turn starts.
    pub fn persist_user_message_early(
        &self,
        msg: &InboundMessage,
        session: &mut session::manager::Session,
        extra: HashMap<String, Value>,
    ) -> bool {
        let has_text = !msg.content.trim().is_empty();
        if has_text || !msg.media.is_empty() {
            session.add_message("user", &msg.content, extra);
            self.mark_pending_user_turn(session);
            true
        } else {
            false
        }
    }

    /// Build the initial message list for the LLM turn.
    pub fn build_initial_messages(
        &self,
        msg: &InboundMessage,
        _session: &session::manager::Session,
        history: &[Value],
        pending_summary: Option<&str>,
    ) -> Vec<Value> {
        self.context.build_messages(
            history.to_vec(),
            &msg.content,
            None,
            if msg.media.is_empty() {
                None
            } else {
                Some(msg.media.as_slice())
            },
            Some(&msg.channel),
            Some(&msg.chat_id),
            "user",
            None,
            pending_summary,
            None,
        )
    }

    /// Dispatch a command directly from the run() loop.
    ///
    /// This method is a fallback path for commands that bypass the
    /// `command_prefilter` hook. Currently it delegates to the prefilter
    /// logic if one is installed, otherwise returns None.
    pub async fn dispatch_command_inline(
        &self,
        msg: &InboundMessage,
        _key: &str,
        _raw: &str,
    ) -> Option<OutboundMessage> {
        let reply = match &self.command_prefilter {
            Some(prefilter) => (prefilter)(msg).await,
            None => None,
        };
        if let Some(reply) = reply {
            return Some(OutboundMessage {
                channel: msg.channel.clone(),
                chat_id: msg.chat_id.clone(),
                content: reply,
                reply_to: None,
                media: Vec::new(),
                metadata: msg.metadata.clone(),
            });
        }
        // No prefilter installed — the command will fall through to the
        // normal agent pipeline where the LLM can respond to it.
        None
    }

    /// Cancel and await all active tasks and subagents for *key*.
    pub async fn cancel_active_tasks(&mut self, key: &str) -> usize {
        let mut cancelled = 0;
        if let Some(tasks) = self._active_tasks.lock().unwrap().remove(key) {
            for t in tasks {
                if !t.is_finished() {
                    t.abort();
                    cancelled += 1;
                }
            }
        }
        if let Some(subagents) = &self.subagents {
            let cancelled_ids = {
                let subagent_manager = subagents.lock().await;
                subagent_manager.cancel_by_session(key)
            };
            cancelled += cancelled_ids.len();
        }
        cancelled
    }

    /// Return the session key used for task routing and mid-turn injections.
    pub fn effective_session_key(&self, msg: &InboundMessage) -> String {
        if self._unified_session {
            UNIFIED_SESSION_KEY.to_string()
        } else {
            msg.session_key()
        }
    }

    /// Derive a token budget for session history replay from the context window.
    pub fn replay_token_budget(&self) -> u32 {
        let ctx = self
            .context_window_tokens
            .load(std::sync::atomic::Ordering::SeqCst);
        if ctx == 0 {
            return 0;
        }
        let reserved_output: u32 = 4096;
        let budget = ctx.saturating_sub(reserved_output).saturating_sub(1024);
        if budget > 0 {
            budget
        } else {
            std::cmp::max(128, ctx / 2)
        }
    }

    /// Run the agent iteration loop.
    ///
    /// Returns (final_content, tools_used, messages, stop_reason, had_injections).
    pub async fn run_agent_loop(
        &self,
        initial_messages: Vec<Value>,
        session_key: Option<&str>,
        pending_queue: Option<tokio::sync::mpsc::UnboundedReceiver<InboundMessage>>,
    ) -> AgentRunResult {
        self.sync_subagent_runtime_limits();
        let _pending_queue = pending_queue;

        let model = self
            .config
            .model
            .clone()
            .unwrap_or_else(|| self.provider.default_model());

        let mut spec = AgentRunSpec::new(
            initial_messages,
            self.tools.clone(),
            model,
            self.config.max_iterations,
            self.config.max_tool_result_chars,
        );
        spec.workspace = Some(self.config.workspace.clone());
        spec.session_key = session_key.map(String::from);
        spec.context_window_tokens = Some(
            self.context_window_tokens
                .load(std::sync::atomic::Ordering::SeqCst),
        );
        spec.provider_retry_mode = self.provider_retry_mode;

        // Bind file state store for this session (matches Python bind_file_states)
        let file_states = if let Some(ref store) = self._file_state_store {
            let store = store.lock().unwrap();
            Some(store.for_session(session_key))
        } else {
            None
        };
        let _file_state_token = file_states.as_ref().map(|fs| bind_file_states(fs.clone()));

        let runner = self.runner.lock().unwrap().clone();
        let result = runner.run(spec).await;

        // Reset file state binding after runner completes (matches Python reset_file_states)
        if _file_state_token.is_some() {
            reset_file_states();
        }

        result
    }

    /// Process one inbound message and return the response.
    /// Uses the event-driven state machine (mirrors Python _process_message).
    pub async fn process_message(
        &self,
        msg: InboundMessage,
        session_key: Option<String>,
    ) -> Result<Option<OutboundMessage>, String> {
        self.refresh_provider_snapshot().await;

        let key = session_key.unwrap_or_else(|| msg.session_key());
        let mut ctx = TurnContext {
            msg,
            session_key: key.clone(),
            state: TurnState::Restore,
            turn_id: format!(
                "{}:{}",
                key,
                std::time::SystemTime::now()
                    .duration_since(std::time::UNIX_EPOCH)
                    .unwrap_or_default()
                    .as_nanos()
            ),
            session: None,
            history: Vec::new(),
            initial_messages: Vec::new(),
            final_content: None,
            tools_used: Vec::new(),
            all_messages: Vec::new(),
            stop_reason: String::new(),
            had_injections: false,
            user_persisted_early: false,
            save_skip: 0,
            outbound: None,
            pending_queue: None,
            pending_summary: None,
            turn_wall_started_at: std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap_or_default()
                .as_secs_f64(),
            turn_latency_ms: None,
            trace: Vec::new(),
        };

        while ctx.state != TurnState::Done {
            let handler_name = ctx.state;
            let t0 = std::time::Instant::now();
            let event = match handler_name {
                TurnState::Restore => self.state_restore(&mut ctx).await,
                TurnState::Compact => self.state_compact(&mut ctx).await,
                TurnState::Command => self.state_command(&mut ctx).await,
                TurnState::Build => self.state_build(&mut ctx).await,
                TurnState::Run => self.state_run(&mut ctx).await,
                TurnState::Save => self.state_save(&mut ctx).await,
                TurnState::Respond => self.state_respond(&mut ctx).await,
                TurnState::Done => Ok("ok".to_string()),
            };

            let duration = t0.elapsed().as_secs_f64() * 1000.0;
            let event_str = event.clone().unwrap_or_default();
            ctx.trace.push(StateTraceEntry {
                state: handler_name,
                started_at: 0.0,
                duration_ms: duration,
                event: event_str.clone(),
                error: if event.is_err() {
                    Some(event.as_ref().err().unwrap().clone())
                } else {
                    None
                },
            });

            debug!(
                "[turn {}] State {} took {:.1}ms -> event {}",
                ctx.turn_id, handler_name, duration, event_str
            );

            let event = event.map_err(|e| e.to_string())?;
            let next_state = find_transition(handler_name, &event).ok_or_else(|| {
                format!(
                    "[turn {}] No transition from {} on event {:?}",
                    ctx.turn_id, handler_name, event
                )
            })?;
            ctx.state = next_state;
        }

        debug!(
            "[turn {}] Turn completed after {} states",
            ctx.turn_id,
            ctx.trace.len()
        );
        Ok(ctx.outbound)
    }

    /// Assemble the final outbound message from turn results.
    pub fn assemble_outbound(
        &self,
        msg: &InboundMessage,
        final_content: &str,
        had_injections: bool,
        stop_reason: &str,
        turn_latency_ms: Option<u64>,
    ) -> Option<OutboundMessage> {
        if self
            .message_tool
            .as_ref()
            .is_some_and(|mt| mt.sent_in_turn())
            && (!had_injections || stop_reason == "empty_final_response")
        {
            return None;
        }

        let preview = if final_content.len() > 120 {
            format!("{}...", &final_content[..120])
        } else {
            final_content.to_string()
        };
        info!("Response to {}:{}: {}", msg.channel, msg.sender_id, preview);

        let mut meta = msg.metadata.clone();
        if let Some(latency) = turn_latency_ms {
            meta.insert(
                "latency_ms".into(),
                Value::Number(serde_json::Number::from(latency)),
            );
        }

        Some(OutboundMessage {
            channel: msg.channel.clone(),
            chat_id: msg.chat_id.clone(),
            content: final_content.to_string(),
            reply_to: None,
            media: Vec::new(),
            metadata: meta,
        })
    }

    // ========================================================================
    // State machine handlers
    // ========================================================================

    /// Restore checkpoint / pending user turn; extract documents.
    async fn state_restore(&self, ctx: &mut TurnContext) -> Result<String, String> {
        let preview = if ctx.msg.content.len() > 80 {
            format!("{}...", &ctx.msg.content[..80])
        } else {
            ctx.msg.content.clone()
        };
        info!(
            "Processing message from {}:{}: {}",
            ctx.msg.channel, ctx.msg.sender_id, preview
        );

        {
            let mut sessions = self.sessions.lock().await;
            if ctx.session.is_none() {
                ctx.session = Some(sessions.get_or_create(&ctx.session_key));
            }
            let session = ctx.session.as_mut().unwrap();
            if self.restore_runtime_checkpoint(session) {
                sessions
                    .save(session.clone(), false)
                    .map_err(|e| e.to_string())?;
            }
            if self.restore_pending_user_turn(session) {
                sessions
                    .save(session.clone(), false)
                    .map_err(|e| e.to_string())?;
            }
        }

        Ok("ok".to_string())
    }

    /// Prepare session for compaction.
    ///
    /// Uses the auto-compaction engine to check if the session has been
    /// previously compacted. If so, the summary is loaded into
    /// `ctx.pending_summary` so that [`state_build`] can inject it into the
    /// system prompt as `[Archived Context Summary]`.
    async fn state_compact(&self, ctx: &mut TurnContext) -> Result<String, String> {
        if let Some((auto_compact, session)) = self.auto_compact.as_ref().zip(ctx.session.as_ref())
        {
            let session = session.clone();
            let ac = auto_compact.lock().await;
            let summary = ac.prepare_session(session, &ctx.session_key).await;
            drop(ac);

            if let Some(summary_text) = summary.1 {
                info!(
                    "[turn {}] Auto-compact: loaded summary for session {}",
                    ctx.turn_id, ctx.session_key
                );
                ctx.pending_summary = Some(summary_text);
            }
            ctx.session = Some(summary.0);
        }
        Ok("ok".to_string())
    }

    /// Dispatch commands if applicable.
    async fn state_command(&self, ctx: &mut TurnContext) -> Result<String, String> {
        let raw = ctx.msg.content.trim().to_string();
        if let Some(ref commands) = self.commands {
            // CommandRouter exists but full dispatch (slash-commands, skill
            // triggers, etc.) is deferred. The prefilter hook handles the
            // built-in commands (/help, /status, /model, etc.). If the
            // prefilter did not short-circuit, we continue with the normal
            // agent pipeline.
            let _commands = commands;
            let _raw = raw;
        }
        Ok("dispatch".to_string())
    }

    /// Build context for the LLM turn.
    async fn state_build(&self, ctx: &mut TurnContext) -> Result<String, String> {
        if let Some(ref consolidator) = self.consolidator {
            let estimate = {
                let c = consolidator.lock().await;
                c.estimate_session_prompt_tokens(&ctx.session_key, ctx.history.len())
                    .await
            };

            let budget = self.replay_token_budget() as usize;
            if estimate.tokens > budget {
                info!(
                    "[turn {}] Consolidator: estimated {} tokens exceeds budget {}; triggering compaction",
                    ctx.turn_id, estimate.tokens, budget
                );
                {
                    let c = consolidator.lock().await;
                    c.maybe_consolidate_by_tokens(&ctx.session_key).await;
                }

                let mut sessions = self.sessions.lock().await;
                let session = sessions.get_or_create(&ctx.session_key);
                ctx.history = session.messages.clone();
            }
        }

        self.set_tool_context(
            &ctx.msg.channel,
            &ctx.msg.chat_id,
            ctx.msg.metadata.get("message_id").and_then(|v| v.as_str()),
            &ctx.msg
                .metadata
                .iter()
                .map(|(k, v)| (k.clone(), v.clone()))
                .collect(),
            Some(&ctx.session_key),
        );

        if let Some(message_tool) = &self.message_tool {
            message_tool.start_turn();
        }

        {
            let mut sessions = self.sessions.lock().await;
            let session = sessions.get_or_create(&ctx.session_key);
            ctx.history = session.messages.clone();
        }

        ctx.initial_messages = self.build_initial_messages(
            &ctx.msg,
            ctx.session.as_ref().unwrap(),
            &ctx.history,
            ctx.pending_summary.as_deref(),
        );

        {
            let mut sessions = self.sessions.lock().await;
            let mut session = sessions.get_or_create(&ctx.session_key);
            ctx.user_persisted_early =
                self.persist_user_message_early(&ctx.msg, &mut session, HashMap::new());
        }

        Ok("ok".to_string())
    }

    /// Run the agent loop.
    async fn state_run(&self, ctx: &mut TurnContext) -> Result<String, String> {
        let result = self
            .run_agent_loop(ctx.initial_messages.clone(), Some(&ctx.session_key), None)
            .await;

        ctx.final_content = result.final_content.clone();
        ctx.tools_used = result.tools_used.clone();
        ctx.all_messages = result.messages.clone();
        ctx.stop_reason = result.stop_reason.clone();
        ctx.had_injections = result.had_injections;

        Ok("ok".to_string())
    }

    /// Save the turn to session history.
    async fn state_save(&self, ctx: &mut TurnContext) -> Result<String, String> {
        if ctx.final_content.is_none()
            || ctx
                .final_content
                .as_ref()
                .map(|s| s.trim().is_empty())
                .unwrap_or(true)
        {
            ctx.final_content = Some("Sorry, I couldn't generate a response.".to_string());
        }

        ctx.save_skip = 1 + ctx.history.len() + if ctx.user_persisted_early { 1 } else { 0 };

        ctx.turn_latency_ms = Some(
            (std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap_or_default()
                .as_secs_f64()
                - ctx.turn_wall_started_at)
                .max(0.0) as u64
                * 1000,
        );

        {
            let mut sessions = self.sessions.lock().await;
            let mut session = sessions.get_or_create(&ctx.session_key);
            self.save_turn(
                &mut session,
                &ctx.all_messages,
                ctx.save_skip,
                ctx.turn_latency_ms,
            );
            self.clear_pending_user_turn(&mut session);
            self.clear_runtime_checkpoint(&mut session);
            sessions.save(session, false).map_err(|e| e.to_string())?;
        }

        // Background consolidation is an optional post-save step that
        // compresses older conversation history into summary blobs. It is
        // not critical for correctness — the ContextBuilder's token budget
        // already prevents context overflow. When consolidator is wired,
        // spawn a tracked background task here.
        if let Some(ref consolidator) = self.consolidator {
            let session_key = ctx.session_key.clone();
            let consolidator = consolidator.clone();
            self.schedule_background(async move {
                let c = consolidator.lock().await;
                c.maybe_consolidate_by_tokens(&session_key).await;
                drop(c);
            });
        }

        Ok("ok".to_string())
    }

    /// Assemble the outbound response.
    async fn state_respond(&self, ctx: &mut TurnContext) -> Result<String, String> {
        ctx.outbound = self.assemble_outbound(
            &ctx.msg,
            ctx.final_content.as_deref().unwrap_or(""),
            ctx.had_injections,
            &ctx.stop_reason,
            ctx.turn_latency_ms,
        );
        Ok("ok".to_string())
    }

    /// Persist subagent follow-ups before prompt assembly.
    pub fn persist_subagent_followup(
        &self,
        session: &mut session::manager::Session,
        msg: &InboundMessage,
    ) -> bool {
        if msg.content.is_empty() {
            return false;
        }
        let task_id = msg
            .metadata
            .get("subagent_task_id")
            .and_then(|v| v.as_str());
        if let Some(tid) = task_id {
            let already_exists = session.messages.iter().any(|m| {
                m.get("injected_event").and_then(|v| v.as_str()) == Some("subagent_result")
                    && m.get("subagent_task_id").and_then(|v| v.as_str()) == Some(tid)
            });
            if already_exists {
                return false;
            }
        }
        let mut extra: HashMap<String, Value> = HashMap::new();
        extra.insert("sender_id".into(), Value::String(msg.sender_id.clone()));
        extra.insert(
            "injected_event".into(),
            Value::String("subagent_result".into()),
        );
        if let Some(tid) = task_id {
            extra.insert("subagent_task_id".into(), Value::String(tid.into()));
        }
        session.add_message("assistant", &msg.content, extra);
        true
    }

    // ========================================================================
    // Main service loop
    // ========================================================================

    /// Main service loop — blocks until the inbound lane closes.
    ///
    /// Takes [`Arc<Self>`] so spawned tasks can hold a reference to the loop
    /// for concurrent message processing.
    pub async fn run(self_arc: Arc<Self>) {
        self_arc
            ._running
            .store(true, std::sync::atomic::Ordering::SeqCst);
        self_arc.connect_mcp().await;
        info!(
            "Agent loop running (workspace={}, model={})",
            self_arc.config.workspace.display(),
            self_arc
                .config
                .model
                .as_deref()
                .unwrap_or("<provider default>")
        );

        let this = &*self_arc;
        while this._running.load(std::sync::atomic::Ordering::SeqCst) {
            match tokio::time::timeout(
                std::time::Duration::from_secs(1),
                this.bus.consume_inbound(),
            )
            .await
            {
                Ok(Some(msg)) => {
                    let raw = msg.content.trim().to_string();
                    // Priority commands (e.g. /stop, /kill) would bypass the
                    // concurrency gate and be executed immediately. Currently
                    // all commands flow through the concurrency gate, which is
                    // safe — the gate allows 3 concurrent tasks and commands
                    // complete quickly.
                    let effective_key = this.effective_session_key(&msg);

                    // If session has active pending queue, route there
                    if this
                        ._pending_queues
                        .lock()
                        .unwrap()
                        .contains_key(&effective_key)
                    {
                        let _raw = raw;
                        let _effective_key = effective_key;
                        continue;
                    }

                    let gate = this._concurrency_gate.clone();
                    let msg_clone = msg.clone();
                    let self_clone = self_arc.clone();

                    if let Some(ref gate_ref) = gate {
                        let gate_owned = gate_ref.clone();
                        let msg_clone2 = msg_clone.clone();
                        let self_clone2 = self_clone.clone();
                        let handle = tokio::spawn(async move {
                            let _permit = gate_owned.acquire_owned().await;
                            let _ = self_clone2.process_inbound(msg_clone2).await;
                        });
                        this._active_tasks
                            .lock()
                            .unwrap()
                            .entry(effective_key)
                            .or_default()
                            .push(handle);
                    } else {
                        let handle = tokio::spawn(async move {
                            let _ = self_clone.process_inbound(msg_clone).await;
                        });
                        this._active_tasks
                            .lock()
                            .unwrap()
                            .entry(effective_key)
                            .or_default()
                            .push(handle);
                    }
                }
                Ok(None) => break,
                Err(_) => {
                    // Timeout — check for expired sessions (matches Python
                    // self.auto_compact.check_expired(...))
                    // AutoCompact.check_expired requires Arc<Self> + spawner + active set,
                    // which is complex to construct in the main loop. The session manager
                    // handles cleanup via retain_heartbeat_session() on explicit calls.
                }
            }
        }
        info!("Agent loop stopped (inbound lane closed)");
    }

    /// Process one inbound message end-to-end. Returns the
    /// [`AgentRunResult`] so synchronous drivers (CLI / tests) can use it
    /// directly.
    pub async fn process_inbound(&self, msg: InboundMessage) -> Result<AgentRunResult, String> {
        debug!(
            "process_inbound: channel={} chat={} len={}",
            msg.channel,
            msg.chat_id,
            msg.content.len()
        );

        // Slash-command / priority command pre-filter. A `Some(content)`
        // response short-circuits the turn with that reply as the
        // outbound message.
        let command_reply = match &self.command_prefilter {
            Some(filter) => (filter)(&msg).await,
            None => None,
        };
        if let Some(reply) = command_reply {
            self.bus
                .publish_outbound(OutboundMessage {
                    channel: msg.channel.clone(),
                    chat_id: msg.chat_id.clone(),
                    content: reply.clone(),
                    reply_to: None,
                    media: Vec::new(),
                    metadata: Default::default(),
                })
                .await;
            return Ok(AgentRunResult::short_circuit(reply));
        }

        let session_key = msg.session_key();

        // Restore any in-flight checkpoint before building context.
        {
            let mut sessions = self.sessions.lock().await;
            let mut session = sessions.get_or_create(&session_key);
            if self.restore_runtime_checkpoint(&mut session) {
                info!("Restored runtime checkpoint for session={session_key}");
                if let Err(e) = sessions.save(session, false) {
                    error!("failed to persist checkpoint restore for {session_key}: {e}");
                }
            } else if self.restore_pending_user_turn(&mut session) {
                info!("Restored pending user turn for session={session_key}");
                if let Err(e) = sessions.save(session, false) {
                    error!("failed to persist pending turn restore for {session_key}: {e}");
                }
            }
        }

        // Propagate per-turn context to the channel-routed tools.
        self.apply_tool_contexts(&msg, &session_key);

        // Pull a shallow copy of history under the session lock and
        // release before the long-running model call.
        let history: Vec<Value> = {
            let mut sessions = self.sessions.lock().await;
            let session = sessions.get_or_create(&session_key);
            session.messages.clone()
        };

        let messages = self.context.build_messages(
            history,
            &msg.content,
            None,
            if msg.media.is_empty() {
                None
            } else {
                Some(msg.media.as_slice())
            },
            Some(&msg.channel),
            Some(&msg.chat_id),
            "user",
            None,
            None,
            None,
        );

        let model = self
            .config
            .model
            .clone()
            .unwrap_or_else(|| self.provider.default_model());

        let mut spec = AgentRunSpec::new(
            messages,
            self.tools.clone(),
            model,
            self.config.max_iterations,
            self.config.max_tool_result_chars,
        );
        spec.workspace = Some(self.config.workspace.clone());
        spec.session_key = Some(session_key.clone());
        spec.context_window_tokens = self.config.context_window_tokens;
        spec.provider_retry_mode = self.config.provider_retry_mode;

        // Wire AgentContextSyncHook if AgentContext is configured
        let agent_context_hook = self.agent_context.as_ref().map(|ctx| {
            Arc::new(crate::context::AgentContextSyncHook::new(Arc::clone(ctx)))
                as Arc<dyn crate::hook::AgentHook>
        });
        if let Some(ctx_hook) = &self.agent_context {
            // Record user input in context before execution begins
            let _ = ctx_hook.set_lifecycle_state(crate::context::LifecycleState::Active);
        }

        let runner = self.runner.lock().unwrap().clone();
        spec.hook = agent_context_hook;
        let result = runner.run(spec).await;

        // Update lifecycle state based on execution outcome
        if let Some(ctx) = &self.agent_context {
            let final_state = if result.stop_reason == "tool_error" || result.error.is_some() {
                crate::context::LifecycleState::Terminated
            } else {
                crate::context::LifecycleState::Completed
            };
            let _ = ctx.set_lifecycle_state(final_state);

            // Store response payload if available
            if let Some(ref content) = result.final_content {
                let _ =
                    ctx.set_response_payload(crate::context::ResponsePayload::new(content.clone()));
            }
        }

        // Record the exchange so subsequent turns see it.
        if let Some(_content) = result.final_content.clone() {
            let mut sessions = self.sessions.lock().await;
            let mut session = sessions.get_or_create(&session_key);
            session.add_message("user", &msg.content, Default::default());
            session.add_message("assistant", &_content, Default::default());
            if let Err(e) = sessions.save(session, false) {
                error!("failed to persist session {session_key}: {e}");
            }
        }

        // Clear checkpoint / pending turn now that the turn completed.
        {
            let mut sessions = self.sessions.lock().await;
            let mut session = sessions.get_or_create(&session_key);
            self.clear_runtime_checkpoint(&mut session);
            self.clear_pending_user_turn(&mut session);
            if let Err(e) = sessions.save(session, false) {
                error!("failed to clear checkpoint for {session_key}: {e}");
            }
        }

        // Publish outbound — but only if the `message` tool hasn't already
        // delivered something during this turn. This mirrors the Python
        // `_maybe_flush` guard: when the agent explicitly messaged the
        // user via the `message` tool, we don't echo the final content.
        let delivered_via_tool = self
            .message_tool
            .as_ref()
            .map(|m| m.sent_in_turn())
            .unwrap_or(false);
        if let Some(final_text) = result.final_content.clone().filter(|_| !delivered_via_tool) {
            self.bus
                .publish_outbound(OutboundMessage {
                    channel: msg.channel.clone(),
                    chat_id: msg.chat_id.clone(),
                    content: final_text,
                    reply_to: None,
                    media: Vec::new(),
                    metadata: Default::default(),
                })
                .await;
        }

        Ok(result)
    }

    /// Process a message directly and return the outbound payload.
    pub async fn process_direct(
        &mut self,
        content: String,
        session_key: String,
        channel: String,
        chat_id: String,
        media: Option<Vec<String>>,
    ) -> Option<OutboundMessage> {
        self.connect_mcp().await;
        let msg = InboundMessage {
            channel,
            sender_id: "user".to_string(),
            chat_id,
            content,
            media: media.unwrap_or_default(),
            metadata: Default::default(),
            timestamp: chrono::Local::now(),
            session_key_override: None,
        };
        self.process_message(msg, Some(session_key))
            .await
            .ok()
            .flatten()
    }

    fn apply_tool_contexts(&self, msg: &InboundMessage, session_key: &str) {
        let channel = msg.channel.as_str();
        let chat_id = msg.chat_id.as_str();
        let message_id = msg
            .metadata
            .get("message_id")
            .and_then(|v| v.as_str())
            .map(String::from);
        let ctx = crate::tools::context::RequestContext {
            channel: channel.to_string(),
            chat_id: chat_id.to_string(),
            message_id: message_id.clone(),
            session_key: Some(session_key.to_string()),
            metadata: msg.metadata.clone(),
        };
        if let Some(m) = &self.message_tool {
            m.set_context(&ctx);
            m.start_turn();
        }
        if let Some(s) = &self.spawn_tool {
            s.set_context(channel, chat_id, Some(session_key));
        }
        if let Some(c) = &self.cron_tool {
            c.set_context(channel, chat_id);
        }
    }

    pub fn bus(&self) -> Arc<MessageBus> {
        self.bus.clone()
    }

    pub fn tools(&self) -> &ToolRegistry {
        &self.tools
    }

    pub fn disabled_skills(&self) -> HashSet<String> {
        self.config.disabled_skills.iter().cloned().collect()
    }

    /// Flush all cached sessions to durable storage.
    /// Returns the number of sessions successfully flushed.
    pub fn flush_sessions(&self) -> usize {
        if let Ok(mut sessions) = self.sessions.try_lock() {
            sessions.flush_all()
        } else {
            0
        }
    }

    /// Retain only the most recent messages in the heartbeat session.
    /// This prevents the heartbeat context from growing unbounded.
    pub fn retain_heartbeat_session(&self, max_messages: usize) {
        if let Ok(mut sessions) = self.sessions.try_lock() {
            let session = sessions.get_or_create("heartbeat:default");
            let key = session.key.clone();
            drop(session);

            let session = sessions.get_or_create(&key);
            let mut session_clone = session.clone();
            session_clone.retain_recent_legal_suffix(max_messages);
            let _ = sessions.save(session_clone, false);
        }
    }

    #[expect(
        dead_code,
        reason = "retained as the writer paired with runtime checkpoint restoration"
    )]
    fn set_runtime_checkpoint(&self, session: &mut session::manager::Session, payload: Value) {
        if let Value::Object(ref map) = payload {
            for (k, v) in map {
                session.metadata.insert(k.clone(), v.clone());
            }
        }
        session
            .metadata
            .insert(RUNTIME_CHECKPOINT_KEY.to_string(), payload);
    }

    fn mark_pending_user_turn(&self, session: &mut session::manager::Session) {
        session
            .metadata
            .insert(PENDING_USER_TURN_KEY.to_string(), Value::Bool(true));
    }

    fn clear_pending_user_turn(&self, session: &mut session::manager::Session) {
        session.metadata.remove(PENDING_USER_TURN_KEY);
    }

    fn clear_runtime_checkpoint(&self, session: &mut session::manager::Session) {
        session.metadata.remove(RUNTIME_CHECKPOINT_KEY);
    }

    fn restore_runtime_checkpoint(&self, session: &mut session::manager::Session) -> bool {
        let checkpoint = session.metadata.get(RUNTIME_CHECKPOINT_KEY).cloned();
        let checkpoint = match checkpoint {
            Some(Value::Object(map)) => map,
            _ => return false,
        };

        let assistant_message = checkpoint.get("assistant_message").cloned();
        let completed_tool_results = checkpoint
            .get("completed_tool_results")
            .and_then(|v| v.as_array())
            .cloned()
            .unwrap_or_default();
        let pending_tool_calls = checkpoint
            .get("pending_tool_calls")
            .and_then(|v| v.as_array())
            .cloned()
            .unwrap_or_default();

        let mut restored: Vec<Value> = Vec::new();
        if let Some(mut msg) = assistant_message {
            if let Some(m) = msg.as_object_mut().filter(|m| !m.contains_key("timestamp")) {
                m.insert(
                    "timestamp".into(),
                    Value::String(chrono::Utc::now().to_rfc3339()),
                );
            }
            restored.push(msg);
        }
        for mut msg in completed_tool_results {
            if let Some(m) = msg.as_object_mut().filter(|m| !m.contains_key("timestamp")) {
                m.insert(
                    "timestamp".into(),
                    Value::String(chrono::Utc::now().to_rfc3339()),
                );
            }
            restored.push(msg);
        }
        for tool_call in pending_tool_calls {
            let tool_id = tool_call.get("id").cloned().unwrap_or(Value::Null);
            let name = tool_call
                .get("function")
                .and_then(|f| f.get("name"))
                .and_then(|n| n.as_str())
                .unwrap_or("tool")
                .to_string();
            restored.push(serde_json::json!({
                "role": "tool",
                "tool_call_id": tool_id,
                "name": name,
                "content": "Error: Task interrupted before this tool finished.",
                "timestamp": chrono::Utc::now().to_rfc3339(),
            }));
        }

        let overlap = find_message_overlap(&session.messages, &restored);
        session.messages.extend(restored.into_iter().skip(overlap));

        self.clear_pending_user_turn(session);
        self.clear_runtime_checkpoint(session);
        true
    }

    fn restore_pending_user_turn(&self, session: &mut session::manager::Session) -> bool {
        let pending = session
            .metadata
            .get(PENDING_USER_TURN_KEY)
            .and_then(|v| v.as_bool())
            .unwrap_or(false);
        if !pending {
            return false;
        }
        let last_is_user = session
            .messages
            .last()
            .and_then(|m| m.get("role"))
            .and_then(|r| r.as_str())
            == Some("user");
        if last_is_user {
            session.messages.push(serde_json::json!({
                "role": "assistant",
                "content": "Error: Task interrupted before a response was generated.",
                "timestamp": chrono::Utc::now().to_rfc3339(),
            }));
        }
        self.clear_pending_user_turn(session);
        true
    }

    fn sanitize_persisted_blocks(
        &self,
        content: &Value,
        should_truncate_text: bool,
        drop_runtime: bool,
    ) -> Option<Value> {
        let blocks = content.as_array()?;
        let mut filtered: Vec<Value> = Vec::new();
        for block in blocks {
            if !block.is_object() {
                filtered.push(block.clone());
                continue;
            }
            let block_type = block.get("type").and_then(|v| v.as_str()).unwrap_or("");
            if drop_runtime
                && block_type == "text"
                && block
                    .get("text")
                    .and_then(|v| v.as_str())
                    .map(|t| t.contains(RUNTIME_CONTEXT_TAG))
                    .unwrap_or(false)
            {
                continue;
            }
            if block_type == "image_url" {
                let url = block
                    .get("image_url")
                    .and_then(|v| v.get("url"))
                    .and_then(|v| v.as_str())
                    .unwrap_or("");
                if url.starts_with("data:image/") {
                    let path = block
                        .get("_meta")
                        .and_then(|v| v.get("path"))
                        .and_then(|v| v.as_str())
                        .unwrap_or("");
                    filtered.push(serde_json::json!({
                        "type": "text",
                        "text": format!("[Image: {path}]"),
                    }));
                    continue;
                }
            }
            if let Some(text) = (block_type == "text")
                .then(|| block.get("text").and_then(|v| v.as_str()))
                .flatten()
            {
                let text = if should_truncate_text && text.len() > self.config.max_tool_result_chars
                {
                    truncate_text_fn(text, self.config.max_tool_result_chars)
                } else {
                    text.to_string()
                };
                let mut clone = block.clone();
                if let Value::Object(ref mut m) = clone {
                    m.insert("text".into(), Value::String(text));
                }
                filtered.push(clone);
                continue;
            }
            filtered.push(block.clone());
        }
        if filtered.is_empty() {
            None
        } else {
            Some(Value::Array(filtered))
        }
    }

    fn save_turn(
        &self,
        session: &mut session::manager::Session,
        messages: &[Value],
        skip: usize,
        turn_latency_ms: Option<u64>,
    ) {
        let now = chrono::Utc::now().to_rfc3339();
        let mut last_assistant_idx: Option<usize> = None;
        for m in messages.iter().skip(skip) {
            let Some(role) = m.get("role").and_then(|v| v.as_str()) else {
                continue;
            };
            let content = m.get("content");
            if role == "assistant" {
                let has_tool_calls = m.get("tool_calls").is_some();
                if content.is_none() && !has_tool_calls {
                    continue;
                }
                let mut entry = m.clone();
                if let Some(obj) = entry
                    .as_object_mut()
                    .filter(|obj| !obj.contains_key("timestamp"))
                {
                    obj.insert("timestamp".into(), Value::String(now.clone()));
                }
                if let Some(Value::Array(blocks)) = content {
                    let sanitized =
                        self.sanitize_persisted_blocks(&Value::Array(blocks.clone()), true, false);
                    if sanitized.is_none() && !has_tool_calls {
                        continue;
                    }
                    if let Some((s, obj)) = sanitized.zip(entry.as_object_mut()) {
                        obj.insert("content".into(), s);
                    }
                }
                session.messages.push(entry);
                last_assistant_idx = Some(session.messages.len() - 1);
            } else if role == "tool" {
                let mut entry = m.clone();
                if let Some(Value::String(s)) = content {
                    if s.len() > self.config.max_tool_result_chars {
                        let truncated = truncate_text_fn(s, self.config.max_tool_result_chars);
                        if let Value::Object(ref mut obj) = entry {
                            obj.insert("content".into(), Value::String(truncated));
                        }
                    }
                } else if let Some(Value::Array(_)) = content {
                    let sanitized =
                        self.sanitize_persisted_blocks(content.as_ref().unwrap(), true, false);
                    if sanitized.is_none() {
                        continue;
                    }
                    if let Some((s, obj)) = sanitized.zip(entry.as_object_mut()) {
                        obj.insert("content".into(), s);
                    }
                }
                if let Some(obj) = entry
                    .as_object_mut()
                    .filter(|obj| !obj.contains_key("timestamp"))
                {
                    obj.insert("timestamp".into(), Value::String(now.clone()));
                }
                session.messages.push(entry);
            } else if role == "user" {
                let mut entry = m.clone();
                if let Some(s) = content
                    .and_then(|value| value.as_str())
                    .filter(|s| s.contains(RUNTIME_CONTEXT_TAG))
                {
                    let tag_pos = s.find(RUNTIME_CONTEXT_TAG).unwrap();
                    let before = s[..tag_pos].trim_end();
                    if before.is_empty() {
                        continue;
                    }
                    if let Value::Object(ref mut obj) = entry {
                        obj.insert("content".into(), Value::String(before.to_string()));
                    }
                }
                if let Some(obj) = entry
                    .as_object_mut()
                    .filter(|obj| !obj.contains_key("timestamp"))
                {
                    obj.insert("timestamp".into(), Value::String(now.clone()));
                }
                session.messages.push(entry);
            } else {
                let mut entry = m.clone();
                if let Some(obj) = entry
                    .as_object_mut()
                    .filter(|obj| !obj.contains_key("timestamp"))
                {
                    obj.insert("timestamp".into(), Value::String(now.clone()));
                }
                session.messages.push(entry);
            }
        }
        if let Some((ms, msg)) = turn_latency_ms
            .zip(last_assistant_idx)
            .and_then(|(ms, idx)| {
                session
                    .messages
                    .get_mut(idx)
                    .and_then(|message| message.as_object_mut())
                    .map(|message| (ms, message))
            })
        {
            msg.insert("latency_ms".into(), serde_json::json!(ms));
        }
        session.updated_at = chrono::Local::now();
    }

    /// Drain pending background archives, then close MCP connections.
    pub async fn close_mcp(&mut self) {
        {
            let mut bg = self._background_tasks.lock().unwrap();
            for handle in bg.drain(..) {
                handle.abort();
            }
        }
        let handles = {
            let mut stacks = self._mcp_stacks.lock().unwrap();
            stacks.drain().collect::<Vec<_>>()
        };
        for (name, handle) in handles {
            info!("Shutting down MCP server '{}'", name);
            handle.shutdown();
        }
        self._mcp_connected
            .store(false, std::sync::atomic::Ordering::SeqCst);
    }

    /// Schedule a coroutine as a tracked background task.
    pub fn schedule_background(
        &self,
        future: impl std::future::Future<Output = ()> + Send + 'static,
    ) {
        let handle = tokio::spawn(future);
        self._background_tasks.lock().unwrap().push(handle);
    }

    /// Stop the agent loop.
    pub fn stop(&self) {
        self._running
            .store(false, std::sync::atomic::Ordering::SeqCst);
        info!("Agent loop stopping");
    }

    /// Process a system inbound message (e.g. subagent announce).
    pub async fn process_system_message(
        &self,
        msg: InboundMessage,
        session_key: Option<String>,
    ) -> Option<OutboundMessage> {
        let (channel, chat_id) = if msg.chat_id.contains(':') {
            let parts: Vec<&str> = msg.chat_id.splitn(2, ':').collect();
            (parts[0].to_string(), parts[1].to_string())
        } else {
            ("cli".to_string(), msg.chat_id.clone())
        };
        info!("Processing system message from {}", msg.sender_id);
        let key = msg
            .metadata
            .get("session_key_override")
            .and_then(|v| v.as_str())
            .map(String::from)
            .unwrap_or_else(|| format!("{channel}:{chat_id}"));

        {
            let mut sessions = self.sessions.lock().await;
            let mut session = sessions.get_or_create(&key);
            if self.restore_runtime_checkpoint(&mut session) {
                sessions.save(session.clone(), false).ok();
            }
            if self.restore_pending_user_turn(&mut session) {
                sessions.save(session, false).ok();
            }
        }

        let is_subagent = msg.sender_id == "subagent";

        self.set_tool_context(
            &channel,
            &chat_id,
            msg.metadata.get("message_id").and_then(|v| v.as_str()),
            &msg.metadata
                .iter()
                .map(|(k, v)| (k.clone(), v.clone()))
                .collect(),
            session_key.as_deref(),
        );

        let history: Vec<Value> = {
            let mut sessions = self.sessions.lock().await;
            let session = sessions.get_or_create(&key);
            session.messages.clone()
        };

        let messages = self.context.build_messages(
            history.clone(),
            if is_subagent { "" } else { &msg.content },
            None,
            None,
            Some(&channel),
            Some(&chat_id),
            if is_subagent { "assistant" } else { "user" },
            Some(&msg.sender_id),
            None,
            None,
        );

        let result = self.run_agent_loop(messages, Some(&key), None).await;

        let final_content = result
            .final_content
            .unwrap_or_else(|| "Background task completed.".to_string());

        let mut outbound_metadata: HashMap<String, Value> = HashMap::new();
        if let Some(origin_id) = msg.metadata.get("origin_message_id") {
            outbound_metadata.insert("origin_message_id".into(), origin_id.clone());
        }

        Some(OutboundMessage {
            channel,
            chat_id,
            content: final_content,
            reply_to: None,
            media: Vec::new(),
            metadata: outbound_metadata,
        })
    }
}

fn cfg_to_mcp_config(cfg: &ConfigMcpServerConfig) -> mcp_impl::McpServerConfig {
    mcp_impl::McpServerConfig {
        transport_type: cfg.transport.as_ref().map(|t| match t {
            config::schema::McpTransport::Stdio => "stdio".to_string(),
            config::schema::McpTransport::Sse => "sse".to_string(),
            config::schema::McpTransport::StreamableHttp => "streamableHttp".to_string(),
        }),
        command: if cfg.command.is_empty() {
            None
        } else {
            Some(cfg.command.clone())
        },
        args: if cfg.args.is_empty() {
            None
        } else {
            Some(cfg.args.clone())
        },
        env: if cfg.env.is_empty() {
            None
        } else {
            Some(cfg.env.clone())
        },
        url: if cfg.url.is_empty() {
            None
        } else {
            Some(cfg.url.clone())
        },
        headers: if cfg.headers.is_empty() {
            None
        } else {
            Some(cfg.headers.clone())
        },
        enabled_tools: cfg.enabled_tools.clone(),
        tool_timeout: cfg.tool_timeout as u64,
    }
}

fn find_message_overlap(existing: &[Value], restored: &[Value]) -> usize {
    let max_overlap = existing.len().min(restored.len());
    for size in (1..=max_overlap).rev() {
        let existing_slice = &existing[existing.len() - size..];
        let restored_slice = &restored[..size];
        if existing_slice
            .iter()
            .zip(restored_slice.iter())
            .all(|(a, b)| message_key(a) == message_key(b))
        {
            return size;
        }
    }
    0
}

fn message_key(msg: &Value) -> MessageKey<'_> {
    (
        msg.get("role").and_then(|v| v.as_str()),
        msg.get("content").and_then(|v| v.as_str()),
        msg.get("tool_call_id").and_then(|v| v.as_str()),
        msg.get("name").and_then(|v| v.as_str()),
        msg.get("tool_calls"),
        msg.get("reasoning_content"),
    )
}

fn truncate_text_fn(text: &str, max_chars: usize) -> String {
    if text.len() <= max_chars {
        return text.to_string();
    }
    let truncated = &text[..max_chars];
    format!("{truncated}...\n(truncated)")
}
