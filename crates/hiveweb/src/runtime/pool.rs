//! Plugin Instance Pool（FR-029 / research §2 / US4 commit 2）
//!
//! 层级结构：`HashMap<PluginId, PluginPool>` + 内层 compiled-slot FIFO。
//! live Extism `Plugin` 永不复用；每次 checkout 都从 `CompiledPlugin` 创建
//! 全新的 Store / Instance，归还时仅缓存不可变编译产物。
//!
//! Pool 与 Extism 的边界：本模块负责"哪个 Plugin 有几个 idle / in_use 实例 +
//! 命中统计"；具体 WASM 编译与 host_call host_fn 注册在 `invoker`。

use aws_sdk_s3::Client as S3Client;
use extism::{CompiledPlugin, Plugin as ExtismPlugin};
use sha2::{Digest, Sha256};
use std::collections::{HashMap, VecDeque};
use std::future::Future;
use std::sync::Arc;
use std::time::{Duration, Instant};
use tokio::sync::oneshot;

use crate::models::Plugin as PluginRow;
use crate::storage::s3;

const DEFAULT_CALL_TIMEOUT_MS: u64 = 30_000;
const DEFAULT_CALL_FUEL: u64 = 10_000_000_000;

#[derive(Debug, Clone, Default, serde::Serialize)]
pub struct PoolMetrics {
    /// Fresh Store/Instance checkouts currently executing.
    pub in_use: u64,
    /// Reusable compiled-slot entries; these do not consume invocation permits.
    pub idle: u64,
    /// Successful fresh Store/Instance creations, including compiled-cache hits.
    pub created_total: u64,
    /// Successful actual Wasmtime compilations only.
    pub cache_misses: u64,
    /// Acquisitions that entered the FIFO wait queue.
    pub wait_count: u64,
    /// Response-compatibility field; fresh Store/Instance execution keeps it at zero.
    pub reset_failures: u64,
}

#[derive(Debug, Clone, Default, serde::Serialize)]
pub struct PerPluginMetrics {
    pub plugin_id: i64,
    pub identifier: String,
    pub version: String,
    pub in_use: u64,
    pub idle: u64,
    pub cache_misses: u64,
}

#[derive(Debug, Clone)]
pub struct PoolConfig {
    pub max_per_plugin: usize,
    pub max_total: usize,
    pub idle_timeout: Duration,
    pub acquire_timeout: Duration,
    pub call_timeout_ms: u64,
    pub call_memory_mb: u64,
    pub call_fuel: u64,
}

impl PoolConfig {
    fn parse_nonzero_u64(name: &str, value: Option<&str>, default: u64) -> Result<u64, String> {
        let Some(value) = value else {
            return Ok(default);
        };
        let parsed = value
            .parse::<u64>()
            .map_err(|_| format!("{name} must be a non-zero unsigned integer"))?;
        if parsed == 0 {
            return Err(format!("{name} must be greater than zero"));
        }
        Ok(parsed)
    }

    fn nonzero_u64_from_env(name: &str, default: u64) -> u64 {
        match std::env::var(name) {
            Ok(value) => Self::parse_nonzero_u64(name, Some(&value), default)
                .unwrap_or_else(|error| panic!("invalid Plugin runtime configuration: {error}")),
            Err(std::env::VarError::NotPresent) => default,
            Err(std::env::VarError::NotUnicode(_)) => {
                panic!("invalid Plugin runtime configuration: {name} must be valid UTF-8")
            }
        }
    }

    pub fn from_env() -> Self {
        fn env<T: std::str::FromStr>(name: &str, default: T) -> T {
            std::env::var(name)
                .ok()
                .and_then(|v| v.parse().ok())
                .unwrap_or(default)
        }
        Self {
            max_per_plugin: env("PLUGIN_POOL_MAX_PER_PLUGIN", 8),
            max_total: env("PLUGIN_POOL_MAX_TOTAL", 64),
            idle_timeout: Duration::from_secs(env("PLUGIN_POOL_IDLE_TIMEOUT_SEC", 600u64)),
            acquire_timeout: Duration::from_millis(env("PLUGIN_POOL_ACQUIRE_TIMEOUT_MS", 5000u64)),
            call_timeout_ms: Self::nonzero_u64_from_env(
                "PLUGIN_CALL_TIMEOUT_MS",
                DEFAULT_CALL_TIMEOUT_MS,
            ),
            call_memory_mb: env("PLUGIN_CALL_MAX_MEMORY_MB", 128u64),
            call_fuel: Self::nonzero_u64_from_env("PLUGIN_CALL_FUEL", DEFAULT_CALL_FUEL),
        }
    }
}

/// A checked-out invocation instance.
///
/// The live Extism `Plugin` is never returned to the idle queue. Only its
/// immutable compiled template is retained so every checkout gets a fresh
/// Store, Instance, globals, linear memory and tables.
pub struct PooledPlugin {
    pub plugin: ExtismPlugin,
    pub plugin_id: i64,
    compiled: Arc<CompiledPlugin>,
}

impl std::fmt::Debug for PooledPlugin {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("PooledPlugin")
            .field("plugin_id", &self.plugin_id)
            .finish()
    }
}

struct CompiledSlot {
    compiled: Arc<CompiledPlugin>,
    last_used: Instant,
}

impl std::fmt::Debug for CompiledSlot {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("CompiledSlot")
            .field("last_used", &self.last_used)
            .finish()
    }
}

