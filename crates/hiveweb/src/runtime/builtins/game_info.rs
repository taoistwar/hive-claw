use serde_json::Value;
use std::sync::Arc;

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
        tokio::runtime::Handle::current()
            .block_on(async move {
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
/// Input: { "game_id": "<text>", "put_to_ac": <bool> }
/// - game_id: 文本形式的游戏 ID，内部转为数字。转换失败返回空字符串。
/// - game_id == 0: 使用 LLM 识别用户输入的游戏分类，返回该分类下的前 3 个游戏。
/// - put_to_ac: true → 游戏信息写入 AgentContext extensions (card/game)；false → 作为输出变量返回。
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
    let put_to_ac = args
        .get("put_to_ac")
        .and_then(|v| v.as_bool())
        .unwrap_or(false);

    let game_id: i64 = match game_id_str.trim().parse() {
        Ok(id) => id,
        Err(_) => {
            // 转换失败，输出空字符串
            return Ok(serde_json::json!({
                "found": false,
                "data": "",
                "id": null,
                "name": null
            }));
        }
    };

    // ★ game_id == 0: LLM 分类 + 模拟按分类返回游戏列表
    if game_id == 0 {
        return handle_classify_and_list(args, pool, ext_pool, redis, channel, client_type, llm, agent_id).await;
    }

    let ext_pool = ext_pool.ok_or_else(|| BuiltinError::Exec("外部数据库未配置".into()))?;

    // 2. Query external DB with caching — single top-priority game info
    let game_info = if let Some(r) = redis {
        crate::services::game_service::get_single_external_game_info_cached(
            r, ext_pool, game_id, client_type, channel,
        )
        .await
        .map_err(|e| BuiltinError::Exec(format!("{e}")))?
    } else {
        let mut games =
            crate::services::game_service::get_external_game_by_id(ext_pool, game_id, client_type, channel)
                .await
                .map_err(|e| BuiltinError::Exec(format!("{e}")))?;
        crate::services::game_service::sort_external_games_by_priority(ext_pool, &mut games).await;
        games.into_iter().next()
    };

    let game_info = match game_info {
        Some(info) if info.logic_game_id != 0 => info,
        _ => {
            return Ok(serde_json::json!({
                "found": false,
                "data": format!("未找到游戏 ID {} 的信息", game_id),
                "id": game_id,
                "name": null
            }));
        }
    };

    let id = game_info.logic_game_id;
    let name = &game_info.name;

    let game_payload = serde_json::json!({
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

    // 6. put_to_ac: true → 写入 AgentContext extensions；false → 纯输出
    if put_to_ac {
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
    }

    Ok(output)
}

// ─── game_id == 0: LLM 分类 + 模拟数据 ──────────────────────────────────────

/// Mock game categories and their associated game lists.
///
/// TODO: 替换为外部数据库查询（cc_logic_game 按 category 过滤）。
fn mock_games_by_category(category: &str) -> Vec<MockGameItem> {
    let all = mock_game_data();
    let category_lower = category.to_lowercase().trim().to_string();

    // 按分类匹配（模糊匹配：分类名包含在 category 中或 vice versa）
    let matched: Vec<&MockGameItem> = all
        .iter()
        .filter(|g| {
            let cats: Vec<String> = g
                .categories
                .iter()
                .map(|c| c.to_lowercase())
                .collect();
            cats.iter().any(|c| c.contains(&category_lower) || category_lower.contains(c.as_str()))
        })
        .collect();

    if matched.is_empty() {
        // 无匹配时返回前 3 个作为兜底
        all.iter().take(3).cloned().collect()
    } else {
        matched.into_iter().take(3).cloned().collect()
    }
}

/// Fetch game categories (tags) from the external cc_game_tag table.
///
/// Queries `SELECT name FROM cc_game_tag WHERE type = 1`.
/// Results are cached in Redis for 10 minutes.
/// Returns `Ok(Vec::new())` if no rows are returned.
async fn fetch_categories(
    ext_pool: &sqlx::MySqlPool,
    redis: Option<&redis::Client>,
) -> Result<Vec<String>, String> {
    if let Some(r) = redis {
        let cache_key = "game_tags:cc_game_tag_type1";
        crate::services::cache_helper::cached_or_fetch(
            r,
            cache_key,
            600, // 10 min TTL
            || async {
                let rows: Vec<(String,)> =
                    sqlx::query_as("SELECT name FROM cc_game_tag WHERE `type` = 1")
                        .fetch_all(ext_pool)
                        .await
                        .map_err(|e| format!("cc_game_tag query: {e}"))?;
                Ok::<Vec<String>, String>(rows.into_iter().map(|(n,)| n).collect())
            },
        )
        .await
    } else {
        let rows: Vec<(String,)> =
            sqlx::query_as("SELECT name FROM cc_game_tag WHERE `type` = 1")
                .fetch_all(ext_pool)
                .await
                .map_err(|e| format!("cc_game_tag query: {e}"))?;
        Ok(rows.into_iter().map(|(n,)| n).collect())
    }
}

/// Mock game data entry.
#[derive(Debug, Clone)]
struct MockGameItem {
    id: i64,
    name: String,
    description: String,
    cover_image: String,
    categories: Vec<String>,
    platform_name: String,
    game_icon: String,
}

/// Mock game data.
///
/// TODO: 替换为外部数据库查询（cc_logic_game 表）。
fn mock_game_data() -> Vec<MockGameItem> {
    vec![
        MockGameItem {
            id: 1001,
            name: "幻境奇谭".into(),
            description: "一款开放世界角色扮演游戏，探索神秘的幻境大陆。".into(),
            cover_image: "https://cdn.example.com/covers/huanjing.png".into(),
            categories: vec!["角色扮演".into(), "开放世界".into()],
            platform_name: "CloudGame".into(),
            game_icon: "https://cdn.example.com/icons/huanjing.png".into(),
        },
        MockGameItem {
            id: 1002,
            name: "暗影之刃".into(),
            description: "快节奏动作冒险游戏，扮演暗影刺客执行秘密任务。".into(),
            cover_image: "https://cdn.example.com/covers/anying.png".into(),
            categories: vec!["动作冒险".into(), "潜入".into()],
            platform_name: "CloudGame".into(),
            game_icon: "https://cdn.example.com/icons/anying.png".into(),
        },
        MockGameItem {
            id: 1003,
            name: "星际战线".into(),
            description: "科幻题材第一人称射击游戏，在外星战场抵御异形入侵。".into(),
            cover_image: "https://cdn.example.com/covers/xingji.png".into(),
            categories: vec!["射击游戏".into(), "科幻".into()],
            platform_name: "CloudGame".into(),
            game_icon: "https://cdn.example.com/icons/xingji.png".into(),
        },
        MockGameItem {
            id: 1004,
            name: "帝国征途".into(),
            description: "大型策略游戏，建立帝国、指挥军队、征服世界。".into(),
            cover_image: "https://cdn.example.com/covers/diguo.png".into(),
            categories: vec!["策略游戏".into(), "历史".into()],
            platform_name: "CloudGame".into(),
            game_icon: "https://cdn.example.com/icons/diguo.png".into(),
        },
        MockGameItem {
            id: 1005,
            name: "巅峰足球".into(),
            description: "真实物理引擎足球竞技游戏，支持在线多人对战。".into(),
            cover_image: "https://cdn.example.com/covers/zuqiu.png".into(),
            categories: vec!["体育竞技".into(), "足球".into()],
            platform_name: "CloudGame".into(),
            game_icon: "https://cdn.example.com/icons/zuqiu.png".into(),
        },
        MockGameItem {
            id: 1006,
            name: "天空之城".into(),
            description: "模拟经营类游戏，建造并管理你的空中城市。".into(),
            cover_image: "https://cdn.example.com/covers/tiankong.png".into(),
            categories: vec!["模拟经营".into(), "建造".into()],
            platform_name: "CloudGame".into(),
            game_icon: "https://cdn.example.com/icons/tiankong.png".into(),
        },
        MockGameItem {
            id: 1007,
            name: "糖果消消乐".into(),
            description: "轻松愉快的三消休闲游戏，数百个关卡等你挑战。".into(),
            cover_image: "https://cdn.example.com/covers/tangguo.png".into(),
            categories: vec!["休闲益智".into(), "三消".into()],
            platform_name: "CloudGame".into(),
            game_icon: "https://cdn.example.com/icons/tangguo.png".into(),
        },
        MockGameItem {
            id: 1008,
            name: "节奏大师".into(),
            description: "跟随音乐节拍点击屏幕，挑战你的反应速度。".into(),
            cover_image: "https://cdn.example.com/covers/jiezou.png".into(),
            categories: vec!["音乐节奏".into(), "休闲".into()],
            platform_name: "CloudGame".into(),
            game_icon: "https://cdn.example.com/icons/jiezou.png".into(),
        },
    ]
}

/// Build an LLM classification prompt to identify the game category from user input.
fn build_classification_prompt(user_input: &str, categories: &[String]) -> String {
    let cat_list = categories
        .iter()
        .enumerate()
        .map(|(i, c)| format!("{}. {}", i + 1, c))
        .collect::<Vec<_>>()
        .join("\n");

    format!(
        r#"你是一个游戏分类助手。请根据对话内容判断对话中最相关的游戏分类。

可选的游戏分类如下：
{cat_list}

对话内容："{user_input}"

请只返回一个最匹配的分类名称，不要输出其他任何内容。如果无法判断，请返回"动作冒险"。"#
    )
}

/// Handle game_id == 0 path: classify user input → return top 3 games for the category.
async fn handle_classify_and_list(
    args: Value,
    _pool: &sqlx::MySqlPool,
    ext_pool: Option<&sqlx::MySqlPool>,
    redis: Option<&redis::Client>,
    _channel: &str,
    _client_type: &str,
    llm: Option<&Arc<LlmRegistry>>,
    agent_id: Option<i64>,
) -> BuiltinResult {
    let put_to_ac = args
        .get("put_to_ac")
        .and_then(|v| v.as_bool())
        .unwrap_or(false);

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
            "data": "无法获取用户输入，无法进行分类"
        }));
    }

    // 2. 从外部 DB 获取游戏分类标签
    let ext_pool = match ext_pool {
        Some(p) => p,
        None => {
            tracing::warn!("ext_pool not available, game_info classify aborted");
            return Ok(serde_json::json!({
                "found": false,
                "data": "游戏分类服务不可用"
            }));
        }
    };

    let categories = match fetch_categories(ext_pool, redis).await {
        Ok(cats) if !cats.is_empty() => cats,
        Ok(_) => {
            tracing::warn!("cc_game_tag returned empty, game_info classify aborted");
            return Ok(serde_json::json!({
                "found": false,
                "data": "暂无游戏分类数据"
            }));
        }
        Err(e) => {
            tracing::warn!(error = %e, "fetch_categories failed, game_info classify aborted");
            return Ok(serde_json::json!({
                "found": false,
                "data": "游戏分类查询失败"
            }));
        }
    };
    let category = classify_user_input(&user_input, &categories, llm, agent_id).await;

    if category.is_empty() {
        tracing::warn!(user_input = %user_input, "game_info classify returned empty category");
        return Ok(serde_json::json!({
            "found": false,
            "data": "无法识别游戏分类"
        }));
    }

    tracing::info!(
        user_input = %user_input,
        category = %category,
        "game_info: LLM classified user input"
    );

    // 3. 根据分类获取游戏列表（取前 3 个）
    let games = mock_games_by_category(&category);

    // 4. 构造返回结果
    let game_items: Vec<Value> = games
        .iter()
        .map(|g| {
            serde_json::json!({
                "id": g.id,
                "name": g.name,
                "description": g.description,
                "cover_image": g.cover_image,
                "platform_name": g.platform_name,
                "game_icon": g.game_icon,
            })
        })
        .collect();

    let summary = format!(
        "根据你的描述，分类为「{}」，为你推荐以下 {} 款游戏：\n{}",
        category,
        game_items.len(),
        game_items
            .iter()
            .enumerate()
            .map(|(i, g)| format!(
                "{}. {}（ID: {}）- {}",
                i + 1,
                g["name"].as_str().unwrap_or(""),
                g["id"].as_i64().unwrap_or(0),
                g["description"].as_str().unwrap_or("")
            ))
            .collect::<Vec<_>>()
            .join("\n")
    );

    let mut output = serde_json::json!({
        "found": true,
        "id": 0,
        "name": null,
        "data": summary,
        "category": category,
        "games": game_items,
        "classified": true,
    });

    // 5. put_to_ac: true → 写入 AgentContext extensions
    if put_to_ac {
        let extension = serde_json::json!({
            "content_type": "card",
            "payload": {
                "type": "game_list",
                "category": category,
                "games": game_items,
            },
        });
        output["_agent_context_updates"] = serde_json::json!({
            "extensions": [extension],
        });
    }

    Ok(output)
}

