//! Legacy recommended-game HTTP API.
//!
//! The admin CRUD routes and the public `top`/`execute` routes are deprecated
//! business functionality. They remain registered only for compatibility with
//! existing clients and data; do not add new callers or extend this API.

// This module is the compatibility implementation of the deprecated API, so
// its handlers necessarily use deprecated request, model, and service items.
#![allow(deprecated)]

use axum::{
    Json, Router,
    extract::{Path, Query, State},
    routing::{get, post},
};
use redis::AsyncCommands;
use std::collections::HashMap;

use serde::{Deserialize, Serialize};

use crate::api::AppState;
use crate::api::chat_common;
use crate::models::Role;
use crate::models::chat_user::ChatMessageUser;
use crate::models::recommended_game_strategy::RecommendedGameStrategy;
use crate::services::chat_user as chat_svc;
use crate::services::membership;
use crate::services::recommended_game::{self as svc, CreateMeta, StrategyMeta, UpdateMeta};
use crate::services::user_auth;
use crate::utils::error::{ApiResponse, AppError};
use crate::utils::jwt::Claims;

/// Expand `"*"` in strategy channel/client_type arrays to all available values
/// for the given game, queried from the external DB.
async fn expand_strategy_wildcards(
    ext_pool: Option<&sqlx::MySqlPool>,
    game_id: &str,
    strategies: &mut [StrategyMeta],
) {
    let gid: i64 = match game_id.parse() {
        Ok(id) => id,
        Err(_) => return,
    };
    let ext_pool = match ext_pool {
        Some(p) => p,
        None => return,
    };

    let (channels, client_types) = tokio::join!(
        crate::services::game_service::get_game_channels(ext_pool, gid),
        crate::services::game_service::get_game_client_types(ext_pool, gid),
    );
    let channels = channels.unwrap_or_default();
    let client_types = client_types.unwrap_or_default();

    for s in strategies {
        if s.channel
            .as_array()
            .is_some_and(|a| a.iter().any(|v| v == "*"))
        {
            s.channel = serde_json::to_value(&channels).unwrap_or_default();
        }
        if s.client_type
            .as_array()
            .is_some_and(|a| a.iter().any(|v| v == "*"))
        {
            s.client_type = serde_json::to_value(&client_types).unwrap_or_default();
        }
    }
}

/// Builds the legacy admin CRUD routes under `/api/recommended-games`.
///
/// These routes remain available for compatibility; the feature is no longer
/// developed.
#[deprecated(note = "Legacy recommended-game API; retained for compatibility only")]
pub fn router() -> Router<AppState> {
    Router::new()
        .route(
            "/recommended-games",
            get(list_recommended_games).post(create_recommended_game),
        )
        .route("/recommended-games/game-ids", get(get_existing_game_ids))
        .route(
            "/recommended-games/:id",
            get(get_recommended_game)
                .put(update_recommended_game)
                .delete(delete_recommended_game),
        )
}

/// Builds the legacy public recommendation routes.
///
/// `/api/recommended-games/top` and `/api/recommended-games/execute` remain
/// available for compatibility; new integrations must not depend on them.
#[deprecated(note = "Legacy recommended-game API; retained for compatibility only")]
pub fn router_public() -> Router<AppState> {
    Router::new()
        .route("/recommended-games/top", post(top_recommended_games))
        .route("/recommended-games/execute", post(execute_recommendation))
}

