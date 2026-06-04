//! Builtin function implementations (T079 / FR-010 v5)
//!
//! 5 个不依赖 WASM 的"胶水"函数，启动期 upsert 到 `functions` 表（kind=1）。
//! 调用入口：当 orchestrator 选中 kind=1 Tool 时直接走宿主代码，绕过 Plugin invoker。
//!
//! 内置不可删除；可被禁用（disabled 字段暂未引入 — 后续 schema 扩展时加）。
mod query_balance;
use regex::Regex;
use rust_decimal::prelude::*;
use rust_decimal::Decimal;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use sqlx::MySqlPool;
use std::sync::Arc;
use query_balance::query_balance;
use agent::context::AgentContext;

#[derive(Debug, thiserror::Error)]
pub enum BuiltinError {
    #[error("invalid arguments: {0}")]
    BadArgs(String),
    #[error("execution failed: {0}")]
    Exec(String),
}

pub type BuiltinResult = Result<Value, BuiltinError>;

/// 内置函数执行时需要的上下文（DB 连接池 + AgentContext）
pub struct BuiltinContext<'a> {
    pub pool: &'a MySqlPool,
    pub ext_pool: Option<&'a MySqlPool>,
    /// AgentContext for reading/writing runtime state during hook/tool execution.
    /// `None` when called from contexts without AgentContext (e.g., workflow executor).
    pub agent_ctx: Option<Arc<AgentContext>>,
}

// ---------- format.template ----------

#[derive(Debug, Deserialize)]
struct FormatTemplateArgs {
    template: String,
    #[serde(default)]
    vars: serde_json::Map<String, Value>,
}

/// 简单 `{var}` 占位符替换；嵌套对象暂不支持。
pub fn format_template(args: Value, _ctx: &BuiltinContext) -> BuiltinResult {
    let parsed: FormatTemplateArgs =
        serde_json::from_value(args).map_err(|e| BuiltinError::BadArgs(format!("{e}")))?;
    let mut out = parsed.template;
    for (k, v) in parsed.vars.iter() {
        let placeholder = format!("{{{k}}}");
        let replacement = match v {
            Value::String(s) => s.clone(),
            other => other.to_string(),
        };
        out = out.replace(&placeholder, &replacement);
    }
    Ok(Value::String(out))
}

pub const FORMAT_TEMPLATE_INPUT_SCHEMA: &str = r#"{
  "type": "object",
  "properties": {
    "template": { "type": "string", "description": "Template with {var} placeholders" },
    "vars": { "type": "object", "description": "Map of placeholder name → value" }
  },
  "required": ["template"]
}"#;

pub const FORMAT_TEMPLATE_OUTPUT_SCHEMA: &str =
    r#"{ "type": "string", "description": "Rendered template" }"#;

// ---------- json.parse ----------

#[derive(Debug, Deserialize)]
struct JsonParseArgs {
    text: String,
}

pub fn json_parse(args: Value, _ctx: &BuiltinContext) -> BuiltinResult {
    let parsed: JsonParseArgs =
        serde_json::from_value(args).map_err(|e| BuiltinError::BadArgs(format!("{e}")))?;
    serde_json::from_str::<Value>(&parsed.text).map_err(|e| BuiltinError::Exec(format!("{e}")))
}

pub const JSON_PARSE_INPUT_SCHEMA: &str = r#"{
  "type": "object",
  "properties": { "text": { "type": "string" } },
  "required": ["text"]
}"#;
pub const JSON_PARSE_OUTPUT_SCHEMA: &str =
    r#"{ "type": ["object", "array", "string", "number", "boolean", "null"] }"#;

// ---------- json.stringify ----------

#[derive(Debug, Deserialize)]
struct JsonStringifyArgs {
    value: Value,
    #[serde(default)]
    pretty: bool,
}

pub fn json_stringify(args: Value, _ctx: &BuiltinContext) -> BuiltinResult {
    let parsed: JsonStringifyArgs =
        serde_json::from_value(args).map_err(|e| BuiltinError::BadArgs(format!("{e}")))?;
    let s = if parsed.pretty {
        serde_json::to_string_pretty(&parsed.value)
    } else {
        serde_json::to_string(&parsed.value)
    }
    .map_err(|e| BuiltinError::Exec(format!("{e}")))?;
    Ok(Value::String(s))
}

pub const JSON_STRINGIFY_INPUT_SCHEMA: &str = r#"{
  "type": "object",
  "properties": {
    "value": {},
    "pretty": { "type": "boolean", "default": false }
  },
  "required": ["value"]
}"#;
pub const JSON_STRINGIFY_OUTPUT_SCHEMA: &str = r#"{ "type": "string" }"#;

// ---------- text.regex_match ----------

