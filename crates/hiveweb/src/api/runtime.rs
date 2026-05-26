//! Runtime API — pool 统计 (T162) + custom function 调用入口
//!
//! `GET /api/runtime/pool/stats` ：Pool 健康度（FR-029 / contracts/api.md §9b）
//! `POST /api/functions/:id/invoke` ：把 Function 当 RPC 调一次（US4 演示用 +
//!  Workflow 节点也会复用此路径）

use axum::{
    extract::{Path, State},
    routing::{get, post},
    Json, Router,
};
use serde::Serialize;
use serde_json::Value;
use std::sync::Arc;

use crate::api::AppState;
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
}

fn default_agent_id() -> i64 { 1 }

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
    let fn_row = row.ok_or_else(|| {
        AppError::NotFound(format!("function id={id} not found")).into_response()
    })?;

    // 2. dispatch by kind
    if fn_row.kind == 1 {
        // builtin: 待 US5 接入 ToolRegistry 后正式调；此处占位
        return Err(AppError::Internal(
            "builtin function invocation 待 US5 接入 ToolRegistry".into(),
        )
        .into_response());
    }

    // custom (kind=2) — 走 Plugin invoker
    let plugin_id = fn_row.plugin_id.ok_or_else(|| {
        AppError::Internal("custom function 缺 plugin_id".into()).into_response()
    })?;
    let export = fn_row.plugin_export.clone().ok_or_else(|| {
        AppError::Internal("custom function 缺 plugin_export".into()).into_response()
    })?;

    let dispatch_ctx = DispatchCtx {
        request_id: None,
        session_id: None,
        agent_id: body.agent_id,
        plugin_id,
        function_id: Some(id),
    };

    let input_json = serde_json::to_string(&body.input)
        .map_err(|e| AppError::BadRequest(format!("input serialize: {e}")).into_response())?;

    let output_str = state
        .runtime_state
        .invoker
        .invoke(
            &state.pool,
            &state.s3,
            Arc::clone(&state.runtime_state.capabilities),
            plugin_id,
            &export,
            input_json,
            dispatch_ctx,
        )
        .await
        .map_err(|e| AppError::from(e).into_response())?;

    let output: Value = serde_json::from_str(&output_str).unwrap_or(Value::String(output_str));
    Ok(ApiResponse::success(InvokeResp {
        output,
        elapsed_ms: t0.elapsed().as_millis() as i32,
    }))
}
