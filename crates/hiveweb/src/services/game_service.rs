//! Game data services.
//!
//! The local `games`/`game_alias_entries` CRUD functions at the start of this
//! module back the deprecated game-alias management feature and are retained
//! only for compatibility. External-database query helpers later in this file
//! are still used by the assistant runtime and are not deprecated.

use serde::{Deserialize, Serialize};
use sqlx::{MySqlPool, Row};
use std::collections::HashSet;

use crate::cache::redis::RedisClient;
use crate::db::sql_safety::audit_sql;
#[allow(deprecated)]
use crate::models::game::{
    CreateGameRequest, DEFAULT_PAGE_SIZE, Game, GameListResponse, GameResponse, MAX_ALIAS_LENGTH,
    MAX_ALIASES_COUNT, MAX_NAME_LENGTH, MAX_PAGE_SIZE, UpdateGameRequest,
};
use crate::utils::error::AppError;

use super::cache_helper;
use super::cache_helper::cached_or_fetch;

// Legacy game-alias management service. Keep the CRUD behavior and schema for
// compatibility, but do not add new callers or features.

#[allow(deprecated)]
#[deprecated(note = "Legacy game-alias service; retained for compatibility only")]
pub async fn list_games(
    pool: &MySqlPool,
    page: i64,
    page_size: i64,
    q: Option<&str>,
) -> Result<GameListResponse, AppError> {
    let page = if page < 1 { 1 } else { page };
    let page_size = if page_size < 1 {
        DEFAULT_PAGE_SIZE as i64
    } else if page_size > MAX_PAGE_SIZE as i64 {
        MAX_PAGE_SIZE as i64
    } else {
        page_size
    };

    let (where_clause, count_params, data_params): (String, Vec<String>, Vec<String>) =
        if let Some(keyword) = q {
            if keyword.is_empty() {
                (String::new(), vec![], vec![])
            } else {
                let like = format!("%{}%", keyword);
                (
                    "WHERE g.name LIKE ? OR EXISTS (SELECT 1 FROM game_alias_entries WHERE game_id = g.id AND alias LIKE ?)".to_string(),
                    vec![like.clone(), like.clone()],
                    vec![like.clone(), like.clone()],
                )
            }
        } else {
            (String::new(), vec![], vec![])
        };

    let count_sql = audit_sql(format!(
        "SELECT COUNT(DISTINCT g.id) FROM games g {}",
        where_clause
    ));
    let mut count_query = sqlx::query(count_sql);
    for p in &count_params {
        count_query = count_query.bind(p);
    }
    let total: i64 = count_query
        .fetch_one(pool)
        .await
        .map_err(|e| AppError::Internal(format!("game list count: {}", e)))?
        .get(0);

    let offset = (page - 1) * page_size;
    let data_sql = audit_sql(format!(
        "SELECT g.id, g.name, g.created_at, g.updated_at, COALESCE(JSON_ARRAYAGG(gae.alias), JSON_ARRAY()) AS aliases
         FROM games g
         LEFT JOIN game_alias_entries gae ON gae.game_id = g.id
         {}
         GROUP BY g.id, g.name, g.created_at, g.updated_at
         ORDER BY g.id DESC
         LIMIT ? OFFSET ?",
        where_clause
    ));
    let mut data_query = sqlx::query(data_sql);
    for p in &data_params {
        data_query = data_query.bind(p);
    }
    let rows = data_query
        .bind(page_size)
        .bind(offset)
        .fetch_all(pool)
        .await
        .map_err(|e| AppError::Internal(format!("game list query: {}", e)))?;

    let games: Vec<GameResponse> = rows
        .iter()
        .map(row_to_game)
        .map(GameResponse::from)
        .collect();

    Ok(GameListResponse {
        total: total as u64,
        games,
    })
}

