use serde_json::Value;
use std::sync::Arc;
use std::time::Duration;
use uuid::Uuid;

use crate::cache::redis::RedisClient;
use crate::runtime::execution_context::RuntimeExecutionContext;
use crate::runtime::llm::{LlmAdapterError, LlmRegistry};
use crate::runtime::llm_audit::{
    LlmAuditGuard, LlmAuditSource, LlmLocalFallbackReason, record_local_fallback,
};
use providers::{ChatRequest, LLMProvider, LLMResponse, LlmCallOptions};

use super::{BuiltinContext, BuiltinError, BuiltinResult};

/// sync wrapper: bridges async DB queries inside the tokio runtime via `block_in_place`.
/// client_type and channel are extracted from AgentContext metadata.
pub fn game_info(args: Value, ctx: &BuiltinContext) -> BuiltinResult {
    let agent_ctx = ctx.agent_ctx.clone();
    let channel = agent_ctx
        .as_ref()
        .and_then(|ac| ac.get_user_metadata("channel"))
        .unwrap_or_default();
    let client_type = agent_ctx
        .as_ref()
        .and_then(|ac| ac.get_user_metadata("client_type"))
        .unwrap_or_default();
    let target_client_type = agent_ctx
        .as_ref()
        .and_then(|ac| ac.get_metadata("target_client_type"))
        .filter(|s| !s.is_empty());

    let pool = ctx.pool.clone();
    let ext_pool = ctx.ext_pool.cloned();
    let redis = ctx.redis.cloned();
    let llm = ctx.llm.cloned();
    let agent_id = ctx.agent_id;
    let execution_context = ctx.execution_context.clone();
    tokio::task::block_in_place(move || {
        tokio::runtime::Handle::current().block_on(async move {
            game_info_async_impl(
                args,
                &pool,
                ext_pool.as_ref(),
                redis.as_ref(),
                &channel,
                &client_type,
                llm.as_ref(),
                agent_id,
                execution_context.as_ref(),
                target_client_type.as_deref(),
            )
            .await
        })
    })
}

/// Query a single game's details by game_id (text input, parsed to integer).
/// Input: { "game_id": "<text>" }
/// - game_id: 文本形式的游戏 ID，内部转为数字。id <= 0 或转换失败 → LLM 分类推荐。
/// - 游戏信息始终写入 AgentContext extensions (card/game)。
#[expect(
    clippy::too_many_arguments,
    reason = "the builtin wrapper passes each optional runtime dependency explicitly"
)]
async fn game_info_async_impl(
    args: Value,
    pool: &sqlx::MySqlPool,
    ext_pool: Option<&sqlx::MySqlPool>,
    redis: Option<&RedisClient>,
    channel: &str,
    client_type: &str,
    llm: Option<&Arc<LlmRegistry>>,
    agent_id: Option<i64>,
    execution_context: Option<&RuntimeExecutionContext>,
    target_client_type: Option<&str>,
) -> BuiltinResult {
    // 1. Parse game_id from input — text → integer
    let game_id_str = args.get("game_id").and_then(|v| v.as_str()).unwrap_or("");

    tracing::debug!(
        game_id_bytes = game_id_str.len(),
        channel_bytes = channel.len(),
        client_type_bytes = client_type.len(),
        has_llm = llm.is_some(),
        has_agent_id = agent_id.is_some(),
        "game_info called"
    );

    let game_id: i64 = match game_id_str.trim().parse() {
        Ok(id) if id > 0 => {
            tracing::debug!(game_id = %id, "game_info: direct lookup by id");
            id
        }
        _ => {
            tracing::debug!("game_info: game_id <= 0 or parse failed, entering classify path");
            return handle_classify_and_list(
                args,
                pool,
                ext_pool,
                redis,
                channel,
                client_type,
                llm,
                agent_id,
                execution_context,
                target_client_type,
            )
            .await;
        }
    };

    let ext_pool = ext_pool.ok_or_else(|| BuiltinError::Exec("外部数据库未配置".into()))?;

    // 2. Query external DB with caching — single top-priority game info
    let game_info = crate::services::game_service::get_single_external_game_info(
        ext_pool,
        game_id,
        client_type,
        channel,
    )
    .await
    .map_err(|e| BuiltinError::Exec(format!("{e}")))?;

    let game_info = match game_info {
        Some(info) if info.logic_game_id != 0 => {
            tracing::debug!(
                logic_game_id = %info.logic_game_id,
                "game_info: found by direct lookup"
            );
            info
        }
        _ => {
            tracing::warn!(game_id = %game_id, "game_info: direct lookup returned empty");
            return Ok(serde_json::json!({
                "found": false,
                "data": "未找到相关游戏"
            }));
        }
    };

    let id = game_info.logic_game_id;
    let name = &game_info.name;

    let mut game_payload = serde_json::json!({
        "id": id,
        "name": name,
        "channel": game_info.channel,
        "client_type": game_info.client_type,
        "reason": game_info.description,
        "game_tags": game_info.game_tags,
        "cover_image": game_info.cover_image,
        "computer_id": game_info.computer_id,
        "platform_name": game_info.platform_name,
        "game_icon": game_info.game_icon,
    });

    // 如果目标客户端平台与当前不一致，提醒用户并隐藏 computer_id
    let cross_platform = target_client_type
        .filter(|t| !t.eq_ignore_ascii_case(client_type))
        .is_some();

    if cross_platform && let Some(obj) = game_payload.as_object_mut() {
        obj.remove("computer_id");
    }

    let mut output = serde_json::json!({
        "found": true,
        "id": id,
        "name": name,
        "description": game_info.raw_description,
        "cover_image": game_info.cover_image,
        "game_tags": game_info.game_tags,
        "computer_id": game_info.computer_id,
        "platform_name": game_info.platform_name,
        "client_type": game_info.client_type,
        "channel": game_info.channel,
        "game_icon": game_info.game_icon,
    });

    if cross_platform && let Some(obj) = output.as_object_mut() {
        obj.remove("computer_id");
        obj.insert(
            "data".into(),
            serde_json::Value::String(format!(
                "注意：游戏《{}》信息为 {} 客户端，与您当前使用的 {} 客户端，需要到{}客户端才能玩。",
                game_info.name,
                target_client_type.unwrap_or(""),
                client_type,
                target_client_type.unwrap_or(""),
            )),
        );
    }

    // 6. 写入 AgentContext extensions
    let extension = serde_json::json!({
        "content_type": "card",
        "payload": {
            "type": "game",
            "info": game_payload,
        },
    });
    output["_agent_context_updates"] = serde_json::json!({
        "extensions": [extension],
    });

    Ok(output)
}

