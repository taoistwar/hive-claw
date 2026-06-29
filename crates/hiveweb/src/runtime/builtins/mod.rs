//! Builtin function implementations (T079 / FR-010 v5)
//!
//! Glue functions that don't depend on WASM, upserted into the `functions` table
//! at startup (kind=1). When the orchestrator selects a kind=1 Tool it goes directly
//! through host code, bypassing the Plugin invoker.
//!
//! Builtins are non-removable; can be disabled (disabled column TBD — future schema extension).

mod chat_respond;
mod format_template;
mod game_info;
mod game_list;
mod json_parse;
mod json_stringify;
mod query_balance;
mod support_card;
mod text_regex_match;
mod tools;

use serde_json::Value;
use sqlx::MySqlPool;
use std::sync::Arc;

use agent::context::AgentContext;
use crate::runtime::llm::LlmRegistry;

// Re-export handlers for the registry
use chat_respond::chat_respond;
use format_template::format_template;
use game_info::game_info;
use game_list::game_list;
use json_parse::json_parse;
use json_stringify::json_stringify;
use query_balance::query_balance;
use support_card::support_card;
use text_regex_match::text_regex_match;

// Re-export schemas for ensure_registered
use chat_respond::{CHAT_RESPOND_INPUT_SCHEMA, CHAT_RESPOND_OUTPUT_SCHEMA};
use format_template::{FORMAT_TEMPLATE_INPUT_SCHEMA, FORMAT_TEMPLATE_OUTPUT_SCHEMA};
use game_info::{GAME_INFO_INPUT_SCHEMA, GAME_INFO_OUTPUT_SCHEMA};
use game_list::{GAME_LIST_INPUT_SCHEMA, GAME_LIST_OUTPUT_SCHEMA};
use json_parse::{JSON_PARSE_INPUT_SCHEMA, JSON_PARSE_OUTPUT_SCHEMA};
use json_stringify::{JSON_STRINGIFY_INPUT_SCHEMA, JSON_STRINGIFY_OUTPUT_SCHEMA};
use query_balance::{QUERY_BALANCE_INPUT_SCHEMA, QUERY_BALANCE_OUTPUT_SCHEMA};
use support_card::{SUPPORT_CARD_INPUT_SCHEMA, SUPPORT_CARD_OUTPUT_SCHEMA};
use text_regex_match::{TEXT_REGEX_MATCH_INPUT_SCHEMA, TEXT_REGEX_MATCH_OUTPUT_SCHEMA};

#[derive(Debug, thiserror::Error)]
pub enum BuiltinError {
    #[error("invalid arguments: {0}")]
    BadArgs(String),
    #[error("execution failed: {0}")]
    Exec(String),
}

pub type BuiltinResult = Result<Value, BuiltinError>;

/// Context needed when executing a builtin function (DB connection pool + AgentContext)
pub struct BuiltinContext<'a> {
    pub pool: &'a MySqlPool,
    pub ext_pool: Option<&'a MySqlPool>,
    /// Redis client for cache-aside operations. `None` when Redis is unavailable.
    pub redis: Option<&'a redis::Client>,
    /// AgentContext for reading/writing runtime state during hook/tool execution.
    /// `None` when called from contexts without AgentContext (e.g., workflow executor).
    pub agent_ctx: Option<Arc<AgentContext>>,
    /// LLM registry for builtins that need classification/summarization.
    /// `None` when called from contexts without LLM access (e.g., workflow executor, test).
    pub llm: Option<&'a Arc<LlmRegistry>>,
    /// Current agent ID for resolving model preset when calling LLM from builtins.
    /// `None` when agent_id is unavailable.
    pub agent_id: Option<i64>,
}

// ---------- Registry ----------

#[derive(Debug, Clone, Copy)]
pub struct BuiltinDef {
    pub identifier: &'static str,
    pub name: &'static str,
    pub description: &'static str,
    pub input_schema: &'static str,
    pub output_schema: &'static str,
    pub required_capabilities: &'static [&'static str],
    pub handler: fn(Value, &BuiltinContext) -> BuiltinResult,
}

