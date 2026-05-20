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

use crate::context::{ContextBuilder, RUNTIME_CONTEXT_TAG};
use crate::runner::{AgentRunResult, AgentRunSpec, AgentRunner};
use crate::tools::{BuiltinToolSet, CronTool, MessageTool, SpawnTool, ToolRegistry};

const RUNTIME_CHECKPOINT_KEY: &str = "runtime_checkpoint";
const PENDING_USER_TURN_KEY: &str = "pending_user_turn";

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

        let result = self.runner.run(spec).await;

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

    fn set_runtime_checkpoint(&self, session: &mut session::manager::Session, payload: Value) {
        if let Value::Object(ref map) = payload {
            for (k, v) in map {
                session.metadata.insert(k.clone(), v.clone());
            }
        }
        session.metadata.insert(
            RUNTIME_CHECKPOINT_KEY.to_string(),
            payload,
        );
    }

    fn mark_pending_user_turn(&self, session: &mut session::manager::Session) {
        session.metadata.insert(
            PENDING_USER_TURN_KEY.to_string(),
            Value::Bool(true),
        );
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
            if let Value::Object(ref mut m) = msg {
                if !m.contains_key("timestamp") {
                    m.insert(
                        "timestamp".into(),
                        Value::String(chrono::Utc::now().to_rfc3339()),
                    );
                }
            }
            restored.push(msg);
        }
        for mut msg in completed_tool_results {
            if let Value::Object(ref mut m) = msg {
                if !m.contains_key("timestamp") {
                    m.insert(
                        "timestamp".into(),
                        Value::String(chrono::Utc::now().to_rfc3339()),
                    );
                }
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
        let blocks = match content.as_array() {
            Some(arr) => arr,
            None => return None,
        };
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
            if block_type == "text" {
                if let Some(text) = block.get("text").and_then(|v| v.as_str()) {
                    let text = if should_truncate_text
                        && text.len() > self.config.max_tool_result_chars
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
                if let Value::Object(ref mut obj) = entry {
                    if !obj.contains_key("timestamp") {
                        obj.insert("timestamp".into(), Value::String(now.clone()));
                    }
                }
                if let Some(Value::Array(blocks)) = content {
                    let sanitized = self.sanitize_persisted_blocks(
                        &Value::Array(blocks.clone()),
                        true,
                        false,
                    );
                    if sanitized.is_none() && !has_tool_calls {
                        continue;
                    }
                    if let Some(s) = sanitized {
                        if let Value::Object(ref mut obj) = entry {
                            obj.insert("content".into(), s);
                        }
                    }
                }
                session.messages.push(entry);
                last_assistant_idx = Some(session.messages.len() - 1);
            } else if role == "tool" {
                let mut entry = m.clone();
                if let Some(Value::String(s)) = content {
                    if s.len() > self.config.max_tool_result_chars {
                        let truncated =
                            truncate_text_fn(s, self.config.max_tool_result_chars);
                        if let Value::Object(ref mut obj) = entry {
                            obj.insert("content".into(), Value::String(truncated));
                        }
                    }
                } else if let Some(Value::Array(_)) = content {
                    let sanitized = self.sanitize_persisted_blocks(
                        content.as_ref().unwrap(),
                        true,
                        false,
                    );
                    if sanitized.is_none() {
                        continue;
                    }
                    if let Some(s) = sanitized {
                        if let Value::Object(ref mut obj) = entry {
                            obj.insert("content".into(), s);
                        }
                    }
                }
                if let Value::Object(ref mut obj) = entry {
                    if !obj.contains_key("timestamp") {
                        obj.insert("timestamp".into(), Value::String(now.clone()));
                    }
                }
                session.messages.push(entry);
            } else if role == "user" {
                let mut entry = m.clone();
                if let Some(Value::String(s)) = content {
                    if s.contains(RUNTIME_CONTEXT_TAG) {
                        let tag_pos = s.find(RUNTIME_CONTEXT_TAG).unwrap();
                        let before = s[..tag_pos].trim_end();
                        if before.is_empty() {
                            continue;
                        }
                        if let Value::Object(ref mut obj) = entry {
                            obj.insert("content".into(), Value::String(before.to_string()));
                        }
                    }
                }
                if let Value::Object(ref mut obj) = entry {
                    if !obj.contains_key("timestamp") {
                        obj.insert("timestamp".into(), Value::String(now.clone()));
                    }
                }
                session.messages.push(entry);
            } else {
                let mut entry = m.clone();
                if let Value::Object(ref mut obj) = entry {
                    if !obj.contains_key("timestamp") {
                        obj.insert("timestamp".into(), Value::String(now.clone()));
                    }
                }
                session.messages.push(entry);
            }
        }
        if let Some(ms) = turn_latency_ms {
            if let Some(idx) = last_assistant_idx {
                if let Some(Value::Object(msg)) = session.messages.get_mut(idx) {
                    msg.insert("latency_ms".into(), serde_json::json!(ms));
                }
            }
        }
        session.updated_at = chrono::Local::now();
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

fn message_key(msg: &Value) -> (Option<&str>, Option<&str>, Option<&str>, Option<&str>, Option<&Value>, Option<&Value>) {
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