// ─── game_id == 0: LLM 分类 + 外部数据库查询 ──────────────────────────────

/// Fetch game categories (tags) from the external cc_game_tag table.
///
/// Queries `SELECT id, name FROM cc_game_tag WHERE type = 1`.
/// Results are cached in Redis for 10 minutes.
/// Returns `Ok(Vec::new())` if no rows are returned.
async fn fetch_categories(
    ext_pool: &sqlx::MySqlPool,
    redis: Option<&RedisClient>,
) -> Result<Vec<(i64, String)>, String> {
    if let Some(r) = redis {
        let cache_key = "game_tags:cc_game_tag_type1";
        crate::services::cache_helper::cached_or_fetch(
            r,
            cache_key,
            600, // 10 min TTL
            || async {
                let rows: Vec<(i64, String)> =
                    sqlx::query_as("SELECT id, name FROM cc_game_tag WHERE `type` = 1")
                        .fetch_all(ext_pool)
                        .await
                        .map_err(|e| format!("cc_game_tag query: {e}"))?;
                Ok::<Vec<(i64, String)>, String>(rows)
            },
        )
        .await
    } else {
        let rows: Vec<(i64, String)> =
            sqlx::query_as("SELECT id, name FROM cc_game_tag WHERE `type` = 1")
                .fetch_all(ext_pool)
                .await
                .map_err(|e| format!("cc_game_tag query: {e}"))?;
        Ok(rows)
    }
}

/// Build conversation context from _agent_context messages for LLM classification.
/// Takes the last 3 messages, formats as "role: content" pairs.
fn build_classify_context(
    args: &Value,
    #[expect(
        unused_variables,
        reason = "the classification input contract is retained pending a product decision on prompt composition"
    )]
    user_input: &str,
) -> String {
    let messages: Vec<&Value> = args
        .get("_agent_context")
        .and_then(|ac| ac.get("messages"))
        .and_then(|v| v.as_array())
        .map(|arr| {
            let len = arr.len();
            if len > 3 {
                arr.iter().skip(len - 3).collect()
            } else {
                arr.iter().collect()
            }
        })
        .unwrap_or_default();

    let mut lines: Vec<String> = Vec::new();
    for msg in &messages {
        let role = msg.get("role").and_then(|v| v.as_str()).unwrap_or("");
        let content = msg.get("content").and_then(|v| v.as_str()).unwrap_or("");
        if !content.is_empty() {
            let label = if role == "user" { "用户" } else { "助手" };
            lines.push(format!("{label}: {content}"));
        }
    }

    lines.join("\n")
}

