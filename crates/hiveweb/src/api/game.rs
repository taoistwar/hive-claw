use axum::{
    Json, Router,
    extract::{Path, Query, State},
    routing::{get, post},
};
use redis::AsyncCommands;
use serde::{Deserialize, Serialize};

use crate::api::AppState;
use crate::models::Role;
use crate::models::game::{
    CreateGameRequest, DEFAULT_PAGE_SIZE, GameListResponse, UpdateGameRequest,
};
use crate::services::game_service::{self as svc};
use crate::services::{admin, audit};
use crate::utils::error::{ApiResponse, AppError};
use crate::utils::jwt::Claims;

/// 外部游戏列表缓存 Key + TTL（30 分钟）
const EXTERNAL_GAMES_CACHE_KEY: &str = "external_games:list";
const EXTERNAL_GAMES_CACHE_TTL: u64 = 1800;

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
    pub client_types: Option<serde_json::Value>,
    #[serde(default)]
    pub channels: Vec<String>,
}

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
    // 1. 尝试从 Redis 缓存读取
    match try_cache_read(&state.redis).await {
        Ok(Some(options)) => return Ok(ApiResponse::success(options)),
        Ok(None) => {} // cache miss, continue
        Err(e) => tracing::warn!("external games cache read failed: {e}"),
    }

    // 2. 缓存未命中，从外部 DB 查询
    let ext_pool = match &state.ext_pool {
        Some(p) => p,
        None => {
            return Err(AppError::Internal("External DB unavailable".to_string()).into_response());
        }
    };
    let rows = sqlx::query_as::<_, (i64, String)>(
        "SELECT id, name FROM cc_logic_game ORDER BY id",
    )
    .fetch_all(ext_pool)
    .await
    .map_err(|e| AppError::Internal(format!("external game list: {}", e)).into_response())?;
    let options: Vec<ExternalGameOption> = rows
        .into_iter()
        .map(|(id, name)| ExternalGameOption {
            id,
            name,
        })
        .collect();

    // 3. 写入缓存（best-effort，失败不阻塞响应）
    if let Err(e) = try_cache_write(&state.redis, &options).await {
        tracing::warn!("external games cache write failed: {e}");
    }

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

    let row = sqlx::query_as::<_, (i64, String, Option<String>, Option<String>, Option<serde_json::Value>)>(
        "SELECT g.id, g.name, w.description, w.cover_image, w.game_tags
         FROM cc_logic_game g
         LEFT JOIN cc_logic_game_wide w ON w.logic_game_id = g.id
         WHERE g.id = ?",
    )
    .bind(id)
    .fetch_optional(ext_pool)
    .await
    .map_err(|e| AppError::Internal(format!("external game detail: {}", e)).into_response())?;

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

/// 从 Redis 读取缓存的外部游戏列表
async fn try_cache_read(redis: &redis::Client) -> Result<Option<Vec<ExternalGameOption>>, String> {
    let mut conn = redis
        .get_multiplexed_async_connection()
        .await
        .map_err(|e| format!("redis connect: {e}"))?;
    let cached: Option<String> = conn
        .get(EXTERNAL_GAMES_CACHE_KEY)
        .await
        .map_err(|e| format!("redis get: {e}"))?;
    match cached {
        Some(json) => {
            let options: Vec<ExternalGameOption> =
                serde_json::from_str(&json).map_err(|e| format!("deserialize: {e}"))?;
            Ok(Some(options))
        }
        None => Ok(None),
    }
}

/// 将外部游戏列表写入 Redis 缓存（30 分钟 TTL）
async fn try_cache_write(
    redis: &redis::Client,
    options: &[ExternalGameOption],
) -> Result<(), String> {
    let mut conn = redis
        .get_multiplexed_async_connection()
        .await
        .map_err(|e| format!("redis connect: {e}"))?;
    let json = serde_json::to_string(options).map_err(|e| format!("serialize: {e}"))?;
    let _: () = conn
        .set_ex(EXTERNAL_GAMES_CACHE_KEY, json, EXTERNAL_GAMES_CACHE_TTL)
        .await
        .map_err(|e| format!("redis setex: {e}"))?;
    Ok(())
}

pub fn router() -> Router<AppState> {
    Router::new()
        .route("/game-aliases", get(list_games).post(create_game))
        .route(
            "/game-aliases/:id",
            get(get_game).put(update_game).delete(delete_game),
        )
        .route("/external-games", get(get_external_games))
        .route("/external-games/:id", get(get_external_game_detail))
}