#[allow(deprecated)]
#[deprecated(note = "Legacy game-alias service; retained for compatibility only")]
pub async fn get_game_by_id(pool: &MySqlPool, id: i64) -> Result<Option<Game>, AppError> {
    let row = sqlx::query(
        "SELECT g.id, g.name, g.created_at, g.updated_at,
                COALESCE(JSON_ARRAYAGG(gae.alias), JSON_ARRAY()) AS aliases
         FROM games g
         LEFT JOIN game_alias_entries gae ON gae.game_id = g.id
         WHERE g.id = ?
         GROUP BY g.id, g.name, g.created_at, g.updated_at",
    )
    .bind(id)
    .fetch_optional(pool)
    .await
    .map_err(|e| AppError::Internal(format!("game fetch: {}", e)))?;

    match row {
        Some(row) => Ok(Some(row_to_game(&row))),
        None => Ok(None),
    }
}

#[allow(deprecated)]
#[deprecated(note = "Legacy game-alias service; retained for compatibility only")]
pub async fn create_game(pool: &MySqlPool, req: CreateGameRequest) -> Result<Game, AppError> {
    validate_name(&req.name)?;
    validate_aliases(&req.aliases)?;

    let deduplicated_aliases: Vec<String> = req
        .aliases
        .into_iter()
        .collect::<HashSet<_>>()
        .into_iter()
        .collect();

    let mut tx = pool
        .begin()
        .await
        .map_err(|e| AppError::Internal(format!("game create tx begin: {}", e)))?;

    let result = sqlx::query("INSERT INTO games (name) VALUES (?)")
        .bind(&req.name)
        .execute(&mut *tx)
        .await;

    if let Err(e) = result {
        let msg = e.to_string();
        if msg.contains("Duplicate") && msg.contains("name") {
            return Err(AppError::GameNameAlreadyExists(
                "游戏名称已存在".to_string(),
            ));
        }
        return Err(AppError::Internal(format!("game insert: {}", e)));
    }

    let game_id = result.unwrap().last_insert_id() as i64;

    for alias in &deduplicated_aliases {
        let insert_result =
            sqlx::query("INSERT INTO game_alias_entries (game_id, alias) VALUES (?, ?)")
                .bind(game_id)
                .bind(alias)
                .execute(&mut *tx)
                .await;

        if let Err(e) = insert_result {
            let msg = e.to_string();
            if msg.contains("Duplicate") {
                tx.rollback().await.ok();
                return Err(AppError::AliasAlreadyInUse(
                    "别名已被其他游戏使用".to_string(),
                ));
            }
            tx.rollback().await.ok();
            return Err(AppError::Internal(format!("alias insert: {}", e)));
        }
    }

    tx.commit()
        .await
        .map_err(|e| AppError::Internal(format!("game commit: {}", e)))?;

    get_game_by_id(pool, game_id)
        .await?
        .ok_or_else(|| AppError::Internal("Game created but not found".to_string()))
}