#[derive(Default)]
pub struct PluginPool {
    pub plugin_id: i64,
    pub identifier: String,
    pub version: String,
    idle: VecDeque<CompiledSlot>,
    pub in_use: usize,
    reserved: usize,
    compiled: Option<Arc<CompiledPlugin>>,
    compiled_last_used: Option<Instant>,
    pub cache_misses: u64,
}

impl std::fmt::Debug for PluginPool {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("PluginPool")
            .field("plugin_id", &self.plugin_id)
            .field("identifier", &self.identifier)
            .field("version", &self.version)
            .field("idle", &self.idle.len())
            .field("in_use", &self.in_use)
            .field("reserved", &self.reserved)
            .field("has_compiled", &self.compiled.is_some())
            .field("compiled_last_used", &self.compiled_last_used)
            .field("cache_misses", &self.cache_misses)
            .finish()
    }
}

#[derive(Debug, Clone)]
struct PoolKey {
    plugin_id: i64,
    identifier: String,
    version: String,
}

impl PoolKey {
    fn from_row(row: &PluginRow) -> Self {
        Self {
            plugin_id: row.id,
            identifier: row.identifier.clone(),
            version: row.version.clone(),
        }
    }
}

struct Waiter {
    id: u64,
    key: PoolKey,
    sender: oneshot::Sender<AdmissionGuard>,
}

#[derive(Default)]
struct PoolState {
    pools: HashMap<i64, PluginPool>,
    metrics: PoolMetrics,
    waiters: VecDeque<Waiter>,
    next_waiter_id: u64,
}

struct PoolCore {
    state: std::sync::Mutex<PoolState>,
    max_per_plugin: usize,
    max_total: usize,
}

enum AdmissionKind {
    Idle {
        plugin_id: i64,
        slot: CompiledSlot,
    },
    Cold {
        plugin_id: i64,
        compiled: Option<Arc<CompiledPlugin>>,
    },
}

struct AdmissionGuard {
    core: Arc<PoolCore>,
    kind: Option<AdmissionKind>,
}

impl AdmissionGuard {
    fn compiled(&self) -> Option<Arc<CompiledPlugin>> {
        match self
            .kind
            .as_ref()
            .expect("admission guard must remain armed until checkout completes")
        {
            AdmissionKind::Idle { slot, .. } => Some(Arc::clone(&slot.compiled)),
            AdmissionKind::Cold { compiled, .. } => compiled.clone(),
        }
    }

    fn complete(mut self, compiled: Arc<CompiledPlugin>, plugin: ExtismPlugin) -> PooledPlugin {
        let kind = self
            .kind
            .take()
            .expect("admission guard must complete exactly once");
        let plugin_id = match kind {
            AdmissionKind::Idle { plugin_id, .. } => {
                self.core.commit_idle_checkout();
                plugin_id
            }
            AdmissionKind::Cold { plugin_id, .. } => {
                self.core.commit_cold(plugin_id, Arc::clone(&compiled));
                plugin_id
            }
        };
        PooledPlugin {
            plugin,
            plugin_id,
            compiled,
        }
    }
}

impl Drop for AdmissionGuard {
    fn drop(&mut self) {
        if let Some(kind) = self.kind.take() {
            self.core.rollback_admission(kind);
        }
    }
}

struct WaitRegistration {
    core: Arc<PoolCore>,
    waiter_id: u64,
    armed: bool,
}

impl WaitRegistration {
    fn disarm(&mut self) {
        self.armed = false;
    }
}

impl Drop for WaitRegistration {
    fn drop(&mut self) {
        if self.armed {
            self.core.cancel_waiter(self.waiter_id);
        }
    }
}

pub struct InstancePool {
    pub config: PoolConfig,
    core: Arc<PoolCore>,
}

impl std::fmt::Debug for InstancePool {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("InstancePool")
            .field("config", &self.config)
            .finish_non_exhaustive()
    }
}

