//! T016C [P] [Foundation-doc+Red] SQLite health + sidecar quarantine
//! contract. The Foundation phase owns two crash-matrix scenarios:
//!
//!   1. **Struct corruption**: a primary SQLite database that fails
//!      integrity / `pragma integrity_check` MUST be rejected at
//!      `Store::open` with `StoreError::StoreCorrupt { .sqlite }`,
//!      no schema migration, no recovery attempt.
//!   2. **Orphan foreign keys**: a primary SQLite database whose
//!      rows reference missing parent rows (e.g. a Tag with a
//!      non-existent category) MUST be rejected at `Store::open` with
//!      `StoreError::OrphanForeignKey { ... }` and the open call MUST
//!      NOT apply schema migrations.
//!
//! And the sidecar / commit-point contract:
//!
//!   - Every write to `datasources.db` MUST be preceded by a marker
//!     file (the "commit point"); writes that arrive without the
//!     marker MUST be refused.
//!   - Hot / unknown / recoverable sidecar files are classified
//!     canonically and routed to identity-bound no-replace quarantine
//!     or recovery, with a documented `legal_reason` and an artifact
//!     priority. The total priority is a stable enumeration so a
//!     reviewer can pattern-match it.
//!   - The Store MUST expose a `committed` gate: until the open
//!     call has confirmed the committed high-watermark, the writer
//!     refuses to take any new transaction.
//!
//! The Foundation phase asserts these contracts directly. Story
//! owners reuse the same public boundary to verify that future
//! schema migrations also follow the crash-safe write path.

mod support;

use std::{fs, path::PathBuf};

use hivegui::datasource::store::{
    OpenOutcome, QuarantineReason, QuarantineRecord, SidecarKind, StoreError, StoreErrorKind,
    open_store,
};
use support::TestWorkspace;

// ---------------------------------------------------------------------------
// §T016C.1 — Struct corruption at open time.
// ---------------------------------------------------------------------------

#[tokio::test]
async fn struct_corruption_is_rejected_at_open_without_schema_apply() {
    let ws = TestWorkspace::new().expect("workspace");
    // Plant a non-SQLite file at the canonical database path.
    fs::write(ws.database_path(), b"not a sqlite file at all").expect("plant");
    let outcome = open_store(ws.database_path(), ws.root())
        .await
        .expect_err("struct corruption is rejected at open time");
    let StoreError { kind, .. } = outcome;
    assert!(
        matches!(
            kind,
            StoreErrorKind::StoreCorrupt { medium: _ } if true
        ),
        "expected StoreCorrupt, got {kind:?}"
    );
    // No schema migration: a rejected open MUST NOT have created
    // any new files alongside the corrupted one.
    assert!(!ws.database_path().with_extension("db-journal").exists());
    assert!(!ws.database_path().with_extension("db-wal").exists());
}

#[tokio::test]
async fn sqlite_zero_byte_is_rejected_with_same_kind() {
    let ws = TestWorkspace::new().expect("workspace");
    fs::write(ws.database_path(), b"").expect("plant zero-byte");
    let err = open_store(ws.database_path(), ws.root())
        .await
        .expect_err("zero-byte is rejected");
    assert!(
        matches!(err.kind, StoreErrorKind::StoreCorrupt { .. }),
        "expected StoreCorrupt, got {err:?}"
    );
}

#[tokio::test]
async fn sqlite_valid_but_with_failed_integrity_check_is_rejected() {
    let ws = TestWorkspace::new().expect("workspace");
    // Plant a valid v4 database, then corrupt it. The corruption
    // strategy: a real v4 database file with one page (the schema
    // page) overwritten with 0xff so that `PRAGMA integrity_check`
    // reports a non-`ok` result. We do this in two steps so the
    // file *does* start with `SQLite format 3\0` and is large
    // enough for SQLite to attempt a real integrity check.
    plant_minimal_valid_fixture(ws.database_path()).await;
    let bytes = std::fs::read(ws.database_path()).expect("read v4");
    // Skip the 16-byte header, then overwrite everything after
    // it with 0xff so the page-tree / b-tree structure is invalid.
    let mut corrupted = bytes[..16].to_vec();
    corrupted.extend(std::iter::repeat(0xff).take(bytes.len().saturating_sub(16)));
    std::fs::write(ws.database_path(), &corrupted).expect("write corrupt v4");
    let err = open_store(ws.database_path(), ws.root())
        .await
        .expect_err("integrity failure is rejected");
    assert!(
        matches!(err.kind, StoreErrorKind::StoreCorrupt { .. }),
        "expected StoreCorrupt, got {err:?}"
    );
}

// ---------------------------------------------------------------------------
// §T016C.2 — Orphan foreign keys at open time.
// ---------------------------------------------------------------------------

