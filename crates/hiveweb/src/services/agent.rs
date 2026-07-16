//! Agent service (T116 / US5)
//!
//! 不变量（data-model §4）：
//!   #1  main agent (identifier='main', id=1) 不可删除（→ 5001）；非 Super 不可修改其 system_prompt/permissions（→ 2001）
//!   #2  depth = parent.depth + 1，最大 10（→ 5006）
//!   #8  is_dangerous capability 仅 Super (role=3) 可授予（→ 2001）
//!   #10 model_preset 必须命中启动加载的 preset 集合（→ 5007）

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use sqlx::MySqlPool;
use std::collections::HashSet;
use std::sync::Arc;

use crate::models::Agent;
use crate::runtime::capability::CapabilityRegistry;
use crate::runtime::llm::LlmRegistry;
use crate::utils::error::AppError;

pub const MAX_DEPTH: i8 = 10;
pub const MAIN_AGENT_IDENTIFIER: &str = "main";

#[derive(Debug, Deserialize)]
pub struct CreateMeta {
    pub identifier: String,
    pub name: String,
    pub description: Option<String>,
    pub system_prompt: String,
    pub parent_agent_id: Option<i64>,
    pub model_preset: Option<String>,
    #[serde(default)]
    pub tool_ids: Vec<i64>,
    #[serde(default)]
    pub skill_ids: Vec<i64>,
    #[serde(default)]
    pub permissions: Vec<String>,
}

#[derive(Debug, Deserialize)]
pub struct UpdateMeta {
    pub name: Option<String>,
    pub description: Option<String>,
    pub system_prompt: Option<String>,
    pub parent_agent_id: Option<i64>,
    pub model_preset: Option<String>,
    #[serde(default)]
    pub tool_ids: Option<Vec<i64>>,
    #[serde(default)]
    pub skill_ids: Option<Vec<i64>>,
    #[serde(default)]
    pub permissions: Option<Vec<String>>,
    pub updated_at: DateTime<Utc>,
}

#[derive(Debug, Serialize)]
pub struct AgentDetail {
    #[serde(flatten)]
    pub agent: Agent,
    pub tools: Vec<ToolBrief>,
    pub skills: Vec<SkillBrief>,
    pub permissions: Vec<String>,
}

#[derive(Debug, Serialize, sqlx::FromRow)]
pub struct ToolBrief {
    pub id: i64,
    pub identifier: String,
    pub name: String,
}

#[derive(Debug, Serialize, sqlx::FromRow)]
pub struct SkillBrief {
    pub id: i64,
    pub identifier: String,
    pub name: String,
}

#[derive(Debug, Serialize)]
pub struct AgentTreeNode {
    #[serde(flatten)]
    pub agent: Agent,
    pub children: Vec<AgentTreeNode>,
}

/// 校验 model_preset 是否合法
fn check_model_preset(llm: &LlmRegistry, preset: Option<&str>) -> Result<(), AppError> {
    let Some(p) = preset else { return Ok(()) };
    if !llm.presets.contains_key(p) {
        return Err(AppError::ModelPresetUnknown(format!(
            "模型 preset「{p}」不存在，请重新选择"
        )));
    }
    Ok(())
}

/// 校验：若 permissions 包含任何 dangerous capability，则当前 actor 必须是 Super
fn check_dangerous_permissions(
    registry: &CapabilityRegistry,
    permissions: &[String],
    actor_role: i8,
) -> Result<(), AppError> {
    for p in permissions {
        if registry.is_dangerous(p) && actor_role != 3 {
            return Err(AppError::InsufficientPermission(format!(
                "授予危险能力「{p}」需 Super 角色"
            )));
        }
    }
    // 校验所有 capability 存在
    for p in permissions {
        if registry.lookup(p).is_none() {
            return Err(AppError::BadRequest(format!("未知 capability「{p}」")));
        }
    }
    Ok(())
}

