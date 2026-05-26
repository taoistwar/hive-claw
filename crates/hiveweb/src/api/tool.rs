//! Tool API handlers (T085 / US2)

use axum::{
    extract::{Path, Query, State},
    routing::get,
    Json, Router,
};
use serde::Deserialize;

use crate::api::AppState;
use crate::services::tool::{self as svc, CreateMeta, ListFilter, UpdateMeta};
use crate::utils::error::ApiResponse;

pub fn router() -> Router<AppState> {
    Router::new()
        .route("/tools", get(list_tools).post(create_tool))
        .route(
            "/tools/:id",
            get(get_tool).put(update_tool).delete(delete_tool),
        )
}

#[derive(Debug, Deserialize)]
pub struct ListQuery {
    #[serde(default)] pub offset: Option<i64>,
    #[serde(default)] pub limit: Option<i64>,
    #[serde(default)] pub search: Option<String>,
    #[serde(default)] pub kind: Option<i8>,
}

async fn list_tools(
    State(state): State<AppState>,
    Query(q): Query<ListQuery>,
) -> Result<ApiResponse<svc::ToolList>, ApiResponse<()>> {
    let filter = ListFilter {
        offset: q.offset.unwrap_or(0).max(0),
        limit: q.limit.unwrap_or(20).clamp(1, 100),
        search: q.search.filter(|s| !s.is_empty()),
        kind: q.kind,
    };
    svc::list(&state.pool, filter)
        .await
        .map(ApiResponse::success)
        .map_err(|e| e.into_response())
}

async fn get_tool(
    State(state): State<AppState>,
    Path(id): Path<i64>,
) -> Result<ApiResponse<crate::models::Tool>, ApiResponse<()>> {
    svc::fetch_by_id(&state.pool, id)
        .await
        .map(ApiResponse::success)
        .map_err(|e| e.into_response())
}

async fn create_tool(
    State(state): State<AppState>,
    Json(meta): Json<CreateMeta>,
) -> Result<ApiResponse<crate::models::Tool>, ApiResponse<()>> {
    svc::create(&state.pool, meta)
        .await
        .map(ApiResponse::success)
        .map_err(|e| e.into_response())
}

async fn update_tool(
    State(state): State<AppState>,
    Path(id): Path<i64>,
    Json(meta): Json<UpdateMeta>,
) -> Result<ApiResponse<crate::models::Tool>, ApiResponse<()>> {
    svc::update(&state.pool, id, meta)
        .await
        .map(ApiResponse::success)
        .map_err(|e| e.into_response())
}

async fn delete_tool(
    State(state): State<AppState>,
    Path(id): Path<i64>,
) -> Result<ApiResponse<()>, ApiResponse<()>> {
    match svc::delete(&state.pool, id).await {
        Ok(()) => Ok(ApiResponse::success(())),
        Err(e) => Err(e.into_response()),
    }
}