impl PoolCore {
    fn lock_state(&self) -> std::sync::MutexGuard<'_, PoolState> {
        self.state
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
    }

    fn active_slots(state: &PoolState) -> usize {
        state
            .pools
            .values()
            .map(|pool| pool.in_use + pool.reserved)
            .sum()
    }

    fn insert_pool<'a>(state: &'a mut PoolState, key: &PoolKey) -> &'a mut PluginPool {
        state
            .pools
            .entry(key.plugin_id)
            .or_insert_with(|| PluginPool {
                plugin_id: key.plugin_id,
                identifier: key.identifier.clone(),
                version: key.version.clone(),
                ..PluginPool::default()
            })
    }

    fn try_admit_locked(&self, state: &mut PoolState, key: &PoolKey) -> Option<AdmissionKind> {
        let plugin_slots = state
            .pools
            .get(&key.plugin_id)
            .map(|pool| pool.in_use + pool.reserved)
            .unwrap_or_default();
        if plugin_slots >= self.max_per_plugin || Self::active_slots(state) >= self.max_total {
            return None;
        }

        if state
            .pools
            .get(&key.plugin_id)
            .is_some_and(|pool| !pool.idle.is_empty())
        {
            let pool = state
                .pools
                .get_mut(&key.plugin_id)
                .expect("idle Plugin pool must remain present");
            let slot = pool
                .idle
                .pop_front()
                .expect("idle Plugin pool must contain a slot");
            pool.in_use += 1;
            pool.compiled_last_used = Some(Instant::now());
            state.metrics.in_use += 1;
            state.metrics.idle = state.metrics.idle.saturating_sub(1);
            return Some(AdmissionKind::Idle {
                plugin_id: key.plugin_id,
                slot,
            });
        }

        let pool = Self::insert_pool(state, key);
        pool.reserved += 1;
        Some(AdmissionKind::Cold {
            plugin_id: key.plugin_id,
            compiled: pool.compiled.clone(),
        })
    }

    fn rollback_locked(state: &mut PoolState, kind: AdmissionKind) {
        match kind {
            AdmissionKind::Idle { plugin_id, slot } => {
                if let Some(pool) = state.pools.get_mut(&plugin_id) {
                    pool.in_use = pool.in_use.saturating_sub(1);
                    pool.idle.push_front(slot);
                }
                state.metrics.in_use = state.metrics.in_use.saturating_sub(1);
                state.metrics.idle += 1;
            }
            AdmissionKind::Cold { plugin_id, .. } => {
                let remove_empty = if let Some(pool) = state.pools.get_mut(&plugin_id) {
                    pool.reserved = pool.reserved.saturating_sub(1);
                    pool.reserved == 0
                        && pool.in_use == 0
                        && pool.idle.is_empty()
                        && pool.compiled.is_none()
                        && pool.cache_misses == 0
                } else {
                    false
                };
                if remove_empty {
                    state.pools.remove(&plugin_id);
                }
            }
        }
    }

    fn drive_waiters_locked(self: &Arc<Self>, state: &mut PoolState) {
        while let Some(key) = state.waiters.front().map(|waiter| waiter.key.clone()) {
            let Some(kind) = self.try_admit_locked(state, &key) else {
                break;
            };
            let waiter = state
                .waiters
                .pop_front()
                .expect("front waiter must remain queued");
            let guard = AdmissionGuard {
                core: Arc::clone(self),
                kind: Some(kind),
            };
            if let Err(mut rejected) = waiter.sender.send(guard) {
                let kind = rejected
                    .kind
                    .take()
                    .expect("rejected admission must remain armed");
                Self::rollback_locked(state, kind);
            }
        }
    }

    fn rollback_admission(self: &Arc<Self>, kind: AdmissionKind) {
        let mut state = self.lock_state();
        Self::rollback_locked(&mut state, kind);
        self.drive_waiters_locked(&mut state);
    }

    fn cancel_waiter(self: &Arc<Self>, waiter_id: u64) {
        let mut state = self.lock_state();
        if let Some(position) = state
            .waiters
            .iter()
            .position(|waiter| waiter.id == waiter_id)
        {
            state.waiters.remove(position);
            self.drive_waiters_locked(&mut state);
        }
    }

    fn commit_cold(self: &Arc<Self>, plugin_id: i64, compiled: Arc<CompiledPlugin>) {
        let mut state = self.lock_state();
        let pool = state
            .pools
            .get_mut(&plugin_id)
            .expect("reserved Plugin pool must remain present");
        pool.reserved = pool.reserved.saturating_sub(1);
        pool.in_use += 1;
        pool.compiled = Some(compiled);
        pool.compiled_last_used = Some(Instant::now());
        // End the per-Plugin mutable borrow before updating global metrics.
        let _ = pool;
        state.metrics.in_use += 1;
        state.metrics.created_total += 1;
    }

    fn commit_idle_checkout(&self) {
        self.lock_state().metrics.created_total += 1;
    }

    fn record_compile_success(&self, plugin_id: i64, compiled: Arc<CompiledPlugin>) {
        let mut state = self.lock_state();
        let pool = state
            .pools
            .get_mut(&plugin_id)
            .expect("successful compilation must retain its reserved Plugin pool");
        pool.compiled = Some(compiled);
        pool.compiled_last_used = Some(Instant::now());
        pool.cache_misses += 1;
        let _ = pool;
        state.metrics.cache_misses += 1;
    }
}

impl InstancePool {
    pub fn new(config: PoolConfig) -> Arc<Self> {
        Arc::new(Self {
            core: Arc::new(PoolCore {
                state: std::sync::Mutex::new(PoolState::default()),
                max_per_plugin: config.max_per_plugin,
                max_total: config.max_total,
            }),
            config,
        })
    }

    pub async fn metrics_snapshot(&self) -> PoolMetrics {
        self.core.lock_state().metrics.clone()
    }

    pub async fn per_plugin_snapshot(&self) -> Vec<PerPluginMetrics> {
        let state = self.core.lock_state();
        let mut snapshot: Vec<_> = state
            .pools
            .values()
            .map(|pool| PerPluginMetrics {
                plugin_id: pool.plugin_id,
                identifier: pool.identifier.clone(),
                version: pool.version.clone(),
                in_use: pool.in_use as u64,
                idle: pool.idle.len() as u64,
                cache_misses: pool.cache_misses,
            })
            .collect();
        snapshot.sort_by_key(|pool| pool.plugin_id);
        snapshot
    }

    #[cfg(test)]
    pub(crate) fn seed_in_use_for_test(&self, plugin_id: i64, identifier: &str, version: &str) {
        let mut state = self.core.lock_state();
        let pool = state.pools.entry(plugin_id).or_insert_with(|| PluginPool {
            plugin_id,
            identifier: identifier.into(),
            version: version.into(),
            ..PluginPool::default()
        });
        pool.in_use += 1;
        state.metrics.in_use += 1;
    }

