//! Function service (T081 / US2)
//!
//! Builtin (`kind=1`) — 由代码在启动期 idempotent upsert 写入；API 拒绝创建 / 删除。
//! Custom (`kind=2`) — 必须绑定已存在且未软删的 Plugin + plugin_export。

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use sqlx::MySqlPool;

use crate::models::Function;
use crate::utils::error::AppError;

#[derive(Debug, Deserialize)]
pub struct CreateMeta {
    pub identifier: String,
    pub name: String,
    pub description: Option<String>,
    pub plugin_id: i64,
    pub plugin_export: String,
    pub input_schema: Value,
    pub output_schema: Value,
    pub category_id: Option<i64>,
    #[serde(default)]
    pub tag_ids: Vec<i64>,
}

#[derive(Debug, Deserialize)]
pub struct UpdateMeta {
    pub name: Option<String>,
    pub description: Option<String>,
    pub category_id: Option<i64>,
    pub input_schema: Option<Value>,
    pub output_schema: Option<Value>,
    pub tag_ids: Option<Vec<i64>>,
    /// 乐观锁
    pub updated_at: DateTime<Utc>,
}

#[derive(Debug, Serialize)]
pub struct FunctionListItem {
    #[serde(flatten)]
    pub function: Function,
    pub plugin_identifier: Option<String>,
}

#[derive(Debug, Serialize)]
pub struct FunctionList {
    pub items: Vec<FunctionListItem>,
    pub total: i64,
    pub offset: i64,
    pub limit: i64,
}

#[derive(Debug, Default)]
pub struct ListFilter {
    pub offset: i64,
    pub limit: i64,
    pub search: Option<String>,
    pub category_id: Option<i64>,
    pub kind: Option<i8>,
}

fn validate_schema(schema: &Value, label: &str) -> Result<(), AppError> {
    // 轻量 JSON Schema 校验：仅尝试编译，编译通过即认为格式合法
    jsonschema::JSONSchema::compile(schema).map_err(|e| {
        AppError::BadRequest(format!("{label} 不是合法 JSON Schema: {e}"))
    })?;
    Ok(())
}

pub async fn create_custom(pool: &MySqlPool, meta: CreateMeta) -> Result<Function, AppError> {
    validate_schema(&meta.input_schema, "input_schema")?;
    validate_schema(&meta.output_schema, "output_schema")?;

    // Plugin 必须存在且未软删
    let plugin_row: Option<(i64, Option<DateTime<Utc>>)> = sqlx::query_as(
        "SELECT id, deleted_at FROM plugins WHERE id = ?",
    )
    .bind(meta.plugin_id)
    .fetch_optional(pool)
    .await
    .map_err(|e| AppError::Internal(format!("plugin lookup: {e}")))?;
    let Some((_, deleted_at)) = plugin_row else {
        return Err(AppError::NotFound(format!("plugin id={} not found", meta.plugin_id)));
    };
    if deleted_at.is_some() {
        return Err(AppError::Conflict(format!(
            "plugin id={} 已软删除，无法绑定 Function",
            meta.plugin_id
        )));
    }

    let res = sqlx::query(
        r#"INSERT INTO functions
           (identifier, name, description, kind, input_schema, output_schema, plugin_id, plugin_export, category_id)
           VALUES (?, ?, ?, 2, ?, ?, ?, ?, ?)"#,
    )
    .bind(&meta.identifier)
    .bind(&meta.name)
    .bind(&meta.description)
    .bind(&meta.input_schema)
    .bind(&meta.output_schema)
    .bind(meta.plugin_id)
    .bind(&meta.plugin_export)
    .bind(meta.category_id)
    .execute(pool)
    .await
    .map_err(|e| {
        let msg = e.to_string();
        if msg.contains("Duplicate") {
            AppError::Conflict(format!("function identifier 已存在：{}", meta.identifier))
        } else {
            AppError::Internal(format!("function insert: {e}"))
        }
    })?;

    let id = res.last_insert_id() as i64;

    for tid in &meta.tag_ids {
        sqlx::query(
            "INSERT IGNORE INTO taggings (tag_id, entity_type, entity_id) VALUES (?, 'function', ?)",
        )
        .bind(tid)
        .bind(id)
        .execute(pool)
        .await
        .map_err(|e| AppError::Internal(format!("tag bind: {e}")))?;
    }

    fetch_by_id(pool, id).await
}

pub async fn fetch_by_id(pool: &MySqlPool, id: i64) -> Result<Function, AppError> {
    sqlx::query_as::<_, Function>("SELECT * FROM functions WHERE id = ?")
        .bind(id)
        .fetch_optional(pool)
        .await
        .map_err(|e| AppError::Internal(format!("function fetch: {e}")))?
        .ok_or_else(|| AppError::NotFound(format!("function id={id} not found")))
}

