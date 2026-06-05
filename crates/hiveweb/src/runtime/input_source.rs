//! Structured input source spec for workflow nodes.
//!
//! Each node's input fields are declared as a `InputSpec` — a map of
//! `field_name → InputSource`. The `InputSource` enum has three variants:
//!
//! - `Upstream`      — value comes from a peer node's `output.<field>`
//! - `Custom`        — literal value (any JSON)
//! - `AgentContext`  — value comes from the runtime `AgentContext` snapshot
//!
//! Stored in `workflow_nodes.node_config.input_mapping` (function_node) or
//! `workflow_nodes.node_config.variables` (generate_answer_node) as JSON.
//!
//! Reserved input keys (rejected at save time):
//! - `_agent_context`        — injected by `inject_agent_context_snapshot`
//! - `_agent_context_updates` — extracted by `apply_agent_context_updates`

use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::collections::BTreeMap;

/// Sub-categories of `AgentContext` that a node input may reference.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AgentContextCategory {
    /// `agent::context::UserInput` (raw_text, session_id, message_id, timestamp, metadata)
    UserInput,
    /// `Category::Entities` records (key → value)
    Entities,
    /// `Category::ToolResults` records (key → value)
    ToolResults,
    /// `Category::StateChanges` records (key → value)
    StateChanges,
    /// `agent::context::ExtensionContent` list (id → content fields)
    Extensions,
}

impl AgentContextCategory {
    /// Static whitelist of `UserInput` field names (validated at save time).
    /// `metadata.<key>` accesses pass through with any sub-key.
    pub const USER_INPUT_FIELDS: &'static [&'static str] = &[
        "raw_text",
        "session_id",
        "message_id",
        "timestamp",
        "metadata",
    ];

    pub fn is_valid(&self) -> bool {
        matches!(
            self,
            Self::UserInput
                | Self::Entities
                | Self::ToolResults
                | Self::StateChanges
                | Self::Extensions
        )
    }
}

/// Structured description of where a single input field's value comes from.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum InputSource {
    /// Value comes from another node's `output.<field>`. If `field` is None,
    /// the entire upstream output Value is used.
    Upstream {
        node_key: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        field: Option<String>,
    },
    /// Literal value (any JSON).
    Custom { value: Value },
    /// Value comes from `AgentContext`.
    AgentContext {
        category: AgentContextCategory,
        key: String,
        /// For `UserInput`: optional sub-key (e.g. `"actor_id"` → `metadata.actor_id`).
        /// For `Extensions`: the field name within the extension content.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        sub_key: Option<String>,
    },
}

/// Map of `input.<field>` → source. Stored as JSON in
/// `node_config.input_mapping` (function_node) or `node_config.variables`
/// (generate_answer_node).
pub type InputSpec = BTreeMap<String, InputSource>;

/// Input keys that the framework injects into the input map at runtime.
/// Users must NOT declare these as input fields.
pub const RESERVED_INPUT_KEYS: &[&str] = &["_agent_context", "_agent_context_updates"];

/// Parse a JSON value into an `InputSpec`. The value should be a JSON object
/// where each entry maps `field_name` → `InputSource`.
pub fn parse_input_spec(value: &Value) -> Result<InputSpec, String> {
    if value.is_null() {
        return Ok(BTreeMap::new());
    }
    let obj = value
        .as_object()
        .ok_or_else(|| "input spec must be a JSON object".to_string())?;
    let mut spec = BTreeMap::new();
    for (field, src) in obj {
        let parsed: InputSource = serde_json::from_value(src.clone())
            .map_err(|e| format!("field '{field}': invalid InputSource — {e}"))?;
        spec.insert(field.clone(), parsed);
    }
    Ok(spec)
}

