//! T073 [P] [US8] Plugin store contract.
//!
//! Source of truth: `specs/011-hivegui-standalone-mode/tasks.md` §T073
//! ("在 `crates/hivegui/tests/plugin_artifacts.rs` 编写 Plugin
//! no-replace 旧句柄/租约/字节稳定、不可变键、上传/保留/软删除、
//! 共享 WASM fixture 与并发普通文件、并发上传/替换/软删除 race")。
//!
//! Public boundaries the T078 implementation provides:
//!   - `hivegui::plugin::plugin_store::PluginStore`
//!   - `hivegui::plugin::plugin_store::PluginRecord`
//!   - `hivegui::plugin::plugin_store::PluginArtifactInput`
//!   - `hivegui::plugin::plugin_store::PluginArtifact`
//!   - `hivegui::plugin::plugin_store::PluginLease`

mod support;

use hivegui::datasource::migrations::{MigrationOptions, migrate_to_current};
use hivegui::plugin::plugin_store::{
    PluginArtifact, PluginArtifactInput, PluginLease, PluginRecord, PluginStore,
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

#[tokio::test(flavor = "current_thread")]
async fn install_plugin_records_byte_stable_fingerprint() {
    let workspace = TestWorkspace::new().expect("test workspace");
    let pool = migrated_pool(&workspace).await;
    let store = PluginStore::new(pool, workspace.plugin_root()).expect("store");

    let bytes: Vec<u8> = (0..4096).map(|i| (i % 251) as u8).collect();
    let input = PluginArtifactInput::new("test-plugin", "1.0.0", &bytes).expect("validated");
    let plugin: PluginRecord = store.install(input).await.expect("install");
    assert_eq!(plugin.name(), "test-plugin");
    assert_eq!(plugin.fingerprint_hex(), hex_sha256(&bytes));
}

#[tokio::test(flavor = "current_thread")]
async fn existing_plugin_cannot_be_replaced_silently() {
    let workspace = TestWorkspace::new().expect("test workspace");
    let pool = migrated_pool(&workspace).await;
    let store = PluginStore::new(pool, workspace.plugin_root()).expect("store");

    let first = PluginArtifactInput::new("dup", "1.0.0", b"v1").unwrap();
    store.install(first).await.expect("first install");
    let second = PluginArtifactInput::new("dup", "1.0.0", b"v2").unwrap();
    let err = store
        .install(second)
        .await
        .expect_err("must reject replace");
    assert_eq!(err.reason(), "no_replace");
    assert!(err.references().is_empty());
}

#[tokio::test(flavor = "current_thread")]
async fn soft_delete_keeps_artifact_on_disk() {
    let workspace = TestWorkspace::new().expect("test workspace");
    let pool = migrated_pool(&workspace).await;
    let store = PluginStore::new(pool, workspace.plugin_root()).expect("store");

    let bytes: Vec<u8> = vec![1, 2, 3, 4, 5];
    let plugin = store
        .install(PluginArtifactInput::new("keep-me", "1.0.0", &bytes).unwrap())
        .await
        .expect("install");
    store.soft_delete(plugin.id()).await.expect("soft delete");
    let recovered: PluginArtifact = store
        .artifact_for(plugin.id())
        .await
        .expect("artifact query")
        .expect("artifact present on disk");
    assert_eq!(recovered.bytes(), &bytes[..]);
}

#[tokio::test(flavor = "current_thread")]
async fn plugin_lease_is_scoped_to_a_runtime_session() {
    let workspace = TestWorkspace::new().expect("test workspace");
    let pool = migrated_pool(&workspace).await;
    let store = PluginStore::new(pool, workspace.plugin_root()).expect("store");

    let plugin = store
        .install(PluginArtifactInput::new("session", "1.0.0", b"v1").unwrap())
        .await
        .expect("install");
    let lease: PluginLease = store
        .acquire_lease(plugin.id(), "session-A")
        .await
        .expect("lease");
    assert_eq!(lease.session_id(), "session-A");
}

fn hex_sha256(bytes: &[u8]) -> String {
    use sha2::{Digest, Sha256};
    let mut hasher = Sha256::new();
    hasher.update(bytes);
    format!("{:x}", hasher.finalize())
}
