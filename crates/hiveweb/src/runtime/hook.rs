//! Hook execution engine — runs agent lifecycle hooks during orchestrator execution.
//!
//! Hook execution outcomes are emitted as structured tracing and never persisted.
//! Function/Workflow actions read a serialized AgentContext snapshot and may apply
//! only the controlled `_agent_context_updates` contract to the current execution.

use aws_sdk_s3::Client as S3Client;
use chrono::Utc;
use reqwest::StatusCode;
use serde::Serialize;
use serde_json::{Value, json};
use sqlx::MySqlPool;
use std::collections::HashMap;
use std::future::Future;
use std::sync::Arc;
use std::time::{Duration, Instant};

use crate::models::agent_hook::AgentHook;
use crate::runtime::capability::{CapabilityRegistry, DispatchCtx};
use crate::runtime::execution_context::RuntimeExecutionContext;
use crate::runtime::invoker::Invoker;
use crate::runtime::llm::LlmRegistry;
use agent::context::{AgentContext, Category, ExtensionContent, ExtensionType};

use crate::cache::redis::RedisClient;

const DEFAULT_WEBHOOK_RETRY_MAX: usize = 3;
const WEBHOOK_RETRY_BACKOFF_SECS: [u64; 3] = [1, 2, 4];

/// Context passed to each hook invocation.
#[derive(Debug, Clone, Serialize)]
pub struct HookContext {
    pub agent_id: i64,
    pub identifier: String,
    pub session_id: i64,
    pub actor_id: i64,
    pub request_id: String,
    pub trigger_point: String,
    /// 用户输入消息
    pub message: String,
    /// 渠道（如 "app", "web", "api" 等）
    pub channel: String,
    /// 端：android、iphone、ipad、web 等
    pub client_type: String,
    /// 客户端版本号
    pub client_version: String,
}

/// Dependencies needed by hook actions (call_function, call_workflow).
#[derive(Clone)]
pub struct HookDeps {
    /// Correlation inherited from the parent runtime, permanently restricted
    /// to tracing-only for every nested Hook action.
    pub execution_context: RuntimeExecutionContext,
    /// 仅在 `PLUGIN_SYSTEM_ENABLED=true` 时为 `Some`。
    pub s3: Option<S3Client>,
    pub llm: Arc<LlmRegistry>,
    pub registry: Arc<CapabilityRegistry>,
    pub invoker: Arc<Invoker>,
    pub ext_pool: Option<MySqlPool>,
    /// Redis client for cache-aside operations.
    pub redis: Option<RedisClient>,
    /// AgentContext for functions/workflows to read/write runtime state
    pub agent_ctx: Arc<AgentContext>,
}

/// Run all enabled hooks for a given trigger point.
///
/// Non-blocking mode: individual failures do not prevent subsequent hooks.
/// Blocking mode: first failure aborts the agent flow and returns `Err`.
#[tracing::instrument(skip(pool, hooks, ctx, deps), fields(
    agent_id = %ctx.agent_id,
    trigger_point = %point,
    session_id = %ctx.session_id
))]
pub async fn run_hooks(
    pool: Arc<MySqlPool>,
    hooks: &HashMap<String, Vec<AgentHook>>,
    point: &str,
    ctx: &HookContext,
    deps: &HookDeps,
) -> Result<(), HookError> {
    let list = match hooks.get(point) {
        Some(h) => h,
        None => return Ok(()),
    };

    for hook in list {
        if !hook.enabled {
            trace_hook_exec(hook, ctx, point, "skipped", None, None);
            continue;
        }

        let start = Instant::now();
        let timeout_ms = hook.timeout_ms.max(1000) as u64;

        // Webhook attempts own one timeout covering DNS, pinned-client
        // construction and send. Wrapping the same future here would create
        // equal-deadline cancellation races that can prevent retry scheduling.
        let result = if hook.action_type == "http_webhook" {
            Ok(execute_hook_action(hook, ctx, pool.clone(), deps).await)
        } else {
            tokio::time::timeout(
                Duration::from_millis(timeout_ms),
                execute_hook_action(hook, ctx, pool.clone(), deps),
            )
            .await
        };

        let elapsed = start.elapsed().as_millis() as i32;

        match result {
            Ok(Ok(())) => {
                trace_hook_exec(hook, ctx, point, "success", None, Some(elapsed));
            }
            Ok(Err(error)) if error.is_timeout() => {
                trace_hook_exec(hook, ctx, point, "timeout", Some("timeout"), Some(elapsed));

                if hook.blocking_mode {
                    return Err(HookError::Timeout(format!(
                        "Hook「{}」阻塞模式执行超时",
                        hook.name
                    )));
                }
            }
            Ok(Err(e)) => {
                trace_hook_exec(
                    hook,
                    ctx,
                    point,
                    "error",
                    Some(hook_error_kind(&hook.action_type)),
                    Some(elapsed),
                );

                if hook.blocking_mode {
                    return Err(HookError::BlockingFailed(blocking_failure_message(
                        &hook.name, &e,
                    )));
                }
                // Non-blocking: continue with next hook
            }
            Err(_timeout) => {
                trace_hook_exec(hook, ctx, point, "timeout", Some("timeout"), Some(elapsed));

                if hook.blocking_mode {
                    return Err(HookError::Timeout(format!(
                        "Hook「{}」阻塞模式执行超时",
                        hook.name
                    )));
                }
            }
        }
    }

    Ok(())
}

