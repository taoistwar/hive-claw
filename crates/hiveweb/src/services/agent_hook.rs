use sqlx::MySqlPool;
use serde::Serialize;

use crate::models::agent_hook::{
    AgentHook, CreateHookRequest, HookExecution, HookExecutionQuery, UpdateHookRequest,
};
use crate::services::agent::MAIN_AGENT_IDENTIFIER;
use crate::utils::error::{AppError, codes};
use crate::services::optimistic_lock;

/// Enriched Hook response with resolved reference names for call_function/call_workflow.
#[derive(Debug, Clone, Serialize)]
pub struct AgentHookEnriched {
    #[serde(flatten)]
    pub hook: AgentHook,
    pub function_name: Option<String>,
    pub workflow_name: Option<String>,
}

/// Create a new Hook for the given agent.
pub async fn create_hook(
    pool: &MySqlPool,
    agent_id: i64,
    actor_role: i8,
    meta: CreateHookRequest,
) -> Result<AgentHook, AppError> {
    // ── Permission check ──
    check_hook_permission(pool, agent_id, actor_role).await?;

    // ── Per-trigger-point limit check (FR-001: max 5) ──
    let count: (i64,) = sqlx::query_as(
        "SELECT COUNT(*) FROM agent_hooks WHERE agent_id = ? AND trigger_point = ?",
    )
    .bind(agent_id)
    .bind(&meta.trigger_point)
    .fetch_one(pool)
    .await
    .map_err(|e| AppError::Internal(format!("hook count: {e}")))?;

    if count.0 >= 5 {
        return Err(AppError::HookTriggerLimitExceeded(
            "该触发点最多配置 5 个 Hook".into(),
        ));
    }

    // ── Reference validation for call_function / call_workflow (FR-006) ──
    match meta.action_type.as_str() {
        "call_function" => {
            let function_id = meta
                .action_params
                .get("function_id")
                .and_then(|v| v.as_i64());
            if let Some(fid) = function_id {
                let exists: Option<(i64,)> = sqlx::query_as(
                    "SELECT id FROM functions WHERE id = ?",
                )
                .bind(fid)
                .fetch_optional(pool)
                .await
                .map_err(|e| AppError::Internal(format!("function lookup: {e}")))?;
                if exists.is_none() {
                    return Err(AppError::HookReferenceInvalid(
                        "Hook 引用的 Function 不存在或已删除".into(),
                    ));
                }
            }
        }
        "call_workflow" => {
            let workflow_id = meta
                .action_params
                .get("workflow_id")
                .and_then(|v| v.as_i64());
            if let Some(wid) = workflow_id {
                let exists: Option<(i64,)> =
                    sqlx::query_as("SELECT id FROM workflows WHERE id = ?")
                        .bind(wid)
                        .fetch_optional(pool)
                        .await
                        .map_err(|e| AppError::Internal(format!("workflow lookup: {e}")))?;
                if exists.is_none() {
                    return Err(AppError::HookReferenceInvalid(
                        "Hook 引用的 Workflow 不存在或已删除".into(),
                    ));
                }
            }
        }
        "http_webhook" => {
            // FR-007: Validate URL
            let url = meta
                .action_params
                .get("webhook_url")
                .and_then(|v| v.as_str())
                .unwrap_or("");
            validate_webhook_url(url)?;
            // FR-007b: Validate HTTP headers (prevent header injection)
            validate_webhook_headers(&meta.action_params)?;
        }
        _ => {
            return Err(AppError::BadRequest(format!(
                "不支持的动作类型: {}",
                meta.action_type
            )));
        }
    }

    // ── Insert ──
    let params = serde_json::to_string(&meta.action_params)
        .map_err(|e| AppError::BadRequest(format!("action_params: {e}")))?;

    let result = sqlx::query(
        r#"INSERT INTO agent_hooks
           (agent_id, name, description, trigger_point, action_type, action_params,
            enabled, sort_order, blocking_mode, timeout_ms)
           VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?)"#,
    )
    .bind(agent_id)
    .bind(&meta.name)
    .bind(&meta.description)
    .bind(&meta.trigger_point)
    .bind(&meta.action_type)
    .bind(&params)
    .bind(meta.enabled)
    .bind(meta.sort_order)
    .bind(meta.blocking_mode)
    .bind(meta.timeout_ms)
    .execute(pool)
    .await
    .map_err(|e| AppError::Internal(format!("hook insert: {e}")))?;

    let hook = sqlx::query_as::<_, AgentHook>(
        "SELECT * FROM agent_hooks WHERE id = ?",
    )
    .bind(result.last_insert_id() as i64)
    .fetch_one(pool)
    .await
    .map_err(|e| AppError::Internal(format!("hook fetch after insert: {e}")))?;

    Ok(hook)
}

