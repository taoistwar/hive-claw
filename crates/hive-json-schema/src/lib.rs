//! Shared, fail-closed JSON Schema compilation and validation.
//!
//! The public error intentionally contains only stable categories. Neither an
//! invalid schema nor a rejected instance is retained in the error value or
//! rendered by [`std::fmt::Display`].

use std::fmt;
use std::sync::Arc;

use jsonschema::error::ValidationErrorKind;
use jsonschema::{Draft, JSONSchema, SchemaResolver, SchemaResolverError};
use serde_json::Value;
use url::Url;

/// A stable, non-leaking JSON Schema failure category.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
#[non_exhaustive]
pub enum JsonSchemaError {
    /// The caller's serialized JSON size limit was exceeded.
    TooLarge,
    /// The caller could not parse its serialized JSON input.
    InvalidJson,
    /// A schema passed to [`CompiledJsonSchema::compile`] was not an object.
    ObjectRequired,
    /// A `$ref` was not a local JSON Pointer (`#` or `#/...`).
    ExternalReferenceForbidden,
    /// Draft 7 compilation rejected the schema.
    InvalidSchema,
    /// A compiled schema rejected an instance.
    SchemaMismatch,
}

impl JsonSchemaError {
    /// Return the stable wire/log category for this error.
    #[must_use]
    pub const fn category(&self) -> &'static str {
        match self {
            Self::TooLarge => "too_large",
            Self::InvalidJson => "invalid_json",
            Self::ObjectRequired => "object_required",
            Self::ExternalReferenceForbidden => "external_reference_forbidden",
            Self::InvalidSchema => "invalid_schema",
            Self::SchemaMismatch => "schema_mismatch",
        }
    }
}

impl fmt::Display for JsonSchemaError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(self.category())
    }
}

impl std::error::Error for JsonSchemaError {}

/// A Draft 7 JSON Schema compiled for repeated validation.
pub struct CompiledJsonSchema {
    inner: JSONSchema,
}

impl fmt::Debug for CompiledJsonSchema {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("CompiledJsonSchema")
            .finish_non_exhaustive()
    }
}

impl CompiledJsonSchema {
    /// Compile an object schema as Draft 7.
    ///
    /// Only local JSON Pointer references (`#` and `#/...`) are accepted. A
    /// rejecting resolver is installed as a second fail-closed layer so HTTP,
    /// file, and other external resolution remains disabled even if another
    /// workspace dependency later enables resolver features on `jsonschema`.
    pub fn compile(schema: &Value) -> Result<Self, JsonSchemaError> {
        if !schema.is_object() {
            return Err(JsonSchemaError::ObjectRequired);
        }
        reject_non_draft_7_declaration(schema)?;
        reject_non_local_references(schema)?;

        let inner = JSONSchema::options()
            .with_draft(Draft::Draft7)
            .with_resolver(RejectExternalResolver)
            .compile(schema)
            .map_err(classify_compile_error)?;

        Ok(Self { inner })
    }

    /// Validate an instance without exposing the instance or validator detail.
    pub fn validate(&self, instance: &Value) -> Result<(), JsonSchemaError> {
        if self.inner.is_valid(instance) {
            Ok(())
        } else {
            Err(JsonSchemaError::SchemaMismatch)
        }
    }
}

fn reject_non_draft_7_declaration(schema: &Value) -> Result<(), JsonSchemaError> {
    let Some(declaration) = schema.get("$schema") else {
        return Ok(());
    };
    if matches!(
        declaration.as_str(),
        Some(
            "http://json-schema.org/draft-07/schema#"
                | "http://json-schema.org/draft-07/schema"
                | "https://json-schema.org/draft-07/schema#"
                | "https://json-schema.org/draft-07/schema"
        )
    ) {
        Ok(())
    } else {
        Err(JsonSchemaError::InvalidSchema)
    }
}

#[derive(Debug)]
struct RejectExternalResolver;

