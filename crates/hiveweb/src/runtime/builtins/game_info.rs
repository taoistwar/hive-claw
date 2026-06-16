use serde_json::Value;

use super::{BuiltinContext, BuiltinError, BuiltinResult};

/// sync wrapper: bridges async DB queries inside the tokio runtime via `block_in_place`.
pub fn game_info(args: Value, ctx: &BuiltinContext) -> BuiltinResult {
    let pool = ctx.pool.clone();
    let ext_pool = ctx.ext_pool.cloned();
    let redis = ctx.redis.cloned();
    tokio::task::block_in_place(move || {
        tokio::runtime::Handle::current()
            .block_on(async move { game_info_async_impl(args, &pool, ext_pool.as_ref(), redis.as_ref()).await })
    })
}

/// Query a single game's details by game_id (text input, parsed to integer).
/// Input: { "game_id": "<text>", "put_to_ac": <bool> }
/// - game_id: 文本形式的游戏 ID，内部转为数字。转换失败返回空字符串。
/// - put_to_ac: true → 游戏信息写入 AgentContext extensions (card/game)；false → 作为输出变量返回。
async fn game_info_async_impl(
    args: Value,
    pool: &sqlx::MySqlPool,
    ext_pool: Option<&sqlx::MySqlPool>,
    redis: Option<&redis::Client>,
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
                "name": null,
                "aliases": [],
                "games": [],
            }));
        }
    };

    let ext_pool = ext_pool.ok_or_else(|| BuiltinError::Exec("外部数据库未配置".into()))?;

    // 2. Query external DB (primary) — Redis 缓存优先
    let external = if let Some(r) = redis {
        crate::services::game_service::get_external_game_by_id_cached(r, ext_pool, game_id)
            .await
            .map_err(|e| BuiltinError::Exec(format!("{e}")))?
    } else {
        crate::services::game_service::get_external_game_by_id(ext_pool, game_id)
            .await
            .map_err(|e| BuiltinError::Exec(format!("{e}")))?
    };

    let (id, name, ext_alias) = match external {
        Some(row) if row.0 != 0 => row,
        _ => {
            return Ok(serde_json::json!({
                "found": false,
                "data": format!("未找到游戏 ID {} 的信息", game_id),
                "id": game_id,
                "name": null,
                "aliases": [],
                "games": [],
            }));
        }
    };

    // 3. Load supplementary aliases from internal games table
    let internal_aliases = crate::services::game_service::load_internal_aliases(pool)
        .await
        .map_err(|e| BuiltinError::Exec(format!("{e}")))?;

    // 4. Merge aliases (external alias + internal supplements)
    let mut aliases: Vec<String> = Vec::new();

    for alias in ext_alias.split(',') {
        let alias = alias.trim();
        if alias.is_empty() {
            continue;
        }
        if !aliases.contains(&alias.to_string()) {
            aliases.push(alias.to_string());
        }
    }

    if let Some(supp) = internal_aliases.get(&name) {
        for alias in supp {
            if !aliases.contains(alias) {
                aliases.push(alias.clone());
            }
        }
    }

    let game_payload = serde_json::json!({
        "id": id,
        "name": name,
    });

    let mut output = serde_json::json!({
        "found": true,
        "id": id,
        "name": name,
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

pub const GAME_INFO_INPUT_SCHEMA: &str = r#"{
  "type": "object",
  "properties": {
    "game_id": {
      "type": "string",
      "description": "游戏 ID（文本形式，内部转为数字）"
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
      "description": "是否找到该游戏"
    },
    "id": {
      "type": "integer",
      "description": "游戏 ID"
    },
    "name": {
      "type": "string",
      "description": "游戏名称"
    }
  }
}"#;