pub async fn create(
    pool: &MySqlPool,
    redis: &redis::Client,
    registry: &CapabilityRegistry,
    llm: &LlmRegistry,
    actor_role: i8,
    meta: CreateMeta,
) -> Result<AgentDetail, AppError> {
    check_model_preset(llm, meta.model_preset.as_deref())?;
    check_dangerous_permissions(registry, &meta.permissions, actor_role)?;

    // 计算 depth
    let depth = if let Some(pid) = meta.parent_agent_id {
        let p: Option<(i8,)> = sqlx::query_as("SELECT depth FROM agents WHERE id = ?")
            .bind(pid)
            .fetch_optional(pool)
            .await
            .map_err(|e| AppError::Internal(format!("parent lookup: {e}")))?;
        let parent_depth = p
            .ok_or_else(|| AppError::NotFound(format!("parent agent id={pid} not found")))?
            .0;
        if parent_depth + 1 > MAX_DEPTH {
            return Err(AppError::AgentDepthExceeded(format!(
                "Agent 层级已达最大深度 {MAX_DEPTH}"
            )));
        }
        parent_depth + 1
    } else {
        0
    };

    let mut tx = pool
        .begin()
        .await
        .map_err(|e| AppError::Internal(format!("tx begin: {e}")))?;

    let res = sqlx::query(
        r#"INSERT INTO agents
           (identifier, name, description, system_prompt, parent_agent_id, depth, model_preset)
           VALUES (?, ?, ?, ?, ?, ?, ?)"#,
    )
    .bind(&meta.identifier)
    .bind(&meta.name)
    .bind(&meta.description)
    .bind(&meta.system_prompt)
    .bind(meta.parent_agent_id)
    .bind(depth)
    .bind(&meta.model_preset)
    .execute(&mut *tx)
    .await
    .map_err(|e| {
        let msg = e.to_string();
        if msg.contains("Duplicate") {
            AppError::Conflict(format!("Agent identifier 已存在：{}", meta.identifier))
        } else {
            AppError::Internal(format!("agent insert: {e}"))
        }
    })?;
    let id = res.last_insert_id() as i64;

    for tid in &meta.tool_ids {
        sqlx::query("INSERT IGNORE INTO agent_tools (agent_id, tool_id) VALUES (?, ?)")
            .bind(id)
            .bind(tid)
            .execute(&mut *tx)
            .await
            .map_err(|e| AppError::Internal(format!("agent_tools: {e}")))?;
    }
    for sid in &meta.skill_ids {
        sqlx::query("INSERT IGNORE INTO agent_skills (agent_id, skill_id) VALUES (?, ?)")
            .bind(id)
            .bind(sid)
            .execute(&mut *tx)
            .await
            .map_err(|e| AppError::Internal(format!("agent_skills: {e}")))?;
    }
    for cap in &meta.permissions {
        sqlx::query("INSERT IGNORE INTO agent_permissions (agent_id, capability) VALUES (?, ?)")
            .bind(id)
            .bind(cap)
            .execute(&mut *tx)
            .await
            .map_err(|e| AppError::Internal(format!("agent_permissions: {e}")))?;
    }

    tx.commit()
        .await
        .map_err(|e| AppError::Internal(format!("tx commit: {e}")))?;

    // 新建子 agent → 父 agent 的 children 列表已变
    if let Some(pid) = meta.parent_agent_id {
        invalidate_content_cache(redis, pid).await;
    }

    fetch_detail(pool, id).await
}

pub async fn fetch_detail(pool: &MySqlPool, id: i64) -> Result<AgentDetail, AppError> {
    let agent: Agent = sqlx::query_as("SELECT * FROM agents WHERE id = ?")
        .bind(id)
        .fetch_optional(pool)
        .await
        .map_err(|e| AppError::Internal(format!("agent fetch: {e}")))?
        .ok_or_else(|| AppError::NotFound(format!("agent id={id} not found")))?;

    let tools: Vec<ToolBrief> = sqlx::query_as(
        r#"SELECT t.id, t.identifier, t.name FROM tools t
           JOIN agent_tools at ON at.tool_id = t.id
           WHERE at.agent_id = ?"#,
    )
    .bind(id)
    .fetch_all(pool)
    .await
    .map_err(|e| AppError::Internal(format!("tools list: {e}")))?;

    let skills: Vec<SkillBrief> = sqlx::query_as(
        r#"SELECT s.id, s.identifier, s.name FROM skills s
           JOIN agent_skills ax ON ax.skill_id = s.id
           WHERE ax.agent_id = ?"#,
    )
    .bind(id)
    .fetch_all(pool)
    .await
    .map_err(|e| AppError::Internal(format!("skills list: {e}")))?;

    let perms: Vec<(String,)> =
        sqlx::query_as("SELECT capability FROM agent_permissions WHERE agent_id = ?")
            .bind(id)
            .fetch_all(pool)
            .await
            .map_err(|e| AppError::Internal(format!("perms list: {e}")))?;

    Ok(AgentDetail {
        agent,
        tools,
        skills,
        permissions: perms.into_iter().map(|(c,)| c).collect(),
    })
}