/// Build an LLM classification prompt to identify the game category from conversation context.
fn build_classification_prompt(context: &str, categories: &[(i64, String)]) -> String {
    let cat_list = categories
        .iter()
        .map(|(id, name)| format!("- id:{}, name:{}", id, name))
        .collect::<Vec<_>>()
        .join("\n");

    format!(
        r#"你是一个游戏分类助手。请根据对话内容判断对话中最相关的游戏分类。

可选的游戏分类如下：
{cat_list}

对话内容："{context}"

请只返回一个最匹配的分类 ID（数字），不要输出其他任何内容。如果无法判断，请返回第一个分类的 ID。"#
    )
}

/// Handle game_id == 0 path: classify user input → return top 3 games for the category.
#[expect(
    clippy::too_many_arguments,
    reason = "keeps the builtin dispatcher dependencies explicit"
)]
async fn handle_classify_and_list(
    args: Value,
    pool: &sqlx::MySqlPool,
    ext_pool: Option<&sqlx::MySqlPool>,
    redis: Option<&RedisClient>,
    channel: &str,
    client_type: &str,
    llm: Option<&Arc<LlmRegistry>>,
    agent_id: Option<i64>,
    execution_context: Option<&RuntimeExecutionContext>,
    target_client_type: Option<&str>,
) -> BuiltinResult {
    // 1. 获取用户输入文本（从 args._agent_context.user_input.raw_text 读取）
    let user_input = args
        .get("_agent_context")
        .and_then(|ac| ac.get("user_input"))
        .and_then(|ui| ui.get("raw_text"))
        .and_then(|v| v.as_str())
        .unwrap_or("")
        .to_string();

    if user_input.is_empty() {
        return Ok(serde_json::json!({
            "found": false,
            "data": "未找到相关游戏"
        }));
    }

    // 从 AgentContext 取最近 3 条消息构建对话上下文
    let context = build_classify_context(&args, &user_input);

    // 2. 从外部 DB 获取游戏分类标签
    let ext_pool = match ext_pool {
        Some(p) => p,
        None => {
            tracing::warn!("ext_pool not available, game_info classify aborted");
            return Ok(serde_json::json!({
                "found": false,
                "data": "未找到相关游戏"
            }));
        }
    };

    let categories = match fetch_categories(ext_pool, redis).await {
        Ok(cats) if !cats.is_empty() => {
            tracing::debug!(count = cats.len(), "game_info: fetched categories");
            cats
        }
        Ok(_) => {
            tracing::warn!("cc_game_tag returned empty, game_info classify aborted");
            return Ok(serde_json::json!({
                "found": false,
                "data": "未找到相关游戏"
            }));
        }
        Err(_) => {
            tracing::warn!(
                error_kind = "category_query_failed",
                "fetch_categories failed, game_info classify aborted"
            );
            return Ok(serde_json::json!({
                "found": false,
                "data": "未找到相关游戏"
            }));
        }
    };
    let category_id = match classify_game_category(
        &context,
        &categories,
        pool,
        llm,
        agent_id,
        execution_context,
    )
    .await?
    {
        Some(id) => id,
        None => {
            tracing::warn!(
                user_input_bytes = user_input.len(),
                "game_info classify returned None"
            );
            return Ok(serde_json::json!({
                "found": false,
                "data": "未找到相关游戏"
            }));
        }
    };

    // 查找分类名称（用于展示和 mock 匹配）
    let category_name = categories
        .iter()
        .find(|(id, _)| *id == category_id)
        .map(|(_, name)| name.clone())
        .unwrap_or_default();

    tracing::info!(
        user_input_bytes = user_input.len(),
        category_id = %category_id,
        "game_info: LLM classified user input"
    );

    // 3. 根据分类查询 logic_game_id（从 cc_logic_game_display 按 tag 过滤，查10取3）
    let game_ids = match crate::services::game_service::fetch_logic_game_ids_by_tag(
        ext_pool,
        &category_name,
        target_client_type.unwrap_or(client_type),
        channel,
        10,
    )
    .await
    {
        Ok(ids) => {
            tracing::debug!(count = ids.len(), "game_info: fetched logic_game_ids");
            if ids.is_empty() {
                tracing::warn!(category_id = %category_id, "no logic_game_id found for tag");
                return Ok(serde_json::json!({
                    "found": false,
                    "data": "未找到相关游戏"
                }));
            }
            ids
        }
        Err(_) => {
            tracing::error!(
                category_id = %category_id,
                error_kind = "game_id_query_failed",
                "fetch logic_game_ids failed"
            );
            return Ok(serde_json::json!({
                "found": false,
                "data": "未找到相关游戏"
            }));
        }
    };

    // 4. 查询每个游戏的详细信息（查 10 个，取前 3 个有效）
    let mut games: Vec<Value> = Vec::new();
    for &gid in &game_ids {
        let info = crate::services::game_service::get_single_external_game_info(
            ext_pool,
            gid,
            client_type,
            channel,
        )
        .await;
        match info {
            Ok(Some(info)) if info.logic_game_id != 0 => {
                games.push(serde_json::json!({
                    "id": info.logic_game_id,
                    "name": info.name,
                    "channel": info.channel,
                    "client_type": info.client_type,
                    "reason": info.description,
                    "game_tags": info.game_tags,
                    "description": info.description,
                    "cover_image": info.cover_image,
                    "computer_id": info.computer_id,
                    "platform_name": info.platform_name,
                    "game_icon": info.game_icon,
                    "raw_description": info.raw_description,
                }));
                if games.len() >= 3 {
                    break;
                }
            }
            _ => {
                tracing::warn!(logic_game_id = %gid, "game info not found, skipping");
            }
        }
    }

    if games.is_empty() {
        tracing::warn!(
            category_id = %category_id,
            game_id_count = game_ids.len(),
            "game_info: all game lookups failed, no games to return"
        );
        return Ok(serde_json::json!({
            "found": false,
            "data": "未找到相关游戏"
        }));
    }

    tracing::debug!(
        game_count = games.len(),
        "game_info: returning game list extension"
    );

    // 跨平台时移除 computer_id，添加提醒
    let cross_platform = target_client_type
        .filter(|t| !t.eq_ignore_ascii_case(client_type))
        .is_some();
    if cross_platform {
        for g in &mut games {
            if let Some(obj) = g.as_object_mut() {
                obj.remove("computer_id");
            }
        }
    }

    // 5. 构造返回结果
    let mut output = serde_json::json!({
        "found": false,
        "data": "未找到相关游戏"
    });

    // 6. 写入 AgentContext extensions — 每个游戏一个独立 card
    let extensions: Vec<Value> = games
        .iter()
        .map(|g| {
            let mut info = g.clone();
            if let Some(obj) = info.as_object_mut() {
                obj.remove("raw_description");
            }
            serde_json::json!({
                "id": format!("game-{}", g.get("id").and_then(|v| v.as_i64()).map(|id| id.to_string()).unwrap_or_else(|| Uuid::new_v4().to_string())),
                "content_type": "card",
                "payload": {
                    "type": "game",
                    "info": info,
                },
            })
        })
        .collect();

    let mut metadata = serde_json::json!({
        "agent_loop_break": "true"
    });
    // 推荐游戏时，重置 content 为推荐文案

    let mut parts: Vec<String> = Vec::new();
    parts.push("很遗憾，您查询的这款游戏暂未在平台上架。为您推荐相似游戏，这些游戏支持云端畅玩，您可以点击下方游戏卡片查看详情。".into());
    parts.push(String::new());
    for g in &games {
        let name = g.get("name").and_then(|v| v.as_str()).unwrap_or("");
        let desc = g
            .get("raw_description")
            .and_then(|v| v.as_str())
            .filter(|s| !s.is_empty());
        if !name.is_empty() {
            if let Some(d) = desc {
                parts.push(format!("「{name}」：{d}；"));
            } else {
                parts.push(format!("「{name}」；"));
            }
        }
    }
    parts.push(String::new());
    parts.push("这些游戏在玩法、题材或体验上与您查询的游戏较为接近，请尽情体验。".into());

    if cross_platform {
        parts.push(format!(
            "注意：以下游戏信息为 {} 平台数据，与您当前使用的 {} 平台不同，需要到相应客户端才能玩。",
            target_client_type.unwrap_or(""),
            client_type
        ));
    }
    let content = parts.join("\n");
    metadata["response_content"] = serde_json::Value::String(content);

    output["_agent_context_updates"] = serde_json::json!({
        "extensions": extensions,
        "metadata": metadata,
    });
    Ok(output)
}

