//! Builtin function implementations (T079 / FR-010 v5)
//!
//! 5 个不依赖 WASM 的"胶水"函数，启动期 upsert 到 `functions` 表（kind=1）。
//! 调用入口：当 orchestrator 选中 kind=1 Tool 时直接走宿主代码，绕过 Plugin invoker。
//!
//! 内置不可删除；可被禁用（disabled 字段暂未引入 — 后续 schema 扩展时加）。

use regex::Regex;
use serde::{Deserialize, Serialize};
use serde_json::Value;

#[derive(Debug, thiserror::Error)]
pub enum BuiltinError {
    #[error("invalid arguments: {0}")]
    BadArgs(String),
    #[error("execution failed: {0}")]
    Exec(String),
}

pub type BuiltinResult = Result<Value, BuiltinError>;

// ---------- format.template ----------

#[derive(Debug, Deserialize)]
struct FormatTemplateArgs {
    template: String,
    #[serde(default)]
    vars: serde_json::Map<String, Value>,
}

/// 简单 `{var}` 占位符替换；嵌套对象暂不支持。
pub fn format_template(args: Value) -> BuiltinResult {
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

pub fn json_parse(args: Value) -> BuiltinResult {
    let parsed: JsonParseArgs =
        serde_json::from_value(args).map_err(|e| BuiltinError::BadArgs(format!("{e}")))?;
    serde_json::from_str::<Value>(&parsed.text).map_err(|e| BuiltinError::Exec(format!("{e}")))
}

pub const JSON_PARSE_INPUT_SCHEMA: &str = r#"{
  "type": "object",
  "properties": { "text": { "type": "string" } },
  "required": ["text"]
}"#;
pub const JSON_PARSE_OUTPUT_SCHEMA: &str = r#"{ "type": ["object", "array", "string", "number", "boolean", "null"] }"#;

// ---------- json.stringify ----------

#[derive(Debug, Deserialize)]
struct JsonStringifyArgs {
    value: Value,
    #[serde(default)]
    pretty: bool,
}

pub fn json_stringify(args: Value) -> BuiltinResult {
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

pub fn text_regex_match(args: Value) -> BuiltinResult {
    let parsed: RegexMatchArgs =
        serde_json::from_value(args).map_err(|e| BuiltinError::BadArgs(format!("{e}")))?;
    let re = Regex::new(&parsed.pattern)
        .map_err(|e| BuiltinError::BadArgs(format!("pattern compile: {e}")))?;
    let collect_one = |caps: regex::Captures| RegexMatchEntry {
        full: caps.get(0).map(|m| m.as_str().to_string()).unwrap_or_default(),
        groups: caps
            .iter()
            .skip(1)
            .map(|opt| opt.map(|m| m.as_str().to_string()))
            .collect(),
    };
    let matches: Vec<RegexMatchEntry> = if parsed.all {
        re.captures_iter(&parsed.text).map(collect_one).collect()
    } else {
        re.captures(&parsed.text).map(collect_one).into_iter().collect()
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
pub fn chat_respond(args: Value) -> BuiltinResult {
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

// ---------- Registry ----------

#[derive(Debug, Clone, Copy)]
pub struct BuiltinDef {
    pub identifier: &'static str,
    pub name: &'static str,
    pub description: &'static str,
    pub input_schema: &'static str,
    pub output_schema: &'static str,
    pub handler: fn(Value) -> BuiltinResult,
}

pub const BUILTINS: &[BuiltinDef] = &[
    BuiltinDef {
        identifier: "format.template",
        name: "Format Template",
        description: "Render a template string with named {var} placeholders.",
        input_schema: FORMAT_TEMPLATE_INPUT_SCHEMA,
        output_schema: FORMAT_TEMPLATE_OUTPUT_SCHEMA,
        handler: format_template,
    },
    BuiltinDef {
        identifier: "json.parse",
        name: "JSON Parse",
        description: "Parse a JSON string into a structured value.",
        input_schema: JSON_PARSE_INPUT_SCHEMA,
        output_schema: JSON_PARSE_OUTPUT_SCHEMA,
        handler: json_parse,
    },
    BuiltinDef {
        identifier: "json.stringify",
        name: "JSON Stringify",
        description: "Serialize a value to a JSON string (optionally pretty-printed).",
        input_schema: JSON_STRINGIFY_INPUT_SCHEMA,
        output_schema: JSON_STRINGIFY_OUTPUT_SCHEMA,
        handler: json_stringify,
    },
    BuiltinDef {
        identifier: "text.regex_match",
        name: "Regex Match",
        description: "Apply a Rust-syntax regex against text and return matches with capture groups.",
        input_schema: TEXT_REGEX_MATCH_INPUT_SCHEMA,
        output_schema: TEXT_REGEX_MATCH_OUTPUT_SCHEMA,
        handler: text_regex_match,
    },
    BuiltinDef {
        identifier: "chat.respond",
        name: "Chat Respond",
        description: "Submit the final user-visible reply (signals orchestrator to end the turn).",
        input_schema: CHAT_RESPOND_INPUT_SCHEMA,
        output_schema: CHAT_RESPOND_OUTPUT_SCHEMA,
        handler: chat_respond,
    },
];

pub fn lookup(identifier: &str) -> Option<&'static BuiltinDef> {
    BUILTINS.iter().find(|b| b.identifier == identifier)
}

/// 启动期 idempotent upsert — INSERT IGNORE 兜底 (identifier UNIQUE)
pub async fn ensure_registered(
    pool: &sqlx::MySqlPool,
) -> Result<(), sqlx::Error> {
    for b in BUILTINS {
        sqlx::query(
            r#"INSERT INTO functions
               (identifier, name, description, kind, input_schema, output_schema, plugin_id, plugin_export)
               VALUES (?, ?, ?, 1, CAST(? AS JSON), CAST(? AS JSON), NULL, NULL)
               ON DUPLICATE KEY UPDATE
                 name = VALUES(name),
                 description = VALUES(description),
                 input_schema = VALUES(input_schema),
                 output_schema = VALUES(output_schema)"#,
        )
        .bind(b.identifier)
        .bind(b.name)
        .bind(b.description)
        .bind(b.input_schema)
        .bind(b.output_schema)
        .execute(pool)
        .await?;
    }
    tracing::info!(count = BUILTINS.len(), "builtin functions upserted");
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn format_template_basic() {
        let r = format_template(json!({
            "template": "Hello {name}, you are {age} years old",
            "vars": {"name": "Alice", "age": 30}
        }))
        .unwrap();
        assert_eq!(r, Value::String("Hello Alice, you are 30 years old".into()));
    }

    #[test]
    fn json_parse_roundtrip() {
        let r = json_parse(json!({"text": r#"{"a":1}"#})).unwrap();
        assert_eq!(r, json!({"a": 1}));
    }

    #[test]
    fn json_stringify_pretty() {
        let r = json_stringify(json!({"value": {"a": 1}, "pretty": true})).unwrap();
        let s = r.as_str().unwrap();
        assert!(s.contains("\n"));
    }

    #[test]
    fn regex_capture_groups() {
        let r = text_regex_match(json!({
            "text": "name=alice id=42",
            "pattern": r"(\w+)=(\w+)",
            "all": true
        }))
        .unwrap();
        let matches = r.get("matches").and_then(|m| m.as_array()).unwrap();
        assert_eq!(matches.len(), 2);
    }

    #[test]
    fn chat_respond_wraps() {
        let r = chat_respond(json!({"content": "final answer"})).unwrap();
        assert_eq!(r["final_content"], "final answer");
    }
}
