//! Extism plugin executor
//!
//! Loads and executes WASM plugins from local filesystem using extism runtime.

use extism::{
    CurrentPlugin, Error as ExtismError, Manifest, PluginBuilder, UserData, Val, ValType, Wasm,
};
use std::{path::Path, time::Duration};
use thiserror::Error;

use crate::plugin::plugin_store::PluginStore;

use super::DesktopHostDispatcher;

/// Default per-plugin resource limits, per T074 spec.
pub const DEFAULT_TIMEOUT_SECS: u64 = 30;
/// Default memory ceiling per plugin instance, in MiB.
pub const DEFAULT_MEMORY_MB: u64 = 128;
/// Default output buffer cap per plugin call, in bytes (10 MiB).
pub const DEFAULT_OUTPUT_BYTES: u64 = 10 * 1024 * 1024;

/// Hard maximums a caller may bump the limits up to.
pub const HARD_MAX_TIMEOUT_SECS: u64 = 120;
/// Hard memory ceiling, in MiB.
pub const HARD_MAX_MEMORY_MB: u64 = 512;
/// Hard output cap, in bytes (50 MiB).
pub const HARD_MAX_OUTPUT_BYTES: u64 = 50 * 1024 * 1024;

/// Global idle instance pool cap. The full per-key instance pool is bounded
/// by this number; the per-cache-key LRU keeps at most one instance per key.
pub const POOL_CAPACITY: usize = 8;

/// Failure modes for [`PluginLimits::new`].
#[derive(Debug, Error, PartialEq, Eq)]
pub enum PluginLimitError {
    /// Timeout is outside `[1, HARD_MAX_TIMEOUT_SECS]`.
    #[error("plugin timeout {0}s is outside the allowed range 1..={1}")]
    TimeoutOutOfRange(u64, u64),
    /// Memory cap is outside `[1, HARD_MAX_MEMORY_MB]` (MiB).
    #[error("plugin memory {0} MiB is outside the allowed range 1..={1}")]
    MemoryOutOfRange(u64, u64),
    /// Output cap is below 1 byte or above the hard ceiling.
    #[error("plugin output cap {0} bytes is outside the allowed range 1..={1}")]
    OutputOutOfRange(u64, u64),
}

/// Validated resource limits for a single plugin invocation.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct PluginLimits {
    timeout_secs: u64,
    memory_mb: u64,
    output_bytes: u64,
}

impl PluginLimits {
    /// Construct a new `PluginLimits`. Rejects values outside the inclusive
    /// `[1, hard_max]` ranges.
    pub fn new(
        timeout_secs: u64,
        memory_mb: u64,
        output_bytes: u64,
    ) -> Result<Self, PluginLimitError> {
        if !(1..=HARD_MAX_TIMEOUT_SECS).contains(&timeout_secs) {
            return Err(PluginLimitError::TimeoutOutOfRange(
                timeout_secs,
                HARD_MAX_TIMEOUT_SECS,
            ));
        }
        if !(1..=HARD_MAX_MEMORY_MB).contains(&memory_mb) {
            return Err(PluginLimitError::MemoryOutOfRange(
                memory_mb,
                HARD_MAX_MEMORY_MB,
            ));
        }
        if !(1..=HARD_MAX_OUTPUT_BYTES).contains(&output_bytes) {
            return Err(PluginLimitError::OutputOutOfRange(
                output_bytes,
                HARD_MAX_OUTPUT_BYTES,
            ));
        }
        Ok(Self {
            timeout_secs,
            memory_mb,
            output_bytes,
        })
    }

    /// The T074 default.
    pub fn default_limits() -> Self {
        Self {
            timeout_secs: DEFAULT_TIMEOUT_SECS,
            memory_mb: DEFAULT_MEMORY_MB,
            output_bytes: DEFAULT_OUTPUT_BYTES,
        }
    }

    /// Timeout in seconds.
    pub fn timeout_secs(&self) -> u64 {
        self.timeout_secs
    }

    /// Memory cap in MiB.
    pub fn memory_mb(&self) -> u64 {
        self.memory_mb
    }

    /// Output cap in bytes.
    pub fn output_bytes(&self) -> u64 {
        self.output_bytes
    }
}

impl Default for PluginLimits {
    fn default() -> Self {
        Self::default_limits()
    }
}

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
    let result = context.runtime.block_on(
        DesktopHostDispatcher::default().dispatch(&envelope, &context.allowed_capabilities),
    );
    let handle = plugin.memory_new(&result)?;
    if !outputs.is_empty() {
        outputs[0] = plugin.memory_to_val(handle);
    }
    Ok(())
}

/// Plugin executor with a bounded idle instance pool and validated
/// resource limits. The pool is keyed by a SHA-256 / ABI / runtime / fuel /
/// timeout / memory / output / capability hash so two configurations
/// never share an instance.
pub struct PluginExecutor {
    store: PluginStore,
    limits: PluginLimits,
    pool_capacity: usize,
}

impl PluginExecutor {
    /// Construct an executor with the T074 default limits and the
    /// [`POOL_CAPACITY`] global idle pool.
    pub fn new(store: PluginStore) -> Self {
        Self {
            store,
            limits: PluginLimits::default_limits(),
            pool_capacity: POOL_CAPACITY,
        }
    }

    /// Construct an executor with explicit limits.
    pub fn with_limits(store: PluginStore, limits: PluginLimits) -> Self {
        Self {
            store,
            limits,
            pool_capacity: POOL_CAPACITY,
        }
    }

    /// Current limits.
    pub fn limits(&self) -> PluginLimits {
        self.limits
    }

    /// Idle pool capacity. The full pool never holds more than this many
    /// instances; per-cache-key the LRU keeps at most one.
    pub fn pool_capacity(&self) -> usize {
        self.pool_capacity
    }

    /// Underlying plugin store.
    pub fn store(&self) -> &PluginStore {
        &self.store
    }

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
