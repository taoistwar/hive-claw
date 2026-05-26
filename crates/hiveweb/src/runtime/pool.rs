//! Plugin Instance Pool（FR-029 / research §2）
//!
//! 层级结构：`HashMap<PluginId, PluginPool>` + 内层 `VecDeque<PooledInstance>`。
//! 实例归还前必须 `reset()`；reset 失败则丢弃实例并记入指标。
//!
//! 当前为骨架：尚未接 Extism `Plugin`，只暴露形状供其它模块编译依赖。

use std::collections::{HashMap, VecDeque};
use std::sync::Arc;
use std::time::{Duration, Instant};
use tokio::sync::Mutex;

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

#[derive(Debug)]
pub struct PoolConfig {
    pub max_per_plugin: usize,
    pub max_total: usize,
    pub idle_timeout: Duration,
    pub acquire_timeout: Duration,
}

impl PoolConfig {
    pub fn from_env() -> Self {
        let max_per_plugin = std::env::var("PLUGIN_POOL_MAX_PER_PLUGIN")
            .ok()
            .and_then(|v| v.parse().ok())
            .unwrap_or(8);
        let max_total = std::env::var("PLUGIN_POOL_MAX_TOTAL")
            .ok()
            .and_then(|v| v.parse().ok())
            .unwrap_or(64);
        let idle_secs: u64 = std::env::var("PLUGIN_POOL_IDLE_TIMEOUT_SEC")
            .ok()
            .and_then(|v| v.parse().ok())
            .unwrap_or(600);
        let acquire_ms: u64 = std::env::var("PLUGIN_POOL_ACQUIRE_TIMEOUT_MS")
            .ok()
            .and_then(|v| v.parse().ok())
            .unwrap_or(5000);
        Self {
            max_per_plugin,
            max_total,
            idle_timeout: Duration::from_secs(idle_secs),
            acquire_timeout: Duration::from_millis(acquire_ms),
        }
    }
}

/// 内层池中的一个空闲实例占位。真实实现会替换为 `Mutex<extism::Plugin>` + sha256。
#[derive(Debug)]
pub struct PooledInstance {
    pub plugin_id: i64,
    pub last_used: Instant,
    // TODO(US1+US4): plugin: extism::Plugin,
}

#[derive(Debug, Default)]
pub struct PluginPool {
    pub plugin_id: i64,
    pub identifier: String,
    pub version: String,
    pub idle: VecDeque<PooledInstance>,
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
}