/// Call LLM to classify user input into one of the given categories.
/// Returns the category ID on success, or `None` if classification failed.
fn audit_local_game_fallback(
    execution_context: Option<&RuntimeExecutionContext>,
    agent_id: Option<i64>,
    model_preset: Option<&str>,
    reason: LlmLocalFallbackReason,
) {
    if let Some(execution_context) = execution_context {
        record_local_fallback(
            execution_context,
            agent_id,
            model_preset,
            LlmAuditSource::BuiltinGameInfo,
            reason,
        );
    }
}

trait GameInfoLlmRegistry {
    fn resolve_preset_name(&self, preset_name: Option<&str>) -> Result<String, LlmAdapterError>;

    fn build_chain(
        &self,
        preset_name: Option<&str>,
    ) -> Result<(Arc<dyn LLMProvider>, String), LlmAdapterError>;
}

impl GameInfoLlmRegistry for LlmRegistry {
    fn resolve_preset_name(&self, preset_name: Option<&str>) -> Result<String, LlmAdapterError> {
        self.resolve(preset_name).map(|entry| entry.name.clone())
    }

    fn build_chain(
        &self,
        preset_name: Option<&str>,
    ) -> Result<(Arc<dyn LLMProvider>, String), LlmAdapterError> {
        LlmRegistry::build_chain(self, preset_name)
    }
}

