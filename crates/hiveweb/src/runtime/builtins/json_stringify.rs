use serde::Deserialize;
use serde_json::Value;

use super::{BuiltinContext, BuiltinError, BuiltinResult};

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