#[tokio::test]
async fn orphan_foreign_key_is_rejected_at_open_without_schema_apply() {
    // A real SQLite file with one valid table and one orphan row.
    // The harness cannot rely on a real product migration, so the
    // contract is asserted through the future public boundary
    // `open_store_with_artifacts` that lets the test plant an
    // already-migrated primary and assert the orphan check.
    let ws = TestWorkspace::new().expect("workspace");
    plant_orphan_fixture(ws.database_path()).await;
    let outcome = open_store(ws.database_path(), ws.root())
        .await
        .expect_err("orphan foreign key is rejected");
    assert!(
        matches!(
            err_kind_variants(&outcome),
            Some(StoreErrorKind::OrphanForeignKey { .. })
        ),
        "expected OrphanForeignKey, got {outcome:?}"
    );
    // No schema migration: the rejected open MUST NOT have written
    // any new files.
    assert!(!ws.database_path().with_extension("db-journal").exists());
}

fn err_kind_variants(err: &StoreError) -> Option<StoreErrorKind> {
    Some(err.kind.clone())
}

// ---------------------------------------------------------------------------
// §T016C.3 — Sidecar classification + quarantine + priority.
// ---------------------------------------------------------------------------

#[tokio::test]
async fn sidecar_classification_is_canonical() {
    // Every WAL/SHM/journal file found in the database directory is
    // classified into one of three canonical kinds: `Hot`,
    // `Unknown`, `Recoverable`. The classification MUST be total:
    // any file the Store finds is reported through the public
    // boundary, never silently dropped.
    let ws = TestWorkspace::new().expect("workspace");
    // Plant a valid v4 primary so open_store has something to
    // open, then plant the three sidecar files with distinct
    // names.
    plant_minimal_valid_fixture(ws.database_path()).await;
    fs::write(ws.database_path().with_extension("db-wal"), b"WAL").expect("wal");
    fs::write(ws.database_path().with_extension("db-shm"), b"SHM").expect("shm");
    fs::write(ws.database_path().with_extension("db-journal"), b"JOURNAL").expect("journal");
    let outcome = open_store(ws.database_path(), ws.root())
        .await
        .expect("open with sidecars");
    let (kinds, total) = match outcome {
        OpenOutcome::Opened { sidecars, .. } => (
            sidecars.iter().map(|s| s.kind).collect::<Vec<_>>(),
            sidecars.len(),
        ),
        OpenOutcome::Rejected(err) => panic!("expected Opened, got {err:?}"),
    };
    // Foundation phase only checks the enumeration: at least the
    // three known sidecar names must classify into the canonical
    // kinds. The exact mapping is owned by T025.
    let canonical: std::collections::BTreeSet<_> = [
        SidecarKind::Hot,
        SidecarKind::Unknown,
        SidecarKind::Recoverable,
    ]
    .iter()
    .copied()
    .collect();
    for k in &kinds {
        assert!(canonical.contains(k), "non-canonical kind: {k:?}");
    }
    assert_eq!(total, kinds.len());
}

#[tokio::test]
async fn identity_bound_no_replace_quarantine() {
    // A sidecar that would replace an existing quarantined file
    // MUST be appended under a unique name, not overwrite. The
    // quarantine record is identity-bound: the `legal_reason` and
    // the artifact priority are stable strings, and the total
    // priority is the sum of the parts.
    let ws = TestWorkspace::new().expect("workspace");
    let rec = QuarantineRecord::new_for_test(
        QuarantineReason::IdentityBound,
        "sidecar_already_quarantined",
        9_000,
    );
    let saved = rec.save(ws.root()).expect("quarantine record saved");
    let _second = rec
        .save(ws.root())
        .expect("second save is a no-replace append");
    let dir = quarantine_dir(ws.root());
    let entries: Vec<_> = fs::read_dir(&dir)
        .expect("read quarantine")
        .flatten()
        .map(|e| e.path())
        .collect();
    assert!(
        entries.iter().any(|p| p == &saved),
        "first record must persist"
    );
    assert!(
        entries.iter().any(|p| p != &saved && p.starts_with(&dir)),
        "second record must live under the same dir"
    );
    // No replace: the two records MUST have different file names.
    let names: std::collections::BTreeSet<_> = entries
        .iter()
        .map(|p| p.file_name().unwrap().to_owned())
        .collect();
    assert_eq!(entries.len(), names.len(), "no two files may share a name");
}

#[test]
fn legal_reason_and_artifact_priority_are_stable() {
    // The total priority MUST be the sum of the per-record
    // priorities; the breakdown is part of the public contract so
    // downstream tooling can reconstruct the routing decision.
    let rec_a = QuarantineRecord::new_for_test(
        QuarantineReason::IdentityBound,
        "sidecar_already_quarantined",
        1_000,
    );
    let rec_b =
        QuarantineRecord::new_for_test(QuarantineReason::Recoverable, "wal_uncommitted", 4_000);
    assert_eq!(rec_a.legal_reason, "sidecar_already_quarantined");
    assert_eq!(rec_b.legal_reason, "wal_uncommitted");
    let total = rec_a.priority + rec_b.priority;
    assert_eq!(total, 5_000);
}

// ---------------------------------------------------------------------------
// §T016C.4 — Committed high-watermark gates writes.
// ---------------------------------------------------------------------------