pub const BUILTINS: &[BuiltinDef] = &[
    BuiltinDef {
        identifier: "format.template",
        name: "Format Template",
        description: "Render a template string with named {var} placeholders.",
        input_schema: FORMAT_TEMPLATE_INPUT_SCHEMA,
        output_schema: FORMAT_TEMPLATE_OUTPUT_SCHEMA,
        required_capabilities: &[],
        handler: format_template,
    },
    BuiltinDef {
        identifier: "json.parse",
        name: "JSON Parse",
        description: "Parse a JSON string into a structured value.",
        input_schema: JSON_PARSE_INPUT_SCHEMA,
        output_schema: JSON_PARSE_OUTPUT_SCHEMA,
        required_capabilities: &[],
        handler: json_parse,
    },
    BuiltinDef {
        identifier: "json.stringify",
        name: "JSON Stringify",
        description: "Serialize a value to a JSON string (optionally pretty-printed).",
        input_schema: JSON_STRINGIFY_INPUT_SCHEMA,
        output_schema: JSON_STRINGIFY_OUTPUT_SCHEMA,
        required_capabilities: &[],
        handler: json_stringify,
    },
    BuiltinDef {
        identifier: "text.regex_match",
        name: "Regex Match",
        description: "Apply a Rust-syntax regex against text and return matches with capture groups.",
        input_schema: TEXT_REGEX_MATCH_INPUT_SCHEMA,
        output_schema: TEXT_REGEX_MATCH_OUTPUT_SCHEMA,
        required_capabilities: &[],
        handler: text_regex_match,
    },
    BuiltinDef {
        identifier: "chat.respond",
        name: "Chat Respond",
        description: "Submit the final user-visible reply (signals orchestrator to end the turn).",
        input_schema: CHAT_RESPOND_INPUT_SCHEMA,
        output_schema: CHAT_RESPOND_OUTPUT_SCHEMA,
        required_capabilities: &[super::capability::CHAT_RESPOND],
        handler: chat_respond,
    },
    BuiltinDef {
        identifier: "game.list",
        name: "Game List",
        description: "Get merged game list with aliases from internal and external databases.",
        input_schema: GAME_LIST_INPUT_SCHEMA,
        output_schema: GAME_LIST_OUTPUT_SCHEMA,
        required_capabilities: &[],
        handler: game_list,
    },
    BuiltinDef {
        identifier: "game.info",
        name: "Game Info",
        description: "根据游戏 ID查询游戏信息，生成游戏卡片。",
        input_schema: GAME_INFO_INPUT_SCHEMA,
        output_schema: GAME_INFO_OUTPUT_SCHEMA,
        required_capabilities: &[],
        handler: game_info,
    },
    BuiltinDef {
        identifier: "query_balance",
        name: "Query Balance",
        description: "查询用户余额与会员等级等信息，生成会员卡片。",
        input_schema: QUERY_BALANCE_INPUT_SCHEMA,
        output_schema: QUERY_BALANCE_OUTPUT_SCHEMA,
        required_capabilities: &[],
        handler: query_balance,
    },
    BuiltinDef {
        identifier: "support_card",
        name: "Support Card",
        description: "为客服内容生成支持卡片。",
        input_schema: SUPPORT_CARD_INPUT_SCHEMA,
        output_schema: SUPPORT_CARD_OUTPUT_SCHEMA,
        required_capabilities: &[],
        handler: support_card,
    },
];

pub fn lookup(identifier: &str) -> Option<&'static BuiltinDef> {
    BUILTINS.iter().find(|b| b.identifier == identifier)
}

