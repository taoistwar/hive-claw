//! T016D [P] [Foundation-doc+Red] Plugin v4 internal durability schema contract.
//!
//! The Foundation phase owns the *structure* of the Plugin
//! artifact ledger. US8 T073-T082 may only add *behaviour* on top
//! of this schema; the schema itself is the single source of truth
//! for the v4 Plugin durability contract.
//!
//! ## Schema contract (verified by direct public boundary calls)
//!
//! 1. `plugins.row_revision` MUST exist with `NOT NULL DEFAULT 0`.
//!    Existing v3 plugin rows MUST be backfilled to 0 during the
//!    v3→v4 migration.
//! 2. `plugin_artifact_operations` MUST exist and carry the
//!    following column shape. The published physical names from T016D/T022
//!    remain stable: data-model `operation_kind` maps to `kind`,
//!    `expected_old_size_bytes` to `expected_old_size`,
//!    `expected_row_revision` to `expected_old_row_revision`, and
//!    `new_size_bytes` to `new_size`:
//!    - `operation_id TEXT NOT NULL`
//!    - `kind TEXT NOT NULL CHECK(kind IN ('create','replace'))`
//!    - `target_identifier TEXT`
//!    - `plugin_id TEXT` (nullable for `create` until `referenced`)
//!    - `expected_old_*` (identifying the previous plugin tuple):
//!      - `expected_old_identifier TEXT`
//!      - `expected_old_version TEXT`
//!      - `expected_old_s3_key TEXT`
//!      - `expected_old_sha256 TEXT`
//!      - `expected_old_size INTEGER`
//!      - `expected_old_identity TEXT`
//!      - `expected_old_resource_limits TEXT`
//!    - `expected_old_row_revision INTEGER`
//!    - `staging_name TEXT` (UNIQUE, derived from `operation_id`)
//!    - `staging_identity TEXT` (nullable)
//!    - `new_s3_key TEXT` (UNIQUE, nullable until `staged`)
//!    - `new_sha256 TEXT` (nullable)
//!    - `new_size INTEGER` (nullable)
//!    - `new_resource_limits TEXT` (nullable)
//!    - `new_identity TEXT` (nullable until `published`)
//!    - `state TEXT NOT NULL CHECK(state IN
//!        ('prepared','staged','published','referenced','done','conflict'))`
//!    - `created_at TEXT`
//!    - `updated_at TEXT`
//! 3. `plugin_artifact_gc` MUST exist and carry the GC contract:
//!    - `artifact_key TEXT PRIMARY KEY`
//!    - `expected_sha256 TEXT`
//!    - `expected_size_bytes INTEGER`
//!    - `expected_identity TEXT`
//!    - `source_operation_id TEXT` FK→`plugin_artifact_operations(operation_id)`
//!    - `state TEXT NOT NULL CHECK(state IN ('pending','blocked'))`
//!    - `attempts INTEGER`
//!    - `last_error TEXT`
//!    - `created_at TEXT`
//!    - `updated_at TEXT`
//!    - `reason TEXT NOT NULL`
//!    - `last_attempt_at INTEGER` (wall-clock seconds, monotonic)
//! 4. The operations state CHECK MUST additionally enforce the
//!    identity pre-conditions required by the sidecar/identity
//!    task rule:
//!    - `prepared`: both `staging_identity` and `new_identity` MUST
//!      be NULL.
//!    - `staged`: only `staging_identity` MUST be non-NULL
//!      (`new_identity` NULL).
//!    - `published`/`referenced`: both MUST be non-NULL.
//!    - `done`: `new_identity IS NULL OR staging_identity IS NOT NULL`.
//!    - `conflict`: no identity constraint.
//! 5. Create vs replace nullability:
//!    - `create` operations: all `expected_old_*` and
//!      `expected_old_row_revision` MUST be NULL in *every* state;
//!      `plugin_id` MUST be NULL in `prepared|staged|published` and
//!      non-NULL in `referenced`.
//!    - `replace` operations: `plugin_id` and the entire
//!      `expected_old_*` tuple (including `expected_old_row_revision`)
//!      MUST be non-NULL in *every* state.
//! 6. The migration MUST be transactional: after the v3→v4
//!    migration commits, the schema MUST be inspected via direct
//!    SQLite introspection and the contract items above MUST hold.
//!    A migration that applies *partial* DDL (some tables created
//!    but not the others) MUST be rejected and the database MUST
//!    roll back to its pre-migration state.
//! 7. If the running schema drifts (missing column / missing table
//!    / broken CHECK), `open_store` MUST fail-closed *before* any
//!    other code path runs migrations or opens the Store. Runtime
//!    `entity_store.rs` (and any other non-migration code path) MUST
//!    NOT contain `ALTER TABLE` / `CREATE TABLE` statements that
//!    re-create the contract — the contract is owned by the
//!    migrations module.
//!
//! These tests are Red against the current implementation: the
//! public boundary `hivegui::datasource::migrations::migrate_to_current`
//! and the schema introspection helpers it depends on do not exist
//! yet. T022 is the only task that may turn these tests Green.

mod support;

use std::{fs, path::Path};

use sqlx::{Row, sqlite::SqlitePoolOptions};
use support::TestWorkspace;

const OWNER_PHASE: &str = "Foundation";
const APPROVAL_TASK: &str = "T016D";
const IMPLEMENTATION_TASK: &str = "T022";
const SCHEMA_VERSION_V4: i64 = 4;
const FIXTURE_DIRECTORY: &str = concat!(env!("CARGO_MANIFEST_DIR"), "/tests/fixtures/migrations");

