//! Extism plugin executor
//!
//! Loads and executes WASM plugins from local filesystem using extism runtime.

use extism::{
    CurrentPlugin, Error as ExtismError, Manifest, Plugin as ExtismPlugin, PluginBuilder, UserData,
    Val, ValType, Wasm,
};
use hive_runtime_core::wasm::HOST_CALL_IMPORT;
use hive_runtime_core::{
    abi::HIVE_EXTISM_ABI_V1,
    wasm::{ABI_VERSION_EXPORT, WasmModuleShape, WasmValidationError, validate_wasm_shape},
};
use std::collections::{HashMap, HashSet, VecDeque};
use std::{
    path::Path,
    sync::{LazyLock, Mutex, MutexGuard},
    time::Duration,
};
use thiserror::Error;

use crate::plugin::plugin_store::PluginStore;

use super::DesktopHostDispatcher;

/// Default per-plugin resource limits, per T074 spec.
pub const DEFAULT_TIMEOUT_SECS: u64 = 30;
/// Default memory ceiling per plugin instance, in MiB.
pub const DEFAULT_MEMORY_MB: u64 = 128;
/// Default output buffer cap per plugin call, in bytes (10 MiB).
pub const DEFAULT_OUTPUT_BYTES: u64 = 10 * 1024 * 1024;

/// Default fuel (Wasmtime instruction budget) per plugin call.
///
/// Aligned with HiveWeb's `PLUGIN_CALL_FUEL` default (`10_000_000_000`) so a
/// Plugin is subject to the same instruction budget on both hosts. Fuel is
/// not user-configurable per Plugin in HiveGUI (the resource-limit form only
/// surfaces timeout/memory/output), so the shared default is applied
/// unconditionally.
pub const DEFAULT_FUEL: u64 = 10_000_000_000;

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
/// This container is value-type agnostic. The production executor stores
/// Extism 1.30 instances here (that release is `Send + Sync`) and checks each
/// one out before moving it into a `spawn_blocking` call, keeping the
/// container itself testable without the Extism runtime.
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

/// Exact store-local ownership of a managed artifact execution.
///
/// The process-wide instance pool is intentionally shared by freshly-created
/// [`PluginExecutor`] values. A persisted `s3_key` alone is not a sufficient
/// owner id for legacy numeric keys because two independent stores can contain
/// the same relative key, so the canonical store root and Plugin id are part
/// of the runtime-reference identity. They are deliberately *not* part of the
/// nine-component instance cache key: identical verified bytes and execution
/// policy remain reusable across stores, while ownership is reassigned to the
/// current checkout before the instance becomes idle again.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
struct ManagedArtifactRef {
    scope: String,
    plugin_id: i64,
    artifact_key: String,
}

impl ManagedArtifactRef {
    fn new(plugin_root: &Path, plugin_id: i64, artifact_key: &str) -> Self {
        Self {
            scope: normalized_plugin_scope(plugin_root),
            plugin_id,
            artifact_key: artifact_key.to_string(),
        }
    }
}

/// One reusable Extism instance plus the host context shared with its
/// registered `host_call` function. The context is refreshed on every
/// checkout so a pooled instance never retains a stale Tokio runtime handle
/// or Capability policy.
struct PooledPlugin {
    plugin: ExtismPlugin,
    host_context: UserData<DesktopHostContext>,
    owner: Option<ManagedArtifactRef>,
}

/// Process-wide production pool state.
///
/// Extism 1.30's `Plugin` is `Send + Sync`, so the bounded idle LRU can be
/// shared directly instead of being stranded in a short-lived executor. No
/// plugin call runs while this mutex is held: checkout removes the instance,
/// and only a successful, healthy call returns it.
struct ProductionPool {
    idle: BoundedInstancePool<PooledPlugin>,
    active: HashMap<ManagedArtifactRef, usize>,
    artifact_generation: HashMap<ManagedArtifactRef, u64>,
    key_generation: HashMap<InstanceCacheKey, u64>,
}

impl ProductionPool {
    fn new() -> Self {
        Self {
            idle: BoundedInstancePool::new(POOL_CAPACITY),
            active: HashMap::new(),
            artifact_generation: HashMap::new(),
            key_generation: HashMap::new(),
        }
    }

