//! GlobalConfig service — 全局配置 CRUD

use serde::Deserialize;
use sqlx::MySqlPool;

use crate::models::GlobalConfig;
use crate::utils::error::AppError;

#[derive(Debug, Deserialize)]
pub struct CreateMeta {
    pub name: String,
    pub key: String,
    pub config_type: String,
    pub data: serde_json::Value,
}

#[derive(Debug, Deserialize)]
pub struct UpdateMeta {
    pub name: Option<String>,
    pub config_type: Option<String>,
    pub data: Option<serde_json::Value>,
}

pub async fn create(pool: &MySqlPool, meta: CreateMeta) -> Result<GlobalConfig, AppError> {
    let res =
        sqlx::query("INSERT INTO global_configs (name, `key`, type, data) VALUES (?, ?, ?, ?)")
            .bind(&meta.name)
            .bind(&meta.key)
            .bind(&meta.config_type)
            .bind(&meta.data)
            .execute(pool)
            .await
            .map_err(|e| {
                let msg = e.to_string();
                if msg.contains("Duplicate") {
                    AppError::Conflict(format!("配置 key 已存在：{}", meta.key))
                } else {
                    AppError::Internal(format!("global_config insert: {e}"))
                }
            })?;
    fetch_by_id(pool, res.last_insert_id() as i64).await
}

pub async fn fetch_by_id(pool: &MySqlPool, id: i64) -> Result<GlobalConfig, AppError> {
    sqlx::query_as::<_, GlobalConfig>(
        "SELECT id, name, `key`, type as config_type, data, created_at, updated_at FROM global_configs WHERE id = ?",
    )
    .bind(id)
    .fetch_optional(pool)
    .await
    .map_err(|e| AppError::Internal(format!("global_config fetch: {e}")))?
    .ok_or_else(|| AppError::NotFound(format!("global_config id={id} not found")))
}

pub async fn fetch_by_key(pool: &MySqlPool, key: &str) -> Result<GlobalConfig, AppError> {
    sqlx::query_as::<_, GlobalConfig>(
        "SELECT id, name, `key`, type as config_type, data, created_at, updated_at FROM global_configs WHERE `key` = ?",
    )
    .bind(key)
    .fetch_optional(pool)
    .await
    .map_err(|e| AppError::Internal(format!("global_config fetch by key: {e}")))?
    .ok_or_else(|| AppError::NotFound(format!("global_config key={key} not found")))
}

pub async fn list(
    pool: &MySqlPool,
    q: Option<&str>,
    offset: i64,
    limit: i64,
) -> Result<(Vec<GlobalConfig>, i64), AppError> {
    if let Some(keyword) = q {
        let like = format!("%{keyword}%");
        let total: (i64,) =
            sqlx::query_as("SELECT COUNT(*) FROM global_configs WHERE name LIKE ? OR `key` LIKE ?")
                .bind(&like)
                .bind(&like)
                .fetch_one(pool)
                .await
                .map_err(|e| AppError::Internal(format!("global_config count: {e}")))?;

        let rows: Vec<GlobalConfig> = sqlx::query_as(
            "SELECT id, name, `key`, type as config_type, data, created_at, updated_at \
             FROM global_configs WHERE name LIKE ? OR `key` LIKE ? \
             ORDER BY id LIMIT ? OFFSET ?",
        )
        .bind(&like)
        .bind(&like)
        .bind(limit)
        .bind(offset)
        .fetch_all(pool)
        .await
        .map_err(|e| AppError::Internal(format!("global_config list: {e}")))?;

        Ok((rows, total.0))
    } else {
        let total: (i64,) = sqlx::query_as("SELECT COUNT(*) FROM global_configs")
            .fetch_one(pool)
            .await
            .map_err(|e| AppError::Internal(format!("global_config count: {e}")))?;

        let rows: Vec<GlobalConfig> = sqlx::query_as(
            "SELECT id, name, `key`, type as config_type, data, created_at, updated_at \
             FROM global_configs ORDER BY id LIMIT ? OFFSET ?",
        )
        .bind(limit)
        .bind(offset)
        .fetch_all(pool)
        .await
        .map_err(|e| AppError::Internal(format!("global_config list: {e}")))?;

        Ok((rows, total.0))
    }
}

pub async fn update(pool: &MySqlPool, id: i64, meta: UpdateMeta) -> Result<GlobalConfig, AppError> {
    sqlx::query(
        "UPDATE global_configs SET \
         name = COALESCE(?, name), \
         type = COALESCE(?, type), \
         data = COALESCE(?, data) \
         WHERE id = ?",
    )
    .bind(&meta.name)
    .bind(&meta.config_type)
    .bind(&meta.data)
    .bind(id)
    .execute(pool)
    .await
    .map_err(|e| AppError::Internal(format!("global_config update: {e}")))?;

    fetch_by_id(pool, id).await
}

pub async fn delete(pool: &MySqlPool, id: i64) -> Result<(), AppError> {
    let rows = sqlx::query("DELETE FROM global_configs WHERE id = ?")
        .bind(id)
        .execute(pool)
        .await
        .map_err(|e| AppError::Internal(format!("global_config delete: {e}")))?;

    if rows.rows_affected() == 0 {
        return Err(AppError::NotFound(format!(
            "global_config id={id} not found"
        )));
    }
    Ok(())
}
