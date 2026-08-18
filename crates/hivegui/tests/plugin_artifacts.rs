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

use hivegui::datasource::entity_store::Plugin;
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

#[tokio::test(flavor = "current_thread")]
async fn recovery_clears_interrupted_prepared_and_staged_operations() {
    // T077: startup replay must roll back non-terminal operations left
    // by a crash. A `prepared` row (no bytes on disk) and a `staged`
    // row (staging bytes on disk) must both be cleared.
    let workspace = TestWorkspace::new().expect("test workspace");
    let pool = migrated_pool(&workspace).await;
    let store = PluginStore::new(pool.clone(), workspace.plugin_root()).expect("store");

    // Seed a `prepared` operation (no staging bytes).
    sqlx::query(
        "INSERT INTO plugin_artifact_operations (operation_id, kind, staging_name, state) \
         VALUES ('op-prepared', 'create', 'staging-prepared', 'prepared')",
    )
    .execute(&pool)
    .await
    .expect("seed prepared");

    // Seed a `staged` operation with real staging bytes on disk.
    let staging_dir = workspace.plugin_root().join(".staging");
    std::fs::create_dir_all(&staging_dir).expect("staging dir");
    std::fs::write(staging_dir.join("staging-staged"), b"partial").expect("staging bytes");
    sqlx::query(
        "INSERT INTO plugin_artifact_operations (operation_id, kind, staging_name, staging_identity, state) \
         VALUES ('op-staged', 'create', 'staging-staged', 'deadbeef', 'staged')",
    )
    .execute(&pool)
    .await
    .expect("seed staged");

    let recovered = store
        .recover_interrupted_operations()
        .await
        .expect("recover");
    assert_eq!(
        recovered, 2,
        "both prepared and staged operations are recovered"
    );

    let remaining: i64 = sqlx::query_scalar(
        "SELECT COUNT(*) FROM plugin_artifact_operations WHERE operation_id IN ('op-prepared','op-staged')",
    )
    .fetch_one(&pool)
    .await
    .expect("count remaining");
    assert_eq!(remaining, 0, "interrupted operations must be cleared");

    // The staged bytes on disk must be removed.
    assert!(
        !staging_dir.join("staging-staged").exists(),
        "staging bytes of a rolled-back staged operation must be removed"
    );
}

#[tokio::test(flavor = "current_thread")]
async fn replace_cas_switches_artifact_and_increments_revision() {
    // T078: the replace CAS must switch s3_key/size/SHA and increment
    // row_revision only when the exact revision matches; a stale revision
    // returns None (stable concurrent-conflict signal), never a partial
    // write.
    let workspace = TestWorkspace::new().expect("test workspace");
    let pool = migrated_pool(&workspace).await;

    let plugin = Plugin::create(
        &pool,
        "cas-plugin".to_string(),
        "CAS Plugin".to_string(),
        None,
        None,
        "wasm32".to_string(),
        "1.0.0".to_string(),
        None,
        None,
        "plugins/cas/1.0.0.wasm".to_string(),
        "old-sha".to_string(),
        1024,
        None,
    )
    .await
    .expect("create plugin");

    assert_eq!(plugin.row_revision, 0, "fresh plugin starts at revision 0");

    let replaced = Plugin::replace(
        &pool,
        plugin.id,
        plugin.row_revision,
        "plugins/cas/2.0.0.wasm".to_string(),
        "new-sha".to_string(),
        2048,
    )
    .await
    .expect("replace");
    let replaced = replaced.expect("revision matches so replace must succeed");
    assert_eq!(replaced.row_revision, 1, "replace increments row_revision");
    assert_eq!(replaced.s3_key, "plugins/cas/2.0.0.wasm");
    assert_eq!(replaced.sha256, "new-sha");
    assert_eq!(replaced.size_bytes, 2048);

    // A stale revision must not match; the row stays at revision 1.
    let conflict = Plugin::replace(
        &pool,
        plugin.id,
        0, // stale revision
        "plugins/cas/3.0.0.wasm".to_string(),
        "other-sha".to_string(),
        4096,
    )
    .await
    .expect("replace");
    assert!(
        conflict.is_none(),
        "stale revision must yield a CAS conflict"
    );

    let after = Plugin::get(&pool, plugin.id)
        .await
        .expect("get")
        .expect("plugin still exists");
    assert_eq!(
        after.row_revision, 1,
        "conflicting replace must not bump revision"
    );
    assert_eq!(
        after.sha256, "new-sha",
        "conflicting replace must not change SHA"
    );
}

