//! JSON Schema validation for tool parameters. Port of
//! `nanobot.agent.tools.base.Schema.validate_json_schema_value`.

use serde_json::{Map, Value};

/// Resolve the non-null type from a JSON Schema `type` field which may be a
/// string (`"integer"`) or a union list (`["string","null"]`).
pub fn resolve_json_schema_type(t: &Value) -> Option<&str> {
    match t {
        Value::String(s) => Some(s.as_str()),
        Value::Array(items) => items
            .iter()
            .find_map(|v| v.as_str().filter(|s| *s != "null")),
        _ => None,
    }
}

/// Normalize a schema value: either a Schema trait object (`to_json_schema`)
/// or an already-prepared dict. Strings/raw values raise an error.
pub fn fragment_of(value: &Value) -> Option<&Map<String, Value>> {
    value.as_object()
}

fn subpath(path: &str, key: &str) -> String {
    if path.is_empty() {
        key.to_string()
    } else {
        format!("{path}.{key}")
    }
}

/// Validate `val` against a JSON Schema fragment; returns error messages
/// (empty list means the value is valid).
pub fn validate_json_schema_value(val: &Value, schema: &Value, path: &str) -> Vec<String> {
    let Some(schema_obj) = schema.as_object() else {
        return Vec::new();
    };

    let raw_type = schema_obj.get("type").cloned().unwrap_or(Value::Null);
    let nullable_field = matches!(raw_type, Value::Array(ref list) if list.iter().any(|v| v.as_str() == Some("null")))
        || schema_obj
            .get("nullable")
            .and_then(|v| v.as_bool())
            .unwrap_or(false);
    let t = resolve_json_schema_type(&raw_type).map(|s| s.to_string());
    let label = if path.is_empty() { "parameter" } else { path };

    if nullable_field && val.is_null() {
        return Vec::new();
    }

    let mut errors: Vec<String> = Vec::new();

    match t.as_deref() {
        Some("integer") => {
            if !is_integer(val) {
                return vec![format!("{label} should be integer")];
            }
        }
        Some("number") => {
            if !is_number_non_bool(val) {
                return vec![format!("{label} should be number")];
            }
        }
        Some("string") => {
            if !val.is_string() {
                return vec![format!("{label} should be string")];
            }
        }
        Some("boolean") => {
            if !val.is_boolean() {
                return vec![format!("{label} should be boolean")];
            }
        }
        Some("array") => {
            if !val.is_array() {
                return vec![format!("{label} should be array")];
            }
        }
        Some("object") if !val.is_object() => {
            return vec![format!("{label} should be object")];
        }
        _ => {}
    }

    if let Some(en) = schema_obj.get("enum").and_then(|v| v.as_array())
        && !en.iter().any(|v| v == val)
    {
        errors.push(format!(
            "{label} must be one of {}",
            serde_json::to_string(en).unwrap_or_default()
        ));
    }

    match t.as_deref() {
        Some("integer") | Some("number") => {
            if let Some(n) = val.as_f64() {
                if let Some(min) = schema_obj.get("minimum").and_then(|v| v.as_f64())
                    && n < min
                {
                    errors.push(format!("{label} must be >= {min}"));
                }
                if let Some(max) = schema_obj.get("maximum").and_then(|v| v.as_f64())
                    && n > max
                {
                    errors.push(format!("{label} must be <= {max}"));
                }
            }
        }
        Some("string") => {
            if let Some(s) = val.as_str() {
                if let Some(min) = schema_obj.get("minLength").and_then(|v| v.as_u64())
                    && (s.chars().count() as u64) < min
                {
                    errors.push(format!("{label} must be at least {min} chars"));
                }
                if let Some(max) = schema_obj.get("maxLength").and_then(|v| v.as_u64())
                    && (s.chars().count() as u64) > max
                {
                    errors.push(format!("{label} must be at most {max} chars"));
                }
            }
        }
        Some("object") => {
            if let Some(obj) = val.as_object() {
                let empty_props = Map::new();
                let props = schema_obj
                    .get("properties")
                    .and_then(|v| v.as_object())
                    .unwrap_or(&empty_props);
                if let Some(required) = schema_obj.get("required").and_then(|v| v.as_array()) {
                    for k in required {
                        if let Some(k) = k.as_str()
                            && !obj.contains_key(k)
                        {
                            errors.push(format!("missing required {}", subpath(path, k)));
                        }
                    }
                }
                for (k, v) in obj {
                    if let Some(sub_schema) = props.get(k) {
                        errors.extend(validate_json_schema_value(v, sub_schema, &subpath(path, k)));
                    }
                }
            }
        }
        Some("array") => {
            if let Some(arr) = val.as_array() {
                if let Some(min) = schema_obj.get("minItems").and_then(|v| v.as_u64())
                    && (arr.len() as u64) < min
                {
                    errors.push(format!("{label} must have at least {min} items"));
                }
                if let Some(max) = schema_obj.get("maxItems").and_then(|v| v.as_u64())
                    && (arr.len() as u64) > max
                {
                    errors.push(format!("{label} must be at most {max} items"));
                }
                if let Some(items_schema) = schema_obj.get("items") {
                    let prefix = if path.is_empty() {
                        "[{}]".to_string()
                    } else {
                        format!("{path}[{{}}]")
                    };
                    for (i, item) in arr.iter().enumerate() {
                        let child_path = prefix.replace("{}", &i.to_string());
                        errors.extend(validate_json_schema_value(item, items_schema, &child_path));
                    }
                }
            }
        }
        _ => {}
    }

    errors
}

fn is_integer(val: &Value) -> bool {
    match val {
        Value::Number(n) => n.is_i64() || n.is_u64(),
        _ => false,
    }
}

fn is_number_non_bool(val: &Value) -> bool {
    matches!(val, Value::Number(_))
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn type_mismatch_caught() {
        let errs = validate_json_schema_value(&json!("x"), &json!({"type":"integer"}), "");
        assert_eq!(errs, vec!["parameter should be integer".to_string()]);
    }

    #[test]
    fn nested_required_missing() {
        let schema = json!({
            "type": "object",
            "properties": {
                "a": {"type": "object", "properties": {"b": {"type": "string"}}, "required": ["b"]},
            },
            "required": ["a"],
        });
        let errs = validate_json_schema_value(&json!({"a": {}}), &schema, "");
        assert_eq!(errs, vec!["missing required a.b".to_string()]);
    }

    #[test]
    fn nullable_allows_null() {
        let schema = json!({"type": ["string", "null"]});
        let errs = validate_json_schema_value(&json!(null), &schema, "");
        assert!(errs.is_empty());
    }
}