/// Execute the action specified by hook.action_type.
async fn execute_hook_action(
    hook: &AgentHook,
    ctx: &HookContext,
    pool: Arc<MySqlPool>,
    deps: &HookDeps,
) -> Result<(), ActionError> {
    match hook.action_type.as_str() {
        "call_function" => execute_call_function(hook, ctx, pool, deps).await,
        "call_workflow" => execute_call_workflow(hook, ctx, pool, deps).await,
        "http_webhook" => execute_http_webhook(hook, ctx).await,
        other => Err(ActionError::failed(format!("Unknown action type: {other}"))),
    }
}

async fn execute_call_function(
    hook: &AgentHook,
    ctx: &HookContext,
    pool: Arc<MySqlPool>,
    deps: &HookDeps,
) -> Result<(), ActionError> {
    let function_id = hook
        .action_params
        .get("function_id")
        .and_then(|v| v.as_i64())
        .ok_or_else(|| ActionError::failed("call_function: function_id is required"))?;

    // Query function info from DB
    let func_row: Option<(String, i8, Option<i64>, Option<String>)> = sqlx::query_as(
        "SELECT identifier, kind, plugin_id, plugin_export FROM functions WHERE id = ?",
    )
    .bind(function_id)
    .fetch_optional(pool.as_ref())
    .await
    .map_err(|e| ActionError::failed(format!("function lookup: {e}")))?;

    let Some((func_ident, func_kind, plugin_id, plugin_export)) = func_row else {
        return Err(ActionError::failed(format!(
            "call_function: function id={function_id} 不存在"
        )));
    };

    // Both action types receive the same trusted HookContext fields. Runtime
    // values override configured args, and `_agent_context` is injected last.
    let function_input = build_hook_action_input(&hook.action_params, ctx, &deps.agent_ctx)?;

    match func_kind {
        1 if plugin_id.is_none() => {
            // Builtin function — direct call
            let Some(builtin) = super::builtins::lookup(&func_ident) else {
                return Err(ActionError::failed(format!(
                    "call_function: builtin「{func_ident}」handler 未找到"
                )));
            };
            let bctx = super::builtins::BuiltinContext {
                execution_context: Some(deps.execution_context.clone()),
                pool: &pool,
                ext_pool: deps.ext_pool.as_ref(),
                redis: deps.redis.as_ref(),
                agent_ctx: Some(Arc::clone(&deps.agent_ctx)),
                llm: Some(&deps.llm),
                agent_id: Some(ctx.agent_id),
            };
            let output = (builtin.handler)(function_input, &bctx)
                .map_err(|e| ActionError::failed(format!("builtin function 执行失败: {e}")))?;
            // ★ Apply AgentContext updates from function output
            apply_agent_context_updates(&deps.agent_ctx, &output);
        }
        1 | 2 => {
            // Plugin-based or custom function — via invoker
            let Some(pid) = plugin_id else {
                return Err(ActionError::failed("call_function: function 缺 plugin_id"));
            };
            let Some(ref export) = plugin_export else {
                return Err(ActionError::failed(
                    "call_function: function 缺 plugin_export",
                ));
            };
            let input_json = serde_json::to_string(&function_input)
                .map_err(|e| ActionError::failed(format!("args serialize: {e}")))?;
            // 查询当前 agent 的 capability 权限
            let perms: Vec<String> =
                sqlx::query_as("SELECT capability FROM agent_permissions WHERE agent_id = ?")
                    .bind(ctx.agent_id)
                    .fetch_all(pool.as_ref())
                    .await
                    .map(|rows: Vec<(String,)>| rows.into_iter().map(|(c,)| c).collect())
                    .unwrap_or_default();
            let dispatch_ctx = DispatchCtx {
                execution_context: deps.execution_context.for_hook(),
                agent_id: ctx.agent_id,
                plugin_id: pid,
                function_id: Some(function_id),
                permissions: perms,
            };
            let output_str = deps
                .invoker
                .invoke(
                    &pool,
                    deps.s3.as_ref(),
                    Arc::clone(&deps.registry),
                    Arc::clone(&deps.llm),
                    pid,
                    export,
                    input_json,
                    dispatch_ctx,
                )
                .await
                .map_err(|e| ActionError::failed(format!("plugin invoke failed: {e}")))?;
            // ★ Parse output and apply AgentContext updates
            if let Ok(output_val) = serde_json::from_str::<Value>(&output_str) {
                apply_agent_context_updates(&deps.agent_ctx, &output_val);
            }
        }
        _ => {
            return Err(ActionError::failed(format!(
                "call_function: function「{func_ident}」kind={func_kind} 不支持"
            )));
        }
    }
    Ok(())
}

