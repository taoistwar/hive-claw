use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use sqlx::MySqlPool;

use crate::db::sql_safety::audit_sql;
use crate::models::Capability as CapabilityModel;
use crate::utils::error::AppError;

#[derive(Debug, Deserialize)]
pub struct CreateMeta {
    pub name: String,
    pub description: String,
    pub is_dangerous: bool,
    pub category_id: Option<i64>,
}

#[derive(Debug, Deserialize)]
pub struct UpdateMeta {
    pub description: Option<String>,
    pub is_dangerous: Option<bool>,
    pub category_id: Option<i64>,
}

#[derive(Debug, Default)]
pub struct ListFilter {
    pub name: Option<String>,
    pub description: Option<String>,
    pub is_dangerous: Option<bool>,
    pub category_id: Option<i64>,
}

#[derive(Debug, Serialize)]
pub struct CapabilityItem {
    pub name: String,
    pub description: String,
    pub is_dangerous: bool,
    pub category_id: Option<i64>,
    pub created_at: DateTime<Utc>,
}

impl From<&CapabilityModel> for CapabilityItem {
    fn from(c: &CapabilityModel) -> Self {
        Self {
            name: c.name.clone(),
            description: c.description.clone(),
            is_dangerous: c.is_dangerous != 0,
            category_id: c.category_id,
            created_at: c.created_at,
        }
    }
}

pub async fn list(
    pool: &MySqlPool,
    offset: i64,
    limit: i64,
    filter: ListFilter,
) -> Result<(Vec<CapabilityItem>, i64), AppError> {
    let mut where_clauses: Vec<String> = Vec::new();
    let mut params: Vec<String> = Vec::new();

    if let Some(ref name) = filter.name {
        where_clauses.push("name LIKE ?".to_string());
        params.push(format!("%{}%", name));
    }
    if let Some(ref desc) = filter.description {
        where_clauses.push("description LIKE ?".to_string());
        params.push(format!("%{}%", desc));
    }
    if let Some(dangerous) = filter.is_dangerous {
        where_clauses.push("is_dangerous = ?".to_string());
        params.push(if dangerous {
            "1".to_string()
        } else {
            "0".to_string()
        });
    }
    if let Some(cid) = filter.category_id {
        where_clauses.push("category_id = ?".to_string());
        params.push(cid.to_string());
    }

    let where_sql = if where_clauses.is_empty() {
        String::new()
    } else {
        format!("WHERE {}", where_clauses.join(" AND "))
    };

    let count_sql = format!("SELECT COUNT(*) FROM capabilities {where_sql}");
    let count_sql = audit_sql(count_sql);
    let mut count_q = sqlx::query_as::<_, (i64,)>(count_sql);
    for p in &params {
        count_q = count_q.bind(p);
    }
    let total: (i64,) = count_q
        .fetch_one(pool)
        .await
        .map_err(|e| AppError::Internal(format!("capability count: {e}")))?;

    let list_sql =
        format!("SELECT * FROM capabilities {where_sql} ORDER BY name ASC LIMIT ? OFFSET ?");
    let list_sql = audit_sql(list_sql);
    let mut q = sqlx::query_as::<_, CapabilityModel>(list_sql);
    for p in &params {
        q = q.bind(p);
    }
    q = q.bind(limit).bind(offset);

    let caps: Vec<CapabilityModel> = q
        .fetch_all(pool)
        .await
        .map_err(|e| AppError::Internal(format!("capability list: {e}")))?;

    let items: Vec<CapabilityItem> = caps.iter().map(CapabilityItem::from).collect();
    Ok((items, total.0))
}

pub async fn fetch_by_name(pool: &MySqlPool, name: &str) -> Result<CapabilityModel, AppError> {
    sqlx::query_as::<_, CapabilityModel>("SELECT * FROM capabilities WHERE name = ?")
        .bind(name)
        .fetch_optional(pool)
        .await
        .map_err(|e| AppError::Internal(format!("capability fetch: {e}")))?
        .ok_or_else(|| AppError::NotFound(format!("capability name={name} not found")))
}

pub async fn create(pool: &MySqlPool, meta: CreateMeta) -> Result<CapabilityModel, AppError> {
    sqlx::query(
        "INSERT INTO capabilities (name, description, is_dangerous, category_id) VALUES (?, ?, ?, ?)",
    )
    .bind(&meta.name)
    .bind(&meta.description)
    .bind(if meta.is_dangerous { 1i8 } else { 0i8 })
    .bind(meta.category_id)
    .execute(pool)
    .await
    .map_err(|e| {
        let msg = e.to_string();
        if msg.contains("Duplicate") {
            AppError::Conflict(format!("capability name 已存在：{}", meta.name))
        } else {
            AppError::Internal(format!("capability insert: {e}"))
        }
    })?;
    fetch_by_name(pool, &meta.name).await
}

pub async fn update(
    pool: &MySqlPool,
    name: &str,
    meta: UpdateMeta,
) -> Result<CapabilityModel, AppError> {
    sqlx::query(
        r#"UPDATE capabilities SET
              description = COALESCE(?, description),
              is_dangerous = COALESCE(?, is_dangerous),
              category_id = COALESCE(?, category_id)
           WHERE name = ?"#,
    )
    .bind(&meta.description)
    .bind(meta.is_dangerous.map(|b| if b { 1i8 } else { 0i8 }))
    .bind(meta.category_id)
    .bind(name)
    .execute(pool)
    .await
    .map_err(|e| AppError::Internal(format!("capability update: {e}")))?;
    fetch_by_name(pool, name).await
}

pub async fn delete(pool: &MySqlPool, name: &str) -> Result<(), AppError> {
    sqlx::query("DELETE FROM capabilities WHERE name = ?")
        .bind(name)
        .execute(pool)
        .await
        .map_err(|e| AppError::Internal(format!("capability delete: {e}")))?;
    Ok(())
}

/// 启动期由 runtime registry 同步 upsert 到 DB。
/// 代码侧的 CAPABILITIES 是真值源；DB 中多余项仅 warn 不删。
pub async fn sync_from_registry(
    pool: &MySqlPool,
    caps: &[(String, String, bool, Option<i64>)],
) -> Result<(), AppError> {
    for (name, desc, is_dangerous, category_id) in caps {
        sqlx::query(
            "INSERT INTO capabilities (name, description, is_dangerous, category_id) VALUES (?, ?, ?, ?) \
             ON DUPLICATE KEY UPDATE description = VALUES(description), is_dangerous = VALUES(is_dangerous), category_id = VALUES(category_id)",
        )
        .bind(name)
        .bind(desc)
        .bind(if *is_dangerous { 1i8 } else { 0i8 })
        .bind(category_id)
        .execute(pool)
        .await
        .map_err(|e| AppError::Internal(format!("capability upsert {name}: {e}")))?;
    }
    Ok(())
}
