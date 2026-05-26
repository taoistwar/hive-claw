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

use crate::models::Tool;
use crate::utils::error::AppError;

#[derive(Debug, Deserialize)]
pub struct CreateMeta {
    pub identifier: String,
    pub name: String,
    pub description: String,
    /// 1 = function-wrap, 2 = workflow-wrap
    pub kind: i8,
    pub function_id: Option<i64>,
    pub workflow_id: Option<i64>,
    pub input_schema: Value,
    pub output_schema: Value,
}

#[derive(Debug, Deserialize)]
pub struct UpdateMeta {
    pub name: Option<String>,
    pub description: Option<String>,
    pub input_schema: Option<Value>,
    pub output_schema: Option<Value>,
    pub updated_at: DateTime<Utc>,
}

#[derive(Debug, Serialize)]
pub struct ToolList {
    pub items: Vec<Tool>,
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
    let row: Option<(Value, Value)> = sqlx::query_as(
        "SELECT input_schema, output_schema FROM functions WHERE id = ?",
    )
    .bind(function_id)
    .fetch_optional(pool)
    .await
    .map_err(|e| AppError::Internal(format!("function schema fetch: {e}")))?;
    let (fn_in, fn_out) = row
        .ok_or_else(|| AppError::NotFound(format!("function id={function_id} not found")))?;
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
    // 入口 = 无 incoming edge 的 workflow_node；取其 function 的 input_schema
    let row: Option<(Value,)> = sqlx::query_as(
        r#"SELECT f.input_schema FROM workflow_nodes n
           JOIN functions f ON f.id = n.function_id
           WHERE n.workflow_id = ?
             AND n.id NOT IN (SELECT dst_node_id FROM workflow_edges WHERE workflow_id = ?)
           LIMIT 1"#,
    )
    .bind(workflow_id)
    .bind(workflow_id)
    .fetch_optional(pool)
    .await
    .map_err(|e| AppError::Internal(format!("workflow entry schema: {e}")))?;
    let (entry,) = row.ok_or_else(|| {
        AppError::BadRequest(format!(
            "workflow id={workflow_id} 没有入口节点，无法包装为 Tool"
        ))
    })?;

    // sub-schema 检查（MVP 简化）：tool.required ⊇ entry.required；
    // tool.properties 中 entry.required 项的 type 必须一致
    let empty_arr = Value::Array(vec![]);
    let empty_obj = Value::Object(serde_json::Map::new());
    let entry_required = entry.get("required").unwrap_or(&empty_arr);
    let entry_props = entry.get("properties").unwrap_or(&empty_obj);
    let tool_props = tool_input.get("properties").unwrap_or(&empty_obj);

    if let Some(req_arr) = entry_required.as_array() {
        for r in req_arr {
            let Some(field) = r.as_str() else { continue };
            let entry_type = entry_props
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
            if entry_type != tool_type {
                return Err(AppError::SchemaMismatch(format!(
                    "Tool input_schema 字段「{field}」类型不匹配（期望 {:?}，得到 {:?}）",
                    entry_type, tool_type
                )));
            }
        }
    }
    Ok(())
}

pub async fn create(pool: &MySqlPool, meta: CreateMeta) -> Result<Tool, AppError> {
    match meta.kind {
        1 => {
            let fid = meta.function_id.ok_or_else(|| {
                AppError::BadRequest("kind=1 时必须提供 function_id".into())
            })?;
            if meta.workflow_id.is_some() {
                return Err(AppError::BadRequest("kind=1 时不允许提供 workflow_id".into()));
            }
            validate_schemas_function(pool, fid, &meta.input_schema, &meta.output_schema).await?;
        }
        2 => {
            let wid = meta.workflow_id.ok_or_else(|| {
                AppError::BadRequest("kind=2 时必须提供 workflow_id".into())
            })?;
            if meta.function_id.is_some() {
                return Err(AppError::BadRequest("kind=2 时不允许提供 function_id".into()));
            }
            validate_schemas_workflow(pool, wid, &meta.input_schema).await?;
        }
        _ => return Err(AppError::BadRequest("kind 必须为 1 或 2".into())),
    }

    let res = sqlx::query(
        r#"INSERT INTO tools
           (identifier, name, description, kind, function_id, workflow_id, input_schema, output_schema)
           VALUES (?, ?, ?, ?, ?, ?, ?, ?)"#,
    )
    .bind(&meta.identifier)
    .bind(&meta.name)
    .bind(&meta.description)
    .bind(meta.kind)
    .bind(meta.function_id)
    .bind(meta.workflow_id)
    .bind(&meta.input_schema)
    .bind(&meta.output_schema)
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

    fetch_by_id(pool, res.last_insert_id() as i64).await
}

