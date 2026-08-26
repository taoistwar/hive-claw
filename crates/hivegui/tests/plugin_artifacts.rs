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

use hive_runtime_core::wasm::{WasmModuleShape, validate_wasm_shape};
use hivegui::datasource::entity_store::{
    NewArtifact, OperationTransition, Plugin, PluginArtifactLedger, PluginResourceLimits,
    PreparePluginOperation,
};
use hivegui::datasource::migrations::{MigrationOptions, migrate_to_current};
use hivegui::datasource::plugin_artifacts::OperationState;
use hivegui::plugin::plugin_store::{
    PluginArtifact, PluginArtifactInput, PluginLease, PluginMetadata, PluginRecord, PluginStore,
};
use support::TestWorkspace;
use uuid::Uuid;

const VALID_V1_WASM: &[u8] = include_bytes!("fixtures/plugins/shared-smoke/plugin.wasm");

async fn migrated_pool(workspace: &TestWorkspace) -> sqlx::SqlitePool {
    migrate_to_current(MigrationOptions::new(
        workspace.database_path(),
        workspace.plugin_root(),
    ))
    .await
    .expect("migrate to current");
    workspace.sqlite_pool().await.expect("sqlite pool")
}

async fn persisted_s3_key(pool: &sqlx::SqlitePool, plugin_id: i64) -> String {
    sqlx::query_scalar("SELECT s3_key FROM plugins WHERE id = ?")
        .bind(plugin_id)
        .fetch_one(pool)
        .await
        .expect("persisted plugin artifact key")
}

#[cfg(unix)]
fn artifact_identity(path: &std::path::Path) -> String {
    use std::os::unix::fs::MetadataExt;
    let metadata = std::fs::metadata(path).expect("staging metadata");
    format!(
        "unix-v1:{}:{}:{}:{}",
        metadata.dev(),
        metadata.ino(),
        metadata.ctime(),
        metadata.ctime_nsec()
    )
}

#[cfg(not(unix))]
fn artifact_identity(path: &std::path::Path) -> String {
    use std::time::UNIX_EPOCH;
    let metadata = std::fs::metadata(path).expect("staging metadata");
    let modified = metadata
        .modified()
        .ok()
        .and_then(|value| value.duration_since(UNIX_EPOCH).ok())
        .map_or(0, |duration| duration.as_nanos());
    format!("portable-v1:{}:{modified}", metadata.len())
}

#[tokio::test(flavor = "current_thread")]
async fn install_plugin_records_byte_stable_fingerprint() {
    let workspace = TestWorkspace::new().expect("test workspace");
    let pool = migrated_pool(&workspace).await;
    let store = PluginStore::new(pool, workspace.plugin_root()).expect("store");

    let bytes = VALID_V1_WASM;
    let input = PluginArtifactInput::new("test-plugin", "1.0.0", bytes).expect("validated");
    let plugin: PluginRecord = store.install(input).await.expect("install");
    assert_eq!(plugin.name(), "test-plugin");
    assert_eq!(plugin.fingerprint_hex(), hex_sha256(bytes));
}

#[tokio::test(flavor = "current_thread")]
async fn existing_plugin_cannot_be_replaced_silently() {
    let workspace = TestWorkspace::new().expect("test workspace");
    let pool = migrated_pool(&workspace).await;
    let store = PluginStore::new(pool, workspace.plugin_root()).expect("store");

    let first = PluginArtifactInput::new("dup", "1.0.0", VALID_V1_WASM).unwrap();
    store.install(first).await.expect("first install");
    let second = PluginArtifactInput::new("dup", "1.0.0", VALID_V1_WASM).unwrap();
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

    let bytes = VALID_V1_WASM;
    let plugin = store
        .install(PluginArtifactInput::new("keep-me", "1.0.0", bytes).unwrap())
        .await
        .expect("install");
    store.soft_delete(plugin.id()).await.expect("soft delete");
    let recovered: PluginArtifact = store
        .artifact_for(plugin.id())
        .await
        .expect("artifact query")
        .expect("artifact present on disk");
    assert_eq!(recovered.bytes(), bytes);
}

