//! Tool service (T082 / US2)
//!
//! Tool 是 Function/Workflow 的包装（kind 互斥）。
//! Schema 一致性校验（data-model 不变量 #11）：
//!   - `kind=1` function-wrap：input_schema / output_schema 必须**深度 JSON 等值**
//!     于引用 function 的 schema，否则 5002 SchemaMismatch
//!   - `kind=2` workflow-wrap：input_schema 必须能赋值给 workflow 入口 function 的 input_schema
//!     （此 MVP 不做完整 sub-schema 校验，仅做 required 字段必含 + 顶层 type 一致检查）

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use sqlx::MySqlPool;

use crate::db::sql_safety::audit_sql;
use crate::models::Tool;
use crate::services::optimistic_lock::OptimisticLockTable;
use crate::utils::error::AppError;

#[derive(Debug, Deserialize)]
pub struct CreateMeta {
    pub identifier: String,
    pub name: String,
    pub description: String,
    /// 1 = function-wrap, 2 = workflow-wrap
    pub kind: i8,
    /// workspace | builtin
    pub source: String,
    /// 0 = normal, 1 = always available for all agents
    pub is_always: bool,
    pub function_id: Option<i64>,
    pub workflow_id: Option<i64>,
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
    pub input_schema: Option<Value>,
    pub output_schema: Option<Value>,
    pub category_id: Option<i64>,
    pub required_capabilities: Option<Vec<String>>,
    pub tag_ids: Option<Vec<i64>>,
    pub is_always: Option<bool>,
    pub updated_at: DateTime<Utc>,
}

#[derive(Debug, Serialize, sqlx::FromRow)]
pub struct TagSummary {
    pub id: i64,
    pub name: String,
}

#[derive(Debug, Serialize)]
pub struct ToolListItem {
    #[serde(flatten)]
    pub tool: Tool,
    pub tags: Vec<TagSummary>,
}

#[derive(Debug, Serialize)]
pub struct ToolList {
    pub items: Vec<ToolListItem>,
    pub total: i64,
    pub offset: i64,
    pub limit: i64,
}

#[derive(Debug, Default)]
pub struct ListFilter {
    pub offset: i64,
    pub limit: i64,
    pub search: Option<String>,
    pub kind: Option<i8>,
    pub source: Option<String>,
    pub category_id: Option<i64>,
    pub created_at_start: Option<chrono::NaiveDateTime>,
    pub created_at_end: Option<chrono::NaiveDateTime>,
    pub updated_at_start: Option<chrono::NaiveDateTime>,
    pub updated_at_end: Option<chrono::NaiveDateTime>,
}

/// 深度 JSON 值相等（顺序无关的 object key 比较）
fn json_deep_equal(a: &Value, b: &Value) -> bool {
    match (a, b) {
        (Value::Object(am), Value::Object(bm)) => {
            am.len() == bm.len()
                && am
                    .iter()
                    .all(|(k, v)| bm.get(k).map(|bv| json_deep_equal(v, bv)).unwrap_or(false))
        }
        (Value::Array(av), Value::Array(bv)) => {
            av.len() == bv.len() && av.iter().zip(bv.iter()).all(|(x, y)| json_deep_equal(x, y))
        }
        _ => a == b,
    }
}

async fn validate_schemas_function(
    pool: &MySqlPool,
    function_id: i64,
    tool_input: &Value,
    tool_output: &Value,
) -> Result<(), AppError> {
    let row: Option<(Value, Value)> =
        sqlx::query_as("SELECT input_schema, output_schema FROM functions WHERE id = ?")
            .bind(function_id)
            .fetch_optional(pool)
            .await
            .map_err(|e| AppError::Internal(format!("function schema fetch: {e}")))?;
    let (fn_in, fn_out) =
        row.ok_or_else(|| AppError::NotFound(format!("function id={function_id} not found")))?;
    if !json_deep_equal(&fn_in, tool_input) {
        return Err(AppError::SchemaMismatch(
            "Tool input_schema 与 function 不一致".into(),
        ));
    }
    if !json_deep_equal(&fn_out, tool_output) {
        return Err(AppError::SchemaMismatch(
            "Tool output_schema 与 function 不一致".into(),
        ));
    }
    Ok(())
}

