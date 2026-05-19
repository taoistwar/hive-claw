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

use std::collections::HashSet;
use std::path::PathBuf;
use std::sync::Arc;

use log::{debug, error, info};
use serde_json::Value;
use tokio::sync::Mutex;

use bus::{InboundMessage, MessageBus, OutboundMessage};
use config::schema::{Config, ProviderRetryMode};
use providers::{LLMProvider, RetryMode};
use session::manager::SessionManager;

use crate::context::ContextBuilder;
use crate::runner::{AgentRunResult, AgentRunSpec, AgentRunner};
use crate::tools::{BuiltinToolSet, CronTool, MessageTool, SpawnTool, ToolRegistry};

/// Dependencies + tunables for [`AgentLoop`].
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

/// Core agent orchestration — inbound -> runner -> outbound.
pub struct AgentLoop {
    bus: Arc<MessageBus>,
    provider: Arc<dyn LLMProvider>,
    runner: AgentRunner,
    context: ContextBuilder,
    sessions: Arc<Mutex<SessionManager>>,
    tools: ToolRegistry,
    // Tools that need per-turn context propagation.
    message_tool: Option<Arc<MessageTool>>,
    spawn_tool: Option<Arc<SpawnTool>>,
    cron_tool: Option<Arc<CronTool>>,
    command_prefilter: Option<CommandPrefilter>,
    config: LoopConfig,
}

impl AgentLoop {
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
        let context = ContextBuilder::new(
            config.workspace.clone(),
            config.timezone.clone(),
            Some(config.disabled_skills.clone()),
        );
        let runner = AgentRunner::new(provider.clone());
        Self {
            bus,
            provider,
            runner,
            context,
            sessions,
            tools: builtin.registry,
            message_tool: Some(builtin.message),
            spawn_tool: builtin.spawn,
            cron_tool: builtin.cron,
            command_prefilter: None,
            config,
        }
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
        let context = ContextBuilder::new(
            config.workspace.clone(),
            config.timezone.clone(),
            Some(config.disabled_skills.clone()),
        );
        let runner = AgentRunner::new(provider.clone());
        Self {
            bus,
            provider,
            runner,
            context,
            sessions,
            tools,
            message_tool: None,
            spawn_tool: None,
            cron_tool: None,
            command_prefilter: None,
            config,
        }
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

    /// Install a pre-filter (e.g. slash-command dispatcher) that runs
    /// before context/model.
    pub fn set_command_prefilter(&mut self, filter: CommandPrefilter) {
        self.command_prefilter = Some(filter);
    }

    /// Main service loop — blocks until the inbound lane closes.
    pub async fn run(&self) {
        info!(
            "Agent loop running (workspace={}, model={})",
            self.config.workspace.display(),
            self.config.model.as_deref().unwrap_or("<provider default>")
        );
        while let Some(msg) = self.bus.consume_inbound().await {
            if let Err(e) = self.process_inbound(msg).await {
                error!("Agent loop: error processing inbound: {e}");
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
        if let Some(filter) = &self.command_prefilter {
            if let Some(reply) = (filter)(&msg).await {
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
        }

        let session_key = msg.session_key();

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

        let result = self.runner.run(spec).await;

        // Record the exchange so subsequent turns see it.
        if let Some(content) = result.final_content.clone() {
            let mut sessions = self.sessions.lock().await;
            let mut session = sessions.get_or_create(&session_key);
            session.add_message("user", &msg.content, Default::default());
            session.add_message("assistant", &content, Default::default());
            if let Err(e) = sessions.save(session, false) {
                error!("failed to persist session {session_key}: {e}");
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
        if !delivered_via_tool {
            if let Some(final_text) = result.final_content.clone() {
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
        }

        Ok(result)
    }

    fn apply_tool_contexts(&self, msg: &InboundMessage, session_key: &str) {
        let channel = msg.channel.as_str();
        let chat_id = msg.chat_id.as_str();
        let message_id = msg
            .metadata
            .get("message_id")
            .and_then(|v| v.as_str())
            .map(String::from);
        if let Some(m) = &self.message_tool {
            m.set_context(channel, chat_id, message_id);
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
}
