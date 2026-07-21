use anyhow::Result;
use chrono::Utc;
use sqlx::{Pool, Row, Sqlite};
use std::time::{Duration, Instant};

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
            let kind = func.get("kind").and_then(|v| v.as_i64()).unwrap_or(1);
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
            let kind = tool.get("kind").and_then(|v| v.as_i64()).unwrap_or(1);
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
                .bind(id)
                .bind(identifier)
                .bind(name)
                .bind(description)
                .bind(system_prompt)
                .bind(parent_agent_id)
                .bind(depth)
                .bind(model_preset)
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
    if let sqlx::Error::Database(db_err) = &err {
        if db_err.message().contains("UNIQUE constraint failed") {
            tracing::warn!(
                entity_type = "unknown",
                field = field_name,
                value = field_value,
                "UNIQUE constraint violation"
            );
            return anyhow::anyhow!("{} '{}' 已存在，请使用其他值", field_name, field_value);
        }
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
    pub created_at: String,
    pub updated_at: String,
    pub deleted_at: Option<String>,
}

#[derive(Debug, Clone)]
pub struct Function {
    pub id: i64,
    pub identifier: String,
    pub name: String,
    pub description: Option<String>,
    pub kind: i64,
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
    pub kind: i64,
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
            sqlx::query_as::<_, Tag>(
                "SELECT id, name, color, created_at FROM tags WHERE name LIKE ? ORDER BY name LIMIT ? OFFSET ?"
            )
            .bind(format!("%{}%", search_term))
            .bind(limit)
            .bind(offset)
            .fetch_all(pool)
            .await?
        } else {
            sqlx::query_as::<_, Tag>(
                "SELECT id, name, color, created_at FROM tags ORDER BY name LIMIT ? OFFSET ?",
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
            sqlx::query_scalar::<_, i64>("SELECT COUNT(*) FROM tags WHERE name LIKE ?")
                .bind(format!("%{}%", search_term))
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
        let result = sqlx::query_scalar::<_, i64>(
            "INSERT INTO tags (name, color, created_at) VALUES (?, ?, ?) RETURNING id",
        )
        .bind(&name)
        .bind(&color)
        .bind(now)
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
        let result = sqlx::query("UPDATE tags SET name = ?, color = ? WHERE id = ?")
            .bind(&name)
            .bind(&color)
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

        // 将子分类的 parent_id 置为 NULL（级联解绑）
        sqlx::query("UPDATE categories SET parent_id = NULL WHERE parent_id = ?")
            .bind(id)
            .execute(pool)
            .await?;

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
                "SELECT id, identifier, name, description, manifest, runtime, version, author, repository_url, s3_key, sha256, size_bytes, category_id, created_at, updated_at, deleted_at FROM plugins WHERE deleted_at IS NULL AND (name LIKE ? OR identifier LIKE ?) ORDER BY name LIMIT ? OFFSET ?"
            ).bind(format!("%{}%", s)).bind(format!("%{}%", s)).bind(limit).bind(offset).fetch_all(pool).await?
        } else {
            sqlx::query_as::<_, Plugin>(
                "SELECT id, identifier, name, description, manifest, runtime, version, author, repository_url, s3_key, sha256, size_bytes, category_id, created_at, updated_at, deleted_at FROM plugins WHERE deleted_at IS NULL ORDER BY name LIMIT ? OFFSET ?"
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
            "SELECT id, identifier, name, description, manifest, runtime, version, author, repository_url, s3_key, sha256, size_bytes, category_id, created_at, updated_at, deleted_at FROM plugins WHERE id = ?"
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
        ).bind(&identifier).bind(&name).bind(&description).bind(&manifest).bind(&runtime).bind(&version).bind(&author).bind(&repository_url).bind(&s3_key).bind(&sha256).bind(size_bytes).bind(category_id).bind(&now).bind(&now)
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

        let start = Instant::now();
        let now = Utc::now().to_rfc3339();
        let result = sqlx::query(
            "UPDATE plugins SET identifier=?, name=?, description=?, manifest=?, runtime=?, version=?, author=?, repository_url=?, s3_key=?, sha256=?, size_bytes=?, category_id=?, updated_at=? WHERE id=?"
        ).bind(&identifier).bind(&name).bind(&description).bind(&manifest).bind(&runtime).bind(&version).bind(&author).bind(&repository_url).bind(&s3_key).bind(&sha256).bind(size_bytes).bind(category_id).bind(&now).bind(id)
            .execute(pool).await;

        result.map_err(|e| handle_unique_constraint_error(e, "identifier", &identifier))?;

        let duration = start.elapsed().as_millis();
        tracing::info!(entity = "plugin", op = "update", id = id, identifier = %identifier, name = %name, duration_ms = duration, "Plugin updated");

        Plugin::get(pool, id)
            .await?
            .ok_or_else(|| anyhow::anyhow!("Plugin not found"))
    }

    pub async fn delete(pool: &Pool<Sqlite>, id: i64) -> Result<()> {
        let start = Instant::now();
        let now = Utc::now().to_rfc3339();
        sqlx::query("UPDATE plugins SET deleted_at = ? WHERE id = ?")
            .bind(&now)
            .bind(id)
            .execute(pool)
            .await?;

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

    /// Get the local WASM file path for this plugin
    pub fn wasm_path(&self, base_dir: &std::path::Path) -> std::path::PathBuf {
        base_dir
            .join("plugins")
            .join(self.id.to_string())
            .join("plugin.wasm")
    }

    /// Ensure the plugin directory exists
    pub fn ensure_plugin_dir(
        base_dir: &std::path::Path,
        plugin_id: i64,
    ) -> Result<std::path::PathBuf> {
        let plugin_dir = base_dir.join("plugins").join(plugin_id.to_string());
        std::fs::create_dir_all(&plugin_dir)?;
        Ok(plugin_dir)
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
            created_at: row.try_get("created_at")?,
            updated_at: row.try_get("updated_at")?,
            deleted_at: row.try_get("deleted_at")?,
        })
    }
}

