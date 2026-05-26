//! Function API handlers (T084 / US2)

use axum::{
    extract::{Path, Query, State},
    routing::get,
    Json, Router,
};
use serde::Deserialize;

use crate::api::AppState;
use crate::services::function::{self as svc, CreateMeta, ListFilter, UpdateMeta};
use crate::utils::error::{ApiResponse, AppError};

pub fn router() -> Router<AppState> {
    Router::new()
        .route("/functions", get(list_fns).post(create_fn))
        .route(
            "/functions/:id",
            get(get_fn).put(update_fn).delete(delete_fn),
        )
}

#[derive(Debug, Deserialize)]
pub struct ListQuery {
    #[serde(default)] pub offset: Option<i64>,
    #[serde(default)] pub limit: Option<i64>,
    #[serde(default)] pub search: Option<String>,
    #[serde(default)] pub category_id: Option<i64>,
    /// "builtin" | "custom"
    #[serde(default)] pub kind: Option<String>,
}

async fn list_fns(
    State(state): State<AppState>,
    Query(q): Query<ListQuery>,
) -> Result<ApiResponse<svc::FunctionList>, ApiResponse<()>> {
    let kind = q.kind.as_deref().and_then(|s| match s {
        "builtin" => Some(1),
        "custom" => Some(2),
        _ => None,
    });
    let filter = ListFilter {
        offset: q.offset.unwrap_or(0).max(0),
        limit: q.limit.unwrap_or(20).clamp(1, 100),
        search: q.search.filter(|s| !s.is_empty()),
        category_id: q.category_id,
        kind,
    };
    svc::list(&state.pool, filter)
        .await
        .map(ApiResponse::success)
        .map_err(|e| e.into_response())
}

async fn get_fn(
    State(state): State<AppState>,
    Path(id): Path<i64>,
) -> Result<ApiResponse<crate::models::Function>, ApiResponse<()>> {
    svc::fetch_by_id(&state.pool, id)
        .await
        .map(ApiResponse::success)
        .map_err(|e| e.into_response())
}

async fn create_fn(
    State(state): State<AppState>,
    Json(meta): Json<CreateMeta>,
) -> Result<ApiResponse<crate::models::Function>, ApiResponse<()>> {
    svc::create_custom(&state.pool, meta)
        .await
        .map(ApiResponse::success)
        .map_err(|e| e.into_response())
}

async fn update_fn(
    State(state): State<AppState>,
    Path(id): Path<i64>,
    Json(meta): Json<UpdateMeta>,
) -> Result<ApiResponse<crate::models::Function>, ApiResponse<()>> {
    svc::update(&state.pool, id, meta)
        .await
        .map(ApiResponse::success)
        .map_err(|e| e.into_response())
}

async fn delete_fn(
    State(state): State<AppState>,
    Path(id): Path<i64>,
) -> Result<ApiResponse<()>, ApiResponse<()>> {
    match svc::delete(&state.pool, id).await {
        Ok(()) => Ok(ApiResponse::success(())),
        Err(e) => Err(e.into_response()),
    }
}

// 触发 AppError import 让编译器追踪未使用警告
#[allow(dead_code)]
fn _force_app_error_use(_: AppError) {}
