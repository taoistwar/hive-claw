//! `Tool` trait — agent capability (read files, run commands, ...).
//! Port of `nanobot.agent.tools.base`.

use async_trait::async_trait;
use serde_json::{Map, Value};
use thiserror::Error;

use super::schema::{resolve_json_schema_type, validate_json_schema_value};

/// Error surfaced by [`Tool::execute`]. Rendered by the registry as a
/// user-friendly `"Error: ..."` string.
#[derive(Debug, Error)]
pub enum ToolExecError {
    #[error("{0}")]
    InvalidParams(String),
    #[error("{0}")]
    Other(String),
}

/// Agent capability trait.
///
/// Implementations are registered with [`super::ToolRegistry`]. `execute`
/// receives already-cast parameters (see [`Tool::cast_params`]) and returns
/// either a plain string or a structured JSON payload.
#[async_trait]
pub trait Tool: Send + Sync {
    /// Get the name of the tool.
    fn name(&self) -> &str;
    /// Get the description of the tool.
    fn description(&self) -> String;
    /// Get the parameters schema for the tool.
    fn parameters(&self) -> Value;

    /// Whether this tool is side-effect free and safe to parallelize.
    fn read_only(&self) -> bool {
        false
    }
    /// Whether this tool must run alone even if concurrency is enabled.
    fn exclusive(&self) -> bool {
        false
    }
    /// Whether this tool can run alongside other concurrency-safe tools.
    fn concurrency_safe(&self) -> bool {
        self.read_only() && !self.exclusive()
    }

    /// --- Plugin metadata ---

    /// Config section key for this tool's settings.
    fn config_key(&self) -> &str {
        ""
    }
    /// Whether this tool should be auto-discovered.
    fn plugin_discoverable(&self) -> bool {
        true
    }
    /// Execution scopes (e.g. "core", "webui").
    fn scopes(&self) -> &[&str] {
        &["core"]
    }

    /// Execute the tool with the given parameters.
    async fn execute(&self, params: Value) -> Result<Value, ToolExecError>;

    /// Apply safe schema-driven casts before validation.
    fn cast_params(&self, params: Value) -> Value {
        let schema = self.parameters();
        let schema_type = schema
            .get("type")
            .and_then(|v| v.as_str())
            .unwrap_or("object");
        if schema_type != "object" {
            return params;
        }
        cast_object(&params, &schema)
    }

    /// Validate against JSON schema; empty list means valid.
    fn validate_params(&self, params: &Value) -> Vec<String> {
        if !params.is_object() {
            return vec![format!(
                "parameters must be an object, got {}",
                type_name(params)
            )];
        }
        let mut schema = self.parameters();
        if schema.get("type").and_then(|v| v.as_str()) != Some("object") {
            if let Some(obj) = schema.as_object_mut() {
                obj.insert("type".into(), Value::String("object".into()));
            }
        }
        validate_json_schema_value(params, &schema, "")
    }

    /// OpenAI function schema (for request payload).
    fn to_schema(&self) -> Value {
        serde_json::json!({
            "type": "function",
            "function": {
                "name": self.name(),
                "description": self.description(),
                "parameters": self.parameters(),
            },
        })
    }

    /// Whether this tool is enabled for the given context.
    /// Mirrors Python `Tool.enabled(ctx)`.
    fn enabled(&self, _ctx: &Value) -> bool {
        true
    }

    /// Create a new instance of this tool type from config.
    /// Mirrors Python `Tool.create(ctx)`.
    fn create_instance(&self, _ctx: &Value) -> Option<Box<dyn Tool>> {
        None
    }
}

fn type_name(val: &Value) -> &'static str {
    match val {
        Value::Null => "null",
        Value::Bool(_) => "bool",
        Value::Number(_) => "number",
        Value::String(_) => "string",
        Value::Array(_) => "list",
        Value::Object(_) => "dict",
    }
}

/// Cast an object value to the schema type.
/// 将对象值转换为模式类型。
fn cast_object(val: &Value, schema: &Value) -> Value {
    // 获取对象的属性。
    let Some(obj) = val.as_object() else {
        return val.clone();
    };
    let empty = Map::new();
    // 获取 schema 的属性。
    let props = schema
        .get("properties")
        .and_then(|v| v.as_object())
        .unwrap_or(&empty);
    let mut out = Map::new();
    for (k, v) in obj {
        if let Some(sub_schema) = props.get(k) {
            out.insert(k.clone(), cast_value(v, sub_schema));
        } else {
            out.insert(k.clone(), v.clone());
        }
    }
    Value::Object(out)
}

fn cast_value(val: &Value, schema: &Value) -> Value {
    let Some(ty_val) = schema.get("type") else {
        return val.clone();
    };
    let t = resolve_json_schema_type(ty_val);

    match t {
        Some("boolean") => {
            if let Some(b) = val.as_bool() {
                return Value::Bool(b);
            }
            if let Some(s) = val.as_str() {
                let low = s.to_ascii_lowercase();
                if matches!(low.as_str(), "true" | "1" | "yes") {
                    return Value::Bool(true);
                }
                if matches!(low.as_str(), "false" | "0" | "no") {
                    return Value::Bool(false);
                }
            }
            val.clone()
        }
        Some("integer") => {
            if val.is_i64() || val.is_u64() {
                return val.clone();
            }
            if let Some(s) = val.as_str() {
                if let Ok(n) = s.parse::<i64>() {
                    return Value::from(n);
                }
            }
            val.clone()
        }
        Some("number") => {
            if let Some(n) = val.as_f64() {
                return serde_json::Number::from_f64(n)
                    .map(Value::Number)
                    .unwrap_or_else(|| val.clone());
            }
            if let Some(s) = val.as_str() {
                if let Ok(n) = s.parse::<f64>() {
                    if let Some(num) = serde_json::Number::from_f64(n) {
                        return Value::Number(num);
                    }
                }
            }
            val.clone()
        }
        Some("string") => {
            if val.is_null() {
                return val.clone();
            }
            if val.is_string() {
                return val.clone();
            }
            Value::String(serde_json::to_string(val).unwrap_or_default())
        }
        Some("array") => {
            if let (Value::Array(items), Some(items_schema)) = (val, schema.get("items")) {
                return Value::Array(items.iter().map(|i| cast_value(i, items_schema)).collect());
            }
            val.clone()
        }
        Some("object") => {
            if val.is_object() {
                return cast_object(val, schema);
            }
            val.clone()
        }
        _ => val.clone(),
    }
}
