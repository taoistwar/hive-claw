//! Function test execution shared by the management UI.

use std::{collections::HashSet, path::Path, time::Duration};

use serde_json::Value;

use crate::datasource::entity_store::Function;

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

/// Executes builtin and plugin-backed functions for the function test dialog.
pub struct FunctionTestExecutor;

impl FunctionTestExecutor {
    /// Executes `function` with `input`, returning a displayable output or actionable error.
    pub async fn execute(
        function: &Function,
        input: Value,
        base_dir: &Path,
    ) -> Result<String, String> {
        if function.kind == 3 {
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
        if function.kind == 3 {
            return Err("占位函数没有可执行实现，仅用于 LLM 提示词调试".to_string());
        }

        if function.kind == 1 {
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
}