/// List all hooks for an agent, ordered by trigger_point + sort_order.
pub async fn list_hooks(pool: &MySqlPool, agent_id: i64) -> Result<Vec<AgentHook>, AppError> {
    sqlx::query_as::<_, AgentHook>(
        r#"SELECT * FROM agent_hooks
           WHERE agent_id = ?
           ORDER BY trigger_point, sort_order"#,
    )
    .bind(agent_id)
    .fetch_all(pool)
    .await
    .map_err(|e| AppError::Internal(format!("list hooks: {e}")))
}

/// Update an existing Hook with optimistic lock.
pub async fn update_hook(
    pool: &MySqlPool,
    agent_id: i64,
    hook_id: i64,
    actor_role: i8,
    meta: UpdateHookRequest,
) -> Result<AgentHook, AppError> {
    // ── Permission check ──
    check_hook_permission(pool, agent_id, actor_role).await?;

    // ── Load existing hook ──
    let existing = sqlx::query_as::<_, AgentHook>("SELECT * FROM agent_hooks WHERE id = ?")
        .bind(hook_id)
        .fetch_optional(pool)
        .await
        .map_err(|e| AppError::Internal(format!("hook fetch: {e}")))?;

    let existing =
        existing.ok_or_else(|| AppError::HookNotFound("Hook 配置不存在".into()))?;

    if existing.agent_id != agent_id {
        return Err(AppError::HookNotFound("Hook 配置不存在".into()));
    }

    // ── Optimistic lock ──
    optimistic_lock::check_and_bump(pool, "agent_hooks", hook_id, meta.updated_at)
        .await
        .map_err(|_| {
            AppError::OptimisticLockConflict("Hook 配置已被他人修改，请刷新后重试".into())
        })?;

    // ── Apply updates (COALESCE-style: use meta value if Some, else use existing) ──
    let trigger_point_changed = meta.trigger_point.is_some()
        && meta.trigger_point.as_ref() != Some(&existing.trigger_point);
    let action_type_changed = meta.action_type.is_some()
        && meta.action_type.as_ref() != Some(&existing.action_type);

    // Re-validate references / URL if action_type changed (before moving out of meta)
    if action_type_changed {
        let new_at = meta.action_type.as_deref().unwrap_or(&existing.action_type);
        validate_hook_action(pool, new_at, &meta).await?;
    }

    let name = meta.name.unwrap_or(existing.name);
    let description = meta.description.unwrap_or(existing.description);
    let trigger_point = meta.trigger_point.unwrap_or(existing.trigger_point);
    let action_type = meta.action_type.unwrap_or(existing.action_type);

    // Per-trigger-point limit recheck if trigger_point changed
    if trigger_point_changed {
        let count: (i64,) = sqlx::query_as(
            "SELECT COUNT(*) FROM agent_hooks WHERE agent_id = ? AND trigger_point = ? AND id != ?",
        )
        .bind(agent_id)
        .bind(&trigger_point)
        .bind(hook_id)
        .fetch_one(pool)
        .await
        .map_err(|e| AppError::Internal(format!("hook count: {e}")))?;
        if count.0 >= 5 {
            return Err(AppError::HookTriggerLimitExceeded(
                "该触发点最多配置 5 个 Hook".into(),
            ));
        }
    }

    let action_params = meta
        .action_params
        .unwrap_or(existing.action_params);
    let params_str = serde_json::to_string(&action_params)
        .map_err(|e| AppError::BadRequest(format!("action_params: {e}")))?;
    let enabled = meta.enabled.unwrap_or(existing.enabled);
    let sort_order = meta.sort_order.unwrap_or(existing.sort_order);
    let blocking_mode = meta.blocking_mode.unwrap_or(existing.blocking_mode);
    let timeout_ms = meta.timeout_ms.unwrap_or(existing.timeout_ms);

    let updated = sqlx::query_as::<_, AgentHook>(
        r#"UPDATE agent_hooks SET
            name = ?, description = ?, trigger_point = ?, action_type = ?,
            action_params = ?, enabled = ?, sort_order = ?, blocking_mode = ?,
            timeout_ms = ?
           WHERE id = ?"#,
    )
    .bind(&name)
    .bind(&description)
    .bind(&trigger_point)
    .bind(&action_type)
    .bind(&params_str)
    .bind(enabled)
    .bind(sort_order)
    .bind(blocking_mode)
    .bind(timeout_ms)
    .bind(hook_id)
    .fetch_one(pool)
    .await
    .map_err(|e| AppError::Internal(format!("hook update: {e}")))?;

    Ok(updated)
}