// === Function CRUD ===
impl Function {
    pub async fn list(
        pool: &Pool<Sqlite>,
        search: Option<String>,
        limit: i64,
        offset: i64,
    ) -> Result<Vec<Function>> {
        let fns = if let Some(s) = search {
            sqlx::query_as::<_, Function>(
                "SELECT id, identifier, name, description, kind, input_schema, output_schema, plugin_id, plugin_export, category_id, required_capabilities, created_at, updated_at FROM functions WHERE name LIKE ? OR identifier LIKE ? ORDER BY name LIMIT ? OFFSET ?"
            ).bind(format!("%{}%", s)).bind(format!("%{}%", s)).bind(limit).bind(offset).fetch_all(pool).await?
        } else {
            sqlx::query_as::<_, Function>(
                "SELECT id, identifier, name, description, kind, input_schema, output_schema, plugin_id, plugin_export, category_id, required_capabilities, created_at, updated_at FROM functions ORDER BY name LIMIT ? OFFSET ?"
            ).bind(limit).bind(offset).fetch_all(pool).await?
        };
        Ok(fns)
    }

    pub async fn count(pool: &Pool<Sqlite>, search: Option<String>) -> Result<i64> {
        let c = if let Some(s) = search {
            sqlx::query_scalar::<_, i64>(
                "SELECT COUNT(*) FROM functions WHERE name LIKE ? OR identifier LIKE ?",
            )
            .bind(format!("%{}%", s))
            .bind(format!("%{}%", s))
            .fetch_one(pool)
            .await?
        } else {
            sqlx::query_scalar::<_, i64>("SELECT COUNT(*) FROM functions")
                .fetch_one(pool)
                .await?
        };
        Ok(c)
    }

    pub async fn get(pool: &Pool<Sqlite>, id: i64) -> Result<Option<Function>> {
        let f = sqlx::query_as::<_, Function>(
            "SELECT id, identifier, name, description, kind, input_schema, output_schema, plugin_id, plugin_export, category_id, required_capabilities, created_at, updated_at FROM functions WHERE id = ?"
        ).bind(id).fetch_optional(pool).await?;
        Ok(f)
    }

