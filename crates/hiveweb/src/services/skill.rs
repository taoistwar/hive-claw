//! Skill service (T083 / US2)
//!
//! Skill 沿用 markdown 模式：content + 可选 frontmatter（JSON 解析自 YAML）。
//! 调用 Skill = 把 content 拼接到 Agent system prompt 末尾（执行期由 runtime/agent 处理）。
//!
//! 约束：
//! - content ≤ 64 KB → 超出 4001 BadRequest
//! - source = 'builtin' → 不可删除 / 不可修改 identifier+source（5008 BuiltinSkillProtected）
//! - 删除前校验 agent_skills 引用 → 4093

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use sqlx::MySqlPool;

use crate::models::Skill;
use crate::utils::error::AppError;

const SKILL_CONTENT_MAX_BYTES: usize = 64 * 1024;

#[derive(Debug, Deserialize)]
pub struct CreateMeta {
    pub identifier: String,
    pub name: String,
    pub description: String,
    pub frontmatter: Option<Value>,
    pub content: String,
    pub is_always: Option<bool>,
    pub category_id: Option<i64>,
    pub required_capabilities: Option<Vec<String>>,
}

#[derive(Debug, Deserialize)]
pub struct UpdateMeta {
    pub name: Option<String>,
    pub description: Option<String>,
    pub frontmatter: Option<Value>,
    pub content: Option<String>,
    pub is_always: Option<bool>,
    pub category_id: Option<Option<i64>>,
    pub required_capabilities: Option<Vec<String>>,
    pub updated_at: DateTime<Utc>,
}

#[derive(Debug, Serialize)]
pub struct SkillList {
    pub items: Vec<Skill>,
    pub total: i64,
    pub offset: i64,
    pub limit: i64,
}

#[derive(Debug, Default)]
pub struct ListFilter {
    pub offset: i64,
    pub limit: i64,
    pub search: Option<String>,
    pub source: Option<String>,
    pub category_id: Option<i64>,
    pub identifier: Option<String>,
    pub name: Option<String>,
    pub description: Option<String>,
    pub required_capabilities: Option<String>,
    pub created_at_start: Option<String>,
    pub created_at_end: Option<String>,
    pub updated_at_start: Option<String>,
    pub updated_at_end: Option<String>,
}

fn validate_content(content: &str) -> Result<(), AppError> {
    if content.len() > SKILL_CONTENT_MAX_BYTES {
        return Err(AppError::BadRequest(format!(
            "Skill content 大小 {} 超过 64 KB 上限",
            content.len()
        )));
    }
    Ok(())
}

pub async fn create(pool: &MySqlPool, meta: CreateMeta) -> Result<Skill, AppError> {
    validate_content(&meta.content)?;

    let is_always = meta.is_always.unwrap_or(false) as i8;

    let res = sqlx::query(
        r#"INSERT INTO skills (identifier, name, description, frontmatter, content, source, is_always, category_id, required_capabilities)
           VALUES (?, ?, ?, ?, ?, 'workspace', ?, ?, ?)"#,
    )
    .bind(&meta.identifier)
    .bind(&meta.name)
    .bind(&meta.description)
    .bind(&meta.frontmatter)
    .bind(&meta.content)
    .bind(is_always)
    .bind(meta.category_id)
    .bind(meta.required_capabilities.as_ref().map(|c| serde_json::to_value(c).unwrap_or(Value::Array(vec![]))))
    .execute(pool)
    .await
    .map_err(|e| {
        let msg = e.to_string();
        if msg.contains("Duplicate") {
            AppError::Conflict(format!("skill identifier 已存在：{}", meta.identifier))
        } else {
            AppError::Internal(format!("skill insert: {e}"))
        }
    })?;

    fetch_by_id(pool, res.last_insert_id() as i64).await
}

pub async fn fetch_by_id(pool: &MySqlPool, id: i64) -> Result<Skill, AppError> {
    sqlx::query_as::<_, Skill>("SELECT * FROM skills WHERE id = ?")
        .bind(id)
        .fetch_optional(pool)
        .await
        .map_err(|e| AppError::Internal(format!("skill fetch: {e}")))?
        .ok_or_else(|| AppError::NotFound(format!("skill id={id} not found")))
}

