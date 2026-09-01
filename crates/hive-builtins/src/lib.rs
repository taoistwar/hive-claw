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

/// Function pointer used by one pure Builtin definition.
pub type BuiltinHandler = fn(Value) -> BuiltinResult;

/// Complete immutable definition of a desktop-local pure Builtin.
#[derive(Debug, Clone, Copy)]
pub struct BuiltinDefinition {
    /// Stable underscore identifier persisted in `functions.identifier`.
    pub identifier: &'static str,
    /// User-visible name.
    pub name: &'static str,
    /// User-visible description.
    pub description: &'static str,
    /// Draft-7 input JSON Schema.
    pub input_schema: &'static str,
    /// Draft-7 output JSON Schema.
    pub output_schema: &'static str,
    /// Capabilities required by the handler.
    pub required_capabilities: &'static [&'static str],
    /// Pure local handler.
    pub handler: BuiltinHandler,
}

/// Single source of truth for every desktop-local pure Builtin.
pub const BUILTIN_DEFINITIONS: &[BuiltinDefinition] = &[
    BuiltinDefinition {
        identifier: "format_template",
        name: "Format Template",
        description: "Render a template string with named {var} placeholders.",
        input_schema: format_template::FORMAT_TEMPLATE_INPUT_SCHEMA,
        output_schema: format_template::FORMAT_TEMPLATE_OUTPUT_SCHEMA,
        required_capabilities: &[],
        handler: format_template::format_template,
    },
    BuiltinDefinition {
        identifier: "json_parse",
        name: "JSON Parse",
        description: "Parse a JSON string into a structured value.",
        input_schema: json_parse::JSON_PARSE_INPUT_SCHEMA,
        output_schema: json_parse::JSON_PARSE_OUTPUT_SCHEMA,
        required_capabilities: &[],
        handler: json_parse::json_parse,
    },
    BuiltinDefinition {
        identifier: "json_stringify",
        name: "JSON Stringify",
        description: "Serialize a value to a JSON string.",
        input_schema: json_stringify::JSON_STRINGIFY_INPUT_SCHEMA,
        output_schema: json_stringify::JSON_STRINGIFY_OUTPUT_SCHEMA,
        required_capabilities: &[],
        handler: json_stringify::json_stringify,
    },
    BuiltinDefinition {
        identifier: "text_regex_match",
        name: "Regex Match",
        description: "Apply a Rust-syntax regex and return matches with capture groups.",
        input_schema: text_regex_match::TEXT_REGEX_MATCH_INPUT_SCHEMA,
        output_schema: text_regex_match::TEXT_REGEX_MATCH_OUTPUT_SCHEMA,
        required_capabilities: &[],
        handler: text_regex_match::text_regex_match,
    },
];

/// Stable identifier for the template-formatting Builtin, derived from the
/// authoritative definition registry.
pub const FORMAT_TEMPLATE_IDENTIFIER: &str = BUILTIN_DEFINITIONS[0].identifier;
/// Stable identifier for the JSON-parsing Builtin, derived from the
/// authoritative definition registry.
pub const JSON_PARSE_IDENTIFIER: &str = BUILTIN_DEFINITIONS[1].identifier;
/// Stable identifier for the JSON-serialization Builtin, derived from the
/// authoritative definition registry.
pub const JSON_STRINGIFY_IDENTIFIER: &str = BUILTIN_DEFINITIONS[2].identifier;
/// Stable identifier for the regex-match Builtin, derived from the
/// authoritative definition registry.
pub const TEXT_REGEX_MATCH_IDENTIFIER: &str = BUILTIN_DEFINITIONS[3].identifier;

/// Exact reserved identifier set, derived from the sole definition registry.
/// Store/UI/runtime callers consume this list rather than copying identifiers.
pub const BUILTIN_IDENTIFIERS: &[&str] = &[
    FORMAT_TEMPLATE_IDENTIFIER,
    JSON_PARSE_IDENTIFIER,
    JSON_STRINGIFY_IDENTIFIER,
    TEXT_REGEX_MATCH_IDENTIFIER,
];

/// Registry of all pure computation builtins.
pub struct BuiltinRegistry;

impl BuiltinRegistry {
    /// Return the complete immutable registry.
    pub fn definitions() -> &'static [BuiltinDefinition] {
        BUILTIN_DEFINITIONS
    }

    /// Resolve one definition by byte-exact identifier.
    pub fn get(identifier: &str) -> Option<&'static BuiltinDefinition> {
        BUILTIN_DEFINITIONS
            .iter()
            .find(|definition| definition.identifier == identifier)
    }

    /// Execute a builtin function by identifier. Only the underscore
    /// identifiers are registered; the legacy dotted aliases must be
    /// rejected (T083 keeps zero dotted records/aliases).
    pub fn execute(identifier: &str, input: Value) -> BuiltinResult {
        match Self::get(identifier) {
            Some(definition) => (definition.handler)(input),
            None => Err(BuiltinError::BadArgs(format!(
                "Unknown pure builtin: {}",
                identifier
            ))),
        }
    }

    /// Check if a builtin identifier is a pure computation function.
    pub fn is_pure_builtin(identifier: &str) -> bool {
        Self::get(identifier).is_some()
    }
}
