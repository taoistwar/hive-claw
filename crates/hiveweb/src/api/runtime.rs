//! Runtime API — pool 统计 (T162) + custom function 调用入口
//!
//! `GET /api/runtime/pool/stats` ：Pool 健康度（FR-029 / contracts/api.md §9b）
//! `POST /api/functions/:id/invoke` ：把 Function 当 RPC 调一次（US4 演示用 +
//!  Workflow 节点也会复用此路径）

use axum::{
    Json, Router,
    extract::{Path, State},
    routing::{get, post},
};
use chrono::Utc;
use serde::Serialize;
use serde_json::Value;
use std::collections::HashMap;
use std::sync::Arc;

use agent::context::{AgentContext, ContextConfig, UserInput};

use crate::api::{AppState, require_s3};
use crate::runtime::capability::DispatchCtx;
use crate::runtime::pool::{PerPluginMetrics, PoolMetrics};
use crate::utils::error::{ApiResponse, AppError};

pub fn router() -> Router<AppState> {
    Router::new()
        .route("/runtime/pool/stats", get(pool_stats))
        .route("/functions/:id/invoke", post(invoke_function))
}

#[derive(Debug, Serialize)]
pub struct PoolStatsResp {
    pub global: PoolMetrics,
    pub per_plugin: Vec<PerPluginMetrics>,
}

async fn pool_stats(State(state): State<AppState>) -> ApiResponse<PoolStatsResp> {
    let global = state.runtime_state.pool.metrics_snapshot().await;
    let per_plugin = state.runtime_state.pool.per_plugin_snapshot().await;
    ApiResponse::success(PoolStatsResp { global, per_plugin })
}

#[derive(Debug, serde::Deserialize)]
pub struct InvokeBody {
    pub input: Value,
    /// 当前调用 Agent；用于 Capability 鉴权（默认 main agent id=1）
    #[serde(default = "default_agent_id")]
    pub agent_id: i64,
    /// 可选的用户上下文，用于依赖 AgentContext 的函数（如 query_balance）
    /// 当提供时，后台会构建 AgentContext 注入到函数执行中
    #[serde(default)]
    pub user_input: Option<InvokeUserInput>,
}

fn default_agent_id() -> i64 {
    1
}

/// 测试/调试时由前端配置的 UserInput 字段
#[derive(Debug, serde::Deserialize)]
pub struct InvokeUserInput {
    /// 用户原始输入文本
    #[serde(default)]
    pub raw_text: Option<String>,
    /// 用户/玩家 ID（对应 UserInput.metadata["actor_id"]）
    pub actor_id: Option<String>,
    /// 来源渠道（对应 UserInput.metadata["channel"]）
    pub channel: Option<String>,
    /// 平台（对应 UserInput.metadata["client_type"]）
    pub client_type: Option<String>,
    /// 应用版本（对应 UserInput.metadata["client_version"]）
    pub client_version: Option<String>,
}

#[derive(Debug, Serialize)]
pub struct InvokeResp {
    pub output: Value,
    pub elapsed_ms: i32,
}

