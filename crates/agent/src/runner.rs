//! Shared execution loop for tool-using agents.
//! Port of `nanobot.agent.runner`.
//!
//! # Scope
//!
//! The Python runner is ~1000 lines that handle the core iteration plus a
//! long tail of "context governance" features (micro-compact, orphan tool
//! results, length recovery, finalization retries, injection cycles, ...).
//! This Rust port covers the core decision loop and the most-used
//! governance rules. Features left as `TODO` are documented inline where
//! they would hook in; they can be added without changing the public API.

use std::collections::HashMap;
use std::sync::Arc;

use async_trait::async_trait;
use futures::future::BoxFuture;
use log::warn;
use serde_json::Value;

use providers::{
    ChatRequest, LLMProvider, LLMResponse, RetryMode, ToolCallRequest, ToolChoice,
};
use utils::helpers::{build_assistant_message, maybe_persist_tool_result, truncate_text};
use utils::runtime::{
    ensure_nonempty_tool_result, external_lookup_signature, is_blank_text,
    repeated_external_lookup_error, EMPTY_FINAL_RESPONSE_MESSAGE,
};

use crate::hook::{AgentHook, AgentHookContext, ToolEvent};
use crate::tools::ToolRegistry;

const DEFAULT_ERROR_MESSAGE: &str = "Sorry, I encountered an error calling the AI model.";
const MAX_EMPTY_RETRIES: u32 = 2;
const MAX_LENGTH_RECOVERIES: u32 = 3;

/// Callback invoked at iteration checkpoints (awaiting_tools / tools_completed / ...).
pub type CheckpointCallback =
    Arc<dyn Fn(Value) -> BoxFuture<'static, ()> + Send + Sync>;

