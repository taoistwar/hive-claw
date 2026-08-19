//! Extism plugin executor
//!
//! Loads and executes WASM plugins from local filesystem using extism runtime.

use extism::{
    CurrentPlugin, Error as ExtismError, Manifest, PluginBuilder, UserData, Val, ValType, Wasm,
};
use hive_runtime_core::wasm::HOST_CALL_IMPORT;
use std::collections::{HashMap, VecDeque};
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

/// Bytes in a single 64 KiB WASM linear-memory page.
pub const WASM_PAGE_BYTES: u64 = 64 * 1024;
/// Number of 64 KiB WASM pages in one MiB (`1024 KiB / 64 KiB = 16`).
pub const PAGES_PER_MIB: u64 = 16;

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

    /// Memory cap as a 64 KiB WASM linear-memory page count.
    ///
    /// The on-disk field is named `memory_limit_mb` (a schema-compat name),
    /// but its logical unit is MiB. The value is converted to 64 KiB WASM
    /// pages as `memory_mb × 16` before it reaches the Extism manifest
    /// (T074 / plugin-abi contract). Raw MiB values or byte counts must
    /// never be passed to the page parameter directly.
    pub fn memory_pages(&self) -> PageCount {
        PageCount::from_mib(self.memory_mb)
    }

    /// Timeout in milliseconds (`timeout_secs × 1000`). The cache key
    /// uses milliseconds per the plugin-abi contract.
    pub fn timeout_ms(&self) -> u64 {
        self.timeout_secs * 1000
    }
}

impl Default for PluginLimits {
    fn default() -> Self {
        Self::default_limits()
    }
}

/// A validated 64 KiB WASM linear-memory page count.
///
/// The only way to build a [`PageCount`] is [`PageCount::from_mib`], which
/// multiplies a MiB value by [`PAGES_PER_MIB`]. There is deliberately no
/// `From<u64>` or raw constructor: passing a MiB value or a byte count
/// straight into the page parameter is a T074 contract violation, so the
/// type system forces callers through the MiB→page conversion.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct PageCount(u64);

impl PageCount {
    /// Convert a MiB memory cap to a 64 KiB page count (`mib × 16`).
    ///
    /// Inputs:
    /// - `mib`: the memory cap in MiB (already validated by
    ///   [`PluginLimits::new`] to the `1..=HARD_MAX_MEMORY_MB` range).
    ///
    /// Outputs: the equivalent [`PageCount`].
    /// Error modes: total; `128 → 2048` and `512 → 8192` per the T074 spec.
    pub fn from_mib(mib: u64) -> Self {
        Self(mib * PAGES_PER_MIB)
    }

    /// The page count as a `u64`.
    pub fn pages(&self) -> u64 {
        self.0
    }
}

/// Stable, content-addressed identity for a pooled plugin instance.
///
/// Two instances are interchangeable only when *every* component matches.
/// Per the plugin-abi contract the full cache key is the 9-tuple:
/// `(artifact_sha256, abi_version, runtime, runtime_version, fuel_limit,
/// timeout_ms, memory_limit_mb, output_limit_bytes, capability_policy_hash)`.
/// Changing any single component (including a different resource quota or
/// Capability policy) must never reuse an old instance.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct InstanceCacheKey {
    artifact_sha256: String,
    abi_version: String,
    runtime: String,
    runtime_version: String,
    fuel_limit: Option<u64>,
    timeout_ms: u64,
    memory_limit_mb: u64,
    output_limit_bytes: u64,
    capability_policy_hash: String,
}

impl InstanceCacheKey {
    /// Construct an instance cache key from all nine components.
    #[allow(clippy::too_many_arguments)]
    pub fn new(
        artifact_sha256: impl Into<String>,
        abi_version: impl Into<String>,
        runtime: impl Into<String>,
        runtime_version: impl Into<String>,
        fuel_limit: Option<u64>,
        timeout_ms: u64,
        memory_limit_mb: u64,
        output_limit_bytes: u64,
        capability_policy_hash: impl Into<String>,
    ) -> Self {
        Self {
            artifact_sha256: artifact_sha256.into(),
            abi_version: abi_version.into(),
            runtime: runtime.into(),
            runtime_version: runtime_version.into(),
            fuel_limit,
            timeout_ms,
            memory_limit_mb,
            output_limit_bytes,
            capability_policy_hash: capability_policy_hash.into(),
        }
    }

