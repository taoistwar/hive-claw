use anyhow::Result;
use chrono::Utc;
use serde::{Deserialize, Serialize};
use sqlx::{Pool, Row, Sqlite};
use std::time::{Duration, Instant};
use thiserror::Error;
use uuid::Uuid;

use super::function_store::{FunctionInput, FunctionKind, FunctionStore};
use super::plugin_artifacts::{GcState, OperationKind, OperationState, derive_staging_name};
use super::query_count::QueryCountObserver;
use super::query_plan::{
    AGENT_FTS_COUNT_SQL, AGENT_FTS_LIST_SQL, AGENT_MODEL_PRESET_EXISTS_SQL,
    AGENT_RESOURCE_BASE_SQL, AGENT_RESOURCE_CAPABILITIES_SQL, AGENT_RESOURCE_SKILLS_SQL,
    AGENT_RESOURCE_TOOLS_SQL, AGENT_SHORT_GRAM_COUNT_SQL, AGENT_SHORT_GRAM_LIST_SQL,
    AGENT_UNFILTERED_COUNT_SQL, AGENT_UNFILTERED_LIST_SQL,
};
use super::search_index::{
    IndexBackend, IndexSelection, canonical_normalizer, delete_entity_search_documents,
    fts_literal_phrase, normalize_search_query, replace_entity_search_documents,
};
use super::store::Store;

/// FR-025: 自动重试机制 - 指数退避重试临时性错误
/// 重试策略：最多 3 次，间隔 1s, 2s, 4s
/// 适用错误：数据库锁定、文件占用等临时性错误
async fn retry_with_backoff<F, Fut, T>(mut operation: F, operation_name: &str) -> Result<T>
where
    F: FnMut() -> Fut,
    Fut: std::future::Future<Output = Result<T>>,
{
    let max_retries = 3;
    let mut attempt = 0;

    loop {
        match operation().await {
            Ok(result) => return Ok(result),
            Err(e) => {
                let error_msg = e.to_string();
                let is_temporary = error_msg.contains("database is locked")
                    || error_msg.contains("file is busy")
                    || error_msg.contains("SQLITE_BUSY");

                if is_temporary && attempt < max_retries {
                    let delay_secs = 1u64 << attempt; // 1, 2, 4 秒
                    let delay = Duration::from_secs(delay_secs);

                    tracing::warn!(
                        operation = operation_name,
                        attempt = attempt + 1,
                        max_retries = max_retries,
                        delay_secs = delay_secs,
                        error = %e,
                        "临时性错误，正在重试"
                    );

                    tokio::time::sleep(delay).await;
                    attempt += 1;
                } else {
                    tracing::error!(
                        operation = operation_name,
                        attempt = attempt,
                        error = %e,
                        "操作失败，已达到最大重试次数或非临时性错误"
                    );
                    return Err(e);
                }
            }
        }
    }
}

/// FR-025: 手动恢复功能 - 从备份文件恢复数据
pub async fn restore_from_backup(pool: &Pool<Sqlite>, backup_path: &str) -> Result<()> {
    let start = Instant::now();

    // 验证备份文件存在
    if !std::path::Path::new(backup_path).exists() {
        return Err(anyhow::anyhow!("备份文件不存在: {}", backup_path));
    }

    // 读取备份文件内容
    let backup_content = tokio::fs::read_to_string(backup_path).await?;

    // 验证 JSON 格式
    let backup_data: serde_json::Value = serde_json::from_str(&backup_content)
        .map_err(|e| anyhow::anyhow!("无效的备份文件格式: {}", e))?;

    // 验证必需字段
    if !backup_data.is_object() {
        return Err(anyhow::anyhow!("备份文件格式错误：根对象必须是对象"));
    }

    let entities = backup_data
        .get("entities")
        .ok_or_else(|| anyhow::anyhow!("备份文件缺少 entities 字段"))?;

    if !entities.is_object() {
        return Err(anyhow::anyhow!("备份文件格式错误：entities 必须是对象"));
    }

    // 创建当前数据库的临时备份
    let temp_backup_path = format!("{}.pre-restore", backup_path);
    export_all_data(pool, &temp_backup_path).await?;

    tracing::info!(temp_backup = %temp_backup_path, "已创建恢复前备份");

    // 开始事务恢复
    let mut tx = pool.begin().await?;

    // 按依赖顺序恢复数据
    // 1. 恢复 tags
    if let Some(tags) = entities.get("tags").and_then(|v| v.as_array()) {
        sqlx::query("DELETE FROM tags").execute(&mut *tx).await?;
        for tag in tags {
            let id = tag.get("id").and_then(|v| v.as_i64()).unwrap_or(0);
            let name = tag.get("name").and_then(|v| v.as_str()).unwrap_or("");
            let color = tag.get("color").and_then(|v| v.as_str());
            let created_at = tag.get("created_at").and_then(|v| v.as_str()).unwrap_or("");

            sqlx::query("INSERT INTO tags (id, name, color, created_at) VALUES (?, ?, ?, ?)")
                .bind(id)
                .bind(name)
                .bind(color)
                .bind(created_at)
                .execute(&mut *tx)
                .await?;
        }
        tracing::info!(count = tags.len(), "已恢复 tags");
    }

    // 2. 恢复 categories
    if let Some(categories) = entities.get("categories").and_then(|v| v.as_array()) {
        sqlx::query("DELETE FROM categories")
            .execute(&mut *tx)
            .await?;
        for cat in categories {
            let id = cat.get("id").and_then(|v| v.as_i64()).unwrap_or(0);
            let parent_id = cat.get("parent_id").and_then(|v| v.as_i64());
            let name = cat.get("name").and_then(|v| v.as_str()).unwrap_or("");
            let slug = cat.get("slug").and_then(|v| v.as_str()).unwrap_or("");
            let description = cat.get("description").and_then(|v| v.as_str());
            let created_at = cat.get("created_at").and_then(|v| v.as_str()).unwrap_or("");
            let updated_at = cat.get("updated_at").and_then(|v| v.as_str()).unwrap_or("");

            sqlx::query("INSERT INTO categories (id, parent_id, name, slug, description, created_at, updated_at) VALUES (?, ?, ?, ?, ?, ?, ?)")
                .bind(id)
                .bind(parent_id)
                .bind(name)
                .bind(slug)
                .bind(description)
                .bind(created_at)
                .bind(updated_at)
                .execute(&mut *tx)
                .await?;
        }
        tracing::info!(count = categories.len(), "已恢复 categories");
    }

    // 3. 恢复 capabilities
    if let Some(caps) = entities.get("capabilities").and_then(|v| v.as_array()) {
        sqlx::query("DELETE FROM capabilities")
            .execute(&mut *tx)
            .await?;
        for cap in caps {
            let name = cap.get("name").and_then(|v| v.as_str()).unwrap_or("");
            let description = cap
                .get("description")
                .and_then(|v| v.as_str())
                .unwrap_or("");
            let is_dangerous = cap
                .get("is_dangerous")
                .and_then(|v| v.as_i64())
                .unwrap_or(0);
            let category_id = cap.get("category_id").and_then(|v| v.as_i64());
            let created_at = cap.get("created_at").and_then(|v| v.as_str()).unwrap_or("");

            sqlx::query("INSERT INTO capabilities (name, description, is_dangerous, category_id, created_at) VALUES (?, ?, ?, ?, ?)")
                .bind(name)
                .bind(description)
                .bind(is_dangerous)
                .bind(category_id)
                .bind(created_at)
                .execute(&mut *tx)
                .await?;
        }
        tracing::info!(count = caps.len(), "已恢复 capabilities");
    }

    // 4. 恢复 plugins
    if let Some(plugins) = entities.get("plugins").and_then(|v| v.as_array()) {
        sqlx::query("DELETE FROM plugins").execute(&mut *tx).await?;
        for plugin in plugins {
            let id = plugin.get("id").and_then(|v| v.as_i64()).unwrap_or(0);
            let identifier = plugin
                .get("identifier")
                .and_then(|v| v.as_str())
                .unwrap_or("");
            let name = plugin.get("name").and_then(|v| v.as_str()).unwrap_or("");
            let description = plugin.get("description").and_then(|v| v.as_str());
            let manifest = plugin.get("manifest").and_then(|v| v.as_str());
            let runtime = plugin.get("runtime").and_then(|v| v.as_str()).unwrap_or("");
            let version = plugin.get("version").and_then(|v| v.as_str()).unwrap_or("");
            let author = plugin.get("author").and_then(|v| v.as_str());
            let repository_url = plugin.get("repository_url").and_then(|v| v.as_str());
            let s3_key = plugin.get("s3_key").and_then(|v| v.as_str()).unwrap_or("");
            let sha256 = plugin.get("sha256").and_then(|v| v.as_str()).unwrap_or("");
            let size_bytes = plugin
                .get("size_bytes")
                .and_then(|v| v.as_i64())
                .unwrap_or(0);
            let category_id = plugin.get("category_id").and_then(|v| v.as_i64());
            let created_at = plugin
                .get("created_at")
                .and_then(|v| v.as_str())
                .unwrap_or("");
            let updated_at = plugin
                .get("updated_at")
                .and_then(|v| v.as_str())
                .unwrap_or("");
            let deleted_at = plugin.get("deleted_at").and_then(|v| v.as_str());

            sqlx::query("INSERT INTO plugins (id, identifier, name, description, manifest, runtime, version, author, repository_url, s3_key, sha256, size_bytes, category_id, created_at, updated_at, deleted_at) VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?)")
                .bind(id)
                .bind(identifier)
                .bind(name)
                .bind(description)
                .bind(manifest)
                .bind(runtime)
                .bind(version)
                .bind(author)
                .bind(repository_url)
                .bind(s3_key)
                .bind(sha256)
                .bind(size_bytes)
                .bind(category_id)
                .bind(created_at)
                .bind(updated_at)
                .bind(deleted_at)
                .execute(&mut *tx)
                .await?;
        }
        tracing::info!(count = plugins.len(), "已恢复 plugins");
    }

    // 5. 恢复 functions
    if let Some(functions) = entities.get("functions").and_then(|v| v.as_array()) {
        sqlx::query("DELETE FROM functions")
            .execute(&mut *tx)
            .await?;
        for func in functions {
            let id = func.get("id").and_then(|v| v.as_i64()).unwrap_or(0);
            let identifier = func
                .get("identifier")
                .and_then(|v| v.as_str())
                .unwrap_or("");
            let name = func.get("name").and_then(|v| v.as_str()).unwrap_or("");
            let description = func.get("description").and_then(|v| v.as_str());
            let kind = func
                .get("kind")
                .and_then(|v| v.as_str())
                .unwrap_or("builtin")
                .to_string();
            let input_schema = func
                .get("input_schema")
                .and_then(|v| v.as_str())
                .unwrap_or("{}");
            let output_schema = func
                .get("output_schema")
                .and_then(|v| v.as_str())
                .unwrap_or("{}");
            let plugin_id = func.get("plugin_id").and_then(|v| v.as_i64());
            let plugin_export = func.get("plugin_export").and_then(|v| v.as_str());
            let category_id = func.get("category_id").and_then(|v| v.as_i64());
            let required_capabilities = func.get("required_capabilities").and_then(|v| v.as_str());
            let created_at = func
                .get("created_at")
                .and_then(|v| v.as_str())
                .unwrap_or("");
            let updated_at = func
                .get("updated_at")
                .and_then(|v| v.as_str())
                .unwrap_or("");

            sqlx::query("INSERT INTO functions (id, identifier, name, description, kind, input_schema, output_schema, plugin_id, plugin_export, category_id, required_capabilities, created_at, updated_at) VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?)")
                .bind(id)
                .bind(identifier)
                .bind(name)
                .bind(description)
                .bind(kind)
                .bind(input_schema)
                .bind(output_schema)
                .bind(plugin_id)
                .bind(plugin_export)
                .bind(category_id)
                .bind(required_capabilities)
                .bind(created_at)
                .bind(updated_at)
                .execute(&mut *tx)
                .await?;
        }
        tracing::info!(count = functions.len(), "已恢复 functions");
    }

    // 6. 恢复 workflows
    if let Some(workflows) = entities.get("workflows").and_then(|v| v.as_array()) {
        sqlx::query("DELETE FROM workflows")
            .execute(&mut *tx)
            .await?;
        for wf in workflows {
            let id = wf.get("id").and_then(|v| v.as_i64()).unwrap_or(0);
            let identifier = wf.get("identifier").and_then(|v| v.as_str()).unwrap_or("");
            let name = wf.get("name").and_then(|v| v.as_str()).unwrap_or("");
            let description = wf.get("description").and_then(|v| v.as_str());
            let timeout_ms = wf
                .get("timeout_ms")
                .and_then(|v| v.as_i64())
                .unwrap_or(30000);
            let category_id = wf.get("category_id").and_then(|v| v.as_i64());
            let input_schema = wf.get("input_schema").and_then(|v| v.as_str());
            let start_description = wf.get("start_description").and_then(|v| v.as_str());
            let output_schema = wf.get("output_schema").and_then(|v| v.as_str());
            let required_capabilities = wf.get("required_capabilities").and_then(|v| v.as_str());
            let created_at = wf.get("created_at").and_then(|v| v.as_str()).unwrap_or("");
            let updated_at = wf.get("updated_at").and_then(|v| v.as_str()).unwrap_or("");

            sqlx::query("INSERT INTO workflows (id, identifier, name, description, timeout_ms, category_id, input_schema, start_description, output_schema, required_capabilities, created_at, updated_at) VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?)")
                .bind(id)
                .bind(identifier)
                .bind(name)
                .bind(description)
                .bind(timeout_ms)
                .bind(category_id)
                .bind(input_schema)
                .bind(start_description)
                .bind(output_schema)
                .bind(required_capabilities)
                .bind(created_at)
                .bind(updated_at)
                .execute(&mut *tx)
                .await?;
        }
        tracing::info!(count = workflows.len(), "已恢复 workflows");
    }

    // 7. 恢复 tools
    if let Some(tools) = entities.get("tools").and_then(|v| v.as_array()) {
        sqlx::query("DELETE FROM tools").execute(&mut *tx).await?;
        for tool in tools {
            let id = tool.get("id").and_then(|v| v.as_i64()).unwrap_or(0);
            let identifier = tool
                .get("identifier")
                .and_then(|v| v.as_str())
                .unwrap_or("");
            let name = tool.get("name").and_then(|v| v.as_str()).unwrap_or("");
            let description = tool
                .get("description")
                .and_then(|v| v.as_str())
                .unwrap_or("");
            let kind = tool
                .get("kind")
                .and_then(|v| v.as_str())
                .unwrap_or("function-wrap")
                .to_string();
            let source = tool
                .get("source")
                .and_then(|v| v.as_str())
                .unwrap_or("workspace");
            let is_always = tool.get("is_always").and_then(|v| v.as_i64()).unwrap_or(0);
            let function_id = tool.get("function_id").and_then(|v| v.as_i64());
            let workflow_id = tool.get("workflow_id").and_then(|v| v.as_i64());
            let input_schema = tool
                .get("input_schema")
                .and_then(|v| v.as_str())
                .unwrap_or("{}");
            let output_schema = tool
                .get("output_schema")
                .and_then(|v| v.as_str())
                .unwrap_or("{}");
            let category_id = tool.get("category_id").and_then(|v| v.as_i64());
            let required_capabilities = tool.get("required_capabilities").and_then(|v| v.as_str());
            let created_at = tool
                .get("created_at")
                .and_then(|v| v.as_str())
                .unwrap_or("");
            let updated_at = tool
                .get("updated_at")
                .and_then(|v| v.as_str())
                .unwrap_or("");

            sqlx::query("INSERT INTO tools (id, identifier, name, description, kind, source, is_always, function_id, workflow_id, input_schema, output_schema, category_id, required_capabilities, created_at, updated_at) VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?)")
                .bind(id)
                .bind(identifier)
                .bind(name)
                .bind(description)
                .bind(kind)
                .bind(source)
                .bind(is_always)
                .bind(function_id)
                .bind(workflow_id)
                .bind(input_schema)
                .bind(output_schema)
                .bind(category_id)
                .bind(required_capabilities)
                .bind(created_at)
                .bind(updated_at)
                .execute(&mut *tx)
                .await?;
        }
        tracing::info!(count = tools.len(), "已恢复 tools");
    }

    // 8. 恢复 skills
    if let Some(skills) = entities.get("skills").and_then(|v| v.as_array()) {
        sqlx::query("DELETE FROM skills").execute(&mut *tx).await?;
        for skill in skills {
            let id = skill.get("id").and_then(|v| v.as_i64()).unwrap_or(0);
            let identifier = skill
                .get("identifier")
                .and_then(|v| v.as_str())
                .unwrap_or("");
            let name = skill.get("name").and_then(|v| v.as_str()).unwrap_or("");
            let description = skill
                .get("description")
                .and_then(|v| v.as_str())
                .unwrap_or("");
            let frontmatter = skill.get("frontmatter").and_then(|v| v.as_str());
            let content = skill.get("content").and_then(|v| v.as_str()).unwrap_or("");
            let source = skill
                .get("source")
                .and_then(|v| v.as_str())
                .unwrap_or("workspace");
            let is_always = skill.get("is_always").and_then(|v| v.as_i64()).unwrap_or(0);
            let category_id = skill.get("category_id").and_then(|v| v.as_i64());
            let required_capabilities = skill.get("required_capabilities").and_then(|v| v.as_str());
            let created_at = skill
                .get("created_at")
                .and_then(|v| v.as_str())
                .unwrap_or("");
            let updated_at = skill
                .get("updated_at")
                .and_then(|v| v.as_str())
                .unwrap_or("");

            sqlx::query("INSERT INTO skills (id, identifier, name, description, frontmatter, content, source, is_always, category_id, required_capabilities, created_at, updated_at) VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?)")
                .bind(id)
                .bind(identifier)
                .bind(name)
                .bind(description)
                .bind(frontmatter)
                .bind(content)
                .bind(source)
                .bind(is_always)
                .bind(category_id)
                .bind(required_capabilities)
                .bind(created_at)
                .bind(updated_at)
                .execute(&mut *tx)
                .await?;
        }
        tracing::info!(count = skills.len(), "已恢复 skills");
    }

    // 9. 恢复 agents
    if let Some(agents) = entities.get("agents").and_then(|v| v.as_array()) {
        sqlx::query("DELETE FROM agents").execute(&mut *tx).await?;
        for agent in agents {
            let id = agent.get("id").and_then(|v| v.as_i64()).unwrap_or(0);
            let identifier = agent
                .get("identifier")
                .and_then(|v| v.as_str())
                .unwrap_or("");
            let name = agent.get("name").and_then(|v| v.as_str()).unwrap_or("");
            let description = agent.get("description").and_then(|v| v.as_str());
            let system_prompt = agent
                .get("system_prompt")
                .and_then(|v| v.as_str())
                .unwrap_or("");
            let parent_agent_id = agent.get("parent_agent_id").and_then(|v| v.as_i64());
            let depth = agent.get("depth").and_then(|v| v.as_i64()).unwrap_or(0);
            let is_default = agent
                .get("is_default")
                .and_then(|v| v.as_i64())
                .unwrap_or(0);
            let model_preset = agent.get("model_preset").and_then(|v| v.as_str());
            let name_normalized = agent
                .get("name_normalized")
                .and_then(|v| v.as_str())
                .map(str::to_owned)
                .unwrap_or_else(|| name.to_lowercase());
            let created_at = agent
                .get("created_at")
                .and_then(|v| v.as_str())
                .unwrap_or("");
            let updated_at = agent
                .get("updated_at")
                .and_then(|v| v.as_str())
                .unwrap_or("");

            sqlx::query("INSERT INTO agents (id, identifier, name, description, system_prompt, parent_agent_id, depth, is_default, model_preset, name_normalized, created_at, updated_at) VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?)")
                .bind(id)
                .bind(identifier)
                .bind(name)
                .bind(description)
                .bind(system_prompt)
                .bind(parent_agent_id)
                .bind(depth)
                .bind(is_default)
                .bind(model_preset)
                .bind(name_normalized)
                .bind(created_at)
                .bind(updated_at)
                .execute(&mut *tx)
                .await?;
        }
        tracing::info!(count = agents.len(), "已恢复 agents");
    }

    tx.commit().await?;

    let duration = start.elapsed().as_millis();
    tracing::info!(duration_ms = duration, "数据恢复完成");

    Ok(())
}

// 辅助函数：检测 UNIQUE 约束违规并返回友好错误消息
fn handle_unique_constraint_error(
    err: sqlx::Error,
    field_name: &str,
    field_value: &str,
) -> anyhow::Error {
    if let sqlx::Error::Database(db_err) = &err
        && db_err.message().contains("UNIQUE constraint failed")
    {
        tracing::warn!(
            entity_type = "unknown",
            field = field_name,
            value = field_value,
            "UNIQUE constraint violation"
        );
        return anyhow::anyhow!("{} '{}' 已存在，请使用其他值", field_name, field_value);
    }
    err.into()
}

// 字段验证函数
fn validate_identifier(identifier: &str) -> Result<()> {
    if identifier.trim().is_empty() {
        return Err(anyhow::anyhow!("identifier 不能为空"));
    }
    if identifier.len() > 255 {
        return Err(anyhow::anyhow!("identifier 长度不能超过 255 字符"));
    }
    if !identifier
        .chars()
        .all(|c| c.is_ascii_alphanumeric() || c == '_' || c == '-')
    {
        return Err(anyhow::anyhow!(
            "identifier 只能包含字母、数字、下划线和连字符"
        ));
    }
    Ok(())
}

fn validate_slug(slug: &str) -> Result<()> {
    if slug.trim().is_empty() {
        return Err(anyhow::anyhow!("slug 不能为空"));
    }
    if slug.len() > 255 {
        return Err(anyhow::anyhow!("slug 长度不能超过 255 字符"));
    }
    if !slug
        .chars()
        .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '-')
    {
        return Err(anyhow::anyhow!("slug 只能包含小写字母、数字和连字符"));
    }
    Ok(())
}

fn validate_name(name: &str) -> Result<()> {
    if name.trim().is_empty() {
        return Err(anyhow::anyhow!("name 不能为空"));
    }
    if name.len() > 255 {
        return Err(anyhow::anyhow!("name 长度不能超过 255 字符"));
    }
    Ok(())
}

fn validate_description(description: &str) -> Result<()> {
    if description.len() > 2000 {
        return Err(anyhow::anyhow!("description 长度不能超过 2000 字符"));
    }
    Ok(())
}

fn validate_content(content: &str) -> Result<()> {
    if content.trim().is_empty() {
        return Err(anyhow::anyhow!("content 不能为空"));
    }
    if content.len() > 1024 * 1024 {
        return Err(anyhow::anyhow!("content 不能超过 1MB"));
    }
    Ok(())
}

fn validate_json(json: &str) -> Result<()> {
    if json.len() > 1024 * 1024 {
        return Err(anyhow::anyhow!("JSON 内容不能超过 1MB"));
    }
    // 尝试解析为 JSON
    if let Err(e) = serde_json::from_str::<serde_json::Value>(json) {
        return Err(anyhow::anyhow!("无效的 JSON 格式: {}", e));
    }
    Ok(())
}

fn validate_color(color: &str) -> Result<()> {
    if color.len() != 7 {
        return Err(anyhow::anyhow!("颜色格式必须是 #RRGGBB"));
    }
    if !color.starts_with('#') {
        return Err(anyhow::anyhow!("颜色必须以 # 开头"));
    }
    if !color[1..].chars().all(|c| c.is_ascii_hexdigit()) {
        return Err(anyhow::anyhow!("颜色只能包含十六进制字符 (0-9, A-F)"));
    }
    Ok(())
}

/// Normalize a tag name for the `tags.normalized_name` index.
/// Mirrors the pipeline in `tag_store::normalize` (NFKC + lower
/// case) but is duplicated here so the runtime `Tag` CRUD stays
/// independent of `tag_store` (the runtime store is the legacy
/// synchronous path; the typed `TagStore` is the new contract).
fn tag_normalized_name(name: &str) -> String {
    use unicode_normalization::UnicodeNormalization;
    let nfkc: String = name.nfkc().collect();
    let nfc: String = nfkc.nfc().collect();
    nfc.to_lowercase()
}

// Tag 数据结构
#[derive(Debug, Clone)]
pub struct Tag {
    pub id: i64,
    pub name: String,
    pub color: Option<String>,
    pub created_at: String,
}

// Category 数据结构
#[derive(Debug, Clone)]
pub struct Category {
    pub id: i64,
    pub parent_id: Option<i64>,
    pub name: String,
    pub slug: String,
    pub description: Option<String>,
    pub created_at: String,
    pub updated_at: String,
}

#[derive(Debug, Clone)]
pub struct Capability {
    pub name: String,
    pub description: String,
    pub is_dangerous: bool,
    pub category_id: Option<i64>,
    pub created_at: String,
}

#[derive(Debug, Clone)]
pub struct Plugin {
    pub id: i64,
    pub identifier: String,
    pub name: String,
    pub description: Option<String>,
    pub manifest: Option<String>,
    pub runtime: String,
    pub version: String,
    pub author: Option<String>,
    pub repository_url: Option<String>,
    pub s3_key: String,
    pub sha256: String,
    pub size_bytes: i64,
    pub category_id: Option<i64>,
    pub capabilities: String,
    pub resource_limits: String,
    pub row_revision: i64,
    pub created_at: String,
    pub updated_at: String,
    pub deleted_at: Option<String>,
}

/// Optional per-plugin execution limits persisted as a JSON object.
///
/// Missing fields select the runtime defaults. Present fields are validated at
/// the Store boundary before any row is written.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct PluginResourceLimits {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub timeout_ms: Option<i64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub memory_limit_mb: Option<i64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub output_limit_bytes: Option<i64>,
}

/// Immutable tuple for a newly published plugin artifact.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NewArtifact {
    pub s3_key: String,
    pub sha256: String,
    pub size_bytes: i64,
    pub resource_limits: PluginResourceLimits,
}

/// Exact old Plugin tuple used by the original online replace CAS.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ExpectedPluginRevision {
    pub plugin_id: i64,
    pub identifier: String,
    pub version: String,
    pub s3_key: String,
    pub sha256: String,
    pub size_bytes: i64,
    pub identity: String,
    pub resource_limits: PluginResourceLimits,
    pub row_revision: i64,
}

/// Typed input for creating a durable plugin artifact operation.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PreparePluginOperation {
    Create {
        target_identifier: String,
        new: NewArtifact,
    },
    Replace {
        expected_old: ExpectedPluginRevision,
        new: NewArtifact,
    },
}

/// Legal state transitions which do not themselves create user rows or GC
/// ownership records.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum OperationTransition {
    Staged { staging_identity: String },
    Published { new_identity: String },
    Done,
    Conflict,
}

/// An artifact owned by an operation and therefore eligible for derived GC.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum OperationGcTarget {
    PublishedNew,
    ExpectedOld,
}

/// User-visible fields committed only after a create operation is published.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CreatePluginFields {
    pub version: String,
    pub name: String,
    pub description: Option<String>,
    pub manifest: Option<String>,
    pub runtime: String,
    pub author: Option<String>,
    pub repository_url: Option<String>,
    pub category_id: Option<i64>,
    pub capabilities: String,
}

/// Stable result of the original online replace CAS.
#[derive(Debug, Clone)]
pub enum ReplaceOutcome {
    Referenced {
        plugin: Box<Plugin>,
    },
    ConcurrentConflict {
        plugin_id: i64,
        actual_row_revision: Option<i64>,
    },
}

/// Stable result of atomically deriving GC ownership and finishing an
/// operation.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FinishOutcome {
    Finished { gc_state: GcState },
    AlreadyDone { gc_state: GcState },
}

/// Typed representation of one row in `plugin_artifact_operations`.
#[derive(Debug, Clone)]
pub struct PluginArtifactOperation {
    operation_id: Uuid,
    kind: OperationKind,
    state: OperationState,
    target_identifier: String,
    plugin_id: Option<i64>,
    expected_old_identifier: Option<String>,
    expected_old_version: Option<String>,
    expected_old_s3_key: Option<String>,
    expected_old_sha256: Option<String>,
    expected_old_size: Option<i64>,
    expected_old_identity: Option<String>,
    expected_old_resource_limits: Option<String>,
    expected_old_row_revision: Option<i64>,
    staging_name: String,
    staging_identity: Option<String>,
    new_s3_key: String,
    new_sha256: String,
    new_size: i64,
    new_resource_limits: String,
    new_identity: Option<String>,
}

impl PluginArtifactOperation {
    pub fn operation_id(&self) -> Uuid {
        self.operation_id
    }

    pub fn kind(&self) -> OperationKind {
        self.kind
    }

    pub fn state(&self) -> OperationState {
        self.state
    }

    pub fn staging_name(&self) -> &str {
        &self.staging_name
    }

    pub fn plugin_id(&self) -> Option<i64> {
        self.plugin_id
    }

    pub(crate) fn target_identifier(&self) -> &str {
        &self.target_identifier
    }

    pub(crate) fn staging_identity(&self) -> Option<&str> {
        self.staging_identity.as_deref()
    }

    pub(crate) fn new_s3_key(&self) -> &str {
        &self.new_s3_key
    }

    pub(crate) fn new_sha256(&self) -> &str {
        &self.new_sha256
    }

    pub(crate) fn new_size_bytes(&self) -> i64 {
        self.new_size
    }

    pub(crate) fn new_resource_limits_json(&self) -> &str {
        &self.new_resource_limits
    }

    pub(crate) fn new_identity(&self) -> Option<&str> {
        self.new_identity.as_deref()
    }

    pub(crate) fn expected_old_s3_key(&self) -> Option<&str> {
        self.expected_old_s3_key.as_deref()
    }

    pub(crate) fn expected_old_sha256(&self) -> Option<&str> {
        self.expected_old_sha256.as_deref()
    }

    pub(crate) fn expected_old_size_bytes(&self) -> Option<i64> {
        self.expected_old_size
    }

    pub(crate) fn expected_old_identity(&self) -> Option<&str> {
        self.expected_old_identity.as_deref()
    }

    pub(crate) fn expected_old_row_revision(&self) -> Option<i64> {
        self.expected_old_row_revision
    }
}