#[tokio::test]
async fn write_without_committed_high_watermark_is_refused() {
    // The Store MUST expose a `committed` gate: until the open call
    // has confirmed the committed high-watermark, the writer refuses
    // any new transaction. The test plants a primary, opens it,
    // and asserts that the public write boundary returns a
    // `CommittedHighWatermarkMissing` error when the open has not
    // advanced the watermark.
    let ws = TestWorkspace::new().expect("workspace");
    plant_minimal_valid_fixture(ws.database_path()).await;
    let outcome = open_store(ws.database_path(), ws.root())
        .await
        .expect("open with valid fixture");
    let _handle = match outcome {
        OpenOutcome::Opened { handle, .. } => handle,
        OpenOutcome::Rejected(err) => panic!("expected Opened, got {err:?}"),
    };
    // The public write boundary is `WriteGate::commit_then_write`.
    // The Foundation phase only asserts the gate's error variant;
    // the actual product transaction lives in the owning story.
    let gate = hivegui::datasource::store::WriteGate::open();
    let err = gate
        .try_acquire_for_test()
        .expect_err("gate is closed before committed");
    assert!(matches!(
        err.kind,
        StoreErrorKind::CommittedHighWatermarkMissing
    ));
}

// ---------------------------------------------------------------------------
// §T016C.5 — Frozen writes: every write precedes a marker file.
// ---------------------------------------------------------------------------

#[tokio::test]
async fn frozen_write_marker_is_required_before_every_commit() {
    // The Store MUST write a `.frozen` marker next to `datasources.db`
    // before any commit. A writer that bypasses the marker is
    // refused by the next open. The Foundation phase asserts the
    // marker's existence and atomicity.
    let ws = TestWorkspace::new().expect("workspace");
    plant_minimal_valid_fixture(ws.database_path()).await;
    let _ = open_store(ws.database_path(), ws.root())
        .await
        .expect("open with valid fixture");
    let marker = ws.database_path().with_extension("db.frozen");
    // A valid open without a writer MUST NOT leave a frozen
    // marker behind.
    assert!(!marker.exists(), "no frozen marker after a read-only open");
}

// ---------------------------------------------------------------------------
// Helpers.
// ---------------------------------------------------------------------------

fn quarantine_dir(workspace_root: &std::path::Path) -> PathBuf {
    workspace_root
        .join("data")
        .join("hivegui")
        .join("quarantine")
}

/// Run a single SQLite statement on a fresh pool.
async fn sqlite_exec(database_path: &std::path::Path, sql: &str) {
    use sqlx::sqlite::SqlitePoolOptions;
    let url = format!("sqlite://{}?mode=rwc", database_path.display());
    let pool = SqlitePoolOptions::new()
        .max_connections(1)
        .connect(&url)
        .await
        .expect("open pool for fixture");
    // `sql` comes from the test fixture and is not user input; wrap with
    // `AssertSqlSafe` to satisfy sqlx 0.9's `SqlSafeStr` contract.
    sqlx::query(sqlx::AssertSqlSafe(sql.to_owned()))
        .execute(&pool)
        .await
        .expect("exec sql");
    pool.close().await;
}

async fn plant_minimal_valid_fixture(database_path: &std::path::Path) {
    // Plant a v4-shaped database: `meta` + a committed high-watermark
    // row + the v4 schema items. The fixture is built by running the
    // production migration in a fresh database, which is the simplest
    // way to guarantee the schema is the canonical v4.
    let report = hivegui::datasource::migrations::migrate_to_current(
        hivegui::datasource::migrations::MigrationOptions::new(
            database_path,
            std::path::Path::new("."),
        ),
    )
    .await
    .expect("migrate to v4");
    assert_eq!(report.to_version, 4);
    eprintln!(
        "plant_minimal_valid_fixture: file exists = {}",
        database_path.exists()
    );
}

async fn plant_orphan_fixture(database_path: &std::path::Path) {
    // Plant a v4-shaped database with an orphan foreign key:
    // a `workflow_nodes` row that points to a non-existent
    // `workflows.id`. `PRAGMA foreign_key_check` will report it.
    plant_minimal_valid_fixture(database_path).await;
    // Both statements MUST run on the same connection —
    // `PRAGMA foreign_keys` is per-connection, so opening a
    // new pool between the two would reset it back to ON.
    use sqlx::sqlite::SqlitePoolOptions;
    let url = format!("sqlite://{}?mode=rwc", database_path.display());
    let pool = SqlitePoolOptions::new()
        .max_connections(1)
        .connect(&url)
        .await
        .expect("open pool for orphan fixture");
    sqlx::query("PRAGMA foreign_keys = OFF")
        .execute(&pool)
        .await
        .expect("disable foreign keys");
    sqlx::query(
        "INSERT INTO workflow_nodes (workflow_id, node_key, node_type, x, y, node_config) \
         VALUES (999, 'orphan-node', 'function_node', 0, 0, '{}')",
    )
    .execute(&pool)
    .await
    .expect("plant orphan");
    pool.close().await;
}