    fn active_increment(&mut self, owner: Option<&ManagedArtifactRef>) {
        if let Some(owner) = owner {
            *self.active.entry(owner.clone()).or_default() += 1;
        }
    }

    fn active_decrement(&mut self, owner: Option<&ManagedArtifactRef>) {
        let Some(owner) = owner else {
            return;
        };
        let Some(count) = self.active.get_mut(owner) else {
            return;
        };
        *count = count.saturating_sub(1);
        if *count == 0 {
            self.active.remove(owner);
        }
    }

    fn artifact_generation(&self, owner: Option<&ManagedArtifactRef>) -> u64 {
        owner
            .and_then(|owner| self.artifact_generation.get(owner).copied())
            .unwrap_or_default()
    }

    fn key_generation(&self, key: &InstanceCacheKey) -> u64 {
        self.key_generation.get(key).copied().unwrap_or_default()
    }
}

static PRODUCTION_POOL: LazyLock<Mutex<ProductionPool>> =
    LazyLock::new(|| Mutex::new(ProductionPool::new()));

fn production_pool() -> MutexGuard<'static, ProductionPool> {
    PRODUCTION_POOL
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
}

/// Active checkout guard. Dropping it on any build/call/trap/output error
/// releases the active reference without returning the instance to the idle
/// pool. Successful completion explicitly calls [`Self::finish_healthy`].
struct ActiveInstance {
    key: InstanceCacheKey,
    owner: Option<ManagedArtifactRef>,
    artifact_generation: u64,
    key_generation: u64,
    pooled: Option<PooledPlugin>,
    released: bool,
}

impl ActiveInstance {
    /// Remove an idle value and record the active reference before the caller
    /// enters `spawn_blocking`.
    fn checkout(key: InstanceCacheKey, owner: Option<ManagedArtifactRef>) -> Self {
        let mut pool = production_pool();
        let pooled = pool.idle.remove(&key);
        let artifact_generation = pool.artifact_generation(owner.as_ref());
        let key_generation = pool.key_generation(&key);
        pool.active_increment(owner.as_ref());
        drop(pool);

        Self {
            key,
            owner,
            artifact_generation,
            key_generation,
            pooled,
            released: false,
        }
    }

    fn take_pooled(&mut self) -> Option<PooledPlugin> {
        self.pooled.take()
    }

    /// Return a healthy instance only if neither artifact invalidation nor a
    /// timeout/key invalidation happened while the call was in flight.
    fn finish_healthy(mut self, mut pooled: PooledPlugin) {
        let mut pool = production_pool();
        pool.active_decrement(self.owner.as_ref());
        let generation_is_current = pool.artifact_generation(self.owner.as_ref())
            == self.artifact_generation
            && pool.key_generation(&self.key) == self.key_generation;
        if generation_is_current {
            pooled.owner = self.owner.clone();
            let _ = pool.idle.insert(self.key.clone(), pooled);
        }
        self.released = true;
    }
}

impl Drop for ActiveInstance {
    fn drop(&mut self) {
        if self.released {
            return;
        }
        let mut pool = production_pool();
        pool.active_decrement(self.owner.as_ref());
    }
}

fn normalized_plugin_scope(plugin_root: &Path) -> String {
    let absolute = std::fs::canonicalize(plugin_root).unwrap_or_else(|_| {
        if plugin_root.is_absolute() {
            plugin_root.to_path_buf()
        } else {
            std::env::current_dir()
                .unwrap_or_default()
                .join(plugin_root)
        }
    });
    absolute.to_string_lossy().into_owned()
}

fn invalidate_cache_key(key: &InstanceCacheKey) {
    let mut pool = production_pool();
    let generation = pool.key_generation.entry(key.clone()).or_default();
    *generation = generation.saturating_add(1);
    let _ = pool.idle.remove(key);
}

