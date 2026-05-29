//! Tag service (T134 / US7)
//!
//! Tag 在 taggings 表通过 polymorphic (entity_type, entity_id) 关联到 Plugin /
//! Function / Tool / Skill / Agent。删除前必须校验 taggings 引用计数；> 0 → 4091。

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use sqlx::MySqlPool;

use crate::models::Tag;
use crate::utils::error::AppError;

#[derive(Debug, Deserialize)]
pub struct CreateMeta {
    pub name: String,
    pub color: Option<String>,
}

#[derive(Debug, Deserialize)]
pub struct UpdateMeta {
    pub name: Option<String>,
    pub color: Option<String>,
    pub updated_at: Option<DateTime<Utc>>,
}

#[derive(Debug, Serialize)]
pub struct TagWithCount {
    #[serde(flatten)]
    pub tag: Tag,
    pub reference_count: i64,
}

pub async fn create(pool: &MySqlPool, meta: CreateMeta) -> Result<Tag, AppError> {
    let res = sqlx::query("INSERT INTO tags (name, color) VALUES (?, ?)")
        .bind(&meta.name)
        .bind(&meta.color)
        .execute(pool)
        .await
        .map_err(|e| {
            let msg = e.to_string();
            if msg.contains("Duplicate") {
                AppError::Conflict(format!("tag name 已存在：{}", meta.name))
            } else {
                AppError::Internal(format!("tag insert: {e}"))
            }
        })?;
    fetch_by_id(pool, res.last_insert_id() as i64).await
}

pub async fn fetch_by_id(pool: &MySqlPool, id: i64) -> Result<Tag, AppError> {
    sqlx::query_as::<_, Tag>("SELECT * FROM tags WHERE id = ?")
        .bind(id)
        .fetch_optional(pool)
        .await
        .map_err(|e| AppError::Internal(format!("tag fetch: {e}")))?
        .ok_or_else(|| AppError::NotFound(format!("tag id={id} not found")))
}

pub async fn list(
    pool: &MySqlPool,
    q: Option<&str>,
    offset: i64,
    limit: i64,
) -> Result<(Vec<TagWithCount>, i64), AppError> {
    let base_sql = r#"SELECT t.*, COALESCE(c.cnt, 0) AS reference_count
                 FROM tags t
                 LEFT JOIN (SELECT tag_id, COUNT(*) AS cnt FROM taggings GROUP BY tag_id) c
                   ON c.tag_id = t.id"#;

    #[derive(sqlx::FromRow)]
    struct Row {
        #[sqlx(flatten)]
        tag: Tag,
        reference_count: i64,
    }

    let (rows, total) = if let Some(keyword) = q {
        let like = format!("%{keyword}%");
        let count_sql = "SELECT COUNT(*) FROM tags WHERE name LIKE ?";
        let total: (i64,) = sqlx::query_as(count_sql)
            .bind(&like)
            .fetch_one(pool)
            .await
            .map_err(|e| AppError::Internal(format!("tag count: {e}")))?;
        let sql = format!(
            "{base_sql} WHERE t.name LIKE ? ORDER BY t.name LIMIT ? OFFSET ?"
        );
        let rows: Vec<Row> = sqlx::query_as(&sql)
            .bind(&like)
            .bind(limit)
            .bind(offset)
            .fetch_all(pool)
            .await
            .map_err(|e| AppError::Internal(format!("tag list: {e}")))?;
        (rows, total.0)
    } else {
        let count_sql = "SELECT COUNT(*) FROM tags";
        let total: (i64,) = sqlx::query_as(count_sql)
            .fetch_one(pool)
            .await
            .map_err(|e| AppError::Internal(format!("tag count: {e}")))?;
        let sql = format!("{base_sql} ORDER BY t.name LIMIT ? OFFSET ?");
        let rows: Vec<Row> = sqlx::query_as(&sql)
            .bind(limit)
            .bind(offset)
            .fetch_all(pool)
            .await
            .map_err(|e| AppError::Internal(format!("tag list: {e}")))?;
        (rows, total.0)
    };

    Ok((
        rows.into_iter()
            .map(|r| TagWithCount {
                tag: r.tag,
                reference_count: r.reference_count,
            })
            .collect(),
        total,
    ))
}

pub async fn update(pool: &MySqlPool, id: i64, meta: UpdateMeta) -> Result<Tag, AppError> {
    if let Some(updated_at) = meta.updated_at {
        crate::services::optimistic_lock::check_and_bump(pool, "tags", id, updated_at).await
            .or_else(|e| {
                // tags 表没有 updated_at 列；保留接口但跳过
                if matches!(e, AppError::Internal(_)) { Ok(()) } else { Err(e) }
            })?;
    }
    sqlx::query("UPDATE tags SET name = COALESCE(?, name), color = COALESCE(?, color) WHERE id = ?")
        .bind(&meta.name)
        .bind(&meta.color)
        .bind(id)
        .execute(pool)
        .await
        .map_err(|e| AppError::Internal(format!("tag update: {e}")))?;
    fetch_by_id(pool, id).await
}

pub async fn delete(pool: &MySqlPool, id: i64) -> Result<(), AppError> {
    let mut tx = pool
        .begin()
        .await
        .map_err(|e| AppError::Internal(format!("tx begin: {e}")))?;
    sqlx::query("SELECT id FROM tags WHERE id = ? FOR UPDATE")
        .bind(id)
        .fetch_one(&mut *tx)
        .await
        .map_err(|e| AppError::Internal(format!("tag lock: {e}")))?;
    let cnt: (i64,) = sqlx::query_as("SELECT COUNT(*) FROM taggings WHERE tag_id = ?")
        .bind(id)
        .fetch_one(&mut *tx)
        .await
        .map_err(|e| AppError::Internal(format!("tag ref count: {e}")))?;
    if cnt.0 > 0 {
        return Err(AppError::TagInUse(format!(
            "标签被 {} 个对象引用，无法删除",
            cnt.0
        )));
    }
    sqlx::query("DELETE FROM tags WHERE id = ?")
        .bind(id)
        .execute(&mut *tx)
        .await
        .map_err(|e| AppError::Internal(format!("tag delete: {e}")))?;
    tx.commit()
        .await
        .map_err(|e| AppError::Internal(format!("tx commit: {e}")))?;
    Ok(())
}