/// Fail-closed errors emitted by the typed plugin artifact ledger.
#[derive(Debug, Error)]
pub enum PluginLedgerError {
    #[error("not_found: plugin artifact operation {operation_id}")]
    NotFound { operation_id: Uuid },
    #[error("kind_mismatch: operation {operation_id} is {actual:?}, expected {expected:?}")]
    KindMismatch {
        operation_id: Uuid,
        expected: OperationKind,
        actual: OperationKind,
    },
    #[error("state_conflict: operation {operation_id} is {actual:?}, expected {expected:?}")]
    StateConflict {
        operation_id: Uuid,
        expected: OperationState,
        actual: OperationState,
    },
    #[error("invalid_input: {field}: {reason}")]
    InvalidInput { field: String, reason: String },
    #[error("corrupt_ledger: {0}")]
    CorruptLedger(String),
    #[error("backend: {0}")]
    Backend(#[from] sqlx::Error),
}

/// Explicit typed boundary for the internal Plugin artifact ledger.
#[derive(Debug, Clone)]
pub struct PluginArtifactLedger {
    pool: Pool<Sqlite>,
}

#[derive(Debug, sqlx::FromRow)]
struct PluginArtifactOperationRow {
    operation_id: String,
    kind: String,
    state: String,
    target_identifier: Option<String>,
    plugin_id: Option<String>,
    expected_old_identifier: Option<String>,
    expected_old_version: Option<String>,
    expected_old_s3_key: Option<String>,
    expected_old_sha256: Option<String>,
    expected_old_size: Option<i64>,
    expected_old_identity: Option<String>,
    expected_old_resource_limits: Option<String>,
    expected_old_row_revision: Option<i64>,
    staging_name: String,
    staging_identity: Option<String>,
    new_s3_key: Option<String>,
    new_sha256: Option<String>,
    new_size: Option<i64>,
    new_resource_limits: Option<String>,
    new_identity: Option<String>,
}

#[derive(Debug, Clone)]
struct OperationGcOwnership {
    artifact_key: String,
    sha256: String,
    size_bytes: i64,
    identity: String,
    reason: &'static str,
}

#[derive(Debug, Clone)]
pub struct Function {
    pub id: i64,
    pub identifier: String,
    pub name: String,
    pub description: Option<String>,
    pub kind: String,
    pub input_schema: String,
    pub output_schema: String,
    pub plugin_id: Option<i64>,
    pub plugin_export: Option<String>,
    pub category_id: Option<i64>,
    pub required_capabilities: Option<String>,
    pub created_at: String,
    pub updated_at: String,
}

#[derive(Debug, Clone)]
pub struct Workflow {
    pub id: i64,
    pub identifier: String,
    pub name: String,
    pub description: Option<String>,
    pub timeout_ms: i64,
    pub category_id: Option<i64>,
    pub input_schema: Option<String>,
    pub start_description: Option<String>,
    pub output_schema: Option<String>,
    pub required_capabilities: Option<String>,
    pub created_at: String,
    pub updated_at: String,
}

#[derive(Debug, Clone)]
pub struct WorkflowNode {
    pub id: i64,
    pub workflow_id: i64,
    pub node_key: String,
    pub node_type: String,
    pub function_id: Option<i64>,
    pub position_x: f64,
    pub position_y: f64,
    pub node_config: Option<String>,
    pub created_at: String,
}

#[derive(Debug, Clone)]
pub struct WorkflowEdge {
    pub id: i64,
    pub workflow_id: i64,
    pub src_node_key: String,
    pub dst_node_key: String,
    pub mapping: String,
}

#[derive(Debug, Clone)]
pub struct Tool {
    pub id: i64,
    pub identifier: String,
    pub name: String,
    pub description: String,
    pub kind: String,
    pub source: String,
    pub is_always: bool,
    pub function_id: Option<i64>,
    pub workflow_id: Option<i64>,
    pub input_schema: String,
    pub output_schema: String,
    pub category_id: Option<i64>,
    pub required_capabilities: Option<String>,
    pub created_at: String,
    pub updated_at: String,
}

#[derive(Debug, Clone)]
pub struct Skill {
    pub id: i64,
    pub identifier: String,
    pub name: String,
    pub description: String,
    pub frontmatter: Option<String>,
    pub content: String,
    pub source: String,
    pub is_always: bool,
    pub category_id: Option<i64>,
    pub required_capabilities: Option<String>,
    pub created_at: String,
    pub updated_at: String,
}

#[derive(Debug, Clone)]
pub struct Agent {
    pub id: i64,
    pub identifier: String,
    pub name: String,
    pub description: Option<String>,
    pub system_prompt: String,
    pub parent_agent_id: Option<i64>,
    pub depth: i64,
    pub is_default: bool,
    pub model_preset: Option<String>,
    pub created_at: String,
    pub updated_at: String,
}

// Tag CRUD 方法
impl Tag {
    pub async fn list(
        pool: &Pool<Sqlite>,
        search: Option<String>,
        limit: i64,
        offset: i64,
    ) -> Result<Vec<Tag>> {
        let tags = if let Some(search_term) = search {
            let normalized = tag_normalized_name(&search_term);
            sqlx::query_as::<_, Tag>(
                "SELECT id, name, color, created_at FROM tags \
                 WHERE normalized_name LIKE ? \
                 ORDER BY normalized_name LIMIT ? OFFSET ?",
            )
            .bind(format!("%{}%", normalized))
            .bind(limit)
            .bind(offset)
            .fetch_all(pool)
            .await?
        } else {
            sqlx::query_as::<_, Tag>(
                "SELECT id, name, color, created_at FROM tags \
                 ORDER BY normalized_name LIMIT ? OFFSET ?",
            )
            .bind(limit)
            .bind(offset)
            .fetch_all(pool)
            .await?
        };
        Ok(tags)
    }

    pub async fn count(pool: &Pool<Sqlite>, search: Option<String>) -> Result<i64> {
        let count = if let Some(search_term) = search {
            let normalized = tag_normalized_name(&search_term);
            sqlx::query_scalar::<_, i64>("SELECT COUNT(*) FROM tags WHERE normalized_name LIKE ?")
                .bind(format!("%{}%", normalized))
                .fetch_one(pool)
                .await?
        } else {
            sqlx::query_scalar::<_, i64>("SELECT COUNT(*) FROM tags")
                .fetch_one(pool)
                .await?
        };
        Ok(count)
    }

    pub async fn get(pool: &Pool<Sqlite>, id: i64) -> Result<Option<Tag>> {
        let tag =
            sqlx::query_as::<_, Tag>("SELECT id, name, color, created_at FROM tags WHERE id = ?")
                .bind(id)
                .fetch_optional(pool)
                .await?;
        Ok(tag)
    }

    pub async fn create(pool: &Pool<Sqlite>, name: String, color: Option<String>) -> Result<Tag> {
        // 验证字段
        validate_name(&name)?;
        if let Some(ref c) = color {
            validate_color(c)?;
        }

        let start = Instant::now();
        let now = Utc::now().to_rfc3339();
        let normalized = tag_normalized_name(&name);
        let result = sqlx::query_scalar::<_, i64>(
            "INSERT INTO tags (name, color, normalized_name, created_at, updated_at) VALUES (?, ?, ?, ?, ?) RETURNING id",
        )
        .bind(&name)
        .bind(&color)
        .bind(&normalized)
        .bind(now.clone())
        .bind(&now)
        .fetch_one(pool)
        .await;

        let id = result.map_err(|e| handle_unique_constraint_error(e, "name", &name))?;

        let duration = start.elapsed().as_millis();
        tracing::info!(entity = "tag", op = "create", name = %name, id = id, duration_ms = duration, "Tag created");

        Tag::get(pool, id)
            .await?
            .ok_or_else(|| anyhow::anyhow!("Failed to retrieve created tag"))
    }

    pub async fn update(
        pool: &Pool<Sqlite>,
        id: i64,
        name: String,
        color: Option<String>,
    ) -> Result<Tag> {
        validate_name(&name)?;
        if let Some(ref c) = color {
            validate_color(c)?;
        }

        let start = Instant::now();
        let now = Utc::now().to_rfc3339();
        let normalized = tag_normalized_name(&name);
        let result = sqlx::query(
            "UPDATE tags SET name = ?, color = ?, normalized_name = ?, updated_at = ? WHERE id = ?",
        )
        .bind(&name)
        .bind(&color)
        .bind(&normalized)
        .bind(&now)
        .bind(id)
        .execute(pool)
        .await;

        result.map_err(|e| handle_unique_constraint_error(e, "name", &name))?;

        let duration = start.elapsed().as_millis();
        tracing::info!(entity = "tag", op = "update", id = id, name = %name, duration_ms = duration, "Tag updated");

        Tag::get(pool, id)
            .await?
            .ok_or_else(|| anyhow::anyhow!("Tag not found"))
    }

    pub async fn delete(pool: &Pool<Sqlite>, id: i64) -> Result<()> {
        let start = Instant::now();
        sqlx::query("DELETE FROM tags WHERE id = ?")
            .bind(id)
            .execute(pool)
            .await?;

        let duration = start.elapsed().as_millis();
        tracing::info!(
            entity = "tag",
            op = "delete",
            id = id,
            duration_ms = duration,
            "Tag deleted"
        );

        Ok(())
    }
}

// 实现 sqlx::FromRow for Tag
impl sqlx::FromRow<'_, sqlx::sqlite::SqliteRow> for Tag {
    fn from_row(row: &sqlx::sqlite::SqliteRow) -> Result<Self, sqlx::Error> {
        Ok(Tag {
            id: row.try_get("id")?,
            name: row.try_get("name")?,
            color: row.try_get("color")?,
            created_at: row.try_get("created_at")?,
        })
    }
}

// Category CRUD 方法
impl Category {
    pub async fn list(
        pool: &Pool<Sqlite>,
        search: Option<String>,
        limit: i64,
        offset: i64,
    ) -> Result<Vec<Category>> {
        let categories = if let Some(s) = search {
            sqlx::query_as::<_, Category>(
                "SELECT id, parent_id, name, slug, description, created_at, updated_at FROM categories WHERE name LIKE ? OR slug LIKE ? ORDER BY name LIMIT ? OFFSET ?"
            ).bind(format!("%{}%", s)).bind(format!("%{}%", s)).bind(limit).bind(offset).fetch_all(pool).await?
        } else {
            sqlx::query_as::<_, Category>(
                "SELECT id, parent_id, name, slug, description, created_at, updated_at FROM categories ORDER BY name LIMIT ? OFFSET ?"
            ).bind(limit).bind(offset).fetch_all(pool).await?
        };
        Ok(categories)
    }

    pub async fn count(pool: &Pool<Sqlite>, search: Option<String>) -> Result<i64> {
        let count = if let Some(s) = search {
            sqlx::query_scalar::<_, i64>(
                "SELECT COUNT(*) FROM categories WHERE name LIKE ? OR slug LIKE ?",
            )
            .bind(format!("%{}%", s))
            .bind(format!("%{}%", s))
            .fetch_one(pool)
            .await?
        } else {
            sqlx::query_scalar::<_, i64>("SELECT COUNT(*) FROM categories")
                .fetch_one(pool)
                .await?
        };
        Ok(count)
    }

    pub async fn get(pool: &Pool<Sqlite>, id: i64) -> Result<Option<Category>> {
        let category = sqlx::query_as::<_, Category>(
            "SELECT id, parent_id, name, slug, description, created_at, updated_at FROM categories WHERE id = ?"
        )
        .bind(id)
        .fetch_optional(pool)
        .await?;
        Ok(category)
    }

    pub async fn create(
        pool: &Pool<Sqlite>,
        parent_id: Option<i64>,
        name: String,
        slug: String,
        description: Option<String>,
    ) -> Result<Category> {
        // 验证字段
        validate_name(&name)?;
        validate_slug(&slug)?;
        if let Some(ref desc) = description {
            validate_description(desc)?;
        }

        let start = Instant::now();
        let now = Utc::now().to_rfc3339();
        let result = sqlx::query_scalar::<_, i64>(
            "INSERT INTO categories (parent_id, name, slug, description, created_at, updated_at) VALUES (?, ?, ?, ?, ?, ?) RETURNING id"
        )
        .bind(parent_id)
        .bind(&name)
        .bind(&slug)
        .bind(&description)
        .bind(&now)
        .bind(&now)
        .fetch_one(pool)
        .await;

        let id = result.map_err(|e| handle_unique_constraint_error(e, "slug", &slug))?;

        // 创建后检测循环引用
        Self::detect_cycle(pool, id, parent_id).await?;

        let duration = start.elapsed().as_millis();
        tracing::info!(entity = "category", op = "create", id = id, name = %name, slug = %slug, duration_ms = duration, "Category created");

        Category::get(pool, id)
            .await?
            .ok_or_else(|| anyhow::anyhow!("Failed to retrieve created category"))
    }

    pub async fn update(
        pool: &Pool<Sqlite>,
        id: i64,
        parent_id: Option<i64>,
        name: String,
        slug: String,
        description: Option<String>,
    ) -> Result<Category> {
        validate_name(&name)?;
        validate_slug(&slug)?;
        if let Some(ref desc) = description {
            validate_description(desc)?;
        }

        // 更新前检测循环引用
        Self::detect_cycle(pool, id, parent_id).await?;

        let start = Instant::now();
        let now = Utc::now().to_rfc3339();
        let result = sqlx::query(
            "UPDATE categories SET parent_id = ?, name = ?, slug = ?, description = ?, updated_at = ? WHERE id = ?"
        )
        .bind(parent_id)
        .bind(&name)
        .bind(&slug)
        .bind(&description)
        .bind(&now)
        .bind(id)
        .execute(pool)
        .await;

        result.map_err(|e| handle_unique_constraint_error(e, "slug", &slug))?;

        let duration = start.elapsed().as_millis();
        tracing::info!(entity = "category", op = "update", id = id, name = %name, slug = %slug, duration_ms = duration, "Category updated");

        Category::get(pool, id)
            .await?
            .ok_or_else(|| anyhow::anyhow!("Category not found"))
    }

    /// 获取所有分类（不分页，用于构建树形结构）
    pub async fn list_all(pool: &Pool<Sqlite>) -> Result<Vec<Category>> {
        let categories = sqlx::query_as::<_, Category>(
            "SELECT id, parent_id, name, slug, description, created_at, updated_at FROM categories ORDER BY name",
        )
        .fetch_all(pool)
        .await?;
        Ok(categories)
    }

    /// 检测循环引用：从 parent_id 向上遍历，若遇到 current_id 则存在循环
    async fn detect_cycle(
        pool: &Pool<Sqlite>,
        current_id: i64,
        parent_id: Option<i64>,
    ) -> Result<()> {
        if let Some(pid) = parent_id {
            if current_id == pid {
                return Err(anyhow::anyhow!("不允许形成循环引用：分类不能以自身为父级"));
            }

            let mut check_id: Option<i64> = Some(pid);
            let mut visited = std::collections::HashSet::new();
            visited.insert(current_id);

            while let Some(parent) = check_id {
                if visited.contains(&parent) {
                    return Err(anyhow::anyhow!("不允许形成循环引用：检测到分类层级循环"));
                }
                visited.insert(parent);

                let next: Option<i64> =
                    sqlx::query_scalar("SELECT parent_id FROM categories WHERE id = ?")
                        .bind(parent)
                        .fetch_optional(pool)
                        .await?
                        .flatten();

                check_id = next;
            }
        }
        Ok(())
    }

    pub async fn delete(pool: &Pool<Sqlite>, id: i64) -> Result<()> {
        let start = Instant::now();

        // Reject deletion when child categories still reference this one.
        let child_count: i64 =
            sqlx::query_scalar("SELECT COUNT(*) FROM categories WHERE parent_id = ?")
                .bind(id)
                .fetch_one(pool)
                .await?;
        if child_count > 0 {
            return Err(anyhow::anyhow!(
                "无法删除分类：还存在 {child_count} 个子分类"
            ));
        }

        sqlx::query("DELETE FROM categories WHERE id = ?")
            .bind(id)
            .execute(pool)
            .await?;

        let duration = start.elapsed().as_millis();
        tracing::info!(
            entity = "category",
            op = "delete",
            id = id,
            duration_ms = duration,
            "Category deleted"
        );

        Ok(())
    }
}

// 实现 sqlx::FromRow for Category
impl sqlx::FromRow<'_, sqlx::sqlite::SqliteRow> for Category {
    fn from_row(row: &sqlx::sqlite::SqliteRow) -> Result<Self, sqlx::Error> {
        Ok(Category {
            id: row.try_get("id")?,
            parent_id: row.try_get("parent_id")?,
            name: row.try_get("name")?,
            slug: row.try_get("slug")?,
            description: row.try_get("description")?,
            created_at: row.try_get("created_at")?,
            updated_at: row.try_get("updated_at")?,
        })
    }
}

// === Capability CRUD ===
/// Registers the desktop runtime capability catalog without deleting custom entries.
pub async fn register_runtime_capabilities(pool: &Pool<Sqlite>) -> Result<()> {
    let now = Utc::now().to_rfc3339();
    for capability in crate::runtime::desktop_host::DESKTOP_CAPABILITY_CATALOG {
        sqlx::query(
            r#"
            INSERT INTO capabilities (name, description, is_dangerous, category_id, created_at)
            VALUES (?, ?, ?, NULL, ?)
            ON CONFLICT(name) DO UPDATE SET
                description = excluded.description,
                is_dangerous = excluded.is_dangerous
            "#,
        )
        .bind(capability.name)
        .bind(capability.description)
        .bind(capability.is_dangerous as i64)
        .bind(&now)
        .execute(pool)
        .await?;
    }
    Ok(())
}

impl Capability {
    pub async fn list(
        pool: &Pool<Sqlite>,
        search: Option<String>,
        limit: i64,
        offset: i64,
    ) -> Result<Vec<Capability>> {
        let caps = if let Some(s) = search {
            sqlx::query_as::<_, Capability>(
                "SELECT name, description, is_dangerous, category_id, created_at FROM capabilities WHERE name LIKE ? ORDER BY name LIMIT ? OFFSET ?"
            ).bind(format!("%{}%", s)).bind(limit).bind(offset).fetch_all(pool).await?
        } else {
            sqlx::query_as::<_, Capability>(
                "SELECT name, description, is_dangerous, category_id, created_at FROM capabilities ORDER BY name LIMIT ? OFFSET ?"
            ).bind(limit).bind(offset).fetch_all(pool).await?
        };
        Ok(caps)
    }

    pub async fn count(pool: &Pool<Sqlite>, search: Option<String>) -> Result<i64> {
        let c = if let Some(s) = search {
            sqlx::query_scalar::<_, i64>("SELECT COUNT(*) FROM capabilities WHERE name LIKE ?")
                .bind(format!("%{}%", s))
                .fetch_one(pool)
                .await?
        } else {
            sqlx::query_scalar::<_, i64>("SELECT COUNT(*) FROM capabilities")
                .fetch_one(pool)
                .await?
        };
        Ok(c)
    }

    pub async fn get(pool: &Pool<Sqlite>, name: &str) -> Result<Option<Capability>> {
        let cap = sqlx::query_as::<_, Capability>(
            "SELECT name, description, is_dangerous, category_id, created_at FROM capabilities WHERE name = ?"
        ).bind(name).fetch_optional(pool).await?;
        Ok(cap)
    }

    pub async fn create(
        pool: &Pool<Sqlite>,
        name: String,
        description: String,
        is_dangerous: bool,
        category_id: Option<i64>,
    ) -> Result<Capability> {
        // 验证字段
        validate_name(&name)?;
        validate_description(&description)?;

        let start = Instant::now();
        let now = Utc::now().to_rfc3339();
        sqlx::query("INSERT INTO capabilities (name, description, is_dangerous, category_id, created_at) VALUES (?, ?, ?, ?, ?)")
            .bind(&name).bind(&description).bind(is_dangerous as i64).bind(category_id).bind(&now)
            .execute(pool).await?;

        let duration = start.elapsed().as_millis();
        tracing::info!(entity = "capability", op = "create", name = %name, is_dangerous = is_dangerous, duration_ms = duration, "Capability created");

        Capability::get(pool, &name)
            .await?
            .ok_or_else(|| anyhow::anyhow!("Failed to retrieve created capability"))
    }

    pub async fn update(
        pool: &Pool<Sqlite>,
        old_name: &str,
        name: String,
        description: String,
        is_dangerous: bool,
        category_id: Option<i64>,
    ) -> Result<Capability> {
        validate_name(&name)?;
        validate_description(&description)?;

        let start = Instant::now();
        sqlx::query("UPDATE capabilities SET name = ?, description = ?, is_dangerous = ?, category_id = ? WHERE name = ?")
            .bind(&name).bind(&description).bind(is_dangerous as i64).bind(category_id).bind(old_name)
            .execute(pool).await?;

        let duration = start.elapsed().as_millis();
        tracing::info!(entity = "capability", op = "update", old_name = %old_name, new_name = %name, duration_ms = duration, "Capability updated");

        Capability::get(pool, &name)
            .await?
            .ok_or_else(|| anyhow::anyhow!("Capability not found"))
    }

    pub async fn delete(pool: &Pool<Sqlite>, name: &str) -> Result<()> {
        let start = Instant::now();
        sqlx::query("DELETE FROM capabilities WHERE name = ?")
            .bind(name)
            .execute(pool)
            .await?;

        let duration = start.elapsed().as_millis();
        tracing::info!(entity = "capability", op = "delete", name = %name, duration_ms = duration, "Capability deleted");

        Ok(())
    }
}

impl sqlx::FromRow<'_, sqlx::sqlite::SqliteRow> for Capability {
    fn from_row(row: &sqlx::sqlite::SqliteRow) -> Result<Self, sqlx::Error> {
        Ok(Capability {
            name: row.try_get("name")?,
            description: row.try_get("description")?,
            is_dangerous: row.try_get::<i64, _>("is_dangerous")? != 0,
            category_id: row.try_get("category_id")?,
            created_at: row.try_get("created_at")?,
        })
    }
}

const GET_PLUGIN_ARTIFACT_OPERATION_SQL: &str = "SELECT operation_id, kind, state, target_identifier, plugin_id, \
            expected_old_identifier, expected_old_version, expected_old_s3_key, \
            expected_old_sha256, expected_old_size, expected_old_identity, \
            expected_old_resource_limits, expected_old_row_revision, \
            staging_name, staging_identity, new_s3_key, new_sha256, new_size, \
            new_resource_limits, new_identity \
     FROM plugin_artifact_operations WHERE operation_id = ?";

const LIST_PLUGIN_ARTIFACT_OPERATIONS_SQL: &str = "SELECT operation_id, kind, state, target_identifier, plugin_id, \
            expected_old_identifier, expected_old_version, expected_old_s3_key, \
            expected_old_sha256, expected_old_size, expected_old_identity, \
            expected_old_resource_limits, expected_old_row_revision, \
            staging_name, staging_identity, new_s3_key, new_sha256, new_size, \
            new_resource_limits, new_identity \
     FROM plugin_artifact_operations \
     WHERE state <> 'done' ORDER BY created_at, operation_id";

impl TryFrom<PluginArtifactOperationRow> for PluginArtifactOperation {
    type Error = PluginLedgerError;

    fn try_from(row: PluginArtifactOperationRow) -> std::result::Result<Self, Self::Error> {
        let operation_id = Uuid::parse_str(&row.operation_id).map_err(|error| {
            PluginLedgerError::CorruptLedger(format!(
                "operation_id {:?} is not a UUID: {error}",
                row.operation_id
            ))
        })?;
        let kind = parse_operation_kind(&row.kind)?;
        let state = parse_operation_state(&row.state)?;
        let target_identifier =
            required_ledger_text(row.target_identifier, operation_id, "target_identifier")?;
        let plugin_id = row
            .plugin_id
            .map(|value| {
                value.parse::<i64>().map_err(|error| {
                    PluginLedgerError::CorruptLedger(format!(
                        "operation {operation_id} has invalid plugin_id {value:?}: {error}"
                    ))
                })
            })
            .transpose()?;
        let new_s3_key = required_ledger_text(row.new_s3_key, operation_id, "new_s3_key")?;
        let new_sha256 = required_ledger_text(row.new_sha256, operation_id, "new_sha256")?;
        let new_size = row.new_size.ok_or_else(|| {
            PluginLedgerError::CorruptLedger(format!("operation {operation_id} has NULL new_size"))
        })?;
        let new_resource_limits =
            required_ledger_text(row.new_resource_limits, operation_id, "new_resource_limits")?;

        if row.staging_name != derive_staging_name(&operation_id.to_string()) {
            return Err(PluginLedgerError::CorruptLedger(format!(
                "operation {operation_id} has a non-derived staging_name"
            )));
        }

        validate_artifact_fields(&new_s3_key, &new_sha256, new_size).map_err(|error| {
            PluginLedgerError::CorruptLedger(format!(
                "operation {operation_id} has an invalid new artifact tuple: {error}"
            ))
        })?;
        parse_resource_limits_json(&new_resource_limits).map_err(|error| {
            PluginLedgerError::CorruptLedger(format!(
                "operation {operation_id} has invalid new_resource_limits: {error}"
            ))
        })?;

        let operation = Self {
            operation_id,
            kind,
            state,
            target_identifier,
            plugin_id,
            expected_old_identifier: row.expected_old_identifier,
            expected_old_version: row.expected_old_version,
            expected_old_s3_key: row.expected_old_s3_key,
            expected_old_sha256: row.expected_old_sha256,
            expected_old_size: row.expected_old_size,
            expected_old_identity: row.expected_old_identity,
            expected_old_resource_limits: row.expected_old_resource_limits,
            expected_old_row_revision: row.expected_old_row_revision,
            staging_name: row.staging_name,
            staging_identity: row.staging_identity,
            new_s3_key,
            new_sha256,
            new_size,
            new_resource_limits,
            new_identity: row.new_identity,
        };
        validate_operation_invariants(&operation)?;
        Ok(operation)
    }
}

fn required_ledger_text(
    value: Option<String>,
    operation_id: Uuid,
    field: &str,
) -> std::result::Result<String, PluginLedgerError> {
    value.filter(|value| !value.is_empty()).ok_or_else(|| {
        PluginLedgerError::CorruptLedger(format!(
            "operation {operation_id} has NULL or empty {field}"
        ))
    })
}

fn parse_operation_kind(value: &str) -> std::result::Result<OperationKind, PluginLedgerError> {
    match value {
        "create" => Ok(OperationKind::Create),
        "replace" => Ok(OperationKind::Replace),
        _ => Err(PluginLedgerError::CorruptLedger(format!(
            "unknown operation kind {value:?}"
        ))),
    }
}

fn parse_operation_state(value: &str) -> std::result::Result<OperationState, PluginLedgerError> {
    match value {
        "prepared" => Ok(OperationState::Prepared),
        "staged" => Ok(OperationState::Staged),
        "published" => Ok(OperationState::Published),
        "referenced" => Ok(OperationState::Referenced),
        "done" => Ok(OperationState::Done),
        "conflict" => Ok(OperationState::Conflict),
        _ => Err(PluginLedgerError::CorruptLedger(format!(
            "unknown operation state {value:?}"
        ))),
    }
}

fn parse_gc_state(value: &str) -> std::result::Result<GcState, PluginLedgerError> {
    match value {
        "pending" => Ok(GcState::Pending),
        "blocked" => Ok(GcState::Blocked),
        _ => Err(PluginLedgerError::CorruptLedger(format!(
            "unknown GC state {value:?}"
        ))),
    }
}

fn invalid_ledger_input(field: &str, reason: impl Into<String>) -> PluginLedgerError {
    PluginLedgerError::InvalidInput {
        field: field.to_string(),
        reason: reason.into(),
    }
}

impl PluginResourceLimits {
    fn validate(&self) -> std::result::Result<(), PluginLedgerError> {
        validate_optional_limit(self.timeout_ms, "timeout_ms", 1, 120_000)?;
        validate_optional_limit(self.memory_limit_mb, "memory_limit_mb", 1, 512)?;
        validate_optional_limit(self.output_limit_bytes, "output_limit_bytes", 1, 52_428_800)?;
        Ok(())
    }

    fn to_json(&self) -> std::result::Result<String, PluginLedgerError> {
        self.validate()?;
        serde_json::to_string(self).map_err(|error| {
            invalid_ledger_input(
                "resource_limits",
                format!("cannot serialize limits: {error}"),
            )
        })
    }
}

fn validate_optional_limit(
    value: Option<i64>,
    field: &str,
    minimum: i64,
    maximum: i64,
) -> std::result::Result<(), PluginLedgerError> {
    if let Some(value) = value
        && !(minimum..=maximum).contains(&value)
    {
        return Err(invalid_ledger_input(
            field,
            format!("must be in {minimum}..={maximum}, got {value}"),
        ));
    }
    Ok(())
}

fn parse_resource_limits_json(
    value: &str,
) -> std::result::Result<PluginResourceLimits, PluginLedgerError> {
    if value.len() > 1024 * 1024 {
        return Err(invalid_ledger_input(
            "resource_limits",
            "JSON exceeds 1 MiB",
        ));
    }
    let limits = serde_json::from_str::<PluginResourceLimits>(value).map_err(|error| {
        invalid_ledger_input("resource_limits", format!("invalid JSON object: {error}"))
    })?;
    limits.validate()?;
    Ok(limits)
}

fn validate_capabilities_json(value: &str) -> std::result::Result<(), PluginLedgerError> {
    if value.len() > 1024 * 1024 {
        return Err(invalid_ledger_input("capabilities", "JSON exceeds 1 MiB"));
    }
    let capabilities = serde_json::from_str::<Vec<String>>(value).map_err(|error| {
        invalid_ledger_input(
            "capabilities",
            format!("must be a JSON array of strings: {error}"),
        )
    })?;
    if capabilities.iter().any(|value| value.trim().is_empty()) {
        return Err(invalid_ledger_input(
            "capabilities",
            "entries must be non-empty strings",
        ));
    }
    Ok(())
}

fn validate_artifact_fields(
    s3_key: &str,
    sha256: &str,
    size_bytes: i64,
) -> std::result::Result<(), PluginLedgerError> {
    if s3_key.is_empty()
        || s3_key.len() > 1024
        || s3_key.starts_with('/')
        || s3_key.ends_with('/')
        || s3_key.contains('\\')
        || s3_key.contains('\0')
        || s3_key
            .split('/')
            .any(|segment| segment.is_empty() || matches!(segment, "." | ".."))
    {
        return Err(invalid_ledger_input(
            "s3_key",
            "must be a safe non-empty relative artifact key",
        ));
    }
    if sha256.len() != 64
        || !sha256
            .bytes()
            .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
    {
        return Err(invalid_ledger_input(
            "sha256",
            "must contain exactly 64 lowercase hexadecimal characters",
        ));
    }
    if size_bytes <= 0 {
        return Err(invalid_ledger_input(
            "size_bytes",
            "must be greater than zero",
        ));
    }
    Ok(())
}

fn validate_identity(field: &str, identity: &str) -> std::result::Result<(), PluginLedgerError> {
    if identity.trim().is_empty() || identity.len() > 4096 {
        return Err(invalid_ledger_input(
            field,
            "must be a non-empty bounded file identity",
        ));
    }
    Ok(())
}

fn validate_new_artifact(new: &NewArtifact) -> std::result::Result<String, PluginLedgerError> {
    validate_artifact_fields(&new.s3_key, &new.sha256, new.size_bytes)?;
    new.resource_limits.to_json()
}