#[allow(deprecated)]
#[deprecated(note = "Legacy game-alias service; retained for compatibility only")]
pub async fn update_game(
    pool: &MySqlPool,
    id: i64,
    req: UpdateGameRequest,
) -> Result<Game, AppError> {
    let existing = get_game_by_id(pool, id).await?;
    if existing.is_none() {
        return Err(AppError::GameAliasNotFound("游戏别名不存在".to_string()));
    }

    if req.name.is_none() && req.aliases.is_none() {
        return Err(AppError::BadRequest(
            "至少需要提供名称或别名之一".to_string(),
        ));
    }

    if let Some(ref name) = req.name {
        validate_name(name)?;
    }
    if let Some(ref aliases) = req.aliases {
        validate_aliases(aliases)?;
    }

    let deduplicated_aliases: Option<Vec<String>> = req.aliases.map(|als| {
        als.into_iter()
            .collect::<HashSet<_>>()
            .into_iter()
            .collect()
    });

    let mut tx = pool
        .begin()
        .await
        .map_err(|e| AppError::Internal(format!("game update tx begin: {}", e)))?;

    if let Some(ref name) = req.name {
        let update_result = sqlx::query("UPDATE games SET name = ? WHERE id = ?")
            .bind(name)
            .bind(id)
            .execute(&mut *tx)
            .await;

        if let Err(e) = update_result {
            let msg = e.to_string();
            if msg.contains("Duplicate") && msg.contains("name") {
                return Err(AppError::GameNameAlreadyExists(
                    "游戏名称已存在".to_string(),
                ));
            }
            return Err(AppError::Internal(format!("game update: {}", e)));
        }
    }

    if let Some(ref aliases) = deduplicated_aliases {
        sqlx::query("DELETE FROM game_alias_entries WHERE game_id = ?")
            .bind(id)
            .execute(&mut *tx)
            .await
            .map_err(|e| AppError::Internal(format!("alias delete: {}", e)))?;

        for alias in aliases {
            let insert_result =
                sqlx::query("INSERT INTO game_alias_entries (game_id, alias) VALUES (?, ?)")
                    .bind(id)
                    .bind(alias)
                    .execute(&mut *tx)
                    .await;

            if let Err(e) = insert_result {
                let msg = e.to_string();
                if msg.contains("Duplicate") {
                    return Err(AppError::AliasAlreadyInUse(
                        "别名已被其他游戏使用".to_string(),
                    ));
                }
                return Err(AppError::Internal(format!("alias insert: {}", e)));
            }
        }
    }

    tx.commit()
        .await
        .map_err(|e| AppError::Internal(format!("game update commit: {}", e)))?;

    get_game_by_id(pool, id)
        .await?
        .ok_or_else(|| AppError::Internal("Game updated but not found".to_string()))
}

#[allow(deprecated)]
#[deprecated(note = "Legacy game-alias service; retained for compatibility only")]
pub async fn delete_game(pool: &MySqlPool, id: i64) -> Result<bool, AppError> {
    let result = sqlx::query("DELETE FROM games WHERE id = ?")
        .bind(id)
        .execute(pool)
        .await
        .map_err(|e| AppError::Internal(format!("game delete: {}", e)))?;

    Ok(result.rows_affected() > 0)
}

fn validate_name(name: &str) -> Result<(), AppError> {
    let trimmed = name.trim();
    if trimmed.is_empty() {
        return Err(AppError::GameNameEmpty("游戏名称不能为空".to_string()));
    }
    if trimmed.len() > MAX_NAME_LENGTH {
        return Err(AppError::GameNameTooLong(format!(
            "游戏名称不能超过 {} 个字符",
            MAX_NAME_LENGTH
        )));
    }
    Ok(())
}

fn validate_aliases(aliases: &[String]) -> Result<(), AppError> {
    let non_empty: Vec<&String> = aliases.iter().filter(|a| !a.trim().is_empty()).collect();
    if non_empty.is_empty() {
        return Err(AppError::AliasesEmpty("别名数组不能为空".to_string()));
    }
    if aliases.len() > MAX_ALIASES_COUNT {
        return Err(AppError::AliasesTooMany(format!(
            "别名数量不能超过 {} 个",
            MAX_ALIASES_COUNT
        )));
    }
    for alias in aliases {
        if alias.trim().len() > MAX_ALIAS_LENGTH {
            return Err(AppError::AliasTooLong(format!(
                "别名不能超过 {} 个字符",
                MAX_ALIAS_LENGTH
            )));
        }
    }
    Ok(())
}

#[allow(deprecated)]
fn row_to_game(row: &sqlx::mysql::MySqlRow) -> Game {
    let id: i64 = row.get("id");
    let name: String = row.get("name");
    let created_at: chrono::NaiveDateTime = row.get("created_at");
    let updated_at: chrono::NaiveDateTime = row.get("updated_at");
    let aliases_json: serde_json::Value = row.get("aliases");

    let aliases: Vec<String> = aliases_json
        .as_array()
        .map(|arr| {
            arr.iter()
                .filter_map(|v| v.as_str().map(String::from))
                .filter(|s| !s.is_empty())
                .collect()
        })
        .unwrap_or_default();

    Game {
        id,
        name,
        aliases,
        created_at: created_at.and_utc(),
        updated_at: updated_at.and_utc(),
    }
}

