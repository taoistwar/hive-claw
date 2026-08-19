//! Plugin service (T070 / US1)
//!
//! 实现：
//! - WASM 上传：magic bytes + 大小 + sha256 + S3 put + DB insert
//! - 元数据更新（乐观锁）
//! - 软删除（事务 + FOR UPDATE 行锁 + 引用检查，spec SC-009 race window）
//! - 三维检索（FULLTEXT + category + tags）

use aws_sdk_s3::Client as S3Client;
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use sha2::{Digest, Sha256};
use sqlx::MySqlPool;

use crate::db::sql_safety::audit_sql;
use crate::models::Plugin;
use crate::runtime::scan_wasm_imports;
use crate::services::optimistic_lock::OptimisticLockTable;
use crate::storage::s3;
use crate::utils::error::AppError;

const WASM_MAGIC: &[u8; 4] = b"\0asm";

pub fn plugin_max_bytes() -> usize {
    std::env::var("PLUGIN_MAX_BYTES")
        .ok()
        .and_then(|v| v.parse().ok())
        .unwrap_or(16 * 1024 * 1024)
}

#[derive(Debug, Deserialize)]
pub struct UploadMeta {
    pub identifier: String,
    pub name: String,
    pub version: String,
    #[serde(default)]
    pub description: Option<String>,
    #[serde(default)]
    pub manifest: Option<Value>,
    #[serde(default = "default_runtime")]
    pub runtime: String,
    #[serde(default)]
    pub author: Option<String>,
    #[serde(default)]
    pub repository_url: Option<String>,
    #[serde(default)]
    pub category_id: Option<i64>,
    #[serde(default)]
    pub tag_ids: Vec<i64>,
}

fn default_runtime() -> String {
    "extism".into()
}

#[derive(Debug, Deserialize)]
pub struct UpdateMeta {
    pub name: Option<String>,
    pub description: Option<String>,
    pub author: Option<String>,
    pub repository_url: Option<String>,
    pub category_id: Option<i64>,
    pub tag_ids: Option<Vec<i64>>,
    /// 乐观锁：client 必须携带 GET 时拿到的 updated_at
    pub updated_at: DateTime<Utc>,
}

#[derive(Debug, Serialize)]
pub struct PluginListItem {
    #[serde(flatten)]
    pub plugin: Plugin,
    pub tags: Vec<TagSummary>,
}

#[derive(Debug, Serialize, sqlx::FromRow)]
pub struct TagSummary {
    pub id: i64,
    pub name: String,
    pub color: Option<String>,
}

#[derive(Debug, Serialize)]
pub struct PluginList {
    pub items: Vec<PluginListItem>,
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
    pub tag_ids: Vec<i64>,
    pub deleted_only: bool,
    pub identifier: Option<String>,
    pub name: Option<String>,
    pub description: Option<String>,
    pub runtime: Option<String>,
    pub version: Option<String>,
    pub author: Option<String>,
    pub repository_url: Option<String>,
    pub created_at_start: Option<String>,
    pub created_at_end: Option<String>,
    pub updated_at_start: Option<String>,
    pub updated_at_end: Option<String>,
}

