//! Hook execution engine — runs agent lifecycle hooks during orchestrator execution.
//!
//! Hooks are **read-only observers**: results are written only to `hook_executions`
//! audit table and never injected into agent state (system_prompt, tools, etc.).

use chrono::Utc;
use serde_json::{Value, json};
use sqlx::MySqlPool;
use std::collections::HashMap;
use std::sync::Arc;
use std::time::Instant;

use crate::models::agent_hook::AgentHook;

/// Context passed to each hook invocation.
#[derive(Debug, Clone)]
pub struct HookContext {
    pub agent_id: i64,
    pub identifier: String,
    pub session_id: i64,
    pub actor_id: i64,
    pub request_id: String,
    pub trigger_point: String,
}

/// Run all enabled hooks for a given trigger point.
///
/// Non-blocking mode: individual failures do not prevent subsequent hooks.
/// Blocking mode: first failure aborts the agent flow and returns `Err`.
#[tracing::instrument(skip(pool, hooks, ctx), fields(
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
) -> Result<(), HookError> {
    let list = match hooks.get(point) {
        Some(h) => h,
        None => return Ok(()),
    };

    for hook in list {
        if !hook.enabled {
            // Record skipped
            audit_hook_exec(
                &pool,
                hook.id,
                ctx,
                point,
                "skipped",
                None,
                None,
            )
            .await;
            continue;
        }

        let start = Instant::now();
        let timeout_ms = hook.timeout_ms.max(1000) as u64;

        let result = tokio::time::timeout(
            std::time::Duration::from_millis(timeout_ms),
            execute_hook_action(hook, ctx, pool.clone()),
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
) -> Result<(), ActionError> {
    match hook.action_type.as_str() {
        "call_function" => Err(ActionError("call_function not yet integrated".into())),
        "call_workflow" => Err(ActionError("call_workflow not yet integrated".into())),
        "http_webhook" => {
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
            });

            let client = reqwest::Client::builder()
                .timeout(std::time::Duration::from_millis(hook.timeout_ms.max(1000) as u64))
                .build()
                .map_err(|e| ActionError(format!("webhook client: {e}")))?;

            let mut req = client.post(url).json(&payload);

            if let Some(headers) = hook.action_params.get("headers").and_then(|v| v.as_object()) {
                for (key, val) in headers {
                    if let Some(v_str) = val.as_str() {
                        if key.contains('\r') || key.contains('\n') || v_str.contains('\r') || v_str.contains('\n') {
                            return Err(ActionError("Header contains illegal characters".into()));
                        }
                        if key.chars().all(|c| c.is_alphanumeric() || c == '-' || c == '_') {
                            req = req.header(key.as_str(), v_str);
                        }
                    }
                }
            }

            match req.send().await {
                Ok(resp) if resp.status().is_success() => Ok(()),
                Ok(resp) => {
                    // Spawn async retry
                    spawn_webhook_retry(pool, hook, ctx, &payload);
                    Err(ActionError(format!("Webhook returned HTTP {}", resp.status())))
                }
                Err(_e) => {
                    spawn_webhook_retry(pool, hook, ctx, &payload);
                    // Return Err so blocking hooks can abort; retry handles async audit
                    Err(ActionError("Webhook connection failed — pending async retry".into()))
                }
            }
        }
        other => Err(ActionError(format!("Unknown action type: {other}"))),
    }
}

fn spawn_webhook_retry(
    pool: Arc<MySqlPool>,
    hook: &AgentHook,
    ctx: &HookContext,
    payload: &Value,
) {
    let url = hook.action_params.get("webhook_url")
        .and_then(|v| v.as_str()).unwrap_or("").to_string();
    let payload = payload.clone();
    let hook_id = hook.id;
    let agent_id = ctx.agent_id;
    let identifier = ctx.identifier.clone();
    let session_id = ctx.session_id;
    let trigger_point = ctx.trigger_point.clone();
    let max_retries = std::env::var("HOOK_WEBHOOK_RETRY_MAX")
        .ok().and_then(|v| v.parse().ok()).unwrap_or(3u32);

    tokio::spawn(async move {
        for attempt in 0..max_retries {
            tokio::time::sleep(std::time::Duration::from_secs(2u64.pow(attempt))).await;
            let client = match reqwest::Client::new().post(&url).json(&payload).send().await {
                Ok(resp) if resp.status().is_success() => {
                    // Success on retry — write audit
                    let _ = sqlx::query(
                        r#"INSERT INTO hook_executions
                           (agent_id, agent_identifier, hook_id, session_id, trigger_point,
                            action_type, outcome, error_summary, elapsed_ms, request_id)
                           VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?)"#,
                    )
                    .bind(agent_id).bind(&identifier).bind(hook_id).bind(session_id)
                    .bind(&trigger_point).bind("http_webhook").bind("success")
                    .bind::<Option<String>>(None).bind::<Option<i32>>(None)
                    .bind::<Option<String>>(None)
                    .execute(pool.as_ref()).await;
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
        .bind(agent_id).bind(&identifier).bind(hook_id).bind(session_id)
        .bind(&trigger_point).bind("http_webhook").bind("error")
        .bind(Some(format!("Webhook failed after {max_retries} retries")))
        .bind::<Option<i32>>(None).bind::<Option<String>>(None)
        .execute(pool.as_ref()).await;
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

// ── Error types ──

#[derive(Debug)]
pub enum HookError {
    BlockingFailed(String),
    Timeout(String),
}

impl std::fmt::Display for HookError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            HookError::BlockingFailed(s) | HookError::Timeout(s) => write!(f, "{s}"),
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
