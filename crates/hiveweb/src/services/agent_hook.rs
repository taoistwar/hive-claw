use serde::Serialize;
use sqlx::MySqlPool;

use crate::cache::redis::RedisClient;
use crate::models::agent_hook::{AgentHook, CreateHookRequest, UpdateHookRequest};
use crate::services::agent::MAIN_AGENT_IDENTIFIER;
use crate::services::optimistic_lock;
use crate::services::optimistic_lock::OptimisticLockTable;
use crate::utils::error::AppError;

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
    redis: &RedisClient,
    agent_id: i64,
    actor_role: i8,
    meta: CreateHookRequest,
) -> Result<AgentHook, AppError> {
    // ── Permission check ──
    check_hook_permission(pool, agent_id, actor_role).await?;

    // ── Per-trigger-point limit check (FR-001: max 5) ──
    let count: (i64,) =
        sqlx::query_as("SELECT COUNT(*) FROM agent_hooks WHERE agent_id = ? AND trigger_point = ?")
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

    // ── Reference / outbound target validation ──
    validate_hook_action(pool, &meta.action_type, &meta.action_params).await?;

    // ── Insert ──
    let params = serde_json::to_string(&meta.action_params)
        .map_err(|e| AppError::BadRequest(format!("action_params: {e}")))?;

    let mutation_result = sqlx::query(
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
    .map_err(|e| AppError::Internal(format!("hook insert: {e}")));
    let result = invalidate_agent_content_after_success(mutation_result, || {
        crate::services::agent::invalidate_content_cache(redis, agent_id)
    })
    .await?;

    let hook = sqlx::query_as::<_, AgentHook>("SELECT * FROM agent_hooks WHERE id = ?")
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
    redis: &RedisClient,
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

    let existing = existing.ok_or_else(|| AppError::HookNotFound("Hook 配置不存在".into()))?;

    if existing.agent_id != agent_id {
        return Err(AppError::HookNotFound("Hook 配置不存在".into()));
    }

    // ── Apply updates (COALESCE-style: use meta value if Some, else use existing) ──
    let trigger_point_changed = meta.trigger_point.is_some()
        && meta.trigger_point.as_ref() != Some(&existing.trigger_point);
    let action_definition_changed = meta.action_type.is_some() || meta.action_params.is_some();
    let action_type = meta
        .action_type
        .clone()
        .unwrap_or_else(|| existing.action_type.clone());
    let action_params = meta
        .action_params
        .clone()
        .unwrap_or_else(|| existing.action_params.clone());

    // Always validate the merged action definition when either half changes.
    // This prevents an update that changes only action_params from bypassing
    // URL/DNS/header checks for an existing http_webhook action.
    if action_definition_changed {
        validate_hook_action(pool, &action_type, &action_params).await?;
    }

    // Validation is side-effect free; only bump the optimistic version after
    // the candidate action has passed policy.
    optimistic_lock::check_and_bump(
        pool,
        OptimisticLockTable::AgentHooks,
        hook_id,
        meta.updated_at,
    )
    .await
    .map_err(|_| AppError::OptimisticLockConflict("Hook 配置已被他人修改，请刷新后重试".into()))?;

    let name = meta.name.unwrap_or(existing.name);
    let description = meta.description.unwrap_or(existing.description);
    let trigger_point = meta.trigger_point.unwrap_or(existing.trigger_point);

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

    let params_str = serde_json::to_string(&action_params)
        .map_err(|e| AppError::BadRequest(format!("action_params: {e}")))?;
    let enabled = meta.enabled.unwrap_or(existing.enabled);
    let sort_order = meta.sort_order.unwrap_or(existing.sort_order);
    let blocking_mode = meta.blocking_mode.unwrap_or(existing.blocking_mode);
    let timeout_ms = meta.timeout_ms.unwrap_or(existing.timeout_ms);

    let result = sqlx::query(
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
    .execute(pool)
    .await
    .map_err(|e| AppError::Internal(format!("hook update: {e}")))?;

    let mutation_result = if result.rows_affected() == 0 {
        Err(AppError::HookNotFound("Hook 配置不存在".into()))
    } else {
        Ok(())
    };
    invalidate_agent_content_after_success(mutation_result, || {
        crate::services::agent::invalidate_content_cache(redis, agent_id)
    })
    .await?;

    let updated = sqlx::query_as::<_, AgentHook>("SELECT * FROM agent_hooks WHERE id = ?")
        .bind(hook_id)
        .fetch_one(pool)
        .await
        .map_err(|e| AppError::Internal(format!("hook fetch after update: {e}")))?;

    Ok(updated)
}

/// Delete a Hook configuration.
pub async fn delete_hook(
    pool: &MySqlPool,
    redis: &RedisClient,
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

    let mutation_result = if result.rows_affected() == 0 {
        Err(AppError::HookNotFound("Hook 配置不存在".into()))
    } else {
        Ok(())
    };
    invalidate_agent_content_after_success(mutation_result, || {
        crate::services::agent::invalidate_content_cache(redis, agent_id)
    })
    .await
}

async fn invalidate_agent_content_after_success<T, E, F, Fut>(
    result: Result<T, E>,
    invalidate: F,
) -> Result<T, E>
where
    F: FnOnce() -> Fut,
    Fut: std::future::Future<Output = ()>,
{
    if result.is_ok() {
        invalidate().await;
    }
    result
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

// ── Helpers ──

async fn check_hook_permission(
    pool: &MySqlPool,
    agent_id: i64,
    actor_role: i8,
) -> Result<(), AppError> {
    let ident: (String,) = sqlx::query_as("SELECT identifier FROM agents WHERE id = ?")
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
            if let Some(fid) = hook
                .action_params
                .get("function_id")
                .and_then(|v| v.as_i64())
            {
                fn_name = sqlx::query_scalar("SELECT name FROM functions WHERE id = ?")
                    .bind(fid)
                    .fetch_optional(pool)
                    .await
                    .unwrap_or(None);
            }
        } else if hook.action_type == "call_workflow"
            && let Some(wid) = hook
                .action_params
                .get("workflow_id")
                .and_then(|v| v.as_i64())
        {
            wf_name = sqlx::query_scalar("SELECT name FROM workflows WHERE id = ?")
                .bind(wid)
                .fetch_optional(pool)
                .await
                .unwrap_or(None);
        }

        enriched.push(AgentHookEnriched {
            hook,
            function_name: fn_name,
            workflow_name: wf_name,
        });
    }
    Ok(enriched)
}

async fn validate_webhook_url(url: &str) -> Result<(), AppError> {
    if url.is_empty() {
        return Err(AppError::HookWebhookUrlInvalid(
            "Webhook URL 不能为空".into(),
        ));
    }
    crate::runtime::capabilities::network_http::resolve_outbound_target(
        url,
        crate::runtime::capabilities::network_http::OutboundScheme::HttpsOnly,
    )
    .await
    .map(|_| ())
    .map_err(map_webhook_target_error)
}

fn map_webhook_target_error(
    _error: crate::runtime::capabilities::network_http::OutboundTargetError,
) -> AppError {
    AppError::HookWebhookUrlInvalid("Webhook URL 必须使用 HTTPS，且目标必须解析到公网地址".into())
}

/// FR-007b: Validate HTTP headers in webhook action_params.
/// Uses the same parser as the runtime request path and rejects Host override.
fn validate_webhook_headers(action_params: &serde_json::Value) -> Result<(), AppError> {
    if let Some(raw_headers) = action_params.get("headers") {
        let headers = raw_headers.as_object().ok_or_else(|| {
            AppError::HookWebhookUrlInvalid("HTTP headers 必须是字符串键值对象".into())
        })?;
        for (name, value) in headers {
            let value = value.as_str().ok_or_else(|| {
                AppError::HookWebhookUrlInvalid("HTTP header 值必须是字符串".into())
            })?;
            crate::runtime::capabilities::network_http::parse_outbound_header(name, value)
                .map_err(|_| {
                    AppError::HookWebhookUrlInvalid(
                        "HTTP header 名称或值非法，且不得覆盖 Host".into(),
                    )
                })?;
        }
    }
    Ok(())
}

async fn validate_hook_action(
    pool: &MySqlPool,
    action_type: &str,
    params: &serde_json::Value,
) -> Result<(), AppError> {
    match action_type {
        "call_function" => {
            if let Some(fid) = params.get("function_id").and_then(|v| v.as_i64()) {
                let exists: Option<(i64,)> =
                    sqlx::query_as("SELECT id FROM functions WHERE id = ?")
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
            let url = params
                .get("webhook_url")
                .and_then(|v| v.as_str())
                .unwrap_or("");
            validate_webhook_url(url).await?;
            validate_webhook_headers(params)?;
        }
        _ => {
            return Err(AppError::BadRequest(format!(
                "不支持的动作类型: {action_type}"
            )));
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use std::sync::atomic::{AtomicUsize, Ordering};

    use super::{
        invalidate_agent_content_after_success, map_webhook_target_error, validate_webhook_headers,
    };
    use crate::runtime::capabilities::network_http::OutboundTargetError;
    use serde_json::json;

    #[tokio::test]
    async fn successful_hook_mutation_invalidates_once_and_preserves_result() {
        let invalidations = AtomicUsize::new(0);

        let result = invalidate_agent_content_after_success::<_, &str, _, _>(Ok(42), || async {
            invalidations.fetch_add(1, Ordering::Relaxed);
        })
        .await;

        assert_eq!(result, Ok(42));
        assert_eq!(invalidations.load(Ordering::Relaxed), 1);
    }

    #[tokio::test]
    async fn failed_hook_mutation_skips_invalidation_and_preserves_error() {
        let invalidations = AtomicUsize::new(0);

        let result =
            invalidate_agent_content_after_success(Err::<(), _>("mutation failed"), || async {
                invalidations.fetch_add(1, Ordering::Relaxed);
            })
            .await;

        assert_eq!(result, Err("mutation failed"));
        assert_eq!(invalidations.load(Ordering::Relaxed), 0);
    }

    #[test]
    fn all_hook_mutations_wire_cache_invalidation_after_the_database_write() {
        let source = include_str!("agent_hook.rs");
        let cases = [
            ("pub async fn create_hook(", "pub async fn list_hooks("),
            ("pub async fn update_hook(", "pub async fn delete_hook("),
            (
                "pub async fn delete_hook(",
                "async fn invalidate_agent_content_after_success",
            ),
        ];

        for (start, end) in cases {
            let body = source
                .split_once(start)
                .and_then(|(_, rest)| rest.split_once(end).map(|(body, _)| body))
                .unwrap_or_else(|| panic!("missing Hook mutation source range for {start}"));
            let write = body
                .find(".execute(pool)")
                .unwrap_or_else(|| panic!("{start} must execute its database mutation"));
            let invalidate = body
                .find("invalidate_agent_content_after_success(")
                .unwrap_or_else(|| panic!("{start} must invalidate AgentContent cache"));

            assert!(
                write < invalidate,
                "{start} must invalidate only after its database write"
            );
        }
    }

    #[test]
    fn update_revalidates_effective_action_when_only_action_params_change() {
        let source = include_str!("agent_hook.rs");
        let update_body = source
            .split_once("pub async fn update_hook(")
            .and_then(|(_, rest)| {
                rest.split_once("/// Delete a Hook configuration.")
                    .map(|(body, _)| body)
            })
            .expect("update_hook source range");

        assert!(
            update_body.contains("validate_hook_action(pool, &action_type, &action_params).await?"),
            "update_hook must validate the merged action_type/action_params even when only \
             action_params changes"
        );
    }

    #[test]
    fn webhook_header_contract_rejects_non_object_and_non_string_values() {
        assert!(validate_webhook_headers(&json!({"headers": []})).is_err());
        assert!(validate_webhook_headers(&json!({"headers": {"x-event": 7}})).is_err());
        assert!(validate_webhook_headers(&json!({"headers": {"x-event": "ready"}})).is_ok());
    }

    #[test]
    fn webhook_target_errors_map_to_static_6003_without_resolver_details() {
        for target_error in [
            OutboundTargetError::Policy("POLICY_DETAIL_SECRET"),
            OutboundTargetError::ResolveUnavailable("DNS_DETAIL_SECRET"),
        ] {
            let error = map_webhook_target_error(target_error);
            assert_eq!(error.code(), 6003);
            assert_eq!(
                error.message(),
                "Webhook URL 必须使用 HTTPS，且目标必须解析到公网地址"
            );
            assert!(!error.message().contains("SECRET"));
        }
    }
}
