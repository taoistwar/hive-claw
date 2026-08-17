//! Pure computation builtin functions shared between hiveweb and hivegui.
//!
//! These functions don't require database access or external context,
//! making them suitable for direct execution in the desktop client.

pub mod format_template;
pub mod json_parse;
pub mod json_stringify;
pub mod text_regex_match;

use serde_json::Value;

#[derive(Debug, thiserror::Error)]
pub enum BuiltinError {
    #[error("invalid arguments: {0}")]
    BadArgs(String),
    #[error("execution failed: {0}")]
    Exec(String),
}

pub type BuiltinResult = Result<Value, BuiltinError>;

/// Registry of all pure computation builtins.
pub struct BuiltinRegistry;

impl BuiltinRegistry {
    /// Execute a builtin function by identifier. Only the underscore
    /// identifiers are registered; the legacy dotted aliases must be
    /// rejected (T083 keeps zero dotted records/aliases).
    pub fn execute(identifier: &str, input: Value) -> BuiltinResult {
        match identifier {
            "format_template" => format_template::format_template(input),
            "json_parse" => json_parse::json_parse(input),
            "json_stringify" => json_stringify::json_stringify(input),
            "text_regex_match" => text_regex_match::text_regex_match(input),
            _ => Err(BuiltinError::BadArgs(format!(
                "Unknown pure builtin: {}",
                identifier
            ))),
        }
    }

    /// Check if a builtin identifier is a pure computation function.
    pub fn is_pure_builtin(identifier: &str) -> bool {
        matches!(
            identifier,
            "format_template" | "json_parse" | "json_stringify" | "text_regex_match"
        )
    }
}
