//! Shared execution loop for tool-using agents.
//! Port of `nanobot.agent.runner`.
//!
//! # Scope
//!
//! The Python runner is ~1000 lines that handle the core iteration plus a
//! long tail of "context governance" features (micro-compact, orphan tool
//! results, length recovery, finalization retries, injection cycles, ...).
//! This Rust port covers the core decision loop and all governance rules
//! from the Python original.

use std::collections::{HashMap, HashSet};
use std::path::Path;
use std::sync::{Arc, Mutex};

use async_trait::async_trait;
use futures::future::BoxFuture;
use log::{info, warn};
use providers::{
    ChatRequest, LLMProvider, LLMResponse, RetryMode, StreamDeltaCallback, ToolCallRequest,
    ToolChoice,
};
use serde_json::Value;
use tokio::sync::mpsc;
use tokio::time::timeout;
use utils::file_edit_events::StreamingFileEditTracker;
use utils::helpers::{
    IncrementalThinkExtractor, build_assistant_message, estimate_message_tokens,
    estimate_prompt_tokens, extract_reasoning, extract_think, find_legal_message_start,
    maybe_persist_tool_result, strip_think, truncate_text,
};
use utils::progress_events::on_progress_accepts_file_edit_events;
use utils::runtime::{
    ensure_nonempty_tool_result, external_lookup_signature, is_blank_text,
    repeated_external_lookup_error,
};

use crate::hook::{AgentHook, AgentHookContext, ToolEvent};
use crate::tools::ToolRegistry;

const DEFAULT_ERROR_MESSAGE: &str = "Sorry, I encountered an error calling the AI model.";
const EMPTY_FINAL_RESPONSE_MESSAGE: &str = "(No response from model)";
const FINALIZATION_RETRY_MESSAGE: &str =
    "Your last response appeared to be empty. Please provide your answer.";
const LENGTH_RECOVERY_MESSAGE: &str =
    "Your response was truncated. Please continue from where you left off.";
const MAX_EMPTY_RETRIES: u32 = 2;
const MAX_LENGTH_RECOVERIES: u32 = 3;
const MAX_REPEAT_WORKSPACE_VIOLATIONS: u32 = 2;
const MAX_INJECTIONS_PER_TURN: usize = 3;
const MAX_INJECTION_CYCLES: usize = 5;
const SNIP_SAFETY_BUFFER: usize = 1024;
const MICROCOMPACT_KEEP_RECENT: usize = 10;
const MICROCOMPACT_MIN_CHARS: usize = 500;
const BACKFILL_CONTENT: &str = "[Tool result unavailable — call was interrupted or lost]";
const COMPACTABLE_TOOLS: &[&str] = &[
    "read_file",
    "exec",
    "grep",
    "web_search",
    "web_fetch",
    "list_dir",
];

/// Callback invoked at iteration checkpoints (awaiting_tools / tools_completed / ...).
pub type CheckpointCallback = Arc<dyn Fn(Value) -> BoxFuture<'static, ()> + Send + Sync>;

/// Callback invoked when the runner wants to drain pending user injections.
pub type InjectionCallback = Arc<dyn Fn(usize) -> BoxFuture<'static, Vec<Value>> + Send + Sync>;

/// Callback invoked with progress updates (status messages, tool execution info).
pub type ProgressCallback = Arc<dyn Fn(&str) -> BoxFuture<'static, ()> + Send + Sync>;

/// Callback invoked when the runner is waiting between retries (rate limits, etc).
pub type RetryWaitCallback = Arc<dyn Fn(&str) -> BoxFuture<'static, ()> + Send + Sync>;

/// Configuration for a single agent execution.
#[derive(Clone)]
pub struct AgentRunSpec {
    pub initial_messages: Vec<Value>,
    pub tools: ToolRegistry,
    pub model: String,
    pub max_iterations: u32,
    pub max_tool_result_chars: usize,
    pub temperature: Option<f32>,
    pub max_tokens: Option<u32>,
    pub reasoning_effort: Option<String>,
    pub hook: Option<Arc<dyn AgentHook>>,
    pub error_message: Option<String>,
    pub max_iterations_message: Option<String>,
    pub concurrent_tools: bool,
    pub fail_on_tool_error: bool,
    pub workspace: Option<std::path::PathBuf>,
    pub session_key: Option<String>,
    pub context_window_tokens: Option<u32>,
    pub context_block_limit: Option<u32>,
    pub provider_retry_mode: RetryMode,
    pub checkpoint_callback: Option<CheckpointCallback>,
    pub injection_callback: Option<InjectionCallback>,
    pub llm_timeout_s: Option<f64>,
    /// Publish progress updates to the message bus.
    pub progress_callback: Option<ProgressCallback>,
    /// Publish retry-wait notifications to the message bus.
    pub retry_wait_callback: Option<RetryWaitCallback>,
    /// Whether to emit streaming content deltas as progress.
    pub stream_progress_deltas: bool,
}

impl AgentRunSpec {
    pub fn new(
        initial_messages: Vec<Value>,
        tools: ToolRegistry,
        model: impl Into<String>,
        max_iterations: u32,
        max_tool_result_chars: usize,
    ) -> Self {
        Self {
            initial_messages,
            tools,
            model: model.into(),
            max_iterations,
            max_tool_result_chars,
            temperature: None,
            max_tokens: None,
            reasoning_effort: None,
            hook: None,
            error_message: Some(DEFAULT_ERROR_MESSAGE.into()),
            max_iterations_message: None,
            concurrent_tools: false,
            fail_on_tool_error: false,
            workspace: None,
            session_key: None,
            context_window_tokens: None,
            context_block_limit: None,
            provider_retry_mode: RetryMode::Standard,
            checkpoint_callback: None,
            injection_callback: None,
            llm_timeout_s: None,
            progress_callback: None,
            retry_wait_callback: None,
            stream_progress_deltas: true,
        }
    }
}

/// Outcome of a shared agent execution.
#[derive(Debug, Clone, Default)]
pub struct AgentRunResult {
    pub final_content: Option<String>,
    pub messages: Vec<Value>,
    pub tools_used: Vec<String>,
    pub usage: HashMap<String, i64>,
    pub stop_reason: String,
    pub error: Option<String>,
    pub tool_events: Vec<ToolEvent>,
    pub had_injections: bool,
}

impl AgentRunResult {
    /// Convenience constructor used by short-circuit paths (command
    /// pre-filter, manual replies) that want a run-result shape without
    /// driving the full runner pipeline.
    pub fn short_circuit(content: impl Into<String>) -> Self {
        Self {
            final_content: Some(content.into()),
            stop_reason: "short_circuit".into(),
            ..Default::default()
        }
    }
}

/// Run a tool-capable LLM loop without product-layer concerns.
pub struct AgentRunner {
    provider: Arc<dyn LLMProvider>,
}

impl AgentRunner {
    pub fn new(provider: Arc<dyn LLMProvider>) -> Self {
        Self { provider }
    }

    // ========================================================================
    // Context governance pipeline (ported from Python runner)
    // ========================================================================

    /// Drop tool results that have no matching assistant tool_call earlier in the history.
    fn drop_orphan_tool_results(&self, messages: &[Value]) -> Vec<Value> {
        let mut declared: HashSet<String> = HashSet::new();
        let mut updated: Option<Vec<Value>> = None;
        for (idx, msg) in messages.iter().enumerate() {
            if let Some(role) = msg.get("role").and_then(Value::as_str) {
                if role == "assistant" {
                    if let Some(tcs) = msg.get("tool_calls").and_then(Value::as_array) {
                        for tc in tcs {
                            if let Some(id) = tc.get("id").and_then(Value::as_str) {
                                declared.insert(id.to_string());
                            }
                        }
                    }
                }
                if role == "tool" {
                    let tid = msg
                        .get("tool_call_id")
                        .and_then(Value::as_str)
                        .unwrap_or("");
                    if !tid.is_empty() && !declared.contains(tid) {
                        if updated.is_none() {
                            updated = Some(messages[..idx].iter().map(|m| m.clone()).collect());
                        }
                        continue;
                    }
                }
                if let Some(ref mut u) = updated {
                    u.push(msg.clone());
                }
            }
        }
        updated.unwrap_or_else(|| messages.to_vec())
    }

