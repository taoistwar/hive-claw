//! Extism plugin executor
//!
//! Loads and executes WASM plugins from local filesystem using extism runtime.

use extism::{
    CurrentPlugin, Error as ExtismError, Manifest, PluginBuilder, UserData, Val, ValType, Wasm,
};
use std::{path::Path, time::Duration};

use super::DesktopHostDispatcher;

#[derive(Clone)]
struct DesktopHostContext {
    runtime: tokio::runtime::Handle,
    allowed_capabilities: Vec<String>,
}

fn host_call(
    plugin: &mut CurrentPlugin,
    inputs: &[Val],
    outputs: &mut [Val],
    user_data: UserData<DesktopHostContext>,
) -> Result<(), ExtismError> {
    let envelope: String = plugin.memory_get_val(&inputs[0])?;
    let context = user_data.get()?;
    let context = context
        .lock()
        .map_err(|_| ExtismError::msg("desktop host context lock poisoned"))?
        .clone();
    let result = context.runtime.block_on(DesktopHostDispatcher::dispatch(
        &envelope,
        &context.allowed_capabilities,
    ));
    let handle = plugin.memory_new(&result)?;
    if !outputs.is_empty() {
        outputs[0] = plugin.memory_to_val(handle);
    }
    Ok(())
}

pub struct PluginExecutor;

impl PluginExecutor {
    /// Execute a plugin export function
    ///
    /// # Arguments
    /// * `wasm_path` - Path to the local WASM file
    /// * `export_name` - Name of the export function to call
    /// * `input_json` - JSON string input
    ///
    /// # Returns
    /// JSON string output from the plugin
    pub async fn execute(
        wasm_path: &Path,
        export_name: &str,
        input_json: &str,
    ) -> Result<String, String> {
        Self::execute_with_timeout(
            wasm_path,
            export_name,
            input_json,
            super::FUNCTION_TEST_TIMEOUT,
        )
        .await
    }

    /// Execute a plugin export with an Extism-enforced timeout.
    pub async fn execute_with_timeout(
        wasm_path: &Path,
        export_name: &str,
        input_json: &str,
        timeout: Duration,
    ) -> Result<String, String> {
        Self::execute_with_capabilities(wasm_path, export_name, input_json, timeout, Vec::new())
            .await
    }

    /// Execute a plugin export with the desktop host-call capability bridge.
    pub async fn execute_with_capabilities(
        wasm_path: &Path,
        export_name: &str,
        input_json: &str,
        timeout: Duration,
        allowed_capabilities: Vec<String>,
    ) -> Result<String, String> {
        let wasm_bytes = tokio::fs::read(wasm_path)
            .await
            .map_err(|e| format!("读取 WASM 文件失败: {e}"))?;
        let export_name = export_name.to_string();
        let input_json = input_json.to_string();
        let runtime = tokio::runtime::Handle::current();

        let execution = tokio::task::spawn_blocking(move || {
            let manifest = Manifest::new([Wasm::data(wasm_bytes)]).with_timeout(timeout);
            let mut plugin = PluginBuilder::new(manifest)
                .with_wasi(true)
                .with_function(
                    "host_call",
                    [ValType::I64],
                    [ValType::I64],
                    UserData::new(DesktopHostContext {
                        runtime,
                        allowed_capabilities,
                    }),
                    host_call,
                )
                .build()
                .map_err(|e| format!("构建插件失败: {e}"))?;

            plugin
                .call::<&str, String>(&export_name, &input_json)
                .map_err(|e| {
                    if e.to_string().contains("timeout") {
                        "插件执行超时".to_string()
                    } else {
                        format!("插件执行失败: {e}")
                    }
                })
        });

        match tokio::time::timeout(timeout, execution).await {
            Err(_) => Err(format!("插件执行超时（{} 秒）", timeout.as_secs_f64())),
            Ok(Err(e)) => Err(format!("插件执行任务失败: {e}")),
            Ok(Ok(result)) => result,
        }
    }
}