/// Validate an `InputSpec` for structural and semantic correctness.
/// Catches reserved keys, illegal category, illegal user_input sub-key, etc.
pub fn validate_input_spec(spec: &InputSpec) -> Result<(), String> {
    for field in spec.keys() {
        if RESERVED_INPUT_KEYS.contains(&field.as_str()) {
            return Err(format!(
                "input field '{field}' is reserved by the runtime and cannot be declared"
            ));
        }
        if field.is_empty() {
            return Err("input field name cannot be empty".to_string());
        }
    }
    for (field, src) in spec {
        match src {
            InputSource::Upstream { node_key, .. } => {
                if node_key.is_empty() {
                    return Err(format!("field '{field}': upstream.node_key cannot be empty"));
                }
            }
            InputSource::Custom { .. } => { /* any value is fine */ }
            InputSource::AgentContext {
                category,
                key,
                sub_key,
            } => {
                if !category.is_valid() {
                    return Err(format!(
                        "field '{field}': agent_context.category {:?} is not valid",
                        category
                    ));
                }
                if key.is_empty() {
                    return Err(format!(
                        "field '{field}': agent_context.key cannot be empty"
                    ));
                }
                if matches!(category, AgentContextCategory::UserInput) {
                    // `key` must be a known UserInput field
                    if !AgentContextCategory::USER_INPUT_FIELDS.contains(&key.as_str()) {
                        return Err(format!(
                            "field '{field}': agent_context.user_input.key '{key}' is not a known field; expected one of {:?}",
                            AgentContextCategory::USER_INPUT_FIELDS
                        ));
                    }
                    // sub_key is optional; if present, only `metadata` allows it
                    if let Some(sk) = sub_key {
                        if sk.is_empty() {
                            return Err(format!(
                                "field '{field}': agent_context.sub_key cannot be empty when set"
                            ));
                        }
                        if key != "metadata" {
                            return Err(format!(
                                "field '{field}': agent_context.user_input.key '{key}' does not accept a sub_key; only 'metadata' does"
                            ));
                        }
                    }
                }
                if matches!(category, AgentContextCategory::Extensions) && sub_key.is_none() {
                    return Err(format!(
                        "field '{field}': agent_context.extensions requires both key (extension id) and sub_key (field name)"
                    ));
                }
            }
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn reserved_keys_rejected() {
        let mut spec = BTreeMap::new();
        spec.insert(
            "_agent_context".to_string(),
            InputSource::Custom {
                value: json!("x"),
            },
        );
        assert!(validate_input_spec(&spec).is_err());
    }

    #[test]
    fn user_input_metadata_with_sub_key_ok() {
        let mut spec = BTreeMap::new();
        spec.insert(
            "channel".to_string(),
            InputSource::AgentContext {
                category: AgentContextCategory::UserInput,
                key: "metadata".to_string(),
                sub_key: Some("channel".to_string()),
            },
        );
        assert!(validate_input_spec(&spec).is_ok());
    }

    #[test]
    fn user_input_raw_text_no_sub_key_ok() {
        let mut spec = BTreeMap::new();
        spec.insert(
            "raw_text".to_string(),
            InputSource::AgentContext {
                category: AgentContextCategory::UserInput,
                key: "raw_text".to_string(),
                sub_key: None,
            },
        );
        assert!(validate_input_spec(&spec).is_ok());
    }

    #[test]
    fn user_input_invalid_field_rejected() {
        let mut spec = BTreeMap::new();
        spec.insert(
            "x".to_string(),
            InputSource::AgentContext {
                category: AgentContextCategory::UserInput,
                key: "non_existent_field".to_string(),
                sub_key: None,
            },
        );
        assert!(validate_input_spec(&spec).is_err());
    }

    #[test]
    fn extensions_requires_sub_key() {
        let mut spec = BTreeMap::new();
        spec.insert(
            "x".to_string(),
            InputSource::AgentContext {
                category: AgentContextCategory::Extensions,
                key: "ext_1".to_string(),
                sub_key: None,
            },
        );
        assert!(validate_input_spec(&spec).is_err());
    }

    #[test]
    fn parse_spec_roundtrip() {
        let v = json!({
            "query": { "kind": "upstream", "node_key": "fn_1", "field": "text" },
            "static": { "kind": "custom", "value": 42 },
            "channel": { "kind": "agent_context", "category": "user_input", "key": "metadata", "sub_key": "channel" }
        });
        let spec = parse_input_spec(&v).unwrap();
        assert_eq!(spec.len(), 3);
        assert!(validate_input_spec(&spec).is_ok());
    }
}
