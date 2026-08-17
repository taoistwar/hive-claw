//! T074 [P] [US8] Plugin resource limits + instance pool contract.
//!
//! Source of truth: `specs/011-hivegui-standalone-mode/tasks.md` §T074
//! ("在 `crates/hivegui/tests/plugin_limits.rs` 编写默认30s/128MiB/10MiB、
//! 硬上限120s/512MiB/50MiB、`memory_limit_mb` 兼容字段语义、128→2048与
//! 512→8192个64KiB pages、拒绝把 MiB 值或字节数直接传给 page 参数、fuel，
//! 以及闲置实例池全局最多8个/每个完整 cache key 最多1个的 LRU 测试")。
//!
//! Public boundaries the T079 implementation provides:
//!   - `hivegui::runtime::plugin_executor::PluginExecutor`
//!   - `hivegui::runtime::plugin_executor::PluginLimits`

mod support;

use hivegui::datasource::migrations::{MigrationOptions, migrate_to_current};
use hivegui::plugin::plugin_store::PluginStore;
use hivegui::runtime::plugin_executor::{
    DEFAULT_MEMORY_MB, DEFAULT_OUTPUT_BYTES, DEFAULT_TIMEOUT_SECS, HARD_MAX_MEMORY_MB,
    HARD_MAX_OUTPUT_BYTES, HARD_MAX_TIMEOUT_SECS, PluginExecutor, PluginLimits,
};
use support::TestWorkspace;

async fn migrated_pool(workspace: &TestWorkspace) -> sqlx::SqlitePool {
    migrate_to_current(MigrationOptions::new(
        workspace.database_path(),
        workspace.plugin_root(),
    ))
    .await
    .expect("migrate to current");
    workspace.sqlite_pool().await.expect("sqlite pool")
}

#[test]
fn default_limits_match_spec() {
    assert_eq!(DEFAULT_TIMEOUT_SECS, 30);
    assert_eq!(DEFAULT_MEMORY_MB, 128);
    assert_eq!(DEFAULT_OUTPUT_BYTES, 10 * 1024 * 1024);
}

#[test]
fn hard_caps_match_spec() {
    assert_eq!(HARD_MAX_TIMEOUT_SECS, 120);
    assert_eq!(HARD_MAX_MEMORY_MB, 512);
    assert_eq!(HARD_MAX_OUTPUT_BYTES, 50 * 1024 * 1024);
}

#[test]
fn plugin_limits_reject_out_of_range_values() {
    let err = PluginLimits::new(0, 128, 10 * 1024 * 1024).expect_err("timeout 0");
    assert!(matches!(
        err,
        hivegui::runtime::plugin_executor::PluginLimitError::TimeoutOutOfRange { .. }
    ));
    let err = PluginLimits::new(121, 128, 10 * 1024 * 1024).expect_err("timeout 121");
    assert!(matches!(
        err,
        hivegui::runtime::plugin_executor::PluginLimitError::TimeoutOutOfRange { .. }
    ));
    let err = PluginLimits::new(30, 0, 10 * 1024 * 1024).expect_err("memory 0");
    assert!(matches!(
        err,
        hivegui::runtime::plugin_executor::PluginLimitError::MemoryOutOfRange { .. }
    ));
    let err = PluginLimits::new(30, 513, 10 * 1024 * 1024).expect_err("memory 513");
    assert!(matches!(
        err,
        hivegui::runtime::plugin_executor::PluginLimitError::MemoryOutOfRange { .. }
    ));
}

#[test]
fn plugin_limits_accept_in_range_values() {
    let limits = PluginLimits::new(30, 128, 10 * 1024 * 1024).expect("in range");
    assert_eq!(limits.timeout_secs(), 30);
    assert_eq!(limits.memory_mb(), 128);
    assert_eq!(limits.output_bytes(), 10 * 1024 * 1024);
    let limits = PluginLimits::new(120, 512, 50 * 1024 * 1024).expect("hard cap");
    assert_eq!(limits.timeout_secs(), 120);
    assert_eq!(limits.memory_mb(), 512);
    assert_eq!(limits.output_bytes(), 50 * 1024 * 1024);
}

#[tokio::test(flavor = "current_thread")]
async fn plugin_executor_constructs_with_default_limits() {
    let workspace = TestWorkspace::new().expect("test workspace");
    let pool = migrated_pool(&workspace).await;
    let store = PluginStore::new(pool.clone(), workspace.plugin_root()).expect("store");
    let executor = PluginExecutor::new(store);
    let limits = executor.limits();
    assert_eq!(limits.timeout_secs(), DEFAULT_TIMEOUT_SECS);
    assert_eq!(limits.memory_mb(), DEFAULT_MEMORY_MB);
    assert_eq!(limits.output_bytes(), DEFAULT_OUTPUT_BYTES);
}

#[tokio::test(flavor = "current_thread")]
async fn plugin_executor_pool_capacity_is_bounded() {
    let workspace = TestWorkspace::new().expect("test workspace");
    let pool = migrated_pool(&workspace).await;
    let store = PluginStore::new(pool.clone(), workspace.plugin_root()).expect("store");
    let executor = PluginExecutor::new(store);
    assert_eq!(executor.pool_capacity(), 8);
}