    async fn reserve(&self, key: PoolKey) -> Result<AdmissionGuard, PoolError> {
        let receiver;
        let waiter_id;
        {
            let mut state = self.core.lock_state();
            if state.waiters.is_empty()
                && let Some(kind) = self.core.try_admit_locked(&mut state, &key)
            {
                return Ok(AdmissionGuard {
                    core: Arc::clone(&self.core),
                    kind: Some(kind),
                });
            }

            let (sender, rx) = oneshot::channel();
            receiver = rx;
            state.next_waiter_id = state.next_waiter_id.wrapping_add(1);
            waiter_id = state.next_waiter_id;
            state.metrics.wait_count += 1;
            state.waiters.push_back(Waiter {
                id: waiter_id,
                key,
                sender,
            });
            self.core.drive_waiters_locked(&mut state);
        }

        let mut registration = WaitRegistration {
            core: Arc::clone(&self.core),
            waiter_id,
            armed: true,
        };
        match tokio::time::timeout(self.config.acquire_timeout, receiver).await {
            Ok(Ok(admission)) => {
                registration.disarm();
                Ok(admission)
            }
            Ok(Err(_)) | Err(_) => Err(PoolError::Busy),
        }
    }

    /// Return a checkout. The live Store/Instance is always dropped; only the
    /// immutable compiled template can become idle.
    pub async fn release(&self, inst: PooledPlugin, reset_ok: bool, invocation_ok: bool) {
        let PooledPlugin {
            plugin,
            plugin_id,
            compiled,
        } = inst;
        drop(plugin);

        let mut state = self.core.lock_state();
        if state.pools.contains_key(&plugin_id) {
            state.metrics.in_use = state.metrics.in_use.saturating_sub(1);
            if !reset_ok {
                state.metrics.reset_failures += 1;
            }
            let pool = state
                .pools
                .get_mut(&plugin_id)
                .expect("Plugin pool existence was checked while holding the lock");
            pool.in_use = pool.in_use.saturating_sub(1);
            let returned_idle = reset_ok && invocation_ok;
            if reset_ok && invocation_ok {
                pool.idle.push_back(CompiledSlot {
                    compiled,
                    last_used: Instant::now(),
                });
                pool.compiled_last_used = Some(Instant::now());
            } else {
                if pool.in_use == 0 && pool.reserved == 0 && pool.idle.is_empty() {
                    pool.compiled = None;
                    pool.compiled_last_used = None;
                }
            }
            let _ = pool;
            if returned_idle {
                state.metrics.idle += 1;
            }
            self.core.drive_waiters_locked(&mut state);
        }
    }

    fn discard_lost_now(&self, plugin_id: i64) {
        let mut state = self.core.lock_state();
        if state.pools.contains_key(&plugin_id) {
            state.metrics.in_use = state.metrics.in_use.saturating_sub(1);
            let pool = state
                .pools
                .get_mut(&plugin_id)
                .expect("Plugin pool existence was checked while holding the lock");
            pool.in_use = pool.in_use.saturating_sub(1);
            if pool.in_use == 0 && pool.reserved == 0 && pool.idle.is_empty() {
                pool.compiled = None;
                pool.compiled_last_used = None;
            }
            self.core.drive_waiters_locked(&mut state);
        }
    }

    /// Reconcile a panicked/cancelled invocation without returning a live
    /// instance or compiled capacity slot to idle.
    pub async fn discard_lost(&self, plugin_id: i64) {
        self.discard_lost_now(plugin_id);
    }

    fn reap_idle_at(&self, now: Instant) -> usize {
        let mut state = self.core.lock_state();
        let mut removed = 0usize;
        for pool in state.pools.values_mut() {
            let before = pool.idle.len();
            pool.idle.retain(|slot| {
                now.saturating_duration_since(slot.last_used) < self.config.idle_timeout
            });
            removed += before - pool.idle.len();
            let cache_expired = pool.compiled_last_used.is_some_and(|last_used| {
                now.saturating_duration_since(last_used) >= self.config.idle_timeout
            });
            if pool.in_use == 0 && pool.reserved == 0 && cache_expired {
                pool.compiled = None;
                pool.compiled_last_used = None;
            }
        }
        state.metrics.idle = state.metrics.idle.saturating_sub(removed as u64);
        self.core.drive_waiters_locked(&mut state);
        removed
    }

    /// Start a weakly-owned reaper tied to the Tokio/HiveWeb runtime. Dropping
    /// the last pool handle lets the task exit without an ownership cycle.
    pub fn start_idle_reaper(self: &Arc<Self>) -> tokio::task::JoinHandle<()> {
        let weak = Arc::downgrade(self);
        let interval = self
            .config
            .idle_timeout
            .min(Duration::from_secs(60))
            .max(Duration::from_millis(1));
        tokio::spawn(async move {
            loop {
                tokio::time::sleep(interval).await;
                let Some(pool) = weak.upgrade() else {
                    break;
                };
                pool.reap_idle_at(Instant::now());
            }
        })
    }

    pub(crate) async fn acquire_with_loader<L, LF, B>(
        &self,
        row: &PluginRow,
        load: L,
        build_compiled: B,
    ) -> Result<PooledPlugin, PoolError>
    where
        L: FnOnce() -> LF,
        LF: Future<Output = Result<Vec<u8>, PoolError>>,
        B: FnOnce(Vec<u8>) -> Result<CompiledPlugin, anyhow::Error> + Send + 'static,
    {
        self.acquire_with_loader_and_factory(row, load, build_compiled, |compiled| {
            ExtismPlugin::new_from_compiled(compiled.as_ref())
        })
        .await
    }