    /// Insert synthetic error results for orphaned tool_use blocks.
    fn backfill_missing_tool_results(&self, messages: Vec<Value>) -> Vec<Value> {
        let mut declared: Vec<(usize, String, String)> = Vec::new();
        let mut fulfilled: HashSet<String> = HashSet::new();
        for (idx, msg) in messages.iter().enumerate() {
            if let Some(role) = msg.get("role").and_then(Value::as_str) {
                if role == "assistant" {
                    if let Some(tcs) = msg.get("tool_calls").and_then(Value::as_array) {
                        for tc in tcs {
                            if let Some(id) = tc.get("id").and_then(Value::as_str) {
                                let name = tc
                                    .get("function")
                                    .and_then(|f| f.get("name"))
                                    .and_then(Value::as_str)
                                    .unwrap_or("")
                                    .to_string();
                                declared.push((idx, id.to_string(), name));
                            }
                        }
                    }
                } else if role == "tool" {
                    if let Some(tid) = msg.get("tool_call_id").and_then(Value::as_str) {
                        fulfilled.insert(tid.to_string());
                    }
                }
            }
        }

        let missing: Vec<(usize, String, String)> = declared
            .into_iter()
            .filter(|(_, cid, _)| !fulfilled.contains(cid))
            .collect();

        if missing.is_empty() {
            return messages;
        }

        let mut updated = messages;
        let mut offset = 0;
        for (assistant_idx, call_id, name) in missing {
            let mut insert_at = assistant_idx + 1 + offset;
            while insert_at < updated.len() {
                if let Some(role) = updated[insert_at].get("role").and_then(Value::as_str) {
                    if role == "tool" {
                        insert_at += 1;
                        continue;
                    }
                }
                break;
            }
            updated.insert(
                insert_at,
                serde_json::json!({
                    "role": "tool",
                    "tool_call_id": call_id,
                    "name": name,
                    "content": BACKFILL_CONTENT,
                }),
            );
            offset += 1;
        }
        updated
    }

    /// Replace old compactable tool results with one-line summaries.
    fn microcompact(&self, messages: Vec<Value>) -> Vec<Value> {
        let compactable_indices: Vec<usize> = messages
            .iter()
            .enumerate()
            .filter(|(_, msg)| {
                match (
                    msg.get("role").and_then(Value::as_str),
                    msg.get("name").and_then(Value::as_str),
                ) {
                    (Some("tool"), Some(name)) => COMPACTABLE_TOOLS.contains(&name),
                    _ => false,
                }
            })
            .map(|(i, _)| i)
            .collect();

        if compactable_indices.len() <= MICROCOMPACT_KEEP_RECENT {
            return messages;
        }

        let stale_indices =
            &compactable_indices[..compactable_indices.len() - MICROCOMPACT_KEEP_RECENT];
        let mut updated: Option<Vec<Value>> = None;

        for &idx in stale_indices {
            let content = messages[idx]
                .get("content")
                .and_then(Value::as_str)
                .unwrap_or("");
            if content.len() < MICROCOMPACT_MIN_CHARS {
                continue;
            }
            let name = messages[idx]
                .get("name")
                .and_then(Value::as_str)
                .unwrap_or("tool");
            let summary = format!("[{} result omitted from context]", name);
            let u = updated.get_or_insert_with(|| messages.clone());
            u[idx] = serde_json::json!({
                "role": "tool",
                "tool_call_id": messages[idx].get("tool_call_id").cloned().unwrap_or(Value::Null),
                "name": name,
                "content": summary,
            });
        }

        updated.unwrap_or(messages)
    }

    /// Apply tool result budget (truncation + normalization).
    fn apply_tool_result_budget(&self, spec: &AgentRunSpec, messages: Vec<Value>) -> Vec<Value> {
        let mut updated: Option<Vec<Value>> = None;
        for (idx, msg) in messages.iter().enumerate() {
            if msg.get("role").and_then(Value::as_str) != Some("tool") {
                continue;
            }
            let tool_call_id = msg
                .get("tool_call_id")
                .and_then(Value::as_str)
                .unwrap_or(&format!("tool_{}", idx))
                .to_string();
            let tool_name = msg
                .get("name")
                .and_then(Value::as_str)
                .unwrap_or("tool")
                .to_string();
            let content = msg.get("content").cloned().unwrap_or(Value::Null);

            let normalized = normalize_tool_result(spec, &tool_call_id, &tool_name, content);
            if Some(&normalized) == msg.get("content") {
                continue;
            }
            let u = updated.get_or_insert_with(|| messages.clone());
            u[idx] = {
                let mut m = msg.clone();
                if let Value::Object(ref mut obj) = m {
                    obj.insert("content".into(), normalized);
                }
                m
            };
        }
        updated.unwrap_or(messages)
    }

    /// Snip history to fit within the token budget.
    fn snip_history(&self, spec: &AgentRunSpec, messages: Vec<Value>) -> Vec<Value> {
        let context_window = match spec.context_window_tokens {
            Some(n) if n > 0 => n,
            _ => return messages,
        };

        let provider_max: u32 = spec.max_tokens.unwrap_or_else(|| {
            // Fallback — provider generation max_tokens not directly accessible here
            4096
        });

        let budget = spec.context_block_limit.unwrap_or_else(|| {
            context_window
                .saturating_sub(provider_max)
                .saturating_sub(SNIP_SAFETY_BUFFER as u32)
        });

        if budget == 0 {
            return messages;
        }

        let estimate = estimate_prompt_tokens(&messages, None);
        if estimate <= budget as usize {
            return messages;
        }

        let system: Vec<Value> = messages
            .iter()
            .filter(|m| m.get("role").and_then(Value::as_str) == Some("system"))
            .cloned()
            .collect();
        let non_system: Vec<Value> = messages
            .iter()
            .filter(|m| m.get("role").and_then(Value::as_str) != Some("system"))
            .cloned()
            .collect();

        if non_system.is_empty() {
            return messages;
        }

        let system_tokens: usize = system.iter().map(|m| estimate_message_tokens(m)).sum();
        let remaining_budget: usize =
            std::cmp::max(128, budget as usize).saturating_sub(system_tokens);

        let mut kept: Vec<Value> = Vec::new();
        let mut kept_tokens = 0;

        for message in non_system.iter().rev() {
            let msg_tokens = estimate_message_tokens(message);
            if !kept.is_empty() && kept_tokens + msg_tokens > remaining_budget {
                break;
            }
            kept.push(message.clone());
            kept_tokens += msg_tokens;
        }
        kept.reverse();

        // Ensure we start with a user message (GLM rejects system→assistant)
        if !kept.is_empty() {
            let first_user = kept
                .iter()
                .position(|m| m.get("role").and_then(Value::as_str) == Some("user"));
            if let Some(i) = first_user {
                kept = kept[i..].to_vec();
            } else {
                // Recover nearest user message from outside the kept window
                for idx in (0..non_system.len()).rev() {
                    if non_system[idx].get("role").and_then(Value::as_str) == Some("user") {
                        kept = non_system[idx..].to_vec();
                        break;
                    }
                }
            }
            let start = find_legal_message_start(&kept);
            if start > 0 {
                kept = kept[start..].to_vec();
            }
        }

        if kept.is_empty() {
            let keep = std::cmp::min(non_system.len(), 4);
            kept = non_system[non_system.len() - keep..].to_vec();
            let start = find_legal_message_start(&kept);
            if start > 0 {
                kept = kept[start..].to_vec();
            }
        }

        let mut result = system;
        result.extend(kept);
        result
    }