pub async fn list_tree(pool: &MySqlPool) -> Result<Vec<AgentTreeNode>, AppError> {
    let rows: Vec<Agent> = sqlx::query_as("SELECT * FROM agents ORDER BY parent_agent_id, name")
        .fetch_all(pool)
        .await
        .map_err(|e| AppError::Internal(format!("agent list: {e}")))?;

    use std::collections::HashMap;
    let mut by_parent: HashMap<Option<i64>, Vec<Agent>> = HashMap::new();
    for r in rows {
        by_parent.entry(r.parent_agent_id).or_default().push(r);
    }

    fn build(
        parent: Option<i64>,
        map: &mut std::collections::HashMap<Option<i64>, Vec<Agent>>,
    ) -> Vec<AgentTreeNode> {
        let mut out = Vec::new();
        if let Some(rs) = map.remove(&parent) {
            for r in rs {
                let id = r.id;
                let children = build(Some(id), map);
                out.push(AgentTreeNode { agent: r, children });
            }
        }
        out
    }
    Ok(build(None, &mut by_parent))
}

pub async fn update(
    pool: &MySqlPool,
    redis: &redis::Client,
    registry: &CapabilityRegistry,
    llm: &LlmRegistry,
    actor_role: i8,
    id: i64,
    meta: UpdateMeta,
) -> Result<AgentDetail, AppError> {
    let existing: Agent = sqlx::query_as("SELECT * FROM agents WHERE id = ?")
        .bind(id)
        .fetch_optional(pool)
        .await
        .map_err(|e| AppError::Internal(format!("agent fetch: {e}")))?
        .ok_or_else(|| AppError::NotFound(format!("agent id={id} not found")))?;

    // main agent 非 Super 不可改
    if existing.identifier == MAIN_AGENT_IDENTIFIER && actor_role != 3 {
        return Err(AppError::InsufficientPermission(
            "入口 Agent「main」仅 Super 可修改".into(),
        ));
    }

    check_model_preset(llm, meta.model_preset.as_deref())?;
    if let Some(ref perms) = meta.permissions {
        check_dangerous_permissions(registry, perms, actor_role)?;
    }

    // 计算 depth（如果 parent_agent_id 有变更）
    let new_depth: Option<i8> =
        if meta.parent_agent_id.is_some() && meta.parent_agent_id != existing.parent_agent_id {
            let pid = meta.parent_agent_id.unwrap();
            // 不允许设置自己为父
            if pid == id {
                return Err(AppError::BadRequest("parent_agent_id 不能指向自身".into()));
            }
            let p: Option<(i8,)> = sqlx::query_as("SELECT depth FROM agents WHERE id = ?")
                .bind(pid)
                .fetch_optional(pool)
                .await
                .map_err(|e| AppError::Internal(format!("parent lookup: {e}")))?;
            let parent_depth = p
                .ok_or_else(|| AppError::NotFound(format!("parent agent id={pid} not found")))?
                .0;
            if parent_depth + 1 > MAX_DEPTH {
                return Err(AppError::AgentDepthExceeded(format!(
                    "Agent 层级已达最大深度 {MAX_DEPTH}"
                )));
            }
            Some(parent_depth + 1)
        } else {
            None
        };

    crate::services::optimistic_lock::check_and_bump(pool, "agents", id, meta.updated_at).await?;

    let mut tx = pool
        .begin()
        .await
        .map_err(|e| AppError::Internal(format!("tx begin: {e}")))?;

    sqlx::query(
        r#"UPDATE agents SET
              name = COALESCE(?, name),
              description = COALESCE(?, description),
              system_prompt = COALESCE(?, system_prompt),
              parent_agent_id = COALESCE(?, parent_agent_id),
              depth = COALESCE(?, depth),
              model_preset = COALESCE(?, model_preset)
           WHERE id = ?"#,
    )
    .bind(&meta.name)
    .bind(&meta.description)
    .bind(&meta.system_prompt)
    .bind(meta.parent_agent_id)
    .bind(new_depth)
    .bind(&meta.model_preset)
    .bind(id)
    .execute(&mut *tx)
    .await
    .map_err(|e| AppError::Internal(format!("agent update: {e}")))?;

    // 全量替换关联表
    if let Some(tids) = meta.tool_ids {
        sqlx::query("DELETE FROM agent_tools WHERE agent_id = ?")
            .bind(id)
            .execute(&mut *tx)
            .await
            .map_err(|e| AppError::Internal(format!("agent_tools clear: {e}")))?;
        for tid in &tids {
            sqlx::query("INSERT IGNORE INTO agent_tools (agent_id, tool_id) VALUES (?, ?)")
                .bind(id)
                .bind(tid)
                .execute(&mut *tx)
                .await
                .map_err(|e| AppError::Internal(format!("agent_tools insert: {e}")))?;
        }
    }
    if let Some(sids) = meta.skill_ids {
        sqlx::query("DELETE FROM agent_skills WHERE agent_id = ?")
            .bind(id)
            .execute(&mut *tx)
            .await
            .map_err(|e| AppError::Internal(format!("agent_skills clear: {e}")))?;
        for sid in &sids {
            sqlx::query("INSERT IGNORE INTO agent_skills (agent_id, skill_id) VALUES (?, ?)")
                .bind(id)
                .bind(sid)
                .execute(&mut *tx)
                .await
                .map_err(|e| AppError::Internal(format!("agent_skills insert: {e}")))?;
        }
    }
    if let Some(perms) = meta.permissions {
        sqlx::query("DELETE FROM agent_permissions WHERE agent_id = ?")
            .bind(id)
            .execute(&mut *tx)
            .await
            .map_err(|e| AppError::Internal(format!("agent_permissions clear: {e}")))?;
        for p in &perms {
            sqlx::query(
                "INSERT IGNORE INTO agent_permissions (agent_id, capability) VALUES (?, ?)",
            )
            .bind(id)
            .bind(p)
            .execute(&mut *tx)
            .await
            .map_err(|e| AppError::Internal(format!("agent_permissions insert: {e}")))?;
        }
    }

    tx.commit()
        .await
        .map_err(|e| AppError::Internal(format!("tx commit: {e}")))?;

    // 自己必失效；parent_agent_id 真改动了的话旧父/新父的 children 列表也要刷
    invalidate_content_cache(redis, id).await;
    if meta.parent_agent_id.is_some() && meta.parent_agent_id != existing.parent_agent_id {
        if let Some(old_pid) = existing.parent_agent_id {
            invalidate_content_cache(redis, old_pid).await;
        }
        if let Some(new_pid) = meta.parent_agent_id {
            invalidate_content_cache(redis, new_pid).await;
        }
    }

    fetch_detail(pool, id).await
}

