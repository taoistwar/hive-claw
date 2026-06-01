use axum::{
    extract::{Path, Query, State},
    routing::get,
    Json, Router,
};
use serde::{Deserialize, Serialize};

use crate::api::AppState;
use crate::models::Role;
use crate::services::recommended_game::{self as svc, CreateMeta, UpdateMeta};
use crate::utils::error::{ApiResponse, AppError};
use crate::utils::jwt::Claims;

pub fn router() -> Router<AppState> {
    Router::new()
        .route("/recommended-games", get(list_recommended_games).post(create_recommended_game))
        .route("/recommended-games/:id", get(get_recommended_game).put(update_recommended_game).delete(delete_recommended_game))
}

pub fn router_public() -> Router<AppState> {
    Router::new()
        .route("/recommended-games/top", get(top_recommended_games))
}

#[derive(Debug, Deserialize)]
pub struct ListQuery {
    #[serde(default)] pub q: Option<String>,
    #[serde(default = "default_page")] pub page: i64,
    #[serde(default = "default_page_size")] pub page_size: i64,
}

fn default_page() -> i64 { 1 }
fn default_page_size() -> i64 { 20 }

#[derive(Debug, Serialize)]
pub struct ListResponse {
    pub items: Vec<crate::models::RecommendedGame>,
    pub total: i64,
}

async fn list_recommended_games(
    State(state): State<AppState>,
    Query(q): Query<ListQuery>,
) -> Result<ApiResponse<ListResponse>, ApiResponse<()>> {
    let (items, total) = svc::list(
        &state.pool,
        q.q.as_deref().filter(|s| !s.is_empty()),
        q.page,
        q.page_size,
    )
    .await
    .map_err(|e| e.into_response())?;
    Ok(ApiResponse::success(ListResponse { items, total }))
}

async fn get_recommended_game(
    State(state): State<AppState>,
    Path(id): Path<i64>,
) -> Result<ApiResponse<crate::models::RecommendedGame>, ApiResponse<()>> {
    svc::fetch_by_id(&state.pool, id)
        .await
        .map(ApiResponse::success)
        .map_err(|e| e.into_response())
}

async fn create_recommended_game(
    State(state): State<AppState>,
    axum::Extension(claims): axum::Extension<Claims>,
    Json(meta): Json<CreateMeta>,
) -> Result<ApiResponse<crate::models::RecommendedGame>, ApiResponse<()>> {
    let caller_role = match Role::try_from(claims.role) {
        Ok(role) => role,
        Err(_) => {
            return Err(AppError::InsufficientPermission("Invalid role in token".to_string())
                .into_response())
        }
    };

    if !caller_role.can_manage_recommended_games() {
        return Err(AppError::InsufficientPermission("Insufficient permissions".to_string())
            .into_response());
    }

    svc::create(&state.pool, meta)
        .await
        .map(ApiResponse::success)
        .map_err(|e| e.into_response())
}

async fn update_recommended_game(
    State(state): State<AppState>,
    Path(id): Path<i64>,
    axum::Extension(claims): axum::Extension<Claims>,
    Json(meta): Json<UpdateMeta>,
) -> Result<ApiResponse<crate::models::RecommendedGame>, ApiResponse<()>> {
    let caller_role = match Role::try_from(claims.role) {
        Ok(role) => role,
        Err(_) => {
            return Err(AppError::InsufficientPermission("Invalid role in token".to_string())
                .into_response())
        }
    };

    if !caller_role.can_manage_recommended_games() {
        return Err(AppError::InsufficientPermission("Insufficient permissions".to_string())
            .into_response());
    }

    svc::update(&state.pool, id, meta)
        .await
        .map(ApiResponse::success)
        .map_err(|e| e.into_response())
}

async fn delete_recommended_game(
    State(state): State<AppState>,
    Path(id): Path<i64>,
    axum::Extension(claims): axum::Extension<Claims>,
) -> Result<ApiResponse<()>, ApiResponse<()>> {
    let caller_role = match Role::try_from(claims.role) {
        Ok(role) => role,
        Err(_) => {
            return Err(AppError::InsufficientPermission("Invalid role in token".to_string())
                .into_response())
        }
    };

    if !caller_role.can_delete_recommended_games() {
        return Err(AppError::InsufficientPermission("Insufficient permissions".to_string())
            .into_response());
    }

    match svc::delete(&state.pool, id).await {
        Ok(()) => Ok(ApiResponse::success(())),
        Err(e) => Err(e.into_response()),
    }
}

#[derive(Debug, Deserialize)]
pub struct TopQuery {
    pub n: i64,
}

async fn top_recommended_games(
    State(state): State<AppState>,
    Query(q): Query<TopQuery>,
) -> Result<ApiResponse<Vec<crate::models::RecommendedGame>>, ApiResponse<()>> {
    let n = q.n.clamp(1, 10);
    svc::fetch_top_n(&state.pool, n)
        .await
        .map(ApiResponse::success)
        .map_err(|e| e.into_response())
}
