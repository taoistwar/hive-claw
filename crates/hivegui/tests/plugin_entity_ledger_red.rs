//! T078 [US8] entity-store Plugin artifact-ledger Red contracts.
//!
//! These tests deliberately stay below the filesystem-owned T077 boundary:
//! they exercise the migrated v4 SQLite schema and the production
//! `entity_store` public APIs only. No artifact is opened or deleted here.

mod support;

use hivegui::datasource::entity_store::{
    CreatePluginFields, ExpectedPluginRevision, FinishOutcome, NewArtifact, OperationGcTarget,
    OperationTransition, Plugin, PluginArtifactLedger, PluginResourceLimits,
    PreparePluginOperation, ReplaceOutcome,
};
use hivegui::datasource::migrations::{MigrationOptions, migrate_to_current};
use hivegui::datasource::plugin_artifacts::{
    GcState, OperationKind, OperationState, derive_staging_name,
};
use support::TestWorkspace;
use uuid::Uuid;

const SHA_OLD: &str = "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa";
const SHA_NEW: &str = "bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb";
const OLD_IDENTITY: &str = "old-file-identity";
const STAGING_IDENTITY: &str = "staging-file-identity";
const NEW_IDENTITY: &str = "new-file-identity";

async fn migrated_pool(workspace: &TestWorkspace) -> sqlx::SqlitePool {
    migrate_to_current(MigrationOptions::new(
        workspace.database_path(),
        workspace.plugin_root(),
    ))
    .await
    .expect("migrate v4 fixture");
    workspace.sqlite_pool().await.expect("sqlite pool")
}

async fn seed_plugin(pool: &sqlx::SqlitePool, identifier: &str, s3_key: &str) -> Plugin {
    let id = sqlx::query_scalar::<_, i64>(
        "INSERT INTO plugins (identifier, name, runtime, version, author, repository_url, \
         s3_key, sha256, size_bytes, capabilities, resource_limits, row_revision, \
         created_at, updated_at) \
         VALUES (?, ?, 'extism', '1.0.0', '', '', ?, ?, 1024, '[]', '{}', 0, ?, ?) \
         RETURNING id",
    )
    .bind(identifier)
    .bind(identifier)
    .bind(s3_key)
    .bind(SHA_OLD)
    .bind("2026-08-20T00:00:00Z")
    .bind("2026-08-20T00:00:00Z")
    .fetch_one(pool)
    .await
    .expect("seed plugin");
    Plugin::get(pool, id)
        .await
        .expect("read plugin")
        .expect("seeded plugin exists")
}

fn new_artifact(s3_key: &str, size_bytes: i64) -> NewArtifact {
    NewArtifact {
        s3_key: s3_key.to_string(),
        sha256: SHA_NEW.to_string(),
        size_bytes,
        resource_limits: PluginResourceLimits::default(),
    }
}

fn create_fields(name: &str) -> CreatePluginFields {
    CreatePluginFields {
        version: "1.0.0".to_string(),
        name: name.to_string(),
        description: None,
        manifest: None,
        runtime: "extism".to_string(),
        author: None,
        repository_url: None,
        category_id: None,
        capabilities: "[]".to_string(),
    }
}

async fn publish_create(
    ledger: &PluginArtifactLedger,
    operation_id: Uuid,
    identifier: &str,
    new_s3_key: &str,
) {
    let prepared = ledger
        .prepare(
            operation_id,
            PreparePluginOperation::Create {
                target_identifier: identifier.to_string(),
                new: new_artifact(new_s3_key, 1024),
            },
        )
        .await
        .expect("prepare create operation");
    assert_eq!(prepared.state(), OperationState::Prepared);

    ledger
        .transition(
            operation_id,
            OperationState::Prepared,
            OperationTransition::Staged {
                staging_identity: STAGING_IDENTITY.to_string(),
            },
        )
        .await
        .expect("record staged identity");
    ledger
        .transition(
            operation_id,
            OperationState::Staged,
            OperationTransition::Published {
                new_identity: NEW_IDENTITY.to_string(),
            },
        )
        .await
        .expect("record published identity");
}