async fn validate_schemas_workflow(
    pool: &MySqlPool,
    workflow_id: i64,
    tool_input: &Value,
) -> Result<(), AppError> {
    let row: Option<(Value,)> = sqlx::query_as("SELECT input_schema FROM workflows WHERE id = ?")
        .bind(workflow_id)
        .fetch_optional(pool)
        .await
        .map_err(|e| AppError::Internal(format!("workflow input_schema fetch: {e}")))?;
    let (wf_input,) =
        row.ok_or_else(|| AppError::NotFound(format!("workflow id={workflow_id} not found")))?;

    let empty_arr = Value::Array(vec![]);
    let empty_obj = Value::Object(serde_json::Map::new());
    let wf_required = wf_input.get("required").unwrap_or(&empty_arr);
    let wf_props = wf_input.get("properties").unwrap_or(&empty_obj);
    let tool_props = tool_input.get("properties").unwrap_or(&empty_obj);

    if let Some(req_arr) = wf_required.as_array() {
        for r in req_arr {
            let Some(field) = r.as_str() else { continue };
            let wf_type = wf_props
                .get(field)
                .and_then(|s| s.get("type"))
                .and_then(|t| t.as_str());
            let tool_type = tool_props
                .get(field)
                .and_then(|s| s.get("type"))
                .and_then(|t| t.as_str());
            if tool_type.is_none() {
                return Err(AppError::SchemaMismatch(format!(
                    "Tool input_schema 缺 workflow 入口必填字段「{field}」"
                )));
            }
            if wf_type != tool_type {
                return Err(AppError::SchemaMismatch(format!(
                    "Tool input_schema 字段「{field}」类型不匹配（期望 {:?}，得到 {:?}）",
                    wf_type, tool_type
                )));
            }
        }
    }
    Ok(())
}

async fn fetch_tags(pool: &MySqlPool, tool_id: i64) -> Result<Vec<TagSummary>, AppError> {
    let tags = sqlx::query_as::<_, TagSummary>(
        r#"SELECT t.id, t.name FROM tags t
           JOIN taggings tg ON tg.tag_id = t.id
           WHERE tg.entity_type = 'tool' AND tg.entity_id = ?"#,
    )
    .bind(tool_id)
    .fetch_all(pool)
    .await
    .map_err(|e| AppError::Internal(format!("tool tags: {e}")))?;
    Ok(tags)
}

async fn fetch_tool_with_tags(pool: &MySqlPool, id: i64) -> Result<ToolListItem, AppError> {
    let tool = sqlx::query_as::<_, Tool>("SELECT * FROM tools WHERE id = ?")
        .bind(id)
        .fetch_optional(pool)
        .await
        .map_err(|e| AppError::Internal(format!("tool fetch: {e}")))?
        .ok_or_else(|| AppError::NotFound(format!("tool id={id} not found")))?;
    let tags = fetch_tags(pool, id).await?;
    Ok(ToolListItem { tool, tags })
}