/// Startup idempotent upsert — INSERT IGNORE fallback (identifier UNIQUE)
/// Also creates a corresponding builtin tool (source='builtin', kind=1) for each builtin function.
pub async fn ensure_registered(pool: &sqlx::MySqlPool) -> Result<(), sqlx::Error> {
    for b in BUILTINS {
        let caps_json =
            serde_json::to_string(b.required_capabilities).unwrap_or_else(|_| "[]".to_string());

        sqlx::query(
            r#"INSERT INTO functions
               (identifier, name, description, kind, input_schema, output_schema,
                required_capabilities, plugin_id, plugin_export)
               VALUES (?, ?, ?, 1, CAST(? AS JSON), CAST(? AS JSON), CAST(? AS JSON), NULL, NULL)
               ON DUPLICATE KEY UPDATE
                 name = VALUES(name),
                 description = VALUES(description),
                 input_schema = VALUES(input_schema),
                 output_schema = VALUES(output_schema),
                 required_capabilities = VALUES(required_capabilities)"#,
        )
        .bind(b.identifier)
        .bind(b.name)
        .bind(b.description)
        .bind(b.input_schema)
        .bind(b.output_schema)
        .bind(&caps_json)
        .execute(pool)
        .await?;

        // Create corresponding builtin tool for each builtin function
        // tool.identifier = function.identifier (consistent)
        sqlx::query(
            r#"INSERT INTO tools
               (identifier, name, description, kind, source, is_always,
                function_id, workflow_id, input_schema, output_schema, required_capabilities)
               VALUES (?, ?, ?, 1, 'builtin', 0,
                       (SELECT id FROM functions WHERE identifier = ? LIMIT 1),
                       NULL, CAST(? AS JSON), CAST(? AS JSON), CAST(? AS JSON))
               ON DUPLICATE KEY UPDATE
                 name = VALUES(name),
                 description = VALUES(description),
                 input_schema = VALUES(input_schema),
                 output_schema = VALUES(output_schema),
                 required_capabilities = VALUES(required_capabilities)"#,
        )
        .bind(b.identifier)
        .bind(b.name)
        .bind(b.description)
        .bind(b.identifier)
        .bind(b.input_schema)
        .bind(b.output_schema)
        .bind(&caps_json)
        .execute(pool)
        .await?;
    }
    tracing::info!(
        count = BUILTINS.len(),
        "builtin functions and tools upserted"
    );

    // Seed invoke_function meta-tool (is_always=1 → all Agents auto-loaded)
    // function_id=NULL means it doesn't wrap a specific function, resolved dynamically at runtime
    sqlx::query(
        r#"INSERT INTO tools
           (identifier, name, description, kind, source, is_always,
            function_id, workflow_id, input_schema, output_schema)
           VALUES ('invoke_function', 'Invoke Function',
                   'Invoke a function by its identifier. Dynamically resolves to any function (builtin or custom) and executes it.',
                   1, 'builtin', 1,
                   NULL, NULL,
                   CAST('{
                     "type": "object",
                     "properties": {
                       "function_identifier": { "type": "string", "description": "The identifier of the function to invoke" },
                       "function_input": { "type": "object", "description": "The input arguments matching the function input_schema" }
                     },
                     "required": ["function_identifier", "function_input"]
                   }' AS JSON),
                   CAST('{ "type": "object" }' AS JSON))
           ON DUPLICATE KEY UPDATE
             name = VALUES(name),
             description = VALUES(description),
             input_schema = VALUES(input_schema),
             output_schema = VALUES(output_schema)"#,
    )
    .execute(pool)
    .await?;

    // Seed invoke_workflow meta-tool (is_always=1 → all Agents auto-loaded)
    sqlx::query(
        r#"INSERT INTO tools
           (identifier, name, description, kind, source, is_always,
            function_id, workflow_id, input_schema, output_schema)
           VALUES ('invoke_workflow', 'Invoke Workflow',
                   'Invoke a workflow by its identifier. Executes a DAG of function calls.',
                   1, 'builtin', 1,
                   NULL, NULL,
                   CAST('{
                     "type": "object",
                     "properties": {
                       "workflow_identifier": { "type": "string", "description": "The identifier of the workflow to invoke" },
                       "workflow_input": { "type": "object", "description": "The input arguments matching the workflow input_schema" }
                     },
                     "required": ["workflow_identifier", "workflow_input"]
                   }' AS JSON),
                   CAST('{ "type": "object" }' AS JSON))
           ON DUPLICATE KEY UPDATE
             name = VALUES(name),
             description = VALUES(description),
             input_schema = VALUES(input_schema),
             output_schema = VALUES(output_schema)"#,
    )
    .execute(pool)
    .await?;

    tracing::info!("meta-tools (invoke_function, invoke_workflow) upserted");

    // Register agent-level builtin tools (read_file, write_file, exec, etc.)
    tools::ensure_registered(pool).await?;

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    /// Test helper — connect_lazy won't actually connect to DB, just satisfies the signature
    async fn test_ctx() -> BuiltinContext<'static> {
        let test_url = std::env::var("DATABASE_URL")
            .unwrap_or_else(|_| "mysql://root:root@localhost:3306/hive_claw_test".to_string());
        let pool = Box::leak(Box::new(
            sqlx::MySqlPool::connect_lazy(&test_url).expect("connect_lazy"),
        ));
        BuiltinContext {
            pool,
            ext_pool: None,
            redis: None,
            agent_ctx: None,
            llm: None,
            agent_id: None,
        }
    }

    #[tokio::test]
    async fn format_template_basic() {
        let ctx = test_ctx().await;
        let r = format_template(
            json!({
                "template": "Hello {name}, you are {age} years old",
                "vars": {"name": "Alice", "age": 30}
            }),
            &ctx,
        )
        .unwrap();
        assert_eq!(r, Value::String("Hello Alice, you are 30 years old".into()));
    }

    #[tokio::test]
    async fn json_parse_roundtrip() {
        let ctx = test_ctx().await;
        let r = json_parse(json!({"text": r#"{"a":1}"#}), &ctx).unwrap();
        assert_eq!(r, json!({"a": 1}));
    }

    #[tokio::test]
    async fn json_stringify_pretty() {
        let ctx = test_ctx().await;
        let r = json_stringify(json!({"value": {"a": 1}, "pretty": true}), &ctx).unwrap();
        let s = r.as_str().unwrap();
        assert!(s.contains("\n"));
    }

    #[tokio::test]
    async fn regex_capture_groups() {
        let ctx = test_ctx().await;
        let r = text_regex_match(
            json!({
                "text": "name=alice id=42",
                "pattern": r"(\w+)=(\w+)",
                "all": true
            }),
            &ctx,
        )
        .unwrap();
        let matches = r.get("matches").and_then(|m| m.as_array()).unwrap();
        assert_eq!(matches.len(), 2);
    }

    #[tokio::test]
    async fn chat_respond_wraps() {
        let ctx = test_ctx().await;
        let r = chat_respond(json!({"content": "final answer"}), &ctx).unwrap();
        assert_eq!(r["final_content"], "final answer");
    }
}