// Active assistant-runtime external-game queries begin here. These helpers are
// independent of the deprecated local game-alias management tables.

/// Row returned by `get_external_game_by_id`.
#[derive(Debug, Clone, Serialize, Deserialize, sqlx::FromRow)]
pub struct ExternalGameInfo {
    pub logic_game_id: i64,
    pub name: String,
    pub description: Option<String>,
    pub cover_image: Option<String>,
    pub game_tags: Option<serde_json::Value>,
    pub computer_id: Option<i64>,
    pub platform_name: Option<String>,
    pub client_type: Option<String>,
    pub channel: Option<String>,
    pub game_icon: Option<String>,
    pub raw_description: Option<String>,
}

/// Query games from cc_logic_game by logic_game_id (foreign key, may return multiple rows).
pub async fn get_external_game_by_id(
    ext_pool: &MySqlPool,
    logic_game_id: i64,
    client_type: &str,
    channel: &str,
) -> Result<Vec<ExternalGameInfo>, AppError> {
    sqlx::query_as::<_, ExternalGameInfo>(
        r#"SELECT
  t1.logic_game_id, t1.name, t9.recommend_reason as description, t1.description as raw_description, t1.cover_image, t1.game_tags,
  t2.computer_id,
  t3.name as platform_name,
  t1.client_type,
  t5.prom_channel as channel,
  t8.game_icon
FROM (
  select * from cc_logic_game_wide where logic_game_id=? and lower(client_type)=lower(?)
) t1
INNER JOIN (
  select * from cc_game where logic_game_id=? and available = 1
) t2 ON t1.logic_game_id = t2.logic_game_id
INNER JOIN cc_game_platform t3 on t2.game_platform_id = t3.id
INNER JOIN (
	select * from cc_computer_info where status = 1
) ci ON t2.computer_id  = ci.id
INNER JOIN cc_logic_game_version t4 ON t1.version = t4.version
INNER JOIN (
  SELECT pc.id, pc.game_tag, pc.prom_channel FROM cc_promotion_channel pc where pc.prom_channel = ?
) t5 ON t1.channel_game_tag = t5.game_tag
LEFT JOIN (
  select * from cc_logic_game_exclude where client_type=? and channel=?
) t6 on t1.logic_game_id = t6.logic_game_id
LEFT JOIN cc_logic_game_blacklist t7 ON t1.logic_game_id = t7.logic_game_id
LEFT JOIN (
  select * from cc_logic_game where id=? and status = 1
) t8 on t1.logic_game_id = t8.id
LEFT JOIN (
  select * from cc_ranking_recommended_game
  where logic_game_id = ? order by update_time desc limit 1
) t9 on t1.logic_game_id = t9.logic_game_id
where t6.id is null
AND t7.id is null
"#,
    )
    .bind(logic_game_id)
    .bind(client_type)
    .bind(logic_game_id)
    .bind(channel)
    .bind(client_type)
    .bind(channel)
    .bind(logic_game_id)
    .bind(logic_game_id)
    .fetch_all(ext_pool)
    .await
    .map_err(|e| AppError::Internal(format!("game_info external query: {e}")))
}

/// Query cc_logic_game from external database, filtered by channel and client_type.
///
/// The channel is mapped to a `channel_game_tag` via `cc_promotion_channel`,
/// then matched against `cc_logic_game_wide.channel_game_tag`.
/// Games listed in `cc_logic_game_exclude` for the given channel and client_type are omitted.
///
/// Returns vec of (id, name, alias).
pub async fn list_external_games(
    ext_pool: &MySqlPool,
    channel: &str,
    client_type: &str,
) -> Result<Vec<(i64, String, String)>, AppError> {
    let sql = r#"SELECT
  z2.id, z2.name, COALESCE(z2.alias, '') AS alias
FROM (
  SELECT t1.logic_game_id, t1.name
  FROM (
    select * from cc_logic_game_wide where client_type=?
  ) t1
  LEFT JOIN (
    select * from cc_logic_game_exclude where client_type=? and channel=?
  ) t2 on t1.logic_game_id = t2.logic_game_id
  INNER JOIN cc_logic_game_version t3 ON t1.version = t3.version
  LEFT JOIN cc_logic_game_blacklist t4 ON t1.logic_game_id = t4.logic_game_id
  where t2.id is null AND t4.id is null
  group by t1.logic_game_id,t1.name
) z1
INNER JOIN (
  select * from cc_logic_game where status = 1
) z2 on z1.logic_game_id = z2.id
    "#;
    sqlx::query_as::<_, (i64, String, String)>(sql)
        .bind(client_type)
        .bind(client_type)
        .bind(channel)
        .fetch_all(ext_pool)
        .await
        .map_err(|e| AppError::Internal(format!("game_list external query: {e}")))
}

