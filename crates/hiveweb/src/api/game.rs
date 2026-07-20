//! Game HTTP APIs.
//!
//! `/game-aliases*` implements the deprecated game-alias management feature
//! and remains registered only for compatibility. `/external-games*` queries
//! active external-game data and is not part of that deprecation.

use axum::{
    Json, Router,
    extract::{Path, Query, State},
    routing::{get, post},
};
use serde::{Deserialize, Serialize};

use crate::api::AppState;
use crate::models::Role;
#[allow(deprecated)]
use crate::models::game::{
    CreateGameRequest, DEFAULT_PAGE_SIZE, GameListResponse, UpdateGameRequest,
};
use crate::services::cache_helper;
use crate::services::game_service::{self as svc};
use crate::services::{admin, audit};
use crate::utils::error::{ApiResponse, AppError};
use crate::utils::jwt::Claims;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ExternalGameOption {
    pub id: i64,
    pub name: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ExternalGameDetail {
    pub id: i64,
    pub name: String,
    #[serde(default)]
    pub description: Option<String>,
    #[serde(default)]
    pub cover_image: Option<String>,
    #[serde(default)]
    pub game_tags: Option<serde_json::Value>,
    #[serde(default)]
    pub client_types: Vec<String>,
    #[serde(default)]
    pub channels: Vec<String>,
}

#[deprecated(note = "Legacy game-alias API; retained for compatibility only")]
#[derive(Debug, Deserialize)]
pub struct ListQuery {
    #[serde(default = "default_page")]
    pub page: i64,
    #[serde(default = "default_page_size")]
    pub page_size: i64,
    #[serde(default)]
    pub q: Option<String>,
}

fn default_page() -> i64 {
    1
}
fn default_page_size() -> i64 {
    DEFAULT_PAGE_SIZE as i64
}

fn check_write_permission(claims: &Claims) -> Result<(), ApiResponse<()>> {
    let caller_role = match Role::try_from(claims.role) {
        Ok(role) => role,
        Err(_) => {
            return Err(
                AppError::InsufficientPermission("Invalid role in token".to_string())
                    .into_response(),
            );
        }
    };
    if !matches!(caller_role, Role::System | Role::Super) {
        return Err(
            AppError::InsufficientPermission("Insufficient permissions".to_string())
                .into_response(),
        );
    }
    Ok(())
}

#[allow(deprecated)]
#[deprecated(note = "Legacy game-alias API; retained for compatibility only")]
async fn audit_event(
    pool: &sqlx::MySqlPool,
    claims: &Claims,
    op: audit::Operation,
    game_id: i64,
    game_name: &str,
) {
    let admin_id = match claims.admin_id {
        Some(id) => id,
        None => {
            tracing::warn!("No admin ID in claims for game alias audit");
            return;
        }
    };
    let operator = match admin::get_admin_by_id(pool, admin_id).await {
        Ok(Some(a)) => a,
        _ => return,
    };
    if let Err(e) = audit::record_game_alias(
        pool,
        admin_id,
        &operator.phone,
        game_id,
        game_name,
        op,
        Some(serde_json::json!({ "game_name": game_name })),
    )
    .await
    {
        tracing::error!("Failed to write game alias audit log: {}", e);
    }
}

#[allow(deprecated)]
#[deprecated(note = "Legacy game-alias API; retained for compatibility only")]
async fn list_games(
    State(state): State<AppState>,
    Query(q): Query<ListQuery>,
) -> Result<ApiResponse<GameListResponse>, ApiResponse<()>> {
    let q_str = q.q.as_deref().filter(|s| !s.is_empty());
    svc::list_games(&state.pool, q.page, q.page_size, q_str)
        .await
        .map(ApiResponse::success)
        .map_err(|e| e.into_response())
}

#[allow(deprecated)]
#[deprecated(note = "Legacy game-alias API; retained for compatibility only")]
async fn get_game(
    State(state): State<AppState>,
    Path(id): Path<i64>,
) -> Result<ApiResponse<crate::models::Game>, ApiResponse<()>> {
    svc::get_game_by_id(&state.pool, id)
        .await
        .map(|g| g.ok_or_else(|| AppError::GameAliasNotFound("游戏别名不存在".to_string())))
        .map_err(|e| e.into_response())?
        .map(ApiResponse::success)
        .map_err(|e| e.into_response())
}

#[allow(deprecated)]
#[deprecated(note = "Legacy game-alias API; retained for compatibility only")]
async fn create_game(
    State(state): State<AppState>,
    axum::Extension(claims): axum::Extension<Claims>,
    Json(req): Json<CreateGameRequest>,
) -> Result<ApiResponse<crate::models::Game>, ApiResponse<()>> {
    check_write_permission(&claims)?;

    let game = svc::create_game(&state.pool, req)
        .await
        .map_err(|e| e.into_response())?;

    audit_event(
        &state.pool,
        &claims,
        audit::Operation::GameAliasCreate,
        game.id,
        &game.name,
    )
    .await;

    Ok(ApiResponse::success(game))
}

#[allow(deprecated)]
#[deprecated(note = "Legacy game-alias API; retained for compatibility only")]
async fn update_game(
    State(state): State<AppState>,
    Path(id): Path<i64>,
    axum::Extension(claims): axum::Extension<Claims>,
    Json(req): Json<UpdateGameRequest>,
) -> Result<ApiResponse<crate::models::Game>, ApiResponse<()>> {
    check_write_permission(&claims)?;

    let game = svc::update_game(&state.pool, id, req)
        .await
        .map_err(|e| e.into_response())?;

    audit_event(
        &state.pool,
        &claims,
        audit::Operation::GameAliasUpdate,
        game.id,
        &game.name,
    )
    .await;

    Ok(ApiResponse::success(game))
}

#[allow(deprecated)]
#[deprecated(note = "Legacy game-alias API; retained for compatibility only")]
async fn delete_game(
    State(state): State<AppState>,
    Path(id): Path<i64>,
    axum::Extension(claims): axum::Extension<Claims>,
) -> Result<ApiResponse<()>, ApiResponse<()>> {
    check_write_permission(&claims)?;

    let existing = svc::get_game_by_id(&state.pool, id)
        .await
        .map_err(|e| e.into_response())?;

    let game_name = existing
        .as_ref()
        .map(|g| g.name.clone())
        .unwrap_or_default();

    let deleted = svc::delete_game(&state.pool, id)
        .await
        .map_err(|e| e.into_response())?;

    if !deleted {
        return Err(AppError::GameAliasNotFound("游戏别名不存在".to_string()).into_response());
    }

    if let Some(_game) = existing {
        audit_event(
            &state.pool,
            &claims,
            audit::Operation::GameAliasDelete,
            id,
            &game_name,
        )
        .await;
    }

    Ok(ApiResponse::success(()))
}

async fn get_external_games(
    State(state): State<AppState>,
) -> Result<ApiResponse<Vec<ExternalGameOption>>, ApiResponse<()>> {
    let ext_pool = match &state.ext_pool {
        Some(p) => p.clone(),
        None => {
            return Err(AppError::Internal("External DB unavailable".to_string()).into_response());
        }
    };

    let options = cache_helper::cached_or_fetch(
        &state.redis,
        cache_helper::KEY_EXTERNAL_GAMES,
        cache_helper::TTL_EXTERNAL_GAMES,
        || async {
            let rows = sqlx::query_as::<_, (i64, String)>(
                "SELECT id, name FROM cc_logic_game ORDER BY id",
            )
            .fetch_all(&ext_pool)
            .await
            .map_err(|e| format!("external game list: {e}"))?;
            Ok(rows
                .into_iter()
                .map(|(id, name)| ExternalGameOption { id, name })
                .collect::<Vec<_>>())
        },
    )
    .await
    .map_err(|e| AppError::Internal(e).into_response())?;

    Ok(ApiResponse::success(options))
}

/// 按 id 查询 cc_logic_game + cc_logic_game_wide 的详细数据
async fn get_external_game_detail(
    State(state): State<AppState>,
    Path(id): Path<i64>,
) -> Result<ApiResponse<ExternalGameDetail>, ApiResponse<()>> {
    let ext_pool = match &state.ext_pool {
        Some(p) => p,
        None => {
            return Err(AppError::Internal("External DB unavailable".to_string()).into_response());
        }
    };

    let row = crate::services::game_service::get_external_game_detail(ext_pool, id)
        .await
        .map_err(|e| AppError::Internal(format!("{e}")).into_response())?;

    match row {
        Some((gid, name, description, cover_image, game_tags)) => {
            // 并行查询 channels 和 client_types
            let (channels, client_types) = tokio::join!(
                crate::services::game_service::get_game_channels(ext_pool, gid),
                crate::services::game_service::get_game_client_types(ext_pool, gid),
            );
            let channels = channels.unwrap_or_default();
            let client_types = client_types.unwrap_or_default();
            Ok(ApiResponse::success(ExternalGameDetail {
                id: gid,
                name,
                description,
                cover_image,
                game_tags,
                client_types,
                channels,
            }))
        }
        None => Err(AppError::GameAliasNotFound("外部游戏不存在".to_string()).into_response()),
    }
}

/// Builds game-alias management and external-game lookup routes.
///
/// Only `/game-aliases*` is legacy and retained for compatibility.
/// `/external-games*` remains an active external-game lookup API.
#[allow(deprecated)]
pub fn router() -> Router<AppState> {
    Router::new()
        // Legacy game-alias management API retained for compatibility.
        .route("/game-aliases", get(list_games).post(create_game))
        .route(
            "/game-aliases/:id",
            get(get_game).put(update_game).delete(delete_game),
        )
        // Active external-game lookups; not part of the alias deprecation.
        .route("/external-games", get(get_external_games))
        .route("/external-games/:id", get(get_external_game_detail))
}