async fn execute_call_workflow(
    hook: &AgentHook,
    ctx: &HookContext,
    pool: Arc<MySqlPool>,
    deps: &HookDeps,
) -> Result<(), ActionError> {
    let workflow_id = hook
        .action_params
        .get("workflow_id")
        .and_then(|v| v.as_i64())
        .ok_or_else(|| ActionError::failed("call_workflow: workflow_id is required"))?;

    let workflow_input = build_hook_action_input(&hook.action_params, ctx, &deps.agent_ctx)?;

    // 查询当前 agent 的 capability 权限
    let perms: Vec<String> =
        sqlx::query_as("SELECT capability FROM agent_permissions WHERE agent_id = ?")
            .bind(ctx.agent_id)
            .fetch_all(&*pool)
            .await
            .map(|rows: Vec<(String,)>| rows.into_iter().map(|(c,)| c).collect())
            .unwrap_or_default();

    let executor_deps = super::workflow::ExecutorDeps {
        execution_context: deps.execution_context.for_hook(),
        pool: (*pool).clone(),
        s3: deps.s3.clone(),
        registry: Arc::clone(&deps.registry),
        llm: Arc::clone(&deps.llm),
        invoker: Arc::clone(&deps.invoker),
        ext_pool: deps.ext_pool.clone(),
        redis: deps.redis.clone(),
        permissions: perms,
    };
    let executor = super::workflow::WorkflowExecutor::new();
    let outcome = executor
        .execute(
            &executor_deps,
            workflow_id,
            workflow_input,
            ctx.agent_id,
            Arc::clone(&deps.agent_ctx),
        )
        .await
        .map_err(|e| ActionError::failed(format!("workflow execute: {e}")))?;

    // ★ Apply AgentContext updates from workflow output
    apply_agent_context_updates(&deps.agent_ctx, &outcome.end_value);

    Ok(())
}

async fn execute_http_webhook(hook: &AgentHook, ctx: &HookContext) -> Result<(), ActionError> {
    let payload = json!({
        "agent_identifier": ctx.identifier,
        "session_id": ctx.session_id,
        "trigger_point": ctx.trigger_point,
        "timestamp": Utc::now().to_rfc3339(),
        "hook_name": hook.name,
        "message": ctx.message,
        "channel": ctx.channel,
        "client_type": ctx.client_type,
        "client_version": ctx.client_version,
    });

    let result = send_webhook_once(hook, &payload).await;
    if webhook_attempt_is_retryable(&result) {
        // Scheduling occurs synchronously before the initial attempt returns,
        // so run_hooks cannot cancel the retry decision.
        spawn_webhook_retry(hook, ctx, &payload);
    }

    match result {
        Ok(status) if status.is_success() => Ok(()),
        Ok(status) => Err(ActionError::failed(format!(
            "Webhook returned HTTP {status}"
        ))),
        Err(error) if error.is_retryable() => {
            let timed_out = error.is_timeout();
            if timed_out {
                Err(ActionError::timeout(
                    "Webhook attempt timed out — pending async retry",
                ))
            } else {
                Err(ActionError::failed(
                    "Webhook connection failed — pending async retry",
                ))
            }
        }
        Err(WebhookRequestError::Policy) | Err(WebhookRequestError::Header) => Err(
            ActionError::failed("Webhook target or headers rejected by outbound policy"),
        ),
        Err(_) => Err(ActionError::failed("Webhook request failed without retry")),
    }
}

#[derive(Debug)]
enum WebhookRequestError {
    Timeout,
    Policy,
    ResolveUnavailable,
    Header,
    Client,
    Request(reqwest::Error),
}

impl From<super::capabilities::network_http::OutboundTargetError> for WebhookRequestError {
    fn from(error: super::capabilities::network_http::OutboundTargetError) -> Self {
        match error {
            super::capabilities::network_http::OutboundTargetError::Policy(_) => Self::Policy,
            super::capabilities::network_http::OutboundTargetError::ResolveUnavailable(_) => {
                Self::ResolveUnavailable
            }
        }
    }
}