/// Call LLM to classify user input into one of the given categories.
async fn classify_user_input(
    user_input: &str,
    categories: &[String],
    llm: Option<&Arc<LlmRegistry>>,
    agent_id: Option<i64>,
) -> String {
    // 尝试 LLM 分类
    if let (Some(llm), Some(_agent_id)) = (llm, agent_id) {
        let prompt = build_classification_prompt(user_input, categories);
        let messages = vec![serde_json::json!({
            "role": "user",
            "content": prompt,
        })];

        match llm.build_primary(None) {
            Ok((provider, model)) => {
                let req = ChatRequest {
                    model: Some(model),
                    messages,
                    max_tokens: 64,
                    temperature: 0.1,
                    tools: None,
                    tool_choice: None,
                    reasoning_effort: None,
                };
                let resp = provider.chat_with_retry(req, RetryMode::Standard, None).await;
                // Clone content before the if-let move, so we can log it on failure
                let raw_content = resp.content.clone();
                if let Some(content) = raw_content {
                    let trimmed = content.trim().to_string();
                    // 验证 LLM 返回的分类是否在列表中
                    let valid = categories
                        .iter()
                        .any(|c| trimmed.contains(c) || c.contains(&trimmed));
                    if valid && !trimmed.is_empty() {
                        return trimmed;
                    }
                }
                tracing::warn!(
                    user_input = %user_input,
                    llm_response = ?resp.content,
                    "LLM 分类失败或返回无效分类，使用关键词匹配兜底"
                );
            }
            Err(e) => {
                tracing::warn!(
                    error = %e,
                    "build_primary 失败，使用关键词匹配兜底"
                );
            }
        }
    }

    // 兜底：关键词匹配
    fallback_classify(user_input, categories)
}