impl SchemaResolver for RejectExternalResolver {
    fn resolve(
        &self,
        _root_schema: &Value,
        _url: &Url,
        _original_reference: &str,
    ) -> Result<Arc<Value>, SchemaResolverError> {
        Err(std::io::Error::new(
            std::io::ErrorKind::PermissionDenied,
            JsonSchemaError::ExternalReferenceForbidden.category(),
        )
        .into())
    }
}

fn classify_compile_error(error: jsonschema::ValidationError<'_>) -> JsonSchemaError {
    match error.kind {
        ValidationErrorKind::FileNotFound { .. }
        | ValidationErrorKind::InvalidURL { .. }
        | ValidationErrorKind::JSONParse { .. }
        | ValidationErrorKind::Resolver { .. }
        | ValidationErrorKind::UnknownReferenceScheme { .. } => {
            JsonSchemaError::ExternalReferenceForbidden
        }
        _ => JsonSchemaError::InvalidSchema,
    }
}

fn reject_non_local_references(schema: &Value) -> Result<(), JsonSchemaError> {
    reject_non_local_references_in(schema, schema)
}

fn reject_non_local_references_in(
    root_schema: &Value,
    schema: &Value,
) -> Result<(), JsonSchemaError> {
    let Value::Object(object) = schema else {
        return Ok(());
    };

    if let Some(reference) = object.get("$ref") {
        let Some(reference) = reference.as_str() else {
            return Err(JsonSchemaError::InvalidSchema);
        };
        if reference != "#" && !reference.starts_with("#/") {
            return Err(JsonSchemaError::ExternalReferenceForbidden);
        }
        let target = if reference == "#" {
            Some(root_schema)
        } else {
            root_schema.pointer(&reference[1..])
        };
        if !matches!(target, Some(Value::Object(_) | Value::Bool(_))) {
            return Err(JsonSchemaError::InvalidSchema);
        }
    }

    for keyword in [
        "additionalItems",
        "additionalProperties",
        "contains",
        "else",
        "if",
        "items",
        "not",
        "propertyNames",
        "then",
        "unevaluatedItems",
        "unevaluatedProperties",
    ] {
        if let Some(child) = object.get(keyword) {
            match child {
                Value::Array(children) if keyword == "items" => {
                    for child in children {
                        reject_non_local_references_in(root_schema, child)?;
                    }
                }
                _ => reject_non_local_references_in(root_schema, child)?,
            }
        }
    }

    for keyword in ["allOf", "anyOf", "oneOf", "prefixItems"] {
        if let Some(children) = object.get(keyword).and_then(Value::as_array) {
            for child in children {
                reject_non_local_references_in(root_schema, child)?;
            }
        }
    }

    for keyword in [
        "$defs",
        "definitions",
        "dependentSchemas",
        "patternProperties",
        "properties",
    ] {
        if let Some(children) = object.get(keyword).and_then(Value::as_object) {
            for child in children.values() {
                reject_non_local_references_in(root_schema, child)?;
            }
        }
    }

    if let Some(dependencies) = object.get("dependencies").and_then(Value::as_object) {
        for dependency in dependencies.values() {
            if dependency.is_object() || dependency.is_boolean() {
                reject_non_local_references_in(root_schema, dependency)?;
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
    fn exposes_exact_stable_categories() {
        let categories = [
            JsonSchemaError::TooLarge,
            JsonSchemaError::InvalidJson,
            JsonSchemaError::ObjectRequired,
            JsonSchemaError::ExternalReferenceForbidden,
            JsonSchemaError::InvalidSchema,
            JsonSchemaError::SchemaMismatch,
        ]
        .map(|error| error.category());

        assert_eq!(
            categories,
            [
                "too_large",
                "invalid_json",
                "object_required",
                "external_reference_forbidden",
                "invalid_schema",
                "schema_mismatch",
            ]
        );
    }

    #[test]
    fn compiles_draft_7_and_validates_instances() {
        let schema = json!({
            "type": "object",
            "properties": {"count": {"type": "integer", "minimum": 1}},
            "required": ["count"],
            "additionalProperties": false
        });
        let compiled = CompiledJsonSchema::compile(&schema).expect("valid Draft 7 schema");

        assert_eq!(compiled.validate(&json!({"count": 2})), Ok(()));
        assert_eq!(
            compiled.validate(&json!({"count": 0})),
            Err(JsonSchemaError::SchemaMismatch)
        );
    }

    #[test]
    fn rejects_non_object_schema() {
        assert_eq!(
            CompiledJsonSchema::compile(&json!(true)).unwrap_err(),
            JsonSchemaError::ObjectRequired
        );
    }

    #[test]
    fn rejects_schema_invalid_under_forced_draft_7() {
        let schema = json!({
            "$schema": "http://json-schema.org/draft-04/schema#",
            "type": "object"
        });

        assert_eq!(
            CompiledJsonSchema::compile(&schema).unwrap_err(),
            JsonSchemaError::InvalidSchema
        );
    }

    #[test]
    fn dangling_local_pointer_is_invalid_schema() {
        let schema = json!({"$ref": "#/definitions/missing"});
        assert_eq!(
            CompiledJsonSchema::compile(&schema).unwrap_err(),
            JsonSchemaError::InvalidSchema
        );
    }

    #[test]
    fn local_json_pointer_reference_is_supported() {
        let schema = json!({
            "definitions": {"name": {"type": "string", "minLength": 1}},
            "properties": {"name": {"$ref": "#/definitions/name"}},
            "required": ["name"]
        });
        let compiled = CompiledJsonSchema::compile(&schema).expect("local reference");

        assert_eq!(compiled.validate(&json!({"name": "Hive"})), Ok(()));
        assert_eq!(
            compiled.validate(&json!({"name": ""})),
            Err(JsonSchemaError::SchemaMismatch)
        );
    }

    #[test]
    fn external_and_non_pointer_references_fail_closed() {
        for reference in [
            "https://example.invalid/schema.json",
            "http://example.invalid/schema.json",
            "file:///tmp/schema.json",
            "relative/schema.json",
            "#named-anchor",
        ] {
            let schema = json!({"$ref": reference});
            assert_eq!(
                CompiledJsonSchema::compile(&schema).unwrap_err(),
                JsonSchemaError::ExternalReferenceForbidden,
                "reference {reference:?}"
            );
        }
    }

    #[test]
    fn nested_external_reference_fails_closed() {
        let schema = json!({
            "properties": {
                "secret": {"$ref": "https://example.invalid/schema.json"}
            }
        });
        assert_eq!(
            CompiledJsonSchema::compile(&schema).unwrap_err(),
            JsonSchemaError::ExternalReferenceForbidden
        );
    }

    #[test]
    fn ref_shaped_instance_literal_is_not_treated_as_a_schema_reference() {
        let schema = json!({
            "enum": [{"$ref": "https://example.invalid/literal-not-a-schema"}]
        });
        let compiled = CompiledJsonSchema::compile(&schema).expect("valid enum schema");

        assert_eq!(
            compiled.validate(&json!({
                "$ref": "https://example.invalid/literal-not-a-schema"
            })),
            Ok(())
        );
    }

    #[test]
    fn errors_and_compiled_debug_do_not_leak_values() {
        let secret = "schema-and-instance-secret";
        let schema = json!({"type": "string", "const": "different"});
        let compiled = CompiledJsonSchema::compile(&schema).expect("valid schema");
        let error = compiled.validate(&json!(secret)).unwrap_err();

        assert_eq!(error.to_string(), "schema_mismatch");
        assert!(!error.to_string().contains(secret));
        assert!(!format!("{compiled:?}").contains("different"));
    }
}