pub async fn upload(
    pool: &MySqlPool,
    s3_client: Option<&S3Client>,
    meta: UploadMeta,
    bytes: Vec<u8>,
) -> Result<Plugin, AppError> {
    let Some(s3_client) = s3_client else {
        return Err(AppError::PluginSystemDisabled(
            "插件系统已关闭，无法上传 Plugin".into(),
        ));
    };
    let max_bytes = plugin_max_bytes();
    if bytes.len() > max_bytes {
        return Err(AppError::BadRequest(format!(
            "WASM 文件大小 {} 超过上限 {}",
            bytes.len(),
            max_bytes
        )));
    }
    if bytes.len() < 4 || &bytes[..4] != WASM_MAGIC {
        return Err(AppError::BadRequest(
            "不是合法的 WASM 文件（magic bytes 校验失败）".into(),
        ));
    }

    scan_wasm_imports(&bytes)?;

    let mut hasher = Sha256::new();
    hasher.update(&bytes);
    let digest = hasher.finalize();
    let sha256: String = digest.iter().map(|b| format!("{b:02x}")).collect();

    // 验证 tag_id 是否存在
    for tag_id in &meta.tag_ids {
        let exists: Option<(i64,)> = sqlx::query_as("SELECT id FROM tags WHERE id = ?")
            .bind(tag_id)
            .fetch_optional(pool)
            .await
            .map_err(|e| AppError::Internal(format!("tag check: {e}")))?;
        if exists.is_none() {
            return Err(AppError::BadRequest(format!("tag_id={} 不存在", tag_id)));
        }
    }

    let s3_key = format!("plugins/{}/{}.wasm", meta.identifier, meta.version);

    let mut tx = pool
        .begin()
        .await
        .map_err(|e| AppError::Internal(format!("tx begin: {e}")))?;

    s3::put_wasm(s3_client, &s3_key, bytes.clone())
        .await
        .map_err(|e| AppError::Internal(format!("S3 upload failed: {e}")))?;

    let size_bytes = bytes.len() as i64;
    let res = sqlx::query(
        r#"INSERT INTO plugins
           (identifier, name, description, manifest, runtime, version,
            author, repository_url, s3_key, sha256, size_bytes, category_id)
           VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?)"#,
    )
    .bind(&meta.identifier)
    .bind(&meta.name)
    .bind(&meta.description)
    .bind(&meta.manifest)
    .bind(&meta.runtime)
    .bind(&meta.version)
    .bind(&meta.author)
    .bind(&meta.repository_url)
    .bind(&s3_key)
    .bind(&sha256)
    .bind(size_bytes)
    .bind(meta.category_id)
    .execute(&mut *tx)
    .await
    .map_err(|e| {
        let key = s3_key.clone();
        let c = s3_client.clone();
        tokio::task::block_in_place(|| {
            let rt = tokio::runtime::Handle::current();
            let _ = rt.block_on(s3::delete_wasm(&c, &key));
        });
        AppError::Conflict(format!(
            "Plugin {}@{} 已存在或插入失败: {}",
            meta.identifier, meta.version, e
        ))
    })?;

    let id = res.last_insert_id() as i64;

    for tag_id in &meta.tag_ids {
        sqlx::query(
            r#"INSERT INTO taggings (tag_id, entity_type, entity_id)
               VALUES (?, 'plugin', ?)"#,
        )
        .bind(tag_id)
        .bind(id)
        .execute(&mut *tx)
        .await
        .map_err(|e| AppError::Internal(format!("tag bind: {e}")))?;
    }

    tx.commit()
        .await
        .map_err(|e| AppError::Internal(format!("tx commit: {e}")))?;

    fetch_by_id(pool, id).await
}

pub async fn fetch_by_id(pool: &MySqlPool, id: i64) -> Result<Plugin, AppError> {
    let row: Option<Plugin> = sqlx::query_as("SELECT * FROM plugins WHERE id = ?")
        .bind(id)
        .fetch_optional(pool)
        .await
        .map_err(|e| AppError::Internal(format!("plugin fetch: {e}")))?;
    row.ok_or_else(|| AppError::NotFound(format!("plugin id={id} not found")))
}

