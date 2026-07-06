use serde_json::Value;
use std::sync::Arc;
use uuid::Uuid;

use crate::runtime::llm::LlmRegistry;
use providers::{ChatRequest, RetryMode};

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

    let pool = ctx.pool.clone();
    let ext_pool = ctx.ext_pool.cloned();
    let redis = ctx.redis.cloned();
    let llm = ctx.llm.cloned();
    let agent_id = ctx.agent_id;
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
            )
            .await
        })
    })
}

/// Query a single game's details by game_id (text input, parsed to integer).
/// Input: { "game_id": "<text>" }
/// - game_id: 文本形式的游戏 ID，内部转为数字。id <= 0 或转换失败 → LLM 分类推荐。
/// - 游戏信息始终写入 AgentContext extensions (card/game)。
async fn game_info_async_impl(
    args: Value,
    pool: &sqlx::MySqlPool,
    ext_pool: Option<&sqlx::MySqlPool>,
    redis: Option<&redis::Client>,
    channel: &str,
    client_type: &str,
    llm: Option<&Arc<LlmRegistry>>,
    agent_id: Option<i64>,
) -> BuiltinResult {
    // 1. Parse game_id from input — text → integer
    let game_id_str = args.get("game_id").and_then(|v| v.as_str()).unwrap_or("");

    tracing::debug!(
        game_id_str = %game_id_str,
        channel = %channel,
        client_type = %client_type,
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
            )
            .await;
        }
    };

    let ext_pool = ext_pool.ok_or_else(|| BuiltinError::Exec("外部数据库未配置".into()))?;

    // 2. Query external DB with caching — single top-priority game info
    let game_info = if let Some(r) = redis {
        crate::services::game_service::get_single_external_game_info_cached(
            r,
            ext_pool,
            game_id,
            client_type,
            channel,
        )
        .await
        .map_err(|e| BuiltinError::Exec(format!("{e}")))?
    } else {
        let mut games = crate::services::game_service::get_external_game_by_id(
            ext_pool,
            game_id,
            client_type,
            channel,
        )
        .await
        .map_err(|e| BuiltinError::Exec(format!("{e}")))?;
        crate::services::game_service::sort_external_games_by_priority(ext_pool, &mut games).await;
        games.into_iter().next()
    };

    let game_info = match game_info {
        Some(info) if info.logic_game_id != 0 => {
            tracing::debug!(
                logic_game_id = %info.logic_game_id,
                name = %info.name,
                "game_info: found by direct lookup"
            );
            info
        }
        _ => {
            tracing::warn!(game_id = %game_id, "game_info: direct lookup returned empty");
            return Ok(serde_json::json!({
                "found": false,
                "name": "未找到相关游戏"
            }));
        }
    };

    let id = game_info.logic_game_id;
    let name = &game_info.name;

    let game_payload = serde_json::json!({
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

    let mut output = serde_json::json!({
        "found": true,
        "id": id,
        "name": name,
        "description": game_info.description,
        "cover_image": game_info.cover_image,
        "game_tags": game_info.game_tags,
        "computer_id": game_info.computer_id,
        "platform_name": game_info.platform_name,
        "client_type": game_info.client_type,
        "channel": game_info.channel,
        "game_icon": game_info.game_icon,
    });

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
    redis: Option<&redis::Client>,
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
        let rows: Vec<(i64, String)> = sqlx::query_as("SELECT id, name FROM cc_game_tag WHERE `type` = 1")
            .fetch_all(ext_pool)
            .await
            .map_err(|e| format!("cc_game_tag query: {e}"))?;
        Ok(rows)
    }
}


