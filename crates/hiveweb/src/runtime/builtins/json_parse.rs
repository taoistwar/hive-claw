use serde::Deserialize;
use serde_json::Value;

use super::{BuiltinContext, BuiltinError, BuiltinResult};

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