fn agent_model_preset_from_row(
    row: Option<(Option<String>,)>,
) -> Result<Option<String>, BuiltinError> {
    row.map(|(preset,)| preset)
        .ok_or_else(|| BuiltinError::Exec("当前 Agent 模型配置不可用".to_string()))
}

async fn load_agent_model_preset(
    pool: &sqlx::MySqlPool,
    agent_id: i64,
) -> Result<Option<String>, BuiltinError> {
    let row: Option<(Option<String>,)> =
        sqlx::query_as("SELECT model_preset FROM agents WHERE id = ?")
            .bind(agent_id)
            .fetch_optional(pool)
            .await
            .map_err(|_| {
                tracing::error!(
                    error_kind = "agent_model_preset_lookup_failed",
                    "game_info could not resolve the current Agent model preset"
                );
                BuiltinError::Exec("当前 Agent 模型配置不可用".to_string())
            })?;
    agent_model_preset_from_row(row)
}

fn finish_registry_failure(
    execution_context: Option<&RuntimeExecutionContext>,
    agent_id: Option<i64>,
    requested_preset: Option<&str>,
    error: LlmAdapterError,
) -> BuiltinError {
    let audit_preset = match &error {
        LlmAdapterError::Unknown(name) => Some(name.as_str()),
        _ => requested_preset,
    };
    if let Some(execution_context) = execution_context {
        let mut audit = LlmAuditGuard::new(
            execution_context.clone(),
            agent_id,
            audit_preset,
            LlmAuditSource::BuiltinGameInfo,
        );
        if matches!(error, LlmAdapterError::Unknown(_)) {
            audit.finish_model_preset_unknown();
        } else {
            audit.finish_response(&LLMResponse {
                finish_reason: "error".to_string(),
                error_kind: Some("registry_error".to_string()),
                error_should_retry: Some(false),
                ..Default::default()
            });
        }
    }

    match error {
        LlmAdapterError::Unknown(name) => BuiltinError::ModelPresetUnknown(name),
        _ => BuiltinError::Exec("LLM 模型配置不可用".to_string()),
    }
}

async fn classify_game_category(
    user_input: &str,
    categories: &[(i64, String)],
    pool: &sqlx::MySqlPool,
    llm: Option<&Arc<LlmRegistry>>,
    agent_id: Option<i64>,
    execution_context: Option<&RuntimeExecutionContext>,
) -> Result<Option<i64>, BuiltinError> {
    tracing::debug!(
        user_input_bytes = user_input.len(),
        category_count = categories.len(),
        has_llm = llm.is_some(),
        "classify_game_category: start"
    );

    let Some(llm) = llm else {
        tracing::debug!("classify_game_category: skipping LLM — llm registry not available");
        audit_local_game_fallback(
            execution_context,
            agent_id,
            None,
            LlmLocalFallbackReason::ProviderUnavailable,
        );
        return Ok(None);
    };

    let agent_id =
        agent_id.ok_or_else(|| BuiltinError::Exec("当前 Agent 模型配置不可用".to_string()))?;
    let model_preset = load_agent_model_preset(pool, agent_id).await?;
    classify_game_category_for_preset(
        user_input,
        categories,
        llm.as_ref(),
        model_preset.as_deref(),
        Some(agent_id),
        execution_context,
    )
    .await
}

