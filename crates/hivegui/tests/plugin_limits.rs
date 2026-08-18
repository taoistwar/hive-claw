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
    BoundedInstancePool, DEFAULT_MEMORY_MB, DEFAULT_OUTPUT_BYTES, DEFAULT_TIMEOUT_SECS,
    HARD_MAX_MEMORY_MB, HARD_MAX_OUTPUT_BYTES, HARD_MAX_TIMEOUT_SECS, InstanceCacheKey,
    PAGES_PER_MIB, PageCount, PluginExecutor, PluginLimits, WASM_PAGE_BYTES,
};
use std::collections::HashSet;
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

/// A base cache-key tuple; each test varies one component off this base.
fn base_key() -> InstanceCacheKey {
    InstanceCacheKey::new(
        "a".repeat(64),
        "hive-extism/v1",
        "extism",
        "1.30.0",
        Some(10_000),
        30_000,
        128,
        10 * 1024 * 1024,
        "cap-policy-0000",
    )
}

#[test]
fn memory_limit_converts_mib_to_64kib_pages() {
    assert_eq!(WASM_PAGE_BYTES, 64 * 1024);
    assert_eq!(PAGES_PER_MIB, 16);

    // 128 MiB → 2048 pages, 512 MiB → 8192 pages (T074 spec).
    assert_eq!(PageCount::from_mib(128).pages(), 2048);
    assert_eq!(PageCount::from_mib(512).pages(), 8192);
    assert_eq!(
        PluginLimits::default_limits().memory_pages().pages(),
        128 * PAGES_PER_MIB
    );
}

#[test]
fn page_count_cannot_be_built_from_raw_mib_or_bytes() {
    // A raw MiB value is not a page count: the conversion must be applied.
    let from_mib = PageCount::from_mib(128).pages();
    assert_eq!(from_mib, 2048);
    assert_ne!(from_mib, 128, "128 MiB must not be used verbatim as pages");

    // A raw byte count is also not a page count.
    let ten_mib_bytes = 10 * 1024 * 1024u64;
    assert_ne!(ten_mib_bytes, PageCount::from_mib(10).pages());

    // `PageCount` has no `From<u64>`/raw constructor; only `from_mib` exists,
    // so the type system rejects passing MiB or bytes straight to the page
    // parameter. This test documents that contract (compile-time invariant).
}

#[test]
fn cache_key_is_deterministic_and_covers_all_nine_components() {
    // Deterministic: the same tuple always hashes identically.
    assert_eq!(base_key().cache_key(), base_key().cache_key());

    // Vary each of the nine components and assert a distinct key.
    let mut distinct: HashSet<String> = HashSet::new();
    distinct.insert(base_key().cache_key());

    let base = base_key();
    let variations = vec![
        InstanceCacheKey::new(
            "b".repeat(64),
            "hive-extism/v1",
            "extism",
            "1.30.0",
            Some(10_000),
            30_000,
            128,
            10 * 1024 * 1024,
            "cap-policy-0000",
        ), // artifact_sha256
        InstanceCacheKey::new(
            "a".repeat(64),
            "hive-extism/v2",
            "extism",
            "1.30.0",
            Some(10_000),
            30_000,
            128,
            10 * 1024 * 1024,
            "cap-policy-0000",
        ), // abi_version
        InstanceCacheKey::new(
            "a".repeat(64),
            "hive-extism/v1",
            "wasmtime",
            "1.30.0",
            Some(10_000),
            30_000,
            128,
            10 * 1024 * 1024,
            "cap-policy-0000",
        ), // runtime
        InstanceCacheKey::new(
            "a".repeat(64),
            "hive-extism/v1",
            "extism",
            "1.31.0",
            Some(10_000),
            30_000,
            128,
            10 * 1024 * 1024,
            "cap-policy-0000",
        ), // runtime_version
        InstanceCacheKey::new(
            "a".repeat(64),
            "hive-extism/v1",
            "extism",
            "1.30.0",
            Some(20_000),
            30_000,
            128,
            10 * 1024 * 1024,
            "cap-policy-0000",
        ), // fuel_limit
        InstanceCacheKey::new(
            "a".repeat(64),
            "hive-extism/v1",
            "extism",
            "1.30.0",
            Some(10_000),
            60_000,
            128,
            10 * 1024 * 1024,
            "cap-policy-0000",
        ), // timeout_ms
        InstanceCacheKey::new(
            "a".repeat(64),
            "hive-extism/v1",
            "extism",
            "1.30.0",
            Some(10_000),
            30_000,
            256,
            10 * 1024 * 1024,
            "cap-policy-0000",
        ), // memory_limit_mb
        InstanceCacheKey::new(
            "a".repeat(64),
            "hive-extism/v1",
            "extism",
            "1.30.0",
            Some(10_000),
            30_000,
            128,
            20 * 1024 * 1024,
            "cap-policy-0000",
        ), // output_limit_bytes
        InstanceCacheKey::new(
            "a".repeat(64),
            "hive-extism/v1",
            "extism",
            "1.30.0",
            Some(10_000),
            30_000,
            128,
            10 * 1024 * 1024,
            "cap-policy-0001",
        ), // capability_policy_hash
    ];
    assert_eq!(variations.len(), 9);
    for (i, key) in variations.iter().enumerate() {
        assert_ne!(
            key.cache_key(),
            base.cache_key(),
            "variation {i} must not collide with the base key"
        );
        assert!(
            distinct.insert(key.cache_key()),
            "variation {i} must be distinct from all others"
        );
    }
    // Fuel disabled vs. enabled must differ too.
    let no_fuel = InstanceCacheKey::new(
        "a".repeat(64),
        "hive-extism/v1",
        "extism",
        "1.30.0",
        None,
        30_000,
        128,
        10 * 1024 * 1024,
        "cap-policy-0000",
    );
    assert_ne!(no_fuel.cache_key(), base.cache_key());
}