    async fn acquire_with_loader_and_factory<L, LF, B, I>(
        &self,
        row: &PluginRow,
        load: L,
        build_compiled: B,
        instantiate: I,
    ) -> Result<PooledPlugin, PoolError>
    where
        L: FnOnce() -> LF,
        LF: Future<Output = Result<Vec<u8>, PoolError>>,
        B: FnOnce(Vec<u8>) -> Result<CompiledPlugin, anyhow::Error> + Send + 'static,
        I: FnOnce(Arc<CompiledPlugin>) -> Result<ExtismPlugin, anyhow::Error> + Send + 'static,
    {
        let admission = self.reserve(PoolKey::from_row(row)).await?;
        let compiled = match admission.compiled() {
            Some(compiled) => compiled,
            None => {
                let bytes = load().await?;
                let actual: String = Sha256::digest(&bytes)
                    .iter()
                    .map(|byte| format!("{byte:02x}"))
                    .collect();
                if actual != row.sha256 {
                    tracing::error!(
                        plugin_id = row.id,
                        error_kind = "plugin_sha256_mismatch",
                        "sha256 verify failed; refusing to instantiate"
                    );
                    return Err(PoolError::Sha256Mismatch);
                }
                let compiled = tokio::task::spawn_blocking(move || build_compiled(bytes))
                    .await
                    .map_err(|error| PoolError::Compile(format!("spawn_blocking: {error}")))?
                    .map_err(|error| PoolError::Compile(format!("{error}")))?;
                let compiled = Arc::new(compiled);
                self.core
                    .record_compile_success(row.id, Arc::clone(&compiled));
                compiled
            }
        };

        let factory_compiled = Arc::clone(&compiled);
        let plugin = tokio::task::spawn_blocking(move || instantiate(factory_compiled))
            .await
            .map_err(|error| PoolError::Instantiate(format!("spawn_blocking: {error}")))?
            .map_err(|error| PoolError::Instantiate(format!("{error}")))?;
        Ok(admission.complete(compiled, plugin))
    }

    /// acquire = idle compiled-slot hit or capacity-controlled cold creation.
    pub async fn acquire(
        &self,
        s3: Option<&S3Client>,
        row: &PluginRow,
        build_compiled: impl FnOnce(Vec<u8>) -> Result<CompiledPlugin, anyhow::Error> + Send + 'static,
    ) -> Result<PooledPlugin, PoolError> {
        self.acquire_with_loader(
            row,
            || async {
                let s3 = s3.ok_or_else(|| {
                    PoolError::S3(
                        "plugin system disabled: S3 client unavailable for cold start".into(),
                    )
                })?;
                s3::get_wasm(s3, &row.s3_key)
                    .await
                    .map_err(|error| PoolError::S3(format!("{error}")))
            },
            build_compiled,
        )
        .await
    }
}

#[derive(Debug, thiserror::Error)]
pub enum PoolError {
    #[error("plugin pool busy")]
    Busy,
    #[error("S3 read failed: {0}")]
    S3(String),
    #[error("sha256 mismatch — refusing to load")]
    Sha256Mismatch,
    #[error("plugin compile failed: {0}")]
    Compile(String),
    #[error("plugin instantiation failed: {0}")]
    Instantiate(String),
}

#[cfg(test)]
mod tests {
    use super::{InstancePool, PoolConfig, PoolError, PooledPlugin};
    use crate::models::Plugin as PluginRow;
    use extism::{CompiledPlugin, Plugin as ExtismPlugin, PluginBuilder};
    use sha2::{Digest, Sha256};
    use std::sync::Arc;
    use std::time::{Duration, Instant};

    const EMPTY_PLUGIN_WAT: &str = r#"
        (module
          (func (export "run") (result i32)
            i32.const 0))
    "#;

    fn test_config(max_per_plugin: usize, max_total: usize) -> PoolConfig {
        PoolConfig {
            max_per_plugin,
            max_total,
            idle_timeout: Duration::from_secs(60),
            acquire_timeout: Duration::from_millis(75),
            call_timeout_ms: 5_000,
            call_memory_mb: 128,
            call_fuel: 10_000,
        }
    }

    fn plugin_bytes() -> Vec<u8> {
        wat::parse_str(EMPTY_PLUGIN_WAT).expect("test WAT must compile")
    }

    fn plugin_row(id: i64, bytes: &[u8]) -> PluginRow {
        let sha256 = Sha256::digest(bytes)
            .iter()
            .map(|byte| format!("{byte:02x}"))
            .collect();
        PluginRow {
            id,
            identifier: format!("plugin-{id}"),
            name: format!("Plugin {id}"),
            description: None,
            manifest: None,
            runtime: "extism".into(),
            version: "1.0.0".into(),
            author: None,
            repository_url: None,
            s3_key: format!("plugins/{id}.wasm"),
            sha256,
            size_bytes: bytes.len() as i64,
            category_id: None,
            created_at: chrono::Utc::now(),
            updated_at: chrono::Utc::now(),
            deleted_at: None,
        }
    }

    fn compile_plugin(bytes: Vec<u8>) -> Result<CompiledPlugin, anyhow::Error> {
        PluginBuilder::new(bytes).with_wasi(false).compile()
    }

    async fn acquire_loaded(
        pool: &Arc<InstancePool>,
        row: &PluginRow,
        bytes: Vec<u8>,
    ) -> Result<PooledPlugin, PoolError> {
        pool.acquire_with_loader(row, move || async move { Ok(bytes) }, compile_plugin)
            .await
    }

    async fn acquire_cached(
        pool: &Arc<InstancePool>,
        row: &PluginRow,
    ) -> Result<PooledPlugin, PoolError> {
        pool.acquire_with_loader(
            row,
            || async {
                Err(PoolError::S3(
                    "compiled cache hit must not reload S3".into(),
                ))
            },
            compile_plugin,
        )
        .await
    }

