use serde::Deserialize;
use serde_json::Value;

use crate::{BuiltinError, BuiltinResult};

#[derive(Debug, Deserialize)]
struct FormatTemplateArgs {
    template: String,
    #[serde(default)]
    vars: serde_json::Map<String, Value>,
}

/// Simple `{var}` placeholder substitution; nested objects not currently supported.
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
    r#"{ "type": "string", "description": "Rendered template string" }"#;