    // ========================================================================
    // Injection helpers (ported from Python runner)
    // ========================================================================

    /// Append injected user messages while preserving role alternation.
    fn append_injected_messages(messages: &mut Vec<Value>, injections: Vec<Value>) {
        for injection in injections {
            let msg_len = messages.len();
            if msg_len > 0
                && injection.get("role").and_then(Value::as_str) == Some("user")
                && messages[msg_len - 1].get("role").and_then(Value::as_str) == Some("user")
            {
                // Merge with last user message
                let last = messages[msg_len - 1].clone();
                let merged_content =
                    merge_message_content(last.get("content"), injection.get("content"));
                if let Value::Object(ref mut obj) = messages[msg_len - 1] {
                    obj.insert("content".into(), merged_content);
                }
                continue;
            }
            messages.push(injection);
        }
    }

    /// Drain pending user messages via the injection callback.
    async fn drain_injections(&self, spec: &AgentRunSpec) -> Vec<Value> {
        let callback = match &spec.injection_callback {
            Some(cb) => cb,
            None => return Vec::new(),
        };

        let items = match callback(MAX_INJECTIONS_PER_TURN).await {
            list if list.is_empty() => return Vec::new(),
            list => list,
        };

        let mut injected: Vec<Value> = Vec::new();
        for item in items {
            if let Some(role) = item.get("role").and_then(Value::as_str) {
                if role == "user" && item.get("content").is_some() {
                    injected.push(item);
                    continue;
                }
            }
            // Try extracting text content
            if let Some(text) = item.get("content").and_then(Value::as_str) {
                if !text.trim().is_empty() {
                    injected.push(serde_json::json!({ "role": "user", "content": text }));
                }
            }
        }

        if injected.len() > MAX_INJECTIONS_PER_TURN {
            let dropped = injected.len() - MAX_INJECTIONS_PER_TURN;
            warn!(
                "Injection callback returned {} messages, capping to {} ({} dropped)",
                injected.len(),
                MAX_INJECTIONS_PER_TURN,
                dropped,
            );
            injected.truncate(MAX_INJECTIONS_PER_TURN);
        }

        injected
    }

    /// Try to drain injections. Returns (should_continue, updated_cycles).
    async fn try_drain_injections(
        &self,
        spec: &AgentRunSpec,
        messages: &mut Vec<Value>,
        assistant_message: Option<Value>,
        injection_cycles: usize,
        iteration: Option<u32>,
        spec_clone: &AgentRunSpec,
    ) -> (bool, usize) {
        if injection_cycles >= MAX_INJECTION_CYCLES {
            return (false, injection_cycles);
        }

        let injected = self.drain_injections(spec).await;
        if injected.is_empty() {
            return (false, injection_cycles);
        }

        let cycles = injection_cycles + 1;

        if let Some(am) = assistant_message {
            messages.push(am.clone());
            if let Some(iter) = iteration {
                emit_checkpoint(
                    spec_clone,
                    serde_json::json!({
                        "phase": "final_response",
                        "iteration": iter,
                        "model": spec_clone.model,
                        "assistant_message": am,
                        "completed_tool_results": [],
                        "pending_tool_calls": [],
                    }),
                )
                .await;
            }
        }

        Self::append_injected_messages(messages, injected);
        info!(
            "Injected follow-up message(s) (cycles: {}/{})",
            cycles, MAX_INJECTION_CYCLES,
        );
        (true, cycles)
    }

    /// Request a finalization retry (no-tools-allowed prompt for empty responses).
    async fn request_finalization_retry(
        &self,
        spec: &AgentRunSpec,
        messages: &[Value],
    ) -> LLMResponse {
        let mut retry_messages = messages.to_vec();
        retry_messages.push(build_finalization_retry_message());
        let req = ChatRequest {
            messages: retry_messages,
            tools: None,
            model: Some(spec.model.clone()),
            max_tokens: spec.max_tokens.unwrap_or(0),
            temperature: spec.temperature.unwrap_or(f32::NAN),
            reasoning_effort: spec.reasoning_effort.clone(),
            tool_choice: None,
        };
        self.provider
            .chat_with_retry(req, spec.provider_retry_mode, None)
            .await
    }

    // ========================================================================
    // Main run loop
    // ========================================================================

