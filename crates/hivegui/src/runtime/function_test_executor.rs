//! Function test execution shared by the management UI.

use std::{collections::HashSet, path::PathBuf, time::Duration};

use hive_json_schema::CompiledJsonSchema;
use serde_json::Value;
use sqlx::SqlitePool;

use crate::datasource::entity_store::{Function, Plugin};
use crate::plugin::plugin_store::PluginStore;

use super::plugin_executor::{DEFAULT_MEMORY_MB, DEFAULT_OUTPUT_BYTES};
use super::{BuiltinExecutor, PluginExecutor};

/// Maximum duration of one function test execution.
pub const FUNCTION_TEST_TIMEOUT: Duration = Duration::from_secs(30);

/// Matches a capability name or description against a case-insensitive filter.
pub fn capability_matches_filter(name: &str, description: &str, filter: &str) -> bool {
    let filter = filter.trim().to_lowercase();
    filter.is_empty()
        || name.to_lowercase().contains(&filter)
        || description.to_lowercase().contains(&filter)
}

/// Resolves stored defaults against the currently available capability names.
pub fn resolve_test_capabilities(
    required_capabilities: Option<&str>,
    available_capabilities: &[String],
) -> HashSet<String> {
    let available = available_capabilities
        .iter()
        .map(String::as_str)
        .collect::<HashSet<_>>();

    required_capabilities
        .and_then(|json| serde_json::from_str::<Vec<String>>(json).ok())
        .unwrap_or_default()
        .into_iter()
        .filter(|name| available.contains(name.as_str()))
        .collect()
}

/// Pretty-prints JSON results while preserving non-JSON plugin output verbatim.
pub fn format_test_output(output: &str) -> String {
    serde_json::from_str::<Value>(output)
        .and_then(|value| serde_json::to_string_pretty(&value))
        .unwrap_or_else(|_| output.to_string())
}

/// Parse a Plugin `resource_limits` JSON object into `(memory_mb,
/// output_bytes)`, falling back to the T074 defaults on a missing or
/// malformed field.
pub fn parse_resource_limits(json: &str) -> (u64, u64) {
    #[derive(serde::Deserialize)]
    struct Limits {
        #[serde(default)]
        memory_limit_mb: u64,
        #[serde(default)]
        output_limit_bytes: u64,
    }
    let limits = serde_json::from_str::<Limits>(json).unwrap_or(Limits {
        memory_limit_mb: DEFAULT_MEMORY_MB,
        output_limit_bytes: DEFAULT_OUTPUT_BYTES,
    });
    let memory_mb = if limits.memory_limit_mb == 0 {
        DEFAULT_MEMORY_MB
    } else {
        limits.memory_limit_mb
    };
    let output_bytes = if limits.output_limit_bytes == 0 {
        DEFAULT_OUTPUT_BYTES
    } else {
        limits.output_limit_bytes
    };
    (memory_mb, output_bytes)
}

/// Executes Builtin and Custom Functions through constructor-owned local state.
pub struct FunctionTestExecutor {
    plugin_root: PathBuf,
    pool: SqlitePool,
}

impl FunctionTestExecutor {
    /// Construct the managed executor from the local Plugin root and Store pool.
    pub fn new(plugin_root: impl Into<PathBuf>, pool: SqlitePool) -> Self {
        Self {
            plugin_root: plugin_root.into(),
            pool,
        }
    }

    /// Execute with a default-deny, empty Capability snapshot.
    pub async fn execute(&self, function: &Function, input: Value) -> Result<String, String> {
        self.execute_with_capabilities(function, input, Vec::new())
            .await
    }

    /// Execute with an explicit Capability snapshot for this invocation.
    pub async fn execute_with_capabilities(
        &self,
        function: &Function,
        input: Value,
        allowed_capabilities: Vec<String>,
    ) -> Result<String, String> {
        if function.kind == "placeholder" {
            return Err("function_not_executable".to_string());
        }

        match function.kind.as_str() {
            "builtin" if !hive_builtins::BuiltinRegistry::is_pure_builtin(&function.identifier) => {
                return Err("not_found".to_string());
            }
            "builtin" | "custom" => {}
            _ => return Err("not_found".to_string()),
        }

        validate_function_schema(&function.input_schema, &input)
            .map_err(|_| "input_schema_mismatch".to_string())?;
        ensure_capabilities_allowed(function, &allowed_capabilities)?;

        let output = if function.kind == "builtin" {
            BuiltinExecutor::execute(&function.identifier, input)
                .map_err(|_| "input_schema_mismatch".to_string())?
        } else {
            self.execute_custom(function, input, allowed_capabilities)
                .await?
        };

        validate_function_schema(&function.output_schema, &output)
            .map_err(|_| "output_schema_mismatch".to_string())?;
        serde_json::to_string_pretty(&output).map_err(|_| "output_schema_mismatch".to_string())
    }

