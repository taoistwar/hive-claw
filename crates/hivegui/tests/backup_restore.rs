//! T129/T130 [US13] Age-encrypted backup export + restore contract.
//!
//! Source of truth: `specs/011-hivegui-standalone-mode/tasks.md` §T129
//! and §T130 (export + restore / switch verification).
//!
//! Red public boundaries the T129/T130 implementation must satisfy:
//!   - `hivegui::datasource::backup::BackupExporter::export_age` writes
//!     a passphrase-encrypted age stream that round-trips through
//!     [`BackupImporter::import_age`] using a matching passphrase.
//!   - The export archive carries a `manifest.json` with the six-tuple
//!     `{schema_version=1, role, UUID, restore_db_id,
//!      database_name=datasources.db, ownership_state=unarmed}`.
//!   - The export refuses to overwrite an existing target file (the
//!     caller is responsible for choosing a fresh target).
//!   - The export refuses symlinks / hardlinks / device files /
//!     FIFOs / sockets and other non-regular entries.
//!   - Format 1 archives are accepted on import; format 2 and 3 are
//!     migrated to format 1 transparently (the import returns a
//!     `format=1` archive). Integer kind codes, short node_type, and
//!     dot-named builtins are rejected.
//!   - The restore side writes to a staging directory and refuses to
//!     touch the canonical `datasources.db` directly.

#![allow(missing_docs)]

mod support;

use std::{
    fs,
    path::Path,
    time::{SystemTime, UNIX_EPOCH},
};

use hivegui::datasource::backup::{BackupExporter, BackupImporter, ExportError, ImportError};
use support::TestWorkspace;

fn unique_target_path(workspace: &TestWorkspace, label: &str) -> std::path::PathBuf {
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_nanos())
        .unwrap_or(0);
    workspace.root().join(format!("{label}-{nanos}.age.tar"))
}

fn write_seed_database(workspace: &TestWorkspace) -> std::path::PathBuf {
    let database = workspace.data_home().join("hivegui").join("datasources.db");
    fs::create_dir_all(database.parent().expect("database parent")).expect("mkdir");
    // Write a minimal SQLite header so the importer recognises the
    // file. The file is a placeholder; the importer does not parse
    // it directly, it just records the bytes.
    fs::write(&database, b"SQLite format 3\0placeholder-seed\0").expect("write seed database");
    database
}

#[tokio::test(flavor = "current_thread")]
async fn export_then_import_round_trip_recovers_seed_database() {
    let workspace = TestWorkspace::new().expect("test workspace");
    let database = write_seed_database(&workspace);
    let target = unique_target_path(&workspace, "roundtrip");

    let exporter = BackupExporter::new(&database);
    let passphrase = "T129-roundtrip-passphrase";
    exporter
        .export_age(&target, passphrase)
        .await
        .expect("export must succeed for a fresh target with a valid source");

    assert!(target.exists(), "export target must be created");
    assert!(
        target.metadata().expect("metadata").len() > 0,
        "export target must contain ciphertext"
    );

    let importer = BackupImporter::new(workspace.root().join("restore-staging"));
    let restored_database = importer
        .import_age(&target, passphrase, workspace.root().join("restore-out"))
        .await
        .expect("import must succeed with matching passphrase");
    let restored_bytes = fs::read(&restored_database).expect("read restored");
    assert_eq!(
        restored_bytes, b"SQLite format 3\0placeholder-seed\0",
        "round-tripped bytes must equal the seed bytes"
    );
}

#[tokio::test(flavor = "current_thread")]
async fn export_refuses_overwrite_of_existing_target() {
    let workspace = TestWorkspace::new().expect("test workspace");
    let database = write_seed_database(&workspace);
    let target = unique_target_path(&workspace, "overwrite");
    fs::write(&target, b"existing-artifact").expect("write target");

    let exporter = BackupExporter::new(&database);
    let result = exporter
        .export_age(&target, "T129-overwrite-passphrase")
        .await;
    match result {
        Err(ExportError::TargetExists(_)) => {}
        other => panic!("expected TargetExists, got {other:?}"),
    }
    // Original target must remain unchanged.
    let bytes = fs::read(&target).expect("read target");
    assert_eq!(
        bytes, b"existing-artifact",
        "target must not be overwritten"
    );
}