fn validate_operation_invariants(
    operation: &PluginArtifactOperation,
) -> std::result::Result<(), PluginLedgerError> {
    validate_identifier(&operation.target_identifier).map_err(|error| {
        PluginLedgerError::CorruptLedger(format!(
            "operation {} has invalid target_identifier: {error}",
            operation.operation_id
        ))
    })?;

    match operation.kind {
        OperationKind::Create => {
            if operation.expected_old_identifier.is_some()
                || operation.expected_old_version.is_some()
                || operation.expected_old_s3_key.is_some()
                || operation.expected_old_sha256.is_some()
                || operation.expected_old_size.is_some()
                || operation.expected_old_identity.is_some()
                || operation.expected_old_resource_limits.is_some()
                || operation.expected_old_row_revision.is_some()
            {
                return Err(PluginLedgerError::CorruptLedger(format!(
                    "create operation {} contains an expected-old tuple",
                    operation.operation_id
                )));
            }
        }
        OperationKind::Replace => {
            let old_identifier = operation
                .expected_old_identifier
                .as_deref()
                .ok_or_else(|| {
                    PluginLedgerError::CorruptLedger(format!(
                        "replace operation {} is missing expected_old_identifier",
                        operation.operation_id
                    ))
                })?;
            let old_version = operation.expected_old_version.as_deref().ok_or_else(|| {
                PluginLedgerError::CorruptLedger(format!(
                    "replace operation {} is missing expected_old_version",
                    operation.operation_id
                ))
            })?;
            let old_s3_key = operation.expected_old_s3_key.as_deref().ok_or_else(|| {
                PluginLedgerError::CorruptLedger(format!(
                    "replace operation {} is missing expected_old_s3_key",
                    operation.operation_id
                ))
            })?;
            let old_sha256 = operation.expected_old_sha256.as_deref().ok_or_else(|| {
                PluginLedgerError::CorruptLedger(format!(
                    "replace operation {} is missing expected_old_sha256",
                    operation.operation_id
                ))
            })?;
            let old_size = operation.expected_old_size.ok_or_else(|| {
                PluginLedgerError::CorruptLedger(format!(
                    "replace operation {} is missing expected_old_size",
                    operation.operation_id
                ))
            })?;
            let old_identity = operation.expected_old_identity.as_deref().ok_or_else(|| {
                PluginLedgerError::CorruptLedger(format!(
                    "replace operation {} is missing expected_old_identity",
                    operation.operation_id
                ))
            })?;
            let old_limits = operation
                .expected_old_resource_limits
                .as_deref()
                .ok_or_else(|| {
                    PluginLedgerError::CorruptLedger(format!(
                        "replace operation {} is missing expected_old_resource_limits",
                        operation.operation_id
                    ))
                })?;
            if operation.plugin_id.is_none() || operation.expected_old_row_revision.is_none() {
                return Err(PluginLedgerError::CorruptLedger(format!(
                    "replace operation {} is missing plugin_id or expected revision",
                    operation.operation_id
                )));
            }
            if old_identifier != operation.target_identifier || old_version.trim().is_empty() {
                return Err(PluginLedgerError::CorruptLedger(format!(
                    "replace operation {} has an inconsistent old identifier/version",
                    operation.operation_id
                )));
            }
            validate_artifact_fields(old_s3_key, old_sha256, old_size).map_err(|error| {
                PluginLedgerError::CorruptLedger(format!(
                    "replace operation {} has an invalid old artifact tuple: {error}",
                    operation.operation_id
                ))
            })?;
            validate_identity("expected_old_identity", old_identity).map_err(|error| {
                PluginLedgerError::CorruptLedger(format!(
                    "replace operation {} has an invalid old identity: {error}",
                    operation.operation_id
                ))
            })?;
            parse_resource_limits_json(old_limits).map_err(|error| {
                PluginLedgerError::CorruptLedger(format!(
                    "replace operation {} has invalid old resource limits: {error}",
                    operation.operation_id
                ))
            })?;
        }
    }

    match operation.state {
        OperationState::Prepared => {
            if operation.staging_identity.is_some() || operation.new_identity.is_some() {
                return Err(PluginLedgerError::CorruptLedger(format!(
                    "prepared operation {} contains an identity",
                    operation.operation_id
                )));
            }
        }
        OperationState::Staged => {
            if operation.staging_identity.is_none() || operation.new_identity.is_some() {
                return Err(PluginLedgerError::CorruptLedger(format!(
                    "staged operation {} has an invalid identity shape",
                    operation.operation_id
                )));
            }
        }
        OperationState::Published | OperationState::Referenced => {
            if operation.staging_identity.is_none() || operation.new_identity.is_none() {
                return Err(PluginLedgerError::CorruptLedger(format!(
                    "published/referenced operation {} has an invalid identity shape",
                    operation.operation_id
                )));
            }
        }
        OperationState::Done => {
            if operation.new_identity.is_some() && operation.staging_identity.is_none() {
                return Err(PluginLedgerError::CorruptLedger(format!(
                    "done operation {} has an invalid identity shape",
                    operation.operation_id
                )));
            }
        }
        OperationState::Conflict => {}
    }
    Ok(())
}

impl PluginArtifactLedger {
    pub fn new(pool: Pool<Sqlite>) -> Self {
        Self { pool }
    }

    pub async fn prepare(
        &self,
        operation_id: Uuid,
        input: PreparePluginOperation,
    ) -> std::result::Result<PluginArtifactOperation, PluginLedgerError> {
        let staging_name = derive_staging_name(&operation_id.to_string());
        let now = Utc::now().to_rfc3339();

        match input {
            PreparePluginOperation::Create {
                target_identifier,
                new,
            } => {
                validate_identifier(&target_identifier).map_err(|error| {
                    invalid_ledger_input("target_identifier", error.to_string())
                })?;
                let new_resource_limits = validate_new_artifact(&new)?;
                sqlx::query(
                    "INSERT INTO plugin_artifact_operations (\
                         operation_id, kind, target_identifier, plugin_id, \
                         expected_old_identifier, expected_old_version, \
                         expected_old_s3_key, expected_old_sha256, expected_old_size, \
                         expected_old_identity, expected_old_resource_limits, \
                         expected_old_row_revision, staging_name, staging_identity, \
                         new_s3_key, new_sha256, new_size, new_resource_limits, \
                         new_identity, state, created_at, updated_at\
                     ) VALUES (?, 'create', ?, NULL, NULL, NULL, NULL, NULL, NULL, \
                         NULL, NULL, NULL, ?, NULL, ?, ?, ?, ?, NULL, 'prepared', ?, ?)",
                )
                .bind(operation_id.to_string())
                .bind(target_identifier)
                .bind(staging_name)
                .bind(new.s3_key)
                .bind(new.sha256)
                .bind(new.size_bytes)
                .bind(new_resource_limits)
                .bind(&now)
                .bind(&now)
                .execute(&self.pool)
                .await?;
            }
            PreparePluginOperation::Replace { expected_old, new } => {
                validate_identifier(&expected_old.identifier).map_err(|error| {
                    invalid_ledger_input("expected_old.identifier", error.to_string())
                })?;
                if expected_old.plugin_id <= 0 {
                    return Err(invalid_ledger_input(
                        "expected_old.plugin_id",
                        "must be greater than zero",
                    ));
                }
                if expected_old.version.trim().is_empty() || expected_old.version.len() > 255 {
                    return Err(invalid_ledger_input(
                        "expected_old.version",
                        "must be non-empty and at most 255 bytes",
                    ));
                }
                if expected_old.row_revision < 0 {
                    return Err(invalid_ledger_input(
                        "expected_old.row_revision",
                        "must be non-negative",
                    ));
                }
                validate_artifact_fields(
                    &expected_old.s3_key,
                    &expected_old.sha256,
                    expected_old.size_bytes,
                )?;
                validate_identity("expected_old.identity", &expected_old.identity)?;
                let old_resource_limits = expected_old.resource_limits.to_json()?;
                let new_resource_limits = validate_new_artifact(&new)?;
                if new.s3_key == expected_old.s3_key {
                    return Err(invalid_ledger_input(
                        "new.s3_key",
                        "replace must publish a distinct immutable key",
                    ));
                }

                sqlx::query(
                    "INSERT INTO plugin_artifact_operations (\
                         operation_id, kind, target_identifier, plugin_id, \
                         expected_old_identifier, expected_old_version, \
                         expected_old_s3_key, expected_old_sha256, expected_old_size, \
                         expected_old_identity, expected_old_resource_limits, \
                         expected_old_row_revision, staging_name, staging_identity, \
                         new_s3_key, new_sha256, new_size, new_resource_limits, \
                         new_identity, state, created_at, updated_at\
                     ) VALUES (?, 'replace', ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, NULL, \
                         ?, ?, ?, ?, NULL, 'prepared', ?, ?)",
                )
                .bind(operation_id.to_string())
                .bind(&expected_old.identifier)
                .bind(expected_old.plugin_id.to_string())
                .bind(&expected_old.identifier)
                .bind(expected_old.version)
                .bind(expected_old.s3_key)
                .bind(expected_old.sha256)
                .bind(expected_old.size_bytes)
                .bind(expected_old.identity)
                .bind(old_resource_limits)
                .bind(expected_old.row_revision)
                .bind(staging_name)
                .bind(new.s3_key)
                .bind(new.sha256)
                .bind(new.size_bytes)
                .bind(new_resource_limits)
                .bind(&now)
                .bind(&now)
                .execute(&self.pool)
                .await?;
            }
        }

        self.required(operation_id).await
    }

    pub async fn get(
        &self,
        operation_id: Uuid,
    ) -> std::result::Result<Option<PluginArtifactOperation>, PluginLedgerError> {
        let row =
            sqlx::query_as::<_, PluginArtifactOperationRow>(GET_PLUGIN_ARTIFACT_OPERATION_SQL)
                .bind(operation_id.to_string())
                .fetch_optional(&self.pool)
                .await?;
        row.map(TryInto::try_into).transpose()
    }

    pub async fn list_not_done(
        &self,
    ) -> std::result::Result<Vec<PluginArtifactOperation>, PluginLedgerError> {
        sqlx::query_as::<_, PluginArtifactOperationRow>(LIST_PLUGIN_ARTIFACT_OPERATIONS_SQL)
            .fetch_all(&self.pool)
            .await?
            .into_iter()
            .map(TryInto::try_into)
            .collect()
    }

    pub async fn transition(
        &self,
        operation_id: Uuid,
        expected: OperationState,
        change: OperationTransition,
    ) -> std::result::Result<PluginArtifactOperation, PluginLedgerError> {
        let operation = self.required(operation_id).await?;
        if operation.state != expected {
            return Err(PluginLedgerError::StateConflict {
                operation_id,
                expected,
                actual: operation.state,
            });
        }

        let now = Utc::now().to_rfc3339();
        let result = match change {
            OperationTransition::Staged { staging_identity } => {
                if expected != OperationState::Prepared {
                    return Err(invalid_ledger_input(
                        "state",
                        "Staged is only legal from Prepared",
                    ));
                }
                validate_identity("staging_identity", &staging_identity)?;
                sqlx::query(
                    "UPDATE plugin_artifact_operations \
                     SET staging_identity = ?, state = 'staged', updated_at = ? \
                     WHERE operation_id = ? AND state = 'prepared'",
                )
                .bind(staging_identity)
                .bind(&now)
                .bind(operation_id.to_string())
                .execute(&self.pool)
                .await?
            }
            OperationTransition::Published { new_identity } => {
                if expected != OperationState::Staged {
                    return Err(invalid_ledger_input(
                        "state",
                        "Published is only legal from Staged",
                    ));
                }
                validate_identity("new_identity", &new_identity)?;
                sqlx::query(
                    "UPDATE plugin_artifact_operations \
                     SET new_identity = ?, state = 'published', updated_at = ? \
                     WHERE operation_id = ? AND state = 'staged'",
                )
                .bind(new_identity)
                .bind(&now)
                .bind(operation_id.to_string())
                .execute(&self.pool)
                .await?
            }
            OperationTransition::Done => {
                let legal = matches!(expected, OperationState::Prepared | OperationState::Staged)
                    || (expected == OperationState::Referenced
                        && operation.kind == OperationKind::Create);
                if !legal {
                    return Err(invalid_ledger_input(
                        "state",
                        "Done would bypass owned-artifact GC or a user commit",
                    ));
                }
                sqlx::query(
                    "UPDATE plugin_artifact_operations SET state = 'done', updated_at = ? \
                     WHERE operation_id = ? AND state = ?",
                )
                .bind(&now)
                .bind(operation_id.to_string())
                .bind(expected.as_str())
                .execute(&self.pool)
                .await?
            }
            OperationTransition::Conflict => {
                if matches!(
                    expected,
                    OperationState::Referenced | OperationState::Done | OperationState::Conflict
                ) {
                    return Err(invalid_ledger_input(
                        "state",
                        "committed or terminal operations cannot transition to Conflict",
                    ));
                }
                sqlx::query(
                    "UPDATE plugin_artifact_operations SET state = 'conflict', updated_at = ? \
                     WHERE operation_id = ? AND state = ?",
                )
                .bind(&now)
                .bind(operation_id.to_string())
                .bind(expected.as_str())
                .execute(&self.pool)
                .await?
            }
        };

        if result.rows_affected() != 1 {
            let actual = self.required(operation_id).await?.state;
            return Err(PluginLedgerError::StateConflict {
                operation_id,
                expected,
                actual,
            });
        }
        self.required(operation_id).await
    }

    pub async fn commit_published_create(
        &self,
        operation_id: Uuid,
        fields: CreatePluginFields,
    ) -> std::result::Result<Plugin, PluginLedgerError> {
        validate_create_plugin_fields(&fields)?;
        let operation = self.required(operation_id).await?;
        if operation.kind != OperationKind::Create {
            return Err(PluginLedgerError::KindMismatch {
                operation_id,
                expected: OperationKind::Create,
                actual: operation.kind,
            });
        }
        if operation.state != OperationState::Published {
            return Err(PluginLedgerError::StateConflict {
                operation_id,
                expected: OperationState::Published,
                actual: operation.state,
            });
        }

        let now = Utc::now().to_rfc3339();
        let mut transaction = self.pool.begin().await?;
        let plugin_id = sqlx::query_scalar::<_, i64>(
            "INSERT INTO plugins (\
                 identifier, name, description, manifest, runtime, version, author, \
                 repository_url, s3_key, sha256, size_bytes, category_id, capabilities, \
                 resource_limits, row_revision, created_at, updated_at\
             ) VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, 0, ?, ?) RETURNING id",
        )
        .bind(&operation.target_identifier)
        .bind(&fields.name)
        .bind(&fields.description)
        .bind(&fields.manifest)
        .bind(&fields.runtime)
        .bind(&fields.version)
        .bind(fields.author.as_deref().unwrap_or(""))
        .bind(fields.repository_url.as_deref().unwrap_or(""))
        .bind(&operation.new_s3_key)
        .bind(&operation.new_sha256)
        .bind(operation.new_size)
        .bind(fields.category_id)
        .bind(&fields.capabilities)
        .bind(&operation.new_resource_limits)
        .bind(&now)
        .bind(&now)
        .fetch_one(&mut *transaction)
        .await?;

        let updated = sqlx::query(
            "UPDATE plugin_artifact_operations \
             SET plugin_id = ?, state = 'referenced', updated_at = ? \
             WHERE operation_id = ? AND kind = 'create' AND state = 'published'",
        )
        .bind(plugin_id.to_string())
        .bind(&now)
        .bind(operation_id.to_string())
        .execute(&mut *transaction)
        .await?;
        if updated.rows_affected() != 1 {
            return Err(PluginLedgerError::StateConflict {
                operation_id,
                expected: OperationState::Published,
                actual: operation.state,
            });
        }

        let plugin = fetch_plugin_in_transaction(&mut transaction, plugin_id).await?;
        transaction.commit().await?;
        Ok(plugin)
    }

    pub async fn commit_published_replace(
        &self,
        operation_id: Uuid,
    ) -> std::result::Result<ReplaceOutcome, PluginLedgerError> {
        let operation = self.required(operation_id).await?;
        if operation.kind != OperationKind::Replace {
            return Err(PluginLedgerError::KindMismatch {
                operation_id,
                expected: OperationKind::Replace,
                actual: operation.kind,
            });
        }
        if operation.state != OperationState::Published {
            return Err(PluginLedgerError::StateConflict {
                operation_id,
                expected: OperationState::Published,
                actual: operation.state,
            });
        }

        let plugin_id = required_old_plugin_id(&operation)?;
        let expected_identifier = required_old_text(&operation, "expected_old_identifier")?;
        let expected_version = required_old_text(&operation, "expected_old_version")?;
        let expected_s3_key = required_old_text(&operation, "expected_old_s3_key")?;
        let expected_sha256 = required_old_text(&operation, "expected_old_sha256")?;
        let expected_size = operation.expected_old_size.ok_or_else(|| {
            PluginLedgerError::CorruptLedger(format!(
                "replace operation {operation_id} is missing expected_old_size"
            ))
        })?;
        let expected_resource_limits =
            required_old_text(&operation, "expected_old_resource_limits")?;
        let expected_row_revision = operation.expected_old_row_revision.ok_or_else(|| {
            PluginLedgerError::CorruptLedger(format!(
                "replace operation {operation_id} is missing expected_old_row_revision"
            ))
        })?;
        let now = Utc::now().to_rfc3339();

        let mut transaction = self.pool.begin().await?;
        let replaced = sqlx::query(
            "UPDATE plugins SET \
                 s3_key = ?, sha256 = ?, size_bytes = ?, resource_limits = ?, \
                 row_revision = row_revision + 1, updated_at = ? \
             WHERE id = ? AND identifier = ? AND version = ? AND s3_key = ? \
                 AND sha256 = ? AND size_bytes = ? AND resource_limits = ? \
                 AND row_revision = ? AND deleted_at IS NULL",
        )
        .bind(&operation.new_s3_key)
        .bind(&operation.new_sha256)
        .bind(operation.new_size)
        .bind(&operation.new_resource_limits)
        .bind(&now)
        .bind(plugin_id)
        .bind(expected_identifier)
        .bind(expected_version)
        .bind(expected_s3_key)
        .bind(expected_sha256)
        .bind(expected_size)
        .bind(expected_resource_limits)
        .bind(expected_row_revision)
        .execute(&mut *transaction)
        .await?;

        if replaced.rows_affected() == 1 {
            let referenced = sqlx::query(
                "UPDATE plugin_artifact_operations SET state = 'referenced', updated_at = ? \
                 WHERE operation_id = ? AND kind = 'replace' AND state = 'published'",
            )
            .bind(&now)
            .bind(operation_id.to_string())
            .execute(&mut *transaction)
            .await?;
            if referenced.rows_affected() != 1 {
                return Err(PluginLedgerError::StateConflict {
                    operation_id,
                    expected: OperationState::Published,
                    actual: operation.state,
                });
            }
            let plugin = fetch_plugin_in_transaction(&mut transaction, plugin_id).await?;
            transaction.commit().await?;
            crate::runtime::plugin_executor::invalidate_artifact_instances_any_scope(
                plugin_id,
                expected_s3_key,
            );
            return Ok(ReplaceOutcome::Referenced {
                plugin: Box::new(plugin),
            });
        }

        let actual_row_revision =
            sqlx::query_scalar::<_, i64>("SELECT row_revision FROM plugins WHERE id = ?")
                .bind(plugin_id)
                .fetch_optional(&mut *transaction)
                .await?;
        let ownership = operation_gc_ownership(&operation, OperationGcTarget::PublishedNew)?;
        insert_or_verify_owned_gc(&mut transaction, operation_id, &ownership, &now).await?;
        let finished = sqlx::query(
            "UPDATE plugin_artifact_operations SET state = 'done', updated_at = ? \
             WHERE operation_id = ? AND kind = 'replace' AND state = 'published'",
        )
        .bind(&now)
        .bind(operation_id.to_string())
        .execute(&mut *transaction)
        .await?;
        if finished.rows_affected() != 1 {
            return Err(PluginLedgerError::StateConflict {
                operation_id,
                expected: OperationState::Published,
                actual: operation.state,
            });
        }
        transaction.commit().await?;
        Ok(ReplaceOutcome::ConcurrentConflict {
            plugin_id,
            actual_row_revision,
        })
    }

    pub async fn finish_with_gc(
        &self,
        operation_id: Uuid,
        expected: OperationState,
        target: OperationGcTarget,
    ) -> std::result::Result<FinishOutcome, PluginLedgerError> {
        let operation = self.required(operation_id).await?;
        let ownership = operation_gc_ownership(&operation, target)?;

        if operation.state == OperationState::Done {
            let gc_state = verify_existing_owned_gc(&self.pool, operation_id, &ownership).await?;
            return Ok(FinishOutcome::AlreadyDone { gc_state });
        }
        if operation.state != expected {
            return Err(PluginLedgerError::StateConflict {
                operation_id,
                expected,
                actual: operation.state,
            });
        }
        let legal = matches!(
            (expected, target, operation.kind),
            (
                OperationState::Published,
                OperationGcTarget::PublishedNew,
                OperationKind::Create | OperationKind::Replace
            ) | (
                OperationState::Referenced,
                OperationGcTarget::ExpectedOld,
                OperationKind::Replace
            )
        );
        if !legal {
            return Err(invalid_ledger_input(
                "gc_target",
                "target is not owned by the operation in its expected state",
            ));
        }

        let now = Utc::now().to_rfc3339();
        let mut transaction = self.pool.begin().await?;
        let gc_state =
            insert_or_verify_owned_gc(&mut transaction, operation_id, &ownership, &now).await?;
        let finished = sqlx::query(
            "UPDATE plugin_artifact_operations SET state = 'done', updated_at = ? \
             WHERE operation_id = ? AND state = ?",
        )
        .bind(&now)
        .bind(operation_id.to_string())
        .bind(expected.as_str())
        .execute(&mut *transaction)
        .await?;
        if finished.rows_affected() != 1 {
            return Err(PluginLedgerError::StateConflict {
                operation_id,
                expected,
                actual: operation.state,
            });
        }
        transaction.commit().await?;
        Ok(FinishOutcome::Finished { gc_state })
    }

    async fn required(
        &self,
        operation_id: Uuid,
    ) -> std::result::Result<PluginArtifactOperation, PluginLedgerError> {
        self.get(operation_id)
            .await?
            .ok_or(PluginLedgerError::NotFound { operation_id })
    }
}

fn validate_create_plugin_fields(
    fields: &CreatePluginFields,
) -> std::result::Result<(), PluginLedgerError> {
    validate_name(&fields.name).map_err(|error| invalid_ledger_input("name", error.to_string()))?;
    if let Some(description) = &fields.description {
        validate_description(description)
            .map_err(|error| invalid_ledger_input("description", error.to_string()))?;
    }
    if let Some(manifest) = &fields.manifest {
        validate_json(manifest)
            .map_err(|error| invalid_ledger_input("manifest", error.to_string()))?;
    }
    if fields.version.trim().is_empty() || fields.version.len() > 255 {
        return Err(invalid_ledger_input(
            "version",
            "must be non-empty and at most 255 bytes",
        ));
    }
    if fields.runtime.trim().is_empty() || fields.runtime.len() > 255 {
        return Err(invalid_ledger_input(
            "runtime",
            "must be non-empty and at most 255 bytes",
        ));
    }
    validate_capabilities_json(&fields.capabilities)?;
    Ok(())
}

fn required_old_plugin_id(
    operation: &PluginArtifactOperation,
) -> std::result::Result<i64, PluginLedgerError> {
    operation.plugin_id.ok_or_else(|| {
        PluginLedgerError::CorruptLedger(format!(
            "replace operation {} is missing plugin_id",
            operation.operation_id
        ))
    })
}

fn required_old_text<'a>(
    operation: &'a PluginArtifactOperation,
    field: &str,
) -> std::result::Result<&'a str, PluginLedgerError> {
    let value = match field {
        "expected_old_identifier" => operation.expected_old_identifier.as_deref(),
        "expected_old_version" => operation.expected_old_version.as_deref(),
        "expected_old_s3_key" => operation.expected_old_s3_key.as_deref(),
        "expected_old_sha256" => operation.expected_old_sha256.as_deref(),
        "expected_old_identity" => operation.expected_old_identity.as_deref(),
        "expected_old_resource_limits" => operation.expected_old_resource_limits.as_deref(),
        _ => None,
    };
    value.ok_or_else(|| {
        PluginLedgerError::CorruptLedger(format!(
            "operation {} is missing {field}",
            operation.operation_id
        ))
    })
}

async fn fetch_plugin_in_transaction(
    transaction: &mut sqlx::Transaction<'_, Sqlite>,
    plugin_id: i64,
) -> std::result::Result<Plugin, PluginLedgerError> {
    sqlx::query_as::<_, Plugin>(
        "SELECT id, identifier, name, description, manifest, runtime, version, author, \
                repository_url, s3_key, sha256, size_bytes, category_id, capabilities, \
                resource_limits, row_revision, created_at, updated_at, deleted_at \
         FROM plugins WHERE id = ?",
    )
    .bind(plugin_id)
    .fetch_optional(&mut **transaction)
    .await?
    .ok_or_else(|| {
        PluginLedgerError::CorruptLedger(format!(
            "Plugin row {plugin_id} disappeared inside its ledger transaction"
        ))
    })
}

fn operation_gc_ownership(
    operation: &PluginArtifactOperation,
    target: OperationGcTarget,
) -> std::result::Result<OperationGcOwnership, PluginLedgerError> {
    match target {
        OperationGcTarget::PublishedNew => {
            let identity = operation.new_identity.clone().ok_or_else(|| {
                PluginLedgerError::CorruptLedger(format!(
                    "operation {} has no published new identity",
                    operation.operation_id
                ))
            })?;
            validate_identity("new_identity", &identity)?;
            Ok(OperationGcOwnership {
                artifact_key: operation.new_s3_key.clone(),
                sha256: operation.new_sha256.clone(),
                size_bytes: operation.new_size,
                identity,
                reason: "published_new",
            })
        }
        OperationGcTarget::ExpectedOld => {
            if operation.kind != OperationKind::Replace {
                return Err(PluginLedgerError::KindMismatch {
                    operation_id: operation.operation_id,
                    expected: OperationKind::Replace,
                    actual: operation.kind,
                });
            }
            let artifact_key = required_old_text(operation, "expected_old_s3_key")?.to_string();
            let sha256 = required_old_text(operation, "expected_old_sha256")?.to_string();
            let size_bytes = operation.expected_old_size.ok_or_else(|| {
                PluginLedgerError::CorruptLedger(format!(
                    "replace operation {} has no expected_old_size",
                    operation.operation_id
                ))
            })?;
            let identity = required_old_text(operation, "expected_old_identity")?.to_string();
            validate_artifact_fields(&artifact_key, &sha256, size_bytes)?;
            validate_identity("expected_old_identity", &identity)?;
            Ok(OperationGcOwnership {
                artifact_key,
                sha256,
                size_bytes,
                identity,
                reason: "expected_old",
            })
        }
    }
}

type ExistingGcOwnership = (
    Option<String>,
    Option<i64>,
    Option<String>,
    Option<String>,
    String,
);

fn verify_gc_ownership_tuple(
    operation_id: Uuid,
    ownership: &OperationGcOwnership,
    existing: ExistingGcOwnership,
) -> std::result::Result<GcState, PluginLedgerError> {
    let (sha256, size_bytes, identity, source_operation_id, state) = existing;
    let matches = sha256.as_deref() == Some(ownership.sha256.as_str())
        && size_bytes == Some(ownership.size_bytes)
        && identity.as_deref() == Some(ownership.identity.as_str())
        && source_operation_id.as_deref() == Some(operation_id.to_string().as_str());
    if !matches {
        return Err(PluginLedgerError::CorruptLedger(format!(
            "GC artifact {:?} has conflicting ownership or identity",
            ownership.artifact_key
        )));
    }
    parse_gc_state(&state)
}

async fn insert_or_verify_owned_gc(
    transaction: &mut sqlx::Transaction<'_, Sqlite>,
    operation_id: Uuid,
    ownership: &OperationGcOwnership,
    now: &str,
) -> std::result::Result<GcState, PluginLedgerError> {
    let existing = sqlx::query_as::<_, ExistingGcOwnership>(
        "SELECT expected_sha256, expected_size_bytes, expected_identity, \
                source_operation_id, state \
         FROM plugin_artifact_gc WHERE artifact_key = ?",
    )
    .bind(&ownership.artifact_key)
    .fetch_optional(&mut **transaction)
    .await?;
    if let Some(existing) = existing {
        return verify_gc_ownership_tuple(operation_id, ownership, existing);
    }

    sqlx::query(
        "INSERT INTO plugin_artifact_gc (\
             artifact_key, expected_sha256, expected_size_bytes, expected_identity, \
             source_operation_id, state, attempts, last_error, created_at, updated_at, \
             reason, last_attempt_at\
         ) VALUES (?, ?, ?, ?, ?, 'pending', 0, NULL, ?, ?, ?, 0)",
    )
    .bind(&ownership.artifact_key)
    .bind(&ownership.sha256)
    .bind(ownership.size_bytes)
    .bind(&ownership.identity)
    .bind(operation_id.to_string())
    .bind(now)
    .bind(now)
    .bind(ownership.reason)
    .execute(&mut **transaction)
    .await?;
    Ok(GcState::Pending)
}

async fn verify_existing_owned_gc(
    pool: &Pool<Sqlite>,
    operation_id: Uuid,
    ownership: &OperationGcOwnership,
) -> std::result::Result<GcState, PluginLedgerError> {
    let existing = sqlx::query_as::<_, ExistingGcOwnership>(
        "SELECT expected_sha256, expected_size_bytes, expected_identity, \
                source_operation_id, state \
         FROM plugin_artifact_gc WHERE artifact_key = ?",
    )
    .bind(&ownership.artifact_key)
    .fetch_optional(pool)
    .await?
    .ok_or_else(|| {
        PluginLedgerError::CorruptLedger(format!(
            "done operation {operation_id} has no owned GC row for {:?}",
            ownership.artifact_key
        ))
    })?;
    verify_gc_ownership_tuple(operation_id, ownership, existing)
}

// === Plugin CRUD ===
impl Plugin {
    pub async fn list(
        pool: &Pool<Sqlite>,
        search: Option<String>,
        limit: i64,
        offset: i64,
    ) -> Result<Vec<Plugin>> {
        let plugins = if let Some(s) = search {
            sqlx::query_as::<_, Plugin>(
                "SELECT id, identifier, name, description, manifest, runtime, version, author, repository_url, s3_key, sha256, size_bytes, category_id, capabilities, resource_limits, row_revision, created_at, updated_at, deleted_at FROM plugins WHERE deleted_at IS NULL AND (name LIKE ? OR identifier LIKE ?) ORDER BY name LIMIT ? OFFSET ?"
            ).bind(format!("%{}%", s)).bind(format!("%{}%", s)).bind(limit).bind(offset).fetch_all(pool).await?
        } else {
            sqlx::query_as::<_, Plugin>(
                "SELECT id, identifier, name, description, manifest, runtime, version, author, repository_url, s3_key, sha256, size_bytes, category_id, capabilities, resource_limits, row_revision, created_at, updated_at, deleted_at FROM plugins WHERE deleted_at IS NULL ORDER BY name LIMIT ? OFFSET ?"
            ).bind(limit).bind(offset).fetch_all(pool).await?
        };
        Ok(plugins)
    }

    pub async fn count(pool: &Pool<Sqlite>, search: Option<String>) -> Result<i64> {
        let c = if let Some(s) = search {
            sqlx::query_scalar::<_, i64>("SELECT COUNT(*) FROM plugins WHERE deleted_at IS NULL AND (name LIKE ? OR identifier LIKE ?)")
                .bind(format!("%{}%", s)).bind(format!("%{}%", s)).fetch_one(pool).await?
        } else {
            sqlx::query_scalar::<_, i64>("SELECT COUNT(*) FROM plugins WHERE deleted_at IS NULL")
                .fetch_one(pool)
                .await?
        };
        Ok(c)
    }

    pub async fn get(pool: &Pool<Sqlite>, id: i64) -> Result<Option<Plugin>> {
        let p = sqlx::query_as::<_, Plugin>(
            "SELECT id, identifier, name, description, manifest, runtime, version, author, repository_url, s3_key, sha256, size_bytes, category_id, capabilities, resource_limits, row_revision, created_at, updated_at, deleted_at FROM plugins WHERE id = ?"
        ).bind(id).fetch_optional(pool).await?;
        Ok(p)
    }

    pub async fn create(
        pool: &Pool<Sqlite>,
        identifier: String,
        name: String,
        description: Option<String>,
        manifest: Option<String>,
        runtime: String,
        version: String,
        author: Option<String>,
        repository_url: Option<String>,
        s3_key: String,
        sha256: String,
        size_bytes: i64,
        category_id: Option<i64>,
    ) -> Result<Plugin> {
        // 验证字段
        validate_identifier(&identifier)?;
        validate_name(&name)?;
        if let Some(ref desc) = description {
            validate_description(desc)?;
        }
        if let Some(ref m) = manifest {
            validate_json(m)?;
        }

        let start = Instant::now();
        let now = Utc::now().to_rfc3339();
        let result = sqlx::query_scalar::<_, i64>(
            "INSERT INTO plugins (identifier, name, description, manifest, runtime, version, author, repository_url, s3_key, sha256, size_bytes, category_id, created_at, updated_at) VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?) RETURNING id"
        ).bind(&identifier).bind(&name).bind(&description).bind(&manifest).bind(&runtime).bind(&version).bind(author.as_deref().unwrap_or("")).bind(repository_url.as_deref().unwrap_or("")).bind(&s3_key).bind(&sha256).bind(size_bytes).bind(category_id).bind(&now).bind(&now)
            .fetch_one(pool).await;

        let id =
            result.map_err(|e| handle_unique_constraint_error(e, "identifier", &identifier))?;

        let duration = start.elapsed().as_millis();
        tracing::info!(entity = "plugin", op = "create", id = id, identifier = %identifier, name = %name, runtime = %runtime, version = %version, duration_ms = duration, "Plugin created");

        Plugin::get(pool, id)
            .await?
            .ok_or_else(|| anyhow::anyhow!("Failed to retrieve created plugin"))
    }