/// Build conversation context from _agent_context messages for LLM classification.
/// Takes the last 3 messages, formats as "role: content" pairs.
fn build_classify_context(args: &Value, user_input: &str) -> String {
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
    // 追加当前用户输入
    lines.push(format!("用户: {user_input}"));

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
async fn handle_classify_and_list(
    args: Value,
    _pool: &sqlx::MySqlPool,
    ext_pool: Option<&sqlx::MySqlPool>,
    redis: Option<&redis::Client>,
    channel: &str,
    client_type: &str,
    llm: Option<&Arc<LlmRegistry>>,
    _agent_id: Option<i64>,
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
            "name": "未找到相关游戏"
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
                "name": "未找到相关游戏"
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
                "name": "未找到相关游戏"
            }));
        }
        Err(e) => {
            tracing::warn!(error = %e, "fetch_categories failed, game_info classify aborted");
            return Ok(serde_json::json!({
                "found": false,
                "name": "未找到相关游戏"
            }));
        }
    };
    let category_id = match classify_game_category(&context, &categories, llm).await {
        Some(id) => id,
        None => {
            tracing::warn!(user_input = %user_input, "game_info classify returned None");
            return Ok(serde_json::json!({
                "found": false,
                "name": "未找到相关游戏"
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
        user_input = %user_input,
        category_id = %category_id,
        category_name = %category_name,
        "game_info: LLM classified user input"
    );

    // 3. 根据分类查询 logic_game_id（从 cc_logic_game_display 按 tag 过滤，查10取3）
    let game_ids = match crate::services::game_service::fetch_logic_game_ids_by_tag(ext_pool, category_id, 10).await {
        Ok(ids) => {
            tracing::debug!(
                category_id = %category_id,
                count = ids.len(),
                game_ids = ?ids,
                "game_info: fetched logic_game_ids"
            );
            if ids.is_empty() {
                tracing::warn!(category_id = %category_id, "no logic_game_id found for tag");
                return Ok(serde_json::json!({
                    "found": false,
                    "name": "未找到相关游戏"
                }));
            }
            ids
        }
        Err(e) => {
            tracing::error!(category_id = %category_id, error = %e, "fetch logic_game_ids failed");
            return Ok(serde_json::json!({
                "found": false,
                "name": "未找到相关游戏"
            }));
        }
    };

    // 4. 查询每个游戏的详细信息（查 10 个，取前 3 个有效）
    let redis_ref = redis;
    let mut games: Vec<Value> = Vec::new();
    for &gid in &game_ids {
        let info = if let Some(r) = redis_ref {
            crate::services::game_service::get_single_external_game_info_cached(
                r,
                ext_pool,
                gid,
                client_type,
                channel,
            )
            .await
        } else {
            let mut results = crate::services::game_service::get_external_game_by_id(
                ext_pool,
                gid,
                client_type,
                channel,
            )
            .await
            .map_err(|e| BuiltinError::Exec(format!("{e}")))?;
            crate::services::game_service::sort_external_games_by_priority(ext_pool, &mut results).await;
            Ok(results.into_iter().next())
        };
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
        tracing::warn!(category_id = %category_id, game_ids = ?game_ids, "game_info: all game lookups failed, no games to return");
        return Ok(serde_json::json!({
            "found": false,
            "name": "未找到相关游戏"
        }));
    }

    tracing::debug!(game_count = games.len(), "game_info: returning game list extension");

    // 5. 构造返回结果
    let mut output = serde_json::json!({
        "found": false,
        "name": "未找到相关游戏"
    });

    // 6. 写入 AgentContext extensions — 每个游戏一个独立 card
    let extensions: Vec<Value> = games
        .iter()
        .map(|g| serde_json::json!({
            "id": format!("game-{}", g.get("id").and_then(|v| v.as_i64()).map(|id| id.to_string()).unwrap_or_else(|| Uuid::new_v4().to_string())),
            "content_type": "card",
            "payload": {
                "type": "game",
                "info": g,
            },
        }))
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
        let desc = g.get("description").and_then(|v| v.as_str()).filter(|s| !s.is_empty());
        if !name.is_empty() {
            if let Some(d) = desc {
                parts.push(format!("「{name}」是一款{d}；"));
            } else {
                parts.push(format!("「{name}」；"));
            }
        }
    }
    parts.push(String::new());
    parts.push("这些游戏在玩法、题材或体验上与您查询的游戏较为接近，请尽情体验。".into());
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
async fn classify_game_category(
    user_input: &str,
    categories: &[(i64, String)],
    llm: Option<&Arc<LlmRegistry>>,
) -> Option<i64> {
    tracing::debug!(
        user_input = %user_input,
        category_count = categories.len(),
        has_llm = llm.is_some(),
        "classify_game_category: start"
    );

    // 尝试 LLM 分类
    if llm.is_none() {
        tracing::debug!("classify_game_category: skipping LLM — llm registry not available");
    }
    if let Some(llm) = llm {
        let prompt = build_classification_prompt(user_input, categories);
        tracing::debug!(prompt_len = prompt.len(), "classify_game_category: built prompt");

        let messages = vec![serde_json::json!({
            "role": "user",
            "content": prompt,
        })];

        match llm.build_primary(None) {
            Ok((provider, model)) => {
                let req = ChatRequest {
                    model: Some(model.clone()),
                    messages,
                    max_tokens: 16,
                    temperature: 0.1,
                    tools: None,
                    tool_choice: None,
                    reasoning_effort: None,
                };
                tracing::debug!(model = %model, "classify_game_category: calling LLM");
                let resp = provider
                    .chat_with_retry(req, RetryMode::Standard, None)
                    .await;
                let raw_content = resp.content.clone();
                tracing::debug!(
                    llm_response = ?raw_content,
                    finish_reason = %resp.finish_reason,
                    "classify_game_category: LLM response"
                );
                if let Some(content) = raw_content {
                    let trimmed = content.trim().to_string();
                    // 尝试解析为数字 ID
                    if let Ok(id) = trimmed.parse::<i64>() {
                        // 验证 ID 是否在可选分类中
                        if categories.iter().any(|(cid, _)| *cid == id) {
                            tracing::debug!(category_id = %id, "classify_game_category: matched by ID");
                            return Some(id);
                        }
                        tracing::debug!(category_id = %id, "classify_game_category: parsed ID not in category list");
                    }
                    // 尝试按名称匹配（LLM 可能返回名称而非 ID）
                    let matched = categories
                        .iter()
                        .find(|(_, name)| trimmed.contains(name.as_str()) || name.contains(&trimmed));
                    if let Some((id, name)) = matched {
                        tracing::debug!(category_id = %id, category_name = %name, "classify_game_category: matched by name");
                        return Some(*id);
                    }
                    tracing::debug!(llm_output = %trimmed, "classify_game_category: could not match LLM output to any category");
                }
                tracing::warn!(
                    user_input = %user_input,
                    llm_response = ?resp.content,
                    "LLM 分类失败或返回无效 ID"
                );
            }
            Err(e) => {
                tracing::warn!(
                    error = %e,
                    "build_primary 失败"
                );
            }
        }
    }

    tracing::debug!("classify_game_category: no LLM available or classification failed, returning None");
    None
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