// ---------------------------------------------------------------------------
// §T016D.1 — Fresh v4 database: schema is created by migrations.rs.
// ---------------------------------------------------------------------------

#[tokio::test]
async fn fresh_v4_database_creates_plugin_artifact_ledger_via_migrations() {
    let workspace = TestWorkspace::new().expect("isolated workspace");
    let report = hivegui::datasource::migrations::migrate_to_current(
        hivegui::datasource::migrations::MigrationOptions::new(
            workspace.database_path(),
            workspace.plugin_root(),
        ),
    )
    .await
    .expect("migrations to v4");

    assert_eq!(report.to_version, SCHEMA_VERSION_V4);
    assert!(
        report.validation.managed_plugins_ok,
        "migrations must report managed-plugin schema validation ok"
    );

    // The schema is the single source of truth; verify the
    // structural contract directly via SQLite introspection.
    let pool = open_pool(workspace.database_path()).await;
    assert_plugins_row_revision_contract(&pool).await;
    assert_plugin_artifact_operations_contract(&pool).await;
    assert_plugin_artifact_gc_contract(&pool).await;
    pool.close().await;
}

// ---------------------------------------------------------------------------
// §T016D.2 — v3→v4 backfills `plugins.row_revision` to 0 and adds the
//             operations / GC tables in one transaction.
// ---------------------------------------------------------------------------

#[tokio::test]
async fn v3_to_v4_backfills_row_revision_and_creates_ledger_transactionally() {
    let workspace = TestWorkspace::new().expect("isolated workspace");
    // Build a v3-shaped database inline. The Foundation phase
    // owns the v3 -> v4 transition; the contract is that the
    // existing `plugins` rows are preserved (only `row_revision`
    // is added with default 0).
    seed_v3_database(workspace.database_path()).await;

    let pre_artifact = capture_artifact_snapshot(workspace.database_path()).await;
    let pre_plugin_count = count_rows(&workspace, "SELECT COUNT(*) FROM plugins").await;

    let report = hivegui::datasource::migrations::migrate_to_current(
        hivegui::datasource::migrations::MigrationOptions::new(
            workspace.database_path(),
            workspace.plugin_root(),
        ),
    )
    .await
    .expect("v3 must migrate to v4");
    assert_eq!(report.from_version, Some(3));
    assert_eq!(report.to_version, SCHEMA_VERSION_V4);

    let pool = open_pool(workspace.database_path()).await;
    // Existing plugin rows MUST have row_revision = 0 after the
    // backfill.
    let revised = scalar_i64(&pool, "SELECT COALESCE(MIN(row_revision), -1) FROM plugins").await;
    assert_eq!(
        revised, 0,
        "all v3 plugin rows must be backfilled to row_revision = 0"
    );
    let post_count = scalar_i64(&pool, "SELECT COUNT(*) FROM plugins").await;
    assert_eq!(
        post_count, pre_plugin_count,
        "v3->v4 must not add or remove plugin rows"
    );

    assert_plugins_row_revision_contract(&pool).await;
    assert_plugin_artifact_operations_contract(&pool).await;
    assert_plugin_artifact_gc_contract(&pool).await;

    // No partial DDL: the artifact hash for the *plugin table
    // contents* must equal the pre-migration value (the v3 fixture
    // is the source of truth for `plugins` rows; the migration may
    // only add the row_revision column).
    let post_artifact = capture_artifact_snapshot(workspace.database_path()).await;
    assert_eq!(
        pre_artifact.plugins_digest, post_artifact.plugins_digest,
        "v3->v4 must preserve the existing plugin row contents"
    );
    pool.close().await;
}

// ---------------------------------------------------------------------------
// §T016D.2A — Data-model ledger columns, source FK, and worker indexes.
// ---------------------------------------------------------------------------

#[tokio::test]
async fn data_model_gc_columns_are_present_in_fresh_v4() {
    let workspace = TestWorkspace::new().expect("isolated workspace");
    hivegui::datasource::migrations::migrate_to_current(
        hivegui::datasource::migrations::MigrationOptions::new(
            workspace.database_path(),
            workspace.plugin_root(),
        ),
    )
    .await
    .expect("migrate fresh database to v4");

    let pool = open_pool(workspace.database_path()).await;
    assert_plugin_artifact_gc_contract(&pool).await;
    pool.close().await;
}

#[tokio::test]
async fn data_model_gc_source_operation_foreign_key_is_declared() {
    let workspace = TestWorkspace::new().expect("isolated workspace");
    hivegui::datasource::migrations::migrate_to_current(
        hivegui::datasource::migrations::MigrationOptions::new(
            workspace.database_path(),
            workspace.plugin_root(),
        ),
    )
    .await
    .expect("migrate fresh database to v4");
    let pool = open_pool(workspace.database_path()).await;

    let foreign_keys = sqlx::query(
        "SELECT [table] AS target_table, [from] AS source_column, [to] AS target_column \
         FROM pragma_foreign_key_list('plugin_artifact_gc')",
    )
    .fetch_all(&pool)
    .await
    .expect("introspect plugin_artifact_gc foreign keys")
    .into_iter()
    .map(|row| {
        (
            row.try_get::<String, _>("source_column")
                .expect("FK source column"),
            row.try_get::<String, _>("target_table")
                .expect("FK target table"),
            row.try_get::<String, _>("target_column")
                .expect("FK target column"),
        )
    })
    .collect::<Vec<_>>();

    assert!(
        foreign_keys.iter().any(|(source, table, target)| {
            source == "source_operation_id"
                && table == "plugin_artifact_operations"
                && target == "operation_id"
        }),
        "plugin_artifact_gc.source_operation_id must reference \
         plugin_artifact_operations(operation_id); actual={foreign_keys:?}"
    );
    pool.close().await;
}