pub async fn create(pool: &MySqlPool, meta: CreateMeta) -> Result<ToolListItem, AppError> {
    let source = if meta.source.is_empty() || meta.source == "workspace" {
        "workspace".to_string()
    } else if meta.source == "builtin" {
        "builtin".to_string()
    } else {
        return Err(AppError::BadRequest(
            "source 必须为 workspace 或 builtin".into(),
        ));
    };

    match meta.kind {
        1 => {
            if let Some(fid) = meta.function_id {
                if meta.workflow_id.is_some() {
                    return Err(AppError::BadRequest(
                        "kind=1 时不允许提供 workflow_id".into(),
                    ));
                }
                if source == "builtin" {
                    let f_kind: Option<i8> =
                        sqlx::query_scalar("SELECT kind FROM functions WHERE id = ?")
                            .bind(fid)
                            .fetch_optional(pool)
                            .await
                            .map_err(|e| AppError::Internal(format!("function fetch: {e}")))?
                            .ok_or_else(|| {
                                AppError::NotFound(format!("function id={fid} not found"))
                            })?;
                    if f_kind != Some(1) {
                        return Err(AppError::BadRequest(
                            "builtin Tool 只能包装 builtin Function（kind=1）".into(),
                        ));
                    }
                }
                validate_schemas_function(pool, fid, &meta.input_schema, &meta.output_schema)
                    .await?;
            }
        }
        2 => {
            let wid = meta
                .workflow_id
                .ok_or_else(|| AppError::BadRequest("kind=2 时必须提供 workflow_id".into()))?;
            if meta.function_id.is_some() {
                return Err(AppError::BadRequest(
                    "kind=2 时不允许提供 function_id".into(),
                ));
            }
            if source == "builtin" {
                return Err(AppError::BadRequest(
                    "builtin Tool 不允许包装 Workflow".into(),
                ));
            }
            validate_schemas_workflow(pool, wid, &meta.input_schema).await?;
        }
        _ => return Err(AppError::BadRequest("kind 必须为 1 或 2".into())),
    }

    let res = sqlx::query(
        r#"INSERT INTO tools
           (identifier, name, description, kind, source, is_always, function_id, workflow_id, input_schema, output_schema, category_id, required_capabilities)
           VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?)"#,
    )
    .bind(&meta.identifier)
    .bind(&meta.name)
    .bind(&meta.description)
    .bind(meta.kind)
    .bind(&source)
    .bind(meta.is_always)
    .bind(meta.function_id)
    .bind(meta.workflow_id)
    .bind(&meta.input_schema)
    .bind(&meta.output_schema)
    .bind(meta.category_id)
    .bind(meta.required_capabilities.as_ref().map(|c| serde_json::to_value(c).unwrap_or(Value::Array(vec![]))))
    .execute(pool)
    .await
    .map_err(|e| {
        let msg = e.to_string();
        if msg.contains("Duplicate") {
            AppError::Conflict(format!("tool identifier 已存在：{}", meta.identifier))
        } else {
            AppError::Internal(format!("tool insert: {e}"))
        }
    })?;

    let id = res.last_insert_id() as i64;

    for tid in &meta.tag_ids {
        sqlx::query(
            "INSERT IGNORE INTO taggings (tag_id, entity_type, entity_id) VALUES (?, 'tool', ?)",
        )
        .bind(tid)
        .bind(id)
        .execute(pool)
        .await
        .map_err(|e| AppError::Internal(format!("tool tag insert: {e}")))?;
    }

    fetch_tool_with_tags(pool, id).await
}

pub async fn fetch_by_id(pool: &MySqlPool, id: i64) -> Result<Tool, AppError> {
    sqlx::query_as::<_, Tool>("SELECT * FROM tools WHERE id = ?")
        .bind(id)
        .fetch_optional(pool)
        .await
        .map_err(|e| AppError::Internal(format!("tool fetch: {e}")))?
        .ok_or_else(|| AppError::NotFound(format!("tool id={id} not found")))
}

pub async fn fetch_by_id_with_tags(pool: &MySqlPool, id: i64) -> Result<ToolListItem, AppError> {
    fetch_tool_with_tags(pool, id).await
}