pub async fn delete(pool: &MySqlPool, redis: &redis::Client, id: i64) -> Result<(), AppError> {
    let row: Option<(String, Option<i64>)> =
        sqlx::query_as("SELECT identifier, parent_agent_id FROM agents WHERE id = ?")
            .bind(id)
            .fetch_optional(pool)
            .await
            .map_err(|e| AppError::Internal(format!("agent fetch: {e}")))?;
    let Some((ident, parent_id)) = row else {
        return Err(AppError::NotFound(format!("agent id={id} not found")));
    };
    if ident == MAIN_AGENT_IDENTIFIER {
        return Err(AppError::CannotDeleteMainAgent(
            "入口 Agent「main」不可删除".into(),
        ));
    }
    // 有子 Agent → 4093
    let cnt: (i64,) = sqlx::query_as("SELECT COUNT(*) FROM agents WHERE parent_agent_id = ?")
        .bind(id)
        .fetch_one(pool)
        .await
        .map_err(|e| AppError::Internal(format!("agent child count: {e}")))?;
    if cnt.0 > 0 {
        return Err(AppError::ResourceInUse(format!(
            "agent 有 {} 个子 Agent，无法删除",
            cnt.0
        )));
    }

    sqlx::query("DELETE FROM agents WHERE id = ?")
        .bind(id)
        .execute(pool)
        .await
        .map_err(|e| AppError::Internal(format!("agent delete: {e}")))?;

    // 自己 + 父（children 列表变了）
    invalidate_content_cache(redis, id).await;
    if let Some(pid) = parent_id {
        invalidate_content_cache(redis, pid).await;
    }
    Ok(())
}