    async fn wait_for_wait_count(pool: &InstancePool, expected: u64) {
        tokio::time::timeout(Duration::from_secs(1), async {
            loop {
                if pool.metrics_snapshot().await.wait_count >= expected {
                    break;
                }
                tokio::task::yield_now().await;
            }
        })
        .await
        .expect("waiter must enter the FIFO queue");
    }

    #[tokio::test]
    async fn discard_lost_reconciles_in_use_without_creating_idle_instance() {
        let pool = InstancePool::new(PoolConfig {
            max_per_plugin: 1,
            max_total: 1,
            idle_timeout: Duration::from_secs(60),
            acquire_timeout: Duration::from_secs(5),
            call_timeout_ms: 5_000,
            call_memory_mb: 128,
            call_fuel: 10_000,
        });
        pool.seed_in_use_for_test(42, "lost-plugin", "1.0.0");

        pool.discard_lost(42).await;

        let metrics = pool.metrics_snapshot().await;
        let per_plugin = pool.per_plugin_snapshot().await;
        assert_eq!(metrics.in_use, 0);
        assert_eq!(metrics.idle, 0);
        assert_eq!(per_plugin.len(), 1);
        assert_eq!(per_plugin[0].in_use, 0);
        assert_eq!(per_plugin[0].idle, 0);
    }

    #[test]
    fn call_limits_use_documented_defaults_and_parse_explicit_values() {
        assert_eq!(
            PoolConfig::parse_nonzero_u64(
                "PLUGIN_CALL_TIMEOUT_MS",
                None,
                super::DEFAULT_CALL_TIMEOUT_MS
            ),
            Ok(30_000)
        );
        assert_eq!(
            PoolConfig::parse_nonzero_u64(
                "PLUGIN_CALL_TIMEOUT_MS",
                Some("2500"),
                super::DEFAULT_CALL_TIMEOUT_MS
            ),
            Ok(2_500)
        );
        assert_eq!(
            PoolConfig::parse_nonzero_u64("PLUGIN_CALL_FUEL", None, super::DEFAULT_CALL_FUEL),
            Ok(10_000_000_000)
        );
        assert_eq!(
            PoolConfig::parse_nonzero_u64(
                "PLUGIN_CALL_FUEL",
                Some("250000"),
                super::DEFAULT_CALL_FUEL
            ),
            Ok(250_000)
        );
    }

    #[test]
    fn call_limits_reject_zero_malformed_and_whitespace_values() {
        for (name, default) in [
            ("PLUGIN_CALL_TIMEOUT_MS", super::DEFAULT_CALL_TIMEOUT_MS),
            ("PLUGIN_CALL_FUEL", super::DEFAULT_CALL_FUEL),
        ] {
            for value in ["0", "-1", "not-a-number", " 42 "] {
                let error = PoolConfig::parse_nonzero_u64(name, Some(value), default)
                    .expect_err("invalid call limits must fail instead of using the default");
                assert!(error.contains(name));
            }
        }
    }

    #[tokio::test]
    async fn per_plugin_limit_waits_fifo_and_reuses_compiled_slot() {
        let bytes = plugin_bytes();
        let row = plugin_row(1, &bytes);
        let pool = InstancePool::new(test_config(1, 4));
        let first = acquire_loaded(&pool, &row, bytes)
            .await
            .expect("first slot must be created");
        let (acquired_tx, mut acquired_rx) = tokio::sync::mpsc::unbounded_channel();

        for ordinal in [1_u8, 2_u8] {
            let waiter_pool = Arc::clone(&pool);
            let row = row.clone();
            let acquired_tx = acquired_tx.clone();
            tokio::spawn(async move {
                let plugin = acquire_cached(&waiter_pool, &row)
                    .await
                    .expect("FIFO waiter must acquire released slot");
                acquired_tx
                    .send((ordinal, plugin))
                    .expect("test receiver must remain alive");
            });
            wait_for_wait_count(&pool, ordinal.into()).await;
        }

        pool.release(first, true, true).await;
        let (first_ordinal, first_waiter) = acquired_rx
            .recv()
            .await
            .expect("first waiter must be woken");
        assert_eq!(first_ordinal, 1, "waiters must be served in FIFO order");
        assert!(
            acquired_rx.try_recv().is_err(),
            "second waiter must remain blocked until the slot is released again"
        );

        pool.release(first_waiter, true, true).await;
        let (second_ordinal, second_waiter) = acquired_rx
            .recv()
            .await
            .expect("second waiter must be woken");
        assert_eq!(second_ordinal, 2, "waiters must be served in FIFO order");
        pool.release(second_waiter, true, true).await;

        let metrics = pool.metrics_snapshot().await;
        assert_eq!(metrics.in_use, 0);
        assert_eq!(metrics.idle, 1);
        assert_eq!(metrics.wait_count, 2);
        assert_eq!(
            metrics.created_total, 3,
            "created_total counts every successfully created fresh Store/Instance"
        );
        assert_eq!(
            metrics.cache_misses, 1,
            "cache_misses counts only successful actual compilations"
        );
    }

    #[tokio::test]
    async fn shared_compiled_plugin_expands_fresh_instances_without_cache_miss() {
        let bytes = plugin_bytes();
        let row = plugin_row(1, &bytes);
        let pool = InstancePool::new(test_config(2, 2));
        let first = acquire_loaded(&pool, &row, bytes)
            .await
            .expect("first invocation must compile");
        let second = acquire_cached(&pool, &row)
            .await
            .expect("shared CompiledPlugin must create a concurrent fresh Instance");

        let active = pool.metrics_snapshot().await;
        assert_eq!(active.in_use, 2);
        assert_eq!(active.created_total, 2);
        assert_eq!(
            active.cache_misses, 1,
            "fresh Instance expansion from a shared CompiledPlugin is not a compile miss"
        );

        pool.release(first, true, true).await;
        pool.release(second, true, true).await;
    }

