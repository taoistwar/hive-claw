use sqlx::{MySqlPool, Row};
use std::collections::HashSet;

use crate::models::game::{
    CreateGameRequest, DEFAULT_PAGE_SIZE, Game, GameListResponse, GameResponse, MAX_ALIAS_LENGTH,
    MAX_ALIASES_COUNT, MAX_NAME_LENGTH, MAX_PAGE_SIZE, UpdateGameRequest,
};
use crate::utils::error::AppError;

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

    let count_sql = format!("SELECT COUNT(DISTINCT g.id) FROM games g {}", where_clause);
    let mut count_query = sqlx::query(&count_sql);
    for p in &count_params {
        count_query = count_query.bind(p);
    }
    let total: i64 = count_query
        .fetch_one(pool)
        .await
        .map_err(|e| AppError::Internal(format!("game list count: {}", e)))?
        .get(0);

    let offset = (page - 1) * page_size;
    let data_sql = format!(
        "SELECT g.id, g.name, g.created_at, g.updated_at, COALESCE(JSON_ARRAYAGG(gae.alias), JSON_ARRAY()) AS aliases
         FROM games g
         LEFT JOIN game_alias_entries gae ON gae.game_id = g.id
         {}
         GROUP BY g.id, g.name, g.created_at, g.updated_at
         ORDER BY g.id DESC
         LIMIT ? OFFSET ?",
        where_clause
    );
    let mut data_query = sqlx::query(&data_sql);
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

/// Load supplementary aliases from internal games table.
/// Returns a map from game name → list of aliases.
pub async fn load_internal_aliases(
    pool: &MySqlPool,
) -> Result<std::collections::HashMap<String, Vec<String>>, AppError> {
    let rows = sqlx::query_as::<_, (String, Option<String>)>(
        "SELECT g.name, gae.alias FROM games g
         LEFT JOIN game_alias_entries gae ON gae.game_id = g.id",
    )
    .fetch_all(pool)
    .await
    .map_err(|e| AppError::Internal(format!("game_list internal query: {e}")))?;

    let mut map: std::collections::HashMap<String, Vec<String>> =
        std::collections::HashMap::new();
    for (name, alias) in rows {
        let entry = map.entry(name).or_default();
        if let Some(a) = alias {
            let a = a.trim().to_string();
            if !a.is_empty() {
                entry.push(a);
            }
        }
    }
    Ok(map)
}

/// Query a single game from cc_logic_game by ID.
/// Returns (id, name, alias).
pub async fn get_external_game_by_id(
    ext_pool: &MySqlPool,
    game_id: i64,
) -> Result<Option<(i64, String, String)>, AppError> {
    sqlx::query_as::<_, (i64, String, String)>(
        "SELECT id, name, COALESCE(alias, '') AS alias FROM cc_logic_game WHERE id = ?",
    )
    .bind(game_id)
    .fetch_optional(ext_pool)
    .await
    .map_err(|e| AppError::Internal(format!("game_info external query: {e}")))
}

/// Query cc_logic_game from external database.
/// Returns vec of (id, name, alias).
pub async fn list_external_games(
    ext_pool: &MySqlPool,
) -> Result<Vec<(i64, String, String)>, AppError> {
    sqlx::query_as::<_, (i64, String, String)>(
        "SELECT id, name, COALESCE(alias, '') AS alias FROM cc_logic_game ORDER BY id",
    )
    .fetch_all(ext_pool)
    .await
    .map_err(|e| AppError::Internal(format!("game_list external query: {e}")))
}