    pub async fn run(&self, spec: AgentRunSpec) -> AgentRunResult {
        let hook: Arc<dyn AgentHook> = spec.hook.clone().unwrap_or_else(|| Arc::new(NoopHook));
        let mut messages = spec.initial_messages.clone();
        let mut final_content: Option<String> = None;
        let mut tools_used: Vec<String> = Vec::new();
        let mut usage: std::collections::HashMap<String, i64> = std::collections::HashMap::from([
            ("prompt_tokens".into(), 0),
            ("completion_tokens".into(), 0),
        ]);
        let mut error: Option<String> = None;
        let mut stop_reason: String = "completed".into();
        let mut tool_events: Vec<ToolEvent> = Vec::new();
        let mut external_lookup_counts: std::collections::HashMap<String, u32> =
            std::collections::HashMap::new();
        let mut workspace_violation_counts: std::collections::HashMap<String, u32> =
            std::collections::HashMap::new();
        let mut empty_retries: u32 = 0;
        let mut length_recoveries: u32 = 0;
        let mut had_injections = false;
        let mut injection_cycles = 0;
        let mut max_iterations_hit = true;

        for iteration in 0..spec.max_iterations {
            // Context governance pipeline (before each model call)
            let mut messages_for_model = self.drop_orphan_tool_results(&messages);
            messages_for_model = self.backfill_missing_tool_results(messages_for_model);
            messages_for_model = self.microcompact(messages_for_model);
            messages_for_model = self.apply_tool_result_budget(&spec, messages_for_model);
            messages_for_model = self.snip_history(&spec, messages_for_model);
            // Clean up any new orphans created by snipping
            messages_for_model = self.drop_orphan_tool_results(&messages_for_model);
            messages_for_model = self.backfill_missing_tool_results(messages_for_model);

            let mut ctx = AgentHookContext {
                iteration: iteration as usize,
                messages: messages.clone(),
                ..Default::default()
            };
            hook.before_iteration(&mut ctx).await;

            let response = self
                .request_model(&spec, messages_for_model, hook.clone(), &mut ctx)
                .await;
            let raw_usage = response.usage.clone();

            // Extract reasoning and clean content (matches Python lines 297-306)
            let (reasoning_text, cleaned_content) = extract_reasoning(
                response.reasoning_content.as_deref(),
                response.thinking_blocks.as_ref().map(|v| v.as_slice()),
                response.content.as_deref(),
            );
            let mut response = response;
            response.content = cleaned_content.clone();
            if let Some(ref rt) = reasoning_text {
                if !rt.is_empty() && !ctx.streamed_reasoning {
                    hook.emit_reasoning(Some(rt)).await;
                    hook.emit_reasoning_end().await;
                    ctx.streamed_reasoning = true;
                }
            }

            ctx.response = Some(response.clone());
            ctx.usage = raw_usage.clone();
            ctx.tool_calls = response.tool_calls.clone();
            accumulate_usage(&mut usage, &raw_usage);

            if response.should_execute_tools() {
                if hook.wants_streaming() {
                    hook.on_stream_end(&mut ctx, true).await;
                }
                let tool_calls_json: Vec<Value> = response
                    .tool_calls
                    .iter()
                    .map(|tc| tc.to_openai_tool_call())
                    .collect();
                let assistant_message = build_assistant_message(
                    response.content.as_deref(),
                    Some(&tool_calls_json),
                    response.reasoning_content.as_deref(),
                    response.thinking_blocks.as_deref(),
                );
                messages.push(assistant_message.clone());
                tools_used.extend(response.tool_calls.iter().map(|tc| tc.name.clone()));

                // Publish progress update: tools about to execute
                if let Some(ref progress_cb) = spec.progress_callback {
                    let tool_names: Vec<String> = response
                        .tool_calls
                        .iter()
                        .map(|tc| tc.name.clone())
                        .collect();
                    let msg = format!("Executing: {}", tool_names.join(", "));
                    progress_cb(&msg).await;
                }

                emit_checkpoint(
                    &spec,
                    serde_json::json!({
                        "phase": "awaiting_tools",
                        "iteration": iteration,
                        "model": spec.model,
                        "assistant_message": assistant_message,
                        "completed_tool_results": [],
                        "pending_tool_calls": tool_calls_json,
                    }),
                )
                .await;

                hook.before_execute_tools(&mut ctx).await;
                let (results, events, fatal) = self
                    .execute_tools(
                        &spec,
                        &response.tool_calls,
                        &mut external_lookup_counts,
                        &mut workspace_violation_counts,
                    )
                    .await;
                tool_events.extend(events.iter().cloned());
                ctx.tool_events = events.clone();
                ctx.tool_results = results.clone();

                let mut completed_tool_results: Vec<Value> = Vec::new();
                for (tc, result) in response.tool_calls.iter().zip(results.iter()) {
                    let content = normalize_tool_result(&spec, &tc.id, &tc.name, result.clone());
                    let tool_message = serde_json::json!({
                        "role": "tool",
                        "tool_call_id": tc.id,
                        "name": tc.name,
                        "content": content,
                    });
                    messages.push(tool_message.clone());
                    completed_tool_results.push(tool_message);
                }

                if let Some(err) = fatal {
                    error = Some(format!("Error: {}", err));
                    final_content = Some(error.as_ref().unwrap().clone());
                    stop_reason = "tool_error".into();
                    append_final_message(&mut messages, final_content.as_deref());
                    ctx.final_content = final_content.clone();
                    ctx.error = error.clone();
                    ctx.stop_reason = Some(stop_reason.clone());
                    await_hook_after_iteration(&hook, &mut ctx).await;

                    let (cont, cycles) = self
                        .try_drain_injections(
                            &spec,
                            &mut messages,
                            None,
                            injection_cycles,
                            None,
                            &spec,
                        )
                        .await;
                    injection_cycles = cycles;
                    if cont {
                        had_injections = true;
                        continue;
                    }
                    max_iterations_hit = false;
                    break;
                }

                emit_checkpoint(
                    &spec,
                    serde_json::json!({
                        "phase": "tools_completed",
                        "iteration": iteration,
                        "model": spec.model,
                        "assistant_message": assistant_message,
                        "completed_tool_results": completed_tool_results,
                        "pending_tool_calls": [],
                    }),
                )
                .await;

                // Publish progress update: tools completed
                if let Some(ref progress_cb) = spec.progress_callback {
                    progress_cb("Tool execution completed, continuing...").await;
                }

                empty_retries = 0;
                length_recoveries = 0;

                // Checkpoint 1: drain injections after tools, before next LLM call
                let (drained, cycles) = self
                    .try_drain_injections(&spec, &mut messages, None, injection_cycles, None, &spec)
                    .await;
                injection_cycles = cycles;
                if drained {
                    had_injections = true;
                }

                await_hook_after_iteration(&hook, &mut ctx).await;
                continue;
            }

            // No tool execution — evaluate final content.
            if response.has_tool_calls() {
                warn!(
                    "Ignoring tool calls under finish_reason='{}' for {}",
                    response.finish_reason,
                    spec.session_key.as_deref().unwrap_or("default")
                );
            }

            let clean = hook.finalize_content(&mut ctx, response.content.clone());

            // Empty response retry
            if response.finish_reason != "error" && is_blank_text(clean.as_deref()) {
                empty_retries += 1;
                if empty_retries < MAX_EMPTY_RETRIES {
                    warn!(
                        "Empty response on turn {} ({}/{}); retrying",
                        iteration, empty_retries, MAX_EMPTY_RETRIES,
                    );
                    // Notify client about retry wait
                    if let Some(ref retry_cb) = spec.retry_wait_callback {
                        retry_cb("Empty response, retrying...").await;
                    }
                    if hook.wants_streaming() {
                        hook.on_stream_end(&mut ctx, false).await;
                    }
                    await_hook_after_iteration(&hook, &mut ctx).await;
                    continue;
                }
                warn!(
                    "Empty response on turn {} after {} retries; attempting finalization",
                    iteration, empty_retries,
                );
                if hook.wants_streaming() {
                    hook.on_stream_end(&mut ctx, false).await;
                }
                let retry_response = self.request_finalization_retry(&spec, &messages).await;
                let retry_usage = retry_response.usage.clone();
                accumulate_usage(&mut usage, &retry_usage);
                let merged_usage = {
                    let mut m = raw_usage.clone();
                    merge_usage(&mut m, &retry_usage);
                    m
                };
                ctx.response = Some(retry_response.clone());
                ctx.usage = merged_usage;
                ctx.tool_calls = retry_response.tool_calls.clone();
                let retry_clean = hook.finalize_content(&mut ctx, retry_response.content.clone());
                // Fall through with the retry content
                return self
                    .handle_final_content(
                        &spec,
                        &hook,
                        messages,
                        retry_clean,
                        retry_response,
                        empty_retries,
                        length_recoveries,
                        iteration as u32,
                        injection_cycles,
                        &mut had_injections,
                        &mut usage,
                    )
                    .await;
            }

            // Length recovery
            if response.finish_reason == "length" && !is_blank_text(clean.as_deref()) {
                length_recoveries += 1;
                if length_recoveries <= MAX_LENGTH_RECOVERIES {
                    info!(
                        "Output truncated on turn {} ({}/{}); continuing",
                        iteration, length_recoveries, MAX_LENGTH_RECOVERIES,
                    );
                    if hook.wants_streaming() {
                        hook.on_stream_end(&mut ctx, true).await;
                    }
                    messages.push(build_assistant_message(
                        clean.as_deref(),
                        None,
                        response.reasoning_content.as_deref(),
                        response.thinking_blocks.as_deref(),
                    ));
                    messages.push(build_length_recovery_message());
                    await_hook_after_iteration(&hook, &mut ctx).await;
                    continue;
                }
            }

            // Final content processing (delegated)
            return self
                .handle_final_content(
                    &spec,
                    &hook,
                    messages,
                    clean,
                    response,
                    empty_retries,
                    length_recoveries,
                    iteration as u32,
                    injection_cycles,
                    &mut had_injections,
                    &mut usage,
                )
                .await;
        }

        // Max iterations reached
        if max_iterations_hit {
            stop_reason = "max_iterations".into();
            final_content = Some(spec.max_iterations_message.clone().unwrap_or_else(|| {
                format!(
                    "Task did not finish within {} iterations.",
                    spec.max_iterations
                )
            }));
            append_final_message(&mut messages, final_content.as_deref());

            // Drain injections so they are appended instead of re-published
            let _ = self
                .try_drain_injections(&spec, &mut messages, None, injection_cycles, None, &spec)
                .await;
            had_injections = true;
        }

        AgentRunResult {
            final_content,
            messages,
            tools_used,
            usage,
            stop_reason,
            error,
            tool_events,
            had_injections,
        }
    }