/// Delete a Hook configuration.
pub async fn delete_hook(
    pool: &MySqlPool,
    agent_id: i64,
    hook_id: i64,
    actor_role: i8,
) -> Result<(), AppError> {
    check_hook_permission(pool, agent_id, actor_role).await?;

    let result = sqlx::query("DELETE FROM agent_hooks WHERE id = ? AND agent_id = ?")
        .bind(hook_id)
        .bind(agent_id)
        .execute(pool)
        .await
        .map_err(|e| AppError::Internal(format!("hook delete: {e}")))?;

    if result.rows_affected() == 0 {
        return Err(AppError::HookNotFound("Hook 配置不存在".into()));
    }

    Ok(())
}

/// Load hooks for orchestrator use: grouped by trigger_point, sorted by sort_order.
pub async fn load_hooks_for_agent(
    pool: &MySqlPool,
    agent_id: i64,
) -> Result<std::collections::HashMap<String, Vec<AgentHook>>, String> {
    let hooks: Vec<AgentHook> = sqlx::query_as(
        r#"SELECT * FROM agent_hooks
           WHERE agent_id = ? AND enabled = TRUE
           ORDER BY trigger_point, sort_order"#,
    )
    .bind(agent_id)
    .fetch_all(pool)
    .await
    .map_err(|e| format!("load hooks: {e}"))?;

    let mut map: std::collections::HashMap<String, Vec<AgentHook>> =
        std::collections::HashMap::new();
    for hook in hooks {
        map.entry(hook.trigger_point.clone())
            .or_default()
            .push(hook);
    }
    Ok(map)
}

/// List hook executions with filtering and pagination.
pub async fn list_executions(
    pool: &MySqlPool,
    query: HookExecutionQuery,
) -> Result<(Vec<HookExecution>, u64), AppError> {
    let mut where_clauses: Vec<String> = Vec::new();
    let mut params: Vec<String> = Vec::new();

    if let Some(aid) = query.agent_id {
        where_clauses.push("agent_id = ?".into());
        params.push(aid.to_string());
    }
    if let Some(ref tp) = query.trigger_point {
        where_clauses.push("trigger_point = ?".into());
        params.push(tp.clone());
    }
    if let Some(ref o) = query.outcome {
        where_clauses.push("outcome = ?".into());
        params.push(o.clone());
    }
    if let Some(ref from) = query.from {
        where_clauses.push("created_at >= ?".into());
        params.push(from.clone());
    }
    if let Some(ref to) = query.to {
        where_clauses.push("created_at <= ?".into());
        params.push(to.clone());
    }

    let where_sql = if where_clauses.is_empty() {
        String::new()
    } else {
        format!("WHERE {}", where_clauses.join(" AND "))
    };

    // Count
    let count_sql = format!("SELECT COUNT(*) FROM hook_executions {where_sql}");
    let mut count_query = sqlx::query_scalar(&count_sql);
    for p in &params {
        count_query = count_query.bind(p);
    }
    let total: i64 = count_query
        .fetch_one(pool)
        .await
        .map_err(|e| AppError::Internal(format!("count executions: {e}")))?;

    // Data
    let offset = ((query.page.saturating_sub(1)) * query.page_size) as i64;
    let limit = query.page_size as i64;
    let data_sql = format!(
        "SELECT * FROM hook_executions {where_sql} ORDER BY created_at DESC LIMIT ? OFFSET ?"
    );
    let mut data_query = sqlx::query_as::<_, HookExecution>(&data_sql);
    for p in &params {
        data_query = data_query.bind(p);
    }
    let items = data_query
        .bind(limit)
        .bind(offset)
        .fetch_all(pool)
        .await
        .map_err(|e| AppError::Internal(format!("list executions: {e}")))?;

    Ok((items, total as u64))
}