fn invalidate_matching_artifacts(matches: impl Fn(&ManagedArtifactRef) -> bool) -> bool {
    let mut pool = production_pool();
    let mut owners = pool
        .artifact_generation
        .keys()
        .chain(pool.active.keys())
        .filter(|owner| matches(owner))
        .cloned()
        .collect::<HashSet<_>>();
    owners.extend(
        pool.idle
            .slots
            .values()
            .filter_map(|pooled| pooled.owner.as_ref())
            .filter(|owner| matches(owner))
            .cloned(),
    );

    for owner in &owners {
        let generation = pool.artifact_generation.entry(owner.clone()).or_default();
        *generation = generation.saturating_add(1);
    }
    let idle_keys = pool
        .idle
        .slots
        .iter()
        .filter_map(|(key, pooled)| {
            pooled
                .owner
                .as_ref()
                .filter(|owner| matches(owner))
                .map(|_| key.clone())
        })
        .collect::<Vec<_>>();
    for key in idle_keys {
        let _ = pool.idle.remove(&key);
    }

    pool.active
        .iter()
        .any(|(owner, count)| *count > 0 && matches(owner))
}

/// Evict idle instances for one exact store artifact and invalidate all
/// in-flight generations so a pre-delete/config-change call cannot reinsert
/// itself afterward. Returns `true` while an active call still references the
/// artifact.
pub(crate) fn invalidate_artifact_instances(
    plugin_root: &Path,
    plugin_id: i64,
    artifact_key: &str,
) -> bool {
    let owner = ManagedArtifactRef::new(plugin_root, plugin_id, artifact_key);
    invalidate_matching_artifacts(|candidate| candidate == &owner)
}

/// Same invalidation boundary for entity-store updates that know the Plugin
/// id and persisted key but do not own a PluginStore/root path. It is
/// intentionally conservative and evicts matching references in every
/// process-local store scope.
pub(crate) fn invalidate_artifact_instances_any_scope(plugin_id: i64, artifact_key: &str) -> bool {
    invalidate_matching_artifacts(|owner| {
        owner.plugin_id == plugin_id && owner.artifact_key == artifact_key
    })
}

/// Whether an exact managed artifact currently has an active call or owns an
/// idle pooled instance. GC uses this after invalidation to wait for active
/// checkouts to drain before unlinking the artifact.
pub(crate) fn artifact_has_runtime_references(
    plugin_root: &Path,
    plugin_id: i64,
    artifact_key: &str,
) -> bool {
    let owner = ManagedArtifactRef::new(plugin_root, plugin_id, artifact_key);
    let pool = production_pool();
    pool.active.get(&owner).copied().unwrap_or_default() > 0
        || pool
            .idle
            .slots
            .values()
            .any(|pooled| pooled.owner.as_ref() == Some(&owner))
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
    artifact_scope: Option<String>,
}

impl PluginExecutor {
    /// Construct an executor with the T074 default limits and the
    /// [`POOL_CAPACITY`] global idle pool.
    pub fn new(store: PluginStore) -> Self {
        Self {
            store,
            limits: PluginLimits::default_limits(),
            pool_capacity: POOL_CAPACITY,
            artifact_scope: None,
        }
    }

    /// Construct a managed executor whose pooled active/idle references are
    /// attributable to an exact plugin artifact root for protected GC.
    pub(crate) fn new_scoped(store: PluginStore, plugin_root: &Path) -> Self {
        Self {
            store,
            limits: PluginLimits::default_limits(),
            pool_capacity: POOL_CAPACITY,
            artifact_scope: Some(normalized_plugin_scope(plugin_root)),
        }
    }