    /// Compute the stable cache-key digest.
    ///
    /// Each component is length-prefixed (or, for fixed-width integers,
    /// big-endian) before hashing, so no two distinct tuples collide. The
    /// result is a SHA-256 hex string and is deterministic for a given tuple.
    pub fn cache_key(&self) -> String {
        use sha2::{Digest, Sha256};
        let mut buf: Vec<u8> = Vec::with_capacity(256);
        fn push_str(buf: &mut Vec<u8>, value: &str) {
            buf.extend_from_slice(&(value.len() as u64).to_be_bytes());
            buf.extend_from_slice(value.as_bytes());
        }
        push_str(&mut buf, &self.artifact_sha256);
        push_str(&mut buf, &self.abi_version);
        push_str(&mut buf, &self.runtime);
        push_str(&mut buf, &self.runtime_version);
        match self.fuel_limit {
            Some(fuel) => {
                buf.push(1);
                buf.extend_from_slice(&fuel.to_be_bytes());
            }
            None => buf.push(0),
        }
        buf.extend_from_slice(&self.timeout_ms.to_be_bytes());
        buf.extend_from_slice(&self.memory_limit_mb.to_be_bytes());
        buf.extend_from_slice(&self.output_limit_bytes.to_be_bytes());
        push_str(&mut buf, &self.capability_policy_hash);

        let mut hasher = Sha256::new();
        hasher.update(&buf);
        format!("{:x}", hasher.finalize())
    }

    /// Artifact SHA-256 (lower-case hex).
    pub fn artifact_sha256(&self) -> &str {
        &self.artifact_sha256
    }

    /// ABI version.
    pub fn abi_version(&self) -> &str {
        &self.abi_version
    }

    /// Runtime identifier (e.g. `extism`).
    pub fn runtime(&self) -> &str {
        &self.runtime
    }

    /// Runtime version (e.g. `1.30.0`).
    pub fn runtime_version(&self) -> &str {
        &self.runtime_version
    }

    /// Optional fuel (instruction budget) limit. `None` means disabled.
    pub fn fuel_limit(&self) -> Option<u64> {
        self.fuel_limit
    }

    /// Timeout in milliseconds.
    pub fn timeout_ms(&self) -> u64 {
        self.timeout_ms
    }

    /// Memory cap in MiB.
    pub fn memory_limit_mb(&self) -> u64 {
        self.memory_limit_mb
    }

    /// Output cap in bytes.
    pub fn output_limit_bytes(&self) -> u64 {
        self.output_limit_bytes
    }

    /// Capability policy hash.
    pub fn capability_policy_hash(&self) -> &str {
        &self.capability_policy_hash
    }
}

/// A bounded, LRU, content-addressed idle instance pool.
///
/// The pool is keyed by [`InstanceCacheKey`]. It holds at most `capacity`
/// instances globally and at most one instance per cache key (T074:
/// "全局最多 8 个、每个完整 cache key 最多 1 个"). On [`Self::insert`]
/// the least-recently-used instance is evicted when the global cap is
/// reached; on [`Self::get`] a hit is promoted to most-recently-used.
///
/// This container is value-type agnostic: the production executor layers
/// it over `extism` instances (which are `!Send`) inside a dedicated
/// `spawn_blocking` worker, keeping the container itself testable without
/// the Extism runtime.
#[derive(Debug)]
pub struct BoundedInstancePool<V> {
    capacity: usize,
    order: VecDeque<InstanceCacheKey>,
    slots: HashMap<InstanceCacheKey, V>,
}

impl<V> BoundedInstancePool<V> {
    /// Construct an empty pool with the given global capacity.
    pub fn new(capacity: usize) -> Self {
        Self {
            capacity,
            order: VecDeque::new(),
            slots: HashMap::new(),
        }
    }

    /// The global capacity (at most this many instances may be resident).
    pub fn capacity(&self) -> usize {
        self.capacity
    }

    /// Number of resident instances.
    pub fn len(&self) -> usize {
        self.slots.len()
    }

    /// Whether the pool is empty.
    pub fn is_empty(&self) -> bool {
        self.slots.is_empty()
    }

    /// Whether an instance is resident for `key`.
    pub fn contains_key(&self, key: &InstanceCacheKey) -> bool {
        self.slots.contains_key(key)
    }

    /// Borrow the instance for `key`, promoting it to most-recently-used.
    pub fn get(&mut self, key: &InstanceCacheKey) -> Option<&V> {
        if !self.slots.contains_key(key) {
            return None;
        }
        self.promote(key);
        self.slots.get(key)
    }

