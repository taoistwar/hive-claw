use chrono::{DateTime, Utc};
use serde::Deserialize;
use sqlx::MySqlPool;

use crate::models::RecommendedGame;
use crate::utils::error::AppError;

#[derive(Debug, Deserialize)]
pub struct CreateMeta {
    pub name: String,
    pub reply: String,
    pub reason: Option<String>,
    pub tag: Option<String>,
    pub game_category: Option<serde_json::Value>,
    pub game_image: Option<String>,
    pub sort_value: Option<i32>,
    pub game_id: String,
    pub game_name: String,
    pub strategies: Option<Vec<StrategyMeta>>,
}

#[derive(Debug, Deserialize)]
pub struct UpdateMeta {
    pub name: Option<String>,
    pub reply: Option<String>,
    pub reason: Option<String>,
    pub tag: Option<String>,
    pub game_category: Option<serde_json::Value>,
    pub game_image: Option<String>,
    pub sort_value: Option<i32>,
    pub game_id: Option<String>,
    pub game_name: Option<String>,
    pub strategies: Option<Vec<StrategyMeta>>,
    pub updated_at: Option<DateTime<Utc>>,
}

#[derive(Debug, Deserialize)]
pub struct StrategyMeta {
    pub channel: serde_json::Value,
    pub client_type: serde_json::Value,
    pub strategy: String,
}

pub async fn create(pool: &MySqlPool, meta: CreateMeta) -> Result<RecommendedGame, AppError> {
    let res = sqlx::query(
        "INSERT INTO recommended_games (name, reply, reason, tag, game_category, game_image, sort_value, game_id, game_name) VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?)"
    )
    .bind(&meta.name)
    .bind(&meta.reply)
    .bind(&meta.reason)
    .bind(&meta.tag)
    .bind(&meta.game_category)
    .bind(&meta.game_image)
    .bind(meta.sort_value.unwrap_or(0))
    .bind(&meta.game_id)
    .bind(&meta.game_name)
    .execute(pool)
    .await
    .map_err(|e| {
        let msg = e.to_string();
        if msg.contains("Duplicate") {
            AppError::Conflict(format!("game_id 已存在：{}", meta.game_id))
        } else {
            AppError::Internal(format!("recommended_game insert: {e}"))
        }
    })?;
    let game_id = res.last_insert_id() as i64;
    // Insert strategies
    if let Some(ref strategies) = meta.strategies {
        insert_strategies(pool, game_id, strategies).await?;
    }
    fetch_by_id(pool, game_id).await
}

pub async fn fetch_by_id(pool: &MySqlPool, id: i64) -> Result<RecommendedGame, AppError> {
    sqlx::query_as::<_, RecommendedGame>("SELECT * FROM recommended_games WHERE id = ?")
        .bind(id)
        .fetch_optional(pool)
        .await
        .map_err(|e| AppError::Internal(format!("recommended_game fetch: {e}")))?
        .ok_or_else(|| AppError::NotFound(format!("recommended_game id={id} not found")))
}

pub async fn list(
    pool: &MySqlPool,
    q: Option<&str>,
    page: i64,
    page_size: i64,
) -> Result<(Vec<RecommendedGame>, i64), AppError> {
    let where_clause = if let Some(keyword) = q {
        let like = format!("%{keyword}%");
        format!("WHERE name LIKE ? OR game_name LIKE ? OR game_id LIKE ?")
    } else {
        String::new()
    };

    let count_sql = format!("SELECT COUNT(*) FROM recommended_games {where_clause}");
    let total: (i64,) = if let Some(keyword) = q {
        let like = format!("%{keyword}%");
        sqlx::query_as(&count_sql)
            .bind(&like)
            .bind(&like)
            .bind(&like)
            .fetch_one(pool)
            .await
    } else {
        sqlx::query_as(&count_sql).fetch_one(pool).await
    }
    .map_err(|e| AppError::Internal(format!("recommended_game count: {e}")))?;

    let offset = (page - 1) * page_size;
    let data_sql = format!(
        "SELECT * FROM recommended_games {where_clause} ORDER BY sort_value DESC, created_at DESC LIMIT ? OFFSET ?"
    );
    let items: Vec<RecommendedGame> = if let Some(keyword) = q {
        let like = format!("%{keyword}%");
        sqlx::query_as(&data_sql)
            .bind(&like)
            .bind(&like)
            .bind(&like)
            .bind(page_size)
            .bind(offset)
            .fetch_all(pool)
            .await
    } else {
        sqlx::query_as(&data_sql)
            .bind(page_size)
            .bind(offset)
            .fetch_all(pool)
            .await
    }
    .map_err(|e| AppError::Internal(format!("recommended_game list: {e}")))?;

    Ok((items, total.0))
}

pub async fn update(
    pool: &MySqlPool,
    id: i64,
    meta: UpdateMeta,
) -> Result<RecommendedGame, AppError> {
    if let Some(updated_at) = meta.updated_at {
        crate::services::optimistic_lock::check_and_bump(pool, "recommended_games", id, updated_at)
            .await?;
    }
    sqlx::query(
        "UPDATE recommended_games SET name = COALESCE(?, name), reply = COALESCE(?, reply), reason = COALESCE(?, reason), tag = COALESCE(?, tag), game_category = COALESCE(?, game_category), game_image = COALESCE(?, game_image), sort_value = COALESCE(?, sort_value), game_id = COALESCE(?, game_id), game_name = COALESCE(?, game_name) WHERE id = ?"
    )
    .bind(&meta.name)
    .bind(&meta.reply)
    .bind(&meta.reason)
    .bind(&meta.tag)
    .bind(&meta.game_category)
    .bind(&meta.game_image)
    .bind(meta.sort_value)
    .bind(&meta.game_id)
    .bind(&meta.game_name)
    .bind(id)
    .execute(pool)
    .await
    .map_err(|e| {
        let msg = e.to_string();
        if msg.contains("Duplicate") {
            AppError::Conflict("game_id 已存在".to_string())
        } else {
            AppError::Internal(format!("recommended_game update: {e}"))
        }
    })?;
    // Replace strategies if provided
    if let Some(ref strategies) = meta.strategies {
        delete_strategies(pool, id).await?;
        insert_strategies(pool, id, strategies).await?;
    }
    fetch_by_id(pool, id).await
}

