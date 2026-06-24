//! Sensitive word CRUD API — admin-only management endpoints.
//!
//! Routes:
//!   GET    /api/sensitive-words            — list (search, pagination)
//!   POST   /api/sensitive-words            — create
//!   PUT    /api/sensitive-words/:id        — update
//!   DELETE /api/sensitive-words/:id        — delete

use axum::{
    Json, Router,
    extract::{Path, Query, State},
    routing::get,
};
use serde::Deserialize;

use crate::api::AppState;
use crate::models::sensitive_word::{
    CreateSensitiveWordRequest, SensitiveWord, SensitiveWordListResponse,
    UpdateSensitiveWordRequest,
};
use crate::services::sensitive_filter;
use crate::utils::error::{ApiResponse, AppError};

pub fn router() -> Router<AppState> {
    Router::new()
        .route("/sensitive-words", get(list).post(create))
        .route(
            "/sensitive-words/:id",
            axum::routing::put(update).delete(delete),
        )
}

#[derive(Debug, Deserialize)]
struct ListQuery {
    #[serde(default)]
    page: Option<i64>,
    #[serde(default)]
    page_size: Option<i64>,
    #[serde(default)]
    search: Option<String>,
    #[serde(default)]
    enabled: Option<bool>,
}

async fn list(
    State(state): State<AppState>,
    Query(q): Query<ListQuery>,
) -> Result<ApiResponse<SensitiveWordListResponse>, ApiResponse<()>> {
    sensitive_filter::list_sensitive_words(
        &state.pool,
        q.page.unwrap_or(1),
        q.page_size.unwrap_or(20),
        q.search.as_deref(),
        q.enabled,
    )
    .await
    .map(ApiResponse::success)
    .map_err(|e| e.into_response())
}

async fn create(
    State(state): State<AppState>,
    Json(req): Json<CreateSensitiveWordRequest>,
) -> Result<ApiResponse<SensitiveWord>, ApiResponse<()>> {
    sensitive_filter::create_sensitive_word(&state.pool, &state.sensitive_filter, &req)
        .await
        .map(ApiResponse::success)
        .map_err(|e| e.into_response())
}

async fn update(
    State(state): State<AppState>,
    Path(id): Path<i64>,
    Json(req): Json<UpdateSensitiveWordRequest>,
) -> Result<ApiResponse<SensitiveWord>, ApiResponse<()>> {
    sensitive_filter::update_sensitive_word(&state.pool, &state.sensitive_filter, id, &req)
        .await
        .map(ApiResponse::success)
        .map_err(|e| e.into_response())
}

async fn delete(
    State(state): State<AppState>,
    Path(id): Path<i64>,
) -> Result<ApiResponse<serde_json::Value>, ApiResponse<()>> {
    match sensitive_filter::delete_sensitive_word(&state.pool, &state.sensitive_filter, id).await {
        Ok(true) => Ok(ApiResponse::success(serde_json::json!({"deleted": true}))),
        Ok(false) => Err(AppError::BadRequest("敏感词不存在".into()).into_response()),
        Err(e) => Err(e.into_response()),
    }
}
