use axum::{
    Json, Router,
    extract::{Path, Query, State},
    routing::get,
};
use serde::Deserialize;

use crate::api::AppState;
use crate::services::capability::{self as svc, CreateMeta, ListFilter, UpdateMeta};
use crate::utils::error::ApiResponse;

pub fn router() -> Router<AppState> {
    Router::new()
        .route(
            "/capabilities",
            get(list_capabilities).post(create_capability),
        )
        .route(
            "/capabilities/:name",
            get(get_capability)
                .put(update_capability)
                .delete(delete_capability),
        )
}

#[derive(Debug, Deserialize)]
pub struct ListQuery {
    pub name: Option<String>,
    pub description: Option<String>,
    pub is_dangerous: Option<u8>,
    pub category_id: Option<i64>,
    pub offset: Option<i64>,
    pub limit: Option<i64>,
}

async fn list_capabilities(
    State(state): State<AppState>,
    Query(q): Query<ListQuery>,
) -> ApiResponse<(Vec<svc::CapabilityItem>, i64)> {
    let filter = ListFilter {
        name: q.name,
        description: q.description,
        is_dangerous: q.is_dangerous.map(|v| v != 0),
        category_id: q.category_id,
    };
    let offset = q.offset.unwrap_or(0);
    let limit = q.limit.unwrap_or(50);
    match svc::list(&state.pool, offset, limit, filter).await {
        Ok((items, total)) => ApiResponse::success((items, total)),
        Err(e) => e.into_response(),
    }
}

async fn get_capability(
    State(state): State<AppState>,
    Path(name): Path<String>,
) -> Result<ApiResponse<svc::CapabilityItem>, ApiResponse<()>> {
    svc::fetch_by_name(&state.pool, &name)
        .await
        .map(|c| ApiResponse::success(svc::CapabilityItem::from(&c)))
        .map_err(|e| e.into_response())
}

async fn create_capability(
    State(state): State<AppState>,
    Json(meta): Json<CreateMeta>,
) -> Result<ApiResponse<svc::CapabilityItem>, ApiResponse<()>> {
    svc::create(&state.pool, meta)
        .await
        .map(|c| ApiResponse::success(svc::CapabilityItem::from(&c)))
        .map_err(|e| e.into_response())
}

async fn update_capability(
    State(state): State<AppState>,
    Path(name): Path<String>,
    Json(meta): Json<UpdateMeta>,
) -> Result<ApiResponse<svc::CapabilityItem>, ApiResponse<()>> {
    svc::update(&state.pool, &name, meta)
        .await
        .map(|c| ApiResponse::success(svc::CapabilityItem::from(&c)))
        .map_err(|e| e.into_response())
}

async fn delete_capability(
    State(state): State<AppState>,
    Path(name): Path<String>,
) -> Result<ApiResponse<()>, ApiResponse<()>> {
    match svc::delete(&state.pool, &name).await {
        Ok(()) => Ok(ApiResponse::success(())),
        Err(e) => Err(e.into_response()),
    }
}