pub async fn delete(pool: &MySqlPool, id: i64) -> Result<(), AppError> {
    let res = sqlx::query("DELETE FROM recommended_games WHERE id = ?")
        .bind(id)
        .execute(pool)
        .await
        .map_err(|e| AppError::Internal(format!("recommended_game delete: {e}")))?;
    if res.rows_affected() == 0 {
        return Err(AppError::NotFound(format!(
            "recommended_game id={id} not found"
        )));
    }
    Ok(())
}

pub async fn fetch_top_n(pool: &MySqlPool, n: i64) -> Result<Vec<RecommendedGame>, AppError> {
    sqlx::query_as::<_, RecommendedGame>(
        "SELECT * FROM recommended_games ORDER BY sort_value DESC, created_at DESC LIMIT ?",
    )
    .bind(n)
    .fetch_all(pool)
    .await
    .map_err(|e| AppError::Internal(format!("recommended_game top: {e}")))
}

/// Filtered top games with strategy-based channel/client_type filtering and per-tag limits.
/// Uses one SQL query per tag category, filtering via SQL JOINs + JSON_CONTAINS.
/// - 新游上线: 3, 运营推荐: 4, 本周热玩: 3
pub async fn fetch_top_filtered(
    pool: &MySqlPool,
    channel: &str,
    client_type: &str,
) -> Result<Vec<RecommendedGame>, AppError> {
    let tags = [
        ("运营推荐", 4i64),
        ("新游上线", 3i64),
        ("本周热玩", 3i64),
    ];

    let mut result: Vec<RecommendedGame> = Vec::new();
    let ch = format!("\"{}\"", channel);
    let ct = format!("\"{}\"", client_type);

    for (tag, limit) in &tags {
        let rows = sqlx::query_as::<_, RecommendedGame>(FILTERED_SQL)
            .bind(tag)
            .bind(&ch).bind(&ct)
            .bind(&ch).bind(&ct)
            .bind(limit)
            .fetch_all(pool)
            .await
            .map_err(|e| AppError::Internal(format!("fetch_top_filtered {tag}: {e}")))?;
        result.extend(rows);
    }

    Ok(result)
}

const FILTERED_SQL: &str = r#"
SELECT rg.* FROM recommended_games rg
WHERE rg.tag = ?
  AND EXISTS (
    SELECT 1 FROM recommended_games_strategy si
    WHERE si.recommended_game_id = rg.id
      AND si.strategy = 'INCLUDE'
      AND (JSON_CONTAINS(si.channel, '"*"') OR JSON_CONTAINS(si.channel, ?))
      AND (JSON_CONTAINS(si.client_type, '"*"') OR JSON_CONTAINS(si.client_type, ?))
  )
  AND NOT EXISTS (
    SELECT 1 FROM recommended_games_strategy se
    WHERE se.recommended_game_id = rg.id
      AND se.strategy = 'EXCLUDE'
      AND (JSON_CONTAINS(se.channel, '"*"') OR JSON_CONTAINS(se.channel, ?))
      AND (JSON_CONTAINS(se.client_type, '"*"') OR JSON_CONTAINS(se.client_type, ?))
  )
ORDER BY rg.sort_value DESC, rg.created_at DESC
LIMIT ?
"#;

// --- strategy helpers ---

async fn insert_strategies(
    pool: &MySqlPool,
    game_id: i64,
    strategies: &[StrategyMeta],
) -> Result<(), AppError> {
    for s in strategies {
        sqlx::query(
            "INSERT INTO recommended_games_strategy (recommended_game_id, channel, client_type, strategy) VALUES (?, ?, ?, ?)",
        )
        .bind(game_id)
        .bind(&s.channel)
        .bind(&s.client_type)
        .bind(&s.strategy)
        .execute(pool)
        .await
        .map_err(|e| AppError::Internal(format!("strategy insert: {e}")))?;
    }
    Ok(())
}

async fn delete_strategies(pool: &MySqlPool, game_id: i64) -> Result<(), AppError> {
    sqlx::query("DELETE FROM recommended_games_strategy WHERE recommended_game_id = ?")
        .bind(game_id)
        .execute(pool)
        .await
        .map_err(|e| AppError::Internal(format!("strategy delete: {e}")))?;
    Ok(())
}

pub async fn fetch_strategies(
    pool: &MySqlPool,
    game_id: i64,
) -> Result<Vec<crate::models::recommended_game_strategy::RecommendedGameStrategy>, AppError> {
    sqlx::query_as::<_, crate::models::recommended_game_strategy::RecommendedGameStrategy>(
        "SELECT * FROM recommended_games_strategy WHERE recommended_game_id = ? ORDER BY id",
    )
    .bind(game_id)
    .fetch_all(pool)
    .await
    .map_err(|e| AppError::Internal(format!("strategy fetch: {e}")))
}
