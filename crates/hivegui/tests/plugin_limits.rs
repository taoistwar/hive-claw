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

#[path = "support/plugin_wat.rs"]
mod plugin_wat;

use hivegui::datasource::{
    entity_store::{Capability, Function, Plugin},
    migrations::{MigrationOptions, migrate_to_current},
    plugin_manifest::build_v1_manifest,
};
use hivegui::plugin::plugin_store::{PluginArtifactInput, PluginMetadata, PluginStore};
use hivegui::runtime::{
    FunctionTestExecutor,
    plugin_executor::{
        BoundedInstancePool, DEFAULT_MEMORY_MB, DEFAULT_OUTPUT_BYTES, DEFAULT_TIMEOUT_SECS,
        HARD_MAX_MEMORY_MB, HARD_MAX_OUTPUT_BYTES, HARD_MAX_TIMEOUT_SECS, InstanceCacheKey,
        PAGES_PER_MIB, PageCount, PluginExecutor, PluginLimits, WASM_PAGE_BYTES, sha256_hex,
    },
};
use std::{collections::HashSet, fmt::Write as _, path::PathBuf};
use support::{CapturedHttpServer, MockHttpResponse, TestWorkspace};

async fn migrated_pool(workspace: &TestWorkspace) -> sqlx::SqlitePool {
    migrate_to_current(MigrationOptions::new(
        workspace.database_path(),
        workspace.plugin_root(),
    ))
    .await
    .expect("migrate to current");
    workspace.sqlite_pool().await.expect("sqlite pool")
}

async fn installed_artifact(
    pool: &sqlx::SqlitePool,
    workspace: &TestWorkspace,
    plugin_id: i64,
) -> (String, PathBuf) {
    let artifact_key: String = sqlx::query_scalar("SELECT s3_key FROM plugins WHERE id = ?")
        .bind(plugin_id)
        .fetch_one(pool)
        .await
        .expect("query persisted plugin artifact key");
    let artifact_path = workspace.plugin_root().join(&artifact_key);
    (artifact_key, artifact_path)
}

fn custom_plugin_function(plugin_id: i64, export_name: &str) -> Function {
    Function {
        id: 79,
        identifier: "t079-pool-probe".into(),
        name: "T079 pool probe".into(),
        description: None,
        kind: "custom".into(),
        input_schema: "{}".into(),
        output_schema: "{}".into(),
        plugin_id: Some(plugin_id),
        plugin_export: Some(export_name.into()),
        category_id: None,
        required_capabilities: None,
        created_at: String::new(),
        updated_at: String::new(),
    }
}

fn v1_metadata(name: &str, exports: &[&str], required_capabilities: &[&str]) -> PluginMetadata {
    let exports: Vec<String> = exports.iter().map(|value| (*value).to_string()).collect();
    let required_capabilities: Vec<String> = required_capabilities
        .iter()
        .map(|value| (*value).to_string())
        .collect();
    PluginMetadata::new(
        name,
        None,
        Some(build_v1_manifest(&exports, &required_capabilities)),
        "extism",
        serde_json::to_string(&required_capabilities).expect("serialize capabilities"),
        "{}",
    )
}

fn minimal_gc_probe_wasm() -> Vec<u8> {
    plugin_wat::compile_v1(
        "",
        r#"
          (func (export "gc_probe") (result i32)
            i32.const 0)
        "#,
    )
}

fn instance_reuse_probe_wasm(callback_url: &str) -> Vec<u8> {
    let envelope = serde_json::json!({
        "capability": "network.http",
        "args": {
            "method": "GET",
            "url": callback_url,
            "timeout_ms": 1_000,
        }
    })
    .to_string();
    let mut stores = String::new();
    for (offset, byte) in envelope.bytes().enumerate() {
        writeln!(
            stores,
            "(call $abi_store_u8 (i64.add (local.get $request) (i64.const {offset})) (i32.const {byte}))"
        )
        .expect("write WAT byte store");
    }
    let functions = format!(
        r#"
          (global $instance_already_observed (mut i32) (i32.const 0))
          (func (export "pool_probe") (result i32)
            (local $request i64)
            (local $result i64)
            (if (i32.eqz (global.get $instance_already_observed))
              (then
                (local.set $request (call $abi_alloc (i64.const {length})))
                {stores}
                (drop (call $pool_host_call (local.get $request)))
                (global.set $instance_already_observed (i32.const 1))
              )
            )
            (local.set $result (call $abi_alloc (i64.const 2)))
            (call $abi_store_u8 (local.get $result) (i32.const 123))
            (call $abi_store_u8 (i64.add (local.get $result) (i64.const 1)) (i32.const 125))
            (call $abi_output_set (local.get $result) (i64.const 2))
            i32.const 0
          )
        "#,
        length = envelope.len(),
    );
    plugin_wat::compile_v1(
        r#"(import "extism:host/user" "host_call"
             (func $pool_host_call (param i64) (result i64)))"#,
        &functions,
    )
}