    #[tokio::test]
    async fn idle_compiled_cache_does_not_consume_global_invocation_permit() {
        let bytes = plugin_bytes();
        let row_one = plugin_row(1, &bytes);
        let row_two = plugin_row(2, &bytes);
        let pool = InstancePool::new(test_config(1, 1));
        let first = acquire_loaded(&pool, &row_one, bytes.clone())
            .await
            .expect("first invocation must compile");
        pool.release(first, true, true).await;

        let second = acquire_loaded(&pool, &row_two, bytes)
            .await
            .expect("idle compiled cache must not consume the global invocation permit");
        let active = pool.metrics_snapshot().await;
        assert_eq!(active.in_use, 1);
        assert_eq!(active.idle, 1);
        assert_eq!(active.created_total, 2);
        assert_eq!(active.cache_misses, 2);
        pool.release(second, true, true).await;
    }

    #[tokio::test]
    async fn global_limit_times_out_as_pool_busy_and_maps_to_5009() {
        let bytes = plugin_bytes();
        let row_one = plugin_row(1, &bytes);
        let row_two = plugin_row(2, &bytes);
        let mut config = test_config(2, 1);
        config.acquire_timeout = Duration::from_millis(25);
        let pool = InstancePool::new(config);
        let first = acquire_loaded(&pool, &row_one, bytes.clone())
            .await
            .expect("first global slot must be created");

        let error = acquire_loaded(&pool, &row_two, bytes)
            .await
            .expect_err("global capacity must not be exceeded");
        assert!(matches!(error, PoolError::Busy));
        let app_error: crate::utils::error::AppError =
            crate::runtime::invoker::InvokerError::from(error).into();
        assert_eq!(app_error.code(), crate::utils::error::codes::POOL_BUSY);

        let metrics = pool.metrics_snapshot().await;
        assert_eq!(metrics.in_use, 1);
        assert_eq!(metrics.idle, 0);
        assert_eq!(metrics.wait_count, 1);
        pool.release(first, true, true).await;
    }

    #[tokio::test]
    async fn cancelling_fifo_waiter_does_not_leak_or_double_release_capacity() {
        let bytes = plugin_bytes();
        let row = plugin_row(1, &bytes);
        let pool = InstancePool::new(test_config(1, 1));
        let first = acquire_loaded(&pool, &row, bytes)
            .await
            .expect("first slot must be created");

        let waiter = tokio::spawn({
            let pool = Arc::clone(&pool);
            let row = row.clone();
            async move { acquire_cached(&pool, &row).await }
        });
        wait_for_wait_count(&pool, 1).await;
        waiter.abort();
        assert!(
            waiter
                .await
                .expect_err("aborted waiter must not finish")
                .is_cancelled()
        );

        pool.release(first, true, true).await;
        let next = acquire_cached(&pool, &row)
            .await
            .expect("cancelled waiter must not retain the only slot");
        pool.release(next, true, true).await;
        let metrics = pool.metrics_snapshot().await;
        assert_eq!(metrics.in_use, 0);
        assert_eq!(metrics.idle, 1);
        assert_eq!(metrics.created_total, 2);
    }

    #[tokio::test]
    async fn cancelling_cold_load_rolls_back_reserved_capacity() {
        let bytes = plugin_bytes();
        let row = plugin_row(1, &bytes);
        let pool = InstancePool::new(test_config(1, 1));
        let (loader_started_tx, loader_started_rx) = tokio::sync::oneshot::channel();
        let (_loader_release_tx, loader_release_rx) = tokio::sync::oneshot::channel::<()>();

        let loading = tokio::spawn({
            let pool = Arc::clone(&pool);
            let row = row.clone();
            async move {
                pool.acquire_with_loader(
                    &row,
                    move || async move {
                        let _ = loader_started_tx.send(());
                        let _ = loader_release_rx.await;
                        unreachable!("cancelled loader must not complete")
                    },
                    compile_plugin,
                )
                .await
            }
        });
        loader_started_rx
            .await
            .expect("cold loader must hold the reserved slot");
        loading.abort();
        assert!(
            loading
                .await
                .expect_err("cancelled cold load must not return")
                .is_cancelled()
        );

        let next = acquire_loaded(&pool, &row, bytes)
            .await
            .expect("cold-load cancellation must roll back capacity");
        pool.release(next, true, true).await;
        let metrics = pool.metrics_snapshot().await;
        assert_eq!(metrics.in_use, 0);
        assert_eq!(metrics.idle, 1);
        assert_eq!(metrics.created_total, 1);
        assert_eq!(metrics.cache_misses, 1);
    }