/// Query available channels for a game from cc_promotion_channel (external DB).
pub async fn get_game_channels(
    ext_pool: &MySqlPool,
    logic_game_id: i64,
) -> Result<Vec<String>, AppError> {
    let rows: Vec<(String,)> = sqlx::query_as(
        "SELECT distinct t2.prom_channel
FROM (
    select * from cc_logic_game_wide where logic_game_id=?
) t1
INNER JOIN cc_promotion_channel t2 ON t2.game_tag = t1.channel_game_tag AND t2.status = 1
LEFT JOIN (
    select * from cc_logic_game_exclude
) t3 on t1.logic_game_id = t3.logic_game_id and t2.prom_channel = t3.channel
where t3.id is null",
    )
    .bind(logic_game_id)
    .fetch_all(ext_pool)
    .await
    .map_err(|e| AppError::Internal(format!("game_channels query: {e}")))?;
    Ok(rows.into_iter().map(|r| r.0).collect())
}

/// Query client_type values from cc_logic_game_wide (external DB).
pub async fn get_game_client_types(
    ext_pool: &MySqlPool,
    logic_game_id: i64,
) -> Result<Vec<String>, AppError> {
    let rows: Vec<(String,)> = sqlx::query_as(
        r#"SELECT t1.client_type
FROM (
    SELECT in1.*,in2.prom_channel
    FROM (
        select * from cc_logic_game_wide where logic_game_id = ?
    ) in1
    INNER JOIN cc_promotion_channel in2 ON in2.game_tag = in1.channel_game_tag AND in2.status = 1
) t1
LEFT JOIN (
  select * from cc_logic_game_exclude where logic_game_id = ?
) t2 on t1.logic_game_id = t2.logic_game_id and t1.prom_channel = t2.channel
INNER JOIN cc_logic_game_version t3 ON t1.version = t3.version
LEFT JOIN cc_logic_game_blacklist t4 ON t1.logic_game_id = t4.logic_game_id
where t2.id is null AND t4.id is null
group by t1.client_type"#,
    )
    .bind(logic_game_id)
    .bind(logic_game_id)
    .fetch_all(ext_pool)
    .await
    .map_err(|e| AppError::Internal(format!("game_client_types query: {e}")))?;
    Ok(rows.into_iter().map(|r| r.0).collect())
}

/// Query trial purchase platform config from external cc_config table.
/// Returns the parsed JSON content for label = 'trialPurchasePlatformConfig'.
pub async fn get_trial_purchase_platform_config(
    ext_pool: &MySqlPool,
) -> Result<Option<serde_json::Value>, AppError> {
    let row: Option<(Option<serde_json::Value>,)> = sqlx::query_as(
        "SELECT content FROM cc_config WHERE label = 'trialPurchasePlatformConfig' LIMIT 1",
    )
    .fetch_optional(ext_pool)
    .await
    .map_err(|e| AppError::Internal(format!("cc_config query: {e}")))?;
    Ok(row.and_then(|r| r.0))
}