#[deprecated(note = "Legacy recommended-game API; retained for compatibility only")]
#[derive(Debug, Deserialize)]
pub struct ListQuery {
    #[serde(default)]
    pub q: Option<String>,
    #[serde(default)]
    pub channel: Option<String>,
    #[serde(default)]
    pub client_type: Option<String>,
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

#[deprecated(note = "Legacy recommended-game API; retained for compatibility only")]
#[derive(Debug, Serialize)]
pub struct ListResponse {
    pub items: Vec<GameItem>,
    pub total: i64,
}

#[deprecated(note = "Legacy recommended-game API; retained for compatibility only")]
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

#[deprecated(note = "Legacy recommended-game API; retained for compatibility only")]
async fn list_recommended_games(
    State(state): State<AppState>,
    Query(q): Query<ListQuery>,
) -> Result<ApiResponse<ListResponse>, ApiResponse<()>> {
    let (items, total) = svc::list(
        &state.pool,
        q.q.as_deref().filter(|s| !s.is_empty()),
        q.channel.as_deref().filter(|s| !s.is_empty()),
        q.client_type.as_deref().filter(|s| !s.is_empty()),
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

#[deprecated(note = "Legacy recommended-game API; retained for compatibility only")]
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

#[deprecated(note = "Legacy recommended-game API; retained for compatibility only")]
async fn get_existing_game_ids(
    State(state): State<AppState>,
) -> Result<ApiResponse<Vec<String>>, ApiResponse<()>> {
    let ids = svc::fetch_all_game_ids(&state.pool)
        .await
        .map_err(|e| e.into_response())?;
    Ok(ApiResponse::success(ids))
}

#[deprecated(note = "Legacy recommended-game API; retained for compatibility only")]
async fn create_recommended_game(
    State(state): State<AppState>,
    axum::Extension(claims): axum::Extension<Claims>,
    Json(mut meta): Json<CreateMeta>,
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

    // Expand '*' wildcards in strategies to actual values from external DB
    if let Some(ref mut strategies) = meta.strategies {
        expand_strategy_wildcards(state.ext_pool.as_ref(), &meta.game_id, strategies).await;
    }

    let game = svc::create(&state.pool, meta)
        .await
        .map_err(|e| e.into_response())?;
    let item = GameItem::from_game(&state.pool, game)
        .await
        .map_err(|e| e.into_response())?;
    Ok(ApiResponse::success(item))
}

#[deprecated(note = "Legacy recommended-game API; retained for compatibility only")]
async fn update_recommended_game(
    State(state): State<AppState>,
    Path(id): Path<i64>,
    axum::Extension(claims): axum::Extension<Claims>,
    Json(mut meta): Json<UpdateMeta>,
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

    // Expand '*' wildcards in strategies to actual values from external DB
    if let Some(ref mut strategies) = meta.strategies {
        // Use provided game_id, or fall back to existing game's game_id
        let gid: String = if let Some(ref gid) = meta.game_id {
            gid.clone()
        } else {
            let existing = svc::fetch_by_id(&state.pool, id)
                .await
                .map_err(|e| e.into_response())?;
            existing.game_id
        };
        expand_strategy_wildcards(state.ext_pool.as_ref(), &gid, strategies).await;
    }

    let game = svc::update(&state.pool, id, meta)
        .await
        .map_err(|e| e.into_response())?;
    let item = GameItem::from_game(&state.pool, game)
        .await
        .map_err(|e| e.into_response())?;
    Ok(ApiResponse::success(item))
}

#[deprecated(note = "Legacy recommended-game API; retained for compatibility only")]
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

/// 对外公开的推荐游戏精简响应（去除内部字段）
#[deprecated(note = "Legacy recommended-game API; retained for compatibility only")]
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

#[deprecated(note = "Legacy recommended-game API; retained for compatibility only")]
#[derive(Debug, Deserialize)]
struct TopRequest {
    user_id: String,
    channel: String,
    client_type: String,
    client_version: String,
}

#[deprecated(note = "Legacy recommended-game API; retained for compatibility only")]
async fn top_recommended_games(
    State(state): State<AppState>,
    Query(params): Query<HashMap<String, String>>,
    body: String,
) -> Result<ApiResponse<Vec<TopRecommendedGame>>, ApiResponse<()>> {
    // 1. MD5 签名校验
    let secret = chat_common::get_assistant_secret();
    if !secret.is_empty() {
        let sign = params.get("sign").map(|s| s.as_str()).unwrap_or("");
        if !chat_common::verify_sign(secret, "/api/recommended-games/top", &body, sign) {
            return Err(AppError::BadRequest("Invalid signature".into()).into_response());
        }
    }

    // 2. 解析 JSON body
    let req: TopRequest = serde_json::from_str(&body)
        .map_err(|e| AppError::BadRequest(format!("invalid JSON: {e}")).into_response())?;

    if req.user_id.is_empty()
        || req.channel.is_empty()
        || req.client_type.is_empty()
        || req.client_version.is_empty()
    {
        return Err(AppError::BadRequest(
            "user_id, channel, client_type, client_version 不能为空".into(),
        )
        .into_response());
    }

    let ext_pool = match &state.ext_pool {
        Some(p) => p,
        None => {
            return Err(AppError::Internal("外部数据库未配置".into()).into_response::<()>());
        }
    };
    let games = svc::fetch_top_filtered(ext_pool, &req.channel, &req.client_type)
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

// ── 执行推荐：记录两条聊天消息 ──

#[deprecated(note = "Legacy recommended-game API; retained for compatibility only")]
#[derive(Debug, Deserialize)]
struct ExecuteRequest {
    user_id: String,
    game_id: String,
    channel: String,
    client_type: String,
    #[expect(
        dead_code,
        reason = "accepted for legacy request compatibility but not used by recommendation logic"
    )]
    client_version: String,
}

#[deprecated(note = "Legacy recommended-game API; retained for compatibility only")]
async fn execute_recommendation(
    State(state): State<AppState>,
    Query(params): Query<HashMap<String, String>>,
    body: String,
) -> Result<ApiResponse<ChatMessageUser>, ApiResponse<()>> {
    // 1. MD5 签名校验
    let secret = chat_common::get_assistant_secret();
    if !secret.is_empty() {
        let sign = params.get("sign").map(|s| s.as_str()).unwrap_or("");
        if !chat_common::verify_sign(secret, "/api/recommended-games/execute", &body, sign) {
            return Err(AppError::BadRequest("Invalid signature".into()).into_response());
        }
    }

    // 2. 解析请求
    let req: ExecuteRequest = serde_json::from_str(&body)
        .map_err(|e| AppError::BadRequest(format!("invalid JSON: {e}")).into_response())?;

    if req.user_id.trim().is_empty() || req.game_id.trim().is_empty() {
        return Err(AppError::BadRequest("user_id, game_id 不能为空".into()).into_response());
    }

    let user_id: i64 = req.user_id.trim().parse().map_err(|_| {
        AppError::BadRequest("user_id must be a number".into()).into_response::<()>()
    })?;

    let ext_pool = match &state.ext_pool {
        Some(p) => p,
        None => {
            return Err(AppError::Internal("外部数据库未配置".into()).into_response::<()>());
        }
    };
    // 3. 查询推荐游戏
    let game = svc::fetch_by_game_id(ext_pool, &req.game_id)
        .await
        .map_err(|e| e.into_response())?;

    // 3b. 查询外部 DB 获取 computer_id / platform_name / game_icon/description（带缓存，按平台优先级排序）
    let (computer_id, platform_name, game_icon, _description) =
        crate::services::game_service::get_single_external_game_info(
            ext_pool,
            game.game_id.parse::<i64>().unwrap_or(0),
            &req.client_type,
            &req.channel,
        )
        .await
        .unwrap_or(None)
        .map(|info| {
            (
                info.computer_id,
                info.platform_name,
                info.game_icon,
                info.description,
            )
        })
        .unwrap_or((None, None, None, None));

    // 3c. 同步用户：确保 users 表存在该用户，避免 chat_sessions_user 外键约束失败
    let cloud_info = membership::get_cloud_user_info_cached(&state.redis, ext_pool, user_id)
        .await
        .unwrap_or_else(|e| {
            tracing::warn!(user_id = user_id, error = %e, "get_cloud_user_info_cached failed");
            None
        });

    let (uid, nickname) = cloud_info
        .as_ref()
        .map(|(u, n)| (Some(u.as_str()), Some(n.as_str())))
        .unwrap_or((None, None));

    user_auth::ensure_user_exists(&state.pool, user_id, uid, nickname)
        .await
        .map_err(|e| AppError::Internal(format!("user sync: {e}")).into_response::<()>())?;

    // 4. 获取或创建 session
    let session = chat_svc::get_or_create_session_user(&state.pool, user_id)
        .await
        .map_err(|e| e.into_response::<()>())?;

    // 5. 记录用户消息（content = game.reply）
    chat_svc::append_user_message_user(&state.pool, session.id, user_id, &game.name)
        .await
        .map_err(|e| e.into_response::<()>())?;

    // 6. 构建 card extensions
    let card = serde_json::json!({
        "content_type": "card",
        "payload": {
            "type": "game",
            "info": {
                "id": game.game_id,
                "name": game.game_name,
                "channel": req.channel,
                "client_type": req.client_type,
                "reason": game.reason,
                "game_tags": game.game_category,
                "cover_image": game.game_image,
                "computer_id": computer_id,
                "platform_name": platform_name,
                "game_icon": game_icon,
            }
        }
    });
    let extensions = serde_json::json!([card]);

    // 7. 记录 assistant 消息（extensions = card）
    let mut saved = chat_svc::append_assistant_message_user(
        &state.pool,
        session.id,
        user_id,
        &game.reply,
        None,
        Some(extensions),
    )
    .await
    .map_err(|e| e.into_response::<()>())?;

    // 8. 追加 usage extension（参考 assistant_chat 逻辑）
    let is_vip = membership::check_vip_membership(ext_pool, user_id)
        .await
        .unwrap_or(false);

    let limit_config = membership::get_ai_assistant_chat_limit_config(ext_pool)
        .await
        .unwrap_or(None)
        .unwrap_or_default();

    let total_times = if is_vip {
        limit_config.vip_ask_times
    } else {
        limit_config.normal_ask_times
    };

    let limit_key = format!("assistant:daily:{}", user_id);
    let current_count: i64 = {
        match state.redis.get_multiplexed_async_connection().await {
            Ok(mut conn) => conn.get(&limit_key).await.unwrap_or(0),
            Err(_) => 0,
        }
    };

    let remaining = total_times - current_count;
    if limit_config.remain_ask_time > 0 && remaining <= limit_config.remain_ask_time {
        let usage_ext = serde_json::json!({
            "content_type": "usage",
            "payload": {
                "used_times": current_count,
                "total_times": total_times,
                "membership_max_times": limit_config.vip_ask_times,
                "remain_ask_time": limit_config.remain_ask_time,
            }
        });
        let mut exts: Vec<serde_json::Value> = saved
            .extensions
            .as_ref()
            .and_then(|v| v.as_array())
            .cloned()
            .unwrap_or_default();
        exts.push(usage_ext);
        saved.extensions = Some(serde_json::Value::Array(exts));
    }

    Ok(ApiResponse::success(saved))
}