/// Callback invoked when the runner wants to drain pending user injections.
pub type InjectionCallback =
    Arc<dyn Fn(usize) -> BoxFuture<'static, Vec<Value>> + Send + Sync>;

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
    pub provider_retry_mode: RetryMode,
    pub checkpoint_callback: Option<CheckpointCallback>,
    pub injection_callback: Option<InjectionCallback>,
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
            provider_retry_mode: RetryMode::Standard,
            checkpoint_callback: None,
            injection_callback: None,
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

    pub async fn run(&self, spec: AgentRunSpec) -> AgentRunResult {
        let hook: Arc<dyn AgentHook> = spec
            .hook
            .clone()
            .unwrap_or_else(|| Arc::new(NoopHook));
        let mut messages = spec.initial_messages.clone();
        let mut final_content: Option<String> = None;
        let mut tools_used: Vec<String> = Vec::new();
        let mut usage: HashMap<String, i64> =
            HashMap::from([("prompt_tokens".into(), 0), ("completion_tokens".into(), 0)]);
        let mut error: Option<String> = None;
        let mut stop_reason: String = "completed".into();
        let mut tool_events: Vec<ToolEvent> = Vec::new();
        let mut external_lookup_counts: HashMap<String, u32> = HashMap::new();
        let mut empty_retries: u32 = 0;
        let mut length_recoveries: u32 = 0;
        let mut max_iterations_hit = true;

        for iteration in 0..spec.max_iterations {
            // TODO(context-governance): the Python runner runs
            // `_drop_orphan_tool_results` / `_backfill_missing_tool_results`
            // / `_microcompact` / `_apply_tool_result_budget` /
            // `_snip_history` before each model call. Those are deferred.
            let messages_for_model = messages.clone();
            let mut ctx = AgentHookContext {
                iteration: iteration as usize,
                messages: messages.clone(),
                ..Default::default()
            };
            hook.before_iteration(&mut ctx).await;

            let response = self
                .request_model(&spec, messages_for_model.clone(), hook.clone(), &mut ctx)
                .await;
            let raw_usage = response.usage.clone();
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

                emit_checkpoint(
                    &spec,
                    serde_json::json!({
                        "phase": "awaiting_tools",
                        "iteration": iteration,
                        "model": spec.model,
                        "assistant_message": assistant_message,
                        "pending_tool_calls": tool_calls_json,
                    }),
                )
                .await;

                hook.before_execute_tools(&mut ctx).await;
                let (results, events, fatal) = self
                    .execute_tools(&spec, &response.tool_calls, &mut external_lookup_counts)
                    .await;
                tool_events.extend(events.iter().cloned());
                ctx.tool_events = events.clone();
                ctx.tool_results = results.clone();
                for (tc, result) in response.tool_calls.iter().zip(results.iter()) {
                    let content = normalize_tool_result(&spec, &tc.id, &tc.name, result.clone());
                    messages.push(serde_json::json!({
                        "role": "tool",
                        "tool_call_id": tc.id,
                        "name": tc.name,
                        "content": content,
                    }));
                }
                if let Some(err) = fatal {
                    final_content = Some(err.clone());
                    stop_reason = "tool_error".into();
                    error = Some(err);
                    append_final_message(&mut messages, final_content.as_deref());
                    ctx.final_content = final_content.clone();
                    ctx.error = error.clone();
                    ctx.stop_reason = Some(stop_reason.clone());
                    hook.after_iteration(&mut ctx).await;
                    max_iterations_hit = false;
                    break;
                }
                emit_checkpoint(
                    &spec,
                    serde_json::json!({
                        "phase": "tools_completed",
                        "iteration": iteration,
                    }),
                )
                .await;
                empty_retries = 0;
                length_recoveries = 0;
                hook.after_iteration(&mut ctx).await;
                continue;
            }

            // No tool calls — evaluate final content.
            if response.has_tool_calls() {
                warn!(
                    "Ignoring tool calls under finish_reason='{}' for {}",
                    response.finish_reason,
                    spec.session_key.as_deref().unwrap_or("default")
                );
            }

            let clean = hook.finalize_content(&mut ctx, response.content.clone());

            if response.finish_reason != "error"
                && is_blank_text(clean.as_deref())
            {
                empty_retries += 1;
                if empty_retries < MAX_EMPTY_RETRIES {
                    warn!("Empty response on turn {iteration}; retrying ({empty_retries}/{MAX_EMPTY_RETRIES})");
                    if hook.wants_streaming() {
                        hook.on_stream_end(&mut ctx, false).await;
                    }
                    hook.after_iteration(&mut ctx).await;
                    continue;
                }
                // TODO: call `_request_finalization_retry` with
                // `build_finalization_retry_message()` — deferred.
            }

            if response.finish_reason == "length"
                && !is_blank_text(clean.as_deref())
            {
                length_recoveries += 1;
                if length_recoveries <= MAX_LENGTH_RECOVERIES {
                    messages.push(build_assistant_message(
                        clean.as_deref(),
                        None,
                        response.reasoning_content.as_deref(),
                        response.thinking_blocks.as_deref(),
                    ));
                    messages.push(serde_json::json!({
                        "role": "user",
                        "content": "(output truncated — please continue)",
                    }));
                    hook.after_iteration(&mut ctx).await;
                    continue;
                }
            }

            if response.finish_reason == "error" {
                let fc = clean
                    .clone()
                    .or_else(|| spec.error_message.clone())
                    .unwrap_or_else(|| DEFAULT_ERROR_MESSAGE.to_string());
                stop_reason = "error".into();
                error = Some(fc.clone());
                final_content = Some(fc);
                ctx.final_content = final_content.clone();
                ctx.error = error.clone();
                ctx.stop_reason = Some(stop_reason.clone());
                hook.after_iteration(&mut ctx).await;
                max_iterations_hit = false;
                break;
            }

            if is_blank_text(clean.as_deref()) {
                let fc = EMPTY_FINAL_RESPONSE_MESSAGE.to_string();
                stop_reason = "empty_final_response".into();
                error = Some(fc.clone());
                final_content = Some(fc);
                append_final_message(&mut messages, final_content.as_deref());
                ctx.final_content = final_content.clone();
                ctx.error = error.clone();
                ctx.stop_reason = Some(stop_reason.clone());
                hook.after_iteration(&mut ctx).await;
                max_iterations_hit = false;
                break;
            }

            let assistant = build_assistant_message(
                clean.as_deref(),
                None,
                response.reasoning_content.as_deref(),
                response.thinking_blocks.as_deref(),
            );
            messages.push(assistant.clone());
            emit_checkpoint(
                &spec,
                serde_json::json!({
                    "phase": "final_response",
                    "iteration": iteration,
                    "model": spec.model,
                    "assistant_message": assistant,
                }),
            )
            .await;
            final_content = clean;
            ctx.final_content = final_content.clone();
            ctx.stop_reason = Some(stop_reason.clone());
            hook.after_iteration(&mut ctx).await;
            max_iterations_hit = false;
            break;
        }

        if max_iterations_hit {
            stop_reason = "max_iterations".into();
            final_content = Some(spec.max_iterations_message.clone().unwrap_or_else(|| {
                format!(
                    "Task did not finish within {} iterations.",
                    spec.max_iterations
                )
            }));
            append_final_message(&mut messages, final_content.as_deref());
        }

        AgentRunResult {
            final_content,
            messages,
            tools_used,
            usage,
            stop_reason,
            error,
            tool_events,
            had_injections: false,
        }
    }

    async fn request_model(
        &self,
        spec: &AgentRunSpec,
        messages: Vec<Value>,
        hook: Arc<dyn AgentHook>,
        _ctx: &mut AgentHookContext,
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
        let _ = hook; // streaming path is TODO
        self.provider
            .chat_with_retry(req, spec.provider_retry_mode, None)
            .await
    }

    async fn execute_tools(
        &self,
        spec: &AgentRunSpec,
        calls: &[ToolCallRequest],
        external_lookup_counts: &mut HashMap<String, u32>,
    ) -> (Vec<Value>, Vec<ToolEvent>, Option<String>) {
        let mut results = Vec::with_capacity(calls.len());
        let mut events = Vec::with_capacity(calls.len());
        let mut fatal: Option<String> = None;
        for tc in calls {
            let args_value = Value::Object(tc.arguments.clone());
            if let Some(err) = repeated_external_lookup_error(
                &tc.name,
                &args_value,
                external_lookup_counts,
            ) {
                results.push(Value::String(err.clone()));
                events.push(ToolEvent {
                    name: tc.name.clone(),
                    status: "error".into(),
                    detail: err,
                });
                continue;
            }
            let _ = external_lookup_signature(&tc.name, &args_value);
            let result = spec
                .tools
                .execute(&tc.name, args_value)
                .await;
            let is_error = matches!(&result, Value::String(s) if s.starts_with("Error"));
            let detail = truncate_text(
                match &result {
                    Value::String(s) => s.clone(),
                    other => other.to_string(),
                }
                .as_str(),
                200,
            );
            events.push(ToolEvent {
                name: tc.name.clone(),
                status: if is_error { "error" } else { "ok" }.into(),
                detail,
            });
            if is_error && spec.fail_on_tool_error {
                fatal = Some(match &result {
                    Value::String(s) => s.clone(),
                    other => other.to_string(),
                });
                results.push(result);
                break;
            }
            results.push(result);
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

struct NoopHook;

#[async_trait]
impl AgentHook for NoopHook {}