pub async fn list(pool: &MySqlPool, filter: ListFilter) -> Result<FunctionList, AppError> {
    let mut where_clauses: Vec<String> = Vec::new();
    if filter.kind.is_some() {
        where_clauses.push("f.kind = ?".into());
    }
    if filter.category_id.is_some() {
        where_clauses.push("f.category_id = ?".into());
    }
    if filter.search.is_some() {
        where_clauses.push(
            "MATCH(f.name, f.description, f.identifier) AGAINST (? IN NATURAL LANGUAGE MODE)".into(),
        );
    }
    let where_sql = if where_clauses.is_empty() {
        String::new()
    } else {
        format!("WHERE {}", where_clauses.join(" AND "))
    };

    let count_sql = format!("SELECT COUNT(*) FROM functions f {where_sql}");
    let list_sql = format!(
        "SELECT f.*, p.identifier AS plugin_identifier \
         FROM functions f LEFT JOIN plugins p ON p.id = f.plugin_id \
         {where_sql} ORDER BY f.created_at DESC LIMIT ? OFFSET ?"
    );

    let mut count_q = sqlx::query_as::<_, (i64,)>(&count_sql);
    if let Some(k) = filter.kind {
        count_q = count_q.bind(k);
    }
    if let Some(c) = filter.category_id {
        count_q = count_q.bind(c);
    }
    if let Some(ref s) = filter.search {
        count_q = count_q.bind(s);
    }
    let total: i64 = count_q
        .fetch_one(pool)
        .await
        .map(|(c,)| c)
        .map_err(|e| AppError::Internal(format!("function count: {e}")))?;

    #[derive(sqlx::FromRow)]
    struct Row {
        #[sqlx(flatten)]
        f: Function,
        plugin_identifier: Option<String>,
    }

    let mut list_q = sqlx::query_as::<_, Row>(&list_sql);
    if let Some(k) = filter.kind {
        list_q = list_q.bind(k);
    }
    if let Some(c) = filter.category_id {
        list_q = list_q.bind(c);
    }
    if let Some(ref s) = filter.search {
        list_q = list_q.bind(s);
    }
    let rows = list_q
        .bind(filter.limit)
        .bind(filter.offset)
        .fetch_all(pool)
        .await
        .map_err(|e| AppError::Internal(format!("function list: {e}")))?;

    Ok(FunctionList {
        items: rows
            .into_iter()
            .map(|r| FunctionListItem {
                function: r.f,
                plugin_identifier: r.plugin_identifier,
            })
            .collect(),
        total,
        offset: filter.offset,
        limit: filter.limit,
    })
}

pub async fn update(pool: &MySqlPool, id: i64, meta: UpdateMeta) -> Result<Function, AppError> {
    // 仅 custom 可编辑（builtin 禁止）
    let existing = fetch_by_id(pool, id).await?;
    if existing.kind == 1 {
        return Err(AppError::Conflict("内置 function 不可编辑".into()));
    }

    crate::services::optimistic_lock::check_and_bump(pool, "functions", id, meta.updated_at).await?;

    if let Some(ref s) = meta.input_schema {
        validate_schema(s, "input_schema")?;
    }
    if let Some(ref s) = meta.output_schema {
        validate_schema(s, "output_schema")?;
    }

    sqlx::query(
        r#"UPDATE functions SET
              name = COALESCE(?, name),
              description = COALESCE(?, description),
              category_id = COALESCE(?, category_id),
              input_schema = COALESCE(?, input_schema),
              output_schema = COALESCE(?, output_schema)
           WHERE id = ?"#,
    )
    .bind(&meta.name)
    .bind(&meta.description)
    .bind(meta.category_id)
    .bind(&meta.input_schema)
    .bind(&meta.output_schema)
    .bind(id)
    .execute(pool)
    .await
    .map_err(|e| AppError::Internal(format!("function update: {e}")))?;

    if let Some(ref tags) = meta.tag_ids {
        sqlx::query("DELETE FROM taggings WHERE entity_type='function' AND entity_id = ?")
            .bind(id)
            .execute(pool)
            .await
            .map_err(|e| AppError::Internal(format!("tag clear: {e}")))?;
        for tid in tags {
            sqlx::query(
                "INSERT IGNORE INTO taggings (tag_id, entity_type, entity_id) VALUES (?, 'function', ?)"
            )
            .bind(tid)
            .bind(id)
            .execute(pool)
            .await
            .map_err(|e| AppError::Internal(format!("tag insert: {e}")))?;
        }
    }
    fetch_by_id(pool, id).await
}

/// 删除：builtin 拒绝；custom 在被 workflow_nodes / tools 引用时拒绝（4093）。
pub async fn delete(pool: &MySqlPool, id: i64) -> Result<(), AppError> {
    let existing = fetch_by_id(pool, id).await?;
    if existing.kind == 1 {
        return Err(AppError::Conflict("内置 function 不可删除".into()));
    }
    let mut tx = pool
        .begin()
        .await
        .map_err(|e| AppError::Internal(format!("tx begin: {e}")))?;

    sqlx::query("SELECT id FROM functions WHERE id = ? FOR UPDATE")
        .bind(id)
        .fetch_one(&mut *tx)
        .await
        .map_err(|e| AppError::Internal(format!("function lock: {e}")))?;

    let cnt: (i64,) = sqlx::query_as(
        r#"SELECT
           (SELECT COUNT(*) FROM workflow_nodes WHERE function_id = ?) +
           (SELECT COUNT(*) FROM tools WHERE function_id = ?) AS cnt"#,
    )
    .bind(id)
    .bind(id)
    .fetch_one(&mut *tx)
    .await
    .map_err(|e| AppError::Internal(format!("function ref count: {e}")))?;
    if cnt.0 > 0 {
        return Err(AppError::ResourceInUse(format!(
            "function 被 {} 个 workflow node / tool 引用，无法删除",
            cnt.0
        )));
    }

    sqlx::query("DELETE FROM functions WHERE id = ?")
        .bind(id)
        .execute(&mut *tx)
        .await
        .map_err(|e| AppError::Internal(format!("function delete: {e}")))?;
    tx.commit()
        .await
        .map_err(|e| AppError::Internal(format!("tx commit: {e}")))?;
    Ok(())
}
