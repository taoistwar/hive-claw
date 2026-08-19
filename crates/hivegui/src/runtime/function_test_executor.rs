//! Function test execution shared by the management UI.

use std::{collections::HashSet, path::Path, time::Duration};

use serde_json::Value;

use crate::datasource::entity_store::{Function, Plugin};

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

/// Executes builtin and plugin-backed functions for the function test dialog.
pub struct FunctionTestExecutor;

impl FunctionTestExecutor {
    /// Executes `function` with `input`, returning a displayable output or actionable error.
    pub async fn execute(
        function: &Function,
        input: Value,
        base_dir: &Path,
    ) -> Result<String, String> {
        if function.kind == "placeholder" {
            return Err("占位函数没有可执行实现，仅用于 LLM 提示词调试".to_string());
        }

        let allowed_capabilities = function
            .required_capabilities
            .as_deref()
            .map(serde_json::from_str::<Vec<String>>)
            .transpose()
            .map_err(|e| format!("函数 Capability 配置无效: {e}"))?
            .unwrap_or_default();

        Self::execute_with_capabilities(function, input, base_dir, allowed_capabilities).await
    }

    /// Executes `function` with an explicit capability snapshot for one test run.
    pub async fn execute_with_capabilities(
        function: &Function,
        input: Value,
        base_dir: &Path,
        allowed_capabilities: Vec<String>,
    ) -> Result<String, String> {
        if function.kind == "placeholder" {
            return Err("占位函数没有可执行实现，仅用于 LLM 提示词调试".to_string());
        }

        if function.kind == "builtin" {
            return BuiltinExecutor::execute(&function.identifier, input).map(|output| {
                serde_json::to_string_pretty(&output).unwrap_or_else(|_| output.to_string())
            });
        }

        let plugin_id = function
            .plugin_id
            .ok_or_else(|| "函数未关联插件".to_string())?;
        let export_name = function
            .plugin_export
            .as_deref()
            .ok_or_else(|| "函数未指定插件导出函数名".to_string())?;
        let wasm_path = base_dir
            .join("plugins")
            .join(plugin_id.to_string())
            .join("plugin.wasm");

        if !wasm_path.exists() {
            return Err(format!(
                "WASM 文件不存在: {}。请重新上传关联插件",
                wasm_path.display()
            ));
        }

        let input_json =
            serde_json::to_string(&input).map_err(|e| format!("序列化输入失败: {e}"))?;

        PluginExecutor::execute_with_capabilities(
            &wasm_path,
            export_name,
            &input_json,
            FUNCTION_TEST_TIMEOUT,
            allowed_capabilities,
        )
        .await
    }

    /// Executes `function` with verification of the associated Plugin
    /// artifact against its trusted SHA-256 digest (T079). The digest is
    /// re-derived from the bytes that are about to be handed to Extism and
    /// compared to the recorded value before any byte reaches the runtime.
    pub async fn execute_with_verification(
        function: &Function,
        input: Value,
        base_dir: &Path,
        allowed_capabilities: Vec<String>,
        pool: &sqlx::Pool<sqlx::Sqlite>,
    ) -> Result<String, String> {
        if function.kind == "placeholder" {
            return Err("占位函数没有可执行实现，仅用于 LLM 提示词调试".to_string());
        }
        if function.kind == "builtin" {
            return BuiltinExecutor::execute(&function.identifier, input).map(|output| {
                serde_json::to_string_pretty(&output).unwrap_or_else(|_| output.to_string())
            });
        }

        let plugin_id = function
            .plugin_id
            .ok_or_else(|| "函数未关联插件".to_string())?;
        let export_name = function
            .plugin_export
            .as_deref()
            .ok_or_else(|| "函数未指定插件导出函数名".to_string())?;
        let wasm_path = base_dir
            .join("plugins")
            .join(plugin_id.to_string())
            .join("plugin.wasm");

        if !wasm_path.exists() {
            return Err(format!(
                "WASM 文件不存在: {}。请重新上传关联插件",
                wasm_path.display()
            ));
        }

        let plugin = Plugin::get(pool, plugin_id)
            .await
            .map_err(|e| format!("读取插件记录失败: {e}"))?
            .ok_or_else(|| "插件记录不存在，请重新导入".to_string())?;

        let input_json =
            serde_json::to_string(&input).map_err(|e| format!("序列化输入失败: {e}"))?;

        let (memory_mb, output_bytes) = parse_resource_limits(&plugin.resource_limits);

        PluginExecutor::execute_with_verified_limits(
            &wasm_path,
            export_name,
            &input_json,
            FUNCTION_TEST_TIMEOUT,
            allowed_capabilities,
            &plugin.sha256,
            memory_mb,
            output_bytes,
        )
        .await
    }
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
