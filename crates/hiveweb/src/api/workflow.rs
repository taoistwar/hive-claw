//! Workflow API (T109 / US3)
//!
//! 端点：
//!   GET    /api/workflows                — list metadata
//!   POST   /api/workflows                — create metadata
//!   GET    /api/workflows/:id            — single metadata
//!   PUT    /api/workflows/:id            — update metadata
//!   DELETE /api/workflows/:id
//!   GET    /api/workflows/:id/graph      — full DAG (nodes + edges)
//!   PUT    /api/workflows/:id/graph      — replace nodes + edges (cycle / mapping check)
//!   POST   /api/workflows/:id/execute    — run（占位；T110 接通）

use axum::{
    extract::{Path, Query, State},
    routing::{get, post, put},
    Json, Router,
};
use serde::Deserialize;
use serde_json::Value;

use crate::api::AppState;
use crate::services::workflow::{
    self as svc, CreateMeta, GraphPut, UpdateMeta,
};
use crate::utils::error::{ApiResponse, AppError};

pub fn router() -> Router<AppState> {
    Router::new()
        .route("/workflows", get(list_workflows).post(create_workflow))
        .route(
            "/workflows/:id",
            get(get_workflow).put(update_workflow).delete(delete_workflow),
        )
        .route(
            "/workflows/:id/graph",
            get(get_graph).put(put_graph_handler),
        )
        .route("/workflows/:id/execute", post(execute_workflow))
}

#[derive(Debug, Deserialize)]
pub struct ListQuery {
    #[serde(default)] pub offset: Option<i64>,
    #[serde(default)] pub limit: Option<i64>,
}

async fn list_workflows(
    State(state): State<AppState>,
    Query(q): Query<ListQuery>,
) -> Result<ApiResponse<svc::WorkflowList>, ApiResponse<()>> {
    let offset = q.offset.unwrap_or(0).max(0);
    let limit = q.limit.unwrap_or(20).clamp(1, 100);
    svc::list(&state.pool, offset, limit)
        .await
        .map(ApiResponse::success)
        .map_err(|e| e.into_response())
}

async fn get_workflow(
    State(state): State<AppState>,
    Path(id): Path<i64>,
) -> Result<ApiResponse<crate::models::Workflow>, ApiResponse<()>> {
    svc::fetch_by_id(&state.pool, id)
        .await
        .map(ApiResponse::success)
        .map_err(|e| e.into_response())
}

async fn create_workflow(
    State(state): State<AppState>,
    Json(meta): Json<CreateMeta>,
) -> Result<ApiResponse<crate::models::Workflow>, ApiResponse<()>> {
    svc::create(&state.pool, meta)
        .await
        .map(ApiResponse::success)
        .map_err(|e| e.into_response())
}

async fn update_workflow(
    State(state): State<AppState>,
    Path(id): Path<i64>,
    Json(meta): Json<UpdateMeta>,
) -> Result<ApiResponse<crate::models::Workflow>, ApiResponse<()>> {
    svc::update(&state.pool, id, meta)
        .await
        .map(ApiResponse::success)
        .map_err(|e| e.into_response())
}

async fn delete_workflow(
    State(state): State<AppState>,
    Path(id): Path<i64>,
) -> Result<ApiResponse<()>, ApiResponse<()>> {
    match svc::delete(&state.pool, id).await {
        Ok(()) => Ok(ApiResponse::success(())),
        Err(e) => Err(e.into_response()),
    }
}

async fn get_graph(
    State(state): State<AppState>,
    Path(id): Path<i64>,
) -> Result<ApiResponse<svc::WorkflowGraph>, ApiResponse<()>> {
    svc::fetch_graph(&state.pool, id)
        .await
        .map(ApiResponse::success)
        .map_err(|e| e.into_response())
}

async fn put_graph_handler(
    State(state): State<AppState>,
    Path(id): Path<i64>,
    Json(graph): Json<GraphPut>,
) -> Result<ApiResponse<svc::WorkflowGraph>, ApiResponse<()>> {
    svc::put_graph(&state.pool, id, graph)
        .await
        .map(ApiResponse::success)
        .map_err(|e| e.into_response())
}

#[derive(Debug, Deserialize)]
pub struct ExecuteBody {
    #[serde(default = "default_input")]
    pub input: Value,
}
fn default_input() -> Value { Value::Object(serde_json::Map::new()) }

async fn execute_workflow(
    State(_state): State<AppState>,
    Path(id): Path<i64>,
    Json(_body): Json<ExecuteBody>,
) -> Result<ApiResponse<Value>, ApiResponse<()>> {
    // T110 真实拓扑执行留待 commit 2；当前返 5001 占位
    Err(AppError::Internal(format!(
        "workflow execute (id={id}) 待 T110 接通；当前 stub"
    ))
    .into_response())
}
