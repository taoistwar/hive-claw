//! Category API handlers (T133 / US7)

use axum::{
    Json, Router,
    extract::{Path, Query, State},
    routing::get,
};
use serde::Deserialize;

use crate::api::AppState;
use crate::services::category::{self as svc, CreateMeta, UpdateMeta};
use crate::utils::error::ApiResponse;

pub fn router() -> Router<AppState> {
    Router::new()
        .route("/categories", get(list_categories).post(create_category))
        .route(
            "/categories/:id",
            get(get_category)
                .put(update_category)
                .delete(delete_category),
        )
}

#[derive(Debug, Deserialize)]
pub struct ListQuery {
    #[serde(default)]
    pub flat: Option<u8>,
}

#[derive(Debug, serde::Serialize)]
#[serde(untagged)]
pub enum ListResp {
    Tree(Vec<svc::CategoryNode>),
    Flat(Vec<crate::models::Category>),
}

async fn list_categories(
    State(state): State<AppState>,
    Query(q): Query<ListQuery>,
) -> Result<ApiResponse<ListResp>, ApiResponse<()>> {
    if q.flat.unwrap_or(0) == 1 {
        svc::list_flat(&state.pool)
            .await
            .map(|v| ApiResponse::success(ListResp::Flat(v)))
            .map_err(|e| e.into_response())
    } else {
        svc::list_tree(&state.pool)
            .await
            .map(|v| ApiResponse::success(ListResp::Tree(v)))
            .map_err(|e| e.into_response())
    }
}

async fn get_category(
    State(state): State<AppState>,
    Path(id): Path<i64>,
) -> Result<ApiResponse<crate::models::Category>, ApiResponse<()>> {
    svc::fetch_by_id(&state.pool, id)
        .await
        .map(ApiResponse::success)
        .map_err(|e| e.into_response())
}

async fn create_category(
    State(state): State<AppState>,
    Json(meta): Json<CreateMeta>,
) -> Result<ApiResponse<crate::models::Category>, ApiResponse<()>> {
    svc::create(&state.pool, meta)
        .await
        .map(ApiResponse::success)
        .map_err(|e| e.into_response())
}

async fn update_category(
    State(state): State<AppState>,
    Path(id): Path<i64>,
    Json(meta): Json<UpdateMeta>,
) -> Result<ApiResponse<crate::models::Category>, ApiResponse<()>> {
    svc::update(&state.pool, id, meta)
        .await
        .map(ApiResponse::success)
        .map_err(|e| e.into_response())
}

async fn delete_category(
    State(state): State<AppState>,
    Path(id): Path<i64>,
) -> Result<ApiResponse<()>, ApiResponse<()>> {
    match svc::delete(&state.pool, id).await {
        Ok(()) => Ok(ApiResponse::success(())),
        Err(e) => Err(e.into_response()),
    }
}