    pub async fn update(
        pool: &Pool<Sqlite>,
        id: i64,
        identifier: String,
        name: String,
        description: Option<String>,
        manifest: Option<String>,
        runtime: String,
        version: String,
        author: Option<String>,
        repository_url: Option<String>,
        s3_key: String,
        sha256: String,
        size_bytes: i64,
        category_id: Option<i64>,
    ) -> Result<Plugin> {
        validate_identifier(&identifier)?;
        validate_name(&name)?;
        if let Some(ref desc) = description {
            validate_description(desc)?;
        }
        if let Some(ref m) = manifest {
            validate_json(m)?;
        }

        let previous_s3_key =
            sqlx::query_scalar::<_, String>("SELECT s3_key FROM plugins WHERE id = ?")
                .bind(id)
                .fetch_optional(pool)
                .await?;
        let start = Instant::now();
        let now = Utc::now().to_rfc3339();
        let result = sqlx::query(
            "UPDATE plugins SET identifier=?, name=?, description=?, manifest=?, runtime=?, version=?, author=?, repository_url=?, s3_key=?, sha256=?, size_bytes=?, category_id=?, row_revision = row_revision + 1, updated_at=? WHERE id=?"
        ).bind(&identifier).bind(&name).bind(&description).bind(&manifest).bind(&runtime).bind(&version).bind(author.as_deref().unwrap_or("")).bind(repository_url.as_deref().unwrap_or("")).bind(&s3_key).bind(&sha256).bind(size_bytes).bind(category_id).bind(&now).bind(id)
            .execute(pool).await;

        result.map_err(|e| handle_unique_constraint_error(e, "identifier", &identifier))?;

        if let Some(previous_s3_key) = previous_s3_key {
            crate::runtime::plugin_executor::invalidate_artifact_instances_any_scope(
                id,
                &previous_s3_key,
            );
        }

        let duration = start.elapsed().as_millis();
        tracing::info!(entity = "plugin", op = "update", id = id, identifier = %identifier, name = %name, duration_ms = duration, "Plugin updated");

        Plugin::get(pool, id)
            .await?
            .ok_or_else(|| anyhow::anyhow!("Plugin not found"))
    }

    /// Optimistic-concurrency replace of a plugin's immutable-artifact
    /// reference. The CAS is conditioned on the exact `row_revision`
    /// read by the caller; the `s3_key`/`sha256`/`size_bytes` tuple is
    /// switched and `row_revision` incremented atomically. Returns
    /// `Ok(None)` when the revision no longer matches (another writer
    /// won) — a stable concurrent-conflict signal, never a partial write.
    pub async fn replace(
        pool: &Pool<Sqlite>,
        id: i64,
        expected_row_revision: i64,
        s3_key: String,
        sha256: String,
        size_bytes: i64,
    ) -> Result<Option<Plugin>> {
        let previous_s3_key =
            sqlx::query_scalar::<_, String>("SELECT s3_key FROM plugins WHERE id = ?")
                .bind(id)
                .fetch_optional(pool)
                .await?;
        let start = Instant::now();
        let now = Utc::now().to_rfc3339();
        let affected = sqlx::query(
            "UPDATE plugins SET s3_key = ?, sha256 = ?, size_bytes = ?, row_revision = row_revision + 1, updated_at = ? WHERE id = ? AND row_revision = ?",
        )
        .bind(&s3_key)
        .bind(&sha256)
        .bind(size_bytes)
        .bind(&now)
        .bind(id)
        .bind(expected_row_revision)
        .execute(pool)
        .await?;

        if affected.rows_affected() != 1 {
            // Revision mismatch: a concurrent writer advanced the row.
            return Ok(None);
        }

        if let Some(previous_s3_key) = previous_s3_key {
            crate::runtime::plugin_executor::invalidate_artifact_instances_any_scope(
                id,
                &previous_s3_key,
            );
        }

        let duration = start.elapsed().as_millis();
        tracing::info!(entity = "plugin", op = "replace", id = id, s3_key = %s3_key, duration_ms = duration, "Plugin artifact reference replaced");

        let plugin = Plugin::get(pool, id)
            .await?
            .ok_or_else(|| anyhow::anyhow!("Plugin not found after replace"))?;
        Ok(Some(plugin))
    }

    pub async fn delete(pool: &Pool<Sqlite>, id: i64) -> Result<()> {
        let previous_s3_key =
            sqlx::query_scalar::<_, String>("SELECT s3_key FROM plugins WHERE id = ?")
                .bind(id)
                .fetch_optional(pool)
                .await?;
        let start = Instant::now();
        let now = Utc::now().to_rfc3339();
        sqlx::query("UPDATE plugins SET deleted_at = ? WHERE id = ?")
            .bind(&now)
            .bind(id)
            .execute(pool)
            .await?;

        if let Some(previous_s3_key) = previous_s3_key {
            crate::runtime::plugin_executor::invalidate_artifact_instances_any_scope(
                id,
                &previous_s3_key,
            );
        }

        let duration = start.elapsed().as_millis();
        tracing::info!(
            entity = "plugin",
            op = "delete",
            id = id,
            duration_ms = duration,
            "Plugin soft deleted"
        );

        Ok(())
    }

    /// Atomically persist the plugin's declared capabilities (a JSON
    /// array) and resource limits (a JSON object), bumping
    /// `row_revision` so the change is observable to CAS readers.
    /// Returns `Ok(None)` when the row no longer exists (soft-deleted
    /// rows are still addressable by id and thus remain updatable).
    pub async fn update_limits(
        pool: &Pool<Sqlite>,
        id: i64,
        capabilities: String,
        resource_limits: String,
    ) -> Result<Option<Plugin>> {
        validate_capabilities_json(&capabilities)
            .map_err(|error| anyhow::anyhow!(error.to_string()))?;
        parse_resource_limits_json(&resource_limits)
            .map_err(|error| anyhow::anyhow!(error.to_string()))?;

        let now = Utc::now().to_rfc3339();
        let affected = sqlx::query(
            "UPDATE plugins SET capabilities = ?, resource_limits = ?, row_revision = row_revision + 1, updated_at = ? WHERE id = ?",
        )
        .bind(&capabilities)
        .bind(&resource_limits)
        .bind(&now)
        .bind(id)
        .execute(pool)
        .await?;

        if affected.rows_affected() != 1 {
            return Ok(None);
        }

        let duration = Instant::now().elapsed().as_millis();
        tracing::info!(
            entity = "plugin",
            op = "update_limits",
            id = id,
            duration_ms = duration,
            "Plugin capabilities/resource_limits updated"
        );

        let plugin = Plugin::get(pool, id).await?;
        if let Some(plugin) = plugin.as_ref() {
            crate::runtime::plugin_executor::invalidate_artifact_instances_any_scope(
                id,
                &plugin.s3_key,
            );
        }
        Ok(plugin)
    }

    /// Persist the canonical artifact key for a plugin whose database id was
    /// only known after the INSERT. Used by the create flow to backfill
    /// `s3_key` with `{identifier}/{version}/{id}/plugin.wasm`.
    pub async fn set_artifact_key(pool: &Pool<Sqlite>, id: i64, s3_key: &str) -> Result<()> {
        let now = Utc::now().to_rfc3339();
        sqlx::query(
            "UPDATE plugins SET s3_key = ?, row_revision = row_revision + 1, updated_at = ? WHERE id = ?",
        )
        .bind(s3_key)
        .bind(&now)
        .bind(id)
        .execute(pool)
        .await?;
        Ok(())
    }

    /// Relative directory for a plugin artifact under the plugins root:
    /// `{identifier}/{version}/{id}`. The database `id` is the terminal
    /// disambiguator, so two rows that share `(identifier, version)` (e.g.
    /// after a soft-delete + re-import) still map to distinct on-disk
    /// locations.
    pub fn artifact_dir(identifier: &str, version: &str, id: i64) -> std::path::PathBuf {
        std::path::PathBuf::from(identifier)
            .join(version)
            .join(id.to_string())
    }

    /// Canonical artifact key: `{identifier}/{version}/{id}/plugin.wasm`.
    /// This is the single source of truth for the logical key stored in
    /// `plugins.s3_key` and for the on-disk file name.
    pub fn artifact_key(identifier: &str, version: &str, id: i64) -> String {
        format!("{identifier}/{version}/{id}/plugin.wasm")
    }

    /// Absolute WASM path for a plugin under `base_dir/plugins`:
    /// `{base_dir}/plugins/{identifier}/{version}/{id}/plugin.wasm`.
    pub fn artifact_wasm_path(
        base_dir: &std::path::Path,
        identifier: &str,
        version: &str,
        id: i64,
    ) -> std::path::PathBuf {
        base_dir
            .join("plugins")
            .join(Self::artifact_dir(identifier, version, id))
            .join("plugin.wasm")
    }

    /// Get the local WASM file path for this plugin
    pub fn wasm_path(&self, base_dir: &std::path::Path) -> std::path::PathBuf {
        Self::artifact_wasm_path(base_dir, &self.identifier, &self.version, self.id)
    }

    /// Ensure the plugin artifact directory exists and return it:
    /// `{base_dir}/plugins/{identifier}/{version}/{id}`.
    pub fn ensure_plugin_dir(
        base_dir: &std::path::Path,
        identifier: &str,
        version: &str,
        plugin_id: i64,
    ) -> Result<std::path::PathBuf> {
        let plugin_dir = base_dir
            .join("plugins")
            .join(Self::artifact_dir(identifier, version, plugin_id));
        std::fs::create_dir_all(&plugin_dir)?;
        Ok(plugin_dir)
    }

    /// Idempotently register an operation-owned artifact for protected GC.
    ///
    /// Ownership is derived only from the persisted Plugin tuple and its
    /// exact source operation. The current filesystem entry is deliberately
    /// never adopted here. If the database cannot prove the source identity,
    /// a `blocked` row is recorded so a worker cannot speculatively unlink a
    /// path. An existing row is idempotent only when its complete ownership
    /// tuple matches; conflicting ownership fails closed.
    pub async fn register_gc_artifact(
        pool: &Pool<Sqlite>,
        artifact_key: String,
        reason: String,
    ) -> Result<()> {
        let mut transaction = pool.begin().await?;
        let proven: Option<(String, i64, String, String)> = sqlx::query_as(
            "SELECT p.sha256, p.size_bytes, o.new_identity, o.operation_id \
             FROM plugins p \
             JOIN plugin_artifact_operations o \
               ON o.plugin_id = CAST(p.id AS TEXT) \
              AND o.new_s3_key = p.s3_key \
              AND o.new_sha256 = p.sha256 \
              AND o.new_size = p.size_bytes \
             WHERE p.s3_key = ? AND o.new_identity IS NOT NULL \
               AND o.state IN ('referenced','done') \
             ORDER BY o.updated_at DESC, o.operation_id DESC LIMIT 1",
        )
        .bind(&artifact_key)
        .fetch_optional(&mut *transaction)
        .await?;

        let plugin_tuple: Option<(String, i64)> = sqlx::query_as(
            "SELECT sha256, size_bytes FROM plugins WHERE s3_key = ? \
             ORDER BY id DESC LIMIT 1",
        )
        .bind(&artifact_key)
        .fetch_optional(&mut *transaction)
        .await?;

        let (
            expected_sha256,
            expected_size_bytes,
            expected_identity,
            source_operation_id,
            state,
            last_error,
        ) = match proven {
            Some((sha256, size_bytes, identity, operation_id)) => (
                Some(sha256),
                Some(size_bytes),
                Some(identity),
                Some(operation_id),
                "pending",
                None,
            ),
            None => (
                plugin_tuple.as_ref().map(|row| row.0.clone()),
                plugin_tuple.as_ref().map(|row| row.1),
                None,
                None,
                "blocked",
                Some("ownership_unproven".to_string()),
            ),
        };

        let existing: Option<(Option<String>, Option<i64>, Option<String>, Option<String>)> =
            sqlx::query_as(
                "SELECT expected_sha256, expected_size_bytes, expected_identity, \
                    source_operation_id \
             FROM plugin_artifact_gc WHERE artifact_key = ?",
            )
            .bind(&artifact_key)
            .fetch_optional(&mut *transaction)
            .await?;
        let ownership = (
            expected_sha256.clone(),
            expected_size_bytes,
            expected_identity.clone(),
            source_operation_id.clone(),
        );
        if let Some(existing) = existing {
            if existing != ownership {
                anyhow::bail!("conflict: plugin artifact GC ownership mismatch");
            }
            transaction.commit().await?;
            return Ok(());
        }

        let now = Utc::now().to_rfc3339();
        sqlx::query(
            "INSERT INTO plugin_artifact_gc (\
                 artifact_key, expected_sha256, expected_size_bytes, expected_identity, \
                 source_operation_id, attempts, last_error, created_at, updated_at, \
                 last_attempt_at, reason, state\
             ) VALUES (?, ?, ?, ?, ?, 0, ?, ?, ?, unixepoch(), ?, ?)",
        )
        .bind(&artifact_key)
        .bind(expected_sha256)
        .bind(expected_size_bytes)
        .bind(expected_identity)
        .bind(source_operation_id)
        .bind(last_error)
        .bind(&now)
        .bind(&now)
        .bind(&reason)
        .bind(state)
        .execute(&mut *transaction)
        .await?;
        transaction.commit().await?;
        Ok(())
    }

    /// Atomically mark a source `plugin_artifact_operations` row `done`.
    /// This is the "operation→done" half of the T078 ledger API; it is
    /// idempotent (a terminal `done`/`conflict` row is left as-is).
    pub async fn complete_operation(pool: &Pool<Sqlite>, operation_id: &str) -> Result<()> {
        sqlx::query(
            "UPDATE plugin_artifact_operations SET state = 'done' \
             WHERE operation_id = ? AND state NOT IN ('done', 'conflict')",
        )
        .bind(operation_id)
        .execute(pool)
        .await?;
        Ok(())
    }
}

impl sqlx::FromRow<'_, sqlx::sqlite::SqliteRow> for Plugin {
    fn from_row(row: &sqlx::sqlite::SqliteRow) -> Result<Self, sqlx::Error> {
        Ok(Plugin {
            id: row.try_get("id")?,
            identifier: row.try_get("identifier")?,
            name: row.try_get("name")?,
            description: row.try_get("description")?,
            manifest: row.try_get("manifest")?,
            runtime: row.try_get("runtime")?,
            version: row.try_get("version")?,
            author: row.try_get("author")?,
            repository_url: row.try_get("repository_url")?,
            s3_key: row.try_get("s3_key")?,
            sha256: row.try_get("sha256")?,
            size_bytes: row.try_get("size_bytes")?,
            category_id: row.try_get("category_id")?,
            capabilities: row.try_get("capabilities")?,
            resource_limits: row.try_get("resource_limits")?,
            row_revision: row.try_get("row_revision")?,
            created_at: row.try_get("created_at")?,
            updated_at: row.try_get("updated_at")?,
            deleted_at: row.try_get("deleted_at")?,
        })
    }
}

// === Legacy Function fixture/compatibility facade ===
// All validation, SQL, transactions, search, and reference policy live in the
// unique FunctionStore. Keep these signatures only while older fixtures and
// callers migrate to that public boundary.
impl Function {
    pub(crate) async fn list(
        pool: &Pool<Sqlite>,
        search: Option<String>,
        limit: i64,
        offset: i64,
    ) -> Result<Vec<Function>> {
        Ok(FunctionStore::new(pool.clone())?
            .list_window(search, limit, offset)
            .await?
            .into_iter()
            .map(|record| record.into_legacy_entity())
            .collect())
    }

    pub(crate) async fn count(pool: &Pool<Sqlite>, search: Option<String>) -> Result<i64> {
        Ok(FunctionStore::new(pool.clone())?
            .count_matching(search)
            .await?)
    }

    pub(crate) async fn get(pool: &Pool<Sqlite>, id: i64) -> Result<Option<Function>> {
        Ok(FunctionStore::new(pool.clone())?
            .get(id)
            .await?
            .map(|record| record.into_legacy_entity()))
    }

    pub(crate) async fn create(
        pool: &Pool<Sqlite>,
        identifier: String,
        name: String,
        description: Option<String>,
        kind: String,
        input_schema: String,
        output_schema: String,
        plugin_id: Option<i64>,
        plugin_export: Option<String>,
        category_id: Option<i64>,
        required_capabilities: Option<String>,
    ) -> Result<Function> {
        let input = legacy_function_input(
            identifier,
            name,
            description,
            kind,
            input_schema,
            output_schema,
            plugin_id,
            plugin_export,
            category_id,
            required_capabilities,
        )?;
        Ok(FunctionStore::new(pool.clone())?
            .create_legacy_fixture(input)
            .await?
            .into_legacy_entity())
    }

    pub(crate) async fn update(
        pool: &Pool<Sqlite>,
        id: i64,
        identifier: String,
        name: String,
        description: Option<String>,
        kind: String,
        input_schema: String,
        output_schema: String,
        plugin_id: Option<i64>,
        plugin_export: Option<String>,
        category_id: Option<i64>,
        required_capabilities: Option<String>,
    ) -> Result<Function> {
        let input = legacy_function_input(
            identifier,
            name,
            description,
            kind,
            input_schema,
            output_schema,
            plugin_id,
            plugin_export,
            category_id,
            required_capabilities,
        )?;
        Ok(FunctionStore::new(pool.clone())?
            .update_legacy_fixture(id, input)
            .await?
            .into_legacy_entity())
    }

    pub(crate) async fn delete(pool: &Pool<Sqlite>, id: i64) -> Result<()> {
        Ok(FunctionStore::new(pool.clone())?
            .delete_legacy_fixture(id)
            .await?)
    }
}

#[allow(clippy::too_many_arguments)]
fn legacy_function_input(
    identifier: String,
    name: String,
    description: Option<String>,
    kind: String,
    input_schema: String,
    output_schema: String,
    plugin_id: Option<i64>,
    plugin_export: Option<String>,
    category_id: Option<i64>,
    required_capabilities: Option<String>,
) -> Result<FunctionInput> {
    Ok(FunctionInput::for_write(
        identifier,
        name,
        description,
        FunctionKind::try_from(kind)?,
        input_schema,
        output_schema,
        plugin_id,
        plugin_export,
        category_id,
        required_capabilities,
    )?)
}

impl sqlx::FromRow<'_, sqlx::sqlite::SqliteRow> for Function {
    fn from_row(row: &sqlx::sqlite::SqliteRow) -> Result<Self, sqlx::Error> {
        Ok(Function {
            id: row.try_get("id")?,
            identifier: row.try_get("identifier")?,
            name: row.try_get("name")?,
            description: row.try_get("description")?,
            kind: row.try_get("kind")?,
            input_schema: row.try_get("input_schema")?,
            output_schema: row.try_get("output_schema")?,
            plugin_id: row.try_get("plugin_id")?,
            plugin_export: row.try_get("plugin_export")?,
            category_id: row.try_get("category_id")?,
            required_capabilities: row.try_get("required_capabilities")?,
            created_at: row.try_get("created_at")?,
            updated_at: row.try_get("updated_at")?,
        })
    }
}

// === Workflow CRUD ===
impl Workflow {
    pub async fn list(
        pool: &Pool<Sqlite>,
        search: Option<String>,
        limit: i64,
        offset: i64,
    ) -> Result<Vec<Workflow>> {
        let wfs = if let Some(s) = search {
            sqlx::query_as::<_, Workflow>(
                "SELECT id, identifier, name, description, timeout_ms, category_id, input_schema, start_description, output_schema, required_capabilities, created_at, updated_at FROM workflows WHERE name LIKE ? OR identifier LIKE ? ORDER BY name LIMIT ? OFFSET ?"
            ).bind(format!("%{}%", s)).bind(format!("%{}%", s)).bind(limit).bind(offset).fetch_all(pool).await?
        } else {
            sqlx::query_as::<_, Workflow>(
                "SELECT id, identifier, name, description, timeout_ms, category_id, input_schema, start_description, output_schema, required_capabilities, created_at, updated_at FROM workflows ORDER BY name LIMIT ? OFFSET ?"
            ).bind(limit).bind(offset).fetch_all(pool).await?
        };
        Ok(wfs)
    }

    pub async fn count(pool: &Pool<Sqlite>, search: Option<String>) -> Result<i64> {
        let c = if let Some(s) = search {
            sqlx::query_scalar::<_, i64>(
                "SELECT COUNT(*) FROM workflows WHERE name LIKE ? OR identifier LIKE ?",
            )
            .bind(format!("%{}%", s))
            .bind(format!("%{}%", s))
            .fetch_one(pool)
            .await?
        } else {
            sqlx::query_scalar::<_, i64>("SELECT COUNT(*) FROM workflows")
                .fetch_one(pool)
                .await?
        };
        Ok(c)
    }

    pub async fn get(pool: &Pool<Sqlite>, id: i64) -> Result<Option<Workflow>> {
        let w = sqlx::query_as::<_, Workflow>(
            "SELECT id, identifier, name, description, timeout_ms, category_id, input_schema, start_description, output_schema, required_capabilities, created_at, updated_at FROM workflows WHERE id = ?"
        ).bind(id).fetch_optional(pool).await?;
        Ok(w)
    }

    pub async fn create(
        pool: &Pool<Sqlite>,
        identifier: String,
        name: String,
        description: Option<String>,
        timeout_ms: i64,
        category_id: Option<i64>,
        input_schema: Option<String>,
        start_description: Option<String>,
        output_schema: Option<String>,
        required_capabilities: Option<String>,
    ) -> Result<Workflow> {
        validate_identifier(&identifier)?;
        validate_name(&name)?;
        if let Some(ref desc) = description {
            validate_description(desc)?;
        }
        if let Some(ref schema) = input_schema {
            validate_json(schema)?;
        }
        if let Some(ref schema) = output_schema {
            validate_json(schema)?;
        }

        let start = Instant::now();
        let now = Utc::now().to_rfc3339();
        let result = sqlx::query_scalar::<_, i64>(
            "INSERT INTO workflows (identifier, name, description, timeout_ms, category_id, input_schema, start_description, output_schema, required_capabilities, created_at, updated_at) VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?) RETURNING id"
        ).bind(&identifier).bind(&name).bind(&description).bind(timeout_ms).bind(category_id).bind(&input_schema).bind(&start_description).bind(&output_schema).bind(&required_capabilities).bind(&now).bind(&now)
            .fetch_one(pool).await;

        let id =
            result.map_err(|e| handle_unique_constraint_error(e, "identifier", &identifier))?;

        let duration = start.elapsed().as_millis();
        tracing::info!(entity = "workflow", op = "create", id = id, identifier = %identifier, name = %name, timeout_ms = timeout_ms, duration_ms = duration, "Workflow created");

        Workflow::get(pool, id)
            .await?
            .ok_or_else(|| anyhow::anyhow!("Failed to retrieve created workflow"))
    }

    pub async fn update(
        pool: &Pool<Sqlite>,
        id: i64,
        identifier: String,
        name: String,
        description: Option<String>,
        timeout_ms: i64,
        category_id: Option<i64>,
        input_schema: Option<String>,
        start_description: Option<String>,
        output_schema: Option<String>,
        required_capabilities: Option<String>,
    ) -> Result<Workflow> {
        validate_identifier(&identifier)?;
        validate_name(&name)?;
        if let Some(ref desc) = description {
            validate_description(desc)?;
        }
        if let Some(ref schema) = input_schema {
            validate_json(schema)?;
        }
        if let Some(ref schema) = output_schema {
            validate_json(schema)?;
        }

        let start = Instant::now();
        let now = Utc::now().to_rfc3339();
        let result = sqlx::query(
            "UPDATE workflows SET identifier=?, name=?, description=?, timeout_ms=?, category_id=?, input_schema=?, start_description=?, output_schema=?, required_capabilities=?, updated_at=? WHERE id=?"
        ).bind(&identifier).bind(&name).bind(&description).bind(timeout_ms).bind(category_id).bind(&input_schema).bind(&start_description).bind(&output_schema).bind(&required_capabilities).bind(&now).bind(id)
            .execute(pool).await;

        result.map_err(|e| handle_unique_constraint_error(e, "identifier", &identifier))?;

        let duration = start.elapsed().as_millis();
        tracing::info!(entity = "workflow", op = "update", id = id, identifier = %identifier, name = %name, duration_ms = duration, "Workflow updated");

        Workflow::get(pool, id)
            .await?
            .ok_or_else(|| anyhow::anyhow!("Workflow not found"))
    }

    pub async fn delete(pool: &Pool<Sqlite>, id: i64) -> Result<()> {
        let start = Instant::now();
        sqlx::query("DELETE FROM workflows WHERE id = ?")
            .bind(id)
            .execute(pool)
            .await?;

        let duration = start.elapsed().as_millis();
        tracing::info!(
            entity = "workflow",
            op = "delete",
            id = id,
            duration_ms = duration,
            "Workflow deleted"
        );

        Ok(())
    }

    /// Return the identifiers of all `Tool` rows referencing this
    /// workflow. Used by `WorkflowStore::delete` to enforce the
    /// `referenced_by_tool` RESTRICT boundary before a delete.
    pub async fn referenced_by_tools(pool: &Pool<Sqlite>, id: i64) -> Result<Vec<String>> {
        let refs = sqlx::query_scalar::<_, String>(
            "SELECT identifier FROM tools WHERE workflow_id = ? ORDER BY id",
        )
        .bind(id)
        .fetch_all(pool)
        .await?;
        Ok(refs)
    }
}

impl sqlx::FromRow<'_, sqlx::sqlite::SqliteRow> for Workflow {
    fn from_row(row: &sqlx::sqlite::SqliteRow) -> Result<Self, sqlx::Error> {
        Ok(Workflow {
            id: row.try_get("id")?,
            identifier: row.try_get("identifier")?,
            name: row.try_get("name")?,
            description: row.try_get("description")?,
            timeout_ms: row.try_get("timeout_ms")?,
            category_id: row.try_get("category_id")?,
            input_schema: row.try_get("input_schema")?,
            start_description: row.try_get("start_description")?,
            output_schema: row.try_get("output_schema")?,
            required_capabilities: row.try_get("required_capabilities")?,
            created_at: row.try_get("created_at")?,
            updated_at: row.try_get("updated_at")?,
        })
    }
}

impl sqlx::FromRow<'_, sqlx::sqlite::SqliteRow> for WorkflowNode {
    fn from_row(row: &sqlx::sqlite::SqliteRow) -> Result<Self, sqlx::Error> {
        Ok(WorkflowNode {
            id: row.try_get("id")?,
            workflow_id: row.try_get("workflow_id")?,
            node_key: row.try_get("node_key")?,
            node_type: row.try_get("node_type")?,
            function_id: row.try_get("function_id")?,
            position_x: row.try_get("position_x")?,
            position_y: row.try_get("position_y")?,
            node_config: row.try_get("node_config")?,
            created_at: row.try_get("created_at")?,
        })
    }
}

impl sqlx::FromRow<'_, sqlx::sqlite::SqliteRow> for WorkflowEdge {
    fn from_row(row: &sqlx::sqlite::SqliteRow) -> Result<Self, sqlx::Error> {
        Ok(WorkflowEdge {
            id: row.try_get("id")?,
            workflow_id: row.try_get("workflow_id")?,
            src_node_key: row.try_get("src_node_key")?,
            dst_node_key: row.try_get("dst_node_key")?,
            mapping: row.try_get("mapping")?,
        })
    }
}

// === WorkflowNode CRUD ===
impl WorkflowNode {
    pub async fn list_by_workflow(
        pool: &Pool<Sqlite>,
        workflow_id: i64,
    ) -> Result<Vec<WorkflowNode>> {
        let nodes = sqlx::query_as::<_, WorkflowNode>(
            "SELECT id, workflow_id, node_key, node_type, function_id, position_x, position_y, node_config, created_at FROM workflow_nodes WHERE workflow_id = ? ORDER BY id",
        )
        .bind(workflow_id)
        .fetch_all(pool)
        .await?;
        Ok(nodes)
    }

    pub async fn upsert(
        pool: &Pool<Sqlite>,
        workflow_id: i64,
        node_key: String,
        node_type: String,
        function_id: Option<i64>,
        position_x: f64,
        position_y: f64,
        node_config: Option<String>,
    ) -> Result<WorkflowNode> {
        // T095: reject legacy short node_type values; only the four
        // stable `*_node` strings are writable.
        const STABLE_NODE_TYPES: [&str; 4] = [
            "start_node",
            "end_node",
            "function_node",
            "generate_answer_node",
        ];
        if !STABLE_NODE_TYPES.contains(&node_type.as_str()) {
            return Err(anyhow::anyhow!("非法 node_type: {node_type}"));
        }

        let now = Utc::now().to_rfc3339();
        sqlx::query(
            r#"INSERT INTO workflow_nodes (workflow_id, node_key, node_type, function_id, position_x, position_y, node_config, created_at)
               VALUES (?, ?, ?, ?, ?, ?, ?, ?)
               ON CONFLICT(workflow_id, node_key) DO UPDATE SET
                   node_type=excluded.node_type, function_id=excluded.function_id,
                   position_x=excluded.position_x, position_y=excluded.position_y,
                   node_config=excluded.node_config"#,
        )
        .bind(workflow_id)
        .bind(&node_key)
        .bind(&node_type)
        .bind(function_id)
        .bind(position_x)
        .bind(position_y)
        .bind(&node_config)
        .bind(&now)
        .execute(pool)
        .await?;

        let node = sqlx::query_as::<_, WorkflowNode>(
            "SELECT id, workflow_id, node_key, node_type, function_id, position_x, position_y, node_config, created_at FROM workflow_nodes WHERE workflow_id = ? AND node_key = ?",
        )
        .bind(workflow_id)
        .bind(&node_key)
        .fetch_optional(pool)
        .await?;
        node.ok_or_else(|| anyhow::anyhow!("Failed to retrieve upserted node"))
    }

    pub async fn delete_by_workflow(pool: &Pool<Sqlite>, workflow_id: i64) -> Result<()> {
        sqlx::query("DELETE FROM workflow_nodes WHERE workflow_id = ?")
            .bind(workflow_id)
            .execute(pool)
            .await?;
        Ok(())
    }

    pub async fn delete_node(pool: &Pool<Sqlite>, workflow_id: i64, node_key: &str) -> Result<()> {
        sqlx::query("DELETE FROM workflow_nodes WHERE workflow_id = ? AND node_key = ?")
            .bind(workflow_id)
            .bind(node_key)
            .execute(pool)
            .await?;
        Ok(())
    }

    pub async fn update_position(
        pool: &Pool<Sqlite>,
        workflow_id: i64,
        node_key: &str,
        position_x: f64,
        position_y: f64,
    ) -> Result<()> {
        sqlx::query(
            "UPDATE workflow_nodes SET position_x = ?, position_y = ? WHERE workflow_id = ? AND node_key = ?",
        )
        .bind(position_x)
        .bind(position_y)
        .bind(workflow_id)
        .bind(node_key)
        .execute(pool)
        .await?;
        Ok(())
    }
}

// === WorkflowEdge CRUD ===
impl WorkflowEdge {
    pub async fn list_by_workflow(
        pool: &Pool<Sqlite>,
        workflow_id: i64,
    ) -> Result<Vec<WorkflowEdge>> {
        let edges = sqlx::query_as::<_, WorkflowEdge>(
            "SELECT id, workflow_id, src_node_key, dst_node_key, mapping FROM workflow_edges WHERE workflow_id = ? ORDER BY id",
        )
        .bind(workflow_id)
        .fetch_all(pool)
        .await?;
        Ok(edges)
    }