    pub async fn create(
        pool: &Pool<Sqlite>,
        identifier: String,
        name: String,
        description: Option<String>,
        kind: i64,
        input_schema: String,
        output_schema: String,
        plugin_id: Option<i64>,
        plugin_export: Option<String>,
        category_id: Option<i64>,
        required_capabilities: Option<String>,
    ) -> Result<Function> {
        validate_identifier(&identifier)?;
        validate_name(&name)?;
        if let Some(ref desc) = description {
            validate_description(desc)?;
        }
        validate_json(&input_schema)?;
        validate_json(&output_schema)?;

        let start = Instant::now();
        let now = Utc::now().to_rfc3339();
        let result = sqlx::query_scalar::<_, i64>(
            "INSERT INTO functions (identifier, name, description, kind, input_schema, output_schema, plugin_id, plugin_export, category_id, required_capabilities, created_at, updated_at) VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?) RETURNING id"
        ).bind(&identifier).bind(&name).bind(&description).bind(kind).bind(&input_schema).bind(&output_schema).bind(plugin_id).bind(&plugin_export).bind(category_id).bind(&required_capabilities).bind(&now).bind(&now)
            .fetch_one(pool).await;

        let id =
            result.map_err(|e| handle_unique_constraint_error(e, "identifier", &identifier))?;

        let duration = start.elapsed().as_millis();
        tracing::info!(entity = "function", op = "create", id = id, identifier = %identifier, name = %name, kind = kind, duration_ms = duration, "Function created");

        Function::get(pool, id)
            .await?
            .ok_or_else(|| anyhow::anyhow!("Failed to retrieve created function"))
    }

    pub async fn update(
        pool: &Pool<Sqlite>,
        id: i64,
        identifier: String,
        name: String,
        description: Option<String>,
        kind: i64,
        input_schema: String,
        output_schema: String,
        plugin_id: Option<i64>,
        plugin_export: Option<String>,
        category_id: Option<i64>,
        required_capabilities: Option<String>,
    ) -> Result<Function> {
        validate_identifier(&identifier)?;
        validate_name(&name)?;
        if let Some(ref desc) = description {
            validate_description(desc)?;
        }
        validate_json(&input_schema)?;
        validate_json(&output_schema)?;

        let start = Instant::now();
        let now = Utc::now().to_rfc3339();
        let result = sqlx::query(
            "UPDATE functions SET identifier=?, name=?, description=?, kind=?, input_schema=?, output_schema=?, plugin_id=?, plugin_export=?, category_id=?, required_capabilities=?, updated_at=? WHERE id=?"
        ).bind(&identifier).bind(&name).bind(&description).bind(kind).bind(&input_schema).bind(&output_schema).bind(plugin_id).bind(&plugin_export).bind(category_id).bind(&required_capabilities).bind(&now).bind(id)
            .execute(pool).await;

        result.map_err(|e| handle_unique_constraint_error(e, "identifier", &identifier))?;

        let duration = start.elapsed().as_millis();
        tracing::info!(entity = "function", op = "update", id = id, identifier = %identifier, name = %name, duration_ms = duration, "Function updated");

        Function::get(pool, id)
            .await?
            .ok_or_else(|| anyhow::anyhow!("Function not found"))
    }

    pub async fn delete(pool: &Pool<Sqlite>, id: i64) -> Result<()> {
        let start = Instant::now();
        sqlx::query("DELETE FROM functions WHERE id = ?")
            .bind(id)
            .execute(pool)
            .await?;

        let duration = start.elapsed().as_millis();
        tracing::info!(
            entity = "function",
            op = "delete",
            id = id,
            duration_ms = duration,
            "Function deleted"
        );

        Ok(())
    }
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
        kind: i64,
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
        if kind == 1 && function_id.is_none() {
            return Err(anyhow::anyhow!("kind=function 时 function_id 不能为空"));
        }
        if kind == 2 && workflow_id.is_none() {
            return Err(anyhow::anyhow!("kind=workflow 时 workflow_id 不能为空"));
        }

        let start = Instant::now();
        let now = Utc::now().to_rfc3339();
        let result = sqlx::query_scalar::<_, i64>(
            "INSERT INTO tools (identifier, name, description, kind, source, is_always, function_id, workflow_id, input_schema, output_schema, category_id, required_capabilities, created_at, updated_at) VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?) RETURNING id"
        ).bind(&identifier).bind(&name).bind(&description).bind(kind).bind(&source).bind(is_always as i64).bind(function_id).bind(workflow_id).bind(&input_schema).bind(&output_schema).bind(category_id).bind(&required_capabilities).bind(&now).bind(&now)
            .fetch_one(pool).await;

        let id =
            result.map_err(|e| handle_unique_constraint_error(e, "identifier", &identifier))?;

        let duration = start.elapsed().as_millis();
        tracing::info!(entity = "tool", op = "create", id = id, identifier = %identifier, name = %name, kind = kind, duration_ms = duration, "Tool created");

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
        kind: i64,
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
        if kind == 1 && function_id.is_none() {
            return Err(anyhow::anyhow!("kind=function 时 function_id 不能为空"));
        }
        if kind == 2 && workflow_id.is_none() {
            return Err(anyhow::anyhow!("kind=workflow 时 workflow_id 不能为空"));
        }