    /// Handle the final content after the model decides not to execute tools.
    #[allow(clippy::too_many_arguments)]
    async fn handle_final_content(
        &self,
        spec: &AgentRunSpec,
        hook: &Arc<dyn AgentHook>,
        mut messages: Vec<Value>,
        clean: Option<String>,
        response: LLMResponse,
        _empty_retries: u32,
        _length_recoveries: u32,
        iteration: u32,
        mut injection_cycles: usize,
        had_injections: &mut bool,
        usage: &mut std::collections::HashMap<String, i64>,
    ) -> AgentRunResult {
        let mut final_content: Option<String> = None;
        let mut stop_reason: String = "completed".into();
        let mut error: Option<String> = None;

        let (is_blank,) = (is_blank_text(clean.as_deref()),);

        // Check for mid-turn injections BEFORE signaling stream end
        let assistant_msg = if response.finish_reason != "error" && !is_blank {
            Some(build_assistant_message(
                clean.as_deref(),
                None,
                response.reasoning_content.as_deref(),
                response.thinking_blocks.as_deref(),
            ))
        } else {
            None
        };

        let (should_continue, cycles) = self
            .try_drain_injections(
                spec,
                &mut messages,
                assistant_msg.clone(),
                injection_cycles,
                None,
                spec,
            )
            .await;
        injection_cycles = cycles;
        if should_continue {
            *had_injections = true;
        }

        if hook.wants_streaming() {
            hook.on_stream_end(&mut AgentHookContext::default(), should_continue)
                .await;
        }

        if should_continue {
            let mut ctx = AgentHookContext {
                messages: messages.clone(),
                ..Default::default()
            };
            await_hook_after_iteration(hook, &mut ctx).await;
            return AgentRunResult {
                final_content,
                messages,
                tools_used: Vec::new(),
                usage: usage.clone(),
                stop_reason: "injected".into(),
                error,
                tool_events: Vec::new(),
                had_injections: true,
            };
        }

        if response.finish_reason == "error" {
            let fc = clean.unwrap_or_else(|| {
                spec.error_message
                    .clone()
                    .unwrap_or_else(|| DEFAULT_ERROR_MESSAGE.to_string())
            });
            stop_reason = "error".into();
            final_content = Some(fc.clone());
            error = Some(fc.clone());
            append_model_error_placeholder(&mut messages);
            let mut ctx = AgentHookContext {
                messages: messages.clone(),
                final_content: Some(fc.clone()),
                error: Some(fc),
                stop_reason: Some(stop_reason.clone()),
                ..Default::default()
            };
            await_hook_after_iteration(hook, &mut ctx).await;

            let (cont, cycles) = self
                .try_drain_injections(spec, &mut messages, None, injection_cycles, None, spec)
                .await;
            injection_cycles = cycles;
            if cont {
                *had_injections = true;
            }
            return AgentRunResult {
                final_content,
                messages,
                tools_used: Vec::new(),
                usage: usage.clone(),
                stop_reason,
                error,
                tool_events: Vec::new(),
                had_injections: *had_injections,
            };
        }

        if is_blank {
            let fc = EMPTY_FINAL_RESPONSE_MESSAGE.to_string();
            stop_reason = "empty_final_response".into();
            final_content = Some(fc.clone());
            error = Some(fc.clone());
            append_final_message(&mut messages, final_content.as_deref());
            let mut ctx = AgentHookContext {
                messages: messages.clone(),
                final_content: Some(fc),
                error: error.clone(),
                stop_reason: Some(stop_reason.clone()),
                ..Default::default()
            };
            await_hook_after_iteration(hook, &mut ctx).await;

            let (cont, cycles) = self
                .try_drain_injections(spec, &mut messages, None, injection_cycles, None, spec)
                .await;
            injection_cycles = cycles;
            if cont {
                *had_injections = true;
            }
            return AgentRunResult {
                final_content,
                messages,
                tools_used: Vec::new(),
                usage: usage.clone(),
                stop_reason,
                error,
                tool_events: Vec::new(),
                had_injections: *had_injections,
            };
        }

        let assistant = assistant_msg.unwrap_or_else(|| {
            build_assistant_message(
                clean.as_deref(),
                None,
                response.reasoning_content.as_deref(),
                response.thinking_blocks.as_deref(),
            )
        });
        messages.push(assistant.clone());
        emit_checkpoint(
            spec,
            serde_json::json!({
                "phase": "final_response",
                "iteration": iteration,
                "model": spec.model,
                "assistant_message": assistant,
                "completed_tool_results": [],
                "pending_tool_calls": [],
            }),
        )
        .await;
        final_content = clean;
        let mut ctx = AgentHookContext {
            messages: messages.clone(),
            final_content: final_content.clone(),
            stop_reason: Some(stop_reason.clone()),
            ..Default::default()
        };
        await_hook_after_iteration(hook, &mut ctx).await;

        AgentRunResult {
            final_content,
            messages,
            tools_used: Vec::new(),
            usage: usage.clone(),
            stop_reason,
            error,
            tool_events: Vec::new(),
            had_injections: *had_injections,
        }
    }