pub async fn list(pool: &MySqlPool, filter: ListFilter) -> Result<PluginList, AppError> {
    let mut where_clauses: Vec<String> = Vec::new();
    let mut like_clauses: Vec<String> = Vec::new();
    if filter.deleted_only {
        where_clauses.push("p.deleted_at IS NOT NULL".into());
    } else {
        where_clauses.push("p.deleted_at IS NULL".into());
    }
    if filter.category_id.is_some() {
        where_clauses.push("p.category_id = ?".into());
    }
    if filter.identifier.is_some() {
        like_clauses.push("p.identifier LIKE ?".into());
    }
    if filter.name.is_some() {
        like_clauses.push("p.name LIKE ?".into());
    }
    if filter.description.is_some() {
        like_clauses.push("p.description LIKE ?".into());
    }
    if filter.runtime.is_some() {
        where_clauses.push("p.runtime = ?".into());
    }
    if filter.version.is_some() {
        where_clauses.push("p.version = ?".into());
    }
    if filter.author.is_some() {
        like_clauses.push("p.author LIKE ?".into());
    }
    if filter.repository_url.is_some() {
        like_clauses.push("p.repository_url LIKE ?".into());
    }
    if filter.created_at_start.is_some() {
        where_clauses.push("p.created_at >= ?".into());
    }
    if filter.created_at_end.is_some() {
        where_clauses.push("p.created_at <= ?".into());
    }
    if filter.updated_at_start.is_some() {
        where_clauses.push("p.updated_at >= ?".into());
    }
    if filter.updated_at_end.is_some() {
        where_clauses.push("p.updated_at <= ?".into());
    }
    if filter.search.is_some() {
        like_clauses.push("(p.name LIKE ? OR p.description LIKE ? OR p.identifier LIKE ?)".into());
    }
    if !filter.tag_ids.is_empty() {
        let placeholders = vec!["?"; filter.tag_ids.len()].join(",");
        where_clauses.push(format!(
            "p.id IN (SELECT entity_id FROM taggings WHERE entity_type='plugin' AND tag_id IN ({placeholders}))"
        ));
    }
    let all_clauses: Vec<String> = where_clauses.into_iter().chain(like_clauses).collect();
    let where_sql = if all_clauses.is_empty() {
        String::new()
    } else {
        format!("WHERE {}", all_clauses.join(" AND "))
    };

    let count_sql = format!("SELECT COUNT(*) FROM plugins p {where_sql}");
    let list_sql = format!(
        "SELECT p.* FROM plugins p {where_sql} ORDER BY p.created_at DESC LIMIT ? OFFSET ?"
    );

    macro_rules! bind_filter_params {
        ($q:ident, $f:expr) => {{
            let mut q = $q;
            if let Some(cid) = $f.category_id {
                q = q.bind(cid);
            }
            if let Some(ref s) = $f.identifier {
                q = q.bind(format!("%{s}%"));
            }
            if let Some(ref s) = $f.name {
                q = q.bind(format!("%{s}%"));
            }
            if let Some(ref s) = $f.description {
                q = q.bind(format!("%{s}%"));
            }
            if let Some(ref s) = $f.runtime {
                q = q.bind(s);
            }
            if let Some(ref s) = $f.version {
                q = q.bind(s);
            }
            if let Some(ref s) = $f.author {
                q = q.bind(format!("%{s}%"));
            }
            if let Some(ref s) = $f.repository_url {
                q = q.bind(format!("%{s}%"));
            }
            if let Some(ref s) = $f.created_at_start {
                q = q.bind(s);
            }
            if let Some(ref s) = $f.created_at_end {
                q = q.bind(s);
            }
            if let Some(ref s) = $f.updated_at_start {
                q = q.bind(s);
            }
            if let Some(ref s) = $f.updated_at_end {
                q = q.bind(s);
            }
            if let Some(ref s) = $f.search {
                let like = format!("%{s}%");
                q = q.bind(like.clone()).bind(like.clone()).bind(like);
            }
            for tid in &$f.tag_ids {
                q = q.bind(tid);
            }
            q
        }};
    }

    let count_sql = audit_sql(count_sql);
    let count_q = sqlx::query_as::<_, (i64,)>(count_sql);
    let total: i64 = bind_filter_params!(count_q, &filter)
        .fetch_one(pool)
        .await
        .map(|(c,)| c)
        .map_err(|e| AppError::Internal(format!("plugin count: {e}")))?;

    let list_sql = audit_sql(list_sql);
    let list_q = sqlx::query_as::<_, Plugin>(list_sql);
    let rows: Vec<Plugin> = bind_filter_params!(list_q, &filter)
        .bind(filter.limit)
        .bind(filter.offset)
        .fetch_all(pool)
        .await
        .map_err(|e| AppError::Internal(format!("plugin list: {e}")))?;

    let mut items = Vec::with_capacity(rows.len());
    for p in rows {
        let tags = fetch_tags(pool, p.id).await?;
        items.push(PluginListItem { plugin: p, tags });
    }
    Ok(PluginList {
        items,
        total,
        offset: filter.offset,
        limit: filter.limit,
    })
}