    pub async fn upsert(
        pool: &Pool<Sqlite>,
        workflow_id: i64,
        src_node_key: String,
        dst_node_key: String,
        mapping: String,
    ) -> Result<WorkflowEdge> {
        sqlx::query(
            r#"INSERT INTO workflow_edges (workflow_id, src_node_key, dst_node_key, mapping)
               VALUES (?, ?, ?, ?)
               ON CONFLICT(workflow_id, src_node_key, dst_node_key) DO UPDATE SET
                   mapping=excluded.mapping"#,
        )
        .bind(workflow_id)
        .bind(&src_node_key)
        .bind(&dst_node_key)
        .bind(&mapping)
        .execute(pool)
        .await?;

        let edge = sqlx::query_as::<_, WorkflowEdge>(
            "SELECT id, workflow_id, src_node_key, dst_node_key, mapping FROM workflow_edges WHERE workflow_id = ? AND src_node_key = ? AND dst_node_key = ?",
        )
        .bind(workflow_id)
        .bind(&src_node_key)
        .bind(&dst_node_key)
        .fetch_optional(pool)
        .await?;
        edge.ok_or_else(|| anyhow::anyhow!("Failed to retrieve upserted edge"))
    }

    pub async fn delete_by_workflow(pool: &Pool<Sqlite>, workflow_id: i64) -> Result<()> {
        sqlx::query("DELETE FROM workflow_edges WHERE workflow_id = ?")
            .bind(workflow_id)
            .execute(pool)
            .await?;
        Ok(())
    }

    pub async fn delete_edge(
        pool: &Pool<Sqlite>,
        workflow_id: i64,
        src_node_key: &str,
        dst_node_key: &str,
    ) -> Result<()> {
        sqlx::query(
            "DELETE FROM workflow_edges WHERE workflow_id = ? AND src_node_key = ? AND dst_node_key = ?",
        )
        .bind(workflow_id)
        .bind(src_node_key)
        .bind(dst_node_key)
        .execute(pool)
        .await?;
        Ok(())
    }

    pub async fn delete_edges_by_node(
        pool: &Pool<Sqlite>,
        workflow_id: i64,
        node_key: &str,
    ) -> Result<()> {
        sqlx::query(
            "DELETE FROM workflow_edges WHERE workflow_id = ? AND (src_node_key = ? OR dst_node_key = ?)",
        )
        .bind(workflow_id)
        .bind(node_key)
        .bind(node_key)
        .execute(pool)
        .await?;
        Ok(())
    }
}

// === Tool CRUD ===
impl Tool {
    pub async fn list(
        pool: &Pool<Sqlite>,
        search: Option<String>,
        limit: i64,
        offset: i64,
    ) -> Result<Vec<Tool>> {
        let tools = if let Some(s) = search {
            sqlx::query_as::<_, Tool>(
                "SELECT id, identifier, name, description, kind, source, is_always, function_id, workflow_id, input_schema, output_schema, category_id, required_capabilities, created_at, updated_at FROM tools WHERE name LIKE ? OR identifier LIKE ? ORDER BY name LIMIT ? OFFSET ?"
            ).bind(format!("%{}%", s)).bind(format!("%{}%", s)).bind(limit).bind(offset).fetch_all(pool).await?
        } else {
            sqlx::query_as::<_, Tool>(
                "SELECT id, identifier, name, description, kind, source, is_always, function_id, workflow_id, input_schema, output_schema, category_id, required_capabilities, created_at, updated_at FROM tools ORDER BY name LIMIT ? OFFSET ?"
            ).bind(limit).bind(offset).fetch_all(pool).await?
        };
        Ok(tools)
    }

    pub async fn count(pool: &Pool<Sqlite>, search: Option<String>) -> Result<i64> {
        let c = if let Some(s) = search {
            sqlx::query_scalar::<_, i64>(
                "SELECT COUNT(*) FROM tools WHERE name LIKE ? OR identifier LIKE ?",
            )
            .bind(format!("%{}%", s))
            .bind(format!("%{}%", s))
            .fetch_one(pool)
            .await?
        } else {
            sqlx::query_scalar::<_, i64>("SELECT COUNT(*) FROM tools")
                .fetch_one(pool)
                .await?
        };
        Ok(c)
    }

    pub async fn get(pool: &Pool<Sqlite>, id: i64) -> Result<Option<Tool>> {
        let t = sqlx::query_as::<_, Tool>(
            "SELECT id, identifier, name, description, kind, source, is_always, function_id, workflow_id, input_schema, output_schema, category_id, required_capabilities, created_at, updated_at FROM tools WHERE id = ?"
        ).bind(id).fetch_optional(pool).await?;
        Ok(t)
    }

    pub async fn create(
        pool: &Pool<Sqlite>,
        identifier: String,
        name: String,
        description: String,
        kind: String,
        source: String,
        is_always: bool,
        function_id: Option<i64>,
        workflow_id: Option<i64>,
        input_schema: String,
        output_schema: String,
        category_id: Option<i64>,
        required_capabilities: Option<String>,
    ) -> Result<Tool> {
        validate_identifier(&identifier)?;
        validate_name(&name)?;
        validate_description(&description)?;
        validate_json(&input_schema)?;
        validate_json(&output_schema)?;

        // 应用层 CHECK 约束验证
        if kind == "function-wrap" && function_id.is_none() {
            return Err(anyhow::anyhow!("kind=function 时 function_id 不能为空"));
        }
        if kind == "workflow-wrap" && workflow_id.is_none() {
            return Err(anyhow::anyhow!("kind=workflow 时 workflow_id 不能为空"));
        }

        let start = Instant::now();
        let now = Utc::now().to_rfc3339();
        let result = sqlx::query_scalar::<_, i64>(
            "INSERT INTO tools (identifier, name, description, kind, source, is_always, function_id, workflow_id, input_schema, output_schema, category_id, required_capabilities, created_at, updated_at) VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?) RETURNING id"
        ).bind(&identifier).bind(&name).bind(&description).bind(&kind).bind(&source).bind(is_always as i64).bind(function_id).bind(workflow_id).bind(&input_schema).bind(&output_schema).bind(category_id).bind(&required_capabilities).bind(&now).bind(&now)
            .fetch_one(pool).await;

        let id =
            result.map_err(|e| handle_unique_constraint_error(e, "identifier", &identifier))?;

        let duration = start.elapsed().as_millis();
        tracing::info!(entity = "tool", op = "create", id = id, identifier = %identifier, name = %name, kind = %kind, duration_ms = duration, "Tool created");

        Tool::get(pool, id)
            .await?
            .ok_or_else(|| anyhow::anyhow!("Failed to retrieve created tool"))
    }

    pub async fn update(
        pool: &Pool<Sqlite>,
        id: i64,
        identifier: String,
        name: String,
        description: String,
        kind: String,
        source: String,
        is_always: bool,
        function_id: Option<i64>,
        workflow_id: Option<i64>,
        input_schema: String,
        output_schema: String,
        category_id: Option<i64>,
        required_capabilities: Option<String>,
    ) -> Result<Tool> {
        validate_identifier(&identifier)?;
        validate_name(&name)?;
        validate_description(&description)?;
        validate_json(&input_schema)?;
        validate_json(&output_schema)?;

        // 应用层 CHECK 约束验证
        if kind == "function-wrap" && function_id.is_none() {
            return Err(anyhow::anyhow!("kind=function 时 function_id 不能为空"));
        }
        if kind == "workflow-wrap" && workflow_id.is_none() {
            return Err(anyhow::anyhow!("kind=workflow 时 workflow_id 不能为空"));
        }

        let start = Instant::now();
        let now = Utc::now().to_rfc3339();
        let result = sqlx::query(
            "UPDATE tools SET identifier=?, name=?, description=?, kind=?, source=?, is_always=?, function_id=?, workflow_id=?, input_schema=?, output_schema=?, category_id=?, required_capabilities=?, updated_at=? WHERE id=?"
        ).bind(&identifier).bind(&name).bind(&description).bind(&kind).bind(&source).bind(is_always as i64).bind(function_id).bind(workflow_id).bind(&input_schema).bind(&output_schema).bind(category_id).bind(&required_capabilities).bind(&now).bind(id)
            .execute(pool).await;

        result.map_err(|e| handle_unique_constraint_error(e, "identifier", &identifier))?;

        let duration = start.elapsed().as_millis();
        tracing::info!(entity = "tool", op = "update", id = id, identifier = %identifier, name = %name, duration_ms = duration, "Tool updated");

        Tool::get(pool, id)
            .await?
            .ok_or_else(|| anyhow::anyhow!("Tool not found"))
    }

    pub async fn delete(pool: &Pool<Sqlite>, id: i64) -> Result<()> {
        let start = Instant::now();
        sqlx::query("DELETE FROM tools WHERE id = ?")
            .bind(id)
            .execute(pool)
            .await?;

        let duration = start.elapsed().as_millis();
        tracing::info!(
            entity = "tool",
            op = "delete",
            id = id,
            duration_ms = duration,
            "Tool deleted"
        );

        Ok(())
    }
}

impl sqlx::FromRow<'_, sqlx::sqlite::SqliteRow> for Tool {
    fn from_row(row: &sqlx::sqlite::SqliteRow) -> Result<Self, sqlx::Error> {
        Ok(Tool {
            id: row.try_get("id")?,
            identifier: row.try_get("identifier")?,
            name: row.try_get("name")?,
            description: row.try_get("description")?,
            kind: row.try_get("kind")?,
            source: row.try_get("source")?,
            is_always: row.try_get::<i64, _>("is_always")? != 0,
            function_id: row.try_get("function_id")?,
            workflow_id: row.try_get("workflow_id")?,
            input_schema: row.try_get("input_schema")?,
            output_schema: row.try_get("output_schema")?,
            category_id: row.try_get("category_id")?,
            required_capabilities: row.try_get("required_capabilities")?,
            created_at: row.try_get("created_at")?,
            updated_at: row.try_get("updated_at")?,
        })
    }
}

// === Skill CRUD ===
impl Skill {
    pub async fn list(
        pool: &Pool<Sqlite>,
        search: Option<String>,
        limit: i64,
        offset: i64,
    ) -> Result<Vec<Skill>> {
        let skills = if let Some(s) = search {
            sqlx::query_as::<_, Skill>(
                "SELECT id, identifier, name, description, frontmatter, content, source, is_always, category_id, required_capabilities, created_at, updated_at FROM skills WHERE name LIKE ? OR identifier LIKE ? ORDER BY name LIMIT ? OFFSET ?"
            ).bind(format!("%{}%", s)).bind(format!("%{}%", s)).bind(limit).bind(offset).fetch_all(pool).await?
        } else {
            sqlx::query_as::<_, Skill>(
                "SELECT id, identifier, name, description, frontmatter, content, source, is_always, category_id, required_capabilities, created_at, updated_at FROM skills ORDER BY name LIMIT ? OFFSET ?"
            ).bind(limit).bind(offset).fetch_all(pool).await?
        };
        Ok(skills)
    }

    pub async fn count(pool: &Pool<Sqlite>, search: Option<String>) -> Result<i64> {
        let c = if let Some(s) = search {
            sqlx::query_scalar::<_, i64>(
                "SELECT COUNT(*) FROM skills WHERE name LIKE ? OR identifier LIKE ?",
            )
            .bind(format!("%{}%", s))
            .bind(format!("%{}%", s))
            .fetch_one(pool)
            .await?
        } else {
            sqlx::query_scalar::<_, i64>("SELECT COUNT(*) FROM skills")
                .fetch_one(pool)
                .await?
        };
        Ok(c)
    }

    pub async fn get(pool: &Pool<Sqlite>, id: i64) -> Result<Option<Skill>> {
        let s = sqlx::query_as::<_, Skill>(
            "SELECT id, identifier, name, description, frontmatter, content, source, is_always, category_id, required_capabilities, created_at, updated_at FROM skills WHERE id = ?"
        ).bind(id).fetch_optional(pool).await?;
        Ok(s)
    }

    pub async fn create(
        pool: &Pool<Sqlite>,
        identifier: String,
        name: String,
        description: String,
        frontmatter: Option<String>,
        content: String,
        source: String,
        is_always: bool,
        category_id: Option<i64>,
        required_capabilities: Option<String>,
    ) -> Result<Skill> {
        validate_identifier(&identifier)?;
        validate_name(&name)?;
        validate_description(&description)?;
        validate_content(&content)?;
        if let Some(ref fm) = frontmatter {
            validate_json(fm)?;
        }

        let start = Instant::now();
        let now = Utc::now().to_rfc3339();
        let result = sqlx::query_scalar::<_, i64>(
            "INSERT INTO skills (identifier, name, description, frontmatter, content, source, is_always, category_id, required_capabilities, created_at, updated_at) VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?) RETURNING id"
        ).bind(&identifier).bind(&name).bind(&description).bind(&frontmatter).bind(&content).bind(&source).bind(is_always as i64).bind(category_id).bind(&required_capabilities).bind(&now).bind(&now)
            .fetch_one(pool).await;

        let id =
            result.map_err(|e| handle_unique_constraint_error(e, "identifier", &identifier))?;

        let duration = start.elapsed().as_millis();
        tracing::info!(entity = "skill", op = "create", id = id, identifier = %identifier, name = %name, source = %source, duration_ms = duration, "Skill created");

        Skill::get(pool, id)
            .await?
            .ok_or_else(|| anyhow::anyhow!("Failed to retrieve created skill"))
    }

    pub async fn update(
        pool: &Pool<Sqlite>,
        id: i64,
        identifier: String,
        name: String,
        description: String,
        frontmatter: Option<String>,
        content: String,
        source: String,
        is_always: bool,
        category_id: Option<i64>,
        required_capabilities: Option<String>,
    ) -> Result<Skill> {
        validate_identifier(&identifier)?;
        validate_name(&name)?;
        validate_description(&description)?;
        validate_content(&content)?;
        if let Some(ref fm) = frontmatter {
            validate_json(fm)?;
        }

        let start = Instant::now();
        let now = Utc::now().to_rfc3339();
        let result = sqlx::query(
            "UPDATE skills SET identifier=?, name=?, description=?, frontmatter=?, content=?, source=?, is_always=?, category_id=?, required_capabilities=?, updated_at=? WHERE id=?"
        ).bind(&identifier).bind(&name).bind(&description).bind(&frontmatter).bind(&content).bind(&source).bind(is_always as i64).bind(category_id).bind(&required_capabilities).bind(&now).bind(id)
            .execute(pool).await;

        result.map_err(|e| handle_unique_constraint_error(e, "identifier", &identifier))?;

        let duration = start.elapsed().as_millis();
        tracing::info!(entity = "skill", op = "update", id = id, identifier = %identifier, name = %name, duration_ms = duration, "Skill updated");

        Skill::get(pool, id)
            .await?
            .ok_or_else(|| anyhow::anyhow!("Skill not found"))
    }

    pub async fn delete(pool: &Pool<Sqlite>, id: i64) -> Result<()> {
        let start = Instant::now();
        sqlx::query("DELETE FROM skills WHERE id = ?")
            .bind(id)
            .execute(pool)
            .await?;

        let duration = start.elapsed().as_millis();
        tracing::info!(
            entity = "skill",
            op = "delete",
            id = id,
            duration_ms = duration,
            "Skill deleted"
        );

        Ok(())
    }
}

impl sqlx::FromRow<'_, sqlx::sqlite::SqliteRow> for Skill {
    fn from_row(row: &sqlx::sqlite::SqliteRow) -> Result<Self, sqlx::Error> {
        Ok(Skill {
            id: row.try_get("id")?,
            identifier: row.try_get("identifier")?,
            name: row.try_get("name")?,
            description: row.try_get("description")?,
            frontmatter: row.try_get("frontmatter")?,
            content: row.try_get("content")?,
            source: row.try_get("source")?,
            is_always: row.try_get::<i64, _>("is_always")? != 0,
            category_id: row.try_get("category_id")?,
            required_capabilities: row.try_get("required_capabilities")?,
            created_at: row.try_get("created_at")?,
            updated_at: row.try_get("updated_at")?,
        })
    }
}

// === Agent CRUD ===
impl Agent {
    pub async fn list(
        pool: &Pool<Sqlite>,
        search: Option<String>,
        limit: i64,
        offset: i64,
    ) -> Result<Vec<Agent>> {
        let agents = if let Some(s) = search {
            sqlx::query_as::<_, Agent>(
                "SELECT id, identifier, name, description, system_prompt, parent_agent_id, depth, is_default, model_preset, created_at, updated_at FROM agents WHERE name LIKE ? OR identifier LIKE ? ORDER BY name LIMIT ? OFFSET ?"
            ).bind(format!("%{}%", s)).bind(format!("%{}%", s)).bind(limit).bind(offset).fetch_all(pool).await?
        } else {
            sqlx::query_as::<_, Agent>(
                "SELECT id, identifier, name, description, system_prompt, parent_agent_id, depth, is_default, model_preset, created_at, updated_at FROM agents ORDER BY name LIMIT ? OFFSET ?"
            ).bind(limit).bind(offset).fetch_all(pool).await?
        };
        Ok(agents)
    }

    pub async fn count(pool: &Pool<Sqlite>, search: Option<String>) -> Result<i64> {
        let c = if let Some(s) = search {
            sqlx::query_scalar::<_, i64>(
                "SELECT COUNT(*) FROM agents WHERE name LIKE ? OR identifier LIKE ?",
            )
            .bind(format!("%{}%", s))
            .bind(format!("%{}%", s))
            .fetch_one(pool)
            .await?
        } else {
            sqlx::query_scalar::<_, i64>("SELECT COUNT(*) FROM agents")
                .fetch_one(pool)
                .await?
        };
        Ok(c)
    }

    pub async fn get(pool: &Pool<Sqlite>, id: i64) -> Result<Option<Agent>> {
        let a = sqlx::query_as::<_, Agent>(
            "SELECT id, identifier, name, description, system_prompt, parent_agent_id, depth, is_default, model_preset, created_at, updated_at FROM agents WHERE id = ?"
        ).bind(id).fetch_optional(pool).await?;
        Ok(a)
    }

    /// 检测循环引用：从 parent_agent_id 向上遍历，若遇到 current_id 则存在循环
    async fn detect_cycle(
        pool: &Pool<Sqlite>,
        current_id: Option<i64>,
        parent_id: Option<i64>,
    ) -> Result<()> {
        if let (Some(cid), Some(pid)) = (current_id, parent_id) {
            if cid == pid {
                return Err(anyhow::anyhow!(
                    "不允许形成循环引用：Agent 不能以自身为父级"
                ));
            }

            let mut check_id: Option<i64> = Some(pid);
            let mut visited = std::collections::HashSet::new();
            visited.insert(cid);

            while let Some(parent) = check_id {
                if visited.contains(&parent) {
                    return Err(anyhow::anyhow!("不允许形成循环引用：检测到 Agent 层级循环"));
                }
                visited.insert(parent);

                let next: Option<i64> =
                    sqlx::query_scalar("SELECT parent_agent_id FROM agents WHERE id = ?")
                        .bind(parent)
                        .fetch_optional(pool)
                        .await?
                        .flatten();

                check_id = next;
            }
        }
        Ok(())
    }

    pub async fn create(
        pool: &Pool<Sqlite>,
        identifier: String,
        name: String,
        description: Option<String>,
        system_prompt: String,
        parent_agent_id: Option<i64>,
        depth: i64,
        model_preset: Option<String>,
    ) -> Result<Agent> {
        validate_identifier(&identifier)?;
        validate_name(&name)?;
        if let Some(ref desc) = description {
            validate_description(desc)?;
        }

        // 检测循环引用（创建时 current_id 为 None，因为还未生成 ID）
        // 这里只做基本的自引用检测
        if let Some(pid) = parent_agent_id
            && pid <= 0
        {
            return Err(anyhow::anyhow!("parent_agent_id 必须为正整数"));
        }

        let start = Instant::now();
        let now = Utc::now().to_rfc3339();
        let result = sqlx::query_scalar::<_, i64>(
            "INSERT INTO agents (identifier, name, description, system_prompt, parent_agent_id, depth, model_preset, created_at, updated_at) VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?) RETURNING id"
        ).bind(&identifier).bind(&name).bind(&description).bind(&system_prompt).bind(parent_agent_id).bind(depth).bind(&model_preset).bind(&now).bind(&now)
            .fetch_one(pool).await;

        let id =
            result.map_err(|e| handle_unique_constraint_error(e, "identifier", &identifier))?;

        // 创建后检测循环引用
        Self::detect_cycle(pool, Some(id), parent_agent_id).await?;

        let duration = start.elapsed().as_millis();
        tracing::info!(entity = "agent", op = "create", id = id, identifier = %identifier, name = %name, depth = depth, duration_ms = duration, "Agent created");

        Agent::get(pool, id)
            .await?
            .ok_or_else(|| anyhow::anyhow!("Failed to retrieve created agent"))
    }

    pub async fn update(
        pool: &Pool<Sqlite>,
        id: i64,
        identifier: String,
        name: String,
        description: Option<String>,
        system_prompt: String,
        parent_agent_id: Option<i64>,
        depth: i64,
        model_preset: Option<String>,
    ) -> Result<Agent> {
        validate_identifier(&identifier)?;
        validate_name(&name)?;
        if let Some(ref desc) = description {
            validate_description(desc)?;
        }

        // 更新前检测循环引用
        Self::detect_cycle(pool, Some(id), parent_agent_id).await?;

        let start = Instant::now();
        let now = Utc::now().to_rfc3339();
        let result = sqlx::query(
            "UPDATE agents SET identifier=?, name=?, description=?, system_prompt=?, parent_agent_id=?, depth=?, model_preset=?, updated_at=? WHERE id=?"
        ).bind(&identifier).bind(&name).bind(&description).bind(&system_prompt).bind(parent_agent_id).bind(depth).bind(&model_preset).bind(&now).bind(id)
            .execute(pool).await;

        result.map_err(|e| handle_unique_constraint_error(e, "identifier", &identifier))?;

        let duration = start.elapsed().as_millis();
        tracing::info!(entity = "agent", op = "update", id = id, identifier = %identifier, name = %name, duration_ms = duration, "Agent updated");

        Agent::get(pool, id)
            .await?
            .ok_or_else(|| anyhow::anyhow!("Agent not found"))
    }

    pub async fn delete(pool: &Pool<Sqlite>, id: i64) -> Result<()> {
        let start = Instant::now();

        // 删除前将子 Agent 的 parent_agent_id 置为 NULL
        sqlx::query("UPDATE agents SET parent_agent_id = NULL WHERE parent_agent_id = ?")
            .bind(id)
            .execute(pool)
            .await?;

        sqlx::query("DELETE FROM agents WHERE id = ?")
            .bind(id)
            .execute(pool)
            .await?;

        let duration = start.elapsed().as_millis();
        tracing::info!(
            entity = "agent",
            op = "delete",
            id = id,
            duration_ms = duration,
            "Agent deleted"
        );

        Ok(())
    }
}

impl sqlx::FromRow<'_, sqlx::sqlite::SqliteRow> for Agent {
    fn from_row(row: &sqlx::sqlite::SqliteRow) -> Result<Self, sqlx::Error> {
        let is_default_i64: i64 = row.try_get("is_default")?;
        Ok(Agent {
            id: row.try_get("id")?,
            identifier: row.try_get("identifier")?,
            name: row.try_get("name")?,
            description: row.try_get("description")?,
            system_prompt: row.try_get("system_prompt")?,
            parent_agent_id: row.try_get("parent_agent_id")?,
            depth: row.try_get("depth")?,
            is_default: is_default_i64 != 0,
            model_preset: row.try_get("model_preset")?,
            created_at: row.try_get("created_at")?,
            updated_at: row.try_get("updated_at")?,
        })
    }
}

/// FR-026: 数据导入功能 - 从 JSON 备份文件导入数据
pub async fn import_from_backup(pool: &Pool<Sqlite>, input_path: &str) -> Result<()> {
    let start = Instant::now();

    // 读取文件
    let json_str = tokio::fs::read_to_string(input_path).await?;
    let import_data: serde_json::Value = serde_json::from_str(&json_str)?;

    // 验证版本
    let version = import_data
        .get("version")
        .and_then(|v| v.as_str())
        .ok_or_else(|| anyhow::anyhow!("备份文件缺少 version 字段"))?;

    if version != "1.0" {
        return Err(anyhow::anyhow!("不支持的备份文件版本: {}", version));
    }

    let entities = import_data
        .get("entities")
        .ok_or_else(|| anyhow::anyhow!("备份文件缺少 entities 字段"))?;

    // 开始事务
    let mut tx = pool.begin().await?;

    // 按依赖顺序导入（先导入被引用的实体）
    // 1. 导入 tags
    if let Some(tags) = entities.get("tags").and_then(|v| v.as_array()) {
        sqlx::query("DELETE FROM tags").execute(&mut *tx).await?;
        for tag in tags {
            let id = tag.get("id").and_then(|v| v.as_i64()).unwrap_or(0);
            let name = tag.get("name").and_then(|v| v.as_str()).unwrap_or("");
            let color = tag.get("color").and_then(|v| v.as_str());
            let created_at = tag.get("created_at").and_then(|v| v.as_str()).unwrap_or("");

            sqlx::query("INSERT INTO tags (id, name, color, created_at) VALUES (?, ?, ?, ?)")
                .bind(id)
                .bind(name)
                .bind(color)
                .bind(created_at)
                .execute(&mut *tx)
                .await?;
        }
        tracing::info!(count = tags.len(), "已导入 tags");
    }

    // 2. 导入 categories
    if let Some(categories) = entities.get("categories").and_then(|v| v.as_array()) {
        sqlx::query("DELETE FROM categories")
            .execute(&mut *tx)
            .await?;
        for cat in categories {
            let id = cat.get("id").and_then(|v| v.as_i64()).unwrap_or(0);
            let parent_id = cat.get("parent_id").and_then(|v| v.as_i64());
            let name = cat.get("name").and_then(|v| v.as_str()).unwrap_or("");
            let slug = cat.get("slug").and_then(|v| v.as_str()).unwrap_or("");
            let description = cat.get("description").and_then(|v| v.as_str());
            let created_at = cat.get("created_at").and_then(|v| v.as_str()).unwrap_or("");
            let updated_at = cat.get("updated_at").and_then(|v| v.as_str()).unwrap_or("");

            sqlx::query("INSERT INTO categories (id, parent_id, name, slug, description, created_at, updated_at) VALUES (?, ?, ?, ?, ?, ?, ?)")
                .bind(id).bind(parent_id).bind(name).bind(slug).bind(description).bind(created_at).bind(updated_at)
                .execute(&mut *tx).await?;
        }
        tracing::info!(count = categories.len(), "已导入 categories");
    }

    // 3. 导入 capabilities
    if let Some(capabilities) = entities.get("capabilities").and_then(|v| v.as_array()) {
        sqlx::query("DELETE FROM capabilities")
            .execute(&mut *tx)
            .await?;
        for cap in capabilities {
            let name = cap.get("name").and_then(|v| v.as_str()).unwrap_or("");
            let description = cap
                .get("description")
                .and_then(|v| v.as_str())
                .unwrap_or("");
            let is_dangerous = cap
                .get("is_dangerous")
                .and_then(|v| v.as_i64())
                .unwrap_or(0);
            let category_id = cap.get("category_id").and_then(|v| v.as_i64());
            let created_at = cap.get("created_at").and_then(|v| v.as_str()).unwrap_or("");

            sqlx::query("INSERT INTO capabilities (name, description, is_dangerous, category_id, created_at) VALUES (?, ?, ?, ?, ?)")
                .bind(name).bind(description).bind(is_dangerous).bind(category_id).bind(created_at)
                .execute(&mut *tx).await?;
        }
        tracing::info!(count = capabilities.len(), "已导入 capabilities");
    }

    // 4. 导入 plugins
    if let Some(plugins) = entities.get("plugins").and_then(|v| v.as_array()) {
        sqlx::query("DELETE FROM plugins").execute(&mut *tx).await?;
        for plugin in plugins {
            let id = plugin.get("id").and_then(|v| v.as_i64()).unwrap_or(0);
            let identifier = plugin
                .get("identifier")
                .and_then(|v| v.as_str())
                .unwrap_or("");
            let name = plugin.get("name").and_then(|v| v.as_str()).unwrap_or("");
            let description = plugin.get("description").and_then(|v| v.as_str());
            let manifest = plugin.get("manifest").and_then(|v| v.as_str());
            let runtime = plugin.get("runtime").and_then(|v| v.as_str()).unwrap_or("");
            let version = plugin.get("version").and_then(|v| v.as_str()).unwrap_or("");
            let author = plugin.get("author").and_then(|v| v.as_str());
            let repository_url = plugin.get("repository_url").and_then(|v| v.as_str());
            let s3_key = plugin.get("s3_key").and_then(|v| v.as_str()).unwrap_or("");
            let sha256 = plugin.get("sha256").and_then(|v| v.as_str()).unwrap_or("");
            let size_bytes = plugin
                .get("size_bytes")
                .and_then(|v| v.as_i64())
                .unwrap_or(0);
            let category_id = plugin.get("category_id").and_then(|v| v.as_i64());
            let created_at = plugin
                .get("created_at")
                .and_then(|v| v.as_str())
                .unwrap_or("");
            let updated_at = plugin
                .get("updated_at")
                .and_then(|v| v.as_str())
                .unwrap_or("");
            let deleted_at = plugin.get("deleted_at").and_then(|v| v.as_str());

            sqlx::query("INSERT INTO plugins (id, identifier, name, description, manifest, runtime, version, author, repository_url, s3_key, sha256, size_bytes, category_id, created_at, updated_at, deleted_at) VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?)")
                .bind(id).bind(identifier).bind(name).bind(description).bind(manifest).bind(runtime).bind(version).bind(author).bind(repository_url).bind(s3_key).bind(sha256).bind(size_bytes).bind(category_id).bind(created_at).bind(updated_at).bind(deleted_at)
                .execute(&mut *tx).await?;
        }
        tracing::info!(count = plugins.len(), "已导入 plugins");
    }

    // 5. 导入 functions
    if let Some(functions) = entities.get("functions").and_then(|v| v.as_array()) {
        sqlx::query("DELETE FROM functions")
            .execute(&mut *tx)
            .await?;
        for func in functions {
            let id = func.get("id").and_then(|v| v.as_i64()).unwrap_or(0);
            let identifier = func
                .get("identifier")
                .and_then(|v| v.as_str())
                .unwrap_or("");
            let name = func.get("name").and_then(|v| v.as_str()).unwrap_or("");
            let description = func.get("description").and_then(|v| v.as_str());
            let kind = func
                .get("kind")
                .and_then(|v| v.as_str())
                .unwrap_or("builtin")
                .to_string();
            let input_schema = func
                .get("input_schema")
                .and_then(|v| v.as_str())
                .unwrap_or("{}");
            let output_schema = func
                .get("output_schema")
                .and_then(|v| v.as_str())
                .unwrap_or("{}");
            let plugin_id = func.get("plugin_id").and_then(|v| v.as_i64());
            let plugin_export = func.get("plugin_export").and_then(|v| v.as_str());
            let category_id = func.get("category_id").and_then(|v| v.as_i64());
            let required_capabilities = func.get("required_capabilities").and_then(|v| v.as_str());
            let created_at = func
                .get("created_at")
                .and_then(|v| v.as_str())
                .unwrap_or("");
            let updated_at = func
                .get("updated_at")
                .and_then(|v| v.as_str())
                .unwrap_or("");

            sqlx::query("INSERT INTO functions (id, identifier, name, description, kind, input_schema, output_schema, plugin_id, plugin_export, category_id, required_capabilities, created_at, updated_at) VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?)")
                .bind(id).bind(identifier).bind(name).bind(description).bind(kind).bind(input_schema).bind(output_schema).bind(plugin_id).bind(plugin_export).bind(category_id).bind(required_capabilities).bind(created_at).bind(updated_at)
                .execute(&mut *tx).await?;
        }
        tracing::info!(count = functions.len(), "已导入 functions");
    }

    // 6. 导入 workflows
    if let Some(workflows) = entities.get("workflows").and_then(|v| v.as_array()) {
        sqlx::query("DELETE FROM workflows")
            .execute(&mut *tx)
            .await?;
        for wf in workflows {
            let id = wf.get("id").and_then(|v| v.as_i64()).unwrap_or(0);
            let identifier = wf.get("identifier").and_then(|v| v.as_str()).unwrap_or("");
            let name = wf.get("name").and_then(|v| v.as_str()).unwrap_or("");
            let description = wf.get("description").and_then(|v| v.as_str());
            let timeout_ms = wf
                .get("timeout_ms")
                .and_then(|v| v.as_i64())
                .unwrap_or(30000);
            let category_id = wf.get("category_id").and_then(|v| v.as_i64());
            let input_schema = wf.get("input_schema").and_then(|v| v.as_str());
            let start_description = wf.get("start_description").and_then(|v| v.as_str());
            let output_schema = wf.get("output_schema").and_then(|v| v.as_str());
            let required_capabilities = wf.get("required_capabilities").and_then(|v| v.as_str());
            let created_at = wf.get("created_at").and_then(|v| v.as_str()).unwrap_or("");
            let updated_at = wf.get("updated_at").and_then(|v| v.as_str()).unwrap_or("");

            sqlx::query("INSERT INTO workflows (id, identifier, name, description, timeout_ms, category_id, input_schema, start_description, output_schema, required_capabilities, created_at, updated_at) VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?)")
                .bind(id).bind(identifier).bind(name).bind(description).bind(timeout_ms).bind(category_id).bind(input_schema).bind(start_description).bind(output_schema).bind(required_capabilities).bind(created_at).bind(updated_at)
                .execute(&mut *tx).await?;
        }
        tracing::info!(count = workflows.len(), "已导入 workflows");
    }

    // 7. 导入 tools
    if let Some(tools) = entities.get("tools").and_then(|v| v.as_array()) {
        sqlx::query("DELETE FROM tools").execute(&mut *tx).await?;
        for tool in tools {
            let id = tool.get("id").and_then(|v| v.as_i64()).unwrap_or(0);
            let identifier = tool
                .get("identifier")
                .and_then(|v| v.as_str())
                .unwrap_or("");
            let name = tool.get("name").and_then(|v| v.as_str()).unwrap_or("");
            let description = tool
                .get("description")
                .and_then(|v| v.as_str())
                .unwrap_or("");
            let kind = tool
                .get("kind")
                .and_then(|v| v.as_str())
                .unwrap_or("function-wrap")
                .to_string();
            let source = tool
                .get("source")
                .and_then(|v| v.as_str())
                .unwrap_or("workspace");
            let is_always = tool.get("is_always").and_then(|v| v.as_i64()).unwrap_or(0);
            let function_id = tool.get("function_id").and_then(|v| v.as_i64());
            let workflow_id = tool.get("workflow_id").and_then(|v| v.as_i64());
            let input_schema = tool
                .get("input_schema")
                .and_then(|v| v.as_str())
                .unwrap_or("{}");
            let output_schema = tool
                .get("output_schema")
                .and_then(|v| v.as_str())
                .unwrap_or("{}");
            let category_id = tool.get("category_id").and_then(|v| v.as_i64());
            let required_capabilities = tool.get("required_capabilities").and_then(|v| v.as_str());
            let created_at = tool
                .get("created_at")
                .and_then(|v| v.as_str())
                .unwrap_or("");
            let updated_at = tool
                .get("updated_at")
                .and_then(|v| v.as_str())
                .unwrap_or("");

            sqlx::query("INSERT INTO tools (id, identifier, name, description, kind, source, is_always, function_id, workflow_id, input_schema, output_schema, category_id, required_capabilities, created_at, updated_at) VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?)")
                .bind(id).bind(identifier).bind(name).bind(description).bind(kind).bind(source).bind(is_always).bind(function_id).bind(workflow_id).bind(input_schema).bind(output_schema).bind(category_id).bind(required_capabilities).bind(created_at).bind(updated_at)
                .execute(&mut *tx).await?;
        }
        tracing::info!(count = tools.len(), "已导入 tools");
    }

    // 8. 导入 skills
    if let Some(skills) = entities.get("skills").and_then(|v| v.as_array()) {
        sqlx::query("DELETE FROM skills").execute(&mut *tx).await?;
        for skill in skills {
            let id = skill.get("id").and_then(|v| v.as_i64()).unwrap_or(0);
            let identifier = skill
                .get("identifier")
                .and_then(|v| v.as_str())
                .unwrap_or("");
            let name = skill.get("name").and_then(|v| v.as_str()).unwrap_or("");
            let description = skill
                .get("description")
                .and_then(|v| v.as_str())
                .unwrap_or("");
            let frontmatter = skill.get("frontmatter").and_then(|v| v.as_str());
            let content = skill.get("content").and_then(|v| v.as_str()).unwrap_or("");
            let source = skill
                .get("source")
                .and_then(|v| v.as_str())
                .unwrap_or("workspace");
            let is_always = skill.get("is_always").and_then(|v| v.as_i64()).unwrap_or(0);
            let category_id = skill.get("category_id").and_then(|v| v.as_i64());
            let required_capabilities = skill.get("required_capabilities").and_then(|v| v.as_str());
            let created_at = skill
                .get("created_at")
                .and_then(|v| v.as_str())
                .unwrap_or("");
            let updated_at = skill
                .get("updated_at")
                .and_then(|v| v.as_str())
                .unwrap_or("");

            sqlx::query("INSERT INTO skills (id, identifier, name, description, frontmatter, content, source, is_always, category_id, required_capabilities, created_at, updated_at) VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?)")
                .bind(id).bind(identifier).bind(name).bind(description).bind(frontmatter).bind(content).bind(source).bind(is_always).bind(category_id).bind(required_capabilities).bind(created_at).bind(updated_at)
                .execute(&mut *tx).await?;
        }
        tracing::info!(count = skills.len(), "已导入 skills");
    }

    // 9. 导入 agents
    if let Some(agents) = entities.get("agents").and_then(|v| v.as_array()) {
        sqlx::query("DELETE FROM agents").execute(&mut *tx).await?;
        for agent in agents {
            let id = agent.get("id").and_then(|v| v.as_i64()).unwrap_or(0);
            let identifier = agent
                .get("identifier")
                .and_then(|v| v.as_str())
                .unwrap_or("");
            let name = agent.get("name").and_then(|v| v.as_str()).unwrap_or("");
            let description = agent.get("description").and_then(|v| v.as_str());
            let system_prompt = agent
                .get("system_prompt")
                .and_then(|v| v.as_str())
                .unwrap_or("");
            let parent_agent_id = agent.get("parent_agent_id").and_then(|v| v.as_i64());
            let depth = agent.get("depth").and_then(|v| v.as_i64()).unwrap_or(0);
            let model_preset = agent.get("model_preset").and_then(|v| v.as_str());
            let created_at = agent
                .get("created_at")
                .and_then(|v| v.as_str())
                .unwrap_or("");
            let updated_at = agent
                .get("updated_at")
                .and_then(|v| v.as_str())
                .unwrap_or("");

            sqlx::query("INSERT INTO agents (id, identifier, name, description, system_prompt, parent_agent_id, depth, model_preset, created_at, updated_at) VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?)")
                .bind(id).bind(identifier).bind(name).bind(description).bind(system_prompt).bind(parent_agent_id).bind(depth).bind(model_preset).bind(created_at).bind(updated_at)
                .execute(&mut *tx).await?;
        }
        tracing::info!(count = agents.len(), "已导入 agents");
    }

    tx.commit().await?;

    let duration = start.elapsed().as_millis();
    tracing::info!(duration_ms = duration, "数据导入完成");

    Ok(())
}

