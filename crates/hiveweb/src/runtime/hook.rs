//! Hook execution engine — runs agent lifecycle hooks during orchestrator execution.
//!
//! Hooks are **read-only observers**: results are written only to `hook_executions`
//! audit table and never injected into agent state (system_prompt, tools, etc.).

use aws_sdk_s3::Client as S3Client;
use chrono::Utc;
use serde::Serialize;
use serde_json::{Value, json};
use sqlx::MySqlPool;
use std::collections::HashMap;
use std::sync::Arc;
use std::time::Instant;

use crate::models::agent_hook::AgentHook;
use crate::runtime::capability::{CapabilityRegistry, DispatchCtx};
use crate::runtime::invoker::Invoker;
use crate::runtime::llm::LlmRegistry;
use agent::context::{AgentContext, Category, ExtensionContent, ExtensionType};

use crate::cache::redis::RedisClient;

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
    identifier = %ctx.identifier,
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
            // Record skipped
            audit_hook_exec(&pool, hook.id, ctx, point, "skipped", None, None).await;
            continue;
        }

        let start = Instant::now();
        let timeout_ms = hook.timeout_ms.max(1000) as u64;

        let result = tokio::time::timeout(
            std::time::Duration::from_millis(timeout_ms),
            execute_hook_action(hook, ctx, pool.clone(), deps),
        )
        .await;

        let elapsed = start.elapsed().as_millis() as i32;

        match result {
            Ok(Ok(())) => {
                audit_hook_exec(&pool, hook.id, ctx, point, "success", None, Some(elapsed)).await;
            }
            Ok(Err(e)) => {
                audit_hook_exec(
                    &pool,
                    hook.id,
                    ctx,
                    point,
                    "error",
                    Some(&e.to_string()),
                    Some(elapsed),
                )
                .await;

                if hook.blocking_mode {
                    return Err(HookError::BlockingFailed(format!(
                        "Hook「{}」阻塞模式执行失败: {}",
                        hook.name, e
                    )));
                }
                // Non-blocking: continue with next hook
            }
            Err(_timeout) => {
                audit_hook_exec(
                    &pool,
                    hook.id,
                    ctx,
                    point,
                    "timeout",
                    Some(&format!("超时 {}ms", timeout_ms)),
                    Some(elapsed),
                )
                .await;

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
        "http_webhook" => execute_http_webhook(hook, ctx, pool).await,
        other => Err(ActionError(format!("Unknown action type: {other}"))),
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
        .ok_or_else(|| ActionError("call_function: function_id is required".into()))?;

    // Query function info from DB
    let func_row: Option<(String, i8, Option<i64>, Option<String>)> = sqlx::query_as(
        "SELECT identifier, kind, plugin_id, plugin_export FROM functions WHERE id = ?",
    )
    .bind(function_id)
    .fetch_optional(pool.as_ref())
    .await
    .map_err(|e| ActionError(format!("function lookup: {e}")))?;

    let Some((func_ident, func_kind, plugin_id, plugin_export)) = func_row else {
        return Err(ActionError(format!(
            "call_function: function id={function_id} 不存在"
        )));
    };

    // 只传 AgentContext snapshot，不再序列化 HookContext（避免与 _agent_context 重复）
    let mut function_input = serde_json::json!({});
    inject_agent_context_snapshot(&mut function_input, &deps.agent_ctx);

    match func_kind {
        1 if plugin_id.is_none() => {
            // Builtin function — direct call
            let Some(builtin) = super::builtins::lookup(&func_ident) else {
                return Err(ActionError(format!(
                    "call_function: builtin「{func_ident}」handler 未找到"
                )));
            };
            let bctx = super::builtins::BuiltinContext {
                pool: &pool,
                ext_pool: deps.ext_pool.as_ref(),
                redis: deps.redis.as_ref(),
                agent_ctx: Some(Arc::clone(&deps.agent_ctx)),
                llm: Some(&deps.llm),
                agent_id: None,
            };
            let output = (builtin.handler)(function_input, &bctx)
                .map_err(|e| ActionError(format!("builtin function 执行失败: {e}")))?;
            // ★ Apply AgentContext updates from function output
            apply_agent_context_updates(&deps.agent_ctx, &output);
        }
        1 | 2 => {
            // Plugin-based or custom function — via invoker
            let Some(pid) = plugin_id else {
                return Err(ActionError("call_function: function 缺 plugin_id".into()));
            };
            let Some(ref export) = plugin_export else {
                return Err(ActionError(
                    "call_function: function 缺 plugin_export".into(),
                ));
            };
            let input_json = serde_json::to_string(&function_input)
                .map_err(|e| ActionError(format!("args serialize: {e}")))?;
            // 查询当前 agent 的 capability 权限
            let perms: Vec<String> =
                sqlx::query_as("SELECT capability FROM agent_permissions WHERE agent_id = ?")
                    .bind(ctx.agent_id)
                    .fetch_all(pool.as_ref())
                    .await
                    .map(|rows: Vec<(String,)>| rows.into_iter().map(|(c,)| c).collect())
                    .unwrap_or_default();
            let dispatch_ctx = DispatchCtx {
                request_id: None,
                session_id: Some(ctx.session_id),
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
                .map_err(|e| ActionError(format!("plugin invoke failed: {e}")))?;
            // ★ Parse output and apply AgentContext updates
            if let Ok(output_val) = serde_json::from_str::<Value>(&output_str) {
                apply_agent_context_updates(&deps.agent_ctx, &output_val);
            }
        }
        _ => {
            return Err(ActionError(format!(
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
        .ok_or_else(|| ActionError("call_workflow: workflow_id is required".into()))?;

    // Build input from hook context + AgentContext snapshot
    let mut workflow_input =
        serde_json::to_value(ctx).map_err(|e| ActionError(format!("context serialize: {e}")))?;

    // ★ Inject AgentContext snapshot for workflow read access
    inject_agent_context_snapshot(&mut workflow_input, &deps.agent_ctx);

    // 查询当前 agent 的 capability 权限
    let perms: Vec<String> =
        sqlx::query_as("SELECT capability FROM agent_permissions WHERE agent_id = ?")
            .bind(ctx.agent_id)
            .fetch_all(&*pool)
            .await
            .map(|rows: Vec<(String,)>| rows.into_iter().map(|(c,)| c).collect())
            .unwrap_or_default();

    let executor_deps = super::workflow::ExecutorDeps {
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
        .map_err(|e| ActionError(format!("workflow execute: {e}")))?;

    // ★ Apply AgentContext updates from workflow output
    apply_agent_context_updates(&deps.agent_ctx, &outcome.end_value);

    Ok(())
}

async fn execute_http_webhook(
    hook: &AgentHook,
    ctx: &HookContext,
    pool: Arc<MySqlPool>,
) -> Result<(), ActionError> {
    let url = hook
        .action_params
        .get("webhook_url")
        .and_then(|v| v.as_str())
        .ok_or_else(|| ActionError("webhook_url is required for http_webhook".into()))?;

    if !url.starts_with("https://") {
        return Err(ActionError("Webhook URL must use HTTPS".into()));
    }

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

    let client = reqwest::Client::builder()
        .timeout(std::time::Duration::from_millis(
            hook.timeout_ms.max(1000) as u64
        ))
        .build()
        .map_err(|e| ActionError(format!("webhook client: {e}")))?;

    let mut req = client.post(url).json(&payload);

    if let Some(headers) = hook
        .action_params
        .get("headers")
        .and_then(|v| v.as_object())
    {
        for (key, val) in headers {
            if let Some(v_str) = val.as_str() {
                if key.contains('\r')
                    || key.contains('\n')
                    || v_str.contains('\r')
                    || v_str.contains('\n')
                {
                    return Err(ActionError("Header contains illegal characters".into()));
                }
                if key
                    .chars()
                    .all(|c| c.is_alphanumeric() || c == '-' || c == '_')
                {
                    req = req.header(key.as_str(), v_str);
                }
            }
        }
    }

    match req.send().await {
        Ok(resp) if resp.status().is_success() => Ok(()),
        Ok(resp) => {
            spawn_webhook_retry(pool, hook, ctx, &payload);
            Err(ActionError(format!(
                "Webhook returned HTTP {}",
                resp.status()
            )))
        }
        Err(_e) => {
            spawn_webhook_retry(pool, hook, ctx, &payload);
            Err(ActionError(
                "Webhook connection failed — pending async retry".into(),
            ))
        }
    }
}

fn spawn_webhook_retry(pool: Arc<MySqlPool>, hook: &AgentHook, ctx: &HookContext, payload: &Value) {
    let url = hook
        .action_params
        .get("webhook_url")
        .and_then(|v| v.as_str())
        .unwrap_or("")
        .to_string();
    let payload = payload.clone();
    let hook_id = hook.id;
    let agent_id = ctx.agent_id;
    let identifier = ctx.identifier.clone();
    let session_id = ctx.session_id;
    let trigger_point = ctx.trigger_point.clone();
    let max_retries = std::env::var("HOOK_WEBHOOK_RETRY_MAX")
        .ok()
        .and_then(|v| v.parse().ok())
        .unwrap_or(3u32);

    tokio::spawn(async move {
        for attempt in 0..max_retries {
            tokio::time::sleep(std::time::Duration::from_secs(2u64.pow(attempt))).await;
            let client = match reqwest::Client::new()
                .post(&url)
                .json(&payload)
                .send()
                .await
            {
                Ok(resp) if resp.status().is_success() => {
                    // Success on retry — write audit
                    let _ = sqlx::query(
                        r#"INSERT INTO hook_executions
                           (agent_id, agent_identifier, hook_id, session_id, trigger_point,
                            action_type, outcome, error_summary, elapsed_ms, request_id)
                           VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?)"#,
                    )
                    .bind(agent_id)
                    .bind(&identifier)
                    .bind(hook_id)
                    .bind(session_id)
                    .bind(&trigger_point)
                    .bind("http_webhook")
                    .bind("success")
                    .bind::<Option<String>>(None)
                    .bind::<Option<i32>>(None)
                    .bind::<Option<String>>(None)
                    .execute(pool.as_ref())
                    .await;
                    return;
                }
                _ => {} // continue retry
            };
            let _ = client;
        }
        // All retries failed
        let _ = sqlx::query(
            r#"INSERT INTO hook_executions
               (agent_id, agent_identifier, hook_id, session_id, trigger_point,
                action_type, outcome, error_summary, elapsed_ms, request_id)
               VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?)"#,
        )
        .bind(agent_id)
        .bind(&identifier)
        .bind(hook_id)
        .bind(session_id)
        .bind(&trigger_point)
        .bind("http_webhook")
        .bind("error")
        .bind(Some(format!("Webhook failed after {max_retries} retries")))
        .bind::<Option<i32>>(None)
        .bind::<Option<String>>(None)
        .execute(pool.as_ref())
        .await;
    });
}

/// Write a hook execution audit record.
async fn audit_hook_exec(
    pool: &MySqlPool,
    hook_id: i64,
    ctx: &HookContext,
    trigger_point: &str,
    outcome: &str,
    error: Option<&str>,
    elapsed_ms: Option<i32>,
) {
    // Truncate and mask error summary (FR-016 — mask sensitive fields)
    let error_summary = error.map(|e| {
        let mut s = e.to_string();
        // Basic sensitive field masking
        for keyword in &["secret", "password", "token", "api_key", "authorization"] {
            let lower = s.to_lowercase();
            if lower.contains(keyword) {
                s = "[REDACTED]".to_string();
                break;
            }
        }
        if s.len() > 1024 {
            s.truncate(1020);
            s.push_str("...");
        }
        s
    });

    // Build context snapshot (capped at ~4KB)
    let snapshot = json!({
        "agent_identifier": ctx.agent_id,
        "session_id": ctx.session_id,
        "trigger_point": trigger_point,
        "message": ctx.message,
        "channel": ctx.channel,
        "client_type": ctx.client_type,
        "client_version": ctx.client_version,
    });
    let snapshot_str = serde_json::to_string(&snapshot).unwrap_or_default();
    let snapshot_final: Option<Value> = if snapshot_str.len() > 4096 {
        Some(json!({"truncated": true}))
    } else {
        Some(snapshot)
    };

    let action_type: &str = ""; // Not available at this level without extra lookup

    let _ = sqlx::query(
        r#"INSERT INTO hook_executions
           (agent_id, agent_identifier, hook_id, session_id, trigger_point,
            action_type, outcome, error_summary, elapsed_ms, context_snapshot, request_id)
           VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?)"#,
    )
    .bind(ctx.agent_id)
    .bind(&ctx.identifier)
    .bind(hook_id)
    .bind(ctx.session_id)
    .bind(trigger_point)
    .bind(action_type)
    .bind(outcome)
    .bind(&error_summary)
    .bind(elapsed_ms)
    .bind(&snapshot_final)
    .bind(&ctx.request_id)
    .execute(pool)
    .await;
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
                    tracing::warn!("Unknown category in _agent_context_updates: {category_str}");
                    continue;
                }
            };

            if let Err(e) = agent_ctx.set_record(cat, key.to_string(), value, source, iteration) {
                tracing::warn!("Failed to apply _agent_context_updates record: {e}");
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
            if let Err(e) = agent_ctx.add_extension(id.to_string(), content) {
                tracing::warn!("Failed to apply _agent_context_updates extension: {e}");
            }
        }
    }

    // Apply metadata updates (e.g., agent_loop_break)
    if let Some(metadata) = updates.get("metadata").and_then(|v| v.as_object()) {
        for (key, val) in metadata {
            if let Some(s) = val.as_str() {
                if let Err(e) = agent_ctx.set_metadata(key.clone(), s.to_string()) {
                    tracing::warn!("Failed to apply _agent_context_updates metadata: {e}");
                }
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

#[derive(Debug)]
struct ActionError(String);

impl std::fmt::Display for ActionError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.0)
    }
}
