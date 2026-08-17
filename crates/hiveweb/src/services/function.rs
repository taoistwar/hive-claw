//! Function service (T081 / US2)
//!
//! Builtin (`kind=1`) — 由代码在启动期 idempotent upsert 写入；API 拒绝创建 / 删除。
//! Custom (`kind=2`) — 必须绑定已存在且未软删的 Plugin + plugin_export。

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use sqlx::MySqlPool;

use crate::db::sql_safety::audit_sql;
use crate::models::Function;
use crate::services::optimistic_lock::OptimisticLockTable;
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
    pub required_capabilities: Option<Vec<String>>,
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
    pub required_capabilities: Option<Vec<String>>,
    pub tag_ids: Option<Vec<i64>>,
    /// 乐观锁
    pub updated_at: DateTime<Utc>,
}

#[derive(Debug, Serialize, sqlx::FromRow)]
pub struct TagSummary {
    pub id: i64,
    pub name: String,
}

#[derive(Debug, Serialize)]
pub struct FunctionListItem {
    #[serde(flatten)]
    pub function: Function,
    pub plugin_identifier: Option<String>,
    #[serde(default)]
    pub tags: Vec<TagSummary>,
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
    pub identifier: Option<String>,
    pub name: Option<String>,
    pub plugin_id: Option<i64>,
    pub plugin_identifier: Option<String>,
    pub required_capabilities: Option<String>,
    pub tag_id: Option<i64>,
    pub created_at_start: Option<String>,
    pub created_at_end: Option<String>,
    pub updated_at_start: Option<String>,
    pub updated_at_end: Option<String>,
}

fn validate_schema(schema: &Value, label: &str) -> Result<(), AppError> {
    // 前置检查：常见 JSON Schema 错误，提供更友好的中文提示
    if let Some(required) = schema.get("required") {
        if required.is_boolean() {
            return Err(AppError::BadRequest(format!(
                "{label} 中 required 字段格式错误：required 应为字符串数组（例如 [\"field1\", \"field2\"]），不能是布尔值 true/false。若 schema 的顶层 type 为 string / number / boolean 等基础类型，请移除 required 字段。"
            )));
        }
        if !required.is_array() {
            return Err(AppError::BadRequest(format!(
                "{label} 中 required 字段格式错误：required 应为字符串数组，例如 [\"field1\", \"field2\"]。"
            )));
        }
    }

    if !schema.is_object() {
        return Err(AppError::BadRequest(format!(
            "{label} 必须是 JSON 对象，不能是 {}。",
            if schema.is_array() {
                "数组"
            } else if schema.is_string() {
                "字符串"
            } else if schema.is_number() {
                "数字"
            } else {
                "布尔值或其他类型"
            }
        )));
    }

    // 无 type 的空 schema {} 是合法的，表示接受任意值，交由 jsonschema 校验

    // 轻量 JSON Schema 校验：仅尝试编译，编译通过即认为格式合法
    jsonschema::JSONSchema::compile(schema)
        .map_err(|e| AppError::BadRequest(format!("{label} 不是合法 JSON Schema: {e}")))?;
    Ok(())
}