async fn classify_game_category_for_preset<R: GameInfoLlmRegistry + ?Sized>(
    user_input: &str,
    categories: &[(i64, String)],
    llm: &R,
    model_preset: Option<&str>,
    agent_id: Option<i64>,
    execution_context: Option<&RuntimeExecutionContext>,
) -> Result<Option<i64>, BuiltinError> {
    let resolved_preset = llm.resolve_preset_name(model_preset).map_err(|error| {
        finish_registry_failure(execution_context, agent_id, model_preset, error)
    })?;
    let prompt = build_classification_prompt(user_input, categories);
    tracing::debug!(
        prompt_len = prompt.len(),
        "classify_game_category: built prompt"
    );

    let messages = vec![serde_json::json!({
        "role": "user",
        "content": prompt,
    })];
    let (provider, model) = llm.build_chain(Some(&resolved_preset)).map_err(|error| {
        finish_registry_failure(execution_context, agent_id, Some(&resolved_preset), error)
    })?;
    let req = ChatRequest {
        model: Some(model),
        messages,
        max_tokens: 1024,
        temperature: 0.1,
        tools: None,
        tool_choice: None,
        reasoning_effort: None,
    };
    tracing::debug!("classify_game_category: calling LLM");
    let mut audit = execution_context.map(|execution_context| {
        LlmAuditGuard::new(
            execution_context.clone(),
            agent_id,
            Some(&resolved_preset),
            LlmAuditSource::BuiltinGameInfo,
        )
    });
    let mut options =
        LlmCallOptions::with_timeouts(Duration::from_secs(25), Duration::from_secs(45));
    if let Some(audit) = audit.as_ref() {
        options = options.with_fallback_callback(audit.on_fallback());
    }
    let resp = provider.chat_with_options(req, options).await;
    if let Some(audit) = audit.as_mut() {
        audit.finish_response(&resp);
    }
    let raw_content = resp.content.clone();
    tracing::debug!(
        llm_response_bytes = raw_content.as_deref().map_or(0, str::len),
        finish_reason = %resp.finish_reason,
        "classify_game_category: LLM response"
    );
    if resp.is_error() {
        audit_local_game_fallback(
            execution_context,
            agent_id,
            Some(&resolved_preset),
            LlmLocalFallbackReason::InvocationFailed,
        );
        return Ok(None);
    }
    if let Some(content) = raw_content {
        let trimmed = content.trim().to_string();
        if !trimmed.is_empty() {
            if let Ok(id) = trimmed.parse::<i64>() {
                if categories.iter().any(|(cid, _)| *cid == id) {
                    tracing::debug!(category_id = %id, "classify_game_category: matched by ID");
                    return Ok(Some(id));
                }
                tracing::debug!(category_id = %id, "classify_game_category: parsed ID not in category list");
            }
            let matched = categories
                .iter()
                .find(|(_, name)| trimmed.contains(name.as_str()) || name.contains(&trimmed));
            if let Some((id, _name)) = matched {
                tracing::debug!(
                    category_id = %id,
                    "classify_game_category: matched by name"
                );
                return Ok(Some(*id));
            }
            tracing::debug!(
                llm_output_bytes = trimmed.len(),
                "classify_game_category: could not match LLM output to any category"
            );
        } else {
            tracing::debug!("classify_game_category: LLM returned empty string");
        }
    }
    tracing::warn!(
        user_input_bytes = user_input.len(),
        llm_response_bytes = resp.content.as_deref().map_or(0, str::len),
        "LLM 分类失败或返回无效 ID"
    );
    audit_local_game_fallback(
        execution_context,
        agent_id,
        Some(&resolved_preset),
        LlmLocalFallbackReason::EmptyResponse,
    );

    tracing::debug!(
        "classify_game_category: no LLM available or classification failed, returning None"
    );
    Ok(None)
}

pub const GAME_INFO_INPUT_SCHEMA: &str = r#"{
  "type": "object",
  "properties": {
    "game_id": {
      "type": "string",
      "description": "游戏 ID（文本形式，内部转为数字）。id <= 0 或转换失败时根据用户输入自动分类并推荐游戏。"
    }
  },
  "required": ["game_id"]
}"#;