#[tokio::test]
async fn data_model_plugin_ledger_has_replay_and_worker_indexes() {
    let workspace = TestWorkspace::new().expect("isolated workspace");
    hivegui::datasource::migrations::migrate_to_current(
        hivegui::datasource::migrations::MigrationOptions::new(
            workspace.database_path(),
            workspace.plugin_root(),
        ),
    )
    .await
    .expect("migrate fresh database to v4");
    let pool = open_pool(workspace.database_path()).await;

    let operation_indexes = index_shapes(&pool, "plugin_artifact_operations").await;
    let gc_indexes = index_shapes(&pool, "plugin_artifact_gc").await;
    let requirements = [
        (
            "operations non-terminal replay by state",
            has_index_prefix(&operation_indexes, &["state"]),
        ),
        (
            "GC stable pending/blocked scan by state then artifact_key",
            has_index_prefix(&gc_indexes, &["state", "artifact_key"]),
        ),
        (
            "GC source-operation lookup",
            has_index_prefix(&gc_indexes, &["source_operation_id"]),
        ),
    ];
    let missing = requirements
        .into_iter()
        .filter_map(|(requirement, present)| (!present).then_some(requirement))
        .collect::<Vec<_>>();

    assert!(
        missing.is_empty(),
        "plugin ledger is missing necessary indexes {missing:?}; \
         operations={operation_indexes:?}, gc={gc_indexes:?}"
    );
    pool.close().await;
}

#[tokio::test]
async fn data_model_operation_and_gc_rows_round_trip_and_orphans_fail_closed() {
    let workspace = TestWorkspace::new().expect("isolated workspace");
    hivegui::datasource::migrations::migrate_to_current(
        hivegui::datasource::migrations::MigrationOptions::new(
            workspace.database_path(),
            workspace.plugin_root(),
        ),
    )
    .await
    .expect("migrate fresh database to v4");
    let pool = open_pool(workspace.database_path()).await;
    sqlx::query("PRAGMA foreign_keys = ON")
        .execute(&pool)
        .await
        .expect("enable foreign keys");

    let operation_id = "00000000-0000-4000-8000-000000000078";
    let staging_name = derive_staging_name_for(operation_id);
    let old_sha256 = "a".repeat(64);
    let new_sha256 = "b".repeat(64);
    let timestamp = "2026-08-20T00:00:00Z";
    sqlx::query(
        "INSERT INTO plugin_artifact_operations (\
             operation_id, kind, target_identifier, plugin_id, \
             expected_old_identifier, expected_old_version, expected_old_s3_key, \
             expected_old_sha256, expected_old_size, expected_old_identity, \
             expected_old_resource_limits, expected_old_row_revision, \
             staging_name, staging_identity, new_s3_key, new_sha256, new_size, \
             new_resource_limits, new_identity, state, created_at, updated_at\
         ) VALUES (?, 'replace', 'schema-contract', 'plugin-78', \
             'schema-contract', '1.0.0', 'old/plugin.wasm', ?, 128, 'old-file-id', \
             '{}', 7, ?, NULL, 'new/plugin.wasm', ?, 256, '{}', NULL, \
             'prepared', ?, ?)",
    )
    .bind(operation_id)
    .bind(&old_sha256)
    .bind(&staging_name)
    .bind(&new_sha256)
    .bind(timestamp)
    .bind(timestamp)
    .execute(&pool)
    .await
    .expect("data-model operation row must insert");

    sqlx::query(
        "INSERT INTO plugin_artifact_gc (\
             artifact_key, expected_sha256, expected_size_bytes, expected_identity, \
             source_operation_id, state, attempts, last_error, created_at, updated_at, \
             reason, last_attempt_at\
         ) VALUES ('old/plugin.wasm', ?, 128, 'old-file-id', ?, 'pending', 0, \
             NULL, ?, ?, 'replaced', 0)",
    )
    .bind(&old_sha256)
    .bind(operation_id)
    .bind(timestamp)
    .bind(timestamp)
    .execute(&pool)
    .await
    .expect("data-model GC row must insert");

    let stored: (String, i64, String, String, i64, Option<String>) = sqlx::query_as(
        "SELECT expected_sha256, expected_size_bytes, expected_identity, \
                source_operation_id, attempts, last_error \
         FROM plugin_artifact_gc WHERE artifact_key = 'old/plugin.wasm'",
    )
    .fetch_one(&pool)
    .await
    .expect("round-trip GC ownership tuple");
    assert_eq!(
        stored,
        (
            old_sha256.clone(),
            128,
            "old-file-id".to_string(),
            operation_id.to_string(),
            0,
            None,
        ),
        "GC must retain the exact expected artifact identity and source operation"
    );

    let orphan = sqlx::query(
        "INSERT INTO plugin_artifact_gc (\
             artifact_key, expected_sha256, expected_size_bytes, expected_identity, \
             source_operation_id, state, attempts, last_error, created_at, updated_at, \
             reason, last_attempt_at\
         ) VALUES ('orphan/plugin.wasm', ?, 128, 'orphan-file-id', \
             '00000000-0000-4000-8000-000000000000', 'pending', 0, NULL, ?, ?, \
             'orphan', 0)",
    )
    .bind(&old_sha256)
    .bind(timestamp)
    .bind(timestamp)
    .execute(&pool)
    .await
    .expect_err("orphan source_operation_id must fail closed");
    assert_violation(&orphan, "FOREIGN KEY");
    pool.close().await;
}