/// FR-026: 数据导出功能 - 导出所有实体数据到 JSON 文件
pub async fn export_all_data(pool: &Pool<Sqlite>, output_path: &str) -> Result<()> {
    let start = Instant::now();

    // 收集所有实体数据
    let tags = sqlx::query_as::<_, Tag>("SELECT id, name, color, created_at FROM tags")
        .fetch_all(pool)
        .await?;

    let categories = sqlx::query_as::<_, Category>(
        "SELECT id, parent_id, name, slug, description, created_at, updated_at FROM categories",
    )
    .fetch_all(pool)
    .await?;

    let capabilities = sqlx::query_as::<_, Capability>(
        "SELECT name, description, is_dangerous, category_id, created_at FROM capabilities",
    )
    .fetch_all(pool)
    .await?;

    let plugins = sqlx::query_as::<_, Plugin>(
        "SELECT id, identifier, name, description, manifest, runtime, version, author, repository_url, s3_key, sha256, size_bytes, category_id, capabilities, resource_limits, row_revision, created_at, updated_at, deleted_at FROM plugins"
    )
    .fetch_all(pool)
    .await?;

    let functions = sqlx::query_as::<_, Function>(
        "SELECT id, identifier, name, description, kind, input_schema, output_schema, plugin_id, plugin_export, category_id, required_capabilities, created_at, updated_at FROM functions"
    )
    .fetch_all(pool)
    .await?;

    let workflows = sqlx::query_as::<_, Workflow>(
        "SELECT id, identifier, name, description, timeout_ms, category_id, input_schema, start_description, output_schema, required_capabilities, created_at, updated_at FROM workflows"
    )
    .fetch_all(pool)
    .await?;

    let tools = sqlx::query_as::<_, Tool>(
        "SELECT id, identifier, name, description, kind, source, is_always, function_id, workflow_id, input_schema, output_schema, category_id, required_capabilities, created_at, updated_at FROM tools"
    )
    .fetch_all(pool)
    .await?;

    let skills = sqlx::query_as::<_, Skill>(
        "SELECT id, identifier, name, description, frontmatter, content, source, is_always, category_id, required_capabilities, created_at, updated_at FROM skills"
    )
    .fetch_all(pool)
    .await?;

    let agents = sqlx::query_as::<_, Agent>(
        "SELECT id, identifier, name, description, system_prompt, parent_agent_id, depth, model_preset, created_at, updated_at FROM agents"
    )
    .fetch_all(pool)
    .await?;

    // 构建 JSON 结构
    let export_data = serde_json::json!({
        "version": "1.0",
        "exported_at": Utc::now().to_rfc3339(),
        "entities": {
            "tags": tags.iter().map(|t| serde_json::json!({
                "id": t.id,
                "name": t.name,
                "color": t.color,
                "created_at": t.created_at
            })).collect::<Vec<_>>(),
            "categories": categories.iter().map(|c| serde_json::json!({
                "id": c.id,
                "parent_id": c.parent_id,
                "name": c.name,
                "slug": c.slug,
                "description": c.description,
                "created_at": c.created_at,
                "updated_at": c.updated_at
            })).collect::<Vec<_>>(),
            "capabilities": capabilities.iter().map(|c| serde_json::json!({
                "name": c.name,
                "description": c.description,
                "is_dangerous": c.is_dangerous,
                "category_id": c.category_id,
                "created_at": c.created_at
            })).collect::<Vec<_>>(),
            "plugins": plugins.iter().map(|p| serde_json::json!({
                "id": p.id,
                "identifier": p.identifier,
                "name": p.name,
                "description": p.description,
                "manifest": p.manifest,
                "runtime": p.runtime,
                "version": p.version,
                "author": p.author,
                "repository_url": p.repository_url,
                "s3_key": p.s3_key,
                "sha256": p.sha256,
                "size_bytes": p.size_bytes,
                "category_id": p.category_id,
                "created_at": p.created_at,
                "updated_at": p.updated_at,
                "deleted_at": p.deleted_at
            })).collect::<Vec<_>>(),
            "functions": functions.iter().map(|f| serde_json::json!({
                "id": f.id,
                "identifier": f.identifier,
                "name": f.name,
                "description": f.description,
                "kind": f.kind,
                "input_schema": f.input_schema,
                "output_schema": f.output_schema,
                "plugin_id": f.plugin_id,
                "plugin_export": f.plugin_export,
                "category_id": f.category_id,
                "required_capabilities": f.required_capabilities,
                "created_at": f.created_at,
                "updated_at": f.updated_at
            })).collect::<Vec<_>>(),
            "workflows": workflows.iter().map(|w| serde_json::json!({
                "id": w.id,
                "identifier": w.identifier,
                "name": w.name,
                "description": w.description,
                "timeout_ms": w.timeout_ms,
                "category_id": w.category_id,
                "input_schema": w.input_schema,
                "start_description": w.start_description,
                "output_schema": w.output_schema,
                "required_capabilities": w.required_capabilities,
                "created_at": w.created_at,
                "updated_at": w.updated_at
            })).collect::<Vec<_>>(),
            "tools": tools.iter().map(|t| serde_json::json!({
                "id": t.id,
                "identifier": t.identifier,
                "name": t.name,
                "description": t.description,
                "kind": t.kind,
                "source": t.source,
                "is_always": t.is_always,
                "function_id": t.function_id,
                "workflow_id": t.workflow_id,
                "input_schema": t.input_schema,
                "output_schema": t.output_schema,
                "category_id": t.category_id,
                "required_capabilities": t.required_capabilities,
                "created_at": t.created_at,
                "updated_at": t.updated_at
            })).collect::<Vec<_>>(),
            "skills": skills.iter().map(|s| serde_json::json!({
                "id": s.id,
                "identifier": s.identifier,
                "name": s.name,
                "description": s.description,
                "frontmatter": s.frontmatter,
                "content": s.content,
                "source": s.source,
                "is_always": s.is_always,
                "category_id": s.category_id,
                "required_capabilities": s.required_capabilities,
                "created_at": s.created_at,
                "updated_at": s.updated_at
            })).collect::<Vec<_>>(),
            "agents": agents.iter().map(|a| serde_json::json!({
                "id": a.id,
                "identifier": a.identifier,
                "name": a.name,
                "description": a.description,
                "system_prompt": a.system_prompt,
                "parent_agent_id": a.parent_agent_id,
                "depth": a.depth,
                "model_preset": a.model_preset,
                "created_at": a.created_at,
                "updated_at": a.updated_at
            })).collect::<Vec<_>>()
        }
    });

    // 写入文件
    let json_str = serde_json::to_string_pretty(&export_data)?;
    tokio::fs::write(output_path, json_str).await?;

    let duration = start.elapsed().as_millis();
    tracing::info!(
        duration_ms = duration,
        tags_count = tags.len(),
        categories_count = categories.len(),
        capabilities_count = capabilities.len(),
        plugins_count = plugins.len(),
        functions_count = functions.len(),
        workflows_count = workflows.len(),
        tools_count = tools.len(),
        skills_count = skills.len(),
        agents_count = agents.len(),
        "数据导出完成"
    );

    Ok(())
}

/// FR-027: 数据库 Schema 版本管理
/// 当前版本: 2.0
const CURRENT_SCHEMA_VERSION: i64 = 2;

/// 获取当前数据库 schema 版本
pub async fn get_current_version(pool: &Pool<Sqlite>) -> Result<i64> {
    // 检查 schema_versions 表是否存在
    let table_exists: bool = sqlx::query_scalar(
        "SELECT COUNT(*) > 0 FROM sqlite_master WHERE type='table' AND name='schema_versions'",
    )
    .fetch_one(pool)
    .await?;

    if !table_exists {
        return Ok(0); // 表不存在，返回版本 0
    }

    // 获取最高版本号
    let version: Option<i64> = sqlx::query_scalar("SELECT MAX(version) FROM schema_versions")
        .fetch_one(pool)
        .await?;

    Ok(version.unwrap_or(0))
}

/// 检测旧版 plugins 表并升级为新版 schema
/// 旧表列: id, name, version, description, wasm_file_path, is_active, created_at, updated_at
/// 新表列: id, identifier, name, description, manifest, runtime, version, author, repository_url, s3_key, sha256, size_bytes, category_id, created_at, updated_at, deleted_at
async fn migrate_old_plugins_table(pool: &Pool<Sqlite>) -> Result<()> {
    // 检查 plugins 表是否存在
    let table_exists: bool = sqlx::query_scalar(
        "SELECT COUNT(*) > 0 FROM sqlite_master WHERE type='table' AND name='plugins'",
    )
    .fetch_one(pool)
    .await?;

    if !table_exists {
        return Ok(());
    }

    // 检查是否缺少新列（旧表没有 identifier 列）
    let has_identifier: bool = sqlx::query_scalar(
        "SELECT COUNT(*) > 0 FROM pragma_table_info('plugins') WHERE name='identifier'",
    )
    .fetch_one(pool)
    .await?;

    if has_identifier {
        // 表已经是新版 schema，无需迁移
        return Ok(());
    }

    tracing::info!("检测到旧版 plugins 表，正在升级到新版 schema");

    // 旧表没有 deleted_at，用 DROP + CREATE 重建
    sqlx::query("DROP TABLE plugins").execute(pool).await?;

    sqlx::query(
        r#"
        CREATE TABLE plugins (
            id INTEGER PRIMARY KEY AUTOINCREMENT,
            identifier TEXT NOT NULL UNIQUE,
            name TEXT NOT NULL,
            description TEXT,
            manifest TEXT,
            runtime TEXT NOT NULL,
            version TEXT NOT NULL,
            author TEXT,
            repository_url TEXT,
            s3_key TEXT NOT NULL,
            sha256 TEXT NOT NULL,
            size_bytes INTEGER NOT NULL,
            category_id INTEGER REFERENCES categories(id) ON DELETE SET NULL,
            created_at TEXT NOT NULL,
            updated_at TEXT NOT NULL,
            deleted_at TEXT
        )
        "#,
    )
    .execute(pool)
    .await?;

    tracing::info!("旧版 plugins 表已升级为新版 schema");
    Ok(())
}

async fn init_workflow_graph_tables(pool: &Pool<Sqlite>) -> Result<()> {
    sqlx::query(
        r#"
        CREATE TABLE IF NOT EXISTS workflow_nodes (
            id INTEGER PRIMARY KEY AUTOINCREMENT,
            workflow_id INTEGER NOT NULL REFERENCES workflows(id) ON DELETE CASCADE,
            node_key TEXT NOT NULL,
            node_type TEXT NOT NULL DEFAULT 'function_node',
            function_id INTEGER REFERENCES functions(id) ON DELETE SET NULL,
            position_x REAL NOT NULL DEFAULT 0,
            position_y REAL NOT NULL DEFAULT 0,
            node_config TEXT,
            created_at TEXT NOT NULL,
            UNIQUE(workflow_id, node_key)
        )
        "#,
    )
    .execute(pool)
    .await?;

    sqlx::query(
        r#"
        CREATE TABLE IF NOT EXISTS workflow_edges (
            id INTEGER PRIMARY KEY AUTOINCREMENT,
            workflow_id INTEGER NOT NULL REFERENCES workflows(id) ON DELETE CASCADE,
            src_node_key TEXT NOT NULL,
            dst_node_key TEXT NOT NULL,
            mapping TEXT NOT NULL DEFAULT '{}',
            UNIQUE(workflow_id, src_node_key, dst_node_key)
        )
        "#,
    )
    .execute(pool)
    .await?;

    sqlx::query(
        "CREATE INDEX IF NOT EXISTS idx_workflow_nodes_workflow ON workflow_nodes(workflow_id)",
    )
    .execute(pool)
    .await?;
    sqlx::query(
        "CREATE INDEX IF NOT EXISTS idx_workflow_edges_workflow ON workflow_edges(workflow_id)",
    )
    .execute(pool)
    .await?;

    Ok(())
}

/// 运行数据库迁移
pub async fn run_migrations(pool: &Pool<Sqlite>) -> Result<()> {
    let current_version = get_current_version(pool).await?;

    tracing::info!(
        current_version = current_version,
        target_version = CURRENT_SCHEMA_VERSION,
        "开始数据库迁移检查"
    );

    if current_version >= CURRENT_SCHEMA_VERSION {
        tracing::info!("数据库已是最新版本，无需迁移");
        return Ok(());
    }

    // 创建 schema_versions 表（如果不存在）
    sqlx::query(
        r#"
        CREATE TABLE IF NOT EXISTS schema_versions (
            version INTEGER PRIMARY KEY,
            applied_at TEXT NOT NULL,
            description TEXT NOT NULL
        )
        "#,
    )
    .execute(pool)
    .await?;

    // 检测并升级旧版 plugins 表（在 init_tables 之前执行）
    migrate_old_plugins_table(pool).await?;

    // 执行迁移脚本
    if current_version < 1 {
        tracing::info!("执行迁移: v0 -> v1 (初始化实体表)");

        // 创建备份（用于回滚）
        let backup_path = format!("migration_backup_v{}.json", current_version);
        if current_version > 0 {
            export_all_data(pool, &backup_path).await?;
            tracing::info!(backup_path = %backup_path, "已创建迁移前备份");
        }

        // 执行初始化迁移（创建所有实体表）
        init_tables(pool).await?;

        // 记录迁移版本
        let now = Utc::now().to_rfc3339();
        sqlx::query(
            "INSERT INTO schema_versions (version, applied_at, description) VALUES (?, ?, ?)"
        )
        .bind(1i64)
        .bind(&now)
        .bind("初始化实体表：tags, categories, capabilities, plugins, functions, workflows, tools, skills, agents")
        .execute(pool)
        .await?;

        tracing::info!("迁移 v1 完成");
    }

    if current_version < 2 {
        tracing::info!("执行迁移: v1 -> v2 (添加 DAG 节点和连线表)");
        init_workflow_graph_tables(pool).await?;

        let now = Utc::now().to_rfc3339();
        sqlx::query(
            "INSERT INTO schema_versions (version, applied_at, description) VALUES (?, ?, ?)",
        )
        .bind(2i64)
        .bind(&now)
        .bind("添加 DAG 节点和连线表：workflow_nodes, workflow_edges")
        .execute(pool)
        .await?;

        tracing::info!("迁移 v2 完成");
    }

    tracing::info!("数据库迁移完成");
    Ok(())
}

/// Idempotently back-fill the `tags.normalized_name` and
/// `tags.updated_at` columns required by the T058 contract. The
/// `tags_normalized_name_idx` index is rebuilt afterwards so a
/// legacy v1 database still produces the indexed plan asserted
/// by the T055 Red test.
async fn upgrade_tags_table_columns(pool: &Pool<Sqlite>) -> Result<()> {
    let columns = sqlx::query("SELECT name FROM pragma_table_info('tags')")
        .fetch_all(pool)
        .await?;
    let has_normalized = columns
        .iter()
        .any(|row| row.try_get::<String, _>("name").unwrap_or_default() == "normalized_name");
    let has_updated_at = columns
        .iter()
        .any(|row| row.try_get::<String, _>("name").unwrap_or_default() == "updated_at");

    if !has_normalized {
        sqlx::query("ALTER TABLE tags ADD COLUMN normalized_name TEXT NOT NULL DEFAULT ''")
            .execute(pool)
            .await?;
    }
    if !has_updated_at {
        sqlx::query("ALTER TABLE tags ADD COLUMN updated_at TEXT NOT NULL DEFAULT ''")
            .execute(pool)
            .await?;
    }

    // Back-fill `normalized_name` for every existing row using the
    // same NFKC + lower-case pipeline the runtime store uses. The
    // pipeline is intentionally duplicated here (no async dep on
    // `tag_store` to keep migration self-contained).
    sqlx::query(
        "UPDATE tags \
         SET normalized_name = LOWER(name) \
         WHERE normalized_name = '' OR normalized_name IS NULL",
    )
    .execute(pool)
    .await?;

    sqlx::query("DROP INDEX IF EXISTS idx_tags_name")
        .execute(pool)
        .await?;
    sqlx::query("CREATE INDEX IF NOT EXISTS tags_normalized_name_idx ON tags (normalized_name)")
        .execute(pool)
        .await?;
    Ok(())
}

/// 初始化所有新增实体表
pub async fn init_tables(pool: &Pool<Sqlite>) -> Result<()> {
    // Enable ON DELETE CASCADE/SET NULL for all FK constraints created
    // below. SQLite defaults to OFF per connection.
    sqlx::query("PRAGMA foreign_keys = ON")
        .execute(pool)
        .await?;

    // tags 表 — T058 requires `normalized_name` + `updated_at` for the
    // `tags_normalized_name_idx` EXPLAIN contract. `CREATE TABLE IF NOT
    // EXISTS` does not retro-fit an existing table, so the migration
    // function below adds the columns / index when the table is opened
    // with the legacy v1 schema.
    sqlx::query(
        r#"
        CREATE TABLE IF NOT EXISTS tags (
            id INTEGER PRIMARY KEY AUTOINCREMENT,
            name TEXT NOT NULL UNIQUE,
            color TEXT,
            normalized_name TEXT NOT NULL DEFAULT '',
            created_at TEXT NOT NULL,
            updated_at TEXT NOT NULL DEFAULT ''
        )
        "#,
    )
    .execute(pool)
    .await?;
    upgrade_tags_table_columns(pool).await?;

    // categories 表（支持树形层级）
    sqlx::query(
        r#"
        CREATE TABLE IF NOT EXISTS categories (
            id INTEGER PRIMARY KEY AUTOINCREMENT,
            parent_id INTEGER REFERENCES categories(id) ON DELETE SET NULL,
            name TEXT NOT NULL,
            slug TEXT NOT NULL UNIQUE,
            description TEXT,
            created_at TEXT NOT NULL,
            updated_at TEXT NOT NULL
        )
        "#,
    )
    .execute(pool)
    .await?;

    // capabilities 表（name 为主键）
    sqlx::query(
        r#"
        CREATE TABLE IF NOT EXISTS capabilities (
            name TEXT PRIMARY KEY,
            description TEXT NOT NULL,
            is_dangerous INTEGER NOT NULL DEFAULT 0,
            category_id INTEGER REFERENCES categories(id) ON DELETE SET NULL,
            created_at TEXT NOT NULL
        )
        "#,
    )
    .execute(pool)
    .await?;

    // plugins 表（identifier 唯一，支持软删除）
    sqlx::query(
        r#"
        CREATE TABLE IF NOT EXISTS plugins (
            id INTEGER PRIMARY KEY AUTOINCREMENT,
            identifier TEXT NOT NULL UNIQUE,
            name TEXT NOT NULL,
            description TEXT,
            manifest TEXT,
            runtime TEXT NOT NULL,
            version TEXT NOT NULL,
            author TEXT NOT NULL DEFAULT '',
            repository_url TEXT NOT NULL DEFAULT '',
            s3_key TEXT NOT NULL,
            sha256 TEXT NOT NULL,
            size_bytes INTEGER NOT NULL,
            category_id INTEGER REFERENCES categories(id) ON DELETE SET NULL,
            capabilities TEXT NOT NULL DEFAULT '[]',
            resource_limits TEXT NOT NULL DEFAULT '{}',
            row_revision INTEGER NOT NULL DEFAULT 0,
            created_at TEXT NOT NULL,
            updated_at TEXT NOT NULL,
            deleted_at TEXT
        )
        "#,
    )
    .execute(pool)
    .await?;

    // functions 表（identifier 唯一）
    sqlx::query(
        r#"
        CREATE TABLE IF NOT EXISTS functions (
            id INTEGER PRIMARY KEY AUTOINCREMENT,
            identifier TEXT NOT NULL UNIQUE,
            name TEXT NOT NULL,
            description TEXT,
            kind TEXT NOT NULL DEFAULT 'builtin',
            input_schema TEXT NOT NULL DEFAULT '{}',
            output_schema TEXT NOT NULL DEFAULT '{}',
            plugin_id INTEGER REFERENCES plugins(id) ON DELETE SET NULL,
            plugin_export TEXT,
            category_id INTEGER REFERENCES categories(id) ON DELETE SET NULL,
            required_capabilities TEXT,
            created_at TEXT NOT NULL,
            updated_at TEXT NOT NULL
        )
        "#,
    )
    .execute(pool)
    .await?;

    // workflows 表（identifier 唯一，仅主表）
    sqlx::query(
        r#"
        CREATE TABLE IF NOT EXISTS workflows (
            id INTEGER PRIMARY KEY AUTOINCREMENT,
            identifier TEXT NOT NULL UNIQUE,
            name TEXT NOT NULL,
            description TEXT,
            timeout_ms INTEGER NOT NULL DEFAULT 30000,
            category_id INTEGER REFERENCES categories(id) ON DELETE SET NULL,
            input_schema TEXT,
            start_description TEXT,
            output_schema TEXT,
            required_capabilities TEXT,
            created_at TEXT NOT NULL,
            updated_at TEXT NOT NULL
        )
        "#,
    )
    .execute(pool)
    .await?;

    // tools 表（identifier 唯一，CHECK 约束）
    sqlx::query(
        r#"
        CREATE TABLE IF NOT EXISTS tools (
            id INTEGER PRIMARY KEY AUTOINCREMENT,
            identifier TEXT NOT NULL UNIQUE,
            name TEXT NOT NULL,
            description TEXT NOT NULL,
            kind TEXT NOT NULL DEFAULT 'function-wrap',
            source TEXT NOT NULL DEFAULT 'workspace',
            is_always INTEGER NOT NULL DEFAULT 0,
            function_id INTEGER REFERENCES functions(id) ON DELETE SET NULL,
            workflow_id INTEGER REFERENCES workflows(id) ON DELETE SET NULL,
            input_schema TEXT NOT NULL DEFAULT '{}',
            output_schema TEXT NOT NULL DEFAULT '{}',
            category_id INTEGER REFERENCES categories(id) ON DELETE SET NULL,
            required_capabilities TEXT,
            created_at TEXT NOT NULL,
            updated_at TEXT NOT NULL,
            CHECK (
                (kind = 'function-wrap' AND function_id IS NOT NULL AND workflow_id IS NULL) OR
                (kind = 'workflow-wrap' AND workflow_id IS NOT NULL AND function_id IS NULL)
            )
        )
        "#,
    )
    .execute(pool)
    .await?;

    // skills 表（identifier 唯一）
    sqlx::query(
        r#"
        CREATE TABLE IF NOT EXISTS skills (
            id INTEGER PRIMARY KEY AUTOINCREMENT,
            identifier TEXT NOT NULL UNIQUE,
            name TEXT NOT NULL,
            description TEXT NOT NULL,
            frontmatter TEXT,
            content TEXT NOT NULL,
            source TEXT NOT NULL DEFAULT 'workspace',
            is_always INTEGER NOT NULL DEFAULT 0,
            category_id INTEGER REFERENCES categories(id) ON DELETE SET NULL,
            required_capabilities TEXT,
            created_at TEXT NOT NULL,
            updated_at TEXT NOT NULL
        )
        "#,
    )
    .execute(pool)
    .await?;

    // agents 表（identifier 唯一，parent_agent_id 自引用）
    sqlx::query(
        r#"
        CREATE TABLE IF NOT EXISTS agents (
            id INTEGER PRIMARY KEY AUTOINCREMENT,
            identifier TEXT NOT NULL UNIQUE,
            name TEXT NOT NULL,
            description TEXT,
            system_prompt TEXT NOT NULL DEFAULT '',
            parent_agent_id INTEGER REFERENCES agents(id) ON DELETE SET NULL,
            depth INTEGER NOT NULL DEFAULT 0,
            is_default INTEGER NOT NULL DEFAULT 0,
            model_preset TEXT,
            category_id INTEGER REFERENCES categories(id) ON DELETE SET NULL,
            name_normalized TEXT NOT NULL DEFAULT '',
            created_at TEXT NOT NULL,
            updated_at TEXT NOT NULL
        )
        "#,
    )
    .execute(pool)
    .await?;

    // Migration: ensure is_default / category_id / name_normalized exist
    // on pre-existing databases. The CREATE TABLE above uses
    // IF NOT EXISTS, so legacy schemas miss the columns added later.
    let agent_columns: Vec<String> =
        sqlx::query_scalar("SELECT name FROM PRAGMA_TABLE_INFO('agents')")
            .fetch_all(pool)
            .await
            .unwrap_or_default();
    if !agent_columns.iter().any(|name| name == "is_default") {
        sqlx::query("ALTER TABLE agents ADD COLUMN is_default INTEGER NOT NULL DEFAULT 0")
            .execute(pool)
            .await?;
    }
    if !agent_columns.iter().any(|name| name == "category_id") {
        sqlx::query("ALTER TABLE agents ADD COLUMN category_id INTEGER REFERENCES categories(id) ON DELETE SET NULL")
            .execute(pool)
            .await
            .ok();
    }
    if !agent_columns.iter().any(|name| name == "name_normalized") {
        sqlx::query("ALTER TABLE agents ADD COLUMN name_normalized TEXT NOT NULL DEFAULT ''")
            .execute(pool)
            .await?;
    }

    // agent_tools 关联表
    sqlx::query(
        r#"
        CREATE TABLE IF NOT EXISTS agent_tools (
            agent_id INTEGER NOT NULL REFERENCES agents(id) ON DELETE CASCADE,
            tool_id INTEGER NOT NULL REFERENCES tools(id) ON DELETE CASCADE,
            created_at TEXT NOT NULL,
            PRIMARY KEY (agent_id, tool_id)
        )
        "#,
    )
    .execute(pool)
    .await?;

    // agent_skills 关联表
    sqlx::query(
        r#"
        CREATE TABLE IF NOT EXISTS agent_skills (
            agent_id INTEGER NOT NULL REFERENCES agents(id) ON DELETE CASCADE,
            skill_id INTEGER NOT NULL REFERENCES skills(id) ON DELETE CASCADE,
            created_at TEXT NOT NULL,
            PRIMARY KEY (agent_id, skill_id)
        )
        "#,
    )
    .execute(pool)
    .await?;

    // agent_capabilities 关联表
    sqlx::query(
        r#"
        CREATE TABLE IF NOT EXISTS agent_capabilities (
            agent_id INTEGER NOT NULL REFERENCES agents(id) ON DELETE CASCADE,
            capability_name TEXT NOT NULL REFERENCES capabilities(name) ON DELETE CASCADE,
            created_at TEXT NOT NULL,
            PRIMARY KEY (agent_id, capability_name)
        )
        "#,
    )
    .execute(pool)
    .await?;

    // unique-default 守护索引 (T115 唯一默认根不变量)
    sqlx::query(
        "CREATE UNIQUE INDEX IF NOT EXISTS idx_agents_is_default_unique \
         ON agents(is_default) WHERE is_default = 1",
    )
    .execute(pool)
    .await
    .ok();

    // DAG 节点和连线表
    init_workflow_graph_tables(pool).await?;

    // 创建索引
    // tags 索引由 init_tables 末尾的 `tags_normalized_name_idx` 维护
    // (T058 contract). 这里不再单独建 idx_tags_name 以避免重复。
    sqlx::query("CREATE INDEX IF NOT EXISTS idx_categories_slug ON categories(slug)")
        .execute(pool)
        .await?;
    sqlx::query("CREATE INDEX IF NOT EXISTS idx_categories_parent_id ON categories(parent_id)")
        .execute(pool)
        .await?;
    sqlx::query("CREATE INDEX IF NOT EXISTS idx_plugins_identifier ON plugins(identifier)")
        .execute(pool)
        .await?;
    sqlx::query("CREATE INDEX IF NOT EXISTS idx_functions_identifier ON functions(identifier)")
        .execute(pool)
        .await?;
    sqlx::query("CREATE INDEX IF NOT EXISTS idx_workflows_identifier ON workflows(identifier)")
        .execute(pool)
        .await?;
    sqlx::query("CREATE INDEX IF NOT EXISTS idx_tools_identifier ON tools(identifier)")
        .execute(pool)
        .await?;
    sqlx::query("CREATE INDEX IF NOT EXISTS idx_skills_identifier ON skills(identifier)")
        .execute(pool)
        .await?;
    sqlx::query("CREATE INDEX IF NOT EXISTS idx_agents_identifier ON agents(identifier)")
        .execute(pool)
        .await?;

    Ok(())
}