    async fn execute_custom(
        &self,
        function: &Function,
        input: Value,
        allowed_capabilities: Vec<String>,
    ) -> Result<Value, String> {
        let plugin_id = function
            .plugin_id
            .ok_or_else(|| "函数未关联插件".to_string())?;
        let export_name = function
            .plugin_export
            .as_deref()
            .ok_or_else(|| "函数未指定插件导出函数名".to_string())?;
        let plugin = Plugin::get(&self.pool, plugin_id)
            .await
            .map_err(|_| "plugin_missing".to_string())?
            .filter(|plugin| plugin.deleted_at.is_none())
            .ok_or_else(|| "plugin_missing".to_string())?;
        let input_json =
            serde_json::to_string(&input).map_err(|_| "input_schema_mismatch".to_string())?;
        let (memory_mb, output_bytes) = parse_resource_limits(&plugin.resource_limits);
        let store = PluginStore::new(self.pool.clone(), &self.plugin_root)
            .map_err(|_| "not_found".to_string())?;
        let executor = PluginExecutor::new_scoped(store, &self.plugin_root);
        let output = executor
            .execute_verified_key(
                plugin_id,
                &plugin.s3_key,
                export_name,
                &input_json,
                FUNCTION_TEST_TIMEOUT,
                allowed_capabilities,
                &plugin.sha256,
                memory_mb,
                output_bytes,
            )
            .await
            .map_err(|_| "not_found".to_string())?;
        serde_json::from_str(&output).map_err(|_| "output_schema_mismatch".to_string())
    }
}

fn validate_function_schema(schema: &str, value: &Value) -> Result<(), ()> {
    let schema: Value = serde_json::from_str(schema).map_err(|_| ())?;
    let compiled = CompiledJsonSchema::compile(&schema).map_err(|_| ())?;
    compiled.validate(value).map_err(|_| ())
}

fn ensure_capabilities_allowed(
    function: &Function,
    allowed_capabilities: &[String],
) -> Result<(), String> {
    let required = function
        .required_capabilities
        .as_deref()
        .map(serde_json::from_str::<Vec<String>>)
        .transpose()
        .map_err(|_| "capability_denied".to_string())?
        .unwrap_or_default();
    let allowed = allowed_capabilities
        .iter()
        .map(String::as_str)
        .collect::<HashSet<_>>();
    if required
        .iter()
        .any(|capability| !allowed.contains(capability.as_str()))
    {
        return Err("capability_denied".to_string());
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::{DEFAULT_MEMORY_MB, DEFAULT_OUTPUT_BYTES, parse_resource_limits};

    #[test]
    fn parse_resource_limits_reads_v1_fields() {
        let (memory_mb, output_bytes) =
            parse_resource_limits(r#"{"memory_limit_mb":256,"output_limit_bytes":20971520}"#);
        assert_eq!(memory_mb, 256);
        assert_eq!(output_bytes, 20 * 1024 * 1024);
    }

    #[test]
    fn parse_resource_limits_falls_back_to_defaults() {
        // Malformed JSON and zero values fall back to T074 defaults.
        let (memory_mb, output_bytes) = parse_resource_limits("not-json");
        assert_eq!(memory_mb, DEFAULT_MEMORY_MB);
        assert_eq!(output_bytes, DEFAULT_OUTPUT_BYTES);

        let (memory_mb, output_bytes) =
            parse_resource_limits(r#"{"memory_limit_mb":0,"output_limit_bytes":0}"#);
        assert_eq!(memory_mb, DEFAULT_MEMORY_MB);
        assert_eq!(output_bytes, DEFAULT_OUTPUT_BYTES);
    }
}