pub async fn create_custom(pool: &MySqlPool, meta: CreateMeta) -> Result<Function, AppError> {
    validate_schema(&meta.input_schema, "input_schema")?;
    validate_schema(&meta.output_schema, "output_schema")?;

    let mut tx = pool
        .begin()
        .await
        .map_err(|e| AppError::Internal(format!("function transaction begin: {e}")))?;

    // 与 Plugin 软删除的 FOR UPDATE 互斥，且锁持有到 Function 写入完成。
    let plugin_row: Option<(i64, Option<DateTime<Utc>>)> =
        sqlx::query_as("SELECT id, deleted_at FROM plugins WHERE id = ? FOR SHARE")
            .bind(meta.plugin_id)
            .fetch_optional(&mut *tx)
            .await
            .map_err(|e| AppError::Internal(format!("plugin lookup: {e}")))?;
    let Some((_, deleted_at)) = plugin_row else {
        return Err(AppError::NotFound(format!(
            "plugin id={} not found",
            meta.plugin_id
        )));
    };
    if deleted_at.is_some() {
        return Err(AppError::ResourceInUse(format!(
            "plugin id={} 已软删除，无法绑定 Function",
            meta.plugin_id
        )));
    }

    let res = sqlx::query(
        r#"INSERT INTO functions
           (identifier, name, description, kind, input_schema, output_schema, plugin_id, plugin_export, category_id, required_capabilities)
           VALUES (?, ?, ?, 2, ?, ?, ?, ?, ?, ?)"#,
    )
    .bind(&meta.identifier)
    .bind(&meta.name)
    .bind(&meta.description)
    .bind(&meta.input_schema)
    .bind(&meta.output_schema)
    .bind(meta.plugin_id)
    .bind(&meta.plugin_export)
    .bind(meta.category_id)
    .bind(meta.required_capabilities.as_ref().map(|c| serde_json::to_value(c).unwrap_or(Value::Array(vec![]))))
    .execute(&mut *tx)
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
        .execute(&mut *tx)
        .await
        .map_err(|e| AppError::Internal(format!("tag bind: {e}")))?;
    }

    tx.commit()
        .await
        .map_err(|e| AppError::Internal(format!("function transaction commit: {e}")))?;

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
    if filter.identifier.is_some() {
        where_clauses.push("f.identifier LIKE ?".into());
    }
    if filter.name.is_some() {
        where_clauses.push("f.name LIKE ?".into());
    }
    if filter.plugin_id.is_some() {
        where_clauses.push("f.plugin_id = ?".into());
    }
    if filter.plugin_identifier.is_some() {
        where_clauses.push("p.identifier LIKE ?".into());
    }
    if filter.required_capabilities.is_some() {
        where_clauses.push("JSON_CONTAINS(f.required_capabilities, ?)".into());
    }
    if filter.tag_id.is_some() {
        where_clauses.push(
            "f.id IN (SELECT entity_id FROM taggings WHERE entity_type='function' AND tag_id=?)"
                .into(),
        );
    }
    if filter.created_at_start.is_some() {
        where_clauses.push("f.created_at >= ?".into());
    }
    if filter.created_at_end.is_some() {
        where_clauses.push("f.created_at <= ?".into());
    }
    if filter.updated_at_start.is_some() {
        where_clauses.push("f.updated_at >= ?".into());
    }
    if filter.updated_at_end.is_some() {
        where_clauses.push("f.updated_at <= ?".into());
    }
    if filter.search.is_some() {
        where_clauses.push("(f.name LIKE ? OR f.description LIKE ? OR f.identifier LIKE ?)".into());
    }
    let where_sql = if where_clauses.is_empty() {
        String::new()
    } else {
        format!("WHERE {}", where_clauses.join(" AND "))
    };

    let count_sql = format!(
        "SELECT COUNT(*) FROM functions f LEFT JOIN plugins p ON p.id = f.plugin_id {where_sql}"
    );
    let list_sql = format!(
        "SELECT f.*, p.identifier AS plugin_identifier \
         FROM functions f LEFT JOIN plugins p ON p.id = f.plugin_id \
         {where_sql} ORDER BY f.created_at DESC LIMIT ? OFFSET ?"
    );

    let count_sql = audit_sql(count_sql);
    let mut count_q = sqlx::query_as::<_, (i64,)>(count_sql);
    if let Some(k) = filter.kind {
        count_q = count_q.bind(k);
    }
    if let Some(c) = filter.category_id {
        count_q = count_q.bind(c);
    }
    if let Some(ref s) = filter.identifier {
        count_q = count_q.bind(format!("%{s}%"));
    }
    if let Some(ref s) = filter.name {
        count_q = count_q.bind(format!("%{s}%"));
    }
    if let Some(pid) = filter.plugin_id {
        count_q = count_q.bind(pid);
    }
    if let Some(ref s) = filter.plugin_identifier {
        count_q = count_q.bind(format!("%{s}%"));
    }
    if let Some(ref cap) = filter.required_capabilities {
        count_q = count_q.bind(format!("\"{cap}\""));
    }
    if let Some(tid) = filter.tag_id {
        count_q = count_q.bind(tid);
    }
    if let Some(ref start) = filter.created_at_start {
        count_q = count_q.bind(start);
    }
    if let Some(ref end) = filter.created_at_end {
        count_q = count_q.bind(end);
    }
    if let Some(ref start) = filter.updated_at_start {
        count_q = count_q.bind(start);
    }
    if let Some(ref end) = filter.updated_at_end {
        count_q = count_q.bind(end);
    }
    if let Some(ref s) = filter.search {
        let like = format!("%{s}%");
        count_q = count_q.bind(like.clone()).bind(like.clone()).bind(like);
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

    let list_sql = audit_sql(list_sql);
    let mut list_q = sqlx::query_as::<_, Row>(list_sql);
    if let Some(k) = filter.kind {
        list_q = list_q.bind(k);
    }
    if let Some(c) = filter.category_id {
        list_q = list_q.bind(c);
    }
    if let Some(ref s) = filter.identifier {
        list_q = list_q.bind(format!("%{s}%"));
    }
    if let Some(ref s) = filter.name {
        list_q = list_q.bind(format!("%{s}%"));
    }
    if let Some(pid) = filter.plugin_id {
        list_q = list_q.bind(pid);
    }
    if let Some(ref s) = filter.plugin_identifier {
        list_q = list_q.bind(format!("%{s}%"));
    }
    if let Some(ref cap) = filter.required_capabilities {
        list_q = list_q.bind(format!("\"{cap}\""));
    }
    if let Some(tid) = filter.tag_id {
        list_q = list_q.bind(tid);
    }
    if let Some(ref start) = filter.created_at_start {
        list_q = list_q.bind(start);
    }
    if let Some(ref end) = filter.created_at_end {
        list_q = list_q.bind(end);
    }
    if let Some(ref start) = filter.updated_at_start {
        list_q = list_q.bind(start);
    }
    if let Some(ref end) = filter.updated_at_end {
        list_q = list_q.bind(end);
    }
    if let Some(ref s) = filter.search {
        let like = format!("%{s}%");
        list_q = list_q.bind(like.clone()).bind(like.clone()).bind(like);
    }
    let rows = list_q
        .bind(filter.limit)
        .bind(filter.offset)
        .fetch_all(pool)
        .await
        .map_err(|e| AppError::Internal(format!("function list: {e}")))?;

    let mut items = Vec::with_capacity(rows.len());
    for r in rows {
        let tags = fetch_tags(pool, r.f.id).await?;
        items.push(FunctionListItem {
            function: r.f,
            plugin_identifier: r.plugin_identifier,
            tags,
        });
    }
    Ok(FunctionList {
        items,
        total,
        offset: filter.offset,
        limit: filter.limit,
    })
}