#[test]
fn bounded_pool_enforces_global_capacity_with_lru_eviction() {
    let mut pool = BoundedInstancePool::new(8);
    assert_eq!(pool.capacity(), 8);

    // Insert 9 distinct keys; the first (LRU) must be evicted.
    let keys: Vec<InstanceCacheKey> = (0..9)
        .map(|i| {
            InstanceCacheKey::new(
                format!("{i:064x}"),
                "hive-extism/v1",
                "extism",
                "1.30.0",
                None,
                30_000,
                128,
                10 * 1024 * 1024,
                "cap-policy-0000",
            )
        })
        .collect();

    for (i, key) in keys.iter().enumerate() {
        pool.insert(key.clone(), i);
    }

    assert_eq!(pool.len(), 8, "global idle pool must be bounded to 8");
    assert!(!pool.contains_key(&keys[0]), "first key is the LRU victim");
    for key in &keys[1..] {
        assert!(pool.contains_key(key), "recent keys remain resident");
    }
}

#[test]
fn bounded_pool_keeps_one_instance_per_cache_key() {
    let mut pool = BoundedInstancePool::new(8);
    let key = base_key();

    assert!(pool.insert(key.clone(), 1).is_none());
    assert!(pool.insert(key.clone(), 2).is_none(), "replace, not evict");
    assert_eq!(pool.len(), 1, "one cache key → at most one instance");
    assert_eq!(pool.get(&key), Some(&2));
}

#[test]
fn bounded_pool_get_promotes_to_most_recently_used() {
    let mut pool = BoundedInstancePool::new(3);
    let keys: Vec<InstanceCacheKey> = (0..3)
        .map(|i| {
            InstanceCacheKey::new(
                format!("{i:064x}"),
                "hive-extism/v1",
                "extism",
                "1.30.0",
                None,
                30_000,
                128,
                10 * 1024 * 1024,
                "cap-policy-0000",
            )
        })
        .collect();

    pool.insert(keys[0].clone(), 0);
    pool.insert(keys[1].clone(), 1);
    pool.insert(keys[2].clone(), 2);

    // Touch keys[0] so it becomes MRU; then keys[1] is the LRU.
    assert_eq!(pool.get(&keys[0]), Some(&0));

    let key3 = InstanceCacheKey::new(
        format!("{:064x}", 3),
        "hive-extism/v1",
        "extism",
        "1.30.0",
        None,
        30_000,
        128,
        10 * 1024 * 1024,
        "cap-policy-0000",
    );
    let evicted = pool.insert(key3, 3);
    assert_eq!(evicted, Some(1), "keys[1] is the LRU victim after touch");
    assert!(pool.contains_key(&keys[0]));
    assert!(pool.contains_key(&keys[2]));
    assert!(!pool.contains_key(&keys[1]));
    assert_eq!(pool.len(), 3);
}

#[test]
fn bounded_pool_remove_releases_slot() {
    let mut pool = BoundedInstancePool::new(8);
    let key = base_key();
    pool.insert(key.clone(), 1);
    assert_eq!(pool.remove(&key), Some(1));
    assert!(pool.is_empty());
    assert_eq!(pool.remove(&key), None);
}