// ── Helpers ──

async fn check_hook_permission(
    pool: &MySqlPool,
    agent_id: i64,
    actor_role: i8,
) -> Result<(), AppError> {
    let ident: (String,) =
        sqlx::query_as("SELECT identifier FROM agents WHERE id = ?")
            .bind(agent_id)
            .fetch_optional(pool)
            .await
            .map_err(|e| AppError::Internal(format!("agent lookup: {e}")))?
            .ok_or_else(|| AppError::NotFound("Agent 不存在".into()))?;

    // FR-017: main agent → Super only
    if ident.0 == MAIN_AGENT_IDENTIFIER && actor_role != 3 {
        return Err(AppError::InsufficientPermission(
            "入口 Agent「main」的 Hook 配置仅 Super 可操作".into(),
        ));
    }

    // FR-018: non-main agent → System+ (role 2 or 3)
    if actor_role < 2 {
        return Err(AppError::InsufficientPermission(
            "Hook 配置需 System 及以上角色".into(),
        ));
    }

    Ok(())
}

/// List hooks enriched with resolved Function/Workflow names (T047).
pub async fn list_hooks_enriched(
    pool: &MySqlPool,
    agent_id: i64,
) -> Result<Vec<AgentHookEnriched>, AppError> {
    let hooks = list_hooks(pool, agent_id).await?;
    let mut enriched = Vec::with_capacity(hooks.len());
    for hook in hooks {
        let mut fn_name: Option<String> = None;
        let mut wf_name: Option<String> = None;

        if hook.action_type == "call_function" {
            if let Some(fid) = hook.action_params.get("function_id").and_then(|v| v.as_i64()) {
                fn_name = sqlx::query_scalar("SELECT name FROM functions WHERE id = ?")
                    .bind(fid)
                    .fetch_optional(pool)
                    .await
                    .unwrap_or(None);
            }
        } else if hook.action_type == "call_workflow" {
            if let Some(wid) = hook.action_params.get("workflow_id").and_then(|v| v.as_i64()) {
                wf_name = sqlx::query_scalar("SELECT name FROM workflows WHERE id = ?")
                    .bind(wid)
                    .fetch_optional(pool)
                    .await
                    .unwrap_or(None);
            }
        }

        enriched.push(AgentHookEnriched {
            hook,
            function_name: fn_name,
            workflow_name: wf_name,
        });
    }
    Ok(enriched)
}

/// Cron cleanup: delete hook_executions older than retention days (T046).
pub async fn cleanup_old_executions(pool: &MySqlPool) {
    let retention_days: i64 = std::env::var("HOOK_EXECUTION_RETENTION_DAYS")
        .ok()
        .and_then(|v| v.parse().ok())
        .unwrap_or(30);

    let result = sqlx::query(
        "DELETE FROM hook_executions WHERE created_at < DATE_SUB(NOW(), INTERVAL ? DAY) LIMIT 1000",
    )
    .bind(retention_days)
    .execute(pool)
    .await;

    match result {
        Ok(r) if r.rows_affected() > 0 => {
            tracing::info!(rows = r.rows_affected(), retention_days, "Cleaned old hook execution records");
        }
        Err(e) => {
            tracing::warn!(error = %e, "Failed to clean old hook executions");
        }
        _ => {}
    }
}