impl WebhookRequestError {
    fn kind(&self) -> &'static str {
        match self {
            Self::Timeout => "timeout",
            Self::Policy => "policy",
            Self::ResolveUnavailable => "resolve_unavailable",
            Self::Header => "header",
            Self::Client => "client",
            Self::Request(error) => webhook_error_kind(error),
        }
    }

    fn is_timeout(&self) -> bool {
        matches!(self, Self::Timeout) || matches!(self, Self::Request(error) if error.is_timeout())
    }

    fn is_retryable(&self) -> bool {
        match self {
            Self::Timeout | Self::ResolveUnavailable => true,
            Self::Request(error) => error.is_connect() || error.is_timeout(),
            Self::Policy | Self::Header | Self::Client => false,
        }
    }
}

fn webhook_attempt_is_retryable(result: &Result<StatusCode, WebhookRequestError>) -> bool {
    matches!(result, Err(error) if error.is_retryable())
}

async fn run_webhook_attempt_with_timeout<T, F>(
    timeout_ms: u64,
    future: F,
) -> Result<T, WebhookRequestError>
where
    F: Future<Output = Result<T, WebhookRequestError>>,
{
    tokio::time::timeout(Duration::from_millis(timeout_ms), future)
        .await
        .map_err(|_| WebhookRequestError::Timeout)?
}

/// Send exactly one Webhook attempt through the same resolve-all, public-IP,
/// DNS-pinned and no-redirect transport used by `network.http`.
///
/// This function resolves again for every retry. A retry therefore cannot
/// silently fall back to reqwest's default DNS or redirect behavior.
async fn send_webhook_once(
    hook: &AgentHook,
    payload: &Value,
) -> Result<StatusCode, WebhookRequestError> {
    run_webhook_attempt_with_timeout(
        hook.timeout_ms.max(1000) as u64,
        send_webhook_once_inner(hook, payload),
    )
    .await
}

async fn send_webhook_once_inner(
    hook: &AgentHook,
    payload: &Value,
) -> Result<StatusCode, WebhookRequestError> {
    let url = hook
        .action_params
        .get("webhook_url")
        .and_then(Value::as_str)
        .ok_or(WebhookRequestError::Policy)?;
    let target = super::capabilities::network_http::resolve_outbound_target(
        url,
        super::capabilities::network_http::OutboundScheme::HttpsOnly,
    )
    .await
    .map_err(WebhookRequestError::from)?;
    let client = super::capabilities::network_http::pinned_outbound_client_for_attempt(&target)
        .map_err(|_| WebhookRequestError::Client)?;
    let mut request = client.post(target.url).json(payload);

    if let Some(raw_headers) = hook.action_params.get("headers") {
        let headers = raw_headers.as_object().ok_or(WebhookRequestError::Header)?;
        for (name, value) in headers {
            let value = value.as_str().ok_or(WebhookRequestError::Header)?;
            let (name, value) =
                super::capabilities::network_http::parse_outbound_header(name, value)
                    .map_err(|_| WebhookRequestError::Header)?;
            request = request.header(name, value);
        }
    }

    request
        .send()
        .await
        .map(|response| response.status())
        .map_err(WebhookRequestError::Request)
}

fn parse_webhook_retry_max(raw: Option<&str>) -> usize {
    raw.and_then(|value| value.parse::<usize>().ok())
        .filter(|value| *value <= WEBHOOK_RETRY_BACKOFF_SECS.len())
        .unwrap_or(DEFAULT_WEBHOOK_RETRY_MAX)
}