#[tokio::test(flavor = "current_thread")]
async fn export_refuses_symlinked_source() {
    let workspace = TestWorkspace::new().expect("test workspace");
    let database = write_seed_database(&workspace);
    // Replace the database with a symlink to a writable file.
    fs::remove_file(&database).expect("remove seed");
    let target_link = workspace.root().join("redirected-seed");
    fs::write(&target_link, b"redirected-bytes").expect("write link target");
    std::os::unix::fs::symlink(&target_link, &database).expect("symlink");

    let exporter = BackupExporter::new(&database);
    let result = exporter
        .export_age(
            &unique_target_path(&workspace, "symlink"),
            "T129-symlink-passphrase",
        )
        .await;
    match result {
        Err(ExportError::UnsafeSource(_)) => {}
        other => panic!("expected UnsafeSource, got {other:?}"),
    }
}

#[tokio::test(flavor = "current_thread")]
async fn import_with_wrong_passphrase_is_rejected() {
    let workspace = TestWorkspace::new().expect("test workspace");
    let database = write_seed_database(&workspace);
    let target = unique_target_path(&workspace, "wrongpass");

    let exporter = BackupExporter::new(&database);
    exporter
        .export_age(&target, "T129-correct-passphrase")
        .await
        .expect("export");

    let importer = BackupImporter::new(workspace.root().join("restore-staging"));
    let result = importer
        .import_age(
            &target,
            "T129-wrong-passphrase",
            workspace.root().join("restore-out"),
        )
        .await;
    match result {
        Err(ImportError::AuthenticationFailed) => {}
        other => panic!("expected AuthenticationFailed, got {other:?}"),
    }
}

#[tokio::test(flavor = "current_thread")]
async fn format_2_archive_is_accepted_and_normalised() {
    let workspace = TestWorkspace::new().expect("test workspace");
    let database = write_seed_database(&workspace);
    let target = unique_target_path(&workspace, "format2");

    let exporter = BackupExporter::with_format(&database, 2);
    exporter
        .export_age(&target, "T129-format2-passphrase")
        .await
        .expect("export format 2 must succeed");

    let importer = BackupImporter::new(workspace.root().join("restore-staging"));
    let restored_database = importer
        .import_age(
            &target,
            "T129-format2-passphrase",
            workspace.root().join("restore-out"),
        )
        .await
        .expect("import format 2 must succeed");
    let bytes = fs::read(&restored_database).expect("read restored");
    assert_eq!(bytes, b"SQLite format 3\0placeholder-seed\0");
}

#[tokio::test(flavor = "current_thread")]
async fn manifest_carries_six_tuple_unarmed_default() {
    let workspace = TestWorkspace::new().expect("test workspace");
    let database = write_seed_database(&workspace);
    let target = unique_target_path(&workspace, "manifest");

    let exporter = BackupExporter::new(&database);
    exporter
        .export_age(&target, "T129-manifest-passphrase")
        .await
        .expect("export");

    let importer = BackupImporter::new(workspace.root().join("restore-staging"));
    let manifest = importer
        .inspect_manifest(&target, "T129-manifest-passphrase")
        .await
        .expect("manifest inspect must succeed");
    assert_eq!(manifest.schema_version, 1);
    assert_eq!(manifest.role, "primary");
    assert_eq!(manifest.database_name, "datasources.db");
    assert_eq!(manifest.ownership_state, "unarmed");
    assert!(
        !manifest.restore_db_id.is_empty(),
        "restore_db_id must be a non-empty UUID"
    );
    assert!(!manifest.uuid.is_empty(), "uuid must be a non-empty UUID");
    // helpers
    let _ = Path::new(&manifest.database_name);
}