async fn publish_replace(
    ledger: &PluginArtifactLedger,
    plugin: &Plugin,
    operation_id: Uuid,
    new_s3_key: &str,
) {
    ledger
        .prepare(
            operation_id,
            PreparePluginOperation::Replace {
                expected_old: ExpectedPluginRevision {
                    plugin_id: plugin.id,
                    identifier: plugin.identifier.clone(),
                    version: plugin.version.clone(),
                    s3_key: plugin.s3_key.clone(),
                    sha256: plugin.sha256.clone(),
                    size_bytes: plugin.size_bytes,
                    identity: OLD_IDENTITY.to_string(),
                    resource_limits: PluginResourceLimits::default(),
                    row_revision: plugin.row_revision,
                },
                new: new_artifact(new_s3_key, 2048),
            },
        )
        .await
        .expect("prepare replace operation");
    ledger
        .transition(
            operation_id,
            OperationState::Prepared,
            OperationTransition::Staged {
                staging_identity: STAGING_IDENTITY.to_string(),
            },
        )
        .await
        .expect("record staged identity");
    ledger
        .transition(
            operation_id,
            OperationState::Staged,
            OperationTransition::Published {
                new_identity: NEW_IDENTITY.to_string(),
            },
        )
        .await
        .expect("record published identity");
}

#[tokio::test(flavor = "current_thread")]
async fn typed_ledger_round_trips_all_six_states_and_owned_gc_target() {
    let workspace = TestWorkspace::new().expect("workspace");
    let pool = migrated_pool(&workspace).await;
    let ledger = PluginArtifactLedger::new(pool.clone());
    let create_operation_id = Uuid::from_u128(0x7801);
    let new_key = "ledger-create/1.0.0/typed-create/plugin.wasm";

    let prepared = ledger
        .prepare(
            create_operation_id,
            PreparePluginOperation::Create {
                target_identifier: "ledger-create".to_string(),
                new: new_artifact(new_key, 1024),
            },
        )
        .await
        .expect("persist prepared create");
    assert_eq!(prepared.operation_id(), create_operation_id);
    assert_eq!(prepared.kind(), OperationKind::Create);
    assert_eq!(prepared.state(), OperationState::Prepared);
    assert_eq!(
        prepared.staging_name(),
        derive_staging_name(&create_operation_id.to_string())
    );
    assert_eq!(
        ledger
            .get(create_operation_id)
            .await
            .expect("query prepared")
            .expect("prepared exists")
            .state(),
        OperationState::Prepared
    );

    let staged = ledger
        .transition(
            create_operation_id,
            OperationState::Prepared,
            OperationTransition::Staged {
                staging_identity: STAGING_IDENTITY.to_string(),
            },
        )
        .await
        .expect("prepared to staged");
    assert_eq!(staged.state(), OperationState::Staged);

    let published = ledger
        .transition(
            create_operation_id,
            OperationState::Staged,
            OperationTransition::Published {
                new_identity: NEW_IDENTITY.to_string(),
            },
        )
        .await
        .expect("staged to published");
    assert_eq!(published.state(), OperationState::Published);

    let plugin = ledger
        .commit_published_create(create_operation_id, create_fields("Ledger Create"))
        .await
        .expect("published create commits its Plugin reference");
    assert_eq!(plugin.identifier, "ledger-create");
    assert_eq!(plugin.s3_key, new_key);
    assert_eq!(plugin.sha256, SHA_NEW);
    assert_eq!(plugin.size_bytes, 1024);
    assert_eq!(plugin.row_revision, 0);

    let referenced = ledger
        .get(create_operation_id)
        .await
        .expect("query referenced")
        .expect("referenced exists");
    assert_eq!(referenced.state(), OperationState::Referenced);
    assert_eq!(referenced.plugin_id(), Some(plugin.id));

    let done = ledger
        .transition(
            create_operation_id,
            OperationState::Referenced,
            OperationTransition::Done,
        )
        .await
        .expect("referenced create to done");
    assert_eq!(done.state(), OperationState::Done);

    let conflict_operation_id = Uuid::from_u128(0x7802);
    ledger
        .prepare(
            conflict_operation_id,
            PreparePluginOperation::Create {
                target_identifier: "ledger-conflict".to_string(),
                new: new_artifact("ledger-conflict/1.0.0/typed-conflict/plugin.wasm", 1024),
            },
        )
        .await
        .expect("prepare conflict operation");
    let conflict = ledger
        .transition(
            conflict_operation_id,
            OperationState::Prepared,
            OperationTransition::Conflict,
        )
        .await
        .expect("persist conflict");
    assert_eq!(conflict.state(), OperationState::Conflict);

    let orphan_operation_id = Uuid::from_u128(0x7803);
    let orphan_key = "ledger-orphan/1.0.0/typed-orphan/plugin.wasm";
    publish_create(&ledger, orphan_operation_id, "ledger-orphan", orphan_key).await;
    let finished = ledger
        .finish_with_gc(
            orphan_operation_id,
            OperationState::Published,
            OperationGcTarget::PublishedNew,
        )
        .await
        .expect("owned published object enters GC and operation finishes atomically");
    assert!(matches!(
        finished,
        FinishOutcome::Finished {
            gc_state: GcState::Pending
        }
    ));
    let gc_state: String =
        sqlx::query_scalar("SELECT state FROM plugin_artifact_gc WHERE artifact_key = ?")
            .bind(orphan_key)
            .fetch_one(&pool)
            .await
            .expect("derived owned GC row");
    assert_eq!(gc_state, "pending");
    assert_eq!(
        ledger
            .get(orphan_operation_id)
            .await
            .expect("query finished orphan")
            .expect("orphan operation exists")
            .state(),
        OperationState::Done
    );

    let unresolved = ledger
        .list_not_done()
        .await
        .expect("query unresolved states");
    assert!(unresolved.iter().any(|row| {
        row.operation_id() == conflict_operation_id && row.state() == OperationState::Conflict
    }));
    assert!(
        unresolved
            .iter()
            .all(|row| row.state() != OperationState::Done)
    );
}