#[derive(Debug, Deserialize)]
struct RegexMatchArgs {
    text: String,
    pattern: String,
    #[serde(default)]
    all: bool,
}

#[derive(Debug, Serialize)]
struct RegexMatchReply {
    matches: Vec<RegexMatchEntry>,
}

#[derive(Debug, Serialize)]
struct RegexMatchEntry {
    full: String,
    groups: Vec<Option<String>>,
}

pub fn text_regex_match(args: Value, _ctx: &BuiltinContext) -> BuiltinResult {
    let parsed: RegexMatchArgs =
        serde_json::from_value(args).map_err(|e| BuiltinError::BadArgs(format!("{e}")))?;
    let re = Regex::new(&parsed.pattern)
        .map_err(|e| BuiltinError::BadArgs(format!("pattern compile: {e}")))?;
    let collect_one = |caps: regex::Captures| RegexMatchEntry {
        full: caps
            .get(0)
            .map(|m| m.as_str().to_string())
            .unwrap_or_default(),
        groups: caps
            .iter()
            .skip(1)
            .map(|opt| opt.map(|m| m.as_str().to_string()))
            .collect(),
    };
    let matches: Vec<RegexMatchEntry> = if parsed.all {
        re.captures_iter(&parsed.text).map(collect_one).collect()
    } else {
        re.captures(&parsed.text)
            .map(collect_one)
            .into_iter()
            .collect()
    };
    Ok(serde_json::to_value(RegexMatchReply { matches })
        .map_err(|e| BuiltinError::Exec(format!("{e}")))?)
}

pub const TEXT_REGEX_MATCH_INPUT_SCHEMA: &str = r#"{
  "type": "object",
  "properties": {
    "text": { "type": "string" },
    "pattern": { "type": "string", "description": "Rust regex syntax" },
    "all": { "type": "boolean", "default": false }
  },
  "required": ["text", "pattern"]
}"#;
pub const TEXT_REGEX_MATCH_OUTPUT_SCHEMA: &str = r#"{
  "type": "object",
  "properties": {
    "matches": {
      "type": "array",
      "items": {
        "type": "object",
        "properties": {
          "full": { "type": "string" },
          "groups": { "type": "array", "items": { "type": ["string", "null"] } }
        }
      }
    }
  }
}"#;

// ---------- chat.respond ----------

#[derive(Debug, Deserialize)]
struct ChatRespondArgs {
    content: String,
}

#[derive(Debug, Serialize)]
struct ChatRespondReply {
    /// orchestrator 用此字段把内容当作"最终用户可见回复"标识
    final_content: String,
}

/// chat.respond 是 orchestrator 的"提交最终回复"信号；执行结果由 orchestrator
/// 拿到 final_content 字段后作为 done event 的最终内容。
/// 本函数本身只做参数透传 + 包装。
pub fn chat_respond(args: Value, _ctx: &BuiltinContext) -> BuiltinResult {
    let parsed: ChatRespondArgs =
        serde_json::from_value(args).map_err(|e| BuiltinError::BadArgs(format!("{e}")))?;
    Ok(serde_json::to_value(ChatRespondReply {
        final_content: parsed.content,
    })
    .unwrap_or(Value::Null))
}

pub const CHAT_RESPOND_INPUT_SCHEMA: &str = r#"{
  "type": "object",
  "properties": {
    "content": { "type": "string", "description": "Final user-visible message" }
  },
  "required": ["content"]
}"#;
pub const CHAT_RESPOND_OUTPUT_SCHEMA: &str = r#"{
  "type": "object",
  "properties": { "final_content": { "type": "string" } }
}"#;

// ---------- game_list ----------

/// sync wrapper：通过 `block_in_place` 在 tokio runtime 内桥接 async DB 查询。
/// 不能用 `Handle::block_on` —— 当 caller 已经在 tokio worker 线程上（如 axum handler）
/// 会 panic "Cannot start a runtime from within a runtime"。
pub fn game_list(_args: Value, ctx: &BuiltinContext) -> BuiltinResult {
    let pool = ctx.pool.clone();
    let ext_pool = ctx.ext_pool.cloned();
    tokio::task::block_in_place(move || {
        tokio::runtime::Handle::current().block_on(async move {
            game_list_async_impl(&pool, ext_pool.as_ref()).await
        })
    })
}