fn spawn_webhook_retry(hook: &AgentHook, ctx: &HookContext, payload: &Value) {
    let payload = payload.clone();
    let hook = hook.clone();
    let ctx = ctx.clone();
    let configured_retry_max = std::env::var("HOOK_WEBHOOK_RETRY_MAX").ok();
    let max_retries = parse_webhook_retry_max(configured_retry_max.as_deref());
    if max_retries == 0 {
        return;
    }

    tokio::spawn(async move {
        for (attempt, backoff_secs) in WEBHOOK_RETRY_BACKOFF_SECS
            .iter()
            .copied()
            .take(max_retries)
            .enumerate()
        {
            tokio::time::sleep(Duration::from_secs(backoff_secs)).await;
            let attempt_number = attempt + 1;
            match send_webhook_once(&hook, &payload).await {
                Ok(status) if status.is_success() => {
                    tracing::info!(
                        event = "hook_webhook_retry",
                        agent_id = ctx.agent_id,
                        hook_id = hook.id,
                        session_id = ctx.session_id,
                        trigger_point = %ctx.trigger_point,
                        action_type = %hook.action_type,
                        outcome = "success",
                        attempt = attempt_number,
                        max_retries,
                        http_status = status.as_u16(),
                        request_id = %ctx.request_id,
                        "Hook Webhook retry completed"
                    );
                    return;
                }
                Ok(status) => {
                    tracing::warn!(
                        event = "hook_webhook_retry",
                        agent_id = ctx.agent_id,
                        hook_id = hook.id,
                        session_id = ctx.session_id,
                        trigger_point = %ctx.trigger_point,
                        action_type = %hook.action_type,
                        outcome = "error",
                        attempt = attempt_number,
                        max_retries,
                        http_status = status.as_u16(),
                        request_id = %ctx.request_id,
                        error_kind = "http_status",
                        "Hook Webhook retry failed"
                    );
                    return;
                }
                Err(error) if !error.is_retryable() => {
                    tracing::warn!(
                        event = "hook_webhook_retry",
                        agent_id = ctx.agent_id,
                        hook_id = hook.id,
                        session_id = ctx.session_id,
                        trigger_point = %ctx.trigger_point,
                        action_type = %hook.action_type,
                        outcome = "error",
                        attempt = attempt_number,
                        max_retries,
                        request_id = %ctx.request_id,
                        error_kind = error.kind(),
                        "Hook Webhook retry stopped after non-retryable failure"
                    );
                    return;
                }
                Err(error) => {
                    tracing::warn!(
                        event = "hook_webhook_retry",
                        agent_id = ctx.agent_id,
                        hook_id = hook.id,
                        session_id = ctx.session_id,
                        trigger_point = %ctx.trigger_point,
                        action_type = %hook.action_type,
                        outcome = "error",
                        attempt = attempt_number,
                        max_retries,
                        request_id = %ctx.request_id,
                        error_kind = error.kind(),
                        "Hook Webhook retry failed"
                    );
                }
            }
        }
        tracing::warn!(
            event = "hook_webhook_retry",
            agent_id = ctx.agent_id,
            hook_id = hook.id,
            session_id = ctx.session_id,
            trigger_point = %ctx.trigger_point,
            action_type = %hook.action_type,
            outcome = "exhausted",
            max_retries,
            request_id = %ctx.request_id,
            error_kind = "retries_exhausted",
            "Hook Webhook retries exhausted"
        );
    });
}

fn trace_hook_exec(
    hook: &AgentHook,
    ctx: &HookContext,
    trigger_point: &str,
    outcome: &str,
    error_kind: Option<&str>,
    elapsed_ms: Option<i32>,
) {
    let error_kind = error_kind.unwrap_or_default();
    let elapsed_ms = elapsed_ms.unwrap_or_default();

    macro_rules! emit {
        ($level:ident) => {
            tracing::$level!(
                event = "hook_execution",
                agent_id = ctx.agent_id,
                hook_id = hook.id,
                session_id = ctx.session_id,
                trigger_point,
                action_type = %hook.action_type,
                outcome,
                elapsed_ms,
                request_id = %ctx.request_id,
                error_kind = %error_kind,
                "Hook execution completed"
            )
        };
    }

    match outcome {
        "error" | "timeout" => emit!(warn),
        "skipped" => emit!(debug),
        _ => emit!(info),
    }
}

fn hook_error_kind(action_type: &str) -> &'static str {
    match action_type {
        "call_function" => "function_action_failed",
        "call_workflow" => "workflow_action_failed",
        "http_webhook" => "webhook_action_failed",
        _ => "unknown_action_failed",
    }
}

fn webhook_error_kind(error: &reqwest::Error) -> &'static str {
    if error.is_timeout() {
        "timeout"
    } else if error.is_connect() {
        "connect"
    } else if error.is_request() {
        "request"
    } else {
        "unknown"
    }
}

// ── AgentContext helpers for function/workflow integration ──

/// Build a serializable snapshot of the relevant `AgentContext` state.
///
/// Shape (matches `inject_agent_context_snapshot` plus the new `user_input`
/// field for explicit `agent_context.user_input.*` references):
///
/// ```json
/// {
///   "user_input": {
///     "raw_text": "...", "session_id": "...", "message_id": "...",
///     "timestamp": 1234, "metadata": { ... }
///   },
///   "tool_results":   [{ "key", "value", "source" }, ...],
///   "entities":       [{ "key", "value", "source" }, ...],
///   "state_changes":  [{ "key", "value", "source" }, ...],
///   "extensions":     [ ExtensionContent, ... ]
/// }
/// ```
pub(crate) fn agent_context_snapshot_value(agent_ctx: &AgentContext) -> Value {
    let ui = agent_ctx.user_input();
    let ui_value = json!({
        "raw_text":   ui.raw_text,
        "session_id": ui.session_id,
        "message_id": ui.message_id,
        "timestamp":  ui.timestamp.timestamp_millis(),
        "metadata":   ui.metadata,
    });

    let cat_to_vec = |cat: Category| -> Vec<Value> {
        agent_ctx
            .get_category(cat)
            .unwrap_or_default()
            .iter()
            .map(|r| json!({"key": r.key, "value": r.value, "source": r.source}))
            .collect()
    };

    json!({
        "user_input":    ui_value,
        "tool_results":  cat_to_vec(Category::ToolResults),
        "entities":      cat_to_vec(Category::Entities),
        "state_changes": cat_to_vec(Category::StateChanges),
        "extensions":    agent_ctx
            .get_extensions()
            .iter()
            .map(|e| serde_json::to_value(e).unwrap_or(Value::Null))
            .collect::<Vec<_>>(),
        "messages":      agent_ctx.get_messages().unwrap_or_default(),
    })
}