        let start = Instant::now();
        let now = Utc::now().to_rfc3339();
        let result = sqlx::query(
            "UPDATE tools SET identifier=?, name=?, description=?, kind=?, source=?, is_always=?, function_id=?, workflow_id=?, input_schema=?, output_schema=?, category_id=?, required_capabilities=?, updated_at=? WHERE id=?"
        ).bind(&identifier).bind(&name).bind(&description).bind(kind).bind(&source).bind(is_always as i64).bind(function_id).bind(workflow_id).bind(&input_schema).bind(&output_schema).bind(category_id).bind(&required_capabilities).bind(&now).bind(id)
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
                "SELECT id, identifier, name, description, system_prompt, parent_agent_id, depth, model_preset, created_at, updated_at FROM agents WHERE name LIKE ? OR identifier LIKE ? ORDER BY name LIMIT ? OFFSET ?"
            ).bind(format!("%{}%", s)).bind(format!("%{}%", s)).bind(limit).bind(offset).fetch_all(pool).await?
        } else {
            sqlx::query_as::<_, Agent>(
                "SELECT id, identifier, name, description, system_prompt, parent_agent_id, depth, model_preset, created_at, updated_at FROM agents ORDER BY name LIMIT ? OFFSET ?"
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
            "SELECT id, identifier, name, description, system_prompt, parent_agent_id, depth, model_preset, created_at, updated_at FROM agents WHERE id = ?"
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
        if let Some(pid) = parent_agent_id {
            if pid <= 0 {
                return Err(anyhow::anyhow!("parent_agent_id 必须为正整数"));
            }
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
        Ok(Agent {
            id: row.try_get("id")?,
            identifier: row.try_get("identifier")?,
            name: row.try_get("name")?,
            description: row.try_get("description")?,
            system_prompt: row.try_get("system_prompt")?,
            parent_agent_id: row.try_get("parent_agent_id")?,
            depth: row.try_get("depth")?,
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
            let kind = func.get("kind").and_then(|v| v.as_i64()).unwrap_or(1);
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
            let kind = tool.get("kind").and_then(|v| v.as_i64()).unwrap_or(1);
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
        "SELECT id, identifier, name, description, manifest, runtime, version, author, repository_url, s3_key, sha256, size_bytes, category_id, created_at, updated_at, deleted_at FROM plugins"
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
/// 当前版本: 1.0
const CURRENT_SCHEMA_VERSION: i64 = 1;

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

    // 未来版本迁移可以在这里添加
    // if current_version < 2 { ... }

    tracing::info!("数据库迁移完成");
    Ok(())
}

/// 初始化所有新增实体表
pub async fn init_tables(pool: &Pool<Sqlite>) -> Result<()> {
    // tags 表
    sqlx::query(
        r#"
        CREATE TABLE IF NOT EXISTS tags (
            id INTEGER PRIMARY KEY AUTOINCREMENT,
            name TEXT NOT NULL UNIQUE,
            color TEXT,
            created_at TEXT NOT NULL
        )
        "#,
    )
    .execute(pool)
    .await?;

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

    // functions 表（identifier 唯一）
    sqlx::query(
        r#"
        CREATE TABLE IF NOT EXISTS functions (
            id INTEGER PRIMARY KEY AUTOINCREMENT,
            identifier TEXT NOT NULL UNIQUE,
            name TEXT NOT NULL,
            description TEXT,
            kind INTEGER NOT NULL DEFAULT 1,
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
            kind INTEGER NOT NULL DEFAULT 1,
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
                (kind = 1 AND function_id IS NOT NULL AND workflow_id IS NULL) OR
                (kind = 2 AND workflow_id IS NOT NULL AND function_id IS NULL)
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
            model_preset TEXT,
            created_at TEXT NOT NULL,
            updated_at TEXT NOT NULL
        )
        "#,
    )
    .execute(pool)
    .await?;

    // workflow_nodes 表（DAG 节点）
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

    // workflow_edges 表（DAG 连线）
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

    // 创建索引
    sqlx::query("CREATE INDEX IF NOT EXISTS idx_tags_name ON tags(name)")
        .execute(pool)
        .await?;
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
    use super::*;

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