#[tokio::test(flavor = "current_thread")]
async fn create_referenced_failure_rolls_back_plugin_insert_and_operation() {
    let workspace = TestWorkspace::new().expect("workspace");
    let pool = migrated_pool(&workspace).await;
    let ledger = PluginArtifactLedger::new(pool.clone());
    let operation_id = Uuid::from_u128(0x7810);
    let new_key = "ledger-atomic/1.0.0/typed-atomic/plugin.wasm";
    publish_create(&ledger, operation_id, "ledger-atomic", new_key).await;
    sqlx::query(
        "CREATE TRIGGER t078_fail_referenced BEFORE UPDATE OF state ON plugin_artifact_operations \
         WHEN NEW.state = 'referenced' \
         BEGIN SELECT RAISE(ABORT, 't078 injected referenced failure'); END",
    )
    .execute(&pool)
    .await
    .expect("fault trigger");

    let result = ledger
        .commit_published_create(operation_id, create_fields("Ledger Atomic"))
        .await;
    let plugin_rows: i64 =
        sqlx::query_scalar("SELECT COUNT(*) FROM plugins WHERE identifier = 'ledger-atomic'")
            .fetch_one(&pool)
            .await
            .expect("plugin count");
    let operation = ledger
        .get(operation_id)
        .await
        .expect("query operation")
        .expect("operation remains");

    assert!(
        result.is_err()
            && plugin_rows == 0
            && operation.state() == OperationState::Published
            && operation.plugin_id().is_none(),
        "injected referenced failure must roll back the user row and ledger together: \
         result={result:?}, plugin_rows={plugin_rows}, operation={operation:?}"
    );
}

#[tokio::test(flavor = "current_thread")]
async fn resource_limit_bounds_fail_without_mutating_plugin() {
    let workspace = TestWorkspace::new().expect("workspace");
    let pool = migrated_pool(&workspace).await;
    let cases = [
        (
            "timeout-zero",
            "timeout_ms",
            serde_json::json!({"timeout_ms": 0}),
        ),
        (
            "timeout-over",
            "timeout_ms",
            serde_json::json!({"timeout_ms": 120_001}),
        ),
        (
            "memory-zero",
            "memory_limit_mb",
            serde_json::json!({"memory_limit_mb": 0}),
        ),
        (
            "memory-over",
            "memory_limit_mb",
            serde_json::json!({"memory_limit_mb": 513}),
        ),
        (
            "output-zero",
            "output_limit_bytes",
            serde_json::json!({"output_limit_bytes": 0}),
        ),
        (
            "output-over",
            "output_limit_bytes",
            serde_json::json!({"output_limit_bytes": 52_428_801}),
        ),
    ];
    let mut accepted = Vec::new();
    let mut mutated = Vec::new();
    let mut unsafe_errors = Vec::new();

    for (case, field, limits) in cases {
        let plugin = seed_plugin(
            &pool,
            &format!("limits-{case}"),
            &format!("limits-{case}/1.0.0/plugin.wasm"),
        )
        .await;
        match Plugin::update_limits(&pool, plugin.id, "[]".to_string(), limits.to_string()).await {
            Ok(_) => accepted.push(case),
            Err(error) => {
                let error = error.to_string();
                if !error.contains("invalid_input") || !error.contains(field) {
                    unsafe_errors.push((case, error));
                }
            }
        }
        let after = Plugin::get(&pool, plugin.id)
            .await
            .expect("read after invalid write")
            .expect("plugin remains");
        if after.resource_limits != "{}" || after.row_revision != 0 {
            mutated.push((case, after.resource_limits, after.row_revision));
        }
    }

    assert!(
        accepted.is_empty() && mutated.is_empty() && unsafe_errors.is_empty(),
        "invalid resource limits crossed the public Store boundary: accepted={accepted:?}, \
         mutated={mutated:?}, unsafe_errors={unsafe_errors:?}"
    );
}

