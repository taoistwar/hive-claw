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

pub async fn fetch_by_game_id(
    pool: &MySqlPool,
    game_id: &str,
) -> Result<RecommendedGame, AppError> {
    sqlx::query_as::<_, RecommendedGame>(
        "SELECT * FROM recommended_games WHERE game_id = ?",
    )
    .bind(game_id)
    .fetch_optional(pool)
    .await
    .map_err(|e| AppError::Internal(format!("recommended_game fetch by game_id: {e}")))?
    .ok_or_else(|| AppError::NotFound(format!("recommended_game game_id={game_id} not found")))
}

pub async fn list(
    pool: &MySqlPool,
    q: Option<&str>,
    channel: Option<&str>,
    client_type: Option<&str>,
    page: i64,
    page_size: i64,
) -> Result<(Vec<RecommendedGame>, i64), AppError> {
    let has_q = q.is_some();
    let has_ch = channel.is_some();
    let has_ct = client_type.is_some();

    // Precompute bind values so they live long enough
    let like_q = q.map(|k| format!("%{}%", k));
    let ch_json = channel.map(|c| format!("\"{}\"", c));
    let ct_json = client_type.map(|c| format!("\"{}\"", c));

    // Build WHERE conditions
    let mut conditions: Vec<String> = Vec::new();
    if has_q {
        conditions.push("(rg.name LIKE ? OR rg.game_name LIKE ? OR rg.game_id LIKE ?)".into());
    }
    if has_ch || has_ct {
        let mut strat_parts: Vec<String> = Vec::new();
        if has_ch {
            strat_parts.push("JSON_CONTAINS(s.channel, ?)".into());
        }
        if has_ct {
            strat_parts.push("JSON_CONTAINS(s.client_type, ?)".into());
        }
        conditions.push(format!(
            "EXISTS (SELECT 1 FROM recommended_games_strategy s WHERE s.recommended_game_id = rg.id AND s.strategy = 'INCLUDE' AND {})",
            strat_parts.join(" AND ")
        ));
    }

    let where_clause = if conditions.is_empty() {
        String::new()
    } else {
        format!("WHERE {}", conditions.join(" AND "))
    };

    // Count query
    let count_sql = format!("SELECT COUNT(*) FROM recommended_games rg {where_clause}");
    let mut count_query = sqlx::query_as::<_, (i64,)>(&count_sql);
    if let Some(ref v) = like_q {
        count_query = count_query.bind(v).bind(v).bind(v);
    }
    if let Some(ref v) = ch_json {
        count_query = count_query.bind(v);
    }
    if let Some(ref v) = ct_json {
        count_query = count_query.bind(v);
    }
    let total: (i64,) = count_query.fetch_one(pool).await
        .map_err(|e| AppError::Internal(format!("recommended_game count: {e}")))?;

    // Data query
    let offset = (page - 1) * page_size;
    let data_sql = format!(
        "SELECT rg.* FROM recommended_games rg {where_clause} ORDER BY rg.sort_value DESC, rg.created_at DESC LIMIT ? OFFSET ?"
    );
    let mut data_query = sqlx::query_as::<_, RecommendedGame>(&data_sql);
    if let Some(ref v) = like_q {
        data_query = data_query.bind(v).bind(v).bind(v);
    }
    if let Some(ref v) = ch_json {
        data_query = data_query.bind(v);
    }
    if let Some(ref v) = ct_json {
        data_query = data_query.bind(v);
    }
    data_query = data_query.bind(page_size).bind(offset);
    let items: Vec<RecommendedGame> = data_query.fetch_all(pool).await
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

/// Fetch all existing game_id values from recommended_games.
pub async fn fetch_all_game_ids(pool: &MySqlPool) -> Result<Vec<String>, AppError> {
    let rows: Vec<(String,)> =
        sqlx::query_as("SELECT game_id FROM recommended_games")
            .fetch_all(pool)
            .await
            .map_err(|e| AppError::Internal(format!("recommended_game fetch_all_game_ids: {e}")))?;
    Ok(rows.into_iter().map(|(id,)| id).collect())
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

/// Query recommended games for a single tag with strategy channel/client_type filtering.
async fn fetch_by_tag(
    pool: &MySqlPool,
    tag: &str,
    ch: &str,
    ct: &str,
) -> Result<Vec<RecommendedGame>, AppError> {
    sqlx::query_as::<_, RecommendedGame>(
        r#"SELECT rg.* FROM recommended_games rg
WHERE rg.tag = ?
  AND EXISTS (
    SELECT 1 FROM recommended_games_strategy si
    WHERE si.recommended_game_id = rg.id
      AND si.strategy = 'INCLUDE'
      AND JSON_CONTAINS(si.channel, ?)
      AND JSON_CONTAINS(si.client_type, ?)
  )
  AND NOT EXISTS (
    SELECT 1 FROM recommended_games_strategy se
    WHERE se.recommended_game_id = rg.id
      AND se.strategy = 'EXCLUDE'
      AND JSON_CONTAINS(se.channel, ?)
      AND JSON_CONTAINS(se.client_type, ?)
  )
ORDER BY rg.sort_value DESC, rg.created_at DESC
LIMIT ?"#,
    )
    .bind(tag)
    .bind(ch).bind(ct)
    .bind(ch).bind(ct)
    .bind(10i64)
    .fetch_all(pool)
    .await
    .map_err(|e| AppError::Internal(format!("fetch_by_tag {tag}: {e}")))
}

/// Filtered top games with strategy-based channel/client_type filtering and per-tag limits.
/// Fetches 10 candidates per tag, then randomly selects the required number.
/// - 运营推荐: 4, 新游上线: 3, 本周热玩: 3
/// - If total < 10 after per-tag selection, fills up from remaining candidates of other tags.
pub async fn fetch_top_filtered(
    pool: &MySqlPool,
    channel: &str,
    client_type: &str,
) -> Result<Vec<RecommendedGame>, AppError> {
    use rand::seq::SliceRandom;
    use std::collections::HashSet;

    let tags = [
        ("运营推荐", 4usize),
        ("新游上线", 3usize),
        ("本周热玩", 3usize),
    ];

    let mut result: Vec<RecommendedGame> = Vec::new();
    let mut selected_ids: HashSet<i64> = HashSet::new();
    // Store all fetched candidates for fallback fill-up
    let mut all_candidates: Vec<RecommendedGame> = Vec::new();
    let ch = format!("\"{}\"", channel);
    let ct = format!("\"{}\"", client_type);

    for (tag, limit) in &tags {
        let limit = *limit;
        let rows = fetch_by_tag(pool, tag, &ch, &ct).await?;

        // Create RNG per iteration — must not cross .await boundary (thread_rng is !Send)
        let mut rng = rand::thread_rng();
        let selected: Vec<RecommendedGame> = rows
            .choose_multiple(&mut rng, limit.min(rows.len()))
            .cloned()
            .collect();
        for g in &selected {
            selected_ids.insert(g.id);
        }
        result.extend(selected);
        // Keep remaining candidates for fill-up
        all_candidates.extend(rows);
    }

    // Fill up to 10 from remaining candidates of other tags
    if result.len() < 10 {
        let remaining: Vec<&RecommendedGame> = all_candidates
            .iter()
            .filter(|g| !selected_ids.contains(&g.id))
            .collect();
        if !remaining.is_empty() {
            let mut rng = rand::thread_rng();
            let needed = 10 - result.len();
            let fill: Vec<RecommendedGame> = remaining
                .choose_multiple(&mut rng, needed.min(remaining.len()))
                .into_iter()
                .cloned()
                .cloned()
                .collect();
            for g in &fill {
                selected_ids.insert(g.id);
            }
            result.extend(fill);
        }
    }

    Ok(result)
}



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