#[tokio::test(flavor = "current_thread")]
async fn plugin_lease_is_scoped_to_a_runtime_session() {
    let workspace = TestWorkspace::new().expect("test workspace");
    let pool = migrated_pool(&workspace).await;
    let store = PluginStore::new(pool, workspace.plugin_root()).expect("store");

    let plugin = store
        .install(PluginArtifactInput::new("session", "1.0.0", VALID_V1_WASM).unwrap())
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
async fn recovery_finishes_interrupted_prepared_and_staged_operations() {
    // T077: startup replay keeps durable operation history. A `prepared` row
    // with no staging object becomes `done`; a `staged` row whose owned bytes
    // match its durable identity is removed through the identity-bound path
    // and likewise becomes `done`.
    let workspace = TestWorkspace::new().expect("test workspace");
    let pool = migrated_pool(&workspace).await;
    let store = PluginStore::new(pool.clone(), workspace.plugin_root()).expect("store");
    let ledger = PluginArtifactLedger::new(pool.clone());
    let sha256 = hex_sha256(VALID_V1_WASM);

    let prepared_id = Uuid::from_u128(0x7701);
    ledger
        .prepare(
            prepared_id,
            PreparePluginOperation::Create {
                target_identifier: "recovery-prepared".to_string(),
                new: NewArtifact {
                    s3_key: format!("recovery-prepared/1.0.0/{prepared_id}/plugin.wasm"),
                    sha256: sha256.clone(),
                    size_bytes: VALID_V1_WASM.len() as i64,
                    resource_limits: PluginResourceLimits::default(),
                },
            },
        )
        .await
        .expect("seed typed prepared operation");

    let staged_id = Uuid::from_u128(0x7702);
    let staged = ledger
        .prepare(
            staged_id,
            PreparePluginOperation::Create {
                target_identifier: "recovery-staged".to_string(),
                new: NewArtifact {
                    s3_key: format!("recovery-staged/1.0.0/{staged_id}/plugin.wasm"),
                    sha256: sha256.clone(),
                    size_bytes: VALID_V1_WASM.len() as i64,
                    resource_limits: PluginResourceLimits::default(),
                },
            },
        )
        .await
        .expect("seed typed staged operation");
    let staging_dir = workspace.plugin_root().join(".staging");
    std::fs::create_dir_all(&staging_dir).expect("staging dir");
    let staged_path = staging_dir.join(staged.staging_name());
    std::fs::write(&staged_path, VALID_V1_WASM).expect("write owned staging bytes");
    let staging_identity = artifact_identity(&staged_path);
    ledger
        .transition(
            staged_id,
            OperationState::Prepared,
            OperationTransition::Staged { staging_identity },
        )
        .await
        .expect("durably record staged identity");

    let recovered = store
        .recover_interrupted_operations()
        .await
        .expect("recover");
    assert_eq!(
        recovered, 2,
        "both prepared and staged operations are recovered"
    );

    assert_eq!(
        ledger
            .get(prepared_id)
            .await
            .expect("read prepared recovery row")
            .expect("prepared recovery row remains")
            .state(),
        OperationState::Done,
        "prepared recovery must retain durable history as done"
    );
    assert_eq!(
        ledger
            .get(staged_id)
            .await
            .expect("read staged recovery row")
            .expect("staged recovery row remains")
            .state(),
        OperationState::Done,
        "staged recovery must retain durable history as done"
    );

    assert!(
        !staged_path.exists(),
        "identity-matched staging bytes must be removed during recovery"
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
async fn gc_registration_is_idempotent_and_operation_completion_is_terminal() {
    // T078: GC registration must be idempotent (a second insert of the
    // same key does not duplicate or change state), and completing an
    // operation must be terminal (a second call leaves `done` as-is).
    let workspace = TestWorkspace::new().expect("test workspace");
    let pool = migrated_pool(&workspace).await;

    // Seed a non-terminal `prepared` operation (identity columns NULL,
    // which satisfies the create/prepared CHECK).
    sqlx::query(
        "INSERT INTO plugin_artifact_operations (operation_id, kind, staging_name, state) \
         VALUES ('op-gc', 'create', 'staging-gc', 'prepared')",
    )
    .execute(&pool)
    .await
    .expect("seed operation");

    // Idempotent GC registration.
    Plugin::register_gc_artifact(
        &pool,
        "plugins/owned.wasm".to_string(),
        "orphaned".to_string(),
    )
    .await
    .expect("register gc");
    Plugin::register_gc_artifact(
        &pool,
        "plugins/owned.wasm".to_string(),
        "orphaned".to_string(),
    )
    .await
    .expect("register gc again");
    let gc_count: i64 =
        sqlx::query_scalar("SELECT COUNT(*) FROM plugin_artifact_gc WHERE artifact_key = ?")
            .bind("plugins/owned.wasm")
            .fetch_one(&pool)
            .await
            .expect("gc count");
    assert_eq!(gc_count, 1, "GC registration must be idempotent");

    // Terminal operation completion.
    Plugin::complete_operation(&pool, "op-gc")
        .await
        .expect("complete operation");
    Plugin::complete_operation(&pool, "op-gc")
        .await
        .expect("complete operation again");
    let state: String =
        sqlx::query_scalar("SELECT state FROM plugin_artifact_operations WHERE operation_id = ?")
            .bind("op-gc")
            .fetch_one(&pool)
            .await
            .expect("operation state");
    assert_eq!(state, "done", "operation completion must be terminal");
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
        .install(PluginArtifactInput::new("stateful", "1.0.0", VALID_V1_WASM).unwrap())
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
        // the `{identifier}/{version}/{id}` directory to it. The no-follow
        // boundary must reject it.
        let outside = workspace.root().join("outside");
        std::fs::create_dir_all(&outside).expect("outside dir");
        let artifact_dir = workspace
            .plugin_root()
            .join(Plugin::artifact_dir("evil", "1.0.0", 1));
        std::fs::create_dir_all(artifact_dir.parent().expect("parent dirs")).expect("parent dirs");
        std::os::unix::fs::symlink(&outside, &artifact_dir).expect("symlink");

        let err = store
            .resolve_artifact_path("evil", "1.0.0", 1)
            .expect_err("symlink identifier dir must be rejected");
        assert_eq!(err.reason(), "unsafe_artifact");
    }

    #[tokio::test(flavor = "current_thread")]
    async fn resolve_artifact_path_rejects_symlink_final_file() {
        let workspace = TestWorkspace::new().expect("test workspace");
        let pool = migrated_pool(&workspace).await;
        let store = PluginStore::new(pool, workspace.plugin_root()).expect("store");

        let artifact_dir = workspace
            .plugin_root()
            .join(Plugin::artifact_dir("linkme", "1.0.0", 1));
        std::fs::create_dir_all(&artifact_dir).expect("artifact dir");
        let outside_file = workspace.root().join("target.wasm");
        std::fs::write(&outside_file, b"v1").expect("outside file");
        std::os::unix::fs::symlink(&outside_file, artifact_dir.join("plugin.wasm"))
            .expect("final symlink");

        let err = store
            .resolve_artifact_path("linkme", "1.0.0", 1)
            .expect_err("symlink final file must be rejected");
        assert_eq!(err.reason(), "unsafe_artifact");
    }

    #[tokio::test(flavor = "current_thread")]
    async fn resolve_artifact_path_rejects_hardlinked_file() {
        let workspace = TestWorkspace::new().expect("test workspace");
        let pool = migrated_pool(&workspace).await;
        let store = PluginStore::new(pool, workspace.plugin_root()).expect("store");

        let artifact_dir = workspace
            .plugin_root()
            .join(Plugin::artifact_dir("hardlink", "1.0.0", 1));
        std::fs::create_dir_all(&artifact_dir).expect("artifact dir");
        let artifact = artifact_dir.join("plugin.wasm");
        std::fs::write(&artifact, b"v1").expect("artifact");
        let linked = workspace.root().join("hardlink-target");
        std::fs::hard_link(&artifact, &linked).expect("hard link");

        let err = store
            .resolve_artifact_path("hardlink", "1.0.0", 1)
            .expect_err("link-count-2 file must be rejected");
        assert_eq!(err.reason(), "unsafe_artifact");
    }

    #[tokio::test(flavor = "current_thread")]
    async fn persisted_artifact_key_accepts_regular_file() {
        let workspace = TestWorkspace::new().expect("test workspace");
        let pool = migrated_pool(&workspace).await;
        let store = PluginStore::new(pool.clone(), workspace.plugin_root()).expect("store");

        let plugin = store
            .install(PluginArtifactInput::new("ok", "1.0.0", VALID_V1_WASM).unwrap())
            .await
            .expect("install");

        let artifact_key = persisted_s3_key(&pool, plugin.id()).await;
        let path = workspace.plugin_root().join(&artifact_key);
        assert!(
            path.exists(),
            "persisted immutable key must address the file"
        );
        assert_eq!(
            store
                .read_verified_artifact_key(&artifact_key)
                .expect("regular persisted artifact must resolve safely"),
            VALID_V1_WASM
        );
    }

    #[tokio::test(flavor = "current_thread")]
    async fn read_verified_artifact_reads_bytes_through_nofollow_descriptor() {
        let workspace = TestWorkspace::new().expect("test workspace");
        let pool = migrated_pool(&workspace).await;
        let store = PluginStore::new(pool.clone(), workspace.plugin_root()).expect("store");

        let plugin = store
            .install(PluginArtifactInput::new("rdok", "1.0.0", VALID_V1_WASM).unwrap())
            .await
            .expect("install");

        let artifact_key = persisted_s3_key(&pool, plugin.id()).await;
        let bytes = store
            .read_verified_artifact_key(&artifact_key)
            .expect("regular file must be readable");
        assert_eq!(bytes, VALID_V1_WASM);
    }

    #[tokio::test(flavor = "current_thread")]
    async fn read_verified_artifact_rejects_symlink_final_file() {
        let workspace = TestWorkspace::new().expect("test workspace");
        let pool = migrated_pool(&workspace).await;
        let store = PluginStore::new(pool, workspace.plugin_root()).expect("store");

        let artifact_dir = workspace
            .plugin_root()
            .join(Plugin::artifact_dir("rdlink", "1.0.0", 1));
        std::fs::create_dir_all(&artifact_dir).expect("artifact dir");
        let outside_file = workspace.root().join("rd-target.wasm");
        std::fs::write(&outside_file, b"v1").expect("outside file");
        std::os::unix::fs::symlink(&outside_file, artifact_dir.join("plugin.wasm"))
            .expect("final symlink");

        // O_NOFOLLOW rejects the symlink at open time.
        let err = store
            .read_verified_artifact("rdlink", "1.0.0", 1)
            .expect_err("symlink final file must be rejected");
        assert_eq!(err.reason(), "unsafe_artifact");
    }

    #[tokio::test(flavor = "current_thread")]
    async fn read_verified_artifact_rejects_hardlinked_file() {
        let workspace = TestWorkspace::new().expect("test workspace");
        let pool = migrated_pool(&workspace).await;
        let store = PluginStore::new(pool, workspace.plugin_root()).expect("store");

        let artifact_dir = workspace
            .plugin_root()
            .join(Plugin::artifact_dir("rdhard", "1.0.0", 1));
        std::fs::create_dir_all(&artifact_dir).expect("artifact dir");
        let artifact = artifact_dir.join("plugin.wasm");
        std::fs::write(&artifact, b"v1").expect("artifact");
        let linked = workspace.root().join("rd-hardlink-target");
        std::fs::hard_link(&artifact, &linked).expect("hard link");

        let err = store
            .read_verified_artifact("rdhard", "1.0.0", 1)
            .expect_err("link-count-2 file must be rejected");
        assert_eq!(err.reason(), "unsafe_artifact");
    }
}

#[tokio::test(flavor = "current_thread")]
async fn delete_soft_deletes_and_gc_removes_artifact() {
    // T079 ③: `PluginStore::delete` must soft-delete the row, register the
    // artifact key for GC, and drain it so the on-disk file is removed
    // through the protected path (no immediate `remove_dir_all`).
    let workspace = TestWorkspace::new().expect("test workspace");
    let pool = migrated_pool(&workspace).await;
    let store = PluginStore::new(pool.clone(), workspace.plugin_root()).expect("store");

    let plugin = store
        .install(PluginArtifactInput::new("to-delete", "1.0.0", VALID_V1_WASM).unwrap())
        .await
        .expect("install");

    let key = persisted_s3_key(&pool, plugin.id()).await;
    let wasm_path = workspace.plugin_root().join(&key);
    assert!(wasm_path.exists(), "artifact must exist before delete");

    store.delete(plugin.id()).await.expect("delete");

    // Soft-deleted row + removed file + drained ledger.
    let deleted_at: Option<String> =
        sqlx::query_scalar("SELECT deleted_at FROM plugins WHERE id = ?")
            .bind(plugin.id())
            .fetch_one(&pool)
            .await
            .expect("deleted_at");
    assert!(deleted_at.is_some(), "plugin row must be soft-deleted");
    assert!(!wasm_path.exists(), "artifact file must be removed");

    let gc_count: i64 =
        sqlx::query_scalar("SELECT COUNT(*) FROM plugin_artifact_gc WHERE artifact_key = ?")
            .bind(&key)
            .fetch_one(&pool)
            .await
            .expect("gc count");
    assert_eq!(
        gc_count, 0,
        "drained GC row must be removed from the ledger"
    );
}

#[tokio::test(flavor = "current_thread")]
async fn drain_pending_gc_blocks_malformed_key_instead_of_deleting() {
    // T079 ③: a GC key that does not resolve to a safe 4-segment
    // `{identifier}/{version}/{id}/plugin.wasm` path must be marked `blocked`
    // (never deleted speculatively) by `drain_pending_gc`.
    let workspace = TestWorkspace::new().expect("test workspace");
    let pool = migrated_pool(&workspace).await;
    let store = PluginStore::new(pool.clone(), workspace.plugin_root()).expect("store");

    Plugin::register_gc_artifact(
        &pool,
        "plugins/owned.wasm".to_string(),
        "orphaned".to_string(),
    )
    .await
    .expect("register gc");

    let drained = store.drain_pending_gc().await.expect("drain");
    assert_eq!(drained, 0, "malformed key must not be drained");

    let state: String =
        sqlx::query_scalar("SELECT state FROM plugin_artifact_gc WHERE artifact_key = ?")
            .bind("plugins/owned.wasm")
            .fetch_one(&pool)
            .await
            .expect("gc state");
    assert_eq!(state, "blocked", "malformed key must be blocked");
}

fn artifact_file_count(root: &std::path::Path) -> usize {
    let Ok(entries) = std::fs::read_dir(root) else {
        return 0;
    };

    entries
        .map(|entry| entry.expect("read plugin artifact entry"))
        .map(|entry| {
            let file_type = entry.file_type().expect("read plugin artifact type");
            if file_type.is_dir() {
                artifact_file_count(&entry.path())
            } else {
                1
            }
        })
        .sum()
}

async fn plugin_install_footprint(
    pool: &sqlx::SqlitePool,
    plugin_root: &std::path::Path,
) -> (i64, i64, i64, usize) {
    let plugin_rows = sqlx::query_scalar("SELECT COUNT(*) FROM plugins")
        .fetch_one(pool)
        .await
        .expect("count plugin rows");
    let operation_rows = sqlx::query_scalar("SELECT COUNT(*) FROM plugin_artifact_operations")
        .fetch_one(pool)
        .await
        .expect("count operation rows");
    let gc_rows = sqlx::query_scalar("SELECT COUNT(*) FROM plugin_artifact_gc")
        .fetch_one(pool)
        .await
        .expect("count GC rows");

    (
        plugin_rows,
        operation_rows,
        gc_rows,
        artifact_file_count(plugin_root),
    )
}

#[tokio::test(flavor = "current_thread")]
async fn t077_red_published_replay_blocks_when_staging_and_final_both_exist() {
    // plugin-abi.md §6: `published` accepts exactly one shape: staging absent
    // and final matching the durable identity/size/hash. Double presence is an
    // ownership ambiguity, so replay must retain both objects, durably mark the
    // operation `conflict`, and fail closed before the Store can open.
    let workspace = TestWorkspace::new().expect("test workspace");
    let pool = migrated_pool(&workspace).await;
    let store = PluginStore::new(pool.clone(), workspace.plugin_root()).expect("store");

    let operation_id = "op-published-double-presence";
    let staging_name = hivegui::datasource::plugin_artifacts::derive_staging_name(operation_id);
    let bytes = b"owned-published-bytes";
    let sha256 = hex_sha256(bytes);
    let artifact_key = Plugin::artifact_key("published-double", "1.0.0", 4242);
    let staging_path = workspace.plugin_root().join(".staging").join(&staging_name);
    let final_path = workspace.plugin_root().join(&artifact_key);
    std::fs::create_dir_all(staging_path.parent().expect("staging parent"))
        .expect("create staging parent");
    std::fs::create_dir_all(final_path.parent().expect("final parent"))
        .expect("create final parent");
    std::fs::write(&staging_path, bytes).expect("write matching staging object");
    std::fs::write(&final_path, bytes).expect("write matching final object");

    sqlx::query(
        "INSERT INTO plugin_artifact_operations (\
             kind, new_identity, new_s3_key, new_sha256, new_size, operation_id, \
             staging_identity, staging_name, state\
         ) VALUES ('create', ?, ?, ?, ?, ?, ?, ?, 'published')",
    )
    .bind(&sha256)
    .bind(&artifact_key)
    .bind(&sha256)
    .bind(bytes.len() as i64)
    .bind(operation_id)
    .bind(&sha256)
    .bind(&staging_name)
    .execute(&pool)
    .await
    .expect("seed published operation with double presence");

    let recovery = store.recover_interrupted_operations().await;
    let state: Option<String> =
        sqlx::query_scalar("SELECT state FROM plugin_artifact_operations WHERE operation_id = ?")
            .bind(operation_id)
            .fetch_optional(&pool)
            .await
            .expect("read replay state");
    let staging_after = std::fs::read(&staging_path).ok();
    let final_after = std::fs::read(&final_path).ok();

    assert!(
        recovery.is_err()
            && state.as_deref() == Some("conflict")
            && staging_after.as_deref() == Some(bytes.as_slice())
            && final_after.as_deref() == Some(bytes.as_slice()),
        "published double presence must block replay and preserve both objects; \
         recovery={recovery:?}, state={state:?}, staging_after={staging_after:?}, \
         final_after={final_after:?}"
    );
}

#[tokio::test(flavor = "current_thread")]
async fn t077_red_missing_abi_export_is_rejected_before_any_ledger_write() {
    // Keep the shared fixture immutable: mutate an owned byte copy without
    // changing its length. The manifest remains a valid v1 declaration, so the
    // only incompatibility is the artifact's missing reserved ABI export.
    const ABI_EXPORT: &[u8] = b"_hive_plugin_abi_version";
    const CORRUPTED_ABI_EXPORT: &[u8] = b"_hive_plugin_abi_versi0n";
    assert_eq!(ABI_EXPORT.len(), CORRUPTED_ABI_EXPORT.len());

    let mut bytes = include_bytes!("fixtures/plugins/shared-smoke/plugin.wasm").to_vec();
    let export_range = wasmparser::Parser::new(0)
        .parse_all(&bytes)
        .find_map(|payload| match payload.expect("parse shared fixture") {
            wasmparser::Payload::ExportSection(reader) => Some(reader.range()),
            _ => None,
        })
        .expect("shared fixture export section");
    let offsets = bytes[export_range.clone()]
        .windows(ABI_EXPORT.len())
        .enumerate()
        .filter_map(|(offset, candidate)| {
            (candidate == ABI_EXPORT).then_some(export_range.start + offset)
        })
        .collect::<Vec<_>>();
    assert_eq!(
        offsets.len(),
        1,
        "shared fixture export section must carry exactly one reserved ABI export name"
    );
    let offset = offsets[0];
    bytes[offset..offset + ABI_EXPORT.len()].copy_from_slice(CORRUPTED_ABI_EXPORT);
    assert_eq!(
        validate_wasm_shape(&bytes, false).rejection_kind(),
        Some(WasmModuleShape::MissingAbiVersionExport),
        "private fixture copy must isolate the missing-ABI-export shape"
    );

    let workspace = TestWorkspace::new().expect("test workspace");
    let pool = migrated_pool(&workspace).await;
    let store = PluginStore::new(pool.clone(), workspace.plugin_root()).expect("store");
    let valid_manifest = r#"{
        "abi_version":"hive-extism/v1",
        "required_capabilities":[],
        "exports":[{"name":"echo","input":"json","output":"json"}]
    }"#;
    let metadata = PluginMetadata::new(
        "Missing ABI export",
        None,
        Some(valid_manifest.to_owned()),
        "extism",
        "[]",
        "{}",
    );

    let result = store
        .install_with_metadata(
            PluginArtifactInput::new("missing-abi-export", "1.0.0", &bytes).expect("input"),
            metadata,
        )
        .await;
    let footprint = plugin_install_footprint(&pool, workspace.plugin_root()).await;

    assert!(
        result.is_err() && footprint == (0, 0, 0, 0),
        "missing reserved ABI export must fail before Plugin/operation/GC/artifact writes; \
         result={result:?}, footprint=(plugins, operations, gc, files)={footprint:?}"
    );
}

#[tokio::test(flavor = "current_thread")]
async fn t077_red_invalid_manifest_is_rejected_before_any_ledger_write() {
    // Use the structurally valid shared fixture so this Red isolates manifest
    // ABI validation and its required pre-ledger ordering.
    let workspace = TestWorkspace::new().expect("test workspace");
    let pool = migrated_pool(&workspace).await;
    let store = PluginStore::new(pool.clone(), workspace.plugin_root()).expect("store");
    let bytes = include_bytes!("fixtures/plugins/shared-smoke/plugin.wasm");
    let unsupported_manifest = r#"{
        "abi_version":"hive-extism/v2",
        "required_capabilities":[],
        "exports":[{"name":"echo","input":"json","output":"json"}]
    }"#;
    let metadata = PluginMetadata::new(
        "Invalid manifest",
        None,
        Some(unsupported_manifest.to_owned()),
        "extism",
        "[]",
        "{}",
    );

    let result = store
        .install_with_metadata(
            PluginArtifactInput::new("invalid-manifest", "1.0.0", bytes).expect("input"),
            metadata,
        )
        .await;
    let footprint = plugin_install_footprint(&pool, workspace.plugin_root()).await;

    assert!(
        result.is_err() && footprint == (0, 0, 0, 0),
        "unsupported manifest ABI must fail before Plugin/operation/GC/artifact writes; \
         result={result:?}, footprint=(plugins, operations, gc, files)={footprint:?}"
    );
}