    #[tokio::test]
    async fn cancelling_detached_compile_rolls_back_reserved_capacity_once() {
        let bytes = plugin_bytes();
        let row = plugin_row(1, &bytes);
        let pool = InstancePool::new(test_config(1, 1));
        let (compile_started_tx, compile_started_rx) = tokio::sync::oneshot::channel();
        let (compile_release_tx, compile_release_rx) = std::sync::mpsc::channel();

        let compiling = tokio::spawn({
            let pool = Arc::clone(&pool);
            let row = row.clone();
            let bytes = bytes.clone();
            async move {
                pool.acquire_with_loader(
                    &row,
                    move || async move { Ok(bytes) },
                    move |bytes| {
                        let _ = compile_started_tx.send(());
                        compile_release_rx
                            .recv()
                            .expect("test must release detached compile");
                        compile_plugin(bytes)
                    },
                )
                .await
            }
        });
        compile_started_rx
            .await
            .expect("cold compile must hold the reserved slot");
        compiling.abort();
        assert!(
            compiling
                .await
                .expect_err("cancelled compile must not return")
                .is_cancelled()
        );
        compile_release_tx
            .send(())
            .expect("detached compile must be allowed to finish");

        let next = acquire_loaded(&pool, &row, bytes)
            .await
            .expect("compile cancellation must roll back capacity immediately");
        pool.release(next, true, true).await;
        tokio::time::sleep(Duration::from_millis(20)).await;
        let metrics = pool.metrics_snapshot().await;
        assert_eq!(metrics.in_use, 0);
        assert_eq!(metrics.idle, 1);
        assert_eq!(metrics.created_total, 1);
        assert_eq!(metrics.cache_misses, 1);
    }

    #[tokio::test]
    async fn compile_and_instantiation_failures_roll_back_capacity_and_counters() {
        let bytes = plugin_bytes();
        let row = plugin_row(1, &bytes);
        let pool = InstancePool::new(test_config(1, 1));

        let compile_error = pool
            .acquire_with_loader(
                &row,
                {
                    let bytes = bytes.clone();
                    move || async move { Ok(bytes) }
                },
                |_| Err(anyhow::anyhow!("compile failure sentinel")),
            )
            .await
            .expect_err("compile failure must surface");
        assert!(matches!(compile_error, PoolError::Compile(_)));
        let after_compile_failure = pool.metrics_snapshot().await;
        assert_eq!(after_compile_failure.created_total, 0);
        assert_eq!(after_compile_failure.cache_misses, 0);

        let instantiate_error = pool
            .acquire_with_loader_and_factory(
                &row,
                {
                    let bytes = bytes.clone();
                    move || async move { Ok(bytes) }
                },
                compile_plugin,
                |_: Arc<CompiledPlugin>| {
                    Err::<ExtismPlugin, _>(anyhow::anyhow!("instantiate failure sentinel"))
                },
            )
            .await
            .expect_err("instance creation failure must surface");
        assert!(matches!(instantiate_error, PoolError::Instantiate(_)));

        let failed_metrics = pool.metrics_snapshot().await;
        assert_eq!(failed_metrics.in_use, 0);
        assert_eq!(failed_metrics.idle, 0);
        assert_eq!(failed_metrics.created_total, 0);
        assert_eq!(
            failed_metrics.cache_misses, 1,
            "a successful actual compile is a miss even if fresh Instance creation fails"
        );

        let plugin = acquire_loaded(&pool, &row, bytes)
            .await
            .expect("failed builds must leave capacity available");
        pool.release(plugin, true, true).await;
        let metrics = pool.metrics_snapshot().await;
        assert_eq!(metrics.created_total, 1);
        assert_eq!(metrics.cache_misses, 1);
    }

    #[tokio::test]
    async fn compiled_cache_without_idle_slot_survives_reaper_until_timeout() {
        let bytes = plugin_bytes();
        let row = plugin_row(1, &bytes);
        let pool = InstancePool::new(test_config(1, 1));

        let error = pool
            .acquire_with_loader_and_factory(
                &row,
                move || async move { Ok(bytes) },
                compile_plugin,
                |_: Arc<CompiledPlugin>| {
                    Err::<ExtismPlugin, _>(anyhow::anyhow!("instantiate failure sentinel"))
                },
            )
            .await
            .expect_err("forced fresh Instance creation must fail");
        assert!(matches!(error, PoolError::Instantiate(_)));
        assert_eq!(pool.reap_idle_at(Instant::now()), 0);

        let plugin = acquire_cached(&pool, &row)
            .await
            .expect("unexpired successful compilation must remain cached");
        pool.release(plugin, true, true).await;
        let metrics = pool.metrics_snapshot().await;
        assert_eq!(metrics.created_total, 1);
        assert_eq!(metrics.cache_misses, 1);
    }

    #[tokio::test]
    async fn idle_reaper_evicts_compiled_cache_and_forces_one_new_compile() {
        let bytes = plugin_bytes();
        let row_one = plugin_row(1, &bytes);
        let mut config = test_config(1, 1);
        config.idle_timeout = Duration::from_millis(20);
        config.acquire_timeout = Duration::from_millis(500);
        let pool = InstancePool::new(config);
        let first = acquire_loaded(&pool, &row_one, bytes.clone())
            .await
            .expect("first slot must be created");
        pool.release(first, true, true).await;
        let reaper = pool.start_idle_reaper();

        tokio::time::timeout(Duration::from_secs(1), async {
            loop {
                if pool.metrics_snapshot().await.idle == 0 {
                    break;
                }
                tokio::task::yield_now().await;
            }
        })
        .await
        .expect("reaper must evict the expired compiled slot");
        let second = acquire_loaded(&pool, &row_one, bytes)
            .await
            .expect("checkout after reaping must recompile");
        reaper.abort();

        let metrics = pool.metrics_snapshot().await;
        assert_eq!(metrics.in_use, 1);
        assert_eq!(metrics.idle, 0);
        assert_eq!(metrics.created_total, 2);
        assert_eq!(metrics.cache_misses, 2);
        pool.release(second, true, true).await;
    }
}