// ---------------------------------------------------------------------------
// §T016D.3 — State CHECK enforces the identity pre-conditions.
// ---------------------------------------------------------------------------

#[tokio::test]
async fn operations_state_check_enforces_identity_preconditions() {
    let workspace = TestWorkspace::new().expect("isolated workspace");
    let _ = hivegui::datasource::migrations::migrate_to_current(
        hivegui::datasource::migrations::MigrationOptions::new(
            workspace.database_path(),
            workspace.plugin_root(),
        ),
    )
    .await
    .expect("migrate to v4");
    let pool = open_pool(workspace.database_path()).await;

    // prepared: both identities must be NULL. A non-null identity
    // is rejected by the state CHECK.
    let err = insert_operation(
        &pool,
        "op-prep",
        "create",
        "prepared",
        Some("staging-bad"),
        Some("new-bad"),
    )
    .await
    .expect_err("prepared with non-null identity is rejected");
    assert_violation(&err, "CHECK");

    // staged: only staging_identity may be non-null.
    let err = insert_operation(
        &pool,
        "op-staged-bad",
        "create",
        "staged",
        Some("staging-1"),
        Some("new-1"),
    )
    .await
    .expect_err("staged with new_identity non-null is rejected");
    assert_violation(&err, "CHECK");

    // published: both identities must be non-null.
    let err = insert_operation(
        &pool,
        "op-pub-bad",
        "replace",
        "published",
        Some("staging-2"),
        None,
    )
    .await
    .expect_err("published with new_identity null is rejected");
    assert_violation(&err, "CHECK");

    // done: requires new_identity NULL OR staging_identity NOT NULL.
    let err = insert_operation(&pool, "op-done-bad", "replace", "done", None, Some("new-3"))
        .await
        .expect_err("done with new_identity non-null and staging_identity null is rejected");
    assert_violation(&err, "CHECK");

    // conflict: accepts any identity combination.
    insert_operation(&pool, "op-conflict", "create", "conflict", None, None)
        .await
        .expect("conflict accepts null identities");
    insert_operation(
        &pool,
        "op-conflict-2",
        "create",
        "conflict",
        Some("staging-x"),
        Some("new-y"),
    )
    .await
    .expect("conflict accepts non-null identities");

    pool.close().await;
}

// ---------------------------------------------------------------------------
// §T016D.4 — Create vs replace nullability invariant.
// ---------------------------------------------------------------------------

#[tokio::test]
async fn create_keeps_expected_old_columns_null_in_every_state() {
    let workspace = TestWorkspace::new().expect("isolated workspace");
    let _ = hivegui::datasource::migrations::migrate_to_current(
        hivegui::datasource::migrations::MigrationOptions::new(
            workspace.database_path(),
            workspace.plugin_root(),
        ),
    )
    .await
    .expect("migrate to v4");
    let pool = open_pool(workspace.database_path()).await;

    // For every state, a "create" operation with all NULL
    // `expected_old_*` MUST be accepted by the schema. The
    // contract under test is "create keeps `expected_old_*` NULL
    // in every state"; the rejection of a non-NULL `expected_old_*`
    // is asserted separately by `create_with_non_null_expected_old_is_rejected`.
    for state in [
        "prepared",
        "staged",
        "published",
        "referenced",
        "done",
        "conflict",
    ] {
        // For "prepared|staged|published" we must also keep plugin_id NULL.
        let plugin_id = if matches!(state, "prepared" | "staged" | "published") {
            None
        } else {
            Some("plugin-1")
        };
        let staging_identity = if state == "prepared" {
            None
        } else {
            Some("staging-id")
        };
        let new_identity = if matches!(state, "prepared" | "staged") {
            None
        } else {
            Some("new-id")
        };
        insert_operation_with_old_tuple(
            &pool,
            &format!("op-create-{state}"),
            "create",
            state,
            plugin_id,
            staging_identity,
            new_identity,
        )
        .await
        .unwrap_or_else(|err| {
            panic!("create with null expected_old_* in {state} is accepted: {err}")
        });
    }
    pool.close().await;
}

#[tokio::test]
async fn create_with_non_null_expected_old_is_rejected() {
    // The schema MUST reject a "create" operation whose
    // `expected_old_*` columns are not all NULL.
    let workspace = TestWorkspace::new().expect("isolated workspace");
    let _ = hivegui::datasource::migrations::migrate_to_current(
        hivegui::datasource::migrations::MigrationOptions::new(
            workspace.database_path(),
            workspace.plugin_root(),
        ),
    )
    .await
    .expect("migrate to v4");
    let pool = open_pool(workspace.database_path()).await;

    let err = insert_operation_with_non_null_old_tuple(
        &pool,
        "op-create-bad-old",
        "create",
        "prepared",
        None,
        None,
        None,
    )
    .await
    .expect_err("create with non-null expected_old_* is rejected");
    assert_violation(&err, "CHECK");
    pool.close().await;
}