    /// Mutably borrow the instance for `key`, promoting it to MRU.
    pub fn get_mut(&mut self, key: &InstanceCacheKey) -> Option<&mut V> {
        if !self.slots.contains_key(key) {
            return None;
        }
        self.promote(key);
        self.slots.get_mut(key)
    }

    /// Insert (or replace) the instance for `key`, enforcing the global
    /// capacity and the one-instance-per-key invariant.
    ///
    /// Returns the evicted value if a least-recently-used victim had to be
    /// removed to make room, or if `capacity == 0` (nothing can be cached).
    pub fn insert(&mut self, key: InstanceCacheKey, value: V) -> Option<V> {
        if self.capacity == 0 {
            return Some(value);
        }
        if self.slots.contains_key(&key) {
            // Replace in place: no capacity change, still promote to MRU.
            self.promote(&key);
            self.slots.insert(key, value);
            return None;
        }
        let evicted = if self.slots.len() >= self.capacity {
            let victim = self.order.pop_front()?;
            self.order.retain(|k| k != &victim);
            self.slots.remove(&victim)
        } else {
            None
        };
        self.order.push_back(key.clone());
        self.slots.insert(key, value);
        evicted
    }

    /// Remove the instance for `key`, returning it if present.
    pub fn remove(&mut self, key: &InstanceCacheKey) -> Option<V> {
        let value = self.slots.remove(key)?;
        self.order.retain(|k| k != key);
        Some(value)
    }

    /// Move `key` to the most-recently-used position in the LRU order.
    fn promote(&mut self, key: &InstanceCacheKey) {
        self.order.retain(|k| k != key);
        self.order.push_back(key.clone());
    }
}

impl<V> Default for BoundedInstancePool<V> {
    fn default() -> Self {
        Self::new(POOL_CAPACITY)
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
        Self::execute_bytes(
            wasm_bytes,
            export_name,
            input_json,
            timeout,
            allowed_capabilities,
        )
        .await
    }

    /// Execute a plugin export after re-verifying the artifact SHA-256
    /// against the trusted record (T079: no byte may reach Extism before
    /// the loaded artifact is re-verified against the stored digest).
    ///
    /// # Arguments
    /// * `expected_sha256` - the trusted lower-case hex digest recorded at
    ///   import time.
    pub async fn execute_with_verified_artifact(
        wasm_path: &Path,
        export_name: &str,
        input_json: &str,
        timeout: Duration,
        allowed_capabilities: Vec<String>,
        expected_sha256: &str,
    ) -> Result<String, String> {
        let wasm_bytes = tokio::fs::read(wasm_path)
            .await
            .map_err(|e| format!("读取 WASM 文件失败: {e}"))?;
        let actual = sha256_hex(&wasm_bytes);
        if !actual.eq_ignore_ascii_case(expected_sha256) {
            return Err(format!(
                "WASM 制品校验失败：期望 SHA-256 {expected_sha256}，实际 {actual}"
            ));
        }
        Self::execute_bytes(
            wasm_bytes,
            export_name,
            input_json,
            timeout,
            allowed_capabilities,
        )
        .await
    }

    /// Execute already-loaded artifact bytes.
    async fn execute_bytes(
        wasm_bytes: Vec<u8>,
        export_name: &str,
        input_json: &str,
        timeout: Duration,
        allowed_capabilities: Vec<String>,
    ) -> Result<String, String> {
        let export_name = export_name.to_string();
        let input_json = input_json.to_string();
        let runtime = tokio::runtime::Handle::current();

        let execution = tokio::task::spawn_blocking(move || {
            let manifest = Manifest::new([Wasm::data(wasm_bytes)]).with_timeout(timeout);
            // T025R ③ / T082: sandbox must keep WASI disabled. Plugins that
            // import any WASI snapshot0/preview1 function (fd_write,
            // fd_read, proc_exit, …) are rejected at instantiation time;
            // only the `host_call` host import is exposed.
            let mut plugin = PluginBuilder::new(manifest)
                .with_wasi(false)
                .with_function(
                    HOST_CALL_IMPORT,
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

/// Compute the lower-case SHA-256 hex digest of a byte slice.
///
/// Used both to record the trusted digest at import time and to re-verify
/// the loaded artifact before execution (T079).
pub fn sha256_hex(bytes: &[u8]) -> String {
    use sha2::{Digest, Sha256};
    let mut hasher = Sha256::new();
    hasher.update(bytes);
    format!("{:x}", hasher.finalize())
}
