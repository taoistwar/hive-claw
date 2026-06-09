//! Tool API handlers (T085 / US2)

use axum::{
    Json, Router,
    extract::{Extension, Path, Query, State},
    routing::{get, post},
};
use serde::Deserialize;

use crate::api::AppState;
use crate::runtime::tool_test::{self as test_svc, TestToolRequest};
use crate::services::audit::{self as audit_svc, Operation};
use crate::services::tool::{self as svc, CreateMeta, ListFilter, UpdateMeta};
use crate::utils::error::ApiResponse;
use crate::utils::jwt::Claims;

pub fn router() -> Router<AppState> {
    Router::new()
        .route("/tools", get(list_tools).post(create_tool))
        .route(
            "/tools/:id",
            get(get_tool).put(update_tool).delete(delete_tool),
        )
        .route("/tools/:id/test", post(test_tool))
}

#[derive(Debug, Deserialize)]
pub struct ListQuery {
    #[serde(default)]
    pub offset: Option<i64>,
    #[serde(default)]
    pub limit: Option<i64>,
    #[serde(default)]
    pub search: Option<String>,
    #[serde(default)]
    pub kind: Option<i8>,
    #[serde(default)]
    pub source: Option<String>,
    #[serde(default)]
    pub category_id: Option<i64>,
    #[serde(default)]
    pub created_at_start: Option<String>,
    #[serde(default)]
    pub created_at_end: Option<String>,
    #[serde(default)]
    pub updated_at_start: Option<String>,
    #[serde(default)]
    pub updated_at_end: Option<String>,
}

async fn list_tools(
    State(state): State<AppState>,
    Query(q): Query<ListQuery>,
) -> Result<ApiResponse<svc::ToolList>, ApiResponse<()>> {
    let parse_dt = |s: &str| -> Result<chrono::NaiveDateTime, ()> {
        chrono::NaiveDateTime::parse_from_str(s, "%Y-%m-%d")
            .or_else(|_| chrono::NaiveDateTime::parse_from_str(s, "%Y-%m-%d %H:%M:%S"))
            .map_err(|_| ())
    };

    let filter = ListFilter {
        offset: q.offset.unwrap_or(0).max(0),
        limit: q.limit.unwrap_or(20).clamp(1, 100),
        search: q.search.filter(|s| !s.is_empty()),
        kind: q.kind,
        source: q.source.filter(|s| !s.is_empty()),
        category_id: q.category_id,
        created_at_start: q.created_at_start.as_ref().and_then(|s| parse_dt(s).ok()),
        created_at_end: q.created_at_end.as_ref().and_then(|s| parse_dt(s).ok()),
        updated_at_start: q.updated_at_start.as_ref().and_then(|s| parse_dt(s).ok()),
        updated_at_end: q.updated_at_end.as_ref().and_then(|s| parse_dt(s).ok()),
    };
    svc::list(&state.pool, filter)
        .await
        .map(ApiResponse::success)
        .map_err(|e| e.into_response())
}

async fn get_tool(
    State(state): State<AppState>,
    Path(id): Path<i64>,
) -> Result<ApiResponse<crate::services::tool::ToolListItem>, ApiResponse<()>> {
    svc::fetch_by_id_with_tags(&state.pool, id)
        .await
        .map(ApiResponse::success)
        .map_err(|e| e.into_response())
}

async fn create_tool(
    State(state): State<AppState>,
    Extension(claims): Extension<Claims>,
    Json(meta): Json<CreateMeta>,
) -> Result<ApiResponse<crate::services::tool::ToolListItem>, ApiResponse<()>> {
    match svc::create(&state.pool, meta).await {
        Ok(tool_item) => {
            if let Err(e) = audit_event(
                &state.pool,
                &claims,
                Operation::Create,
                Some(tool_item.tool.id),
                &tool_item.tool.name,
                serde_json::json!({ "kind": tool_item.tool.kind, "source": tool_item.tool.source }),
            )
            .await
            {
                tracing::error!("Failed to write audit log for tool create: {}", e);
            }
            Ok(ApiResponse::success(tool_item))
        }
        Err(e) => Err(e.into_response()),
    }
}