/// Inject a read-only snapshot of AgentContext into the function/workflow input
/// as `_agent_context` field, so the function can inspect current runtime state.
pub(crate) fn inject_agent_context_snapshot(input: &mut Value, agent_ctx: &AgentContext) {
    let snapshot = agent_context_snapshot_value(agent_ctx);

    if let Value::Object(map) = input {
        map.insert("_agent_context".to_string(), snapshot);
    }
}

/// Build a Hook Function/Workflow action input from configured `args` and
/// trusted runtime fields. Runtime fields keep their existing values when a
/// configured arg uses the same key, and the runtime `_agent_context` snapshot
/// is always injected last so it cannot be forged by configuration.
fn build_hook_action_input(
    action_params: &Value,
    ctx: &HookContext,
    agent_ctx: &AgentContext,
) -> Result<Value, ActionError> {
    let mut input = action_params
        .get("args")
        .and_then(Value::as_object)
        .cloned()
        .map(Value::Object)
        .unwrap_or_else(|| Value::Object(serde_json::Map::new()));

    let runtime_fields = serde_json::to_value(ctx)
        .map_err(|error| ActionError::failed(format!("context serialize: {error}")))?;
    if let (Value::Object(input), Value::Object(runtime_fields)) = (&mut input, runtime_fields) {
        input.extend(runtime_fields);
    }

    inject_agent_context_snapshot(&mut input, agent_ctx);
    Ok(input)
}

/// Extract `_agent_context_updates` from a function/workflow output and
/// apply them back to the AgentContext.
///
/// Supported update actions:
/// - `records`: array of `{category, key, value}` → calls `set_record`
/// - `extensions`: array of `{id, content_type, data}` → calls `add_extension`
pub(crate) fn apply_agent_context_updates(agent_ctx: &AgentContext, output: &Value) {
    let updates = match output.get("_agent_context_updates") {
        Some(u) => u,
        None => return,
    };

    // Apply record updates
    if let Some(records) = updates.get("records").and_then(|v| v.as_array()) {
        for record in records {
            let category_str = record
                .get("category")
                .and_then(|v| v.as_str())
                .unwrap_or("");
            let key = record.get("key").and_then(|v| v.as_str()).unwrap_or("");
            let value = record.get("value").cloned().unwrap_or(Value::Null);
            let source = record
                .get("source")
                .and_then(|v| v.as_str())
                .unwrap_or("hook_function")
                .to_string();
            let iteration = record
                .get("iteration")
                .and_then(|v| v.as_u64())
                .unwrap_or(0) as usize;

            let cat = match category_str {
                "Entities" => Category::Entities,
                "Intentions" => Category::Intentions,
                "ToolResults" => Category::ToolResults,
                "QueryResults" => Category::QueryResults,
                "WorkflowResults" => Category::WorkflowResults,
                "ReasoningResults" => Category::ReasoningResults,
                "Extensions" => Category::Extensions,
                "StateChanges" => Category::StateChanges,
                "SubagentResults" => Category::SubagentResults,
                _ => {
                    tracing::warn!(
                        error_kind = "unknown_agent_context_category",
                        category_bytes = category_str.len(),
                        "Unknown category in _agent_context_updates"
                    );
                    continue;
                }
            };

            if agent_ctx
                .set_record(cat, key.to_string(), value, source, iteration)
                .is_err()
            {
                tracing::warn!(
                    error_kind = "agent_context_set_record_failed",
                    "Failed to apply _agent_context_updates record"
                );
            }
        }
    }

    // Apply extension updates
    if let Some(extensions) = updates.get("extensions").and_then(|v| v.as_array()) {
        for ext in extensions {
            let id = ext.get("id").and_then(|v| v.as_str()).unwrap_or("");
            let content_type_str = ext
                .get("content_type")
                .and_then(|v| v.as_str())
                .unwrap_or("card");
            let content_type = match content_type_str {
                "card" => ExtensionType::Card,
                "image" => ExtensionType::Image,
                "suggestion" => ExtensionType::Suggestion,
                "link" => ExtensionType::Link,
                "button" => ExtensionType::Button,
                "table" => ExtensionType::Table,
                "chart" => ExtensionType::Chart,
                "object_ref" => ExtensionType::ObjectRef,
                "usage" => ExtensionType::Usage,
                _ => ExtensionType::Card,
            };
            // Use explicit "data" key if present; otherwise fall back
            // to the whole object minus meta keys (content_type / id / reply).
            let data = if let Some(d) = ext.get("data") {
                d.clone()
            } else {
                // Strip meta fields from the extension object
                let mut obj = match ext.as_object() {
                    Some(o) => o.clone(),
                    None => continue,
                };
                obj.remove("content_type");
                obj.remove("id");
                obj.remove("reply");
                Value::Object(obj)
            };
            let content = ExtensionContent::new(
                id.to_string(),
                content_type,
                ext.get("reply").cloned(),
                data,
            );
            if agent_ctx.add_extension(id.to_string(), content).is_err() {
                tracing::warn!(
                    error_kind = "agent_context_add_extension_failed",
                    "Failed to apply _agent_context_updates extension"
                );
            }
        }
    }

    // Apply metadata updates (e.g., agent_loop_break)
    if let Some(metadata) = updates.get("metadata").and_then(|v| v.as_object()) {
        for (key, val) in metadata {
            if let Some(s) = val.as_str()
                && agent_ctx.set_metadata(key.clone(), s.to_string()).is_err()
            {
                tracing::warn!(
                    error_kind = "agent_context_set_metadata_failed",
                    "Failed to apply _agent_context_updates metadata"
                );
            }
        }
    }
}

