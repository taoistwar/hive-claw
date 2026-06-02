//! Category service (T133 / US7)
//!
//! Category 是嵌套树（parent_id self-FK）。删除策略：FK ON DELETE SET NULL，
//! 因此删除父节点会让子节点变成顶级节点；没有 4093 引用阻塞。

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use sqlx::MySqlPool;

use crate::models::Category;
use crate::utils::error::AppError;

#[derive(Debug, Deserialize)]
pub struct CreateMeta {
    pub parent_id: Option<i64>,
    pub name: String,
    pub slug: String,
    pub description: Option<String>,
}

#[derive(Debug, Deserialize)]
pub struct UpdateMeta {
    pub name: Option<String>,
    pub slug: Option<String>,
    pub description: Option<String>,
    pub parent_id: Option<i64>,
    pub updated_at: DateTime<Utc>,
}

#[derive(Debug, Serialize)]
pub struct CategoryNode {
    #[serde(flatten)]
    pub cat: Category,
    pub children: Vec<CategoryNode>,
}

pub async fn create(pool: &MySqlPool, meta: CreateMeta) -> Result<Category, AppError> {
    let res = sqlx::query(
        "INSERT INTO categories (parent_id, name, slug, description) VALUES (?, ?, ?, ?)",
    )
    .bind(meta.parent_id)
    .bind(&meta.name)
    .bind(&meta.slug)
    .bind(&meta.description)
    .execute(pool)
    .await
    .map_err(|e| {
        let msg = e.to_string();
        if msg.contains("Duplicate") {
            AppError::Conflict(format!("category slug 已存在：{}", meta.slug))
        } else {
            AppError::Internal(format!("category insert: {e}"))
        }
    })?;
    fetch_by_id(pool, res.last_insert_id() as i64).await
}

pub async fn fetch_by_id(pool: &MySqlPool, id: i64) -> Result<Category, AppError> {
    sqlx::query_as::<_, Category>("SELECT * FROM categories WHERE id = ?")
        .bind(id)
        .fetch_optional(pool)
        .await
        .map_err(|e| AppError::Internal(format!("category fetch: {e}")))?
        .ok_or_else(|| AppError::NotFound(format!("category id={id} not found")))
}

pub async fn list_flat(pool: &MySqlPool) -> Result<Vec<Category>, AppError> {
    sqlx::query_as::<_, Category>("SELECT * FROM categories ORDER BY parent_id, name")
        .fetch_all(pool)
        .await
        .map_err(|e| AppError::Internal(format!("category list: {e}")))
}

pub async fn list_tree(pool: &MySqlPool) -> Result<Vec<CategoryNode>, AppError> {
    let rows = list_flat(pool).await?;
    let mut children_map: std::collections::HashMap<Option<i64>, Vec<Category>> =
        std::collections::HashMap::new();
    for r in rows {
        children_map.entry(r.parent_id).or_default().push(r);
    }

    fn build(
        parent: Option<i64>,
        map: &mut std::collections::HashMap<Option<i64>, Vec<Category>>,
    ) -> Vec<CategoryNode> {
        let mut nodes = Vec::new();
        if let Some(rows) = map.remove(&parent) {
            for r in rows {
                let id = r.id;
                let children = build(Some(id), map);
                nodes.push(CategoryNode { cat: r, children });
            }
        }
        nodes
    }

    Ok(build(None, &mut children_map))
}

pub async fn update(pool: &MySqlPool, id: i64, meta: UpdateMeta) -> Result<Category, AppError> {
    crate::services::optimistic_lock::check_and_bump(pool, "categories", id, meta.updated_at)
        .await?;
    sqlx::query(
        r#"UPDATE categories SET
              name = COALESCE(?, name),
              slug = COALESCE(?, slug),
              description = COALESCE(?, description),
              parent_id = ?
           WHERE id = ?"#,
    )
    .bind(&meta.name)
    .bind(&meta.slug)
    .bind(&meta.description)
    .bind(meta.parent_id)
    .bind(id)
    .execute(pool)
    .await
    .map_err(|e| AppError::Internal(format!("category update: {e}")))?;
    fetch_by_id(pool, id).await
}

pub async fn delete(pool: &MySqlPool, id: i64) -> Result<(), AppError> {
    sqlx::query("DELETE FROM categories WHERE id = ?")
        .bind(id)
        .execute(pool)
        .await
        .map_err(|e| AppError::Internal(format!("category delete: {e}")))?;
    Ok(())
}