async fn fetch_tags(pool: &MySqlPool, plugin_id: i64) -> Result<Vec<TagSummary>, AppError> {
    let tags = sqlx::query_as::<_, TagSummary>(
        r#"SELECT t.id, t.name, t.color FROM tags t
           JOIN taggings tg ON tg.tag_id = t.id
           WHERE tg.entity_type = 'plugin' AND tg.entity_id = ?"#,
    )
    .bind(plugin_id)
    .fetch_all(pool)
    .await
    .map_err(|e| AppError::Internal(format!("plugin tags: {e}")))?;
    Ok(tags)
}

pub async fn update(pool: &MySqlPool, id: i64, meta: UpdateMeta) -> Result<Plugin, AppError> {
    // 1. 乐观锁
    crate::services::optimistic_lock::check_and_bump(
        pool,
        OptimisticLockTable::Plugins,
        id,
        meta.updated_at,
    )
    .await?;

    // 2. 字段更新（部分字段）
    sqlx::query(
        r#"UPDATE plugins SET
              name = COALESCE(?, name),
              description = COALESCE(?, description),
              author = COALESCE(?, author),
              repository_url = COALESCE(?, repository_url),
              category_id = COALESCE(?, category_id)
           WHERE id = ?"#,
    )
    .bind(&meta.name)
    .bind(&meta.description)
    .bind(&meta.author)
    .bind(&meta.repository_url)
    .bind(meta.category_id)
    .bind(id)
    .execute(pool)
    .await
    .map_err(|e| AppError::Internal(format!("plugin update: {e}")))?;

    // 3. tag 全替换
    if let Some(ref tags) = meta.tag_ids {
        sqlx::query("DELETE FROM taggings WHERE entity_type='plugin' AND entity_id = ?")
            .bind(id)
            .execute(pool)
            .await
            .map_err(|e| AppError::Internal(format!("tag clear: {e}")))?;
        for tid in tags {
            sqlx::query(
                "INSERT IGNORE INTO taggings (tag_id, entity_type, entity_id) VALUES (?, 'plugin', ?)"
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

/// 软删除（spec SC-009 race window 防护）：transaction + FOR UPDATE + 同事务内
/// SELECT COUNT(*) FROM functions WHERE plugin_id = ?
pub async fn soft_delete(pool: &MySqlPool, id: i64) -> Result<(), AppError> {
    let mut tx = pool
        .begin()
        .await
        .map_err(|e| AppError::Internal(format!("tx begin: {e}")))?;

    // 行锁
    let plugin: Option<(i64, Option<DateTime<Utc>>)> =
        sqlx::query_as("SELECT id, deleted_at FROM plugins WHERE id = ? FOR UPDATE")
            .bind(id)
            .fetch_optional(&mut *tx)
            .await
            .map_err(|e| AppError::Internal(format!("plugin lock: {e}")))?;

    let Some((_, deleted_at)) = plugin else {
        return Err(AppError::NotFound(format!("plugin id={id} not found")));
    };
    if deleted_at.is_some() {
        return Err(AppError::Conflict("plugin already deleted".into()));
    }

    // 引用检查（同事务，覆盖 SC-009 race window）
    let cnt: (i64,) = sqlx::query_as("SELECT COUNT(*) FROM functions WHERE plugin_id = ?")
        .bind(id)
        .fetch_one(&mut *tx)
        .await
        .map_err(|e| AppError::Internal(format!("plugin ref count: {e}")))?;

    if cnt.0 > 0 {
        return Err(AppError::ResourceInUse(format!(
            "plugin 被 {} 个 function 引用，无法删除",
            cnt.0
        )));
    }

    sqlx::query("UPDATE plugins SET deleted_at = NOW() WHERE id = ?")
        .bind(id)
        .execute(&mut *tx)
        .await
        .map_err(|e| AppError::Internal(format!("plugin soft delete: {e}")))?;

    tx.commit()
        .await
        .map_err(|e| AppError::Internal(format!("tx commit: {e}")))?;

    Ok(())
}