pub const GAME_INFO_OUTPUT_SCHEMA: &str = r#"{
  "type": "object",
  "properties": {
    "found": {
      "type": "boolean",
      "description": "是否找到游戏"
    },
    "id": {
      "type": "integer",
      "description": "游戏 ID（logic_game_id）"
    },
    "name": {
      "type": "string",
      "description": "游戏名称"
    },
    "data": {
      "type": "string",
      "description": "格式化文本"
    },
    "description": {
      "type": "string",
      "description": "游戏描述"
    },
    "cover_image": {
      "type": "string",
      "description": "封面图 URL"
    },
    "game_tags": {
      "type": "object",
      "description": "游戏标签（JSON）"
    },
    "computer_id": {
      "type": "integer",
      "description": "计算机 ID"
    },
    "platform_name": {
      "type": "string",
      "description": "平台名称"
    },
    "client_type": {
      "type": "string",
      "description": "客户端类型"
    },
    "channel": {
      "type": "string",
      "description": "渠道（从推广渠道映射）"
    },
    "classified": {
      "type": "boolean",
      "description": "是否通过 LLM 自动分类（game_id=0 时为 true）"
    },
    "category": {
      "type": "string",
      "description": "LLM 识别的游戏分类（game_id=0 时返回）"
    },
    "games": {
      "type": "array",
      "items": {
        "type": "object",
        "properties": {
          "id": { "type": "integer" },
          "name": { "type": "string" },
          "description": { "type": "string" },
          "cover_image": { "type": "string" },
          "platform_name": { "type": "string" },
          "game_icon": { "type": "string" }
        }
      },
      "description": "推荐的游戏列表（game_id=0 时返回，最多 3 个）"
    }
  }
}"#;

#[cfg(test)]
mod tests {
    use super::{
        GameInfoLlmRegistry, agent_model_preset_from_row, classify_game_category_for_preset,
    };
    use crate::runtime::builtins::BuiltinError;
    use crate::runtime::execution_context::RuntimeExecutionContext;
    use crate::runtime::llm::LlmAdapterError;
    use async_trait::async_trait;
    use providers::{ChatRequest, LLMProvider, LLMResponse, LlmCallOptions};
    use std::collections::HashSet;
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::sync::{Arc, Mutex};
    use std::time::Duration;

    struct CapturingProvider {
        calls: AtomicUsize,
        requests: Mutex<Vec<ChatRequest>>,
        options: Mutex<Vec<(Duration, Duration, bool)>>,
    }

    impl CapturingProvider {
        fn new() -> Arc<Self> {
            Arc::new(Self {
                calls: AtomicUsize::new(0),
                requests: Mutex::new(Vec::new()),
                options: Mutex::new(Vec::new()),
            })
        }
    }

    #[async_trait]
    impl LLMProvider for CapturingProvider {
        fn default_model(&self) -> String {
            "default-must-not-be-requested".to_string()
        }

        async fn chat(&self, _request: ChatRequest) -> LLMResponse {
            panic!("game_info must use the bounded chat_with_options API")
        }

        async fn chat_with_options(
            &self,
            request: ChatRequest,
            options: LlmCallOptions,
        ) -> LLMResponse {
            self.calls.fetch_add(1, Ordering::SeqCst);
            self.requests.lock().expect("request lock").push(request);
            self.options.lock().expect("options lock").push((
                options.node_timeout,
                options.chain_timeout,
                options.on_fallback.is_some(),
            ));
            LLMResponse {
                content: Some("2".to_string()),
                finish_reason: "stop".to_string(),
                actual_model: Some("selected-model".to_string()),
                ..Default::default()
            }
        }
    }

    struct FakeRegistry {
        provider: Arc<dyn LLMProvider>,
        available: HashSet<String>,
        default_name: String,
        resolve_calls: Mutex<Vec<Option<String>>>,
        build_calls: Mutex<Vec<Option<String>>>,
    }

    impl FakeRegistry {
        fn new(provider: Arc<dyn LLMProvider>) -> Self {
            Self {
                provider,
                available: ["global-default", "agent-premium"]
                    .into_iter()
                    .map(str::to_string)
                    .collect(),
                default_name: "global-default".to_string(),
                resolve_calls: Mutex::new(Vec::new()),
                build_calls: Mutex::new(Vec::new()),
            }
        }
    }

    impl GameInfoLlmRegistry for FakeRegistry {
        fn resolve_preset_name(
            &self,
            preset_name: Option<&str>,
        ) -> Result<String, LlmAdapterError> {
            self.resolve_calls
                .lock()
                .expect("resolve lock")
                .push(preset_name.map(str::to_string));
            let name = preset_name.unwrap_or(&self.default_name);
            if self.available.contains(name) {
                Ok(name.to_string())
            } else {
                Err(LlmAdapterError::Unknown(name.to_string()))
            }
        }