#[tokio::test(flavor = "current_thread")]
async fn install_walks_full_durability_state_machine_to_done() {
    // T077: install must record the full
    // prepared → staged → published → referenced → done state machine
    // in `plugin_artifact_operations`, ending in `done`.
    let workspace = TestWorkspace::new().expect("test workspace");
    let pool = migrated_pool(&workspace).await;
    let store = PluginStore::new(pool.clone(), workspace.plugin_root()).expect("store");

    let plugin = store
        .install(PluginArtifactInput::new("stateful", "1.0.0", b"v1").unwrap())
        .await
        .expect("install");

    let (state, staging_identity, new_identity, plugin_id): (
        String,
        Option<String>,
        Option<String>,
        Option<String>,
    ) = sqlx::query_as(
        "SELECT state, staging_identity, new_identity, plugin_id \
         FROM plugin_artifact_operations ORDER BY rowid DESC LIMIT 1",
    )
    .fetch_one(&pool)
    .await
    .expect("latest operation row");

    assert_eq!(state, "done", "install must terminate in done");
    assert!(
        staging_identity.is_some(),
        "staging_identity must be recorded before done"
    );
    assert!(
        new_identity.is_some(),
        "new_identity must be recorded before done"
    );
    assert_eq!(
        plugin_id.as_deref(),
        Some(plugin.id().to_string().as_str()),
        "referenced plugin_id must match the installed row"
    );
}

#[cfg(unix)]
mod no_follow_tests {
    use super::*;

    #[tokio::test(flavor = "current_thread")]
    async fn resolve_artifact_path_rejects_symlink_directory() {
        let workspace = TestWorkspace::new().expect("test workspace");
        let pool = migrated_pool(&workspace).await;
        let store = PluginStore::new(pool, workspace.plugin_root()).expect("store");

        // Create a real directory outside the store root, then symlink
        // `identifier` to it. The no-follow boundary must reject it.
        let outside = workspace.root().join("outside");
        std::fs::create_dir_all(&outside).expect("outside dir");
        let identifier_dir = workspace.plugin_root().join("evil");
        std::os::unix::fs::symlink(&outside, &identifier_dir).expect("symlink");

        let err = store
            .resolve_artifact_path("evil", "1.0.0")
            .expect_err("symlink identifier dir must be rejected");
        assert_eq!(err.reason(), "unsafe_artifact");
    }

    #[tokio::test(flavor = "current_thread")]
    async fn resolve_artifact_path_rejects_symlink_final_file() {
        let workspace = TestWorkspace::new().expect("test workspace");
        let pool = migrated_pool(&workspace).await;
        let store = PluginStore::new(pool, workspace.plugin_root()).expect("store");

        let identifier_dir = workspace.plugin_root().join("linkme");
        std::fs::create_dir_all(&identifier_dir).expect("identifier dir");
        let outside_file = workspace.root().join("target.wasm");
        std::fs::write(&outside_file, b"v1").expect("outside file");
        std::os::unix::fs::symlink(&outside_file, identifier_dir.join("1.0.0.wasm"))
            .expect("final symlink");

        let err = store
            .resolve_artifact_path("linkme", "1.0.0")
            .expect_err("symlink final file must be rejected");
        assert_eq!(err.reason(), "unsafe_artifact");
    }

    #[tokio::test(flavor = "current_thread")]
    async fn resolve_artifact_path_rejects_hardlinked_file() {
        let workspace = TestWorkspace::new().expect("test workspace");
        let pool = migrated_pool(&workspace).await;
        let store = PluginStore::new(pool, workspace.plugin_root()).expect("store");

        let identifier_dir = workspace.plugin_root().join("hardlink");
        std::fs::create_dir_all(&identifier_dir).expect("identifier dir");
        let artifact = identifier_dir.join("1.0.0.wasm");
        std::fs::write(&artifact, b"v1").expect("artifact");
        let linked = workspace.root().join("hardlink-target");
        std::fs::hard_link(&artifact, &linked).expect("hard link");

        let err = store
            .resolve_artifact_path("hardlink", "1.0.0")
            .expect_err("link-count-2 file must be rejected");
        assert_eq!(err.reason(), "unsafe_artifact");
    }

    #[tokio::test(flavor = "current_thread")]
    async fn resolve_artifact_path_accepts_regular_file() {
        let workspace = TestWorkspace::new().expect("test workspace");
        let pool = migrated_pool(&workspace).await;
        let store = PluginStore::new(pool, workspace.plugin_root()).expect("store");

        let plugin = store
            .install(PluginArtifactInput::new("ok", "1.0.0", b"v1").unwrap())
            .await
            .expect("install");

        let path = store
            .resolve_artifact_path("ok", "1.0.0")
            .expect("regular file must resolve");
        assert!(path.exists());
        let _ = plugin;
    }
}
