//! Tag API handlers (T134 / US7)

use axum::{
    extract::{Path, Query, State},
    routing::get,
    Json, Router,
};
use serde::Deserialize;

use crate::api::AppState;
use crate::services::tag::{self as svc, CreateMeta, TagWithCount, UpdateMeta};
use crate::utils::error::ApiResponse;

pub fn router() -> Router<AppState> {
    Router::new()
        .route("/tags", get(list_tags).post(create_tag))
        .route("/tags/:id", get(get_tag).put(update_tag).delete(delete_tag))
}

#[derive(Debug, Deserialize)]
pub struct ListQuery {
    #[serde(default)] pub q: Option<String>,
    #[serde(default)] pub offset: Option<i64>,
    #[serde(default)] pub limit: Option<i64>,
}

async fn list_tags(
    State(state): State<AppState>,
    Query(q): Query<ListQuery>,
) -> Result<ApiResponse<serde_json::Value>, ApiResponse<()>> {
    let offset = q.offset.unwrap_or(0).max(0);
    let limit = q.limit.unwrap_or(20).clamp(1, 100);

    let (items, total) = svc::list(&state.pool, q.q.as_deref().filter(|s| !s.is_empty()), offset, limit)
        .await
        .map_err(|e| e.into_response())?;

    let resp = serde_json::json!({
        "items": items,
        "total": total,
        "offset": offset,
        "limit": limit,
    });

    Ok(ApiResponse::success(resp))
}

async fn get_tag(
    State(state): State<AppState>,
    Path(id): Path<i64>,
) -> Result<ApiResponse<crate::models::Tag>, ApiResponse<()>> {
    svc::fetch_by_id(&state.pool, id)
        .await
        .map(ApiResponse::success)
        .map_err(|e| e.into_response())
}

async fn create_tag(
    State(state): State<AppState>,
    Json(meta): Json<CreateMeta>,
) -> Result<ApiResponse<crate::models::Tag>, ApiResponse<()>> {
    svc::create(&state.pool, meta)
        .await
        .map(ApiResponse::success)
        .map_err(|e| e.into_response())
}

async fn update_tag(
    State(state): State<AppState>,
    Path(id): Path<i64>,
    Json(meta): Json<UpdateMeta>,
) -> Result<ApiResponse<crate::models::Tag>, ApiResponse<()>> {
    svc::update(&state.pool, id, meta)
        .await
        .map(ApiResponse::success)
        .map_err(|e| e.into_response())
}

async fn delete_tag(
    State(state): State<AppState>,
    Path(id): Path<i64>,
) -> Result<ApiResponse<()>, ApiResponse<()>> {
    match svc::delete(&state.pool, id).await {
        Ok(()) => Ok(ApiResponse::success(())),
        Err(e) => Err(e.into_response()),
    }
}