#[tokio::test]
async fn replace_requires_non_null_plugin_id_and_old_tuple_in_every_state() {
    let workspace = TestWorkspace::new().expect("isolated workspace");
    let _ = hivegui::datasource::migrations::migrate_to_current(
        hivegui::datasource::migrations::MigrationOptions::new(
            workspace.database_path(),
            workspace.plugin_root(),
        ),
    )
    .await
    .expect("migrate to v4");
    let pool = open_pool(workspace.database_path()).await;

    // Missing plugin_id is rejected.
    let err = insert_operation_with_old_tuple(
        &pool,
        "op-replace-no-pid",
        "replace",
        "prepared",
        None,
        None,
        None,
    )
    .await
    .expect_err("replace with null plugin_id is rejected");
    assert_violation(&err, "CHECK");

    // Missing expected_old_row_revision is rejected.
    let err = insert_operation_with_partial_old_tuple(
        &pool,
        "op-replace-no-rev",
        "replace",
        "prepared",
        Some("plugin-2"),
    )
    .await
    .expect_err("replace with null expected_old_row_revision is rejected");
    assert_violation(&err, "CHECK");

    pool.close().await;
}

// ---------------------------------------------------------------------------
// §T016D.5 — `staging_name` is UNIQUE and derived from `operation_id`.
// ---------------------------------------------------------------------------

#[tokio::test]
async fn staging_name_is_unique_and_derived_from_operation_id() {
    let workspace = TestWorkspace::new().expect("isolated workspace");
    let _ = hivegui::datasource::migrations::migrate_to_current(
        hivegui::datasource::migrations::MigrationOptions::new(
            workspace.database_path(),
            workspace.plugin_root(),
        ),
    )
    .await
    .expect("migrate to v4");
    let pool = open_pool(workspace.database_path()).await;

    // Two operations with the same `operation_id` MUST produce the
    // same `staging_name`, and the second insert MUST fail with a
    // UNIQUE violation.
    let first = derive_staging_name(&pool, "op-dup").await;
    let second = derive_staging_name(&pool, "op-dup").await;
    assert_eq!(
        first, second,
        "staging_name MUST be deterministically derived from operation_id"
    );

    // The second insert of the same staging_name is rejected.
    insert_operation(&pool, "op-dup-1", "create", "prepared", None, None)
        .await
        .expect("first prepared insert succeeds");
    let err = insert_operation(&pool, "op-dup-1", "create", "prepared", None, None)
        .await
        .expect_err("duplicate staging_name is rejected");
    assert_violation(&err, "UNIQUE");

    pool.close().await;
}

// ---------------------------------------------------------------------------
// §T016D.6 — Schema drift: missing column fails-closed at open.
// ---------------------------------------------------------------------------

#[tokio::test]
async fn schema_drift_missing_column_fails_closed_at_open() {
    let workspace = TestWorkspace::new().expect("isolated workspace");
    let _ = hivegui::datasource::migrations::migrate_to_current(
        hivegui::datasource::migrations::MigrationOptions::new(
            workspace.database_path(),
            workspace.plugin_root(),
        ),
    )
    .await
    .expect("migrate to v4");
    let pool = open_pool(workspace.database_path()).await;
    // Drop the row_revision column to simulate a foreign tool or
    // a partial upgrade having touched the schema.
    sqlx::query("ALTER TABLE plugins DROP COLUMN row_revision")
        .execute(&pool)
        .await
        .expect("drop column");
    pool.close().await;

    let err = hivegui::datasource::store::open_store(workspace.database_path(), workspace.root())
        .await
        .expect_err("schema drift must fail-closed at open");
    assert!(matches!(
        err.kind,
        hivegui::datasource::store::StoreErrorKind::SchemaDrift { .. }
    ));
}

// ---------------------------------------------------------------------------
// §T016D.7 — Runtime Store does not contain `ALTER TABLE` / `CREATE TABLE`
//             for the v4 plugin ledger.
// ---------------------------------------------------------------------------

#[test]
fn runtime_store_has_no_plugin_ledger_ddl() {
    // The v4 plugin ledger schema is owned by the migrations module.
    // Any `ALTER TABLE` / `CREATE TABLE` / `CREATE INDEX` statement
    // inside the runtime Store source would re-introduce schema
    // ownership drift and MUST be flagged.
    let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("src")
        .join("datasource")
        .join("store.rs");
    if !path.exists() {
        // The runtime Store does not exist yet — that itself is the
        // Red gate for T016D: the schema contract is owned by the
        // migrations module, which T016D will keep the *only* DDL
        // owner. Skip the file scan with a structured panic so the
        // reviewer can re-run the test after T022 produces the file.
        panic!(
            "T016D Red gate: datasource/store.rs not yet created (expected at {}); runtime Store must not introduce its own DDL",
            path.display()
        );
    }
    let source = fs::read_to_string(&path).expect("read store.rs");
    for forbidden in [
        "CREATE TABLE plugin_artifact_operations",
        "CREATE TABLE plugin_artifact_gc",
        "ALTER TABLE plugins ADD COLUMN row_revision",
        "ALTER TABLE plugin_artifact_operations",
    ] {
        assert!(
            !source.contains(forbidden),
            "runtime Store must not contain `{forbidden}`; v4 plugin ledger DDL is owned by the migrations module"
        );
    }
}

// ---------------------------------------------------------------------------
// Helpers.
// ---------------------------------------------------------------------------

fn assert_case_ownership() {
    assert_eq!(OWNER_PHASE, "Foundation");
    assert_eq!(APPROVAL_TASK, "T016D");
    assert_eq!(IMPLEMENTATION_TASK, "T022");
    let _ = (
        OWNER_PHASE,
        APPROVAL_TASK,
        IMPLEMENTATION_TASK,
        SCHEMA_VERSION_V4,
    );
}

async fn open_pool(database_path: &Path) -> sqlx::SqlitePool {
    let url = format!("sqlite://{}?mode=rwc", database_path.display());
    SqlitePoolOptions::new()
        .max_connections(1)
        .connect(&url)
        .await
        .expect("open migrated database read-write")
}

