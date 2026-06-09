use axum::{
    Json, Router,
    extract::{Path, Query, State},
    routing::get,
};
use serde::{Deserialize, Serialize};

use crate::api::AppState;
use crate::models::Role;
use crate::models::recommended_game_strategy::RecommendedGameStrategy;
use crate::services::recommended_game::{self as svc, CreateMeta, UpdateMeta};
use crate::utils::error::{ApiResponse, AppError};
use crate::utils::jwt::Claims;

pub fn router() -> Router<AppState> {
    Router::new()
        .route(
            "/recommended-games",
            get(list_recommended_games).post(create_recommended_game),
        )
        .route(
            "/recommended-games/:id",
            get(get_recommended_game)
                .put(update_recommended_game)
                .delete(delete_recommended_game),
        )
}

pub fn router_public() -> Router<AppState> {
    Router::new().route("/recommended-games/top", get(top_recommended_games))
}

#[derive(Debug, Deserialize)]
pub struct ListQuery {
    #[serde(default)]
    pub q: Option<String>,
    #[serde(default = "default_page")]
    pub page: i64,
    #[serde(default = "default_page_size")]
    pub page_size: i64,
}

fn default_page() -> i64 {
    1
}
fn default_page_size() -> i64 {
    20
}

#[derive(Debug, Serialize)]
pub struct ListResponse {
    pub items: Vec<GameItem>,
    pub total: i64,
}

#[derive(Debug, Serialize)]
pub struct GameItem {
    #[serde(flatten)]
    pub game: crate::models::RecommendedGame,
    pub strategies: Vec<RecommendedGameStrategy>,
}

impl GameItem {
    async fn from_game(
        pool: &sqlx::MySqlPool,
        game: crate::models::RecommendedGame,
    ) -> Result<Self, AppError> {
        let strategies = svc::fetch_strategies(pool, game.id).await?;
        Ok(Self { game, strategies })
    }
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
    let mut items_with_strategies = Vec::with_capacity(items.len());
    for game in items {
        items_with_strategies.push(
            GameItem::from_game(&state.pool, game)
                .await
                .map_err(|e| e.into_response())?,
        );
    }
    Ok(ApiResponse::success(ListResponse {
        items: items_with_strategies,
        total,
    }))
}

async fn get_recommended_game(
    State(state): State<AppState>,
    Path(id): Path<i64>,
) -> Result<ApiResponse<GameItem>, ApiResponse<()>> {
    let game = svc::fetch_by_id(&state.pool, id)
        .await
        .map_err(|e| e.into_response())?;
    let item = GameItem::from_game(&state.pool, game)
        .await
        .map_err(|e| e.into_response())?;
    Ok(ApiResponse::success(item))
}

async fn create_recommended_game(
    State(state): State<AppState>,
    axum::Extension(claims): axum::Extension<Claims>,
    Json(meta): Json<CreateMeta>,
) -> Result<ApiResponse<GameItem>, ApiResponse<()>> {
    let caller_role = match Role::try_from(claims.role) {
        Ok(role) => role,
        Err(_) => {
            return Err(
                AppError::InsufficientPermission("Invalid role in token".to_string())
                    .into_response(),
            );
        }
    };

    if !caller_role.can_manage_recommended_games() {
        return Err(
            AppError::InsufficientPermission("Insufficient permissions".to_string())
                .into_response(),
        );
    }

    let game = svc::create(&state.pool, meta)
        .await
        .map_err(|e| e.into_response())?;
    let item = GameItem::from_game(&state.pool, game)
        .await
        .map_err(|e| e.into_response())?;
    Ok(ApiResponse::success(item))
}

async fn update_recommended_game(
    State(state): State<AppState>,
    Path(id): Path<i64>,
    axum::Extension(claims): axum::Extension<Claims>,
    Json(meta): Json<UpdateMeta>,
) -> Result<ApiResponse<GameItem>, ApiResponse<()>> {
    let caller_role = match Role::try_from(claims.role) {
        Ok(role) => role,
        Err(_) => {
            return Err(
                AppError::InsufficientPermission("Invalid role in token".to_string())
                    .into_response(),
            );
        }
    };

    if !caller_role.can_manage_recommended_games() {
        return Err(
            AppError::InsufficientPermission("Insufficient permissions".to_string())
                .into_response(),
        );
    }

    let game = svc::update(&state.pool, id, meta)
        .await
        .map_err(|e| e.into_response())?;
    let item = GameItem::from_game(&state.pool, game)
        .await
        .map_err(|e| e.into_response())?;
    Ok(ApiResponse::success(item))
}

async fn delete_recommended_game(
    State(state): State<AppState>,
    Path(id): Path<i64>,
    axum::Extension(claims): axum::Extension<Claims>,
) -> Result<ApiResponse<()>, ApiResponse<()>> {
    let caller_role = match Role::try_from(claims.role) {
        Ok(role) => role,
        Err(_) => {
            return Err(
                AppError::InsufficientPermission("Invalid role in token".to_string())
                    .into_response(),
            );
        }
    };

    if !caller_role.can_delete_recommended_games() {
        return Err(
            AppError::InsufficientPermission("Insufficient permissions".to_string())
                .into_response(),
        );
    }

    match svc::delete(&state.pool, id).await {
        Ok(()) => Ok(ApiResponse::success(())),
        Err(e) => Err(e.into_response()),
    }
}

#[derive(Debug, Deserialize)]
pub struct TopQuery {
    #[serde(default)]
    pub n: Option<i64>,
    pub user_id: String,
    pub channel: String,
    pub client_type: String,
    pub client_version: String,
}

/// 对外公开的推荐游戏精简响应（去除内部字段）
#[derive(Debug, Serialize)]
pub struct TopRecommendedGame {
    pub name: String,
    pub reply: String,
    pub reason: Option<String>,
    pub tag: Option<String>,
    pub game_category: Option<serde_json::Value>,
    pub game_image: Option<String>,
    pub game_id: String,
    pub game_name: String,
}

impl From<GameItem> for TopRecommendedGame {
    fn from(item: GameItem) -> Self {
        Self {
            name: item.game.name,
            reply: item.game.reply,
            reason: item.game.reason,
            tag: item.game.tag,
            game_category: item.game.game_category,
            game_image: item.game.game_image,
            game_id: item.game.game_id,
            game_name: item.game.game_name,
        }
    }
}

async fn top_recommended_games(
    State(state): State<AppState>,
    Query(q): Query<TopQuery>,
) -> Result<ApiResponse<Vec<TopRecommendedGame>>, ApiResponse<()>> {
    let _n = q.n.unwrap_or(10).clamp(1, 50);

    if q.user_id.is_empty() || q.channel.is_empty() || q.client_type.is_empty() || q.client_version.is_empty() {
        return Err(AppError::BadRequest("user_id, channel, client_type, client_version 不能为空".into()).into_response());
    }

    let games = svc::fetch_top_filtered(
        &state.pool,
        &q.channel,
        &q.client_type,
    )
    .await
    .map_err(|e| e.into_response())?;
    let result: Vec<TopRecommendedGame> = games
        .into_iter()
        .map(|game| TopRecommendedGame {
            name: game.name,
            reply: game.reply,
            reason: game.reason,
            tag: game.tag,
            game_category: game.game_category,
            game_image: game.game_image,
            game_id: game.game_id,
            game_name: game.game_name,
        })
        .collect();
    Ok(ApiResponse::success(result))
}