    async fn request_model(
        &self,
        spec: &AgentRunSpec,
        messages: Vec<Value>,
        hook: Arc<dyn AgentHook>,
        ctx: &mut AgentHookContext,
    ) -> LLMResponse {
        let tools = spec.tools.get_definitions().await;
        let tool_choice = if tools.is_empty() {
            None
        } else {
            Some(ToolChoice::Auto)
        };
        let req = ChatRequest {
            messages,
            tools: if tools.is_empty() { None } else { Some(tools) },
            model: Some(spec.model.clone()),
            max_tokens: spec.max_tokens.unwrap_or(0),
            temperature: spec.temperature.unwrap_or(f32::NAN),
            reasoning_effort: spec.reasoning_effort.clone(),
            tool_choice,
        };

        let wants_streaming = hook.wants_streaming();
        let wants_progress_streaming =
            !wants_streaming && spec.stream_progress_deltas && spec.progress_callback.is_some();

        // Check if we should emit file edit progress events (matches Python
        // on_progress_accepts_file_edit_events check)
        let emit_file_edit_events =
            spec.progress_callback.is_some() && on_progress_accepts_file_edit_events();

        if wants_streaming || wants_progress_streaming {
            let accumulator: Arc<Mutex<String>> = Arc::new(Mutex::new(String::new()));
            let thinking_accumulator: Arc<Mutex<String>> = Arc::new(Mutex::new(String::new()));
            let think_extractor: Arc<Mutex<IncrementalThinkExtractor>> =
                Arc::new(Mutex::new(IncrementalThinkExtractor::new()));

            let (delta_tx, mut delta_rx) = mpsc::unbounded_channel::<String>();
            let (reasoning_tx, mut reasoning_rx) = mpsc::unbounded_channel::<String>();
            let reasoning_tx_for_progress = reasoning_tx.clone();

            // Set up file edit tracker if we're emitting file edit events
            let live_file_edits: Option<Arc<tokio::sync::Mutex<StreamingFileEditTracker>>> =
                if emit_file_edit_events {
                    let progress_cb = spec.progress_callback.clone().unwrap();
                    let workspace = spec.workspace.clone();
                    // The emit callback serializes file edit events to JSON and
                    // sends them through the progress_callback (matches Python
                    // invoke_file_edit_progress pattern).
                    let emit_fn = Arc::new(move |events: Vec<serde_json::Value>| {
                        let cb = progress_cb.clone();
                        let (tx, rx) = tokio::sync::oneshot::channel();
                        tokio::spawn(async move {
                            for event in events {
                                if let Ok(json) = serde_json::to_string(&event) {
                                    cb(&json).await;
                                }
                            }
                            let _ = tx.send(());
                        });
                        rx
                    });
                    Some(Arc::new(tokio::sync::Mutex::new(
                        StreamingFileEditTracker::new(workspace, emit_fn),
                    )))
                } else {
                    None
                };

            let acc_ref = accumulator.clone();
            let think_acc_ref = thinking_accumulator.clone();
            let extractor_ref = think_extractor.clone();

            let on_content_delta: StreamDeltaCallback = Arc::new(move |delta: String| {
                let mut buf = acc_ref.lock().unwrap();
                buf.push_str(&delta);
                let full_buf = buf.clone();
                drop(buf);

                let mut extractor = extractor_ref.lock().unwrap();
                if let Some(reasoning_delta) = extractor.feed(&full_buf) {
                    let mut think_acc = think_acc_ref.lock().unwrap();
                    think_acc.push_str(&reasoning_delta);
                    let _ = reasoning_tx.send(reasoning_delta);
                }
                drop(extractor);

                let _ = delta_tx.send(delta);
            });

            // Set up tool call delta callback for file edit tracking
            let on_tool_call_delta: Option<providers::base::ToolCallDeltaCallback> =
                if emit_file_edit_events {
                    let file_edits_ref = live_file_edits.clone().unwrap();
                    Some(Arc::new(
                        move |delta: serde_json::Map<String, serde_json::Value>| {
                            let edits = file_edits_ref.clone();
                            let event = serde_json::Value::Object(delta);
                            tokio::spawn(async move {
                                let tracker = edits.lock().await;
                                let _ = tracker.update(&event).await;
                            });
                        },
                    ))
                } else {
                    None
                };

            let stream_future = self.provider.chat_stream_with_retry(
                req,
                Some(on_content_delta),
                on_tool_call_delta,
                spec.provider_retry_mode,
                None,
            );

            let response = if wants_streaming {
                let handle = tokio::spawn({
                    let hook = hook.clone();
                    async move {
                        while let Some(delta) = delta_rx.recv().await {
                            let mut ctx = AgentHookContext::default();
                            ctx.streamed_content = true;
                            hook.on_stream(&mut ctx, &delta).await;
                        }
                    }
                });
                let reasoning_handle = tokio::spawn({
                    let hook = hook.clone();
                    async move {
                        while let Some(delta) = reasoning_rx.recv().await {
                            hook.emit_reasoning(Some(&delta)).await;
                        }
                        hook.emit_reasoning_end().await;
                    }
                });

                let response = stream_future.await;

                handle.abort();
                reasoning_handle.abort();

                // File edit tracker lifecycle: flush remaining events and wire up
                // final call IDs (matches Python live_file_edits.flush() + apply_final_call_ids)
                if let Some(ref edits) = live_file_edits {
                    let tracker = edits.lock().await;
                    tracker.flush().await;
                    if response.should_execute_tools() {
                        let tool_call_values: Vec<serde_json::Value> = response
                            .tool_calls
                            .iter()
                            .map(|tc| {
                                serde_json::json!({
                                    "id": tc.id,
                                    "name": tc.name,
                                    "arguments": tc.arguments,
                                })
                            })
                            .collect();
                        tracker.apply_final_call_ids(&tool_call_values).await;
                    } else {
                        let _ = tracker.error_unmatched(&[], "Tool call did not complete.");
                    }
                }

                response
            } else {
                // Progress-streaming mode: emit incremental clean content
                // to progress_callback, reasoning via emit_reasoning.
                let progress_cb = spec.progress_callback.clone().unwrap();
                let handle = tokio::spawn({
                    let reasoning_tx2 = reasoning_tx_for_progress.clone();
                    let _think_extractor = think_extractor.clone();
                    let acc_ref = accumulator.clone();
                    async move {
                        let mut prev_clean_len = 0;
                        let mut reasoning_open = false;
                        while let Some(_delta) = delta_rx.recv().await {
                            let buf = acc_ref.lock().unwrap().clone();
                            let (thinking, cleaned) = extract_think(&buf);
                            if let Some(t) = thinking {
                                if !t.is_empty() {
                                    let _ = reasoning_tx2.send(t);
                                    reasoning_open = true;
                                }
                            }
                            if reasoning_open {
                                if cleaned.is_empty() || cleaned.len() <= prev_clean_len {
                                    continue;
                                }
                                let incremental = &cleaned[prev_clean_len..];
                                if !incremental.is_empty() {
                                    progress_cb(incremental).await;
                                    prev_clean_len = cleaned.len();
                                }
                                // Check if reasoning closed
                                let full_buf = acc_ref.lock().unwrap().clone();
                                let (_, c) = extract_think(&full_buf);
                                let (t, _) = extract_think(&full_buf);
                                if t.is_none() && !c.is_empty() {
                                    reasoning_open = false;
                                }
                            } else {
                                let cleaned_text = strip_think(&buf);
                                if cleaned_text.len() <= prev_clean_len {
                                    continue;
                                }
                                let incremental = &cleaned_text[prev_clean_len..];
                                if !incremental.is_empty() {
                                    progress_cb(incremental).await;
                                    prev_clean_len = cleaned_text.len();
                                }
                            }
                        }
                    }
                });
                let reasoning_handle = tokio::spawn({
                    let hook = hook.clone();
                    async move {
                        let mut reasoning_open = false;
                        while let Some(delta) = reasoning_rx.recv().await {
                            hook.emit_reasoning(Some(&delta)).await;
                            reasoning_open = true;
                        }
                        if reasoning_open {
                            hook.emit_reasoning_end().await;
                        }
                    }
                });

                let response = stream_future.await;

                handle.abort();
                reasoning_handle.abort();

                // Ensure reasoning is closed after stream ends (matches Python
                // lines 733-734: if progress_state and progress_state.get("reasoning_open"))
                hook.emit_reasoning_end().await;

                // File edit tracker lifecycle: flush + apply_final_call_ids
                if let Some(ref edits) = live_file_edits {
                    let tracker = edits.lock().await;
                    tracker.flush().await;
                    if response.should_execute_tools() {
                        let tool_call_values: Vec<serde_json::Value> = response
                            .tool_calls
                            .iter()
                            .map(|tc| {
                                serde_json::json!({
                                    "id": tc.id,
                                    "name": tc.name,
                                    "arguments": tc.arguments,
                                })
                            })
                            .collect();
                        tracker.apply_final_call_ids(&tool_call_values).await;
                    } else {
                        let _ = tracker.error_unmatched(&[], "Tool call did not complete.");
                    }
                }

                response
            };

            let final_content = accumulator.lock().unwrap().clone();
            let final_reasoning = thinking_accumulator.lock().unwrap().clone();

            let content_empty = final_content.is_empty();
            let reasoning_empty = final_reasoning.is_empty();

            let mut response = response;
            if response.content.is_none()
                || response
                    .content
                    .as_ref()
                    .map(|s| s.is_empty())
                    .unwrap_or(true)
            {
                response.content = if content_empty {
                    None
                } else {
                    Some(final_content)
                };
            }
            if response.reasoning_content.is_none() && !reasoning_empty {
                response.reasoning_content = Some(final_reasoning);
            }

            ctx.streamed_content = !content_empty;
            ctx.streamed_reasoning = !reasoning_empty;

            response
        } else {
            let timeout_s = spec.llm_timeout_s.unwrap_or_else(|| {
                std::env::var("NANOBOT_LLM_TIMEOUT_S")
                    .ok()
                    .and_then(|v| v.parse::<f64>().ok())
                    .unwrap_or(300.0)
            });

            let timeout_dur = std::time::Duration::from_secs_f64(timeout_s);
            timeout(
                timeout_dur,
                self.provider
                    .chat_with_retry(req, spec.provider_retry_mode, None),
            )
            .await
            .unwrap_or_else(|_| LLMResponse::error("LLM request timed out"))
        }
    }