pub async fn fetch_by_id(pool: &MySqlPool, id: i64) -> Result<Tool, AppError> {
    sqlx::query_as::<_, Tool>("SELECT * FROM tools WHERE id = ?")
        .bind(id)
        .fetch_optional(pool)
        .await
        .map_err(|e| AppError::Internal(format!("tool fetch: {e}")))?
        .ok_or_else(|| AppError::NotFound(format!("tool id={id} not found")))
}

pub async fn list(pool: &MySqlPool, filter: ListFilter) -> Result<ToolList, AppError> {
    let mut where_clauses = Vec::<String>::new();
    if filter.kind.is_some() {
        where_clauses.push("kind = ?".into());
    }
    if filter.search.is_some() {
        where_clauses.push("(name LIKE ? OR identifier LIKE ? OR description LIKE ?)".into());
    }
    let where_sql = if where_clauses.is_empty() {
        String::new()
    } else {
        format!("WHERE {}", where_clauses.join(" AND "))
    };
    let count_sql = format!("SELECT COUNT(*) FROM tools {where_sql}");
    let list_sql =
        format!("SELECT * FROM tools {where_sql} ORDER BY created_at DESC LIMIT ? OFFSET ?");

    let mut count_q = sqlx::query_as::<_, (i64,)>(&count_sql);
    if let Some(k) = filter.kind {
        count_q = count_q.bind(k);
    }
    if let Some(ref s) = filter.search {
        let like = format!("%{s}%");
        count_q = count_q.bind(like.clone()).bind(like.clone()).bind(like);
    }
    let total = count_q
        .fetch_one(pool)
        .await
        .map(|(c,)| c)
        .map_err(|e| AppError::Internal(format!("tool count: {e}")))?;

    let mut list_q = sqlx::query_as::<_, Tool>(&list_sql);
    if let Some(k) = filter.kind {
        list_q = list_q.bind(k);
    }
    if let Some(ref s) = filter.search {
        let like = format!("%{s}%");
        list_q = list_q.bind(like.clone()).bind(like.clone()).bind(like);
    }
    let items = list_q
        .bind(filter.limit)
        .bind(filter.offset)
        .fetch_all(pool)
        .await
        .map_err(|e| AppError::Internal(format!("tool list: {e}")))?;
    Ok(ToolList {
        items,
        total,
        offset: filter.offset,
        limit: filter.limit,
    })
}

pub async fn update(pool: &MySqlPool, id: i64, meta: UpdateMeta) -> Result<Tool, AppError> {
    crate::services::optimistic_lock::check_and_bump(pool, "tools", id, meta.updated_at).await?;
    let existing = fetch_by_id(pool, id).await?;

    let new_in = meta.input_schema.clone().unwrap_or(existing.input_schema.clone());
    let new_out = meta
        .output_schema
        .clone()
        .unwrap_or(existing.output_schema.clone());

    // schema 变更 → 再验
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
              output_schema = COALESCE(?, output_schema)
           WHERE id = ?"#,
    )
    .bind(&meta.name)
    .bind(&meta.description)
    .bind(&meta.input_schema)
    .bind(&meta.output_schema)
    .bind(id)
    .execute(pool)
    .await
    .map_err(|e| AppError::Internal(format!("tool update: {e}")))?;

    fetch_by_id(pool, id).await
}

/// Tool 被 agent_tools 引用 → 4093
pub async fn delete(pool: &MySqlPool, id: i64) -> Result<(), AppError> {
    let mut tx = pool
        .begin()
        .await
        .map_err(|e| AppError::Internal(format!("tx begin: {e}")))?;
    sqlx::query("SELECT id FROM tools WHERE id = ? FOR UPDATE")
        .bind(id)
        .fetch_one(&mut *tx)
        .await
        .map_err(|e| AppError::Internal(format!("tool lock: {e}")))?;
    let cnt: (i64,) =
        sqlx::query_as("SELECT COUNT(*) FROM agent_tools WHERE tool_id = ?")
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