/// Sort a vec of ExternalGameInfo by trial purchase platform priority in-place.
#[expect(
    clippy::ptr_arg,
    reason = "retain the existing public service API parameter type"
)]
pub async fn sort_external_games_by_priority(
    ext_pool: &MySqlPool,
    games: &mut Vec<ExternalGameInfo>,
) {
    let platform_priority: Vec<String> = get_trial_purchase_platform_config(ext_pool)
        .await
        .unwrap_or(None)
        .and_then(|config| {
            config
                .get("platformPriority")
                .and_then(|v| v.as_array())
                .map(|arr| {
                    arr.iter()
                        .filter_map(|v| v.as_str().map(String::from))
                        .collect()
                })
        })
        .unwrap_or_default();

    if !platform_priority.is_empty() {
        games.sort_by(|a, b| {
            let pa = a
                .platform_name
                .as_deref()
                .and_then(|name| platform_priority.iter().position(|p| p == name))
                .unwrap_or(usize::MAX);
            let pb = b
                .platform_name
                .as_deref()
                .and_then(|name| platform_priority.iter().position(|p| p == name))
                .unwrap_or(usize::MAX);
            pa.cmp(&pb)
        });
    }
}

// ── Redis-cached wrappers ──

/// Cached single-game lookup: queries external DB by (game_id, client_type, channel),
/// sorts by platform priority, and returns only the top-priority result.
///
/// Cache TTL: 1 day when found, 5 minutes when not found.
pub async fn get_single_external_game_info_cached(
    redis: &RedisClient,
    ext_pool: &MySqlPool,
    game_id: i64,
    client_type: &str,
    channel: &str,
) -> Result<Option<ExternalGameInfo>, String> {
    let key = format!(
        "{}:single:{}:{}:{}",
        cache_helper::KEY_GAME_INFO,
        game_id,
        client_type,
        channel
    );

    // 1. Try Redis
    match cache_helper::cached_get::<Option<ExternalGameInfo>>(redis, &key).await {
        Ok(Some(cached)) => return Ok(cached),
        Ok(None) => {} // cache miss
        Err(_) => {
            tracing::debug!(
                error_kind = "cache_read_failed",
                "cache read failed, falling back to DB"
            )
        }
    }

    // 2. Fetch from DB, sort by platform priority, take first
    let mut games = get_external_game_by_id(ext_pool, game_id, client_type, channel)
        .await
        .map_err(|e| format!("get_external_game_by_id: {e}"))?;
    sort_external_games_by_priority(ext_pool, &mut games).await;
    let result: Option<ExternalGameInfo> = games.into_iter().next();

    // 3. Cache with split TTL: found → 1 day, not found → 5 min
    let ttl = if result.is_some() {
        cache_helper::TTL_GAME_INFO_FOUND
    } else {
        cache_helper::TTL_GAME_INFO_NOT_FOUND
    };
    if cache_helper::cached_set(redis, &key, &result, ttl)
        .await
        .is_err()
    {
        tracing::debug!(error_kind = "cache_write_failed", "cache write failed");
    }

    Ok(result)
}

/// Non-cached single-game lookup: queries external DB by (game_id, client_type, channel),
/// sorts by platform priority, and returns only the top-priority result.
pub async fn get_single_external_game_info(
    ext_pool: &MySqlPool,
    game_id: i64,
    client_type: &str,
    channel: &str,
) -> Result<Option<ExternalGameInfo>, AppError> {
    let mut games = get_external_game_by_id(ext_pool, game_id, client_type, channel).await?;
    sort_external_games_by_priority(ext_pool, &mut games).await;
    Ok(games.into_iter().next())
}

/// Cached version of `list_external_games`.
pub async fn list_external_games_cached(
    redis: &RedisClient,
    ext_pool: &MySqlPool,
    channel: &str,
    client_type: &str,
) -> Result<Vec<(i64, String, String)>, String> {
    let key = format!(
        "{}:{}:{}",
        cache_helper::KEY_GAME_LIST,
        channel,
        client_type
    );
    cached_or_fetch(redis, &key, cache_helper::TTL_GAME_LIST, || async {
        list_external_games(ext_pool, channel, client_type)
            .await
            .map_err(|e| format!("list_external_games: {e}"))
    })
    .await
}