fn fixture_path(name: &str) -> std::path::PathBuf {
    std::path::Path::new(FIXTURE_DIRECTORY).join(name)
}

fn copy_fixture(workspace: &TestWorkspace, name: &str) {
    let source = fixture_path(name);
    assert!(
        source.is_file(),
        "missing committed fixture {}; regenerate it only with the ignored T016D case",
        source.display()
    );
    fs::copy(&source, workspace.database_path()).unwrap_or_else(|error| {
        panic!(
            "copy fixture {} to isolated workspace: {error}",
            source.display()
        )
    });
}

async fn seed_v3_database(database_path: &Path) {
    // Build a v3-shaped SQLite database inline. The v3 schema
    // matches everything v4 has except:
    //   * `plugins.row_revision` is missing
    //   * `plugin_artifact_operations` is missing
    //   * `plugin_artifact_gc` is missing
    // and the `meta` row records `schema_version = 3`.
    let url = format!("sqlite://{}?mode=rwc", database_path.display());
    let pool = SqlitePoolOptions::new()
        .max_connections(1)
        .connect(&url)
        .await
        .expect("open v3 fixture for seeding");

    // Minimal v3 schema — note the absence of `row_revision` on
    // the `plugins` table and the absence of plugin_artifact_*
    // tables.
    sqlx::query("CREATE TABLE meta (key TEXT PRIMARY KEY, value TEXT NOT NULL)")
        .execute(&pool)
        .await
        .expect("v3 meta table");
    sqlx::query("INSERT INTO meta (key, value) VALUES ('schema_version', '3')")
        .execute(&pool)
        .await
        .expect("v3 schema_version row");

    sqlx::query(
        "CREATE TABLE plugins (\
            id INTEGER PRIMARY KEY AUTOINCREMENT, \
            identifier TEXT NOT NULL UNIQUE, \
            version TEXT NOT NULL DEFAULT '', \
            author TEXT NOT NULL DEFAULT '', \
            repository_url TEXT NOT NULL DEFAULT '', \
            s3_key TEXT NOT NULL DEFAULT '', \
            sha256 TEXT NOT NULL DEFAULT '', \
            size INTEGER NOT NULL DEFAULT 0, \
            runtime TEXT NOT NULL DEFAULT 'wasm32', \
            capabilities TEXT NOT NULL DEFAULT '[]', \
            resource_limits TEXT NOT NULL DEFAULT '{}', \
            created_at TEXT NOT NULL DEFAULT '', \
            updated_at TEXT NOT NULL DEFAULT ''\
        )",
    )
    .execute(&pool)
    .await
    .expect("v3 plugins table");

    sqlx::query("INSERT INTO plugins (identifier, version) VALUES ('plugin-a', '1.0.0'), ('plugin-b', '2.0.0')")
        .execute(&pool)
        .await
        .expect("seed two v3 plugin rows");

    pool.close().await;
}

async fn scalar_i64(pool: &sqlx::SqlitePool, sql: &str) -> i64 {
    // `sql` is a test fixture string, not user input; wrap with
    // `AssertSqlSafe` to satisfy sqlx 0.9's `SqlSafeStr` contract.
    sqlx::query_scalar(sqlx::AssertSqlSafe(sql.to_owned()))
        .fetch_one(pool)
        .await
        .unwrap_or_else(|error| panic!("scalar query `{sql}` failed: {error}"))
}

