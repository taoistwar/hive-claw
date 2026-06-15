use serde_json::Value;

use super::{BuiltinContext, BuiltinError, BuiltinResult};

/// sync wrapper: bridges async DB queries inside the tokio runtime via `block_in_place`.
/// channel and client_type are extracted from AgentContext metadata.
pub fn game_list(_args: Value, ctx: &BuiltinContext) -> BuiltinResult {
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
    tokio::task::block_in_place(move || {
        tokio::runtime::Handle::current().block_on(async move {
            game_list_async_impl(&pool, ext_pool.as_ref(), &channel, &client_type).await
        })
    })
}

/// Primary: cc_logic_game from external DB (id, name, alias).
/// Secondary: internal games + game_alias_entries supplements extra aliases.
/// Output: "- id: name、alias1、alias2"
async fn game_list_async_impl(
    pool: &sqlx::MySqlPool,
    ext_pool: Option<&sqlx::MySqlPool>,
    channel: &str,
    client_type: &str,
) -> BuiltinResult {
    let ext_pool = ext_pool.ok_or_else(|| BuiltinError::Exec("外部数据库未配置".into()))?;

    // 1. Load supplementary aliases from internal games table
    let internal_aliases = crate::services::game_service::load_internal_aliases(pool)
        .await
        .map_err(|e| BuiltinError::Exec(format!("{e}")))?;

    // 2. Primary: cc_logic_game from external DB, filtered by channel & client_type
    let external = crate::services::game_service::list_external_games(ext_pool, channel, client_type)
        .await
        .map_err(|e| BuiltinError::Exec(format!("{e}")))?;

    // 3. Merge: for each external game, collect all aliases (its own + internal supplements)
    let mut entries: Vec<(u32, String, Vec<String>)> = Vec::new();
    for (id, name, ext_alias) in external {
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

        entries.push((id, name, aliases));
    }

    // 4. Format output: "- id: name、alias1、alias2"
    let lines: Vec<String> = entries
        .iter()
        .map(|(id, name, aliases)| {
            if aliases.is_empty() {
                format!("- ID: {}, 名称: {}", id, name)
            } else {
                format!("- ID: {}, 名称: {}, 别名: {}", id, name, aliases.join("、"))
            }
        })
        .collect();

    let games: Vec<Value> = entries
        .into_iter()
        .map(|(id, name, aliases)| {
            serde_json::json!({
                "id": id,
                "name": name,
                "aliases": aliases,
            })
        })
        .collect();

    Ok(serde_json::json!({
        "data": lines.join("\n"),
        "games": games,
    }))
}

pub const GAME_LIST_INPUT_SCHEMA: &str = r#"{
  "type": "object",
  "properties": {}
}"#;

pub const GAME_LIST_OUTPUT_SCHEMA: &str = r#"{
  "type": "object",
  "properties": {
    "data": {
      "type": "string",
      "description": "Formatted game list with aliases"
    },
    "games": {
      "type": "array",
      "items": {
        "type": "object",
        "properties": {
          "id": { "type": "integer" },
          "name": { "type": "string" },
          "aliases": { "type": "array", "items": { "type": "string" } }
        }
      }
    }
  }
}"#;