async fn fetch_tags(pool: &MySqlPool, function_id: i64) -> Result<Vec<TagSummary>, AppError> {
    let tags = sqlx::query_as::<_, TagSummary>(
        r#"SELECT t.id, t.name FROM tags t
           JOIN taggings tg ON tg.tag_id = t.id
           WHERE tg.entity_type = 'function' AND tg.entity_id = ?"#,
    )
    .bind(function_id)
    .fetch_all(pool)
    .await
    .map_err(|e| AppError::Internal(format!("function tags: {e}")))?;
    Ok(tags)
}

pub async fn update(pool: &MySqlPool, id: i64, meta: UpdateMeta) -> Result<Function, AppError> {
    let existing = fetch_by_id(pool, id).await?;
    let is_builtin = existing.kind == 1;

    // 内置函数仅允许编辑 category_id 和 tag
    if is_builtin && meta.name.is_some() {
        return Err(AppError::Conflict(
            "内置 function 仅可编辑分类和标签".into(),
        ));
    }
    if is_builtin && meta.description.is_some() {
        return Err(AppError::Conflict(
            "内置 function 仅可编辑分类和标签".into(),
        ));
    }
    if is_builtin && meta.input_schema.is_some() {
        return Err(AppError::Conflict(
            "内置 function 仅可编辑分类和标签".into(),
        ));
    }
    if is_builtin && meta.output_schema.is_some() {
        return Err(AppError::Conflict(
            "内置 function 仅可编辑分类和标签".into(),
        ));
    }
    if is_builtin && meta.required_capabilities.is_some() {
        return Err(AppError::Conflict(
            "内置 function 仅可编辑分类和标签".into(),
        ));
    }

    crate::services::optimistic_lock::check_and_bump(
        pool,
        OptimisticLockTable::Functions,
        id,
        meta.updated_at,
    )
    .await?;

    if !is_builtin {
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
                  output_schema = COALESCE(?, output_schema),
                  required_capabilities = COALESCE(?, required_capabilities)
               WHERE id = ?"#,
        )
        .bind(&meta.name)
        .bind(&meta.description)
        .bind(meta.category_id)
        .bind(&meta.input_schema)
        .bind(&meta.output_schema)
        .bind(
            meta.required_capabilities
                .as_ref()
                .map(|c| serde_json::to_value(c).unwrap_or(Value::Array(vec![]))),
        )
        .bind(id)
        .execute(pool)
        .await
        .map_err(|e| AppError::Internal(format!("function update: {e}")))?;
    } else {
        sqlx::query(
            r#"UPDATE functions SET
                  category_id = COALESCE(?, category_id)
               WHERE id = ?"#,
        )
        .bind(meta.category_id)
        .bind(id)
        .execute(pool)
        .await
        .map_err(|e| AppError::Internal(format!("function update: {e}")))?;
    }

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

// ============================== Runtime metadata ==============================

/// Lightweight function metadata needed by the workflow runtime to dispatch a
/// function node (kind, plugin_id, plugin_export, identifier).
#[derive(Debug)]
pub struct FunctionRuntimeMeta {
    pub kind: i8,
    pub plugin_id: Option<i64>,
    pub plugin_export: Option<String>,
    pub identifier: String,
}

/// Fetch the minimal runtime metadata for a function by id.
pub async fn fetch_runtime_meta(
    pool: &MySqlPool,
    function_id: i64,
) -> Result<Option<FunctionRuntimeMeta>, sqlx::Error> {
    sqlx::query_as::<_, (i8, Option<i64>, Option<String>, String)>(
        "SELECT kind, plugin_id, plugin_export, identifier FROM functions WHERE id = ?",
    )
    .bind(function_id)
    .fetch_optional(pool)
    .await
    .map(|row| {
        row.map(
            |(kind, plugin_id, plugin_export, identifier)| FunctionRuntimeMeta {
                kind,
                plugin_id,
                plugin_export,
                identifier,
            },
        )
    })
}