/// Fallback keyword-based classification when LLM is unavailable.
fn fallback_classify(user_input: &str, _categories: &[String]) -> String {
    let input_lower = user_input.to_lowercase();

    // 分类关键词映射
    let keyword_map: &[(&str, &[&str])] = &[
        ("角色扮演", &["角色扮演", "rpg", "角色", "冒险", "奇幻", "仙侠"]),
        ("动作冒险", &["动作", "冒险", "格斗", "战斗", "闯关", "act"]),
        ("射击游戏", &["射击", "枪", "fps", "tps", "吃鸡", "战场"]),
        ("策略游戏", &["策略", "战棋", "slg", "帝国", "战争", "指挥"]),
        ("体育竞技", &["体育", "足球", "篮球", "赛车", "竞技", "运动"]),
        ("模拟经营", &["模拟", "经营", "建造", "养成", "管理"]),
        ("休闲益智", &["休闲", "益智", "消除", "解谜", "三消", "棋牌"]),
        ("音乐节奏", &["音乐", "节奏", "音游", "跳舞", "钢琴"]),
    ];

    for (category, keywords) in keyword_map {
        if keywords.iter().any(|kw| input_lower.contains(kw)) {
            return category.to_string();
        }
    }

    // 默认兜底
    "动作冒险".to_string()
}

pub const GAME_INFO_INPUT_SCHEMA: &str = r#"{
  "type": "object",
  "properties": {
    "game_id": {
      "type": "string",
      "description": "游戏 ID（文本形式，内部转为数字）。设为 \"0\" 时根据用户输入自动分类并推荐游戏。"
    },
    "put_to_ac": {
      "type": "boolean",
      "description": "是否将游戏信息放入 AgentContext（true=写入 AC 扩展卡片，false=仅作为输出变量）",
      "default": false
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
      "description": "游戏 ID（logic_game_id），game_id=0 时返回 0"
    },
    "name": {
      "type": "string",
      "description": "游戏名称，game_id=0 时为 null"
    },
    "data": {
      "type": "string",
      "description": "格式化文本，game_id=0 时为推荐列表摘要"
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