    /// Partition tool calls into batches for concurrent execution (matches
    /// Python _partition_tool_batches).
    async fn partition_tool_batches(
        &self,
        spec: &AgentRunSpec,
        tool_calls: &[ToolCallRequest],
    ) -> Vec<Vec<ToolCallRequest>> {
        if !spec.concurrent_tools {
            return tool_calls.iter().map(|tc| vec![tc.clone()]).collect();
        }

        let mut batches: Vec<Vec<ToolCallRequest>> = Vec::new();
        let mut current: Vec<ToolCallRequest> = Vec::new();
        for tc in tool_calls {
            let tool = spec.tools.get(&tc.name).await;
            let can_batch = tool.as_ref().map_or(false, |t| t.concurrency_safe());
            if can_batch {
                current.push(tc.clone());
                continue;
            }
            if !current.is_empty() {
                batches.push(std::mem::take(&mut current));
            }
            batches.push(vec![tc.clone()]);
        }
        if !current.is_empty() {
            batches.push(current);
        }
        batches
    }

    /// Execute a single tool call and return the result, event, and optional fatal error.
    #[allow(clippy::too_many_arguments)]
    async fn execute_single_tool(
        &self,
        spec: &AgentRunSpec,
        tc: &ToolCallRequest,
        external_lookup_counts: &mut HashMap<String, u32>,
        workspace_violation_counts: &mut HashMap<String, u32>,
    ) -> (Value, ToolEvent, Option<String>) {
        let hint = "\n\n[Analyze the error above and try a different approach.]";
        let args_value = Value::Object(tc.arguments.clone());
        if let Some(err) =
            repeated_external_lookup_error(&tc.name, &args_value, external_lookup_counts)
        {
            return (
                Value::String(format!("{}{}", err, hint)),
                ToolEvent {
                    name: tc.name.clone(),
                    status: "error".into(),
                    detail: truncate_text(&err, 120).to_string(),
                },
                None,
            );
        }
        let _ = external_lookup_signature(&tc.name, &args_value);
        let result = spec.tools.execute(&tc.name, args_value).await;
        let raw_result = match &result {
            Value::String(s) => s.clone(),
            other => other.to_string(),
        };
        let is_error = raw_result.starts_with("Error");
        let detail = truncate_text(&raw_result, 200).to_string();
        let mut event = ToolEvent {
            name: tc.name.clone(),
            status: if is_error { "error" } else { "ok" }.into(),
            detail,
        };
        if is_error {
            if let Some((payload, evt, fatal_opt)) = self.classify_violation(
                &raw_result,
                &raw_result,
                &mut event,
                tc,
                workspace_violation_counts,
            ) {
                if let Some(f) = &fatal_opt {
                    if spec.fail_on_tool_error {
                        return (payload.clone(), evt, Some(f.clone()));
                    }
                }
                return (payload, evt, None);
            }
            if spec.fail_on_tool_error {
                return (
                    Value::String(format!("{}{}", raw_result, hint)),
                    event,
                    Some(raw_result.clone()),
                );
            }
            return (
                Value::String(format!("{}{}", raw_result, hint)),
                event,
                None,
            );
        }
        (result, event, None)
    }

    async fn execute_tools(
        &self,
        spec: &AgentRunSpec,
        calls: &[ToolCallRequest],
        external_lookup_counts: &mut HashMap<String, u32>,
        workspace_violation_counts: &mut HashMap<String, u32>,
    ) -> (Vec<Value>, Vec<ToolEvent>, Option<String>) {
        let batches = self.partition_tool_batches(spec, calls).await;
        let mut results = Vec::with_capacity(calls.len());
        let mut events = Vec::with_capacity(calls.len());
        let mut fatal: Option<String> = None;

        for batch in batches {
            if spec.concurrent_tools && batch.len() > 1 {
                // Execute batch concurrently (matches Python asyncio.gather)
                let mut futures = Vec::new();
                for tc in &batch {
                    let self_ref = self;
                    let spec_ref = spec.clone();
                    let tc_clone = tc.clone();
                    futures.push(async move {
                        self_ref
                            .execute_single_tool(
                                &spec_ref,
                                &tc_clone,
                                &mut HashMap::new(),
                                &mut HashMap::new(),
                            )
                            .await
                    });
                }
                let batch_results = futures::future::join_all(futures).await;
                for (result, event, fatal_opt) in batch_results {
                    if let Some(f) = fatal_opt {
                        fatal = Some(f);
                    }
                    results.push(result);
                    events.push(event);
                }
            } else {
                // Sequential execution
                for tc in &batch {
                    let (result, event, fatal_opt) = self
                        .execute_single_tool(
                            spec,
                            tc,
                            external_lookup_counts,
                            workspace_violation_counts,
                        )
                        .await;
                    if let Some(f) = fatal_opt {
                        fatal = Some(f);
                        results.push(result);
                        events.push(event);
                        break;
                    }
                    results.push(result);
                    events.push(event);
                }
            }
            if fatal.is_some() {
                break;
            }
        }
        (results, events, fatal)
    }
}

fn normalize_tool_result(
    spec: &AgentRunSpec,
    tool_call_id: &str,
    tool_name: &str,
    result: Value,
) -> Value {
    let content = ensure_nonempty_tool_result(tool_name, result);
    maybe_persist_tool_result(
        spec.workspace.as_deref(),
        spec.session_key.as_deref(),
        tool_call_id,
        content,
        spec.max_tool_result_chars,
    )
}

fn accumulate_usage(target: &mut HashMap<String, i64>, delta: &HashMap<String, i64>) {
    for (k, v) in delta {
        *target.entry(k.clone()).or_insert(0) += v;
    }
}

/// Merge two usage dicts — keeps the higher value for each key (matches Python
/// _merge_usage pattern used when combining retry usage with original usage).
fn merge_usage(base: &mut HashMap<String, i64>, other: &HashMap<String, i64>) {
    for (k, v) in other {
        let entry = base.entry(k.clone()).or_insert(0);
        *entry += v;
    }
}

fn merge_message_content(left: Option<&Value>, right: Option<&Value>) -> Value {
    match (left, right) {
        (Some(Value::String(a)), Some(Value::String(b))) => {
            if a.is_empty() {
                Value::String(b.clone())
            } else {
                Value::String(format!("{a}\n\n{b}"))
            }
        }
        (Some(left), Some(right)) => {
            let mut out: Vec<Value> = to_blocks(left.clone());
            out.extend(to_blocks(right.clone()));
            Value::Array(out)
        }
        (Some(val), None) | (None, Some(val)) => val.clone(),
        (None, None) => Value::String(String::new()),
    }
}

fn to_blocks(val: Value) -> Vec<Value> {
    match val {
        Value::Array(items) => items
            .into_iter()
            .map(|item| {
                if item.is_object() {
                    item
                } else {
                    let s = item
                        .as_str()
                        .map(String::from)
                        .unwrap_or_else(|| item.to_string());
                    serde_json::json!({"type":"text","text":s})
                }
            })
            .collect(),
        Value::Null => Vec::new(),
        other => {
            let s = other
                .as_str()
                .map(String::from)
                .unwrap_or_else(|| other.to_string());
            vec![serde_json::json!({"type":"text","text":s})]
        }
    }
}