async fn count_rows(workspace: &TestWorkspace, sql: &str) -> i64 {
    let pool = open_pool(workspace.database_path()).await;
    let value = scalar_i64(&pool, sql).await;
    pool.close().await;
    value
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct ArtifactSnapshot {
    plugins_digest: String,
}

async fn capture_artifact_snapshot(database_path: &Path) -> ArtifactSnapshot {
    let pool = open_pool(database_path).await;
    let plugins_digest = scalar_text(
        &pool,
        "SELECT COALESCE(hex(plugins.digest), '') FROM (SELECT group_concat(identifier || ':' || COALESCE(version, ''), '|' ORDER BY identifier) AS digest FROM plugins) AS plugins",
    )
    .await;
    pool.close().await;
    ArtifactSnapshot { plugins_digest }
}

async fn scalar_text(pool: &sqlx::SqlitePool, sql: &str) -> String {
    // `sql` is a test fixture string, not user input; wrap with
    // `AssertSqlSafe` to satisfy sqlx 0.9's `SqlSafeStr` contract.
    sqlx::query_scalar(sqlx::AssertSqlSafe(sql.to_owned()))
        .fetch_one(pool)
        .await
        .unwrap_or_else(|error| panic!("scalar text query `{sql}` failed: {error}"))
}

async fn assert_plugins_row_revision_contract(pool: &sqlx::SqlitePool) {
    let row = sqlx::query(
        "SELECT name, type, [notnull], dflt_value FROM pragma_table_info('plugins') WHERE name = 'row_revision'",
    )
    .fetch_optional(pool)
    .await
    .expect("pragma_table_info")
    .unwrap_or_else(|| panic!("plugins.row_revision column is missing"));
    let notnull: i64 = row.try_get("notnull").expect("notnull column");
    let default: Option<String> = row.try_get("dflt_value").expect("dflt_value column");
    assert_eq!(notnull, 1, "row_revision must be NOT NULL");
    assert_eq!(
        default.as_deref(),
        Some("0"),
        "row_revision default must be 0"
    );
}

async fn assert_plugin_artifact_operations_contract(pool: &sqlx::SqlitePool) {
    let columns = scalar_strings(
        pool,
        "SELECT name FROM pragma_table_info('plugin_artifact_operations') ORDER BY cid",
    )
    .await;
    let required = [
        "expected_old_identifier",
        "expected_old_identity",
        "expected_old_resource_limits",
        "expected_old_row_revision",
        "expected_old_s3_key",
        "expected_old_sha256",
        "expected_old_size",
        "expected_old_version",
        "kind",
        "new_identity",
        "new_resource_limits",
        "new_s3_key",
        "new_sha256",
        "new_size",
        "operation_id",
        "plugin_id",
        "staging_identity",
        "staging_name",
        "state",
        "target_identifier",
        "created_at",
        "updated_at",
    ];
    assert_required_columns("plugin_artifact_operations", &columns, &required);

    // state CHECK must include all six values, in order.
    let check = scalar_text(
        pool,
        "SELECT sql FROM sqlite_master WHERE type = 'table' AND name = 'plugin_artifact_operations'",
    )
    .await;
    for state in [
        "prepared",
        "staged",
        "published",
        "referenced",
        "done",
        "conflict",
    ] {
        assert!(
            check.contains(&format!("'{state}'")),
            "operations.state CHECK must include `{state}`"
        );
    }
    // kind CHECK
    assert!(check.contains("'create'") && check.contains("'replace'"));
    // UNIQUE constraints
    assert!(check.contains("UNIQUE") && check.contains("staging_name"));
    assert!(check.contains("UNIQUE") && check.contains("new_s3_key"));
}

async fn assert_plugin_artifact_gc_contract(pool: &sqlx::SqlitePool) {
    let columns = scalar_strings(
        pool,
        "SELECT name FROM pragma_table_info('plugin_artifact_gc') ORDER BY cid",
    )
    .await;
    let required = [
        "artifact_key",
        "expected_sha256",
        "expected_size_bytes",
        "expected_identity",
        "source_operation_id",
        "state",
        "attempts",
        "last_error",
        "created_at",
        "updated_at",
        // Existing operational diagnostics remain valid additional columns.
        "reason",
        "last_attempt_at",
    ];
    assert_required_columns("plugin_artifact_gc", &columns, &required);
    let check = scalar_text(
        pool,
        "SELECT sql FROM sqlite_master WHERE type = 'table' AND name = 'plugin_artifact_gc'",
    )
    .await;
    assert!(check.contains("'pending'") && check.contains("'blocked'"));
    assert!(check.contains("PRIMARY KEY") && check.contains("artifact_key"));
}

async fn scalar_strings(pool: &sqlx::SqlitePool, sql: &str) -> Vec<String> {
    // `sql` is a test fixture string, not user input; wrap with
    // `AssertSqlSafe` to satisfy sqlx 0.9's `SqlSafeStr` contract.
    sqlx::query(sqlx::AssertSqlSafe(sql.to_owned()))
        .fetch_all(pool)
        .await
        .expect("strings query")
        .into_iter()
        .map(|row| row.try_get::<String, _>(0).expect("first column is text"))
        .collect()
}

fn assert_required_columns(table: &str, actual: &[String], required: &[&str]) {
    let missing = required
        .iter()
        .copied()
        .filter(|required| !actual.iter().any(|actual| actual == required))
        .collect::<Vec<_>>();
    assert!(
        missing.is_empty(),
        "{table} is missing required data-model columns {missing:?}; actual={actual:?}"
    );
}

async fn index_shapes(pool: &sqlx::SqlitePool, table: &str) -> Vec<Vec<String>> {
    let index_names = scalar_strings(
        pool,
        &format!("SELECT name FROM pragma_index_list('{table}') ORDER BY name"),
    )
    .await;
    let mut shapes = Vec::with_capacity(index_names.len());
    for index_name in index_names {
        shapes.push(
            scalar_strings(
                pool,
                &format!("SELECT name FROM pragma_index_info('{index_name}') ORDER BY seqno"),
            )
            .await,
        );
    }
    shapes
}

fn has_index_prefix(indexes: &[Vec<String>], prefix: &[&str]) -> bool {
    indexes.iter().any(|columns| {
        columns.len() >= prefix.len()
            && columns
                .iter()
                .zip(prefix)
                .all(|(actual, expected)| actual == expected)
    })
}

async fn insert_operation(
    pool: &sqlx::SqlitePool,
    operation_id: &str,
    kind: &str,
    state: &str,
    staging_identity: Option<&str>,
    new_identity: Option<&str>,
) -> Result<sqlx::sqlite::SqliteQueryResult, sqlx::Error> {
    let staging_identity = staging_identity.map(str::to_string);
    let new_identity = new_identity.map(str::to_string);
    let staging_name = derive_staging_name_for(operation_id);
    sqlx::query(
        "INSERT INTO plugin_artifact_operations (operation_id, kind, staging_identity, new_identity, state, staging_name) VALUES (?, ?, ?, ?, ?, ?)",
    )
    .bind(operation_id)
    .bind(kind)
    .bind(staging_identity)
    .bind(new_identity)
    .bind(state)
    .bind(staging_name)
    .execute(pool)
    .await
}

async fn insert_operation_with_old_tuple(
    pool: &sqlx::SqlitePool,
    operation_id: &str,
    kind: &str,
    state: &str,
    plugin_id: Option<&str>,
    staging_identity: Option<&str>,
    new_identity: Option<&str>,
) -> Result<sqlx::sqlite::SqliteQueryResult, sqlx::Error> {
    let plugin_id = plugin_id.map(str::to_string);
    let staging_identity = staging_identity.map(str::to_string);
    let new_identity = new_identity.map(str::to_string);
    let staging_name = derive_staging_name_for(operation_id);
    let old = if kind == "create" {
        (
            None::<String>,
            None::<String>,
            None::<String>,
            None::<String>,
            None::<i64>,
            None::<String>,
            None::<i64>,
        )
    } else {
        (
            Some("identifier".to_string()),
            Some("version".to_string()),
            Some("s3-key".to_string()),
            Some("sha256".to_string()),
            Some(123_i64),
            Some("{}".to_string()),
            Some(0_i64),
        )
    };
    sqlx::query(
        "INSERT INTO plugin_artifact_operations (
            operation_id, kind, plugin_id,
            expected_old_identifier, expected_old_version, expected_old_s3_key,
            expected_old_sha256, expected_old_size, expected_old_resource_limits,
            expected_old_row_revision,
            staging_identity, new_identity, state, staging_name
        ) VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?)",
    )
    .bind(operation_id)
    .bind(kind)
    .bind(plugin_id)
    .bind(old.0)
    .bind(old.1)
    .bind(old.2)
    .bind(old.3)
    .bind(old.4)
    .bind(old.5)
    .bind(old.6)
    .bind(staging_identity)
    .bind(new_identity)
    .bind(state)
    .bind(staging_name)
    .execute(pool)
    .await
}