/// Check whether games are still active (status=1) in cc_logic_game.
/// Returns the set of IDs that are still available.
pub async fn filter_available_games(
    ext_pool: &MySqlPool,
    game_ids: &[i64],
) -> Result<std::collections::HashSet<i64>, String> {
    if game_ids.is_empty() {
        return Ok(std::collections::HashSet::new());
    }
    let placeholders: Vec<String> = game_ids.iter().map(|_| "?".to_string()).collect();
    let sql = format!(
        "SELECT id FROM cc_logic_game WHERE id IN ({}) AND status = 1",
        placeholders.join(",")
    );
    let mut query = sqlx::query_as(audit_sql(sql));
    for id in game_ids {
        query = query.bind(id);
    }
    let rows: Vec<(i64,)> = query
        .fetch_all(ext_pool)
        .await
        .map_err(|e| format!("filter_available_games: {e}"))?;
    Ok(rows.into_iter().map(|(id,)| id).collect())
}

/// Query available logic_game_ids filtered by client_type, channel, and game tag type.
/// Returns distinct game IDs that pass version/exclude/blacklist checks.
pub async fn fetch_logic_game_ids_by_tag(
    ext_pool: &MySqlPool,
    category_name: &str,
    client_type: &str,
    channel: &str,
    limit: i32,
) -> Result<Vec<i64>, String> {
    let sql = r#"SELECT
  distinct z2.id
FROM (
  SELECT t1.logic_game_id
  FROM (
    select * from cc_logic_game_wide where client_type=?
    AND JSON_CONTAINS (game_tags, JSON_OBJECT ('type', 1))
    AND JSON_CONTAINS (game_tags, JSON_OBJECT ('name', ?))
  ) t1
  LEFT JOIN (
    select * from cc_logic_game_exclude where client_type=? and channel=?
  ) t2 on t1.logic_game_id = t2.logic_game_id
  INNER JOIN cc_logic_game_version t3 ON t1.version = t3.version
  LEFT JOIN cc_logic_game_blacklist t4 ON t1.logic_game_id = t4.logic_game_id
  where t2.id is null AND t4.id is null
  group by t1.logic_game_id
) z1
  INNER JOIN (
  select * from cc_logic_game where status = 1
    ) z2 on z1.logic_game_id = z2.id
order by RAND()
limit ?"#;
    let rows: Vec<(i64,)> = sqlx::query_as(audit_sql(sql.to_string()))
        .bind(client_type)
        .bind(category_name)
        .bind(client_type)
        .bind(channel)
        .bind(limit)
        .fetch_all(ext_pool)
        .await
        .map_err(|e| format!("fetch_logic_game_ids_by_tag: {e}"))?;
    tracing::debug!(
        client_type_bytes = client_type.len(),
        channel_bytes = channel.len(),
        category_name_bytes = category_name.len(),
        limit,
        result_count = rows.len(),
        "fetch_logic_game_ids_by_tag done"
    );
    Ok(rows.into_iter().map(|(id,)| id).collect())
}

/// Query cc_logic_game detail by id (for web-admin game lookup).
pub async fn get_external_game_detail(
    ext_pool: &MySqlPool,
    game_id: i64,
) -> Result<
    Option<(
        i64,
        String,
        Option<String>,
        Option<String>,
        Option<serde_json::Value>,
    )>,
    AppError,
> {
    sqlx::query_as::<
        _,
        (
            i64,
            String,
            Option<String>,
            Option<String>,
            Option<serde_json::Value>,
        ),
    >(
        "SELECT g.id, g.name, w.description, w.cover_image, w.game_tags
         FROM cc_logic_game g
         LEFT JOIN cc_logic_game_wide w ON w.logic_game_id = g.id
         WHERE g.id = ?",
    )
    .bind(game_id)
    .fetch_optional(ext_pool)
    .await
    .map_err(|e| AppError::Internal(format!("external game detail: {e}")))
}