#[tokio::test(flavor = "current_thread")]
async fn t077_red_unavailable_capability_is_rejected_before_any_ledger_write() {
    // A valid v1 manifest that asks for a capability absent from the desktop
    // host catalog must be rejected with the same zero-footprint guarantee.
    let workspace = TestWorkspace::new().expect("test workspace");
    let pool = migrated_pool(&workspace).await;
    let store = PluginStore::new(pool.clone(), workspace.plugin_root()).expect("store");
    let bytes = include_bytes!("fixtures/plugins/shared-smoke/plugin.wasm");
    let unavailable = "t077.capability.never.registered";
    let unavailable_count: i64 =
        sqlx::query_scalar("SELECT COUNT(*) FROM capabilities WHERE name = ?")
            .bind(unavailable)
            .fetch_one(&pool)
            .await
            .expect("verify unavailable capability precondition");
    assert_eq!(unavailable_count, 0, "negative capability must be absent");

    let manifest = format!(
        r#"{{
            "abi_version":"hive-extism/v1",
            "required_capabilities":["{unavailable}"],
            "exports":[{{"name":"echo","input":"json","output":"json"}}]
        }}"#
    );
    let metadata = PluginMetadata::new(
        "Unavailable capability",
        None,
        Some(manifest),
        "extism",
        format!(r#"["{unavailable}"]"#),
        "{}",
    );

    let result = store
        .install_with_metadata(
            PluginArtifactInput::new("unavailable-capability", "1.0.0", bytes).expect("input"),
            metadata,
        )
        .await;
    let footprint = plugin_install_footprint(&pool, workspace.plugin_root()).await;

    assert!(
        result.is_err() && footprint == (0, 0, 0, 0),
        "unavailable capability must fail before Plugin/operation/GC/artifact writes; \
         result={result:?}, footprint=(plugins, operations, gc, files)={footprint:?}"
    );
}