fn validate_webhook_url(url: &str) -> Result<(), AppError> {
    if url.is_empty() {
        return Err(AppError::HookWebhookUrlInvalid("Webhook URL 不能为空".into()));
    }
    if !url.starts_with("https://") {
        return Err(AppError::HookWebhookUrlInvalid(
            "Webhook URL 仅支持 HTTPS 协议".into(),
        ));
    }

    // Basic SSRF guard: reject common private ranges
    let lower = url.to_lowercase();
    // Crude but effective: reject URLs pointing to private patterns
    let blocked = [
        "127.0.0.1", "localhost", "10.", "192.168.", "172.16.", "172.17.",
        "172.18.", "172.19.", "172.20.", "172.21.", "172.22.", "172.23.",
        "172.24.", "172.25.", "172.26.", "172.27.", "172.28.", "172.29.",
        "172.30.", "172.31.", "169.254.", "0.0.0.0",
        "metadata.google.internal", "169.254.169.254",
    ];
    for pat in &blocked {
        if lower.contains(pat) {
            return Err(AppError::HookWebhookUrlInvalid(format!(
                "Webhook URL 不允许指向内网地址: {}",
                pat
            )));
        }
    }

    Ok(())
}

/// FR-007b: Validate HTTP headers in webhook action_params.
/// Rejects header keys/values containing CR/LF and
/// ensures header names only use [a-zA-Z0-9_-].
fn validate_webhook_headers(action_params: &serde_json::Value) -> Result<(), AppError> {
    use regex::Regex;
    // header name: [a-zA-Z0-9_-]+
    let name_re = Regex::new(r"^[a-zA-Z0-9_-]+$")
        .map_err(|_| AppError::Internal("header regex compile".into()))?;
    // reject \r or \n in header name or value
    let injection_re = Regex::new(r"[\r\n]")
        .map_err(|_| AppError::Internal("injection regex compile".into()))?;

    if let Some(headers) = action_params.get("headers").and_then(|v| v.as_object()) {
        for (name, value) in headers {
            let val_str = value.as_str().unwrap_or("");
            if injection_re.is_match(name) || injection_re.is_match(val_str) {
                return Err(AppError::HookWebhookUrlInvalid(format!(
                    "HTTP header 不允许包含回车或换行符: header '{}'",
                    name
                )));
            }
            if !name_re.is_match(name) {
                return Err(AppError::HookWebhookUrlInvalid(format!(
                    "HTTP header 名称仅允许 [a-zA-Z0-9_-] 字符: '{}'",
                    name
                )));
            }
        }
    }
    Ok(())
}

async fn validate_hook_action(
    pool: &MySqlPool,
    action_type: &str,
    meta: &UpdateHookRequest,
) -> Result<(), AppError> {
    if let Some(ref params) = meta.action_params {
        match action_type {
            "call_function" => {
                if let Some(fid) = params.get("function_id").and_then(|v| v.as_i64()) {
                    let exists: Option<(i64,)> = sqlx::query_as(
                        "SELECT id FROM functions WHERE id = ?",
                    )
                    .bind(fid)
                    .fetch_optional(pool)
                    .await
                    .map_err(|e| AppError::Internal(format!("function lookup: {e}")))?;
                    if exists.is_none() {
                        return Err(AppError::HookReferenceInvalid(
                            "Hook 引用的 Function 不存在或已删除".into(),
                        ));
                    }
                }
            }
            "call_workflow" => {
                if let Some(wid) = params.get("workflow_id").and_then(|v| v.as_i64()) {
                    let exists: Option<(i64,)> = sqlx::query_as(
                        "SELECT id FROM workflows WHERE id = ?",
                    )
                    .bind(wid)
                    .fetch_optional(pool)
                    .await
                    .map_err(|e| AppError::Internal(format!("workflow lookup: {e}")))?;
                    if exists.is_none() {
                        return Err(AppError::HookReferenceInvalid(
                            "Hook 引用的 Workflow 不存在或已删除".into(),
                        ));
                    }
                }
            }
            "http_webhook" => {
                let url = params
                    .get("webhook_url")
                    .and_then(|v| v.as_str())
                    .unwrap_or("");
                validate_webhook_url(url)?;
            }
            _ => {}
        }
    }
    Ok(())
}