async fn invoke_function(
    State(state): State<AppState>,
    Path(id): Path<i64>,
    Json(body): Json<InvokeBody>,
) -> Result<ApiResponse<InvokeResp>, ApiResponse<()>> {
    let t0 = std::time::Instant::now();
    // 1. resolve function row
    let row: Option<crate::models::Function> =
        sqlx::query_as("SELECT * FROM functions WHERE id = ?")
            .bind(id)
            .fetch_optional(&state.pool)
            .await
            .map_err(|e| AppError::Internal(format!("fn lookup: {e}")).into_response())?;
    let fn_row = row
        .ok_or_else(|| AppError::NotFound(format!("function id={id} not found")).into_response())?;

    // 2. dispatch by kind
    if fn_row.kind == 1 {
        // builtin — 直接调用宿主 handler（与 orchestrator 走相同路径）
        let Some(builtin) = crate::runtime::builtins::lookup(&fn_row.identifier) else {
            return Err(AppError::Internal(format!(
                "builtin function「{}」未找到 handler",
                fn_row.identifier
            ))
            .into_response());
        };

        // 构建 AgentContext（如果提供了 user_input）
        let agent_ctx: Option<Arc<AgentContext>> = match body.user_input {
            Some(ref ui) => {
                let mut metadata: HashMap<String, String> = HashMap::new();
                if let Some(ref v) = ui.actor_id {
                    metadata.insert("actor_id".into(), v.clone());
                }
                if let Some(ref v) = ui.channel {
                    metadata.insert("channel".into(), v.clone());
                }
                if let Some(ref v) = ui.client_type {
                    metadata.insert("client_type".into(), v.clone());
                }
                if let Some(ref v) = ui.client_version {
                    metadata.insert("client_version".into(), v.clone());
                }
                tracing::info!(
                    actor_id = ?ui.actor_id,
                    raw_text = ?ui.raw_text,
                    "invoke_function: 使用前端提供的 UserInput 构建 AgentContext"
                );
                Some(Arc::new(AgentContext::new(
                    format!("test-ctx-{}", uuid::Uuid::new_v4()),
                    UserInput {
                        raw_text: ui.raw_text.clone().unwrap_or_default(),
                        session_id: None,
                        message_id: None,
                        timestamp: Utc::now(),
                        metadata,
                    },
                    ContextConfig::default(),
                )))
            }
            None => {
                tracing::info!("invoke_function: 未提供 user_input，AgentContext 为 None");
                None
            }
        };

        let bctx = crate::runtime::builtins::BuiltinContext {
            pool: &state.pool,
            ext_pool: state.ext_pool.as_ref(),
            redis: Some(&state.redis),
            agent_ctx,
            llm: None,
            agent_id: None,
        };
        match (builtin.handler)(body.input, &bctx) {
            Ok(output) => {
                return Ok(ApiResponse::success(InvokeResp {
                    output,
                    elapsed_ms: t0.elapsed().as_millis() as i32,
                }));
            }
            Err(e) => {
                return Err(AppError::Internal(format!(
                    "builtin「{}」执行失败: {}",
                    fn_row.identifier, e
                ))
                .into_response());
            }
        }
    }

    // 仅 kind=2 (custom function) 走 Plugin invoker；其他 kind 拒绝
    if fn_row.kind != 2 {
        return Err(AppError::BadRequest(format!(
            "unsupported function kind={} for id={}",
            fn_row.kind, id
        ))
        .into_response());
    }

    // 插件系统已关闭时，custom function 无法执行
    let _ = crate::api::require_s3(&state)?;

    let plugin_id = fn_row.plugin_id.ok_or_else(|| {
        AppError::Internal(format!(
            "custom function「{}」(id={}) 缺少 plugin_id",
            fn_row.identifier, fn_row.id
        ))
        .into_response()
    })?;
    let export = fn_row.plugin_export.clone().ok_or_else(|| {
        AppError::Internal(format!(
            "custom function「{}」(id={}) 缺少 plugin_export",
            fn_row.identifier, fn_row.id
        ))
        .into_response()
    })?;

    // 预检查：确认 plugin 存在且未被删除
    let plugin_row: Option<crate::models::Plugin> =
        sqlx::query_as("SELECT * FROM plugins WHERE id = ? AND deleted_at IS NULL")
            .bind(plugin_id)
            .fetch_optional(&state.pool)
            .await
            .map_err(|e| AppError::Internal(format!("plugin lookup: {e}")).into_response())?;
    if plugin_row.is_none() {
        return Err(AppError::NotFound(format!(
            "function「{}」关联的 plugin (id={}) 不存在或已删除",
            fn_row.identifier, plugin_id
        ))
        .into_response());
    }

    // 测试端点：注入全量 capability 权限，绕过 agent 权限限制
    let all_caps: Vec<String> = state
        .runtime_state
        .capabilities
        .all()
        .iter()
        .map(|c| c.name.to_string())
        .collect();

    let dispatch_ctx = DispatchCtx {
        request_id: None,
        session_id: None,
        agent_id: body.agent_id,
        plugin_id,
        function_id: Some(id),
        permissions: all_caps,
    };

    let input_json = serde_json::to_string(&body.input)
        .map_err(|e| AppError::BadRequest(format!("input serialize: {e}")).into_response())?;

    let output_str = state
        .runtime_state
        .invoker
        .invoke(
            &state.pool,
            state.s3.as_ref(),
            Arc::clone(&state.runtime_state.capabilities),
            Arc::clone(&state.runtime_state.llm),
            plugin_id,
            &export,
            input_json,
            dispatch_ctx,
        )
        .await
        .map_err(|e| {
            tracing::error!(
                "WASM invoke failed for function「{}」(id={}, plugin_id={}, export={}): {:?}",
                fn_row.identifier,
                id,
                plugin_id,
                export,
                e
            );
            AppError::Internal(format!("WASM 插件调用失败: {}", e)).into_response()
        })?;

    let output: Value = serde_json::from_str(&output_str).unwrap_or(Value::String(output_str));
    Ok(ApiResponse::success(InvokeResp {
        output,
        elapsed_ms: t0.elapsed().as_millis() as i32,
    }))
}