/// 工具函数：给某 agent 加载 permissions（dispatcher 调用过）
#[allow(dead_code)]
pub async fn permissions_of(pool: &MySqlPool, agent_id: i64) -> Result<HashSet<String>, AppError> {
    let rows: Vec<(String,)> =
        sqlx::query_as("SELECT capability FROM agent_permissions WHERE agent_id = ?")
            .bind(agent_id)
            .fetch_all(pool)
            .await
            .map_err(|e| AppError::Internal(format!("perms fetch: {e}")))?;
    Ok(rows.into_iter().map(|(c,)| c).collect())
}

/// 启动 init 与 service 之间的便利：把 Arc<LlmRegistry> 转给 service
#[allow(dead_code)]
pub fn _llm_smoke(_: Arc<LlmRegistry>) {}

// =====================================================================
// Runtime content — orchestrator 用的 agent 视图（system_prompt + tools +
// permissions + children + hooks）。原 build_agent_content 已迁到这里。
// 类型需要跨模块（orchestrator / runtime::skill_test / runtime::tool_test）共享，
// 因此放在 services 层并 pub 出去。
// =====================================================================

use std::collections::HashMap;

use serde_json::Value;

use crate::models::agent_hook::AgentHook;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AgentContent {
    pub agent_id: i64,
    pub identifier: String,
    pub system_prompt: String,
    pub model_preset: Option<String>,
    pub tools: Vec<ToolRef>,
    pub permissions: Vec<String>,
    pub children: Vec<ChildAgent>,
    pub hooks: HashMap<String, Vec<AgentHook>>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ToolRef {
    pub id: i64,
    pub identifier: String,
    pub name: String,
    pub description: String,
    pub kind: i8, // 1 function-wrap, 2 workflow-wrap
    pub function_id: Option<i64>,
    /// function 的 identifier（builtin 查找用，与 tool identifier 可能不同）
    pub function_identifier: Option<String>,
    pub workflow_id: Option<i64>,
    pub input_schema: Value,
    pub plugin_id: Option<i64>,
    pub plugin_export: Option<String>,
    pub is_builtin_function: bool,
    /// 元工具（meta-tool）：kind=1 且 function_id=NULL（如 invoke_function / invoke_workflow）
    pub is_meta_tool: bool,
    pub required_capabilities: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ChildAgent {
    pub id: i64,
    pub identifier: String,
    pub description: Option<String>,
}

// ---------- AgentContent 缓存（Redis） ----------
//
// 减少 orchestrator 每次 hop 重新跑 5+ 条 SQL 的压力。key 形如
// `agent:content:{id}`，TTL 默认 5 分钟（可用 AGENT_CACHE_TTL_SECS 覆盖）。
// 任何修改 agent / agent_tools / agent_skills / agent_permissions 的写路径
// 都要调 `invalidate_content_cache` 同步失效。

fn agent_content_key(agent_id: i64) -> String {
    format!("agent:content:{agent_id}")
}

fn agent_content_ttl_secs() -> u64 {
    std::env::var("AGENT_CACHE_TTL_SECS")
        .ok()
        .and_then(|v| v.parse().ok())
        .unwrap_or(300)
}

pub async fn invalidate_content_cache(redis: &redis::Client, agent_id: i64) {
    let key = agent_content_key(agent_id);
    match redis.get_multiplexed_async_connection().await {
        Ok(mut conn) => {
            let res: redis::RedisResult<i64> = redis::AsyncCommands::del(&mut conn, &key).await;
            if let Err(e) = res {
                tracing::warn!(error=%e, agent_id, "agent content cache invalidate failed");
            }
        }
        Err(e) => {
            tracing::warn!(error=%e, agent_id, "agent content cache invalidate (connect) failed")
        }
    }
}

/// 加载 agent 运行时所需的全部内容：system_prompt（含 skill markdown）+ tools
/// (agent-specific + always tools，按 id 去重) + permissions + children + hooks。
///
/// 主记录查询失败会返回 `AppError`，子查询（skills/tools/perms/children/hooks）
/// 失败时按 orchestrator 历史行为降级为空集合，保证单步失败不阻塞会话。
///
/// 走 Redis 缓存：每次先查 `agent:content:{id}`，命中即返回；未命中查 DB 后
/// 回写缓存（默认 TTL = `AGENT_CACHE_TTL_SECS`，默认 300s）。Redis 不可用时
/// 仅 `warn!` 并降级走 DB，不影响业务正确性。
pub async fn fetch_content(
    pool: &MySqlPool,
    redis: &redis::Client,
    agent_id: i64,
) -> Result<AgentContent, AppError> {
    let cache_key = agent_content_key(agent_id);

    // 1) 尝试命中缓存
    if let Some(cached) = read_content_cache(redis, &cache_key).await {
        return Ok(cached);
    }

    // 2) 缓存未命中或 Redis 不可用：走 DB（沿用原逻辑）
    let row: Option<(String, String, Option<String>)> =
        sqlx::query_as("SELECT identifier, system_prompt, model_preset FROM agents WHERE id = ?")
            .bind(agent_id)
            .fetch_optional(pool)
            .await
            .map_err(|e| AppError::Internal(format!("agent fetch: {e}")))?;
    let (identifier, mut system_prompt, model_preset) =
        row.ok_or_else(|| AppError::NotFound(format!("agent id={agent_id} not found")))?;

    // Skill markdown 拼到 system prompt
    let skills: Vec<(String,)> = sqlx::query_as(
        r#"SELECT s.content FROM skills s
           JOIN agent_skills ax ON ax.skill_id = s.id
           WHERE ax.agent_id = ?"#,
    )
    .bind(agent_id)
    .fetch_all(pool)
    .await
    .unwrap_or_default();
    for (md,) in &skills {
        system_prompt.push_str("\n\n");
        system_prompt.push_str(md);
    }

    // Tools: agent-specific + always tools (deduplicated by tool id)
    let tool_rows: Vec<(
        i64,
        String,
        String,
        String,
        i8,
        Option<i64>,
        Option<i64>,
        Value,
        Option<i64>,
        Option<String>,
        Option<String>,
        Option<Value>,
    )> = sqlx::query_as(
        r#"SELECT t.id, t.identifier, t.name, t.description, t.kind,
                      t.function_id, t.workflow_id, t.input_schema,
                      f.plugin_id, f.plugin_export, f.identifier,
                      COALESCE(t.required_capabilities, f.required_capabilities)
               FROM tools t
               JOIN agent_tools at ON at.tool_id = t.id
               LEFT JOIN functions f ON f.id = t.function_id
               WHERE at.agent_id = ?
               UNION
               SELECT t.id, t.identifier, t.name, t.description, t.kind,
                      t.function_id, t.workflow_id, t.input_schema,
                      f.plugin_id, f.plugin_export, f.identifier,
                      COALESCE(t.required_capabilities, f.required_capabilities)
               FROM tools t
               LEFT JOIN functions f ON f.id = t.function_id
               WHERE t.is_always = 1
                 AND t.id NOT IN (
                     SELECT at2.tool_id FROM agent_tools at2 WHERE at2.agent_id = ?
                 )"#,
    )
    .bind(agent_id)
    .bind(agent_id)
    .fetch_all(pool)
    .await
    .unwrap_or_default();

    let mut tools: Vec<ToolRef> = Vec::new();
    for (
        id,
        identifier,
        name,
        desc,
        kind,
        function_id,
        workflow_id,
        input_schema,
        plugin_id,
        plugin_export,
        function_identifier,
        caps_json,
    ) in tool_rows
    {
        let is_builtin_function = kind == 1 && plugin_id.is_none();
        let is_meta_tool = kind == 1 && function_id.is_none();
        let required_capabilities: Vec<String> = caps_json
            .and_then(|v| v.as_array().cloned())
            .map(|arr| {
                arr.into_iter()
                    .filter_map(|v| v.as_str().map(String::from))
                    .collect()
            })
            .unwrap_or_default();
        tools.push(ToolRef {
            id,
            identifier,
            name,
            description: desc,
            kind,
            function_id,
            function_identifier,
            workflow_id,
            input_schema,
            plugin_id,
            plugin_export,
            is_builtin_function,
            is_meta_tool,
            required_capabilities,
        });
    }

    // Permissions
    let perms: Vec<(String,)> =
        sqlx::query_as("SELECT capability FROM agent_permissions WHERE agent_id = ?")
            .bind(agent_id)
            .fetch_all(pool)
            .await
            .unwrap_or_default();

    // Children
    let children_rows: Vec<(i64, String, Option<String>)> =
        sqlx::query_as("SELECT id, identifier, description FROM agents WHERE parent_agent_id = ?")
            .bind(agent_id)
            .fetch_all(pool)
            .await
            .unwrap_or_default();
    let children: Vec<ChildAgent> = children_rows
        .into_iter()
        .map(|(id, ident, desc)| ChildAgent {
            id,
            identifier: ident,
            description: desc,
        })
        .collect();

    let hooks = crate::services::agent_hook::load_hooks_for_agent(pool, agent_id)
        .await
        .unwrap_or_default();

    let content = AgentContent {
        agent_id,
        identifier,
        system_prompt,
        model_preset,
        tools,
        permissions: perms.into_iter().map(|(c,)| c).collect(),
        children,
        hooks,
    };

    // 3) 写回缓存（失败仅 warn，不影响本次返回）
    write_content_cache(redis, &cache_key, &content, agent_id).await;

    Ok(content)
}

async fn read_content_cache(redis: &redis::Client, key: &str) -> Option<AgentContent> {
    let mut conn = match redis.get_multiplexed_async_connection().await {
        Ok(c) => c,
        Err(e) => {
            tracing::warn!(error=%e, key, "agent content cache connect failed");
            return None;
        }
    };
    let cached: Option<String> = match redis::AsyncCommands::get(&mut conn, key).await {
        Ok(v) => v,
        Err(e) => {
            tracing::warn!(error=%e, key, "agent content cache get failed");
            return None;
        }
    };
    let Some(json) = cached else {
        return None;
    };
    match serde_json::from_str::<AgentContent>(&json) {
        Ok(c) => Some(c),
        Err(e) => {
            tracing::warn!(error=%e, key, "agent content cache deserialize failed");
            None
        }
    }
}

async fn write_content_cache(
    redis: &redis::Client,
    key: &str,
    content: &AgentContent,
    agent_id: i64,
) {
    let json = match serde_json::to_string(content) {
        Ok(j) => j,
        Err(e) => {
            tracing::warn!(error=%e, agent_id, "agent content cache serialize failed");
            return;
        }
    };
    let mut conn = match redis.get_multiplexed_async_connection().await {
        Ok(c) => c,
        Err(e) => {
            tracing::warn!(error=%e, agent_id, "agent content cache connect (write) failed");
            return;
        }
    };
    let ttl = agent_content_ttl_secs();
    let res: redis::RedisResult<()> = redis::AsyncCommands::set_ex(&mut conn, key, json, ttl).await;
    if let Err(e) = res {
        tracing::warn!(error=%e, agent_id, "agent content cache setex failed");
    }
}