// ── Error types ──

#[derive(Debug)]
pub enum HookError {
    BlockingFailed(String),
    Timeout(String),
}

impl std::fmt::Display for HookError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            HookError::BlockingFailed(s) | HookError::Timeout(s) => {
                write!(f, "{s}")
            }
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ActionErrorKind {
    Failed,
    Timeout,
}

#[derive(Debug)]
struct ActionError {
    message: String,
    kind: ActionErrorKind,
}

impl ActionError {
    fn failed(message: impl Into<String>) -> Self {
        Self {
            message: message.into(),
            kind: ActionErrorKind::Failed,
        }
    }

    fn timeout(message: impl Into<String>) -> Self {
        Self {
            message: message.into(),
            kind: ActionErrorKind::Timeout,
        }
    }

    fn is_timeout(&self) -> bool {
        self.kind == ActionErrorKind::Timeout
    }
}

fn blocking_failure_message(hook_name: &str, _source: &ActionError) -> String {
    // The source may contain SQL, URLs, provider responses, or Plugin details.
    // It is intentionally excluded from the external 6005 response.
    format!("Hook「{hook_name}」阻塞模式执行失败")
}

impl std::fmt::Display for ActionError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.message)
    }
}

#[cfg(test)]
mod tests {
    use super::{
        ActionError, HookContext, WEBHOOK_RETRY_BACKOFF_SECS, WebhookRequestError,
        blocking_failure_message, build_hook_action_input, parse_webhook_retry_max,
        run_webhook_attempt_with_timeout, webhook_attempt_is_retryable,
    };
    use crate::runtime::capabilities::network_http::OutboundTargetError;
    use agent::context::{AgentContext, ContextConfig, UserInput};
    use serde_json::json;
    use std::collections::HashMap;

    fn agent_context() -> AgentContext {
        AgentContext::new(
            "hook-action-input-test".to_string(),
            UserInput {
                raw_text: "trusted runtime input".to_string(),
                session_id: Some("session-1".to_string()),
                message_id: Some("message-1".to_string()),
                timestamp: chrono::Utc::now(),
                metadata: HashMap::new(),
            },
            ContextConfig::default(),
        )
    }

    fn hook_context() -> HookContext {
        HookContext {
            agent_id: 42,
            identifier: "trusted-agent".to_string(),
            session_id: 84,
            actor_id: 21,
            request_id: "d3c43d20-65f8-4f50-8f80-c526c127c6cc".to_string(),
            trigger_point: "before_agent_start".to_string(),
            message: "trusted message".to_string(),
            channel: "api".to_string(),
            client_type: "web".to_string(),
            client_version: "1.2.3".to_string(),
        }
    }

    #[test]
    fn blocking_failure_message_excludes_downstream_error_details() {
        let source =
            ActionError::failed("sensitive-sentinel mysql://user:password@db.example/internal");
        let message = blocking_failure_message("guard-hook", &source);

        assert_eq!(message, "Hook「guard-hook」阻塞模式执行失败");
        assert!(!message.contains("sensitive-sentinel"));
        assert!(!message.contains("password"));
    }