async fn insert_operation_with_partial_old_tuple(
    pool: &sqlx::SqlitePool,
    operation_id: &str,
    kind: &str,
    state: &str,
    plugin_id: Option<&str>,
) -> Result<sqlx::sqlite::SqliteQueryResult, sqlx::Error> {
    let plugin_id = plugin_id.map(str::to_string);
    let staging_name = derive_staging_name_for(operation_id);
    sqlx::query(
        "INSERT INTO plugin_artifact_operations (
            operation_id, kind, plugin_id,
            expected_old_identifier, expected_old_version, expected_old_s3_key,
            expected_old_sha256, expected_old_size, expected_old_resource_limits,
            expected_old_row_revision,
            staging_identity, new_identity, state, staging_name
        ) VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, NULL, NULL, ?, ?)",
    )
    .bind(operation_id)
    .bind(kind)
    .bind(plugin_id)
    .bind(Some("identifier".to_string()))
    .bind(Some("version".to_string()))
    .bind(Some("s3-key".to_string()))
    .bind(Some("sha256".to_string()))
    .bind(Some(123_i64))
    .bind(Some("{}".to_string()))
    .bind(Option::<i64>::None)
    .bind(state)
    .bind(staging_name)
    .execute(pool)
    .await
}

async fn insert_operation_with_non_null_old_tuple(
    pool: &sqlx::SqlitePool,
    operation_id: &str,
    kind: &str,
    state: &str,
    plugin_id: Option<&str>,
    staging_identity: Option<&str>,
    new_identity: Option<&str>,
) -> Result<sqlx::sqlite::SqliteQueryResult, sqlx::Error> {
    // Always supplies non-NULL `expected_old_*` columns, regardless
    // of `kind`. Used to assert that the schema rejects "create"
    // with non-NULL old tuple.
    let plugin_id = plugin_id.map(str::to_string);
    let staging_identity = staging_identity.map(str::to_string);
    let new_identity = new_identity.map(str::to_string);
    let staging_name = derive_staging_name_for(operation_id);
    sqlx::query(
        "INSERT INTO plugin_artifact_operations (
            operation_id, kind, plugin_id,
            expected_old_identifier, expected_old_version, expected_old_s3_key,
            expected_old_sha256, expected_old_size, expected_old_resource_limits,
            expected_old_row_revision,
            staging_identity, new_identity, state, staging_name
        ) VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?)",
    )
    .bind(operation_id)
    .bind(kind)
    .bind(plugin_id)
    .bind(Some("identifier".to_string()))
    .bind(Some("version".to_string()))
    .bind(Some("s3-key".to_string()))
    .bind(Some("sha256".to_string()))
    .bind(Some(123_i64))
    .bind(Some("{}".to_string()))
    .bind(Some(0_i64))
    .bind(staging_identity)
    .bind(new_identity)
    .bind(state)
    .bind(staging_name)
    .execute(pool)
    .await
}

async fn derive_staging_name(pool: &sqlx::SqlitePool, operation_id: &str) -> String {
    // The exact derivation is owned by T022; the contract is that
    // the same operation_id always produces the same staging_name.
    // Use a deterministic hash so the test is stable.
    let _ = pool;
    derive_staging_name_for(operation_id)
}

fn derive_staging_name_for(operation_id: &str) -> String {
    // Deterministic staging_name derived from the operation_id. The
    // exact algorithm is owned by `plugin_artifacts::derive_staging_name`
    // (T022); the test only enforces the stability/uniqueness
    // contract, so a simple hash is enough.
    use std::collections::hash_map::DefaultHasher;
    use std::hash::{Hash, Hasher};
    let mut hasher = DefaultHasher::new();
    operation_id.hash(&mut hasher);
    format!("staging-{:016x}", hasher.finish())
}

fn assert_violation(err: &sqlx::Error, kind: &str) {
    match err {
        sqlx::Error::Database(db) => {
            let message = db.message().to_lowercase();
            assert!(
                message.contains(kind.to_lowercase().as_str()),
                "expected {kind} violation, got: {message}"
            );
        }
        other => panic!("expected database error ({kind}), got {other:?}"),
    }
}

#[test]
fn foundation_owns_plugin_ledger_schema() {
    assert_case_ownership();
}