#[cfg(test)]
mod tests {
    use sqlx::sqlite::SqlitePoolOptions;

    use super::*;

    #[tokio::test]
    async fn migration_adds_dag_tables_to_existing_v1_database() {
        let pool = SqlitePoolOptions::new()
            .max_connections(1)
            .connect("sqlite::memory:")
            .await
            .expect("in-memory database");
        sqlx::query(
            r#"
            CREATE TABLE schema_versions (
                version INTEGER PRIMARY KEY,
                applied_at TEXT NOT NULL,
                description TEXT NOT NULL
            )
            "#,
        )
        .execute(&pool)
        .await
        .expect("v1 schema version table");
        sqlx::query(
            "INSERT INTO schema_versions (version, applied_at, description) VALUES (1, '', 'v1')",
        )
        .execute(&pool)
        .await
        .expect("v1 schema version");

        run_migrations(&pool).await.expect("upgrade v1 database");

        assert_eq!(get_current_version(&pool).await.unwrap(), 2);
        for table in ["workflow_nodes", "workflow_edges"] {
            let exists: bool = sqlx::query_scalar(
                "SELECT COUNT(*) > 0 FROM sqlite_master WHERE type='table' AND name = ?",
            )
            .bind(table)
            .fetch_one(&pool)
            .await
            .unwrap();
            assert!(exists, "migration must create {table}");
        }
    }

    #[test]
    fn test_validate_identifier_valid() {
        assert!(validate_identifier("valid-id").is_ok());
        assert!(validate_identifier("valid_id").is_ok());
        assert!(validate_identifier("ValidId123").is_ok());
        assert!(validate_identifier("a").is_ok());
    }

    #[test]
    fn test_validate_identifier_empty() {
        assert!(validate_identifier("").is_err());
        assert!(validate_identifier("   ").is_err());
    }

    #[test]
    fn test_validate_identifier_too_long() {
        let long_id = "a".repeat(256);
        assert!(validate_identifier(&long_id).is_err());
    }

    #[test]
    fn test_validate_identifier_invalid_chars() {
        assert!(validate_identifier("invalid id").is_err());
        assert!(validate_identifier("invalid@id").is_err());
        assert!(validate_identifier("invalid.id").is_err());
    }

    #[test]
    fn test_validate_name_valid() {
        assert!(validate_name("Valid Name").is_ok());
        assert!(validate_name("名称").is_ok());
        assert!(validate_name("Name with spaces").is_ok());
    }

    #[test]
    fn test_validate_name_empty() {
        assert!(validate_name("").is_err());
        assert!(validate_name("   ").is_err());
    }

    #[test]
    fn test_validate_name_too_long() {
        let long_name = "a".repeat(256);
        assert!(validate_name(&long_name).is_err());
    }

    #[test]
    fn test_validate_slug_valid() {
        assert!(validate_slug("valid-slug").is_ok());
        assert!(validate_slug("valid123").is_ok());
        assert!(validate_slug("a").is_ok());
    }

    #[test]
    fn test_validate_slug_empty() {
        assert!(validate_slug("").is_err());
        assert!(validate_slug("   ").is_err());
    }

    #[test]
    fn test_validate_slug_too_long() {
        let long_slug = "a".repeat(256);
        assert!(validate_slug(&long_slug).is_err());
    }

    #[test]
    fn test_validate_slug_invalid_chars() {
        assert!(validate_slug("Invalid-Slug").is_err());
        assert!(validate_slug("invalid_slug").is_err());
        assert!(validate_slug("invalid slug").is_err());
    }

    #[test]
    fn test_validate_description_valid() {
        assert!(validate_description("").is_ok());
        assert!(validate_description("Valid description").is_ok());
        assert!(validate_description("描述").is_ok());
    }

    #[test]
    fn test_validate_description_too_long() {
        let long_desc = "a".repeat(2001);
        assert!(validate_description(&long_desc).is_err());
    }

    #[test]
    fn test_validate_json_valid() {
        assert!(validate_json("{}").is_ok());
        assert!(validate_json("{\"key\": \"value\"}").is_ok());
        assert!(validate_json("[]").is_ok());
        assert!(validate_json("[1, 2, 3]").is_ok());
    }

    #[test]
    fn test_validate_json_invalid() {
        assert!(validate_json("not json").is_err());
        assert!(validate_json("{key: value}").is_err());
        assert!(validate_json("").is_err());
    }

    #[test]
    fn test_validate_json_too_large() {
        let large_json = format!("{{\"data\": \"{}\"}}", "a".repeat(1024 * 1024));
        assert!(validate_json(&large_json).is_err());
    }

    #[test]
    fn test_validate_color_valid() {
        assert!(validate_color("#FF0000").is_ok());
        assert!(validate_color("#00ff00").is_ok());
        assert!(validate_color("#123abc").is_ok());
    }

    #[test]
    fn test_validate_color_invalid_format() {
        assert!(validate_color("FF0000").is_err());
        assert!(validate_color("#FF000").is_err());
        assert!(validate_color("#FF00000").is_err());
        assert!(validate_color("#GG0000").is_err());
    }
}

// =====================================================================
// US13 T124: typed `AgentStore` public boundary.
//
// The legacy `Agent` struct above remains the runtime "wide row"
// shape used by the recovery/restore path. The typed store
// surfaces only validated, search-indexed, hierarchy-respecting
// operations the UI / runtime call. Both shapes share the same
// underlying `agents` table and association tables; the typed
// store enforces the agent-hierarchy invariants the spec requires:
//   * identifier uniqueness;
//   * system_prompt non-empty and ≤ 1MiB;
//   * model_preset must reference an existing LlmPreset.name when
//     present;
//   * parent_agent_id forms a tree of depth ≤ 10 with no cycle;
//   * at most one default root (parent_agent_id IS NULL) Agent at
//     any time, with first Agent auto-defaulted and a transactional
//     replacement when switching;
//   * the three resource association tables
//     (agent_tools, agent_skills, agent_capabilities) accept
//     deduplicated references and surface stable conflict
//     envelopes.
// =====================================================================

/// Maximum allowed Agent depth (root = 0). The T124 spec requires
/// the Store to refuse hierarchies deeper than this.
pub const AGENT_MAX_DEPTH: i64 = 10;

/// Fixed page size for Agent list / search.
pub const AGENT_PAGE_SIZE: i64 = 20;

/// Failure mode for the typed Agent store. The variant
/// intentionally carries only safe values (no SQL fragments, no
/// raw row content). UI MUST receive the [`AgentStoreError::field`]
/// and reuse the [`AgentStoreError::references`] for the
/// `conflict { shape: "references" }` envelope; raw values are
/// never returned to callers.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum AgentStoreErrorKind {
    /// Caller supplied an invalid value.
    InvalidInput {
        /// Field that failed validation.
        field: String,
        /// Stable reason code.
        reason: String,
    },
    /// Caller requested creation/update that collides with an
    /// existing row.
    Conflict(AgentConflict),
    /// The requested Agent does not exist.
    NotFound,
    /// The new parent would form a cycle or exceed the depth
    /// ceiling.
    HierarchyViolation {
        /// Stable reason code (`cycle` or `depth_exceeded`).
        reason: String,
        /// Optional references to the offending parent.
        references: Vec<String>,
    },
    /// Underlying SQL error with a sanitized cause.
    Backend(String),
}

/// Conflict envelope returned by the Agent store. The shape
/// (value vs references) is preserved so the UI can render
/// either a "duplicate identifier" prompt or a "referenced by
/// other Agent" prompt without leaking the offending value or
/// row count.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum AgentConflict {
    /// `identifier` is not unique.
    DuplicateIdentifier {
        /// Existing identifier.
        value: String,
    },
    /// Trying to delete / unset the default Agent before a
    /// replacement is designated.
    DefaultReplacementRequired {
        /// Default Agent identifier that is being deleted.
        value: String,
    },
}

impl AgentConflict {
    /// Field that triggered the conflict.
    pub fn field(&self) -> &'static str {
        match self {
            Self::DuplicateIdentifier { .. } => "identifier",
            Self::DefaultReplacementRequired { .. } => "is_default",
        }
    }
    /// Stable reason code.
    pub fn reason(&self) -> &'static str {
        match self {
            Self::DuplicateIdentifier { .. } => "duplicate",
            Self::DefaultReplacementRequired { .. } => "replacement_required",
        }
    }
    /// Conflict shape.
    pub fn shape(&self) -> &'static str {
        match self {
            Self::DuplicateIdentifier { .. } => "value",
            Self::DefaultReplacementRequired { .. } => "references",
        }
    }
}

/// Stable error envelope returned by the Agent store.
#[derive(Debug, Error)]
#[error("agent store error: {kind:?}")]
pub struct AgentStoreError {
    /// Error variant.
    pub kind: AgentStoreErrorKind,
}

impl AgentStoreError {
    /// Field that triggered the failure, when known.
    pub fn field(&self) -> &str {
        match &self.kind {
            AgentStoreErrorKind::InvalidInput { field, .. } => field.as_str(),
            AgentStoreErrorKind::Conflict(conflict) => conflict.field(),
            AgentStoreErrorKind::NotFound => "id",
            AgentStoreErrorKind::HierarchyViolation { .. } => "parent_agent_id",
            AgentStoreErrorKind::Backend(_) => "",
        }
    }

    /// Stable reason code, when known.
    pub fn reason(&self) -> &str {
        match &self.kind {
            AgentStoreErrorKind::InvalidInput { reason, .. } => reason.as_str(),
            AgentStoreErrorKind::Conflict(conflict) => conflict.reason(),
            AgentStoreErrorKind::NotFound => "not_found",
            AgentStoreErrorKind::HierarchyViolation { reason, .. } => reason.as_str(),
            AgentStoreErrorKind::Backend(_) => "backend",
        }
    }

    /// Reference identifiers returned with a `references` conflict
    /// or hierarchy violation. UI MAY show these; the values are
    /// pre-trimmed to safe Agent identifiers.
    pub fn references(&self) -> &[String] {
        match &self.kind {
            AgentStoreErrorKind::Conflict(AgentConflict::DefaultReplacementRequired { value }) => {
                std::slice::from_ref(value)
            }
            AgentStoreErrorKind::HierarchyViolation { references, .. } => references.as_slice(),
            _ => &[],
        }
    }
}

impl From<AgentConflict> for AgentStoreError {
    fn from(conflict: AgentConflict) -> Self {
        Self {
            kind: AgentStoreErrorKind::Conflict(conflict),
        }
    }
}

/// Validated input to a typed `AgentStore::create` call.
#[derive(Debug, Clone)]
pub struct AgentInput {
    identifier: String,
    name: String,
    description: Option<String>,
    system_prompt: String,
    parent_agent_id: Option<i64>,
    is_default: bool,
    model_preset: Option<String>,
    category_id: Option<i64>,
    tool_ids: Vec<i64>,
    skill_ids: Vec<i64>,
    always_skill_ids: Vec<i64>,
    capability_names: Vec<String>,
}

impl AgentInput {
    /// Construct a new root Agent input. The Store computes the
    /// depth and handles the `is_default` invariant.
    pub fn new_root(
        identifier: impl Into<String>,
        name: impl Into<String>,
        system_prompt: impl Into<String>,
    ) -> Result<Self, AgentStoreError> {
        let identifier = identifier.into();
        let name = name.into();
        let system_prompt = system_prompt.into();
        if identifier.trim().is_empty() {
            return Err(invalid("identifier", "empty"));
        }
        if identifier.len() > 255 {
            return Err(invalid("identifier", "too_long"));
        }
        if !identifier
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '_' || c == '-')
        {
            return Err(invalid("identifier", "invalid_charset"));
        }
        if name.trim().is_empty() {
            return Err(invalid("name", "empty"));
        }
        if name.len() > 255 {
            return Err(invalid("name", "too_long"));
        }
        if system_prompt.trim().is_empty() {
            return Err(invalid("system_prompt", "empty"));
        }
        if system_prompt.len() > 1024 * 1024 {
            return Err(invalid("system_prompt", "too_long"));
        }
        Ok(Self {
            identifier,
            name,
            description: None,
            system_prompt,
            parent_agent_id: None,
            is_default: false,
            model_preset: None,
            category_id: None,
            tool_ids: Vec::new(),
            skill_ids: Vec::new(),
            always_skill_ids: Vec::new(),
            capability_names: Vec::new(),
        })
    }

    /// Set the description.
    pub fn with_description(mut self, description: impl Into<String>) -> Self {
        self.description = Some(description.into());
        self
    }

    /// Set the explicit parent. `None` means "this is a root".
    pub fn with_parent(mut self, parent_agent_id: Option<i64>) -> Self {
        self.parent_agent_id = parent_agent_id;
        self
    }

    /// Request the default flag. The Store enforces the unique
    /// default root invariant, so callers SHOULD pass `true` only
    /// for the first Agent or after explicitly replacing an
    /// existing default.
    pub fn with_default(mut self, is_default: bool) -> Self {
        self.is_default = is_default;
        self
    }

    /// Bind the model preset by `LlmPreset.name`.
    pub fn with_model_preset(mut self, preset: impl Into<String>) -> Self {
        self.model_preset = Some(preset.into());
        self
    }

    /// Bind a category.
    pub fn with_category(mut self, category_id: i64) -> Self {
        self.category_id = Some(category_id);
        self
    }

    /// Set the explicit Tool association list.
    pub fn with_tools(mut self, tool_ids: impl IntoIterator<Item = i64>) -> Self {
        self.tool_ids = tool_ids.into_iter().collect();
        self
    }

    /// Set the explicit Skill association list.
    pub fn with_skills(mut self, skill_ids: impl IntoIterator<Item = i64>) -> Self {
        self.skill_ids = skill_ids.into_iter().collect();
        self
    }

    /// Set the always-on Skill association list.
    pub fn with_always_skills(mut self, skill_ids: impl IntoIterator<Item = i64>) -> Self {
        self.always_skill_ids = skill_ids.into_iter().collect();
        self
    }

    /// Set the explicit Capability association list.
    pub fn with_capabilities(mut self, capability_names: impl IntoIterator<Item = String>) -> Self {
        self.capability_names = capability_names.into_iter().collect();
        self
    }

    /// Identifier.
    pub fn identifier(&self) -> &str {
        &self.identifier
    }
    /// Name.
    pub fn name(&self) -> &str {
        &self.name
    }
    /// System prompt.
    pub fn system_prompt(&self) -> &str {
        &self.system_prompt
    }
    /// Optional description.
    pub fn description(&self) -> Option<&str> {
        self.description.as_deref()
    }
    /// Optional parent.
    pub fn parent_agent_id(&self) -> Option<i64> {
        self.parent_agent_id
    }
    /// Requested default flag.
    pub fn is_default(&self) -> bool {
        self.is_default
    }
    /// Model preset name, if any.
    pub fn model_preset(&self) -> Option<&str> {
        self.model_preset.as_deref()
    }
    /// Category id, if any.
    pub fn category_id(&self) -> Option<i64> {
        self.category_id
    }
    /// Tool ids.
    pub fn tool_ids(&self) -> &[i64] {
        &self.tool_ids
    }
    /// Skill ids.
    pub fn skill_ids(&self) -> &[i64] {
        &self.skill_ids
    }
    /// Always-on skill ids.
    pub fn always_skill_ids(&self) -> &[i64] {
        &self.always_skill_ids
    }
    /// Capability names.
    pub fn capability_names(&self) -> &[String] {
        &self.capability_names
    }
}

/// Persisted Agent record returned by the typed store.
#[derive(Debug, Clone)]
pub struct AgentRecord {
    id: i64,
    identifier: String,
    name: String,
    description: Option<String>,
    system_prompt: String,
    parent_agent_id: Option<i64>,
    depth: i64,
    is_default: bool,
    model_preset: Option<String>,
    category_id: Option<i64>,
    tool_ids: Vec<i64>,
    skill_ids: Vec<i64>,
    always_skill_ids: Vec<i64>,
    capability_names: Vec<String>,
    created_at: String,
    updated_at: String,
}

impl AgentRecord {
    /// Database id.
    pub fn id(&self) -> i64 {
        self.id
    }
    /// Identifier.
    pub fn identifier(&self) -> &str {
        &self.identifier
    }
    /// Name.
    pub fn name(&self) -> &str {
        &self.name
    }
    /// Description.
    pub fn description(&self) -> Option<&str> {
        self.description.as_deref()
    }
    /// System prompt.
    pub fn system_prompt(&self) -> &str {
        &self.system_prompt
    }
    /// Parent id (None for root Agents).
    pub fn parent_agent_id(&self) -> Option<i64> {
        self.parent_agent_id
    }
    /// Computed depth (root = 0).
    pub fn depth(&self) -> i64 {
        self.depth
    }
    /// Whether this is the unique default Agent.
    pub fn is_default(&self) -> bool {
        self.is_default
    }
    /// Model preset.
    pub fn model_preset(&self) -> Option<&str> {
        self.model_preset.as_deref()
    }
    /// Category.
    pub fn category_id(&self) -> Option<i64> {
        self.category_id
    }
    /// Explicit Tool ids.
    pub fn tool_ids(&self) -> &[i64] {
        &self.tool_ids
    }
    /// Explicit Skill ids.
    pub fn skill_ids(&self) -> &[i64] {
        &self.skill_ids
    }
    /// Always-on skill ids.
    pub fn always_skill_ids(&self) -> &[i64] {
        &self.always_skill_ids
    }
    /// Explicit Capability names.
    pub fn capability_names(&self) -> &[String] {
        &self.capability_names
    }
    /// Created-at timestamp (RFC3339).
    pub fn created_at(&self) -> &str {
        &self.created_at
    }
    /// Updated-at timestamp (RFC3339).
    pub fn updated_at(&self) -> &str {
        &self.updated_at
    }
}

/// Filter for Agent list / search.
#[derive(Debug, Clone, Default)]
pub struct AgentFilter {
    /// Optional normalized search term (NFKC + lower-case, max 255 chars).
    pub search: Option<String>,
    /// Page number (1-based).
    pub page: i64,
    /// Page size (must equal [`AGENT_PAGE_SIZE`]).
    pub page_size: i64,
}

impl AgentFilter {
    /// First page with the fixed page size.
    pub fn first() -> Self {
        Self {
            search: None,
            page: 1,
            page_size: AGENT_PAGE_SIZE,
        }
    }
    /// Builder: search term.
    pub fn with_search(mut self, term: impl Into<String>) -> Self {
        self.search = Some(term.into());
        self
    }
    /// Builder: page.
    pub fn with_page(mut self, page: i64) -> Self {
        self.page = page;
        self
    }
}

/// Paged Agent results.
#[derive(Debug, Clone)]
pub struct AgentPage {
    records: Vec<AgentRecord>,
    total: i64,
    page: i64,
    page_size: i64,
}

impl AgentPage {
    /// Records in the current page.
    pub fn records(&self) -> &[AgentRecord] {
        &self.records
    }
    /// Total record count matching the filter.
    pub fn total(&self) -> i64 {
        self.total
    }
    /// 1-based page number.
    pub fn page(&self) -> i64 {
        self.page
    }
    /// Page size used.
    pub fn page_size(&self) -> i64 {
        self.page_size
    }
}

/// Helper: build an InvalidInput error envelope.
fn invalid(field: &str, reason: &str) -> AgentStoreError {
    AgentStoreError {
        kind: AgentStoreErrorKind::InvalidInput {
            field: field.to_string(),
            reason: reason.to_string(),
        },
    }
}

fn normalize_agent_name(name: &str) -> String {
    use unicode_normalization::UnicodeNormalization;
    let nfkc: String = name.nfkc().collect();
    let nfc: String = nfkc.nfc().collect();
    nfc.to_lowercase()
}

fn is_unique_violation(error: &sqlx::Error) -> bool {
    if let sqlx::Error::Database(db) = error {
        if let Some(code) = db.code() {
            return code == "2067" || code == "1555";
        }
        let msg = db.message().to_ascii_lowercase();
        return msg.contains("unique constraint failed") || msg.contains("unique index");
    }
    false
}

type AgentBaseRow = (
    i64,
    String,
    String,
    Option<String>,
    String,
    Option<i64>,
    i64,
    i64,
    Option<String>,
    Option<i64>,
    String,
    String,
);

#[derive(Debug, Clone)]
enum AgentSearchRoute {
    Unfiltered,
    FtsPhrase(String),
    ShortGram { length: i64, gram: String },
}

impl AgentSearchRoute {
    fn for_input(search: Option<&str>) -> Result<Self, AgentStoreError> {
        let Some(search) = search else {
            return Ok(Self::Unfiltered);
        };
        let normalizer = canonical_normalizer().map_err(agent_search_error)?;
        let normalized = normalize_search_query(&normalizer, search).map_err(agent_search_error)?;
        let selection =
            IndexSelection::for_input(search, &normalizer).map_err(agent_search_error)?;
        Ok(match selection.backend() {
            IndexBackend::Fts5Trigram => Self::FtsPhrase(fts_literal_phrase(&normalized)),
            IndexBackend::ShortGram { length } => Self::ShortGram {
                length: length as i64,
                gram: normalized,
            },
        })
    }
}

fn agent_search_error(error: super::search_index::SearchError) -> AgentStoreError {
    match error {
        super::search_index::SearchError::InvalidInput { reason } => invalid("search", reason),
        other => AgentStoreError {
            kind: AgentStoreErrorKind::Backend(format!("search_index: {other}")),
        },
    }
}

/// Typed Agent store. The handle is a thin wrapper around the
/// `agents` table; every public method runs inside a single
/// transaction so the `is_default` invariant, hierarchy
/// validation and association writes are atomic.
#[derive(Debug, Clone)]
pub struct AgentStore {
    pool: Pool<Sqlite>,
    observer: Option<QueryCountObserver>,
}

impl AgentStore {
    /// Open a new typed Agent store over the given pool.
    pub fn new(pool: Pool<Sqlite>) -> Result<Self, AgentStoreError> {
        Ok(Self {
            pool,
            observer: None,
        })
    }

    /// Open the Agent boundary from the canonical Store, preserving the
    /// production query-count observer configured at Store open time.
    pub fn from_store(store: &Store) -> Result<Self, AgentStoreError> {
        Ok(Self {
            pool: store.pool().clone(),
            observer: store.query_count_observer().cloned(),
        })
    }

    /// Underlying pool.
    pub fn pool(&self) -> &Pool<Sqlite> {
        &self.pool
    }

    /// Create a new Agent. The depth is computed from the parent;
    /// the first Agent in the database is automatically the
    /// default. Switching the default must use
    /// [`AgentStore::set_default`].
    pub async fn create(&self, input: AgentInput) -> Result<AgentRecord, AgentStoreError> {
        validate_input(&input)?;
        let now = Utc::now().to_rfc3339();
        let normalized = normalize_agent_name(&input.name);

        // Compute depth and check the cycle invariant before
        // mutating the database.
        let computed_depth = match input.parent_agent_id {
            None => 0,
            Some(parent_id) => {
                let parent = self.fetch_one(parent_id).await?.ok_or(AgentStoreError {
                    kind: AgentStoreErrorKind::NotFound,
                })?;
                if parent.depth + 1 > AGENT_MAX_DEPTH {
                    return Err(AgentStoreError {
                        kind: AgentStoreErrorKind::HierarchyViolation {
                            reason: "depth_exceeded".to_string(),
                            references: vec![parent.identifier.clone()],
                        },
                    });
                }
                parent.depth + 1
            }
        };

        // Reference checks for FK / model_preset / category.
        self.validate_references(&input).await?;

        let mut tx = self.pool.begin().await.map_err(backend_error)?;

        // First Agent in the table auto-defaults, regardless of
        // the input. The unique-default invariant is enforced by
        // both the application logic here and the partial unique
        // index `idx_agents_is_default_unique`.
        let is_first_agent: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM agents")
            .fetch_one(&mut *tx)
            .await
            .map_err(backend_error)?;
        let effective_is_default = input.is_default || is_first_agent == 0;

        if effective_is_default {
            // Atomically clear the previous default (if any).
            sqlx::query("UPDATE agents SET is_default = 0 WHERE is_default = 1")
                .execute(&mut *tx)
                .await
                .map_err(backend_error)?;
        }

        let insert_result = sqlx::query(
            "INSERT INTO agents \
             (identifier, name, description, system_prompt, parent_agent_id, depth, is_default, model_preset, category_id, name_normalized, created_at, updated_at) \
             VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?)",
        )
        .bind(&input.identifier)
        .bind(&input.name)
        .bind(&input.description)
        .bind(&input.system_prompt)
        .bind(input.parent_agent_id)
        .bind(computed_depth)
        .bind(if effective_is_default { 1_i64 } else { 0_i64 })
        .bind(&input.model_preset)
        .bind(input.category_id)
        .bind(&normalized)
        .bind(&now)
        .bind(&now)
        .execute(&mut *tx)
        .await;

        let exec_result = match insert_result {
            Ok(result) => result,
            Err(error) => {
                if is_unique_violation(&error) {
                    return Err(AgentStoreError {
                        kind: AgentStoreErrorKind::Conflict(AgentConflict::DuplicateIdentifier {
                            value: input.identifier.clone(),
                        }),
                    });
                }
                return Err(backend_error(error));
            }
        };

        let new_id = exec_result.last_insert_rowid();

        // Write the three association tables.
        for tool_id in dedup_i64(&input.tool_ids) {
            sqlx::query(
                "INSERT OR IGNORE INTO agent_tools (agent_id, tool_id, created_at) VALUES (?, ?, ?)",
            )
            .bind(new_id)
            .bind(tool_id)
            .bind(&now)
            .execute(&mut *tx)
            .await
            .map_err(backend_error)?;
        }
        for skill_id in dedup_i64(&input.skill_ids) {
            sqlx::query(
                "INSERT OR IGNORE INTO agent_skills (agent_id, skill_id, created_at) VALUES (?, ?, ?)",
            )
            .bind(new_id)
            .bind(skill_id)
            .bind(&now)
            .execute(&mut *tx)
            .await
            .map_err(backend_error)?;
        }
        for capability in dedup_string(&input.capability_names) {
            sqlx::query(
                "INSERT OR IGNORE INTO agent_capabilities (agent_id, capability_name, created_at) VALUES (?, ?, ?)",
            )
            .bind(new_id)
            .bind(&capability)
            .bind(&now)
            .execute(&mut *tx)
            .await
            .map_err(backend_error)?;
        }

        replace_entity_search_documents(
            &mut tx,
            "agent",
            &new_id.to_string(),
            &[("identifier", &input.identifier), ("name", &input.name)],
        )
        .await
        .map_err(agent_search_error)?;

        tx.commit().await.map_err(backend_error)?;

        self.fetch_one(new_id).await?.ok_or(AgentStoreError {
            kind: AgentStoreErrorKind::NotFound,
        })
    }