#[cfg(test)]
mod tests {
    use super::get_external_game_by_id;
    use crate::utils::error::AppError;
    use sqlx::mysql::{MySqlConnectOptions, MySqlPoolOptions};

    #[tokio::test]
    async fn get_external_game_by_id_wraps_database_errors() {
        let pool = MySqlPoolOptions::new().connect_lazy_with(MySqlConnectOptions::new());
        pool.close().await;

        let error = get_external_game_by_id(&pool, 42, "pc", "official")
            .await
            .expect_err("a closed pool must fail the external game query");

        assert!(
            matches!(error, AppError::Internal(message) if message.starts_with("game_info external query:")),
            "get_external_game_by_id must preserve its database error context"
        );
    }

    #[tokio::test]
    #[ignore = "requires crates/hiveweb/.env EXTERNAL_DB_URL and eligible external game data"]
    async fn get_external_game_by_id_returns_normal_query_results() {
        dotenvy::from_path(concat!(env!("CARGO_MANIFEST_DIR"), "/.env")).ok();
        let database_url = std::env::var("EXTERNAL_DB_URL")
            .expect("EXTERNAL_DB_URL must be configured in crates/hiveweb/.env");
        let pool = MySqlPoolOptions::new()
            .max_connections(1)
            .connect(&database_url)
            .await
            .expect("external database connection must succeed");

        // 自动寻找一条符合过滤条件的可用游戏，无需写死游戏 ID。
        let (game_id, client_type, channel): (i64, String, String) = sqlx::query_as(
            r#"SELECT DISTINCT t1.logic_game_id, t1.client_type, t5.prom_channel
FROM cc_logic_game_wide t1
INNER JOIN cc_game t2
    ON t1.logic_game_id = t2.logic_game_id AND t2.available = 1
INNER JOIN cc_game_platform t3 ON t2.game_platform_id = t3.id
INNER JOIN cc_computer_info ci ON t2.computer_id = ci.id AND ci.status = 1
INNER JOIN cc_logic_game_version t4 ON t1.version = t4.version
INNER JOIN cc_promotion_channel t5 ON t1.channel_game_tag = t5.game_tag
LEFT JOIN cc_logic_game_exclude t6
    ON t1.logic_game_id = t6.logic_game_id
    AND t1.client_type = t6.client_type
    AND t5.prom_channel = t6.channel
LEFT JOIN cc_logic_game_blacklist t7 ON t1.logic_game_id = t7.logic_game_id
WHERE t6.id IS NULL AND t7.id IS NULL
LIMIT 1"#,
        )
        .fetch_one(&pool)
        .await
        .expect("external database must contain at least one eligible game fixture");

        let games = get_external_game_by_id(&pool, game_id, &client_type, &channel)
            .await
            .expect("get_external_game_by_id must query the configured external database");

        assert!(!games.is_empty(), "the discovered game must be returned");
        assert!(games.iter().all(|game| game.logic_game_id == game_id));
        assert!(games.iter().all(|game| {
            game.client_type
                .as_deref()
                .is_some_and(|value| value.eq_ignore_ascii_case(&client_type))
        }));
        assert!(
            games
                .iter()
                .all(|game| game.channel.as_deref() == Some(&channel))
        );
        assert!(games.iter().all(|game| game.computer_id.is_some()));
        assert!(games.iter().all(|game| game.platform_name.is_some()));
    }

    #[test]
    fn external_game_query_only_joins_available_game_instances() {
        let source = include_str!("game_service.rs");
        let function_source = source
            .split_once("pub async fn get_external_game_by_id(")
            .expect("get_external_game_by_id must exist")
            .1
            .split_once("/// Query cc_logic_game from external database")
            .expect("the next external-game query must exist")
            .0;
        let normalized_source = function_source
            .split_whitespace()
            .collect::<Vec<_>>()
            .join(" ")
            .to_ascii_lowercase();

        assert!(
            normalized_source
                .contains("select * from cc_game where logic_game_id=? and available = 1"),
            "get_external_game_by_id must exclude unavailable cc_game rows"
        );
    }
}
