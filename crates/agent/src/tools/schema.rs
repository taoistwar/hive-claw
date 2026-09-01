//! JSON Schema validation for tool parameters. Port of
//! `nanobot.agent.tools.base.Schema.validate_json_schema_value`.

use hive_json_schema::CompiledJsonSchema;
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

/// Validate `val` against a JSON Schema fragment; returns error messages
/// (empty list means the value is valid).
///
/// The existing public Agent API is preserved while validation delegates to
/// the workspace's single Draft 7 implementation. `path` remains accepted for
/// compatibility, but errors are now stable non-leaking categories.
pub fn validate_json_schema_value(val: &Value, schema: &Value, path: &str) -> Vec<String> {
    let _ = path;
    match CompiledJsonSchema::compile(schema).and_then(|compiled| compiled.validate(val)) {
        Ok(()) => Vec::new(),
        Err(error) => vec![error.category().to_owned()],
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn type_mismatch_caught() {
        let errs = validate_json_schema_value(&json!("x"), &json!({"type":"integer"}), "");
        assert_eq!(errs, vec!["schema_mismatch".to_string()]);
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
        assert_eq!(errs, vec!["schema_mismatch".to_string()]);
    }

    #[test]
    fn nullable_allows_null() {
        let schema = json!({"type": ["string", "null"]});
        let errs = validate_json_schema_value(&json!(null), &schema, "");
        assert!(errs.is_empty());
    }

    #[test]
    fn invalid_schema_uses_stable_category() {
        let errs = validate_json_schema_value(&json!({}), &json!({"required": true}), "secret");
        assert_eq!(errs, vec!["invalid_schema".to_string()]);
    }
}