/// 合并内部 games 表 + 外部 cc_logic_game 表的游戏列表，格式化为文本
async fn game_list_async_impl(
    pool: &sqlx::MySqlPool,
    ext_pool: Option<&sqlx::MySqlPool>,
) -> BuiltinResult {
    // 1. 内部 games 表（含别名）
    let internal =
        sqlx::query_as::<_, (String, Option<String>)>(
            "SELECT g.name, gae.alias FROM games g
             LEFT JOIN game_alias_entries gae ON gae.game_id = g.id
             ORDER BY g.name",
        )
        .fetch_all(pool)
        .await
        .map_err(|e| BuiltinError::Exec(format!("game_list internal query: {e}")))?;

    let mut entries: Vec<(String, Vec<String>)> = Vec::new();
    for (name, alias) in internal {
        if let Some(entry) = entries.iter_mut().find(|e| e.0 == name) {
            if let Some(alias) = alias {
                if !alias.trim().is_empty() {
                    entry.1.push(alias.trim().to_string());
                }
            }
        } else {
            let aliases = alias
                .filter(|a| !a.trim().is_empty())
                .map(|a| vec![a.trim().to_string()])
                .unwrap_or_default();
            entries.push((name, aliases));
        }
    }

    // 2. 外部 cc_logic_game 表
    if let Some(ext_pool) = ext_pool {
        let external =
            sqlx::query_as::<_, (String,)>("SELECT name FROM cc_logic_game ORDER BY name")
                .fetch_all(ext_pool)
                .await
                .map_err(|e| BuiltinError::Exec(format!("game_list external query: {e}")))?;

        for (name,) in external {
            if !entries.iter().any(|e| e.0 == name) {
                entries.push((name, vec![]));
            }
        }
    }

    // 3. 格式化输出
    let lines: Vec<String> = entries
        .into_iter()
        .map(|(name, aliases)| {
            if aliases.is_empty() {
                format!("- {}", name)
            } else {
                format!("- {}: {}", name, aliases.join("、"))
            }
        })
        .collect();

    Ok(Value::Object(
        serde_json::Map::from_iter([(
            "data".to_string(),
            Value::String(lines.join("\n")),
        )]),
    ))
}

pub const GAME_LIST_INPUT_SCHEMA: &str = r#"{
  "type": "object",
  "properties": {},
  "additionalProperties": false
}"#;

pub const GAME_LIST_OUTPUT_SCHEMA: &str = r#"{
  "type": "object",
  "properties": {
    "data": {
      "type": "string",
      "description": "Formatted game list with aliases"
    }
  }
}"#;

pub const QUERY_BALANCE_INPUT_SCHEMA: &str = r#"{
  "type": "object",
  "properties": {},
  "description": "查询当前对话用户的余额与会员等级。user_id 从 AgentContext 隐式获取。"
}"#;

pub const QUERY_BALANCE_OUTPUT_SCHEMA: &str = r#"{
  "type": "object",
  "properties": {
    "user_id": { "type": "integer", "description": "用户 ID" },
    "balance": { "type": "number", "description": "用户余额" },
    "currency": { "type": "string", "description": "货币类型" },
    "has_membership": { "type": "boolean", "description": "是否有有效会员" },
    "membership": {
      "type": "object",
      "description": "会员信息（如果有）",
      "properties": {
        "effective_end_time": { "type": "string" },
        "membership_category": { "type": "string" },
        "level_name": { "type": "string" }
      }
    }
  }
}"#;

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
        identifier: "query_balance",
        name: "Query Balance",
        description: "查询用户余额与会员等级：从外部数据库查询 cc_user_asset_coin 资产总和及 cc_user_membership 会员信息。无有效会员时自动推送 firstPay 卡片。",
        input_schema: QUERY_BALANCE_INPUT_SCHEMA,
        output_schema: QUERY_BALANCE_OUTPUT_SCHEMA,
        required_capabilities: &[],
        handler: query_balance,
    },
];

pub fn lookup(identifier: &str) -> Option<&'static BuiltinDef> {
    BUILTINS.iter().find(|b| b.identifier == identifier)
}

/// 启动期 idempotent upsert — INSERT IGNORE 兜底 (identifier UNIQUE)
/// 同时为每个 builtin function 创建对应的 builtin tool（source='builtin', kind=1）
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

        // 为每个 builtin function 创建对应的 builtin tool
        // tool.identifier = function.identifier（保持一致）
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

    // Seed invoke_function meta-tool (is_always=1 → 所有 Agent 自动加载)
    // function_id=NULL 表示不包装具体 function，运行时动态查找
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

    // Seed invoke_workflow meta-tool (is_always=1 → 所有 Agent 自动加载)
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
    super::builtin_tools::ensure_registered(pool).await?;

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    /// 测试用空 context — connect_lazy 不会真正连接 DB，仅满足签名要求
    async fn test_ctx() -> BuiltinContext<'static> {
        let test_url = std::env::var("DATABASE_URL")
            .unwrap_or_else(|_| "mysql://root:root@localhost:3306/hive_claw_test".to_string());
        let pool = Box::leak(Box::new(
            sqlx::MySqlPool::connect_lazy(&test_url).expect("connect_lazy"),
        ));
        BuiltinContext {
            pool,
            ext_pool: None,
            agent_ctx: None,
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