pub async fn list(pool: &MySqlPool, filter: ListFilter) -> Result<ToolList, AppError> {
    let mut where_clauses = Vec::<String>::new();
    if filter.kind.is_some() {
        where_clauses.push("kind = ?".into());
    }
    if filter.source.is_some() {
        where_clauses.push("source = ?".into());
    }
    if filter.category_id.is_some() {
        where_clauses.push("category_id = ?".into());
    }
    if filter.search.is_some() {
        where_clauses.push("(name LIKE ? OR identifier LIKE ? OR description LIKE ?)".into());
    }
    if filter.created_at_start.is_some() {
        where_clauses.push("created_at >= ?".into());
    }
    if filter.created_at_end.is_some() {
        where_clauses.push("created_at < ?".into());
    }
    if filter.updated_at_start.is_some() {
        where_clauses.push("updated_at >= ?".into());
    }
    if filter.updated_at_end.is_some() {
        where_clauses.push("updated_at < ?".into());
    }
    let where_sql = if where_clauses.is_empty() {
        String::new()
    } else {
        format!("WHERE {}", where_clauses.join(" AND "))
    };
    let count_sql = format!("SELECT COUNT(*) FROM tools {where_sql}");
    let list_sql =
        format!("SELECT * FROM tools {where_sql} ORDER BY created_at DESC LIMIT ? OFFSET ?");

    let count_sql = audit_sql(count_sql);
    let mut count_q = sqlx::query_as::<_, (i64,)>(count_sql);
    if let Some(k) = filter.kind {
        count_q = count_q.bind(k);
    }
    if let Some(ref s) = filter.source {
        count_q = count_q.bind(s);
    }
    if let Some(c) = filter.category_id {
        count_q = count_q.bind(c);
    }
    if let Some(ref s) = filter.search {
        let like = format!("%{s}%");
        count_q = count_q.bind(like.clone()).bind(like.clone()).bind(like);
    }
    if let Some(start) = filter.created_at_start {
        count_q = count_q.bind(start);
    }
    if let Some(end) = filter.created_at_end {
        count_q = count_q.bind(end);
    }
    if let Some(start) = filter.updated_at_start {
        count_q = count_q.bind(start);
    }
    if let Some(end) = filter.updated_at_end {
        count_q = count_q.bind(end);
    }
    let total = count_q
        .fetch_one(pool)
        .await
        .map(|(c,)| c)
        .map_err(|e| AppError::Internal(format!("tool count: {e}")))?;

    let list_sql = audit_sql(list_sql);
    let mut list_q = sqlx::query_as::<_, Tool>(list_sql);
    if let Some(k) = filter.kind {
        list_q = list_q.bind(k);
    }
    if let Some(ref s) = filter.source {
        list_q = list_q.bind(s);
    }
    if let Some(c) = filter.category_id {
        list_q = list_q.bind(c);
    }
    if let Some(ref s) = filter.search {
        let like = format!("%{s}%");
        list_q = list_q.bind(like.clone()).bind(like.clone()).bind(like);
    }
    if let Some(start) = filter.created_at_start {
        list_q = list_q.bind(start);
    }
    if let Some(end) = filter.created_at_end {
        list_q = list_q.bind(end);
    }
    if let Some(start) = filter.updated_at_start {
        list_q = list_q.bind(start);
    }
    if let Some(end) = filter.updated_at_end {
        list_q = list_q.bind(end);
    }
    let tools = list_q
        .bind(filter.limit)
        .bind(filter.offset)
        .fetch_all(pool)
        .await
        .map_err(|e| AppError::Internal(format!("tool list: {e}")))?;

    let mut items = Vec::with_capacity(tools.len());
    for t in tools {
        let tags = fetch_tags(pool, t.id).await?;
        items.push(ToolListItem { tool: t, tags });
    }

    Ok(ToolList {
        items,
        total,
        offset: filter.offset,
        limit: filter.limit,
    })
}