    /// Construct an executor with explicit limits.
    pub fn with_limits(store: PluginStore, limits: PluginLimits) -> Self {
        Self {
            store,
            limits,
            pool_capacity: POOL_CAPACITY,
            artifact_scope: None,
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

    /// Execute a plugin export by reading the artifact through the
    /// controlled no-follow store handle and re-verifying its SHA-256
    /// before any byte reaches Extism (T079 ①).
    ///
    /// Unlike the path-based [`PluginExecutor::execute_with_verified_limits`],
    /// this opens the artifact exactly once via
    /// [`PluginStore::read_verified_artifact`] (`O_NOFOLLOW` + `fstat`,
    /// link-count-1 regular file) and re-derives the digest from those exact
    /// bytes. There is no check-then-reopen window between verification and
    /// execution: the verified bytes are the bytes handed to `Wasm::data`.
    ///
    /// # Arguments
    /// * `identifier` / `version` / `plugin_id` — the artifact identity
    ///   resolved through the store root (never a raw `s3_key` path).
    /// * `expected_sha256` — the trusted lower-case hex digest recorded at
    ///   import time.
    #[allow(clippy::too_many_arguments)]
    pub async fn execute_verified(
        &self,
        identifier: &str,
        version: &str,
        plugin_id: i64,
        export_name: &str,
        input_json: &str,
        timeout: Duration,
        allowed_capabilities: Vec<String>,
        expected_sha256: &str,
        memory_mb: u64,
        output_bytes: u64,
    ) -> Result<String, String> {
        let wasm_bytes = self
            .store
            .read_verified_artifact(identifier, version, plugin_id)
            .map_err(|e| format!("读取插件制品失败（{}），请重新上传关联插件", e.reason()))?;
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
            memory_mb,
            output_bytes,
        )
        .await
    }

    /// Execute a persisted opaque artifact key through the no-follow store
    /// boundary and associate its process-wide pool checkout with an exact
    /// `(store root, Plugin id, s3_key)` runtime reference.
    ///
    /// This is the production T079 path for operation-UUID keys. It keeps the
    /// legacy identifier/version/id API above available for older callers,
    /// while avoiding any attempt to reconstruct a key from the database id.
    #[allow(clippy::too_many_arguments)]
    pub(crate) async fn execute_verified_key(
        &self,
        plugin_id: i64,
        artifact_key: &str,
        export_name: &str,
        input_json: &str,
        timeout: Duration,
        allowed_capabilities: Vec<String>,
        expected_sha256: &str,
        memory_mb: u64,
        output_bytes: u64,
    ) -> Result<String, String> {
        let wasm_bytes = self
            .store
            .read_verified_artifact_key(artifact_key)
            .map_err(|e| format!("读取插件制品失败（{}），请重新上传关联插件", e.reason()))?;
        let actual = sha256_hex(&wasm_bytes);
        if !actual.eq_ignore_ascii_case(expected_sha256) {
            return Err(format!(
                "WASM 制品校验失败：期望 SHA-256 {expected_sha256}，实际 {actual}"
            ));
        }
        let scope = self
            .artifact_scope
            .clone()
            .ok_or_else(|| "插件执行器缺少制品作用域，无法建立受保护的运行时引用".to_string())?;
        let owner = ManagedArtifactRef {
            scope,
            plugin_id,
            artifact_key: artifact_key.to_string(),
        };
        Self::execute_bytes_with_owner(
            wasm_bytes,
            export_name,
            input_json,
            timeout,
            allowed_capabilities,
            memory_mb,
            output_bytes,
            Some(owner),
        )
        .await
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

    /// Execute a plugin export with the desktop host-call capability bridge
    /// and the default resource limits (30 s / 128 MiB / 10 MiB output).
    pub async fn execute_with_capabilities(
        wasm_path: &Path,
        export_name: &str,
        input_json: &str,
        timeout: Duration,
        allowed_capabilities: Vec<String>,
    ) -> Result<String, String> {
        Self::execute_with_limits(
            wasm_path,
            export_name,
            input_json,
            timeout,
            allowed_capabilities,
            DEFAULT_MEMORY_MB,
            DEFAULT_OUTPUT_BYTES,
        )
        .await
    }

    /// Execute a plugin export with explicit memory/output caps.
    ///
    /// `memory_mb` is converted to 64 KiB WASM pages (`memory_mb × 16`) before
    /// it reaches `Manifest::with_memory_max`; the fuel budget is the shared
    /// [`DEFAULT_FUEL`]; the output string is length-checked against
    /// `output_bytes` after the call returns (Extism has no built-in output
    /// cap). All three limits therefore match HiveWeb's enforcement.
    pub async fn execute_with_limits(
        wasm_path: &Path,
        export_name: &str,
        input_json: &str,
        timeout: Duration,
        allowed_capabilities: Vec<String>,
        memory_mb: u64,
        output_bytes: u64,
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
            memory_mb,
            output_bytes,
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
        Self::execute_with_verified_limits(
            wasm_path,
            export_name,
            input_json,
            timeout,
            allowed_capabilities,
            expected_sha256,
            DEFAULT_MEMORY_MB,
            DEFAULT_OUTPUT_BYTES,
        )
        .await
    }

    /// Execute a plugin export after re-verifying the artifact SHA-256 and
    /// applying explicit memory/output caps.
    pub async fn execute_with_verified_limits(
        wasm_path: &Path,
        export_name: &str,
        input_json: &str,
        timeout: Duration,
        allowed_capabilities: Vec<String>,
        expected_sha256: &str,
        memory_mb: u64,
        output_bytes: u64,
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
            memory_mb,
            output_bytes,
        )
        .await
    }

    /// Execute already-loaded artifact bytes with memory/fuel/output limits.
    async fn execute_bytes(
        wasm_bytes: Vec<u8>,
        export_name: &str,
        input_json: &str,
        timeout: Duration,
        allowed_capabilities: Vec<String>,
        memory_mb: u64,
        output_bytes: u64,
    ) -> Result<String, String> {
        Self::execute_bytes_with_owner(
            wasm_bytes,
            export_name,
            input_json,
            timeout,
            allowed_capabilities,
            memory_mb,
            output_bytes,
            None,
        )
        .await
    }

    #[allow(clippy::too_many_arguments)]
    async fn execute_bytes_with_owner(
        wasm_bytes: Vec<u8>,
        export_name: &str,
        input_json: &str,
        timeout: Duration,
        allowed_capabilities: Vec<String>,
        memory_mb: u64,
        output_bytes: u64,
        owner: Option<ManagedArtifactRef>,
    ) -> Result<String, String> {
        let validation = validate_wasm_shape(&wasm_bytes, false);
        if !validation.is_ok() {
            return Err(validation.to_string());
        }

        if timeout.is_zero() || timeout > Duration::from_secs(HARD_MAX_TIMEOUT_SECS) {
            return Err(format!(
                "插件超时上限无效: {} 秒（允许范围 0..={} 秒）",
                timeout.as_secs_f64(),
                HARD_MAX_TIMEOUT_SECS
            ));
        }
        if !(1..=HARD_MAX_MEMORY_MB).contains(&memory_mb) {
            return Err(format!(
                "内存上限无效: {memory_mb} MiB（允许范围 1..={HARD_MAX_MEMORY_MB} MiB）"
            ));
        }
        if !(1..=HARD_MAX_OUTPUT_BYTES).contains(&output_bytes) {
            return Err(format!(
                "输出上限无效: {output_bytes} 字节（允许范围 1..={HARD_MAX_OUTPUT_BYTES} 字节）"
            ));
        }

        // Convert MiB → 64 KiB WASM pages (T074: never pass MiB or bytes
        // directly to the page parameter). Clamp-out and zero are rejected.
        let memory_pages = memory_mb
            .checked_mul(PAGES_PER_MIB)
            .and_then(|pages| u32::try_from(pages).ok())
            .ok_or_else(|| format!("内存上限无效: {memory_mb} MiB"))?;

        let artifact_sha256 = sha256_hex(&wasm_bytes);
        let allowed_capabilities = normalized_capability_policy(allowed_capabilities);
        let capability_policy_hash = capability_policy_hash(&allowed_capabilities);
        let timeout_ms = u64::try_from(timeout.as_millis()).unwrap_or(u64::MAX);
        let key = InstanceCacheKey::new(
            artifact_sha256,
            HIVE_EXTISM_ABI_V1,
            "extism",
            extism::extism_version().trim_end_matches('\0'),
            Some(DEFAULT_FUEL),
            timeout_ms,
            memory_mb,
            output_bytes,
            capability_policy_hash,
        );
        // Checkout happens before `spawn_blocking`: an idle instance is never
        // concurrently callable, and GC sees the managed artifact as active
        // for the entire blocking build/call lifetime.
        let active = ActiveInstance::checkout(key.clone(), owner);
        let export_name = export_name.to_string();
        let input_json = input_json.to_string();
        let runtime = tokio::runtime::Handle::current();

        let execution = tokio::task::spawn_blocking(move || {
            let mut active = active;
            let mut pooled = if let Some(pooled) = active.take_pooled() {
                pooled
            } else {
                let manifest = Manifest::new([Wasm::data(wasm_bytes)])
                    .with_timeout(timeout)
                    .with_memory_max(memory_pages);
                // T025R ③ / T082: sandbox must keep WASI disabled. Plugins
                // importing WASI are rejected at instantiation; only the
                // `host_call` bridge is exposed.
                let host_context = UserData::new(DesktopHostContext {
                    runtime: runtime.clone(),
                    allowed_capabilities: allowed_capabilities.clone(),
                });
                let mut plugin = PluginBuilder::new(manifest)
                    .with_wasi(false)
                    .with_fuel_limit(DEFAULT_FUEL)
                    .with_function(
                        HOST_CALL_IMPORT,
                        [ValType::I64],
                        [ValType::I64],
                        host_context.clone(),
                        host_call,
                    )
                    .build()
                    .map_err(|e| format!("构建插件失败: {e}"))?;

                // Probe the mandatory ABI exactly once, when a new Extism
                // instance is built. Healthy pool hits skip this call.
                let abi_version = plugin
                    .call::<&str, String>(ABI_VERSION_EXPORT, "{}")
                    .map_err(|_| {
                        WasmValidationError::rejected(
                            WasmModuleShape::UnsupportedAbiVersion,
                            Some("abi version export could not be queried".to_string()),
                        )
                        .to_string()
                    })?;
                if abi_version != HIVE_EXTISM_ABI_V1 {
                    return Err(WasmValidationError::rejected(
                        WasmModuleShape::UnsupportedAbiVersion,
                        None,
                    )
                    .to_string());
                }

                PooledPlugin {
                    plugin,
                    host_context,
                    owner: None,
                }
            };

            // A process-wide pool can outlive both a short current-thread test
            // runtime and a Capability snapshot. Refresh both before *every*
            // business call, including pool hits.
            {
                let context = pooled
                    .host_context
                    .get()
                    .map_err(|e| format!("读取插件主机上下文失败: {e}"))?;
                let mut context = context
                    .lock()
                    .map_err(|_| "插件主机上下文锁已损坏".to_string())?;
                context.runtime = runtime;
                context.allowed_capabilities = allowed_capabilities;
            }

            let output = pooled
                .plugin
                .call::<&str, String>(&export_name, &input_json)
                .map_err(|e| {
                    let message = e.to_string();
                    let lower = message.to_ascii_lowercase();
                    if lower.contains("timeout") || lower.contains("fuel") {
                        "插件执行超时".to_string()
                    } else {
                        format!("插件执行失败: {e}")
                    }
                })?;

            if output.len() as u64 > output_bytes {
                return Err(format!(
                    "插件输出超过上限（{} 字节，上限 {} 字节）",
                    output.len(),
                    output_bytes
                ));
            }
            active.finish_healthy(pooled);
            Ok(output)
        });

        match tokio::time::timeout(timeout, execution).await {
            Err(_) => {
                // The blocking call may still be unwinding. Key generation
                // invalidation evicts any just-returned idle value and makes a
                // still-running checkout ineligible for reinsertion.
                invalidate_cache_key(&key);
                Err(format!("插件执行超时（{} 秒）", timeout.as_secs_f64()))
            }
            Ok(Err(e)) => Err(format!("插件执行任务失败: {e}")),
            Ok(Ok(result)) => result,
        }
    }
}

fn normalized_capability_policy(mut capabilities: Vec<String>) -> Vec<String> {
    capabilities.sort_unstable();
    capabilities.dedup();
    capabilities
}

fn capability_policy_hash(capabilities: &[String]) -> String {
    use sha2::{Digest, Sha256};
    let mut hasher = Sha256::new();
    for capability in capabilities {
        hasher.update((capability.len() as u64).to_be_bytes());
        hasher.update(capability.as_bytes());
    }
    format!("{:x}", hasher.finalize())
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