        fn build_chain(
            &self,
            preset_name: Option<&str>,
        ) -> Result<(Arc<dyn LLMProvider>, String), LlmAdapterError> {
            self.build_calls
                .lock()
                .expect("build lock")
                .push(preset_name.map(str::to_string));
            let name = preset_name.ok_or(LlmAdapterError::NoDefault)?;
            if !self.available.contains(name) {
                return Err(LlmAdapterError::Unknown(name.to_string()));
            }
            Ok((Arc::clone(&self.provider), format!("{name}-primary-model")))
        }
    }

    fn tracing_only_context(request_id: &str) -> RuntimeExecutionContext {
        RuntimeExecutionContext::best_effort(Some(request_id.to_string()), Some(17)).for_hook()
    }

    fn categories() -> Vec<(i64, String)> {
        vec![(1, "动作".to_string()), (2, "策略".to_string())]
    }

    #[tokio::test]
    async fn explicit_agent_preset_selects_that_chain_and_never_the_global_default() {
        let provider = CapturingProvider::new();
        let registry = FakeRegistry::new(Arc::clone(&provider) as Arc<dyn LLMProvider>);
        let selected = classify_game_category_for_preset(
            "想玩策略游戏",
            &categories(),
            &registry,
            Some("agent-premium"),
            Some(9),
            Some(&tracing_only_context("game-info-agent-preset")),
        )
        .await
        .expect("classification");

        assert_eq!(selected, Some(2));
        assert_eq!(
            registry
                .resolve_calls
                .lock()
                .expect("resolve lock")
                .as_slice(),
            [Some("agent-premium".to_string())]
        );
        assert_eq!(
            registry.build_calls.lock().expect("build lock").as_slice(),
            [Some("agent-premium".to_string())]
        );
        let requests = provider.requests.lock().expect("request lock");
        assert_eq!(
            requests[0].model.as_deref(),
            Some("agent-premium-primary-model")
        );
        drop(requests);
        assert_eq!(
            provider.options.lock().expect("options lock").as_slice(),
            [(Duration::from_secs(25), Duration::from_secs(45), true)]
        );
    }

    #[tokio::test]
    async fn database_null_is_the_only_value_that_selects_the_global_default() {
        let provider = CapturingProvider::new();
        let registry = FakeRegistry::new(Arc::clone(&provider) as Arc<dyn LLMProvider>);
        let selected = classify_game_category_for_preset(
            "想玩策略游戏",
            &categories(),
            &registry,
            None,
            Some(9),
            Some(&tracing_only_context("game-info-default-preset")),
        )
        .await
        .expect("classification");

        assert_eq!(selected, Some(2));
        assert_eq!(
            registry
                .resolve_calls
                .lock()
                .expect("resolve lock")
                .as_slice(),
            [None]
        );
        assert_eq!(
            registry.build_calls.lock().expect("build lock").as_slice(),
            [Some("global-default".to_string())]
        );
        assert_eq!(
            provider.requests.lock().expect("request lock")[0]
                .model
                .as_deref(),
            Some("global-default-primary-model")
        );
    }

    #[test]
    fn agent_row_preserves_null_and_explicit_values_but_missing_agent_cannot_default() {
        assert_eq!(
            agent_model_preset_from_row(Some((None,))).expect("NULL preset"),
            None
        );
        assert_eq!(
            agent_model_preset_from_row(Some((Some("agent-premium".to_string()),)))
                .expect("explicit preset")
                .as_deref(),
            Some("agent-premium")
        );
        assert_eq!(
            agent_model_preset_from_row(Some((Some(String::new()),)))
                .expect("explicit empty remains explicit")
                .as_deref(),
            Some("")
        );
        assert!(matches!(
            agent_model_preset_from_row(None),
            Err(BuiltinError::Exec(message)) if message == "当前 Agent 模型配置不可用"
        ));
    }

    #[tokio::test]
    async fn explicit_empty_or_unknown_preset_is_typed_and_never_builds_or_locally_falls_back() {
        for requested in ["", "removed-preset"] {
            let provider = CapturingProvider::new();
            let registry = FakeRegistry::new(Arc::clone(&provider) as Arc<dyn LLMProvider>);

            let error = classify_game_category_for_preset(
                "must not run",
                &categories(),
                &registry,
                Some(requested),
                Some(9),
                Some(&tracing_only_context("game-info-unknown-preset")),
            )
            .await
            .expect_err("unknown preset must fail closed instead of locally falling back");
            assert!(matches!(
                error,
                BuiltinError::ModelPresetUnknown(name) if name == requested
            ));

            assert!(
                registry.build_calls.lock().expect("build lock").is_empty(),
                "unknown preset must fail before provider construction"
            );
            assert_eq!(provider.calls.load(Ordering::SeqCst), 0);
        }
    }
}
