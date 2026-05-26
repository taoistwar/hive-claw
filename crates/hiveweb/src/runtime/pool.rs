//! Plugin Instance Pool（FR-029 / research §2 / US4 commit 2）
//!
//! 层级结构：`HashMap<PluginId, PluginPool>` + 内层 `VecDeque<PooledPlugin>`。
//! 实例归还前必须 `reset()`；reset 失败则丢弃实例并计入 `reset_failures` 指标。
//!
//! Pool 与 Extism 的边界：本模块负责"哪个 Plugin 有几个 idle / in_use 实例 +
//! 命中统计"；具体 WASM 编译与 host_call host_fn 注册在 `invoker::build_plugin`。

use aws_sdk_s3::Client as S3Client;
use extism::Plugin as ExtismPlugin;
use sha2::{Digest, Sha256};
use std::collections::{HashMap, VecDeque};
use std::sync::Arc;
use std::time::{Duration, Instant};
use tokio::sync::Mutex;

use crate::models::Plugin as PluginRow;
use crate::storage::s3;

#[derive(Debug, Clone, Default)]
pub struct PoolMetrics {
    pub in_use: u64,
    pub idle: u64,
    pub created_total: u64,
    pub cache_misses: u64,
    pub wait_count: u64,
    pub reset_failures: u64,
}

#[derive(Debug, Clone, Default)]
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
}

impl PoolConfig {
    pub fn from_env() -> Self {
        fn env<T: std::str::FromStr>(name: &str, default: T) -> T {
            std::env::var(name).ok().and_then(|v| v.parse().ok()).unwrap_or(default)
        }
        Self {
            max_per_plugin: env("PLUGIN_POOL_MAX_PER_PLUGIN", 8),
            max_total: env("PLUGIN_POOL_MAX_TOTAL", 64),
            idle_timeout: Duration::from_secs(env("PLUGIN_POOL_IDLE_TIMEOUT_SEC", 600u64)),
            acquire_timeout: Duration::from_millis(env("PLUGIN_POOL_ACQUIRE_TIMEOUT_MS", 5000u64)),
            call_timeout_ms: env("PLUGIN_CALL_TIMEOUT_MS", 30000u64),
            call_memory_mb: env("PLUGIN_CALL_MAX_MEMORY_MB", 128u64),
        }
    }
}

/// 一个被池化的 Extism Plugin 实例
pub struct PooledPlugin {
    pub plugin: ExtismPlugin,
    pub plugin_id: i64,
    pub last_used: Instant,
}

impl std::fmt::Debug for PooledPlugin {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("PooledPlugin")
            .field("plugin_id", &self.plugin_id)
            .field("last_used", &self.last_used)
            .finish()
    }
}

#[derive(Debug, Default)]
pub struct PluginPool {
    pub plugin_id: i64,
    pub identifier: String,
    pub version: String,
    pub idle: VecDeque<PooledPlugin>,
    pub in_use: usize,
    pub cache_misses: u64,
}

#[derive(Debug)]
pub struct InstancePool {
    pub config: PoolConfig,
    pub inner: Mutex<HashMap<i64, PluginPool>>,
    pub metrics: Mutex<PoolMetrics>,
}

impl InstancePool {
    pub fn new(config: PoolConfig) -> Arc<Self> {
        Arc::new(Self {
            config,
            inner: Mutex::new(HashMap::new()),
            metrics: Mutex::new(PoolMetrics::default()),
        })
    }

    pub async fn metrics_snapshot(&self) -> PoolMetrics {
        self.metrics.lock().await.clone()
    }

    pub async fn per_plugin_snapshot(&self) -> Vec<PerPluginMetrics> {
        let map = self.inner.lock().await;
        map.values()
            .map(|p| PerPluginMetrics {
                plugin_id: p.plugin_id,
                identifier: p.identifier.clone(),
                version: p.version.clone(),
                in_use: p.in_use as u64,
                idle: p.idle.len() as u64,
                cache_misses: p.cache_misses,
            })
            .collect()
    }

    /// 尝试从 idle 队列取一个实例；找到则 in_use++ 并返回
    async fn try_acquire_idle(&self, plugin_id: i64) -> Option<PooledPlugin> {
        let mut map = self.inner.lock().await;
        let p = map.get_mut(&plugin_id)?;
        let inst = p.idle.pop_front()?;
        p.in_use += 1;
        {
            let mut m = self.metrics.lock().await;
            m.in_use += 1;
            m.idle = m.idle.saturating_sub(1);
        }
        Some(inst)
    }

    /// 归还实例。reset_ok=false 时直接丢弃 + 计 reset_failures
    pub async fn release(&self, mut inst: PooledPlugin, reset_ok: bool) {
        let mut map = self.inner.lock().await;
        if let Some(p) = map.get_mut(&inst.plugin_id) {
            p.in_use = p.in_use.saturating_sub(1);
            let mut m = self.metrics.lock().await;
            m.in_use = m.in_use.saturating_sub(1);
            if reset_ok && p.idle.len() < self.config.max_per_plugin {
                inst.last_used = Instant::now();
                p.idle.push_back(inst);
                m.idle += 1;
            } else {
                if !reset_ok {
                    m.reset_failures += 1;
                }
                // drop instance
            }
        }
    }

    /// acquire = idle 命中 or 冷启动构建
    pub async fn acquire(
        &self,
        s3: &S3Client,
        row: &PluginRow,
        build_plugin: impl FnOnce(Vec<u8>) -> Result<ExtismPlugin, anyhow::Error> + Send + 'static,
    ) -> Result<PooledPlugin, PoolError> {
        // 1. try idle
        if let Some(inst) = self.try_acquire_idle(row.id).await {
            return Ok(inst);
        }
        // 2. cold start with sha256 verify
        let bytes = s3::get_wasm(s3, &row.s3_key)
            .await
            .map_err(|e| PoolError::S3(format!("{e}")))?;
        let mut h = Sha256::new();
        h.update(&bytes);
        let digest = h.finalize();
        let actual: String = digest.iter().map(|b| format!("{b:02x}")).collect();
        if actual != row.sha256 {
            tracing::error!(
                plugin_id = row.id,
                expected = %row.sha256,
                actual = %actual,
                "sha256 verify failed; refusing to instantiate"
            );
            return Err(PoolError::Sha256Mismatch);
        }
        let plugin = tokio::task::spawn_blocking(move || build_plugin(bytes))
            .await
            .map_err(|e| PoolError::Compile(format!("spawn_blocking: {e}")))?
            .map_err(|e| PoolError::Compile(format!("{e}")))?;

        // metric only
        {
            let mut map = self.inner.lock().await;
            let entry = map.entry(row.id).or_insert_with(|| PluginPool {
                plugin_id: row.id,
                identifier: row.identifier.clone(),
                version: row.version.clone(),
                ..Default::default()
            });
            entry.in_use += 1;
            entry.cache_misses += 1;
            let mut m = self.metrics.lock().await;
            m.in_use += 1;
            m.created_total += 1;
            m.cache_misses += 1;
        }

        Ok(PooledPlugin {
            plugin,
            plugin_id: row.id,
            last_used: Instant::now(),
        })
    }
}

#[derive(Debug, thiserror::Error)]
pub enum PoolError {
    #[error("S3 read failed: {0}")]
    S3(String),
    #[error("sha256 mismatch — refusing to load")]
    Sha256Mismatch,
    #[error("plugin compile failed: {0}")]
    Compile(String),
}