#[tokio::test(flavor = "current_thread")]
async fn replace_cas_conflict_registers_derived_new_gc_and_finishes_atomically() {
    let workspace = TestWorkspace::new().expect("workspace");
    let pool = migrated_pool(&workspace).await;
    let ledger = PluginArtifactLedger::new(pool.clone());
    let operation_id = Uuid::from_u128(0x7820);
    let old_key = "replace-stale/1.0.0/old/plugin.wasm";
    let new_key = "replace-stale/1.0.0/new/plugin.wasm";
    let plugin = seed_plugin(&pool, "replace-stale", old_key).await;
    publish_replace(&ledger, &plugin, operation_id, new_key).await;
    sqlx::query("UPDATE plugins SET row_revision = row_revision + 1 WHERE id = ?")
        .bind(plugin.id)
        .execute(&pool)
        .await
        .expect("simulate concurrent writer");

    let replaced = ledger
        .commit_published_replace(operation_id)
        .await
        .expect("stable CAS outcome");
    let after = Plugin::get(&pool, plugin.id)
        .await
        .expect("read plugin")
        .expect("plugin remains");
    let operation = ledger
        .get(operation_id)
        .await
        .expect("query operation")
        .expect("operation remains");
    let gc_row: Option<(String, String)> =
        sqlx::query_as("SELECT artifact_key, state FROM plugin_artifact_gc WHERE artifact_key = ?")
            .bind(new_key)
            .fetch_optional(&pool)
            .await
            .expect("GC row");

    assert!(
        matches!(
            replaced,
            ReplaceOutcome::ConcurrentConflict {
                plugin_id,
                actual_row_revision: Some(1),
            } if plugin_id == plugin.id
        ) && after.s3_key == old_key
            && after.sha256 == SHA_OLD
            && after.size_bytes == 1024
            && after.row_revision == 1
            && operation.state() == OperationState::Done
            && gc_row == Some((new_key.to_string(), "pending".to_string())),
        "stale CAS must preserve the old row and atomically register owned-new GC + done: \
         replaced={replaced:?}, key={}, revision={}, operation={operation:?}, gc={gc_row:?}",
        after.s3_key,
        after.row_revision
    );
}

#[tokio::test(flavor = "current_thread")]
async fn replace_conflict_gc_rolls_back_when_done_transition_fails() {
    let workspace = TestWorkspace::new().expect("workspace");
    let pool = migrated_pool(&workspace).await;
    let ledger = PluginArtifactLedger::new(pool.clone());
    let operation_id = Uuid::from_u128(0x7821);
    let old_key = "replace-atomic/1.0.0/old/plugin.wasm";
    let new_key = "replace-atomic/1.0.0/new/plugin.wasm";
    let plugin = seed_plugin(&pool, "replace-atomic", old_key).await;
    publish_replace(&ledger, &plugin, operation_id, new_key).await;
    sqlx::query("UPDATE plugins SET row_revision = row_revision + 1 WHERE id = ?")
        .bind(plugin.id)
        .execute(&pool)
        .await
        .expect("simulate concurrent writer");
    sqlx::query(
        "CREATE TRIGGER t078_fail_done BEFORE UPDATE OF state ON plugin_artifact_operations \
         WHEN NEW.state = 'done' \
         BEGIN SELECT RAISE(ABORT, 't078 injected done failure'); END",
    )
    .execute(&pool)
    .await
    .expect("fault trigger");

    let result = ledger.commit_published_replace(operation_id).await;
    let operation = ledger
        .get(operation_id)
        .await
        .expect("query operation")
        .expect("operation remains");
    let gc_rows: i64 =
        sqlx::query_scalar("SELECT COUNT(*) FROM plugin_artifact_gc WHERE artifact_key = ?")
            .bind(new_key)
            .fetch_one(&pool)
            .await
            .expect("GC count");
    let after = Plugin::get(&pool, plugin.id)
        .await
        .expect("read plugin")
        .expect("plugin remains");

    assert!(
        result.is_err()
            && operation.state() == OperationState::Published
            && gc_rows == 0
            && after.s3_key == old_key
            && after.sha256 == SHA_OLD
            && after.size_bytes == 1024
            && after.row_revision == 1,
        "GC insert and operation->done must roll back as one transaction: result={result:?}, \
         operation={operation:?}, gc_rows={gc_rows}, key={}, revision={}",
        after.s3_key,
        after.row_revision
    );
}