async fn gc_state(pool: &sqlx::SqlitePool, artifact_key: &str) -> Option<String> {
    sqlx::query_scalar("SELECT state FROM plugin_artifact_gc WHERE artifact_key = ?")
        .bind(artifact_key)
        .fetch_optional(pool)
        .await
        .expect("query GC ledger state")
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

#[test]
fn sha256_hex_is_deterministic_lowercase() {
    let digest = sha256_hex(b"plugin.wasm");
    assert_eq!(digest.len(), 64);
    assert_eq!(digest, sha256_hex(b"plugin.wasm"));
    assert_ne!(digest, sha256_hex(b"plugin.wasm!"));
    assert!(
        digest
            .chars()
            .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit()),
        "digest must be lower-case hex: {digest}"
    );
}

#[tokio::test(flavor = "current_thread")]
async fn execute_with_verified_artifact_rejects_mismatched_digest() {
    let workspace = TestWorkspace::new().expect("test workspace");
    let wasm = wat::parse_str(r#"(module (func (export "run")))"#).expect("build wasm");
    let wasm_path = workspace.root().join("mismatch.wasm");
    std::fs::write(&wasm_path, &wasm).expect("write wasm");

    let err = PluginExecutor::execute_with_verified_artifact(
        &wasm_path,
        "run",
        "{}",
        std::time::Duration::from_secs(2),
        Vec::new(),
        &"0".repeat(64),
    )
    .await
    .unwrap_err();

    assert!(
        err.contains("WASM 制品校验失败"),
        "digest mismatch must be rejected before execution: {err}"
    );
}

#[tokio::test(flavor = "current_thread")]
async fn production_executor_reuses_same_full_key_and_misses_on_capability_policy_change() {
    let server = CapturedHttpServer::spawn(vec![
        MockHttpResponse::json(
            200,
            serde_json::json!({"ok": true})
        );
        4
    ])
    .await
    .expect("start instantiation counter");
    let wasm = instance_reuse_probe_wasm(&format!("{}/instantiated", server.base_url()));

    let workspace = TestWorkspace::new().expect("test workspace");
    let pool = migrated_pool(&workspace).await;
    Capability::create(
        &pool,
        "network.http".to_string(),
        "Test-local HTTP host capability".to_string(),
        true,
        None,
    )
    .await
    .expect("seed DB-local network.http capability");
    let store = PluginStore::new(pool.clone(), workspace.plugin_root()).expect("store");
    let plugin = store
        .install_with_metadata(
            PluginArtifactInput::new("production-pool", "1.0.0", &wasm).expect("plugin input"),
            v1_metadata("Production pool probe", &["pool_probe"], &["network.http"]),
        )
        .await
        .expect("install probe plugin");
    let function = custom_plugin_function(plugin.id(), "pool_probe");

    let policy_a = vec!["network.http".to_string()];
    let policy_b = vec!["network.http".to_string(), "time.now".to_string()];
    for policy in [policy_a.clone(), policy_a, policy_b.clone(), policy_b] {
        let executor = FunctionTestExecutor::new(workspace.plugin_root(), pool.clone());
        executor
            .execute_with_capabilities(&function, serde_json::json!({}), policy)
            .await
            .expect("healthy ABI probe execution");
    }

    assert_eq!(
        server.requests().len(),
        2,
        "the production FunctionTestExecutor→PluginExecutor path must instantiate once for the repeated full key and once after the capability-policy key changes"
    );
}

#[tokio::test(flavor = "current_thread")]
async fn active_lease_blocks_gc_then_release_allows_ledger_to_converge() {
    let workspace = TestWorkspace::new().expect("test workspace");
    let pool = migrated_pool(&workspace).await;
    let store = PluginStore::new(pool.clone(), workspace.plugin_root()).expect("store");
    let wasm = minimal_gc_probe_wasm();
    let plugin = store
        .install_with_metadata(
            PluginArtifactInput::new("leased-gc", "1.0.0", &wasm).expect("plugin input"),
            v1_metadata("Leased GC probe", &["gc_probe"], &[]),
        )
        .await
        .expect("install leased plugin");
    let (artifact_key, artifact_path) = installed_artifact(&pool, &workspace, plugin.id()).await;
    let lease = store
        .acquire_lease(plugin.id(), "active-runtime-session")
        .await
        .expect("acquire runtime lease");

    store.delete(plugin.id()).await.expect("soft delete");

    assert!(
        artifact_path.exists(),
        "GC must not unlink an artifact while a runtime lease is active"
    );
    assert!(
        matches!(
            gc_state(&pool, &artifact_key).await.as_deref(),
            Some("pending" | "blocked")
        ),
        "the protected artifact must remain represented by a retryable GC ledger row"
    );

    drop(lease);
    store
        .drain_pending_gc()
        .await
        .expect("retry GC after lease release");

    assert!(
        !artifact_path.exists(),
        "the artifact must be removed once the last runtime lease is released"
    );
    assert_eq!(
        gc_state(&pool, &artifact_key).await,
        None,
        "successful post-release GC must clear its ledger row"
    );
}

#[tokio::test(flavor = "current_thread")]
async fn live_metadata_reference_keeps_gc_retryable_without_unlinking() {
    let workspace = TestWorkspace::new().expect("test workspace");
    let pool = migrated_pool(&workspace).await;
    let store = PluginStore::new(pool.clone(), workspace.plugin_root()).expect("store");
    let wasm = minimal_gc_probe_wasm();
    let plugin = store
        .install_with_metadata(
            PluginArtifactInput::new("live-reference", "1.0.0", &wasm).expect("plugin input"),
            v1_metadata("Live reference GC probe", &["gc_probe"], &[]),
        )
        .await
        .expect("install referenced plugin");
    let (artifact_key, artifact_path) = installed_artifact(&pool, &workspace, plugin.id()).await;
    Plugin::register_gc_artifact(&pool, artifact_key.clone(), "reference-probe".into())
        .await
        .expect("register GC probe");

    let drained = store.drain_pending_gc().await.expect("scan GC ledger");

    assert_eq!(drained, 0, "a live metadata reference is not collectible");
    assert!(
        artifact_path.exists(),
        "GC must preserve bytes still referenced by live metadata"
    );
    assert_eq!(
        gc_state(&pool, &artifact_key).await.as_deref(),
        Some("pending"),
        "a transient live reference must stay retryable"
    );
}

#[tokio::test(flavor = "current_thread")]
async fn identity_reappearance_blocks_gc_without_deleting_competitor_bytes() {
    let workspace = TestWorkspace::new().expect("test workspace");
    let pool = migrated_pool(&workspace).await;
    let store = PluginStore::new(pool.clone(), workspace.plugin_root()).expect("store");
    let wasm = minimal_gc_probe_wasm();
    let plugin = store
        .install_with_metadata(
            PluginArtifactInput::new("identity-gc", "1.0.0", &wasm).expect("plugin input"),
            v1_metadata("Identity GC probe", &["gc_probe"], &[]),
        )
        .await
        .expect("install identity-bound plugin");
    let (artifact_key, artifact_path) = installed_artifact(&pool, &workspace, plugin.id()).await;
    store.soft_delete(plugin.id()).await.expect("soft delete");
    Plugin::register_gc_artifact(&pool, artifact_key.clone(), "identity-probe".into())
        .await
        .expect("register owned identity");

    let displaced = workspace.root().join("displaced-owned-artifact.wasm");
    std::fs::rename(&artifact_path, &displaced).expect("move the originally owned identity");
    let competitor = b"competitor-reappeared-at-same-key";
    let replacement = artifact_path.with_extension("replacement");
    std::fs::write(&replacement, competitor).expect("stage competitor identity");
    std::fs::rename(&replacement, &artifact_path).expect("publish competitor identity");

    let drained = store
        .drain_pending_gc()
        .await
        .expect("scan replaced identity");

    assert_eq!(drained, 0, "identity reappearance must not be adopted");
    assert_eq!(
        gc_state(&pool, &artifact_key).await.as_deref(),
        Some("blocked"),
        "identity mismatch must stay blocked for explicit operator handling"
    );
    assert_eq!(
        std::fs::read(&artifact_path).expect("competitor remains present"),
        competitor,
        "GC must never delete or rewrite bytes owned by the competing identity"
    );
}