async fn update_tool(
    State(state): State<AppState>,
    Extension(claims): Extension<Claims>,
    Path(id): Path<i64>,
    Json(meta): Json<UpdateMeta>,
) -> Result<ApiResponse<crate::services::tool::ToolListItem>, ApiResponse<()>> {
    match svc::update(&state.pool, id, meta).await {
        Ok(tool_item) => {
            if let Err(e) = audit_event(
                &state.pool,
                &claims,
                Operation::Update,
                Some(tool_item.tool.id),
                &tool_item.tool.name,
                serde_json::json!({ "kind": tool_item.tool.kind, "source": tool_item.tool.source }),
            )
            .await
            {
                tracing::error!("Failed to write audit log for tool update: {}", e);
            }
            Ok(ApiResponse::success(tool_item))
        }
        Err(e) => Err(e.into_response()),
    }
}

async fn delete_tool(
    State(state): State<AppState>,
    Extension(claims): Extension<Claims>,
    Path(id): Path<i64>,
) -> Result<ApiResponse<()>, ApiResponse<()>> {
    let prev = svc::fetch_by_id_with_tags(&state.pool, id).await.ok();
    let target_id = prev.as_ref().map(|t| t.tool.id);
    let target_name = prev
        .as_ref()
        .map(|t| &t.tool.name)
        .cloned()
        .unwrap_or_default();

    match svc::delete(&state.pool, id).await {
        Ok(()) => {
            if let Err(e) = audit_event(
                &state.pool,
                &claims,
                Operation::Delete,
                target_id,
                &target_name,
                serde_json::json!({}),
            )
            .await
            {
                tracing::error!("Failed to write audit log for tool delete: {}", e);
            }
            Ok(ApiResponse::success(()))
        }
        Err(e) => Err(e.into_response()),
    }
}

async fn test_tool(
    State(state): State<AppState>,
    Path(id): Path<i64>,
    Json(mut req): Json<TestToolRequest>,
) -> Result<ApiResponse<test_svc::TestToolResult>, ApiResponse<()>> {
    let trace_id = req.trace_id.take().unwrap_or_else(|| {
        use std::time::{SystemTime, UNIX_EPOCH};
        let ts = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_millis();
        format!("tool_test_{}", ts)
    });
    let deps = crate::runtime::orchestrator::OrchestratorDeps {
        pool: state.pool.clone(),
        redis: state.redis.clone(),
        s3: state.s3.clone(),
        llm: state.runtime_state.llm.clone(),
        registry: state.runtime_state.capabilities.clone(),
        invoker: state.runtime_state.invoker.clone(),
        ext_pool: state.ext_pool.clone(),
        message: String::new(),
        channel: String::new(),
        client_type: String::new(),
        client_version: String::new(),
        sensitive_filter: state.sensitive_filter.clone(),
    };
    let req_with_trace = test_svc::TestToolRequest {
        message: req.message,
        model_preset: req.model_preset,
        trace_id: Some(trace_id.clone()),
    };
    match test_svc::run_tool_test(&state.pool, &deps, id, req_with_trace).await {
        Ok(result) => Ok(ApiResponse::success(result)),
        Err(e) => {
            let log_path = format!("/tmp/tool_test_logs/{}.log", trace_id);
            Err(ApiResponse::err(
                5000,
                format!("{} (debug: {})", e, log_path),
            ))
        }
    }
}

/// 写入审计日志。失败时记录 tracing 日志但不影响主流程。
async fn audit_event(
    pool: &sqlx::MySqlPool,
    claims: &Claims,
    op: Operation,
    target_id: Option<i64>,
    target_name: &str,
    detail: serde_json::Value,
) -> anyhow::Result<()> {
    use crate::services::admin as admin_svc;
    let admin_id = claims
        .admin_id
        .ok_or_else(|| anyhow::anyhow!("No admin ID in claims"))?;
    let operator = admin_svc::get_admin_by_id(pool, admin_id).await?;
    let operator_phone = operator.map(|a| a.phone).unwrap_or_default();
    if let Err(e) = audit_svc::record(
        pool,
        admin_id,
        &operator_phone,
        target_id,
        target_name,
        op,
        Some(detail),
    )
    .await
    {
        tracing::error!("Failed to write audit log: {}", e);
        return Err(e);
    }
    Ok(())
}