async fn await_hook_after_iteration(hook: &Arc<dyn AgentHook>, ctx: &mut AgentHookContext) {
    hook.after_iteration(ctx).await;
}

fn append_model_error_placeholder(messages: &mut Vec<Value>) {
    // Only append placeholder if the last message is NOT already an assistant
    // message without tool_calls (matches Python _append_model_error_placeholder)
    if let Some(last) = messages.last() {
        if last.get("role").and_then(Value::as_str) == Some("assistant")
            && !last
                .get("tool_calls")
                .map(|v| !v.is_null())
                .unwrap_or(false)
        {
            return;
        }
    }
    messages.push(serde_json::json!({
        "role": "assistant",
        "content": "[Assistant reply unavailable due to model error.]",
    }));
}

fn append_final_message(messages: &mut Vec<Value>, content: Option<&str>) {
    let Some(content) = content else {
        return;
    };
    if content.is_empty() {
        return;
    }
    messages.push(serde_json::json!({"role":"assistant","content":content}));
}

async fn emit_checkpoint(spec: &AgentRunSpec, payload: Value) {
    if let Some(cb) = &spec.checkpoint_callback {
        (cb)(payload).await;
    }
}

fn build_finalization_retry_message() -> Value {
    serde_json::json!({"role": "user", "content": FINALIZATION_RETRY_MESSAGE})
}

fn build_length_recovery_message() -> Value {
    serde_json::json!({"role": "user", "content": LENGTH_RECOVERY_MESSAGE})
}

struct NoopHook;

#[async_trait]
impl AgentHook for NoopHook {}

const SSRF_MARKERS: &[&str] = &[
    "internal/private url detected",
    "private/internal address",
    "private address",
];

const SSRF_BOUNDARY_NOTE: &str = concat!(
    "This is a non-bypassable security boundary. Stop trying to access ",
    "private/internal URLs. Do not retry with curl, wget, encoded IPs, ",
    "alternate DNS, redirects, proxies, or another tool. Ask the user for ",
    "local files, logs, screenshots, or an explicit safe public URL instead. ",
    "If the user explicitly trusts this private URL, ask them to whitelist ",
    "the exact IP/CIDR via tools.ssrfWhitelist.",
);

const WORKSPACE_VIOLATION_MARKERS: &[&str] = &[
    "outside the configured workspace",
    "outside allowed directory",
    "working_dir is outside",
    "working_dir could not be resolved",
    "path outside working dir",
    "path traversal detected",
];

impl AgentRunner {
    fn is_ssrf_violation(text: &str) -> bool {
        if text.is_empty() {
            return false;
        }
        let lowered = text.to_lowercase();
        SSRF_MARKERS.iter().any(|m| lowered.contains(m))
    }

    fn is_workspace_violation(text: &str) -> bool {
        if text.is_empty() {
            return false;
        }
        let lowered = text.to_lowercase();
        if Self::is_ssrf_violation(&lowered) {
            return true;
        }
        WORKSPACE_VIOLATION_MARKERS
            .iter()
            .any(|m| lowered.contains(m))
    }

    fn classify_violation(
        &self,
        raw_text: &str,
        soft_payload: &str,
        event: &mut ToolEvent,
        tool_call: &ToolCallRequest,
        workspace_violation_counts: &mut HashMap<String, u32>,
    ) -> Option<(Value, ToolEvent, Option<String>)> {
        if Self::is_ssrf_violation(raw_text) {
            warn!(
                "Tool {} blocked by SSRF guard; returning non-retryable tool error: {}",
                tool_call.name,
                raw_text
                    .replace('\n', " ")
                    .trim()
                    .chars()
                    .take(200)
                    .collect::<String>(),
            );
            event.detail = Self::event_detail("ssrf_violation: ", raw_text, 160);
            let payload = Self::ssrf_soft_payload(raw_text);
            return Some((Value::String(payload), event.clone(), None));
        }

        if Self::is_workspace_violation(raw_text) {
            let args_value = Value::Object(tool_call.arguments.clone());
            let escalation = repeated_workspace_violation_error(
                &tool_call.name,
                &args_value,
                workspace_violation_counts,
            );
            event.detail = Self::event_detail("workspace_violation: ", raw_text, 160);
            if let Some(escalated) = escalation {
                warn!(
                    "Tool {} hit workspace boundary repeatedly; escalating hint",
                    tool_call.name,
                );
                event.detail = Self::event_detail("workspace_violation_escalated: ", raw_text, 160);
                return Some((Value::String(escalated), event.clone(), None));
            }
            return Some((Value::String(soft_payload.to_string()), event.clone(), None));
        }

        None
    }

    fn ssrf_soft_payload(raw_text: &str) -> String {
        let text = raw_text.trim();
        let text = if text.is_empty() {
            "Error: request blocked by SSRF guard"
        } else {
            text
        };
        format!("{}\n\n{}", text, SSRF_BOUNDARY_NOTE)
    }

    fn event_detail(prefix: &str, text: &str, limit: usize) -> String {
        let combined = format!("{}{}", prefix, text.replace('\n', " ").trim());
        combined.chars().take(limit).collect()
    }
}

fn workspace_violation_signature(tool_name: &str, arguments: &Value) -> Option<String> {
    let keys = ["path", "file_path", "target", "source", "destination"];
    for key in &keys {
        if let Some(val) = arguments.get(key).and_then(|v| v.as_str()) {
            let trimmed = val.trim();
            if !trimmed.is_empty() {
                return Some(normalize_violation_target(trimmed));
            }
        }
    }

    if tool_name == "exec" || tool_name == "shell" {
        if let Some(cmd) = arguments.get("command").and_then(|v| v.as_str()) {
            let cmd_trimmed = cmd.trim();
            if !cmd_trimmed.is_empty() {
                for part in cmd_trimmed.split_whitespace() {
                    if part.starts_with('/') {
                        return Some(normalize_violation_target(part));
                    }
                }
            }
        }
        if let Some(cwd) = arguments.get("working_dir").and_then(|v| v.as_str()) {
            let trimmed = cwd.trim();
            if !trimmed.is_empty() {
                return Some(normalize_violation_target(trimmed));
            }
        }
    }

    None
}

fn normalize_violation_target(raw: &str) -> String {
    let normalized = match Path::new(raw).canonicalize() {
        Ok(p) => p.to_string_lossy().to_string(),
        Err(_) => raw.replace('\\', "/"),
    };
    format!("violation:{}", normalized.to_lowercase())
}

fn repeated_workspace_violation_error(
    tool_name: &str,
    arguments: &Value,
    seen_counts: &mut HashMap<String, u32>,
) -> Option<String> {
    let signature = workspace_violation_signature(tool_name, arguments)?;
    let count = *seen_counts.entry(signature.clone()).or_insert(0) + 1;
    if count <= MAX_REPEAT_WORKSPACE_VIOLATIONS {
        return None;
    }
    warn!(
        "Escalating repeated workspace bypass attempt {} (attempt {})",
        &signature[..signature.len().min(160)],
        count,
    );
    let target = signature
        .splitn(2, "violation:")
        .nth(1)
        .unwrap_or(&signature);
    Some(format!(
        "Error: refusing repeated workspace-bypass attempts.\n\
         You have tried to access '{}' (or an equivalent path) \
         {} times in this turn. This is a hard policy boundary -- \
         switching tools, shell tricks, working_dir overrides, symlinks, \
         or base64 piping will NOT change the answer. Stop retrying. \
         If the user genuinely needs this resource, tell them you cannot \
         access it and ask how they want to proceed (e.g. copy the file \
         into the workspace, or disable restrict_to_workspace for this run).",
        target, count,
    ))
}