    /// Update an existing Agent. Cycles, depth, FK and unique
    /// invariants are checked inside the transaction.
    pub async fn update(&self, id: i64, input: AgentInput) -> Result<AgentRecord, AgentStoreError> {
        validate_input(&input)?;
        let now = Utc::now().to_rfc3339();
        let normalized = normalize_agent_name(&input.name);

        let existing = self.fetch_one(id).await?.ok_or(AgentStoreError {
            kind: AgentStoreErrorKind::NotFound,
        })?;

        // Cycle + depth check: walking up from the new parent must
        // not reach `id`. The walk uses indexed reads of
        // `parent_agent_id`.
        if let Some(parent_id) = input.parent_agent_id {
            if parent_id == id {
                return Err(AgentStoreError {
                    kind: AgentStoreErrorKind::HierarchyViolation {
                        reason: "cycle".to_string(),
                        references: vec![existing.identifier.clone()],
                    },
                });
            }
            if self.would_form_cycle(id, parent_id).await? {
                return Err(AgentStoreError {
                    kind: AgentStoreErrorKind::HierarchyViolation {
                        reason: "cycle".to_string(),
                        references: vec![existing.identifier.clone()],
                    },
                });
            }
            let parent = self.fetch_one(parent_id).await?.ok_or(AgentStoreError {
                kind: AgentStoreErrorKind::NotFound,
            })?;
            // The new depth is parent.depth + 1, but the existing
            // subtree also shifts. Compute the delta and re-apply
            // it to every descendant in one transaction.
            let new_depth = parent.depth + 1;
            if new_depth > AGENT_MAX_DEPTH {
                return Err(AgentStoreError {
                    kind: AgentStoreErrorKind::HierarchyViolation {
                        reason: "depth_exceeded".to_string(),
                        references: vec![parent.identifier.clone()],
                    },
                });
            }
            // Subtree deltas are applied below in the same
            // transaction.
            self.validate_references(&input).await?;
            let mut tx = self.pool.begin().await.map_err(backend_error)?;
            apply_subtree_depth_shift(&mut tx, id, new_depth - existing.depth, AGENT_MAX_DEPTH)
                .await?;
            update_row(&mut tx, id, &input, &normalized, &now, existing.is_default).await?;
            replace_associations(&mut tx, id, &input, &now).await?;
            replace_agent_search_documents(&mut tx, id, &input).await?;
            tx.commit().await.map_err(backend_error)?;
        } else {
            self.validate_references(&input).await?;
            let mut tx = self.pool.begin().await.map_err(backend_error)?;
            // Promote to root: depth 0 and shift the entire
            // subtree to remain ≤ AGENT_MAX_DEPTH.
            apply_subtree_depth_shift(&mut tx, id, -existing.depth, AGENT_MAX_DEPTH).await?;
            update_row(&mut tx, id, &input, &normalized, &now, existing.is_default).await?;
            replace_associations(&mut tx, id, &input, &now).await?;
            replace_agent_search_documents(&mut tx, id, &input).await?;
            tx.commit().await.map_err(backend_error)?;
        }

        self.fetch_one(id).await?.ok_or(AgentStoreError {
            kind: AgentStoreErrorKind::NotFound,
        })
    }

    /// Delete an Agent. The Agent's children are reparented to the
    /// deleted Agent's parent (i.e. `parent_agent_id` is set to the
    /// deleted Agent's parent); the cascade does not touch the
    /// subtree beyond that. A delete of the default Agent is
    /// refused unless `replacement_id` is supplied and references
    /// an existing root Agent.
    pub async fn delete(
        &self,
        id: i64,
        replacement_id: Option<i64>,
    ) -> Result<(), AgentStoreError> {
        let existing = self.fetch_one(id).await?.ok_or(AgentStoreError {
            kind: AgentStoreErrorKind::NotFound,
        })?;
        // Resolve the replacement record BEFORE opening the
        // transaction. With a single-connection test pool, doing
        // the fetch inside the transaction would deadlock.
        let replacement_needs_promotion: Option<AgentRecord> = if existing.is_default {
            match replacement_id {
                Some(replacement) => {
                    let record = self.fetch_one(replacement).await?.ok_or(AgentStoreError {
                        kind: AgentStoreErrorKind::NotFound,
                    })?;
                    if !record.is_default && record.parent_agent_id.is_some() {
                        return Err(AgentStoreError {
                            kind: AgentStoreErrorKind::InvalidInput {
                                field: "replacement_id".to_string(),
                                reason: "must_be_root".to_string(),
                            },
                        });
                    }
                    Some(record)
                }
                None => {
                    return Err(AgentStoreError {
                        kind: AgentStoreErrorKind::Conflict(
                            AgentConflict::DefaultReplacementRequired {
                                value: existing.identifier.clone(),
                            },
                        ),
                    });
                }
            }
        } else {
            None
        };
        let mut tx = self.pool.begin().await.map_err(backend_error)?;
        if let Some(replacement) = &replacement_needs_promotion
            && !replacement.is_default
        {
            sqlx::query("UPDATE agents SET is_default = 0 WHERE id = ?")
                .bind(existing.id)
                .execute(&mut *tx)
                .await
                .map_err(backend_error)?;
            sqlx::query("UPDATE agents SET is_default = 1 WHERE id = ?")
                .bind(replacement.id)
                .execute(&mut *tx)
                .await
                .map_err(backend_error)?;
        }
        // Reparent children to the deleted agent's parent.
        sqlx::query("UPDATE agents SET parent_agent_id = ? WHERE parent_agent_id = ?")
            .bind(existing.parent_agent_id)
            .bind(id)
            .execute(&mut *tx)
            .await
            .map_err(backend_error)?;
        delete_entity_search_documents(&mut tx, "agent", &id.to_string())
            .await
            .map_err(agent_search_error)?;
        sqlx::query("DELETE FROM agents WHERE id = ?")
            .bind(id)
            .execute(&mut *tx)
            .await
            .map_err(backend_error)?;
        tx.commit().await.map_err(backend_error)?;
        Ok(())
    }

    /// Atomically replace the default Agent. The new default MUST
    /// be an existing root Agent (`parent_agent_id IS NULL`).
    pub async fn set_default(&self, new_default_id: i64) -> Result<AgentRecord, AgentStoreError> {
        let new_default = self
            .fetch_one(new_default_id)
            .await?
            .ok_or(AgentStoreError {
                kind: AgentStoreErrorKind::NotFound,
            })?;
        if new_default.parent_agent_id.is_some() {
            return Err(AgentStoreError {
                kind: AgentStoreErrorKind::InvalidInput {
                    field: "id".to_string(),
                    reason: "must_be_root".to_string(),
                },
            });
        }
        let mut tx = self.pool.begin().await.map_err(backend_error)?;
        sqlx::query("UPDATE agents SET is_default = 0 WHERE is_default = 1")
            .execute(&mut *tx)
            .await
            .map_err(backend_error)?;
        sqlx::query("UPDATE agents SET is_default = 1 WHERE id = ?")
            .bind(new_default_id)
            .execute(&mut *tx)
            .await
            .map_err(backend_error)?;
        tx.commit().await.map_err(backend_error)?;
        self.fetch_one(new_default_id)
            .await?
            .ok_or(AgentStoreError {
                kind: AgentStoreErrorKind::NotFound,
            })
    }

    /// Fetch the unique default root Agent, if any.
    pub async fn default_agent(&self) -> Result<Option<AgentRecord>, AgentStoreError> {
        let row: Option<(i64,)> =
            sqlx::query_as("SELECT id FROM agents WHERE is_default = 1 ORDER BY id ASC LIMIT 1")
                .fetch_optional(&self.pool)
                .await
                .map_err(backend_error)?;
        match row {
            Some((id,)) => self.fetch_one(id).await,
            None => Ok(None),
        }
    }

    /// List the direct children of a given Agent id, in stable
    /// identifier order.
    pub async fn list_children(&self, parent_id: i64) -> Result<Vec<AgentRecord>, AgentStoreError> {
        let rows: Vec<(i64,)> = sqlx::query_as(
            "SELECT id FROM agents WHERE parent_agent_id = ? ORDER BY identifier ASC",
        )
        .bind(parent_id)
        .fetch_all(&self.pool)
        .await
        .map_err(backend_error)?;
        let mut out = Vec::with_capacity(rows.len());
        for (id,) in rows {
            if let Some(record) = self.fetch_one(id).await? {
                out.push(record);
            }
        }
        Ok(out)
    }

    /// Fetch a single Agent by id, with explicit and always-on
    /// skill sets loaded.
    pub async fn fetch_one(&self, id: i64) -> Result<Option<AgentRecord>, AgentStoreError> {
        let row: Option<AgentBaseRow> =
            sqlx::query_as(
                "SELECT id, identifier, name, description, system_prompt, parent_agent_id, depth, is_default, model_preset, category_id, created_at, updated_at \
                 FROM agents WHERE id = ?",
            )
            .bind(id)
            .fetch_optional(&self.pool)
            .await
            .map_err(backend_error)?;
        let Some((
            id,
            identifier,
            name,
            description,
            system_prompt,
            parent_agent_id,
            depth,
            is_default_i64,
            model_preset,
            category_id,
            created_at,
            updated_at,
        )) = row
        else {
            return Ok(None);
        };
        let tool_ids = load_tool_ids(&self.pool, id).await?;
        let skill_ids = load_skill_ids(&self.pool, id).await?;
        let always_skill_ids = load_always_skill_ids(&self.pool).await?;
        let capability_names = load_capability_names(&self.pool, id).await?;
        Ok(Some(AgentRecord {
            id,
            identifier,
            name,
            description,
            system_prompt,
            parent_agent_id,
            depth,
            is_default: is_default_i64 != 0,
            model_preset,
            category_id,
            tool_ids,
            skill_ids,
            always_skill_ids,
            capability_names,
            created_at,
            updated_at,
        }))
    }

    /// Load up to an arbitrary caller-selected Agent id set using exactly four
    /// indexed production queries: base rows, Tools, Skills (explicit plus
    /// globally always-on), and Capabilities. The query count is independent
    /// of batch cardinality.
    pub async fn load_resource_snapshots(
        &self,
        ids: &[i64],
    ) -> Result<Vec<AgentRecord>, AgentStoreError> {
        if ids.is_empty() {
            return Ok(Vec::new());
        }
        let ids_json = serde_json::to_string(ids).map_err(|error| AgentStoreError {
            kind: AgentStoreErrorKind::Backend(format!("agent id batch: {error}")),
        })?;

        self.record_query("t115.agent.resource.base");
        let rows = sqlx::query_as::<_, AgentBaseRow>(AGENT_RESOURCE_BASE_SQL)
            .bind(&ids_json)
            .fetch_all(&self.pool)
            .await
            .map_err(backend_error)?;

        self.record_query("t115.agent.resource.tools");
        let tool_rows = sqlx::query_as::<_, (i64, i64)>(AGENT_RESOURCE_TOOLS_SQL)
            .bind(&ids_json)
            .fetch_all(&self.pool)
            .await
            .map_err(backend_error)?;

        self.record_query("t115.agent.resource.skills");
        let skill_rows = sqlx::query_as::<_, (i64, i64, i64)>(AGENT_RESOURCE_SKILLS_SQL)
            .bind(&ids_json)
            .bind(&ids_json)
            .fetch_all(&self.pool)
            .await
            .map_err(backend_error)?;

        self.record_query("t115.agent.resource.capabilities");
        let capability_rows = sqlx::query_as::<_, (i64, String)>(AGENT_RESOURCE_CAPABILITIES_SQL)
            .bind(&ids_json)
            .fetch_all(&self.pool)
            .await
            .map_err(backend_error)?;

        let mut tools = std::collections::BTreeMap::<i64, Vec<i64>>::new();
        for (agent_id, tool_id) in tool_rows {
            tools.entry(agent_id).or_default().push(tool_id);
        }
        let mut skills = std::collections::BTreeMap::<i64, Vec<i64>>::new();
        let mut always_skills = std::collections::BTreeMap::<i64, Vec<i64>>::new();
        for (agent_id, skill_id, is_always) in skill_rows {
            if is_always == 0 {
                skills.entry(agent_id).or_default().push(skill_id);
            } else {
                always_skills.entry(agent_id).or_default().push(skill_id);
            }
        }
        let mut capabilities = std::collections::BTreeMap::<i64, Vec<String>>::new();
        for (agent_id, capability) in capability_rows {
            capabilities.entry(agent_id).or_default().push(capability);
        }

        Ok(rows
            .into_iter()
            .map(|row| {
                let id = row.0;
                agent_record_from_row(
                    row,
                    tools.remove(&id).unwrap_or_default(),
                    skills.remove(&id).unwrap_or_default(),
                    always_skills.remove(&id).unwrap_or_default(),
                    capabilities.remove(&id).unwrap_or_default(),
                )
            })
            .collect())
    }

    /// Search / page Agents. `page_size` MUST equal
    /// [`AGENT_PAGE_SIZE`].
    pub async fn search(&self, filter: &AgentFilter) -> Result<AgentPage, AgentStoreError> {
        if filter.page < 1 {
            return Err(invalid("page", "out_of_range"));
        }
        if filter.page_size != AGENT_PAGE_SIZE {
            return Err(invalid("page_size", "fixed_value_required"));
        }
        let offset = (filter.page - 1) * filter.page_size;
        let route = AgentSearchRoute::for_input(filter.search.as_deref())?;
        let (rows, total) = self
            .fetch_agent_window(&route, filter.page_size, offset)
            .await?;
        let records = hydrate_many(&self.pool, rows).await?;
        Ok(AgentPage {
            records,
            total,
            page: filter.page,
            page_size: filter.page_size,
        })
    }

    /// Count records matching the filter (no paging).
    pub async fn count(&self, filter: &AgentFilter) -> Result<i64, AgentStoreError> {
        let route = AgentSearchRoute::for_input(filter.search.as_deref())?;
        self.fetch_agent_count(&route).await
    }

    async fn fetch_agent_window(
        &self,
        route: &AgentSearchRoute,
        limit: i64,
        offset: i64,
    ) -> Result<(Vec<AgentBaseRow>, i64), AgentStoreError> {
        match route {
            AgentSearchRoute::Unfiltered => {
                let rows = sqlx::query_as::<_, AgentBaseRow>(AGENT_UNFILTERED_LIST_SQL)
                    .bind(limit)
                    .bind(offset)
                    .fetch_all(&self.pool)
                    .await
                    .map_err(backend_error)?;
                let total = sqlx::query_scalar::<_, i64>(AGENT_UNFILTERED_COUNT_SQL)
                    .fetch_one(&self.pool)
                    .await
                    .map_err(backend_error)?;
                Ok((rows, total))
            }
            AgentSearchRoute::FtsPhrase(phrase) => {
                let rows = sqlx::query_as::<_, AgentBaseRow>(AGENT_FTS_LIST_SQL)
                    .bind(phrase)
                    .bind(limit)
                    .bind(offset)
                    .fetch_all(&self.pool)
                    .await
                    .map_err(backend_error)?;
                let total = sqlx::query_scalar::<_, i64>(AGENT_FTS_COUNT_SQL)
                    .bind(phrase)
                    .fetch_one(&self.pool)
                    .await
                    .map_err(backend_error)?;
                Ok((rows, total))
            }
            AgentSearchRoute::ShortGram { length, gram } => {
                let rows = sqlx::query_as::<_, AgentBaseRow>(AGENT_SHORT_GRAM_LIST_SQL)
                    .bind(length)
                    .bind(gram)
                    .bind(limit)
                    .bind(offset)
                    .fetch_all(&self.pool)
                    .await
                    .map_err(backend_error)?;
                let total = sqlx::query_scalar::<_, i64>(AGENT_SHORT_GRAM_COUNT_SQL)
                    .bind(length)
                    .bind(gram)
                    .fetch_one(&self.pool)
                    .await
                    .map_err(backend_error)?;
                Ok((rows, total))
            }
        }
    }

    async fn fetch_agent_count(&self, route: &AgentSearchRoute) -> Result<i64, AgentStoreError> {
        match route {
            AgentSearchRoute::Unfiltered => sqlx::query_scalar(AGENT_UNFILTERED_COUNT_SQL)
                .fetch_one(&self.pool)
                .await
                .map_err(backend_error),
            AgentSearchRoute::FtsPhrase(phrase) => sqlx::query_scalar(AGENT_FTS_COUNT_SQL)
                .bind(phrase)
                .fetch_one(&self.pool)
                .await
                .map_err(backend_error),
            AgentSearchRoute::ShortGram { length, gram } => {
                sqlx::query_scalar(AGENT_SHORT_GRAM_COUNT_SQL)
                    .bind(length)
                    .bind(gram)
                    .fetch_one(&self.pool)
                    .await
                    .map_err(backend_error)
            }
        }
    }

    /// Fetch all children of the given parent (depth = 1).
    pub async fn direct_children(
        &self,
        parent_agent_id: i64,
    ) -> Result<Vec<AgentRecord>, AgentStoreError> {
        let rows: Vec<(i64, String, String, Option<String>, String, Option<i64>, i64, i64, Option<String>, Option<i64>, String, String)> = sqlx::query_as(
            "SELECT id, identifier, name, description, system_prompt, parent_agent_id, depth, is_default, model_preset, category_id, created_at, updated_at \
             FROM agents WHERE parent_agent_id = ? ORDER BY name ASC",
        )
        .bind(parent_agent_id)
        .fetch_all(&self.pool)
        .await
        .map_err(backend_error)?;
        hydrate_many(&self.pool, rows).await
    }

    /// EXPLAIN QUERY PLAN of the search query (used by the
    /// query-plan contract).
    pub async fn explain_search_plan(
        &self,
        filter: &AgentFilter,
    ) -> Result<Vec<String>, AgentStoreError> {
        let normalized_search = filter.search.as_deref().map(normalize_agent_name);
        let offset = (filter.page.max(1) - 1) * filter.page_size.max(AGENT_PAGE_SIZE);
        let page_size = filter.page_size.max(AGENT_PAGE_SIZE);
        let rows: Vec<(String,)> = if let Some(norm) = normalized_search {
            let pattern = format!("%{norm}%");
            sqlx::query_as("EXPLAIN QUERY PLAN SELECT id FROM agents WHERE name_normalized LIKE ? OR identifier LIKE ? ORDER BY name ASC LIMIT ? OFFSET ?")
                .bind(pattern.clone())
                .bind(pattern)
                .bind(page_size)
                .bind(offset)
                .fetch_all(&self.pool)
                .await
                .map_err(backend_error)?
        } else {
            sqlx::query_as(
                "EXPLAIN QUERY PLAN SELECT id FROM agents ORDER BY name ASC LIMIT ? OFFSET ?",
            )
            .bind(page_size)
            .bind(offset)
            .fetch_all(&self.pool)
            .await
            .map_err(backend_error)?
        };
        Ok(rows.into_iter().map(|(detail,)| detail).collect())
    }

    /// Whether assigning `parent_id` to `agent_id` would create a
    /// cycle. The walk uses indexed `parent_agent_id` reads.
    async fn would_form_cycle(
        &self,
        agent_id: i64,
        parent_id: i64,
    ) -> Result<bool, AgentStoreError> {
        let mut current: Option<i64> = Some(parent_id);
        let mut visited = std::collections::HashSet::new();
        visited.insert(agent_id);
        while let Some(id) = current {
            if id == agent_id {
                return Ok(true);
            }
            if !visited.insert(id) {
                return Ok(false);
            }
            let next: Option<Option<i64>> =
                sqlx::query_scalar("SELECT parent_agent_id FROM agents WHERE id = ?")
                    .bind(id)
                    .fetch_optional(&self.pool)
                    .await
                    .map_err(backend_error)?;
            current = match next {
                Some(value) => value,
                None => return Ok(false),
            };
        }
        Ok(false)
    }

    /// Validate that any FK / model_preset / category references
    /// in the input are real. Zero-modification invariant: this
    /// runs inside the caller's transaction and either errors
    /// before any write or completes successfully.
    async fn validate_references(&self, input: &AgentInput) -> Result<(), AgentStoreError> {
        if let Some(preset) = input.model_preset.as_deref() {
            let exists: i64 = sqlx::query_scalar(AGENT_MODEL_PRESET_EXISTS_SQL)
                .bind(preset)
                .fetch_one(&self.pool)
                .await
                .map_err(backend_error)?;
            if exists == 0 {
                return Err(AgentStoreError {
                    kind: AgentStoreErrorKind::InvalidInput {
                        field: "model_preset".to_string(),
                        reason: "not_found".to_string(),
                    },
                });
            }
        }
        if let Some(category_id) = input.category_id {
            let exists: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM categories WHERE id = ?")
                .bind(category_id)
                .fetch_one(&self.pool)
                .await
                .map_err(backend_error)?;
            if exists == 0 {
                return Err(AgentStoreError {
                    kind: AgentStoreErrorKind::InvalidInput {
                        field: "category_id".to_string(),
                        reason: "not_found".to_string(),
                    },
                });
            }
        }
        for tool_id in &input.tool_ids {
            let exists: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM tools WHERE id = ?")
                .bind(tool_id)
                .fetch_one(&self.pool)
                .await
                .map_err(backend_error)?;
            if exists == 0 {
                return Err(AgentStoreError {
                    kind: AgentStoreErrorKind::InvalidInput {
                        field: "tool_ids".to_string(),
                        reason: "not_found".to_string(),
                    },
                });
            }
        }
        for skill_id in &input.skill_ids {
            let exists: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM skills WHERE id = ?")
                .bind(skill_id)
                .fetch_one(&self.pool)
                .await
                .map_err(backend_error)?;
            if exists == 0 {
                return Err(AgentStoreError {
                    kind: AgentStoreErrorKind::InvalidInput {
                        field: "skill_ids".to_string(),
                        reason: "not_found".to_string(),
                    },
                });
            }
        }
        for skill_id in dedup_i64(&input.always_skill_ids) {
            let exists: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM skills WHERE id = ?")
                .bind(skill_id)
                .fetch_one(&self.pool)
                .await
                .map_err(backend_error)?;
            if exists == 0 {
                return Err(AgentStoreError {
                    kind: AgentStoreErrorKind::InvalidInput {
                        field: "always_skill_ids".to_string(),
                        reason: "not_found".to_string(),
                    },
                });
            }
        }
        for capability in &input.capability_names {
            let exists: i64 =
                sqlx::query_scalar("SELECT COUNT(*) FROM capabilities WHERE name = ?")
                    .bind(capability)
                    .fetch_one(&self.pool)
                    .await
                    .map_err(backend_error)?;
            if exists == 0 {
                return Err(AgentStoreError {
                    kind: AgentStoreErrorKind::InvalidInput {
                        field: "capability_names".to_string(),
                        reason: "not_found".to_string(),
                    },
                });
            }
        }
        Ok(())
    }

    fn record_query(&self, query_id: &'static str) {
        if let Some(observer) = self.observer.as_ref() {
            observer.record_checked_query(query_id);
        }
    }
}

// ===== Private helpers used by the typed AgentStore =====

fn validate_input(input: &AgentInput) -> Result<(), AgentStoreError> {
    if input.identifier.trim().is_empty() {
        return Err(invalid("identifier", "empty"));
    }
    if input.identifier.len() > 255 {
        return Err(invalid("identifier", "too_long"));
    }
    if !input
        .identifier
        .chars()
        .all(|c| c.is_ascii_alphanumeric() || c == '_' || c == '-')
    {
        return Err(invalid("identifier", "invalid_charset"));
    }
    if input.name.trim().is_empty() {
        return Err(invalid("name", "empty"));
    }
    if input.name.len() > 255 {
        return Err(invalid("name", "too_long"));
    }
    if let Some(ref desc) = input.description {
        if desc.len() > 2000 {
            return Err(invalid("description", "too_long"));
        }
        if desc.chars().any(|c| c.is_control()) {
            return Err(invalid("description", "control_character"));
        }
    }
    if input.system_prompt.trim().is_empty() {
        return Err(invalid("system_prompt", "empty"));
    }
    if input.system_prompt.len() > 1024 * 1024 {
        return Err(invalid("system_prompt", "too_long"));
    }
    if input.system_prompt.chars().any(|c| c.is_control()) {
        return Err(invalid("system_prompt", "control_character"));
    }
    Ok(())
}

fn backend_error(error: sqlx::Error) -> AgentStoreError {
    AgentStoreError {
        kind: AgentStoreErrorKind::Backend(error.to_string()),
    }
}

async fn replace_agent_search_documents(
    transaction: &mut sqlx::Transaction<'_, Sqlite>,
    id: i64,
    input: &AgentInput,
) -> Result<(), AgentStoreError> {
    replace_entity_search_documents(
        transaction,
        "agent",
        &id.to_string(),
        &[("identifier", &input.identifier), ("name", &input.name)],
    )
    .await
    .map_err(agent_search_error)
}

fn agent_store_kind_not_found() -> AgentStoreError {
    AgentStoreError {
        kind: AgentStoreErrorKind::NotFound,
    }
}

fn dedup_i64(values: &[i64]) -> Vec<i64> {
    let mut seen = std::collections::BTreeSet::new();
    let mut out = Vec::new();
    for value in values {
        if seen.insert(*value) {
            out.push(*value);
        }
    }
    out
}

fn dedup_string(values: &[String]) -> Vec<String> {
    let mut seen = std::collections::BTreeSet::new();
    let mut out = Vec::new();
    for value in values {
        let trimmed = value.trim();
        if trimmed.is_empty() {
            continue;
        }
        if seen.insert(trimmed.to_string()) {
            out.push(trimmed.to_string());
        }
    }
    out
}

async fn load_tool_ids(pool: &Pool<Sqlite>, agent_id: i64) -> Result<Vec<i64>, AgentStoreError> {
    let rows: Vec<(i64,)> =
        sqlx::query_as("SELECT tool_id FROM agent_tools WHERE agent_id = ? ORDER BY tool_id ASC")
            .bind(agent_id)
            .fetch_all(pool)
            .await
            .map_err(backend_error)?;
    Ok(rows.into_iter().map(|(id,)| id).collect())
}

async fn load_skill_ids(pool: &Pool<Sqlite>, agent_id: i64) -> Result<Vec<i64>, AgentStoreError> {
    let rows: Vec<(i64,)> = sqlx::query_as(
        "SELECT skill_id FROM agent_skills WHERE agent_id = ? ORDER BY skill_id ASC",
    )
    .bind(agent_id)
    .fetch_all(pool)
    .await
    .map_err(backend_error)?;
    Ok(rows.into_iter().map(|(id,)| id).collect())
}

async fn load_always_skill_ids(pool: &Pool<Sqlite>) -> Result<Vec<i64>, AgentStoreError> {
    let rows: Vec<(i64,)> =
        sqlx::query_as("SELECT id FROM skills WHERE is_always = 1 ORDER BY id ASC")
            .fetch_all(pool)
            .await
            .map_err(backend_error)?;
    Ok(rows.into_iter().map(|(id,)| id).collect())
}

async fn load_capability_names(
    pool: &Pool<Sqlite>,
    agent_id: i64,
) -> Result<Vec<String>, AgentStoreError> {
    let rows: Vec<(String,)> = sqlx::query_as(
        "SELECT capability_name FROM agent_capabilities WHERE agent_id = ? ORDER BY capability_name ASC",
    )
    .bind(agent_id)
    .fetch_all(pool)
    .await
    .map_err(backend_error)?;
    Ok(rows.into_iter().map(|(name,)| name).collect())
}

async fn hydrate_many(
    pool: &Pool<Sqlite>,
    rows: Vec<AgentBaseRow>,
) -> Result<Vec<AgentRecord>, AgentStoreError> {
    let mut out = Vec::with_capacity(rows.len());
    for row in rows {
        let id = row.0;
        let tool_ids = load_tool_ids(pool, id).await?;
        let skill_ids = load_skill_ids(pool, id).await?;
        let always_skill_ids = load_always_skill_ids(pool).await?;
        let capability_names = load_capability_names(pool, id).await?;
        out.push(agent_record_from_row(
            row,
            tool_ids,
            skill_ids,
            always_skill_ids,
            capability_names,
        ));
    }
    Ok(out)
}

fn agent_record_from_row(
    row: AgentBaseRow,
    tool_ids: Vec<i64>,
    skill_ids: Vec<i64>,
    always_skill_ids: Vec<i64>,
    capability_names: Vec<String>,
) -> AgentRecord {
    let (
        id,
        identifier,
        name,
        description,
        system_prompt,
        parent_agent_id,
        depth,
        is_default_i64,
        model_preset,
        category_id,
        created_at,
        updated_at,
    ) = row;
    AgentRecord {
        id,
        identifier,
        name,
        description,
        system_prompt,
        parent_agent_id,
        depth,
        is_default: is_default_i64 != 0,
        model_preset,
        category_id,
        tool_ids,
        skill_ids,
        always_skill_ids,
        capability_names,
        created_at,
        updated_at,
    }
}

async fn update_row(
    tx: &mut sqlx::Transaction<'_, sqlx::Sqlite>,
    id: i64,
    input: &AgentInput,
    normalized: &str,
    now: &str,
    keep_default: bool,
) -> Result<(), AgentStoreError> {
    let result = sqlx::query(
        "UPDATE agents SET identifier=?, name=?, description=?, system_prompt=?, parent_agent_id=?, model_preset=?, category_id=?, name_normalized=?, updated_at=?, is_default=? WHERE id=?",
    )
    .bind(&input.identifier)
    .bind(&input.name)
    .bind(&input.description)
    .bind(&input.system_prompt)
    .bind(input.parent_agent_id)
    .bind(&input.model_preset)
    .bind(input.category_id)
    .bind(normalized)
    .bind(now)
    .bind(if keep_default { 1_i64 } else { 0_i64 })
    .bind(id)
    .execute(&mut **tx)
    .await;
    if let Err(error) = result {
        if is_unique_violation(&error) {
            return Err(AgentStoreError {
                kind: AgentStoreErrorKind::Conflict(AgentConflict::DuplicateIdentifier {
                    value: input.identifier.clone(),
                }),
            });
        }
        return Err(backend_error(error));
    }
    Ok(())
}

async fn replace_associations(
    tx: &mut sqlx::Transaction<'_, sqlx::Sqlite>,
    id: i64,
    input: &AgentInput,
    now: &str,
) -> Result<(), AgentStoreError> {
    sqlx::query("DELETE FROM agent_tools WHERE agent_id = ?")
        .bind(id)
        .execute(&mut **tx)
        .await
        .map_err(backend_error)?;
    sqlx::query("DELETE FROM agent_skills WHERE agent_id = ?")
        .bind(id)
        .execute(&mut **tx)
        .await
        .map_err(backend_error)?;
    sqlx::query("DELETE FROM agent_capabilities WHERE agent_id = ?")
        .bind(id)
        .execute(&mut **tx)
        .await
        .map_err(backend_error)?;
    for tool_id in dedup_i64(&input.tool_ids) {
        sqlx::query(
            "INSERT OR IGNORE INTO agent_tools (agent_id, tool_id, created_at) VALUES (?, ?, ?)",
        )
        .bind(id)
        .bind(tool_id)
        .bind(now)
        .execute(&mut **tx)
        .await
        .map_err(backend_error)?;
    }
    for skill_id in dedup_i64(&input.skill_ids) {
        sqlx::query(
            "INSERT OR IGNORE INTO agent_skills (agent_id, skill_id, created_at) VALUES (?, ?, ?)",
        )
        .bind(id)
        .bind(skill_id)
        .bind(now)
        .execute(&mut **tx)
        .await
        .map_err(backend_error)?;
    }
    for capability in dedup_string(&input.capability_names) {
        sqlx::query(
            "INSERT OR IGNORE INTO agent_capabilities (agent_id, capability_name, created_at) VALUES (?, ?, ?)",
        )
        .bind(id)
        .bind(&capability)
        .bind(now)
        .execute(&mut **tx)
        .await
        .map_err(backend_error)?;
    }
    Ok(())
}

async fn apply_subtree_depth_shift(
    tx: &mut sqlx::Transaction<'_, sqlx::Sqlite>,
    root_id: i64,
    delta: i64,
    max_depth: i64,
) -> Result<(), AgentStoreError> {
    if delta == 0 {
        return Ok(());
    }
    // BFS from the root, capping the depth at `max_depth`.
    let mut frontier = vec![root_id];
    while let Some(id) = frontier.pop() {
        let current_depth: Option<i64> =
            sqlx::query_scalar("SELECT depth FROM agents WHERE id = ?")
                .bind(id)
                .fetch_optional(&mut **tx)
                .await
                .map_err(backend_error)?;
        if let Some(depth) = current_depth {
            let new_depth = depth + delta;
            if new_depth < 0 || new_depth > max_depth {
                return Err(AgentStoreError {
                    kind: AgentStoreErrorKind::HierarchyViolation {
                        reason: "depth_exceeded".to_string(),
                        references: Vec::new(),
                    },
                });
            }
            sqlx::query("UPDATE agents SET depth = ? WHERE id = ?")
                .bind(new_depth)
                .bind(id)
                .execute(&mut **tx)
                .await
                .map_err(backend_error)?;
            let children: Vec<(i64,)> =
                sqlx::query_as("SELECT id FROM agents WHERE parent_agent_id = ?")
                    .bind(id)
                    .fetch_all(&mut **tx)
                    .await
                    .map_err(backend_error)?;
            for (child_id,) in children {
                frontier.push(child_id);
            }
        }
    }
    Ok(())
}