pub async fn update(pool: &MySqlPool, id: i64, meta: UpdateMeta) -> Result<ToolListItem, AppError> {
    crate::services::optimistic_lock::check_and_bump(pool, OptimisticLockTable::Tools, id, meta.updated_at).await?;
    let existing = fetch_by_id(pool, id).await?;

    if existing.source == "builtin" {
        if meta.name.is_some()
            || meta.description.is_some()
            || meta.input_schema.is_some()
            || meta.output_schema.is_some()
            || meta.required_capabilities.is_some()
        {
            return Err(AppError::BuiltinToolProtected(format!(
                "内置工具「{}」仅可修改分类、标签和 always",
                existing.identifier
            )));
        }

        sqlx::query(
            r#"UPDATE tools SET
                  category_id = COALESCE(?, category_id),
                  is_always = COALESCE(?, is_always)
               WHERE id = ?"#,
        )
        .bind(meta.category_id)
        .bind(meta.is_always)
        .bind(id)
        .execute(pool)
        .await
        .map_err(|e| AppError::Internal(format!("builtin tool update: {e}")))?;

        if let Some(ref tags) = meta.tag_ids {
            sqlx::query("DELETE FROM taggings WHERE entity_type='tool' AND entity_id = ?")
                .bind(id)
                .execute(pool)
                .await
                .map_err(|e| AppError::Internal(format!("tag clear: {e}")))?;
            for tid in tags {
                sqlx::query(
                    "INSERT IGNORE INTO taggings (tag_id, entity_type, entity_id) VALUES (?, 'tool', ?)"
                )
                .bind(tid)
                .bind(id)
                .execute(pool)
                .await
                .map_err(|e| AppError::Internal(format!("tool tag insert: {e}")))?;
            }
        }

        return fetch_tool_with_tags(pool, id).await;
    }

    let new_in = meta
        .input_schema
        .clone()
        .unwrap_or(existing.input_schema.clone());
    let new_out = meta
        .output_schema
        .clone()
        .unwrap_or(existing.output_schema.clone());

    if meta.input_schema.is_some() || meta.output_schema.is_some() {
        match existing.kind {
            1 => {
                validate_schemas_function(pool, existing.function_id.unwrap(), &new_in, &new_out)
                    .await?;
            }
            2 => {
                validate_schemas_workflow(pool, existing.workflow_id.unwrap(), &new_in).await?;
            }
            _ => {}
        }
    }

    sqlx::query(
        r#"UPDATE tools SET
              name = COALESCE(?, name),
              description = COALESCE(?, description),
              input_schema = COALESCE(?, input_schema),
              output_schema = COALESCE(?, output_schema),
              category_id = COALESCE(?, category_id),
              required_capabilities = COALESCE(?, required_capabilities),
              is_always = COALESCE(?, is_always)
           WHERE id = ?"#,
    )
    .bind(&meta.name)
    .bind(&meta.description)
    .bind(&meta.input_schema)
    .bind(&meta.output_schema)
    .bind(meta.category_id)
    .bind(
        meta.required_capabilities
            .as_ref()
            .map(|c| serde_json::to_value(c).unwrap_or(Value::Array(vec![]))),
    )
    .bind(meta.is_always)
    .bind(id)
    .execute(pool)
    .await
    .map_err(|e| AppError::Internal(format!("tool update: {e}")))?;

    if let Some(ref tags) = meta.tag_ids {
        sqlx::query("DELETE FROM taggings WHERE entity_type='tool' AND entity_id = ?")
            .bind(id)
            .execute(pool)
            .await
            .map_err(|e| AppError::Internal(format!("tag clear: {e}")))?;
        for tid in tags {
            sqlx::query(
                "INSERT IGNORE INTO taggings (tag_id, entity_type, entity_id) VALUES (?, 'tool', ?)"
            )
            .bind(tid)
            .bind(id)
            .execute(pool)
            .await
            .map_err(|e| AppError::Internal(format!("tool tag insert: {e}")))?;
        }
    }

    fetch_tool_with_tags(pool, id).await
}

/// Tool 被 agent_tools 引用 → 4093；builtin Tool → 5008
pub async fn delete(pool: &MySqlPool, id: i64) -> Result<(), AppError> {
    let existing = fetch_by_id(pool, id).await?;
    if existing.source == "builtin" {
        return Err(AppError::BuiltinToolProtected(format!(
            "内置工具「{}」不可删除",
            existing.identifier
        )));
    }
    let mut tx = pool
        .begin()
        .await
        .map_err(|e| AppError::Internal(format!("tx begin: {e}")))?;
    sqlx::query("SELECT id FROM tools WHERE id = ? FOR UPDATE")
        .bind(id)
        .fetch_one(&mut *tx)
        .await
        .map_err(|e| AppError::Internal(format!("tool lock: {e}")))?;
    let cnt: (i64,) = sqlx::query_as("SELECT COUNT(*) FROM agent_tools WHERE tool_id = ?")
        .bind(id)
        .fetch_one(&mut *tx)
        .await
        .map_err(|e| AppError::Internal(format!("tool ref count: {e}")))?;
    if cnt.0 > 0 {
        return Err(AppError::ResourceInUse(format!(
            "tool 被 {} 个 agent 引用，无法删除",
            cnt.0
        )));
    }
    sqlx::query("DELETE FROM tools WHERE id = ?")
        .bind(id)
        .execute(&mut *tx)
        .await
        .map_err(|e| AppError::Internal(format!("tool delete: {e}")))?;
    tx.commit()
        .await
        .map_err(|e| AppError::Internal(format!("tx commit: {e}")))?;
    Ok(())
}