pub async fn list(pool: &MySqlPool, filter: ListFilter) -> Result<SkillList, AppError> {
    let mut where_clauses = Vec::<String>::new();
    if filter.source.is_some() {
        where_clauses.push("source = ?".into());
    }
    if filter.category_id.is_some() {
        where_clauses.push("category_id = ?".into());
    }
    if filter.identifier.is_some() {
        where_clauses.push("identifier LIKE ?".into());
    }
    if filter.name.is_some() {
        where_clauses.push("name LIKE ?".into());
    }
    if filter.description.is_some() {
        where_clauses.push("description LIKE ?".into());
    }
    if filter.required_capabilities.is_some() {
        where_clauses.push("JSON_CONTAINS(required_capabilities, ?)".into());
    }
    if filter.created_at_start.is_some() {
        where_clauses.push("created_at >= ?".into());
    }
    if filter.created_at_end.is_some() {
        where_clauses.push("created_at <= ?".into());
    }
    if filter.updated_at_start.is_some() {
        where_clauses.push("updated_at >= ?".into());
    }
    if filter.updated_at_end.is_some() {
        where_clauses.push("updated_at <= ?".into());
    }
    if filter.search.is_some() {
        where_clauses.push("(name LIKE ? OR identifier LIKE ? OR description LIKE ? OR content LIKE ?)".into());
    }
    let where_sql = if where_clauses.is_empty() {
        String::new()
    } else {
        format!("WHERE {}", where_clauses.join(" AND "))
    };
    let count_sql = format!("SELECT COUNT(*) FROM skills {where_sql}");
    let list_sql =
        format!("SELECT * FROM skills {where_sql} ORDER BY created_at DESC LIMIT ? OFFSET ?");

    let mut count_q = sqlx::query_as::<_, (i64,)>(&count_sql);
    if let Some(ref src) = filter.source {
        count_q = count_q.bind(src);
    }
    if let Some(ref cid) = filter.category_id {
        count_q = count_q.bind(cid);
    }
    if let Some(ref ident) = filter.identifier {
        count_q = count_q.bind(format!("%{ident}%"));
    }
    if let Some(ref n) = filter.name {
        count_q = count_q.bind(format!("%{n}%"));
    }
    if let Some(ref d) = filter.description {
        count_q = count_q.bind(format!("%{d}%"));
    }
    if let Some(ref cap) = filter.required_capabilities {
        count_q = count_q.bind(format!("\"{cap}\""));
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
        count_q = count_q
            .bind(like.clone())
            .bind(like.clone())
            .bind(like.clone())
            .bind(like);
    }
    let total = count_q
        .fetch_one(pool)
        .await
        .map(|(c,)| c)
        .map_err(|e| AppError::Internal(format!("skill count: {e}")))?;

    let mut list_q = sqlx::query_as::<_, Skill>(&list_sql);
    if let Some(ref src) = filter.source {
        list_q = list_q.bind(src);
    }
    if let Some(ref cid) = filter.category_id {
        list_q = list_q.bind(cid);
    }
    if let Some(ref ident) = filter.identifier {
        list_q = list_q.bind(format!("%{ident}%"));
    }
    if let Some(ref n) = filter.name {
        list_q = list_q.bind(format!("%{n}%"));
    }
    if let Some(ref d) = filter.description {
        list_q = list_q.bind(format!("%{d}%"));
    }
    if let Some(ref cap) = filter.required_capabilities {
        list_q = list_q.bind(format!("\"{cap}\""));
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
        list_q = list_q
            .bind(like.clone())
            .bind(like.clone())
            .bind(like.clone())
            .bind(like);
    }
    let items = list_q
        .bind(filter.limit)
        .bind(filter.offset)
        .fetch_all(pool)
        .await
        .map_err(|e| AppError::Internal(format!("skill list: {e}")))?;

    Ok(SkillList {
        items,
        total,
        offset: filter.offset,
        limit: filter.limit,
    })
}

pub async fn update(pool: &MySqlPool, id: i64, meta: UpdateMeta) -> Result<Skill, AppError> {
    let existing = fetch_by_id(pool, id).await?;
    if existing.source == "builtin" {
        return Err(AppError::BuiltinSkillProtected(format!(
            "内置技能「{}」不可修改",
            existing.identifier
        )));
    }
    if let Some(ref c) = meta.content {
        validate_content(c)?;
    }

    crate::services::optimistic_lock::check_and_bump(pool, "skills", id, meta.updated_at).await?;

    sqlx::query(
        r#"UPDATE skills SET
              name = COALESCE(?, name),
              description = COALESCE(?, description),
              frontmatter = COALESCE(?, frontmatter),
              content = COALESCE(?, content),
              is_always = COALESCE(?, is_always),
              category_id = COALESCE(?, category_id),
              required_capabilities = COALESCE(?, required_capabilities)
           WHERE id = ?"#,
    )
    .bind(&meta.name)
    .bind(&meta.description)
    .bind(&meta.frontmatter)
    .bind(&meta.content)
    .bind(meta.is_always.map(|v| v as i8))
    .bind(meta.category_id)
    .bind(meta.required_capabilities.as_ref().map(|c| serde_json::to_value(c).unwrap_or(Value::Array(vec![]))))
    .bind(id)
    .execute(pool)
    .await
    .map_err(|e| AppError::Internal(format!("skill update: {e}")))?;

    fetch_by_id(pool, id).await
}

pub async fn delete(pool: &MySqlPool, id: i64) -> Result<(), AppError> {
    let existing = fetch_by_id(pool, id).await?;
    if existing.source == "builtin" {
        return Err(AppError::BuiltinSkillProtected(format!(
            "内置技能「{}」不可删除",
            existing.identifier
        )));
    }
    let mut tx = pool
        .begin()
        .await
        .map_err(|e| AppError::Internal(format!("tx begin: {e}")))?;
    sqlx::query("SELECT id FROM skills WHERE id = ? FOR UPDATE")
        .bind(id)
        .fetch_one(&mut *tx)
        .await
        .map_err(|e| AppError::Internal(format!("skill lock: {e}")))?;
    let cnt: (i64,) = sqlx::query_as("SELECT COUNT(*) FROM agent_skills WHERE skill_id = ?")
        .bind(id)
        .fetch_one(&mut *tx)
        .await
        .map_err(|e| AppError::Internal(format!("skill ref count: {e}")))?;
    if cnt.0 > 0 {
        return Err(AppError::ResourceInUse(format!(
            "skill 被 {} 个 agent 引用，无法删除",
            cnt.0
        )));
    }
    sqlx::query("DELETE FROM skills WHERE id = ?")
        .bind(id)
        .execute(&mut *tx)
        .await
        .map_err(|e| AppError::Internal(format!("skill delete: {e}")))?;
    tx.commit()
        .await
        .map_err(|e| AppError::Internal(format!("tx commit: {e}")))?;
    Ok(())
}