    #[test]
    fn function_action_input_merges_trusted_hook_context_and_overwrites_forged_values() {
        let params = json!({
            "function_id": 7,
            "args": {
                "template": "Hello, {{name}}",
                "name": "Hive",
                "agent_id": 999,
                "request_id": "forged-request-id",
                "_agent_context": {"user_input": {"raw_text": "forged"}}
            }
        });

        let input = build_hook_action_input(&params, &hook_context(), &agent_context()).unwrap();

        assert_eq!(input["template"], "Hello, {{name}}");
        assert_eq!(input["name"], "Hive");
        assert_eq!(input["agent_id"], 42);
        assert_eq!(input["request_id"], "d3c43d20-65f8-4f50-8f80-c526c127c6cc");
        assert_eq!(input["trigger_point"], "before_agent_start");
        assert_eq!(
            input["_agent_context"]["user_input"]["raw_text"],
            "trusted runtime input"
        );
    }

    #[test]
    fn workflow_action_input_merges_args_without_overwriting_runtime_hook_fields() {
        let params = json!({
            "workflow_id": 9,
            "args": {
                "report_kind": "daily",
                "agent_id": 999,
                "_agent_context": {"user_input": {"raw_text": "forged"}}
            }
        });
        let input = build_hook_action_input(&params, &hook_context(), &agent_context()).unwrap();

        assert_eq!(input["report_kind"], "daily");
        assert_eq!(input["agent_id"], 42);
        assert_eq!(input["trigger_point"], "before_agent_start");
        assert_eq!(
            input["_agent_context"]["user_input"]["raw_text"],
            "trusted runtime input"
        );
    }

    #[tokio::test]
    async fn webhook_attempt_timeout_covers_work_before_client_send() {
        let result = run_webhook_attempt_with_timeout(1, async {
            tokio::time::sleep(std::time::Duration::from_millis(25)).await;
            Ok(reqwest::StatusCode::OK)
        })
        .await;

        assert!(matches!(result, Err(WebhookRequestError::Timeout)));
        assert!(
            WebhookRequestError::Timeout.is_retryable(),
            "the initial timeout must deterministically schedule background retry"
        );
    }

    #[test]
    fn webhook_retry_max_is_strictly_bounded_and_invalid_values_use_safe_default() {
        assert_eq!(parse_webhook_retry_max(None), 3);
        assert_eq!(parse_webhook_retry_max(Some("0")), 0);
        assert_eq!(parse_webhook_retry_max(Some("1")), 1);
        assert_eq!(parse_webhook_retry_max(Some("3")), 3);
        assert_eq!(parse_webhook_retry_max(Some("4")), 3);
        assert_eq!(parse_webhook_retry_max(Some("-1")), 3);
        assert_eq!(parse_webhook_retry_max(Some("invalid")), 3);
    }

    #[test]
    fn webhook_retry_backoff_is_fixed_and_overflow_free() {
        assert_eq!(WEBHOOK_RETRY_BACKOFF_SECS, [1, 2, 4]);
    }

    #[test]
    fn webhook_retries_resolver_unavailability_but_never_policy_rejection() {
        let unavailable =
            WebhookRequestError::from(OutboundTargetError::ResolveUnavailable("DNS_LOOKUP_SECRET"));
        assert!(unavailable.is_retryable());
        assert_eq!(unavailable.kind(), "resolve_unavailable");
        assert!(webhook_attempt_is_retryable(&Err(unavailable)));

        let policy = WebhookRequestError::from(OutboundTargetError::Policy("POLICY_SECRET"));
        assert!(!policy.is_retryable());
        assert_eq!(policy.kind(), "policy");
        assert!(!webhook_attempt_is_retryable(&Err(policy)));

        assert!(!WebhookRequestError::Header.is_retryable());
        assert!(!WebhookRequestError::Client.is_retryable());

        let builder_error = reqwest::Client::new()
            .get("not a valid URL")
            .build()
            .expect_err("invalid URL must produce a non-connection request error");
        assert!(!builder_error.is_connect());
        assert!(!builder_error.is_timeout());
        assert!(!WebhookRequestError::Request(builder_error).is_retryable());

        let http_failure: Result<reqwest::StatusCode, WebhookRequestError> =
            Ok(reqwest::StatusCode::INTERNAL_SERVER_ERROR);
        assert!(!webhook_attempt_is_retryable(&http_failure));
    }

    #[tokio::test]
    async fn webhook_retries_connection_errors() {
        let listener = tokio::net::TcpListener::bind((std::net::Ipv4Addr::LOCALHOST, 0))
            .await
            .unwrap();
        let address = listener.local_addr().unwrap();
        drop(listener);

        let error = reqwest::Client::builder()
            .no_proxy()
            .timeout(std::time::Duration::from_secs(1))
            .build()
            .unwrap()
            .get(format!("http://{address}/"))
            .send()
            .await
            .expect_err("closed local port must reject the connection");
        assert!(error.is_connect());

        let result: Result<reqwest::StatusCode, WebhookRequestError> =
            Err(WebhookRequestError::Request(error));
        assert!(webhook_attempt_is_retryable(&result));
    }
}
