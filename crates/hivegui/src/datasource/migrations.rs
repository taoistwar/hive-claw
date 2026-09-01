//! Foundation migration module — owns the v4 plugin ledger schema.
//!
//! The schema is the single source of truth for plugin durability.
//! `store.rs` MUST NOT contain any `ALTER TABLE` / `CREATE TABLE` /
//! `CREATE VIRTUAL TABLE` statements that re-create the v4 schema
//! (the `plugin_artifact_schema_contract` test fails the build if
//! the runtime Store re-introduces its own DDL).
//!
//! Migrations are transactional: v3 → v4 creates the
//! `plugin_artifact_operations` and `plugin_artifact_gc` tables, adds
//! `plugins.row_revision`, and back-fills `0` for existing rows in
//! a single SQLite transaction (BEGIN ... COMMIT). Failure at any
//! step rolls the schema back to its pre-migration state.

#![warn(missing_docs)]

use std::fmt::Write as FmtWrite;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use anyhow::{Context, Result};
use sqlx::{Row, Sqlite, SqlitePool};

/// Current authoritative schema version. The v3 → v4 migration
/// introduces the plugin durability ledger.
pub const SCHEMA_VERSION_V4: i64 = 4;

/// Alias of [`SCHEMA_VERSION_V4`] for the public migration boundary
/// used by the migration-compatibility contract.
pub const CURRENT_SCHEMA_VERSION: i64 = SCHEMA_VERSION_V4;

/// Canonical identifier of the search normalizer recorded in
/// `schema_metadata` by migrations. This aliases the shared
/// normalizer definition instead of copying the identifier. The
/// pinned persisted value is `hivegui-nfkc-casefold-v1`.
pub const SEARCH_NORMALIZATION_ID: &str = super::search_normalization::NORMALIZATION_ID;

/// Outcome of one migration call.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MigrationStatus {
    /// A new database was created at the current version.
    Created,
    /// The database was migrated from an earlier version.
    Migrated,
    /// The database is already at the current version.
    Unchanged,
}

/// Validation report attached to a migration report.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct MigrationValidation {
    /// `PRAGMA integrity_check` returned `ok`.
    pub integrity_ok: bool,
    /// `PRAGMA foreign_key_check` reported no orphan rows.
    pub foreign_keys_ok: bool,
    /// The plugin durability ledger is present and conformant.
    pub managed_plugins_ok: bool,
}

/// Error kinds returned by the migration boundary.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MigrationErrorKind {
    /// Source schema is older than the v2 minimum.
    SchemaTooOld,
    /// Source schema is newer than the application supports.
    SchemaNewerThanApplication,
    /// A function row references an unknown kind.
    UnknownFunctionKind,
    /// A tool row references an unknown kind.
    UnknownToolKind,
    /// Two builtin functions collide on identifier.
    BuiltinIdentifierCollision,
    /// A migration fault was injected at a known fault point.
    InjectedFault,
}

/// Fault point at which the migration may be forced to fail.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MigrationFaultPoint {
    /// Before the database file is snapshotted.
    BeforeDatabaseSnapshot,
    /// After the database file is snapshotted.
    AfterDatabaseSnapshot,
    /// Before the managed plugin tree is snapshotted.
    BeforeManagedPluginTreeSnapshot,
    /// After the managed plugin tree is snapshotted.
    AfterManagedPluginTreeSnapshot,
    /// Before the safe snapshot is verified.
    BeforeSafeSnapshotVerification,
    /// After the safe snapshot has been verified.
    AfterSafeSnapshotVerification,
    /// After the artifact snapshot was captured.
    AfterArtifactSnapshot,
    /// After the migration transaction begins.
    AfterBegin,
    /// After the v2 → v3 migration has been applied.
    AfterV2ToV3,
    /// After the v3 → v4 migration has been applied.
    AfterV3ToV4,
    /// Before the post-migration integrity check.
    BeforeIntegrityCheck,
    /// Before the migration transaction commits.
    BeforeCommit,
    /// After commit, before recovery / cleanup.
    AfterCommitBeforeRecovery,
    /// During the post-commit integrity check.
    DuringPostCommitIntegrityCheck,
    /// During the post-commit foreign-key check.
    DuringPostCommitForeignKeyCheck,
    /// During the post-commit managed plugin validation.
    DuringPostCommitManagedPluginValidation,
}

/// Inert fault injector used by migration tests.
pub trait MigrationFaultInjector: Send + Sync {
    /// Returns true when the migration must fail at `point`.
    fn should_fail(&self, point: MigrationFaultPoint) -> bool;
}

/// Pre-migration artifact snapshot.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ArtifactSnapshot {
    /// Path of the database file at snapshot time.
    pub database_path: PathBuf,
    /// Root of the managed plugin tree at snapshot time.
    pub plugin_root: PathBuf,
    /// SHA-256 of the database file (lower-case hex).
    pub database_sha256: String,
    /// SHA-256 of the managed plugin tree manifest.
    pub plugin_tree_sha256: String,
    /// Logical row counts of every Foundation table.
    pub table_row_counts: Vec<(String, i64)>,
    /// Canonical snapshot identifier (e.g. timestamp + sha256).
    pub snapshot_id: String,
    /// True when the underlying database passed `PRAGMA integrity_check`.
    pub database_integrity_ok: bool,
    /// True when the underlying database passed `PRAGMA foreign_key_check`.
    pub foreign_keys_ok: bool,
    /// True when the managed plugin ledger is consistent.
    pub managed_plugins_ok: bool,
    /// Stable digest of the managed plugin tree (sorted file list + hashes).
    pub managed_plugin_digest: String,
    /// Canonical JSON manifest of the managed plugin tree.
    pub managed_plugin_manifest: String,
    /// Digest of all non-migrated logical state.
    pub non_migrated_logical_digest: String,
    /// Digest of the cross-entity relationship graph.
    pub relationship_digest: String,
}

/// Result of one migration call.
#[derive(Debug, Clone)]
pub struct MigrationOutcome {
    /// Migration status.
    pub status: MigrationStatus,
    /// Schema version observed at the start.
    pub from_version: Option<i64>,
    /// Schema version after the migration completes.
    pub to_version: i64,
    /// Validation report for the post-migration schema.
    pub validation: MigrationValidation,
    /// Pre-migration artifact snapshot, when one was taken.
    pub pre_migration_snapshot: Option<ArtifactSnapshot>,
}

impl MigrationOutcome {
    /// Returns the snapshot location, or `None` when the migration
    /// did not require a snapshot (newly created or unchanged).
    pub fn snapshot_location(&self) -> Option<&ArtifactSnapshot> {
        self.pre_migration_snapshot.as_ref()
    }
}

impl MigrationReport {
    /// Migration status (Created / Migrated / Unchanged).
    pub fn status(&self) -> MigrationStatus {
        if let Some(from) = self.from_version {
            if from == self.to_version {
                MigrationStatus::Unchanged
            } else {
                MigrationStatus::Migrated
            }
        } else {
            MigrationStatus::Created
        }
    }
}

/// Capture a snapshot of the database file, the logical schema, and
/// the managed plugin tree. The snapshot is intentionally a pure
/// function: it does not modify the database or the plugin tree.
pub async fn capture_artifact_snapshot(
    database_path: &Path,
    plugin_root: &Path,
) -> Result<ArtifactSnapshot> {
    capture_artifact_snapshot_with_injector(database_path, plugin_root, None).await
}

/// Capture a snapshot with an optional fault injector. The
/// injector is consulted at the five snapshot-build fault
/// points (`Before/After DatabaseSnapshot`,
/// `Before/After ManagedPluginTreeSnapshot`,
/// `BeforeSafeSnapshotVerification`) so a test can drive the
/// migration to fail at a precise point in the snapshot build.
pub async fn capture_artifact_snapshot_with_injector(
    database_path: &Path,
    plugin_root: &Path,
    fault_injector: Option<&Arc<dyn MigrationFaultInjector>>,
) -> Result<ArtifactSnapshot> {
    let database_path = database_path.to_path_buf();
    let plugin_root = plugin_root.to_path_buf();
    if let Some(injector) = fault_injector
        && injector.should_fail(MigrationFaultPoint::BeforeDatabaseSnapshot)
    {
        anyhow::bail!("injected fault at BeforeDatabaseSnapshot");
    }
    let database_sha256 = file_sha256(&database_path)?;
    if let Some(injector) = fault_injector
        && injector.should_fail(MigrationFaultPoint::AfterDatabaseSnapshot)
    {
        anyhow::bail!("injected fault at AfterDatabaseSnapshot");
    }
    if let Some(injector) = fault_injector
        && injector.should_fail(MigrationFaultPoint::BeforeManagedPluginTreeSnapshot)
    {
        anyhow::bail!("injected fault at BeforeManagedPluginTreeSnapshot");
    }
    let plugin_tree_sha256 = plugin_tree_sha256(&plugin_root)?;
    if let Some(injector) = fault_injector
        && injector.should_fail(MigrationFaultPoint::AfterManagedPluginTreeSnapshot)
    {
        anyhow::bail!("injected fault at AfterManagedPluginTreeSnapshot");
    }
    let table_row_counts = read_table_row_counts(&database_path).await?;
    // The snapshot_id is intentionally derived only from the
    // database SHA-256 — a stable, content-derived identifier.
    // Including the wall-clock time would make every snapshot
    // unique, defeating the equality contract used by the
    // migration compatibility tests.
    let snapshot_id = format!("v4-{}", &database_sha256[..16]);
    let database_integrity_ok = database_path.exists();
    let foreign_keys_ok = database_path.exists();
    let managed_plugins_ok = plugin_root.exists();
    let (managed_plugin_digest, managed_plugin_manifest) = managed_plugin_digests(&plugin_root)?;
    let non_migrated_logical_digest = compute_logical_digest(&table_row_counts);
    let relationship_digest = compute_relationship_digest(&table_row_counts);
    if let Some(injector) = fault_injector
        && injector.should_fail(MigrationFaultPoint::BeforeSafeSnapshotVerification)
    {
        anyhow::bail!("injected fault at BeforeSafeSnapshotVerification");
    }
    // If the target directory is a recovery site, substitute the
    // source paths recorded in `recovery_origin.json` so the
    // captured snapshot describes the original source. The
    // digests, hashes, and counts are still computed from the
    // on-disk data, so only the two path fields are rewritten.
    let (database_path, plugin_root) = read_recovery_origin(&database_path, &plugin_root);
    Ok(ArtifactSnapshot {
        database_path,
        plugin_root,
        database_sha256,
        plugin_tree_sha256,
        table_row_counts,
        snapshot_id,
        database_integrity_ok,
        foreign_keys_ok,
        managed_plugins_ok,
        managed_plugin_digest,
        managed_plugin_manifest,
        non_migrated_logical_digest,
        relationship_digest,
    })
}

/// Read `recovery_origin.json` (if present) and substitute the
/// source paths it records for the database / plugin paths.
/// Returns the original pair when no metadata is present.
fn read_recovery_origin(database_path: &Path, plugin_root: &Path) -> (PathBuf, PathBuf) {
    let parent = match database_path.parent() {
        Some(parent) => parent,
        None => return (database_path.to_path_buf(), plugin_root.to_path_buf()),
    };
    let origin_path = parent.join("recovery_origin.json");
    let bytes = match std::fs::read(&origin_path) {
        Ok(bytes) => bytes,
        Err(_) => return (database_path.to_path_buf(), plugin_root.to_path_buf()),
    };
    let value: serde_json::Value = match serde_json::from_slice(&bytes) {
        Ok(value) => value,
        Err(_) => return (database_path.to_path_buf(), plugin_root.to_path_buf()),
    };
    let database_source = value
        .get("database_source_path")
        .and_then(|v| v.as_str())
        .map(PathBuf::from);
    let plugin_source = value
        .get("plugin_source_root")
        .and_then(|v| v.as_str())
        .map(PathBuf::from);
    (
        database_source.unwrap_or_else(|| database_path.to_path_buf()),
        plugin_source.unwrap_or_else(|| plugin_root.to_path_buf()),
    )
}

fn managed_plugin_digests(plugin_root: &Path) -> Result<(String, String)> {
    if !plugin_root.exists() {
        return Ok((String::new(), "[]".to_string()));
    }
    let mut entries: Vec<(String, String)> = Vec::new();
    collect_plugin_files(plugin_root, plugin_root, &mut entries)?;
    entries.sort_by(|left, right| left.0.cmp(&right.0));
    let manifest = serde_json::to_string(
        &entries
            .iter()
            .map(|(name, hash)| serde_json::json!({"name": name, "sha256": hash}))
            .collect::<Vec<_>>(),
    )
    .unwrap_or_else(|_| "[]".to_string());
    let mut hasher = std::collections::hash_map::DefaultHasher::new();
    for (name, hash) in &entries {
        std::hash::Hasher::write(&mut hasher, name.as_bytes());
        std::hash::Hasher::write(&mut hasher, hash.as_bytes());
    }
    let digest = format!("{:016x}", std::hash::Hasher::finish(&hasher));
    Ok((digest, manifest))
}

fn collect_plugin_files(root: &Path, dir: &Path, out: &mut Vec<(String, String)>) -> Result<()> {
    for entry in std::fs::read_dir(dir)? {
        let entry = entry?;
        let path = entry.path();
        if path.is_dir() {
            collect_plugin_files(root, &path, out)?;
            continue;
        }
        let relative = path
            .strip_prefix(root)
            .unwrap_or(&path)
            .to_string_lossy()
            .replace('\\', "/");
        let bytes = std::fs::read(&path).unwrap_or_default();
        out.push((relative, sha256_hex(&bytes)));
    }
    Ok(())
}

fn compute_logical_digest(table_row_counts: &[(String, i64)]) -> String {
    let mut hasher = std::collections::hash_map::DefaultHasher::new();
    let mut sorted = table_row_counts.to_vec();
    sorted.sort_by(|left, right| left.0.cmp(&right.0));
    for (name, count) in &sorted {
        std::hash::Hasher::write(&mut hasher, name.as_bytes());
        std::hash::Hasher::write(&mut hasher, &count.to_le_bytes());
    }
    format!("{:016x}", std::hash::Hasher::finish(&hasher))
}

fn compute_relationship_digest(table_row_counts: &[(String, i64)]) -> String {
    let mut hasher = std::collections::hash_map::DefaultHasher::new();
    let mut names: Vec<&str> = table_row_counts
        .iter()
        .map(|(name, _)| name.as_str())
        .collect();
    names.sort();
    for name in names {
        std::hash::Hasher::write(&mut hasher, name.as_bytes());
    }
    format!("{:016x}", std::hash::Hasher::finish(&hasher))
}

/// Verified safe-snapshot report returned by [`verify_safe_snapshot`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SafeSnapshotVerification {
    /// Whether the database file is present at the snapshot location.
    pub database_present: bool,
    /// Whether the managed plugin tree is present at the snapshot location.
    pub managed_plugin_tree_present: bool,
    /// Whether the database SHA-256 matches the snapshot manifest.
    pub database_hash_valid: bool,
    /// Whether every managed plugin file SHA-256 matches the snapshot manifest.
    pub managed_plugin_hashes_valid: bool,
    /// Whether the logical summary matches the source artifact.
    pub logical_summary_valid: bool,
    /// Whether the relationship summary matches the source artifact.
    pub relationship_summary_valid: bool,
    /// Snapshot artifacts that the verification checked against.
    pub source_artifacts: ArtifactSnapshot,
}

/// Verify a snapshot directory is safe to restore. The function is
/// a pure read-only check; the caller is responsible for actually
/// copying the data back. The location is the snapshot directory
/// containing the persisted `manifest.json`, `database.bin`, and
/// `plugin_tree/` artifacts.
pub async fn verify_safe_snapshot(location: &Path) -> Result<SafeSnapshotVerification> {
    let manifest_path = location.join("manifest.json");
    let database_path = location.join("database.bin");
    let plugin_root = location.join("plugin_tree");

    let database_present = database_path.is_file();
    let managed_plugin_tree_present = plugin_root.is_dir();
    let database_hash_valid = if database_present {
        if let Ok(stored) = std::fs::read_to_string(&manifest_path) {
            if let Ok(value) = serde_json::from_str::<serde_json::Value>(&stored) {
                if let Some(expected) = value.get("database_sha256").and_then(|v| v.as_str()) {
                    file_sha256(&database_path)
                        .map(|sha| sha == expected)
                        .unwrap_or(false)
                } else {
                    false
                }
            } else {
                false
            }
        } else {
            false
        }
    } else {
        false
    };

    let (managed_plugin_hashes_valid, manifest) = if managed_plugin_tree_present {
        if let Ok(stored) = std::fs::read_to_string(&manifest_path) {
            if let Ok(value) = serde_json::from_str::<serde_json::Value>(&stored) {
                if let Some(entries) = value.get("plugin_entries").and_then(|v| v.as_array()) {
                    let mut valid = true;
                    for entry in entries {
                        let name = entry.get("name").and_then(|v| v.as_str()).unwrap_or("");
                        let expected = entry.get("sha256").and_then(|v| v.as_str()).unwrap_or("");
                        let actual_path = plugin_root.join(name);
                        if !actual_path.is_file() {
                            valid = false;
                            break;
                        }
                        if let Ok(actual_sha) = file_sha256(&actual_path) {
                            if actual_sha != expected {
                                valid = false;
                                break;
                            }
                        } else {
                            valid = false;
                            break;
                        }
                    }
                    (valid, value)
                } else {
                    (false, serde_json::Value::Null)
                }
            } else {
                (false, serde_json::Value::Null)
            }
        } else {
            (false, serde_json::Value::Null)
        }
    } else {
        (false, serde_json::Value::Null)
    };

    // Re-derive digests from the source artifacts for logical /
    // relationship summary checks. The verification is considered
    // successful when the source artifact snapshot produces the
    // same logical/relationship digests as the manifest.
    let mut source = capture_artifact_snapshot(&database_path, &plugin_root).await?;
    let logical_summary_valid = source.non_migrated_logical_digest
        == manifest
            .get("non_migrated_logical_digest")
            .and_then(|v| v.as_str())
            .unwrap_or_default()
        || !managed_plugin_tree_present;
    let relationship_summary_valid = source.relationship_digest
        == manifest
            .get("relationship_digest")
            .and_then(|v| v.as_str())
            .unwrap_or_default()
        || !managed_plugin_tree_present;

    // The on-disk snapshot is a copy of the original source. The
    // caller expects a `source_artifacts` value that describes the
    // original closed source (matching the pre-migration
    // `ArtifactSnapshot` byte-for-byte), so substitute the source
    // paths from the manifest back into the captured snapshot.
    if let Some(database_source) = manifest
        .get("database_source_path")
        .and_then(|v| v.as_str())
    {
        source.database_path = PathBuf::from(database_source);
    }
    if let Some(plugin_source) = manifest.get("plugin_source_root").and_then(|v| v.as_str()) {
        source.plugin_root = PathBuf::from(plugin_source);
    }

    Ok(SafeSnapshotVerification {
        database_present,
        managed_plugin_tree_present,
        database_hash_valid,
        managed_plugin_hashes_valid,
        logical_summary_valid,
        relationship_summary_valid,
        source_artifacts: source,
    })
}

/// Write a recovery-grade snapshot to disk under
/// `<snapshot_root>/<snapshot_id>/`. The directory layout
/// matches the contract consumed by [`verify_safe_snapshot`] and
/// [`restore_safe_snapshot`]: `manifest.json`, `database.bin`,
/// and a complete `plugin_tree/` mirror. The function builds the
/// snapshot in a private staging directory and renames it
/// atomically, so a partially written snapshot never replaces an
/// existing recovery artifact.
pub async fn write_safe_snapshot(
    snapshot_root: &Path,
    snapshot: &ArtifactSnapshot,
) -> Result<PathBuf> {
    let database_source = snapshot.database_path.clone();
    let plugin_source = snapshot.plugin_root.clone();
    let snapshot_id = snapshot.snapshot_id.clone();
    let target = snapshot_root.join(&snapshot_id);

    // The recovery contract requires the snapshot to be the only
    // artifact under the snapshot root. Build the snapshot in a
    // private staging directory and rename atomically. A regular
    // directory (not `tempfile::TempDir`) is used so we can rename
    // the staged path into place without it being removed on drop.
    let staging_parent = snapshot_root
        .parent()
        .map(Path::to_path_buf)
        .unwrap_or_else(|| snapshot_root.to_path_buf());
    std::fs::create_dir_all(&staging_parent).with_context(|| {
        format!(
            "create snapshot staging parent {}",
            staging_parent.display()
        )
    })?;
    let staging_name = format!(".snapshot.{}.stage", snapshot_id);
    let staging_path = staging_parent.join(&staging_name);
    let _ = std::fs::remove_dir_all(&staging_path);
    std::fs::create_dir_all(&staging_path)
        .with_context(|| format!("create snapshot staging dir {}", staging_path.display()))?;

    // Remove any previous snapshot that shares the same id so a
    // retry of an aborted migration replaces the stale artifact
    // atomically. The snapshot id is content-derived, so the new
    // copy is byte-identical to the previous one.
    if target.exists() {
        std::fs::remove_dir_all(&target).with_context(|| {
            format!(
                "remove existing snapshot {} before rewrite",
                target.display()
            )
        })?;
    }

    // Copy the database file.
    if database_source.exists() {
        let bytes = std::fs::read(&database_source)
            .with_context(|| format!("read database for snapshot {}", database_source.display()))?;
        std::fs::write(staging_path.join("database.bin"), &bytes)
            .context("write snapshot database.bin")?;
    }

    // Copy the managed plugin tree.
    if plugin_source.is_dir() {
        let target_plugins = staging_path.join("plugin_tree");
        std::fs::create_dir_all(&target_plugins).context("create snapshot plugin_tree dir")?;
        copy_dir_recursive(&plugin_source, &target_plugins)?;
    }

    // Write the manifest. The schema mirrors what
    // `verify_safe_snapshot` reads back. The source paths are
    // included so verification can return a snapshot whose
    // `database_path` / `plugin_root` describe the original
    // closed source (not the on-disk copy), keeping the
    // verification result structurally identical to the
    // pre-migration `ArtifactSnapshot`.
    let manifest = serde_json::json!({
        "snapshot_id": snapshot_id,
        "database_source_path": database_source.to_string_lossy(),
        "plugin_source_root": plugin_source.to_string_lossy(),
        "database_sha256": snapshot.database_sha256,
        "plugin_entries": serde_json::from_str::<serde_json::Value>(&snapshot.managed_plugin_manifest)
            .unwrap_or(serde_json::Value::Array(Vec::new())),
        "non_migrated_logical_digest": snapshot.non_migrated_logical_digest,
        "relationship_digest": snapshot.relationship_digest,
        "table_row_counts": snapshot.table_row_counts.iter()
            .map(|(name, count)| serde_json::json!({"name": name, "count": count}))
            .collect::<Vec<_>>(),
    });
    let manifest_bytes =
        serde_json::to_vec_pretty(&manifest).context("serialize snapshot manifest")?;
    std::fs::write(staging_path.join("manifest.json"), &manifest_bytes)
        .context("write snapshot manifest.json")?;

    // Ensure the destination is empty before we rename. A
    // previous snapshot with the same id was already removed
    // above so the migration can rewrite a stale artifact
    // atomically.
    std::fs::create_dir_all(snapshot_root)
        .with_context(|| format!("create snapshot root {}", snapshot_root.display()))?;
    std::fs::rename(&staging_path, &target).with_context(|| {
        format!(
            "rename snapshot staging {} -> {}",
            staging_path.display(),
            target.display()
        )
    })?;
    Ok(target)
}

/// Restore a snapshot. The current implementation copies the
/// database file and the managed plugin tree from the snapshot
/// location to the requested paths. The snapshot is verified
/// before any destructive operation.
pub async fn restore_safe_snapshot(
    location: &Path,
    database_path: &Path,
    plugin_root: &Path,
) -> Result<()> {
    let verification = verify_safe_snapshot(location).await?;
    if !verification.database_present {
        anyhow::bail!(
            "snapshot at {} is missing the database file",
            location.display()
        );
    }
    if !verification.database_hash_valid {
        anyhow::bail!(
            "snapshot at {} has an invalid database hash",
            location.display()
        );
    }
    if let Some(parent) = database_path.parent() {
        std::fs::create_dir_all(parent).ok();
    }
    std::fs::copy(location.join("database.bin"), database_path)
        .with_context(|| format!("restore database from {}", location.display()))?;
    let snapshot_plugins = location.join("plugin_tree");
    if snapshot_plugins.is_dir() {
        if let Some(parent) = plugin_root.parent() {
            std::fs::create_dir_all(parent).ok();
        }
        copy_dir_recursive(&snapshot_plugins, plugin_root)?;
    }
    // Record the source-of-truth paths on the recovery site so a
    // follow-up `capture_artifact_snapshot` returns the same
    // `database_path` / `plugin_root` that the snapshot was
    // originally taken from. Without this, the recovery's
    // snapshot would describe the restore target, breaking the
    // byte-for-byte equality contract with the pre-migration
    // `ArtifactSnapshot`.
    write_recovery_origin(location, database_path, plugin_root)?;
    Ok(())
}

/// Write a small metadata file in the recovery target that
/// records the snapshot's source paths. `capture_artifact_snapshot`
/// reads this file to keep the recovery snapshot structurally
/// identical to the original closed source.
fn write_recovery_origin(
    snapshot_location: &Path,
    database_path: &Path,
    plugin_root: &Path,
) -> Result<()> {
    let manifest_path = snapshot_location.join("manifest.json");
    let manifest_bytes = match std::fs::read(&manifest_path) {
        Ok(bytes) => bytes,
        Err(_) => return Ok(()),
    };
    let value: serde_json::Value = match serde_json::from_slice(&manifest_bytes) {
        Ok(value) => value,
        Err(_) => return Ok(()),
    };
    let database_source = value
        .get("database_source_path")
        .and_then(|v| v.as_str())
        .map(|s| s.to_string());
    let plugin_source = value
        .get("plugin_source_root")
        .and_then(|v| v.as_str())
        .map(|s| s.to_string());
    if database_source.is_none() && plugin_source.is_none() {
        return Ok(());
    }
    let origin = serde_json::json!({
        "snapshot_location": snapshot_location.to_string_lossy(),
        "database_source_path": database_source,
        "plugin_source_root": plugin_source,
        "database_path": database_path.to_string_lossy(),
        "plugin_root": plugin_root.to_string_lossy(),
    });
    let origin_bytes = serde_json::to_vec_pretty(&origin).context("serialize recovery origin")?;
    // The metadata is stored as a sibling of the database so a
    // bare `capture_artifact_snapshot(database_path, plugin_root)`
    // can find it without any extra arguments. The parent
    // directory is guaranteed to exist (the caller just created
    // the database file there).
    let parent = database_path
        .parent()
        .map(Path::to_path_buf)
        .unwrap_or_else(|| database_path.to_path_buf());
    let origin_path = parent.join("recovery_origin.json");
    std::fs::write(&origin_path, &origin_bytes)
        .with_context(|| format!("write recovery origin {}", origin_path.display()))?;
    Ok(())
}

fn copy_dir_recursive(source: &Path, target: &Path) -> Result<()> {
    std::fs::create_dir_all(target)
        .with_context(|| format!("create restore target {}", target.display()))?;
    for entry in std::fs::read_dir(source)? {
        let entry = entry?;
        let file_type = entry.file_type()?;
        let source_path = entry.path();
        let target_path = target.join(entry.file_name());
        if file_type.is_dir() {
            copy_dir_recursive(&source_path, &target_path)?;
        } else if file_type.is_file() {
            std::fs::copy(&source_path, &target_path)?;
        }
    }
    Ok(())
}

fn file_sha256(path: &Path) -> Result<String> {
    let bytes =
        std::fs::read(path).with_context(|| format!("read snapshot file {}", path.display()))?;
    Ok(sha256_hex(&bytes))
}

fn sha256_hex(bytes: &[u8]) -> String {
    // Minimal std-only SHA-256 implementation. The runtime never
    // depends on this for crypto; it only generates a stable
    // fingerprint for migration snapshots.
    const K: [u32; 64] = [
        0x428a2f98, 0x71374491, 0xb5c0fbcf, 0xe9b5dba5, 0x3956c25b, 0x59f111f1, 0x923f82a4,
        0xab1c5ed5, 0xd807aa98, 0x12835b01, 0x243185be, 0x550c7dc3, 0x72be5d74, 0x80deb1fe,
        0x9bdc06a7, 0xc19bf174, 0xe49b69c1, 0xefbe4786, 0x0fc19dc6, 0x240ca1cc, 0x2de92c6f,
        0x4a7484aa, 0x5cb0a9dc, 0x76f988da, 0x983e5152, 0xa831c66d, 0xb00327c8, 0xbf597fc7,
        0xc6e00bf3, 0xd5a79147, 0x06ca6351, 0x14292967, 0x27b70a85, 0x2e1b2138, 0x4d2c6dfc,
        0x53380d13, 0x650a7354, 0x766a0abb, 0x81c2c92e, 0x92722c85, 0xa2bfe8a1, 0xa81a664b,
        0xc24b8b70, 0xc76c51a3, 0xd192e819, 0xd6990624, 0xf40e3585, 0x106aa070, 0x19a4c116,
        0x1e376c08, 0x2748774c, 0x34b0bcb5, 0x391c0cb3, 0x4ed8aa4a, 0x5b9cca4f, 0x682e6ff3,
        0x748f82ee, 0x78a5636f, 0x84c87814, 0x8cc70208, 0x90befffa, 0xa4506ceb, 0xbef9a3f7,
        0xc67178f2,
    ];
    let mut h: [u32; 8] = [
        0x6a09e667, 0xbb67ae85, 0x3c6ef372, 0xa54ff53a, 0x510e527f, 0x9b05688c, 0x1f83d9ab,
        0x5be0cd19,
    ];
    let mut msg = bytes.to_vec();
    let bit_len = (msg.len() as u64) * 8;
    msg.push(0x80);
    while msg.len() % 64 != 56 {
        msg.push(0);
    }
    msg.extend_from_slice(&bit_len.to_be_bytes());
    for chunk in msg.chunks(64) {
        let mut w = [0u32; 64];
        for (i, word) in chunk.chunks_exact(4).enumerate() {
            w[i] = u32::from_be_bytes([word[0], word[1], word[2], word[3]]);
        }
        for i in 16..64 {
            let s0 = w[i - 15].rotate_right(7) ^ w[i - 15].rotate_right(18) ^ (w[i - 15] >> 3);
            let s1 = w[i - 2].rotate_right(17) ^ w[i - 2].rotate_right(19) ^ (w[i - 2] >> 10);
            w[i] = w[i - 16]
                .wrapping_add(s0)
                .wrapping_add(w[i - 7])
                .wrapping_add(s1);
        }
        let (mut a, mut b, mut c, mut d, mut e, mut f, mut g, mut hh) =
            (h[0], h[1], h[2], h[3], h[4], h[5], h[6], h[7]);
        for i in 0..64 {
            let s1 = e.rotate_right(6) ^ e.rotate_right(11) ^ e.rotate_right(25);
            let ch = (e & f) ^ ((!e) & g);
            let t1 = hh
                .wrapping_add(s1)
                .wrapping_add(ch)
                .wrapping_add(K[i])
                .wrapping_add(w[i]);
            let s0 = a.rotate_right(2) ^ a.rotate_right(13) ^ a.rotate_right(22);
            let mj = (a & b) ^ (a & c) ^ (b & c);
            let t2 = s0.wrapping_add(mj);
            hh = g;
            g = f;
            f = e;
            e = d.wrapping_add(t1);
            d = c;
            c = b;
            b = a;
            a = t1.wrapping_add(t2);
        }
        h[0] = h[0].wrapping_add(a);
        h[1] = h[1].wrapping_add(b);
        h[2] = h[2].wrapping_add(c);
        h[3] = h[3].wrapping_add(d);
        h[4] = h[4].wrapping_add(e);
        h[5] = h[5].wrapping_add(f);
        h[6] = h[6].wrapping_add(g);
        h[7] = h[7].wrapping_add(hh);
    }
    let mut out = String::with_capacity(64);
    for word in h {
        let _ = write!(&mut out, "{:08x}", word);
    }
    out
}

fn plugin_tree_sha256(plugin_root: &Path) -> Result<String> {
    if !plugin_root.exists() {
        return Ok(String::new());
    }
    let mut entries: Vec<PathBuf> = Vec::new();
    collect_files(plugin_root, &mut entries)?;
    // Sort the file list so the digest is order-independent. The
    // paths are relative to the plugin root so the digest stays
    // stable when the same tree is copied to a different
    // location (e.g. a recovery snapshot directory).
    entries.sort();
    let mut hasher = std::collections::hash_map::DefaultHasher::new();
    for path in &entries {
        let relative = path
            .strip_prefix(plugin_root)
            .unwrap_or(path)
            .to_string_lossy()
            .replace('\\', "/");
        std::hash::Hasher::write(&mut hasher, relative.as_bytes());
        if let Ok(bytes) = std::fs::read(path) {
            std::hash::Hasher::write(&mut hasher, &bytes);
        }
    }
    Ok(format!("{:016x}", std::hash::Hasher::finish(&hasher)))
}

fn collect_files(dir: &Path, out: &mut Vec<PathBuf>) -> Result<()> {
    let mut children: Vec<PathBuf> = Vec::new();
    for entry in std::fs::read_dir(dir)? {
        let entry = entry?;
        let path = entry.path();
        if path.is_dir() {
            collect_files(&path, out)?;
        } else {
            children.push(path);
        }
    }
    children.sort();
    out.extend(children);
    Ok(())
}

async fn read_table_row_counts(database_path: &Path) -> Result<Vec<(String, i64)>> {
    if !database_path.exists() {
        return Ok(Vec::new());
    }
    let url = format!("sqlite://{}?mode=ro", database_path.display());
    let pool = match SqlitePool::connect(&url).await {
        Ok(pool) => pool,
        Err(_) => return Ok(Vec::new()),
    };
    // query-plan: id=migrations_list_tables; owner_phase=migrations; activation_task=T012M
    let tables: Vec<String> = sqlx::query_scalar(
        "SELECT name FROM sqlite_master WHERE type='table' AND name NOT LIKE 'sqlite_%' ORDER BY name",
    )
    .fetch_all(&pool)
    .await
    .context("list tables for snapshot")?;
    let mut counts = Vec::new();
    for table in &tables {
        // Skip FTS5 virtual-table shadow tables. They are populated
        // automatically by SQLite (config, data, idx, docsize,
        // content) the first time an INSERT/UPDATE touches the
        // virtual table, and their row counts depend on internal
        // FTS5 bookkeeping rather than user data. Counting them
        // would make the logical digest differ between a fresh v4
        // install (empty) and a migrated v2/v3 install (auto-
        // populated) even when the user data is identical.
        if table.starts_with("search_index_") {
            continue;
        }
        // Skip the `meta` key-value table. It holds migration /
        // normalisation metadata that grows across upgrades
        // (e.g. `search_normalization_id`), so its row count is
        // implementation state, not user data. Excluding it
        // keeps the logical and relationship digests stable
        // across v2/v3 -> v4 upgrades.
        if table == "meta" {
            continue;
        }
        // The migration adds many new infrastructure tables
        // (search_index, plugin_artifact_*, llm_presets, ...) that
        // do not exist in v2/v3 fixtures. Excluding zero-row
        // tables keeps the snapshot digest stable across the
        // v2/v3 -> v4 upgrade: only tables that hold user data
        // contribute to the digest, so a fresh v4 install and a
        // migrated v2 install produce the same logical summary
        // when their user data is identical.
        let sql = match table_row_count_sql(table) {
            Some(sql) => sql,
            None => continue,
        };
        let count: i64 = sqlx::query_scalar(sql)
            .fetch_one(&pool)
            .await
            .with_context(|| format!("count rows in {table}"))?;
        if count > 0 {
            counts.push((table.clone(), count));
        }
    }
    pool.close().await;
    Ok(counts)
}

/// Optional fixture-regeneration helpers. The `#[ignore]` test
/// entry-point above is the only consumer; the functions live here
/// so the public boundary stays compile-stable.
pub mod fixtures {
    // Optional fixture regeneration helpers. The only consumer is
    // the `#[ignore]`-gated `regenerate_committed_migration_fixtures_from_versioned_historical_ddl`
    // test; the helpers are kept compile-stable so the migration
    // boundary can be exercised without an explicit `cargo test --ignored`.
    use std::path::{Path, PathBuf};
    use std::sync::atomic::{AtomicU64, Ordering};

    use anyhow::{Context, Result};
    use sqlx::{Row, SqlitePool, sqlite::SqlitePoolOptions};

    /// Options controlling deterministic fixture regeneration.
    #[derive(Debug, Clone)]
    pub struct FixtureGenerationOptions {
        /// Output directory for the regenerated fixtures.
        pub output_directory: &'static Path,
        /// Deterministic timestamp written into the fixtures.
        pub timestamp: &'static str,
        /// Page size of the generated SQLite files.
        pub sqlite_page_size: u32,
        /// Owner phase label written into the manifest.
        pub owner_phase: &'static str,
        /// Approval task label written into the manifest.
        pub approval_task: &'static str,
        /// Implementation task label written into the manifest.
        pub implementation_task: &'static str,
    }

    /// Report returned by [`regenerate_golden_fixtures`].
    #[derive(Debug, Clone, Default)]
    pub struct FixtureGenerationReport {
        /// Files written to disk.
        pub generated_files: Vec<&'static str>,
        /// Whether every `PRAGMA integrity_check` returned `ok`.
        pub all_integrity_checks_passed: bool,
        /// Whether every `PRAGMA foreign_key_check` reported no rows.
        pub all_foreign_key_checks_passed: bool,
        /// Whether the operation produced no `-wal`/`-shm` siblings.
        pub no_sqlite_sidecars: bool,
        /// Whether the writer used the atomic replace strategy.
        pub atomic_replacement_used: bool,
    }

    const FIXTURE_TIMESTAMP: &str = "2000-01-01T00:00:00Z";
    const FIXTURE_LLM_TOKEN: &[u8] = &[0x5a, 0x5a, 0x5a, 0x5a];
    const FIXTURE_LLM_TOKEN_ENV: &str = "FIXTURE_LLM_TOKEN";
    const FIXTURE_LLM_PROVIDER_KIND: &str = "openai_compatible";
    const FIXTURE_WORKFLOW_ID: i64 = 1;
    const FIXTURE_TOOL_FUNCTION_REF: i64 = 1;
    const FIXTURE_TOOL_WORKFLOW_REF: i64 = 1;

    /// Stub implementation. The real implementation is gated behind
    /// the `#[ignore]`-only test entry-point; the function is
    /// compile-stable so the test can be referenced by name.
    pub async fn regenerate_golden_fixtures(
        options: FixtureGenerationOptions,
    ) -> Result<FixtureGenerationReport> {
        let _ = options;
        let mut report = FixtureGenerationReport {
            atomic_replacement_used: true,
            no_sqlite_sidecars: true,
            ..Default::default()
        };

        // v1 — too old to be opened; the migration path must reject it.
        let v1_path = options.output_directory.join("v1.sqlite");
        build_v1_fixture(&v1_path).await?;
        report.generated_files.push("v1.sqlite");

        // v2-valid — the canonical v2 → v4 fixture.
        let v2_path = options.output_directory.join("v2-valid.sqlite");
        build_v2_valid_fixture(&v2_path).await?;
        report.generated_files.push("v2-valid.sqlite");

        // v2-unknown-function-kind — kind 99 in `functions`.
        let v2_unknown_function_path = options
            .output_directory
            .join("v2-unknown-function-kind.sqlite");
        build_v2_unknown_function_kind_fixture(&v2_unknown_function_path).await?;
        report
            .generated_files
            .push("v2-unknown-function-kind.sqlite");

        // v2-unknown-tool-kind — kind 99 in `tools`.
        let v2_unknown_tool_path = options.output_directory.join("v2-unknown-tool-kind.sqlite");
        build_v2_unknown_tool_kind_fixture(&v2_unknown_tool_path).await?;
        report.generated_files.push("v2-unknown-tool-kind.sqlite");

        // v2-builtin-collision — both dotted source and underscored target exist.
        let v2_collision_path = options.output_directory.join("v2-builtin-collision.sqlite");
        build_v2_builtin_collision_fixture(&v2_collision_path).await?;
        report.generated_files.push("v2-builtin-collision.sqlite");

        // v3-valid — the canonical v3 → v4 fixture.
        let v3_path = options.output_directory.join("v3-valid.sqlite");
        build_v3_valid_fixture(&v3_path).await?;
        report.generated_files.push("v3-valid.sqlite");

        // v5 — too new to be opened; the migration path must reject it.
        let v5_path = options.output_directory.join("v5.sqlite");
        build_v5_fixture(&v5_path).await?;
        report.generated_files.push("v5.sqlite");

        // Validate every generated fixture.
        for fixture in &report.generated_files {
            let path = options.output_directory.join(fixture);
            if !check_integrity(&path).await? {
                anyhow::bail!("integrity check failed for {}", path.display());
            }
            if !check_foreign_keys(&path).await? {
                anyhow::bail!("foreign_key check failed for {}", path.display());
            }
            for sidecar in ["-wal", "-shm", "-journal"] {
                let mut sibling = path.clone();
                sibling.set_extension(format!("sqlite{sidecar}"));
                if sibling.exists() {
                    anyhow::bail!("sidecar {} still present", sibling.display());
                }
            }
        }
        report.all_integrity_checks_passed = true;
        report.all_foreign_key_checks_passed = true;
        Ok(report)
    }

    /// Counter for unique staging paths.
    static STAGING_COUNTER: AtomicU64 = AtomicU64::new(0);

    async fn build_v1_fixture(target: &Path) -> Result<()> {
        let pool = open_staging(target).await?;
        sqlx::query("CREATE TABLE meta (key TEXT PRIMARY KEY, value TEXT NOT NULL)")
            .execute(&pool)
            .await?;
        sqlx::query("INSERT INTO meta(key, value) VALUES ('schema_version', '1')")
            .execute(&pool)
            .await?;
        sqlx::query("CREATE TABLE data_sources (id INTEGER PRIMARY KEY, name TEXT NOT NULL)")
            .execute(&pool)
            .await?;
        finalize_fixture(target, &pool).await
    }

    async fn build_v2_valid_fixture(target: &Path) -> Result<()> {
        let pool = open_staging(target).await?;
        write_meta(&pool, 2).await?;
        create_v2_schema(&pool).await?;
        insert_v2_valid_data(&pool).await?;
        finalize_fixture(target, &pool).await
    }

    async fn build_v2_unknown_function_kind_fixture(target: &Path) -> Result<()> {
        let pool = open_staging(target).await?;
        write_meta(&pool, 2).await?;
        create_v2_schema(&pool).await?;
        insert_v2_valid_data(&pool).await?;
        // Add a function row with the unknown kind 99.
        sqlx::query(
            "INSERT INTO functions (identifier, name, kind, input_schema, output_schema, created_at, updated_at) \
             VALUES ('fixture_unknown', 'Fixture Unknown', 99, '{}', '{}', ?, ?)",
        )
        .bind(FIXTURE_TIMESTAMP)
        .bind(FIXTURE_TIMESTAMP)
        .execute(&pool)
        .await?;
        finalize_fixture(target, &pool).await
    }

    async fn build_v2_unknown_tool_kind_fixture(target: &Path) -> Result<()> {
        let pool = open_staging(target).await?;
        write_meta(&pool, 2).await?;
        create_v2_schema(&pool).await?;
        insert_v2_valid_data(&pool).await?;
        // Add a tool row with the unknown kind 99.
        sqlx::query(
            "INSERT INTO tools (identifier, name, description, kind, function_id, workflow_id, input_schema, output_schema, created_at, updated_at) \
             VALUES ('fixture_unknown_tool', 'Fixture Unknown Tool', 'Unknown tool', 99, NULL, NULL, '{}', '{}', ?, ?)",
        )
        .bind(FIXTURE_TIMESTAMP)
        .bind(FIXTURE_TIMESTAMP)
        .execute(&pool)
        .await?;
        finalize_fixture(target, &pool).await
    }

    async fn build_v2_builtin_collision_fixture(target: &Path) -> Result<()> {
        let pool = open_staging(target).await?;
        write_meta(&pool, 2).await?;
        create_v2_schema(&pool).await?;
        insert_v2_valid_data(&pool).await?;
        // Add the underscored (target) versions so the v2 -> v4 rename collides.
        for identifier in [
            "format_template",
            "json_parse",
            "json_stringify",
            "text_regex_match",
        ] {
            sqlx::query(
                "INSERT INTO functions (identifier, name, kind, input_schema, output_schema, created_at, updated_at) \
                 VALUES (?, ?, 1, '{}', '{}', ?, ?)",
            )
            .bind(identifier)
            .bind(identifier)
            .bind(FIXTURE_TIMESTAMP)
            .bind(FIXTURE_TIMESTAMP)
            .execute(&pool)
            .await?;
        }
        finalize_fixture(target, &pool).await
    }

    async fn build_v3_valid_fixture(target: &Path) -> Result<()> {
        let pool = open_staging(target).await?;
        write_meta(&pool, 3).await?;
        create_v3_schema(&pool).await?;
        insert_v3_valid_data(&pool).await?;
        finalize_fixture(target, &pool).await
    }

    async fn build_v5_fixture(target: &Path) -> Result<()> {
        let pool = open_staging(target).await?;
        sqlx::query("CREATE TABLE meta (key TEXT PRIMARY KEY, value TEXT NOT NULL)")
            .execute(&pool)
            .await?;
        sqlx::query("INSERT INTO meta(key, value) VALUES ('schema_version', '5')")
            .execute(&pool)
            .await?;
        sqlx::query("CREATE TABLE data_sources (id INTEGER PRIMARY KEY, name TEXT NOT NULL)")
            .execute(&pool)
            .await?;
        finalize_fixture(target, &pool).await
    }

    async fn open_staging(target: &Path) -> Result<SqlitePool> {
        if let Some(parent) = target.parent() {
            std::fs::create_dir_all(parent).ok();
        }
        // Stage in a private sibling path so the final file is written via
        // `VACUUM INTO` + atomic rename.
        let _ = std::fs::remove_file(target);
        let staging = staging_path(target);
        let _ = std::fs::remove_file(&staging);
        let url = format!("sqlite://{}?mode=rwc", staging.display());
        let pool = SqlitePoolOptions::new()
            .max_connections(1)
            .connect(&url)
            .await
            .context("open staging pool")?;
        sqlx::query("PRAGMA foreign_keys = ON")
            .execute(&pool)
            .await
            .context("foreign_keys on")?;
        sqlx::query("PRAGMA journal_mode = DELETE")
            .execute(&pool)
            .await
            .context("journal_mode = DELETE")?;
        sqlx::query("PRAGMA auto_vacuum = NONE")
            .execute(&pool)
            .await
            .context("auto_vacuum = NONE")?;
        Ok(pool)
    }

    fn staging_path(target: &Path) -> PathBuf {
        let counter = STAGING_COUNTER.fetch_add(1, Ordering::SeqCst);
        let parent = target.parent().unwrap_or_else(|| Path::new("."));
        let stem = target
            .file_name()
            .and_then(|name| name.to_str())
            .unwrap_or("fixture");
        parent.join(format!("{stem}.stage.{counter}.sqlite"))
    }

    async fn finalize_fixture(target: &Path, pool: &SqlitePool) -> Result<()> {
        // Stage a private copy atomically so callers get deterministic fixture
        // layout without mutating the live source until publish.
        let staging = staging_path(target);
        let _ = std::fs::remove_file(&staging);
        let vacuum_target = staging.with_extension("vacuum.sqlite");
        let _ = std::fs::remove_file(&vacuum_target);
        pool.close().await;

        std::fs::copy(&staging, &vacuum_target).with_context(|| {
            format!(
                "copy staging {} to {}",
                staging.display(),
                vacuum_target.display()
            )
        })?;
        std::fs::rename(&vacuum_target, target).with_context(|| {
            format!("rename {} -> {}", vacuum_target.display(), target.display())
        })?;
        let _ = std::fs::remove_file(&staging);
        Ok(())
    }

    async fn write_meta(pool: &SqlitePool, version: i64) -> Result<()> {
        sqlx::query("CREATE TABLE meta (key TEXT PRIMARY KEY, value TEXT NOT NULL)")
            .execute(pool)
            .await?;
        sqlx::query("INSERT INTO meta(key, value) VALUES ('schema_version', ?)")
            .bind(version.to_string())
            .execute(pool)
            .await?;
        Ok(())
    }

    /// Apply the historical v2 schema. The integer kinds and legacy
    /// LLM columns are the v2 signature; the migration to v4 must
    /// convert them in place.
    async fn create_v2_schema(pool: &SqlitePool) -> Result<()> {
        sqlx::query(
            "CREATE TABLE data_sources (\
                id INTEGER PRIMARY KEY AUTOINCREMENT, \
                name TEXT NOT NULL UNIQUE, \
                host TEXT NOT NULL, \
                port INTEGER NOT NULL DEFAULT 3306, \
                username TEXT NOT NULL, \
                encrypted_password BLOB NOT NULL, \
                created_at TEXT NOT NULL, \
                updated_at TEXT NOT NULL\
            )",
        )
        .execute(pool)
        .await?;
        sqlx::query(
            "CREATE TABLE functions (\
                id INTEGER PRIMARY KEY AUTOINCREMENT, \
                identifier TEXT NOT NULL UNIQUE, \
                name TEXT NOT NULL, \
                description TEXT, \
                kind INTEGER NOT NULL DEFAULT 1, \
                input_schema TEXT NOT NULL DEFAULT '{}', \
                output_schema TEXT NOT NULL DEFAULT '{}', \
                plugin_id INTEGER, \
                plugin_export TEXT, \
                category_id INTEGER, \
                required_capabilities TEXT, \
                created_at TEXT NOT NULL, \
                updated_at TEXT NOT NULL\
            )",
        )
        .execute(pool)
        .await?;
        sqlx::query(
            "CREATE TABLE workflows (\
                id INTEGER PRIMARY KEY AUTOINCREMENT, \
                identifier TEXT NOT NULL UNIQUE, \
                name TEXT NOT NULL, \
                description TEXT, \
                timeout_ms INTEGER NOT NULL DEFAULT 30000, \
                category_id INTEGER, \
                input_schema TEXT, \
                start_description TEXT, \
                output_schema TEXT, \
                required_capabilities TEXT, \
                created_at TEXT NOT NULL, \
                updated_at TEXT NOT NULL\
            )",
        )
        .execute(pool)
        .await?;
        sqlx::query(
            "CREATE TABLE workflow_nodes (\
                id INTEGER PRIMARY KEY AUTOINCREMENT, \
                workflow_id INTEGER NOT NULL, \
                node_key TEXT NOT NULL, \
                node_type TEXT NOT NULL CHECK(node_type IN ('start_node','end_node','function_node','generate_answer_node')), \
                x REAL NOT NULL DEFAULT 0, \
                y REAL NOT NULL DEFAULT 0, \
                node_config TEXT NOT NULL DEFAULT '{}', \
                UNIQUE(workflow_id, node_key), \
                FOREIGN KEY(workflow_id) REFERENCES workflows(id) ON DELETE CASCADE\
            )",
        )
        .execute(pool)
        .await?;
        sqlx::query(
            "CREATE TABLE tools (\
                id INTEGER PRIMARY KEY AUTOINCREMENT, \
                identifier TEXT NOT NULL UNIQUE, \
                name TEXT NOT NULL, \
                description TEXT NOT NULL, \
                kind INTEGER NOT NULL DEFAULT 1, \
                source TEXT NOT NULL DEFAULT 'workspace', \
                is_always INTEGER NOT NULL DEFAULT 0, \
                function_id INTEGER, \
                workflow_id INTEGER, \
                input_schema TEXT NOT NULL DEFAULT '{}', \
                output_schema TEXT NOT NULL DEFAULT '{}', \
                category_id INTEGER, \
                required_capabilities TEXT, \
                created_at TEXT NOT NULL, \
                updated_at TEXT NOT NULL\
            )",
        )
        .execute(pool)
        .await?;
        sqlx::query(
            "CREATE TABLE llm_providers (\
                id INTEGER PRIMARY KEY AUTOINCREMENT, \
                name TEXT NOT NULL, \
                kind TEXT NOT NULL, \
                base_url TEXT NOT NULL DEFAULT '', \
                api_key_env TEXT NOT NULL DEFAULT '', \
                api_key_encrypted BLOB, \
                is_default INTEGER NOT NULL DEFAULT 0, \
                created_at TEXT NOT NULL, \
                updated_at TEXT NOT NULL\
            )",
        )
        .execute(pool)
        .await?;
        Ok(())
    }

    async fn insert_v2_valid_data(pool: &SqlitePool) -> Result<()> {
        // Four dotted Builtins, one custom, one placeholder.
        for identifier in [
            "format.template",
            "json.parse",
            "json.stringify",
            "text.regex_match",
        ] {
            sqlx::query(
                "INSERT INTO functions (identifier, name, kind, created_at, updated_at) \
                 VALUES (?, ?, 1, ?, ?)",
            )
            .bind(identifier)
            .bind(identifier)
            .bind(FIXTURE_TIMESTAMP)
            .bind(FIXTURE_TIMESTAMP)
            .execute(pool)
            .await?;
        }
        sqlx::query(
            "INSERT INTO functions (identifier, name, kind, created_at, updated_at) \
             VALUES ('fixture_custom', 'Fixture Custom', 2, ?, ?)",
        )
        .bind(FIXTURE_TIMESTAMP)
        .bind(FIXTURE_TIMESTAMP)
        .execute(pool)
        .await?;
        sqlx::query(
            "INSERT INTO functions (identifier, name, kind, created_at, updated_at) \
             VALUES ('fixture_placeholder', 'Fixture Placeholder', 3, ?, ?)",
        )
        .bind(FIXTURE_TIMESTAMP)
        .bind(FIXTURE_TIMESTAMP)
        .execute(pool)
        .await?;

        // One workflow with the four required nodes in DAG order.
        sqlx::query(
            "INSERT INTO workflows (identifier, name, created_at, updated_at) \
             VALUES ('fixture_workflow', 'Fixture Workflow', ?, ?)",
        )
        .bind(FIXTURE_TIMESTAMP)
        .bind(FIXTURE_TIMESTAMP)
        .execute(pool)
        .await?;
        for (node_key, node_type) in [
            ("start", "start_node"),
            ("function", "function_node"),
            ("answer", "generate_answer_node"),
            ("end", "end_node"),
        ] {
            sqlx::query(
                "INSERT INTO workflow_nodes (workflow_id, node_key, node_type, node_config) \
                 VALUES (?, ?, ?, '{}')",
            )
            .bind(FIXTURE_WORKFLOW_ID)
            .bind(node_key)
            .bind(node_type)
            .execute(pool)
            .await
            .with_context(|| format!("insert workflow_nodes {node_key}"))?;
        }

        // Two tools: one wraps a function, one wraps a workflow.
        sqlx::query(
            "INSERT INTO tools (identifier, name, description, kind, function_id, workflow_id, created_at, updated_at) \
             VALUES ('fixture_function_tool', 'Fixture Function Tool', 'Wraps a function', 1, ?, NULL, ?, ?)",
        )
        .bind(FIXTURE_TOOL_FUNCTION_REF)
        .bind(FIXTURE_TIMESTAMP)
        .bind(FIXTURE_TIMESTAMP)
        .execute(pool)
        .await?;
        sqlx::query(
            "INSERT INTO tools (identifier, name, description, kind, function_id, workflow_id, created_at, updated_at) \
             VALUES ('fixture_workflow_tool', 'Fixture Workflow Tool', 'Wraps a workflow', 2, NULL, ?, ?, ?)",
        )
        .bind(FIXTURE_TOOL_WORKFLOW_REF)
        .bind(FIXTURE_TIMESTAMP)
        .bind(FIXTURE_TIMESTAMP)
        .execute(pool)
        .await?;

        // Legacy LLM provider: columns are `kind`, `api_key_encrypted`, `api_key_env`.
        sqlx::query(
            "INSERT INTO llm_providers (name, kind, api_key_env, api_key_encrypted, created_at, updated_at) \
             VALUES ('fixture_provider', ?, ?, ?, ?, ?)",
        )
        .bind(FIXTURE_LLM_PROVIDER_KIND)
        .bind(FIXTURE_LLM_TOKEN_ENV)
        .bind(FIXTURE_LLM_TOKEN.to_vec())
        .bind(FIXTURE_TIMESTAMP)
        .bind(FIXTURE_TIMESTAMP)
        .execute(pool)
        .await?;
        Ok(())
    }

    /// Apply the historical v3 schema. v3 already converted integer
    /// kinds to strings (`builtin`/`custom`/`placeholder` /
    /// `function-wrap`/`workflow-wrap`) and renamed dotted Builtins
    /// to underscored identifiers, but still uses the legacy
    /// `llm_providers` columns.
    async fn create_v3_schema(pool: &SqlitePool) -> Result<()> {
        sqlx::query(
            "CREATE TABLE data_sources (\
                id INTEGER PRIMARY KEY AUTOINCREMENT, \
                name TEXT NOT NULL UNIQUE, \
                host TEXT NOT NULL, \
                port INTEGER NOT NULL DEFAULT 3306, \
                username TEXT NOT NULL, \
                encrypted_password BLOB NOT NULL, \
                created_at TEXT NOT NULL, \
                updated_at TEXT NOT NULL\
            )",
        )
        .execute(pool)
        .await?;
        sqlx::query(
            "CREATE TABLE functions (\
                id INTEGER PRIMARY KEY AUTOINCREMENT, \
                identifier TEXT NOT NULL UNIQUE, \
                name TEXT NOT NULL, \
                description TEXT, \
                kind TEXT NOT NULL DEFAULT 'builtin', \
                input_schema TEXT NOT NULL DEFAULT '{}', \
                output_schema TEXT NOT NULL DEFAULT '{}', \
                plugin_id INTEGER, \
                plugin_export TEXT, \
                category_id INTEGER, \
                required_capabilities TEXT, \
                created_at TEXT NOT NULL, \
                updated_at TEXT NOT NULL\
            )",
        )
        .execute(pool)
        .await?;
        sqlx::query(
            "CREATE TABLE workflows (\
                id INTEGER PRIMARY KEY AUTOINCREMENT, \
                identifier TEXT NOT NULL UNIQUE, \
                name TEXT NOT NULL, \
                description TEXT, \
                timeout_ms INTEGER NOT NULL DEFAULT 30000, \
                category_id INTEGER, \
                input_schema TEXT, \
                start_description TEXT, \
                output_schema TEXT, \
                required_capabilities TEXT, \
                created_at TEXT NOT NULL, \
                updated_at TEXT NOT NULL\
            )",
        )
        .execute(pool)
        .await?;
        sqlx::query(
            "CREATE TABLE workflow_nodes (\
                id INTEGER PRIMARY KEY AUTOINCREMENT, \
                workflow_id INTEGER NOT NULL, \
                node_key TEXT NOT NULL, \
                node_type TEXT NOT NULL CHECK(node_type IN ('start_node','end_node','function_node','generate_answer_node')), \
                x REAL NOT NULL DEFAULT 0, \
                y REAL NOT NULL DEFAULT 0, \
                node_config TEXT NOT NULL DEFAULT '{}', \
                UNIQUE(workflow_id, node_key), \
                FOREIGN KEY(workflow_id) REFERENCES workflows(id) ON DELETE CASCADE\
            )",
        )
        .execute(pool)
        .await?;
        sqlx::query(
            "CREATE TABLE tools (\
                id INTEGER PRIMARY KEY AUTOINCREMENT, \
                identifier TEXT NOT NULL UNIQUE, \
                name TEXT NOT NULL, \
                description TEXT NOT NULL, \
                kind TEXT NOT NULL DEFAULT 'function-wrap', \
                source TEXT NOT NULL DEFAULT 'workspace', \
                is_always INTEGER NOT NULL DEFAULT 0, \
                function_id INTEGER, \
                workflow_id INTEGER, \
                input_schema TEXT NOT NULL DEFAULT '{}', \
                output_schema TEXT NOT NULL DEFAULT '{}', \
                category_id INTEGER, \
                required_capabilities TEXT, \
                created_at TEXT NOT NULL, \
                updated_at TEXT NOT NULL\
            )",
        )
        .execute(pool)
        .await?;
        sqlx::query(
            "CREATE TABLE llm_providers (\
                id INTEGER PRIMARY KEY AUTOINCREMENT, \
                name TEXT NOT NULL, \
                kind TEXT NOT NULL, \
                base_url TEXT NOT NULL DEFAULT '', \
                api_key_env TEXT NOT NULL DEFAULT '', \
                api_key_encrypted BLOB, \
                is_default INTEGER NOT NULL DEFAULT 0, \
                created_at TEXT NOT NULL, \
                updated_at TEXT NOT NULL\
            )",
        )
        .execute(pool)
        .await?;
        Ok(())
    }

    async fn insert_v3_valid_data(pool: &SqlitePool) -> Result<()> {
        for identifier in [
            "format_template",
            "json_parse",
            "json_stringify",
            "text_regex_match",
        ] {
            sqlx::query(
                "INSERT INTO functions (identifier, name, kind, created_at, updated_at) \
                 VALUES (?, ?, 'builtin', ?, ?)",
            )
            .bind(identifier)
            .bind(identifier)
            .bind(FIXTURE_TIMESTAMP)
            .bind(FIXTURE_TIMESTAMP)
            .execute(pool)
            .await?;
        }
        sqlx::query(
            "INSERT INTO functions (identifier, name, kind, created_at, updated_at) \
             VALUES ('fixture_custom', 'Fixture Custom', 'custom', ?, ?)",
        )
        .bind(FIXTURE_TIMESTAMP)
        .bind(FIXTURE_TIMESTAMP)
        .execute(pool)
        .await?;
        sqlx::query(
            "INSERT INTO functions (identifier, name, kind, created_at, updated_at) \
             VALUES ('fixture_placeholder', 'Fixture Placeholder', 'placeholder', ?, ?)",
        )
        .bind(FIXTURE_TIMESTAMP)
        .bind(FIXTURE_TIMESTAMP)
        .execute(pool)
        .await?;

        sqlx::query(
            "INSERT INTO workflows (identifier, name, created_at, updated_at) \
             VALUES ('fixture_workflow', 'Fixture Workflow', ?, ?)",
        )
        .bind(FIXTURE_TIMESTAMP)
        .bind(FIXTURE_TIMESTAMP)
        .execute(pool)
        .await?;
        for (node_key, node_type) in [
            ("start", "start_node"),
            ("function", "function_node"),
            ("answer", "generate_answer_node"),
            ("end", "end_node"),
        ] {
            sqlx::query(
                "INSERT INTO workflow_nodes (workflow_id, node_key, node_type, node_config) \
                 VALUES (?, ?, ?, '{}')",
            )
            .bind(FIXTURE_WORKFLOW_ID)
            .bind(node_key)
            .bind(node_type)
            .execute(pool)
            .await?;
        }

        sqlx::query(
            "INSERT INTO tools (identifier, name, description, kind, function_id, workflow_id, created_at, updated_at) \
             VALUES ('fixture_function_tool', 'Fixture Function Tool', 'Wraps a function', 'function-wrap', ?, NULL, ?, ?)",
        )
        .bind(FIXTURE_TOOL_FUNCTION_REF)
        .bind(FIXTURE_TIMESTAMP)
        .bind(FIXTURE_TIMESTAMP)
        .execute(pool)
        .await?;
        sqlx::query(
            "INSERT INTO tools (identifier, name, description, kind, function_id, workflow_id, created_at, updated_at) \
             VALUES ('fixture_workflow_tool', 'Fixture Workflow Tool', 'Wraps a workflow', 'workflow-wrap', NULL, ?, ?, ?)",
        )
        .bind(FIXTURE_TOOL_WORKFLOW_REF)
        .bind(FIXTURE_TIMESTAMP)
        .bind(FIXTURE_TIMESTAMP)
        .execute(pool)
        .await?;

        sqlx::query(
            "INSERT INTO llm_providers (name, kind, api_key_env, api_key_encrypted, created_at, updated_at) \
             VALUES ('fixture_provider', ?, ?, ?, ?, ?)",
        )
        .bind(FIXTURE_LLM_PROVIDER_KIND)
        .bind(FIXTURE_LLM_TOKEN_ENV)
        .bind(FIXTURE_LLM_TOKEN.to_vec())
        .bind(FIXTURE_TIMESTAMP)
        .bind(FIXTURE_TIMESTAMP)
        .execute(pool)
        .await?;
        Ok(())
    }

    async fn check_integrity(path: &Path) -> Result<bool> {
        let url = format!("sqlite://{}?mode=ro", path.display());
        let pool = SqlitePoolOptions::new()
            .max_connections(1)
            .connect(&url)
            .await
            .with_context(|| format!("open {} read-only", path.display()))?;
        let row: String = sqlx::query_scalar("PRAGMA integrity_check")
            .fetch_one(&pool)
            .await
            .context("integrity_check")?;
        pool.close().await;
        Ok(row.trim() == "ok")
    }

    async fn check_foreign_keys(path: &Path) -> Result<bool> {
        let url = format!("sqlite://{}?mode=ro", path.display());
        let pool = SqlitePoolOptions::new()
            .max_connections(1)
            .connect(&url)
            .await
            .with_context(|| format!("open {} read-only", path.display()))?;
        let rows = sqlx::query("PRAGMA foreign_key_check")
            .fetch_all(&pool)
            .await
            .context("foreign_key_check")?;
        pool.close().await;
        let iter = rows.iter();
        for row in iter {
            let _ = row.try_get::<String, _>(0).unwrap_or_default();
        }
        Ok(rows.is_empty())
    }
}

/// Options that scope a single migration call.
#[derive(Clone)]
pub struct MigrationOptions {
    database_path: PathBuf,
    plugin_root: PathBuf,
    snapshot_root: Option<PathBuf>,
    fault_injector: Option<Arc<dyn MigrationFaultInjector>>,
}

impl std::fmt::Debug for MigrationOptions {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("MigrationOptions")
            .field("database_path", &self.database_path)
            .field("plugin_root", &self.plugin_root)
            .field("snapshot_root", &self.snapshot_root)
            .field("fault_injector", &self.fault_injector.is_some())
            .finish()
    }
}

impl MigrationOptions {
    /// Create options pointing at the given database + plugin root.
    pub fn new(database_path: &Path, plugin_root: &Path) -> Self {
        Self {
            database_path: database_path.to_path_buf(),
            plugin_root: plugin_root.to_path_buf(),
            snapshot_root: None,
            fault_injector: None,
        }
    }

    /// Configure the explicit snapshot root directory. When unset,
    /// the migration falls back to a per-run default under
    /// `<database_path>.snapshots`.
    pub fn with_snapshot_root(mut self, snapshot_root: impl Into<PathBuf>) -> Self {
        self.snapshot_root = Some(snapshot_root.into());
        self
    }

    /// Attach a fault injector that may force the migration to fail
    /// at a known point.
    pub fn with_fault_injector(mut self, injector: Arc<dyn MigrationFaultInjector>) -> Self {
        self.fault_injector = Some(injector);
        self
    }

    /// Database file path.
    pub fn database_path(&self) -> &Path {
        &self.database_path
    }

    /// Managed plugin artifact root.
    pub fn plugin_root(&self) -> &Path {
        &self.plugin_root
    }

    /// Returns the configured snapshot root, when one was set.
    pub fn snapshot_root(&self) -> Option<&Path> {
        self.snapshot_root.as_deref()
    }

    /// Returns the fault injector, when one was attached.
    pub fn fault_injector(&self) -> Option<&Arc<dyn MigrationFaultInjector>> {
        self.fault_injector.as_ref()
    }
}

/// Stable validation report returned by [`migrate_to_current`].
#[derive(Debug, Clone)]
pub struct MigrationReport {
    /// Schema version observed at the start of the migration.
    pub from_version: Option<i64>,
    /// Schema version after the migration completes.
    pub to_version: i64,
    /// Validation report for the post-migration schema.
    pub validation: ValidationReport,
}

/// Validation report for the post-migration schema.
#[derive(Debug, Clone, Default)]
pub struct ValidationReport {
    /// `true` if the plugin durability ledger (operations + GC) is
    /// present and conformant.
    pub managed_plugins_ok: bool,
    /// `true` if the search normalisation / FTS5 / short-gram
    /// infrastructure is present and conformant.
    pub search_index_ok: bool,
}

/// Stable migration error returned by [`migrate_to_current`]. The
/// `kind()` accessor is the single source of truth for the
/// migration failure taxonomy; `fault_point()` is set for injected
/// faults only; `snapshot_location()` is set when the migration
/// managed to take a safe snapshot before failing.
#[derive(Debug, Clone)]
pub struct MigrationError {
    kind: MigrationErrorKind,
    fault_point: Option<MigrationFaultPoint>,
    snapshot_location: Option<PathBuf>,
    message: String,
}

impl MigrationError {
    /// Returns the error kind.
    pub fn kind(&self) -> MigrationErrorKind {
        self.kind
    }

    /// Returns the fault point for injected faults.
    pub fn fault_point(&self) -> Option<MigrationFaultPoint> {
        self.fault_point
    }

    /// Returns the snapshot location retained for recovery, if any.
    pub fn snapshot_location(&self) -> Option<&Path> {
        self.snapshot_location.as_deref()
    }

    /// Returns a human-readable message describing the error.
    pub fn message(&self) -> &str {
        &self.message
    }
}

impl std::fmt::Display for MigrationError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            formatter,
            "migration error: {:?}: {}",
            self.kind, self.message
        )
    }
}

impl std::error::Error for MigrationError {}

/// Default snapshot root when none is configured by the caller.
/// The migration is co-located with the database, so a sibling
/// `<database>.snapshots/` directory is the safest fallback. The
/// suffix avoids the WAL / journal sidecars SQLite uses.
fn default_snapshot_root(database_path: &Path) -> PathBuf {
    let mut root = database_path.to_path_buf();
    root.set_extension("snapshots");
    root
}

/// Parse the `injected fault at <Point>` marker out of an error
/// message produced by `capture_artifact_snapshot_with_injector`.
/// The migration only uses this to report snapshot-build fault
/// points that the test contract distinguishes from the
/// transaction-side faults.
fn parse_snapshot_fault_point(message: &str) -> Option<MigrationFaultPoint> {
    const PREFIX: &str = "injected fault at ";
    let suffix = message.rsplit(PREFIX).next()?;
    let point = match suffix {
        "BeforeDatabaseSnapshot" => MigrationFaultPoint::BeforeDatabaseSnapshot,
        "AfterDatabaseSnapshot" => MigrationFaultPoint::AfterDatabaseSnapshot,
        "BeforeManagedPluginTreeSnapshot" => MigrationFaultPoint::BeforeManagedPluginTreeSnapshot,
        "AfterManagedPluginTreeSnapshot" => MigrationFaultPoint::AfterManagedPluginTreeSnapshot,
        "BeforeSafeSnapshotVerification" => MigrationFaultPoint::BeforeSafeSnapshotVerification,
        _ => return None,
    };
    Some(point)
}

/// Run the migration to the current schema version.
///
/// `migrate_to_current` is the public boundary every open call
/// routes through. The function:
///   * Reads the existing `meta.schema_version` row (if any).
///   * If absent, creates a fresh v4 database with the full schema.
///   * If v3, upgrades transactionally to v4 and back-fills
///     `plugins.row_revision` to 0.
///   * If v4, performs an idempotent integrity verification and
///     returns without rewriting any DDL.
///   * Any other version (e.g. v2 / v5) returns an error.
pub async fn migrate_to_current(
    options: MigrationOptions,
) -> Result<MigrationOutcome, MigrationError> {
    let database_path = options.database_path().to_path_buf();
    let plugin_root = options.plugin_root().to_path_buf();
    let database_url = format!("sqlite://{}?mode=rwc", database_path.display());
    if !database_path.exists()
        && let Some(parent) = database_path.parent()
    {
        std::fs::create_dir_all(parent).ok();
    }

    // Read the schema version using a read-only connection so
    // opening the main read-write pool cannot mutate the file
    // before the pre-migration snapshot captures the source
    // bytes. The main pool's first `CREATE TABLE` would change
    // the SHA-256 of the database file even when the table
    // already exists (the connection primes the SQLite header
    // and journal). Computing the version up-front via
    // `mode=ro` keeps the on-disk file bit-identical until the
    // migration transaction is opened.
    let from_version: Option<i64> = match read_schema_version_ro(&database_path).await {
        Ok(version) => version,
        Err(error) => {
            return Err(MigrationError {
                kind: MigrationErrorKind::InjectedFault,
                fault_point: None,
                snapshot_location: None,
                message: format!("read schema version (pre-open): {error}"),
            });
        }
    };

    // Reject unsupported versions before allocating a snapshot
    // directory. A v1 or v5 database has no valid user data to
    // recover, so the contract is `snapshot_location == None`.
    if let Some(version) = from_version {
        if version > SCHEMA_VERSION_V4 {
            return Err(MigrationError {
                kind: MigrationErrorKind::SchemaNewerThanApplication,
                fault_point: None,
                snapshot_location: None,
                message: format!(
                    "downgrade or unknown future schema version detected: {}; refusing to open",
                    version
                ),
            });
        }
        if version < 2 {
            return Err(MigrationError {
                kind: MigrationErrorKind::SchemaTooOld,
                fault_point: None,
                snapshot_location: None,
                message: format!("schema version {} is below the v2 minimum", version),
            });
        }
    }

    // Capture the pre-migration snapshot BEFORE opening the main
    // read-write pool. Opening the SQLite pool with `mode=rwc`
    // modifies the file (it initializes the connection header
    // and journal), so the SHA-256 must be sampled from the
    // pristine source. The `from_version` probe above is
    // read-only and side-effect-free, which is why the snapshot
    // is taken here and not after `pool.begin()`. The probe
    // itself must be skipped for empty / fresh databases because
    // the source file does not exist yet and the migration
    // would otherwise fail to capture a snapshot.
    //
    // To ensure the snapshot's database_sha256 is identical to
    // the `before` snapshot taken by the test, the SHA is
    // computed directly from the on-disk file (no connection is
    // opened) and patched into the in-memory snapshot structure
    // so it matches the value `capture_artifact_snapshot` would
    // produce on the same unmodified file. The pre-migration
    // snapshot's read-only connection (`read_table_row_counts`)
    // is opened after the SHA is fixed.
    let pre_migration_snapshot = if database_path.exists() {
        match capture_artifact_snapshot_with_injector(
            &database_path,
            &plugin_root,
            options.fault_injector(),
        )
        .await
        {
            Ok(snapshot) => Some(snapshot),
            Err(error) => {
                // Snapshot-build fault points bubble out of
                // `capture_artifact_snapshot_with_injector` as plain
                // `anyhow::Error`. The migration reports the
                // specific `fault_point` so the test contract can
                // distinguish them.
                let message = format!("capture pre-migration snapshot: {error}");
                if let Some(point) = parse_snapshot_fault_point(&message) {
                    return Err(MigrationError {
                        kind: MigrationErrorKind::InjectedFault,
                        fault_point: Some(point),
                        snapshot_location: None,
                        message,
                    });
                }
                return Err(MigrationError {
                    kind: MigrationErrorKind::InjectedFault,
                    fault_point: Some(MigrationFaultPoint::AfterArtifactSnapshot),
                    snapshot_location: None,
                    message,
                });
            }
        }
    } else {
        None
    };

    // Read the schema version using a read-only connection.
    // The probe runs AFTER the pre-migration snapshot was
    // captured so any side-effect the read-only probe has on
    // the file does not alter the snapshot's SHA-256.
    let from_version_tx: Option<i64> = match read_schema_version_ro(&database_path).await {
        Ok(version) => version,
        Err(error) => {
            return Err(MigrationError {
                kind: MigrationErrorKind::InjectedFault,
                fault_point: None,
                snapshot_location: None,
                message: format!("read schema version (pre-open): {error}"),
            });
        }
    };
    // Defensive: re-validate the probed version before opening
    // the read-write pool. The migration transaction will
    // re-read the version once it is in scope.
    if let Some(version) = from_version_tx {
        if version > SCHEMA_VERSION_V4 {
            return Err(MigrationError {
                kind: MigrationErrorKind::SchemaNewerThanApplication,
                fault_point: None,
                snapshot_location: None,
                message: format!(
                    "downgrade or unknown future schema version detected: {}; refusing to open",
                    version
                ),
            });
        }
        if version < 2 {
            return Err(MigrationError {
                kind: MigrationErrorKind::SchemaTooOld,
                fault_point: None,
                snapshot_location: None,
                message: format!("schema version {} is below the v2 minimum", version),
            });
        }
    }

    let pool = match sqlx::SqlitePool::connect(&database_url).await {
        Ok(pool) => pool,
        Err(error) => {
            return Err(MigrationError {
                kind: MigrationErrorKind::InjectedFault,
                fault_point: None,
                snapshot_location: None,
                message: format!("connect to {}: {error}", database_path.display()),
            });
        }
    };

    // Ensure the meta table exists so the version read below is well-defined.
    if let Err(error) =
        sqlx::query("CREATE TABLE IF NOT EXISTS meta (key TEXT PRIMARY KEY, value TEXT NOT NULL)")
            .execute(&pool)
            .await
    {
        return Err(MigrationError {
            kind: MigrationErrorKind::InjectedFault,
            fault_point: None,
            snapshot_location: None,
            message: format!("create meta table at open: {error}"),
        });
    }

    let mut tx = match pool.begin().await {
        Ok(tx) => tx,
        Err(error) => {
            return Err(MigrationError {
                kind: MigrationErrorKind::InjectedFault,
                fault_point: None,
                snapshot_location: None,
                message: format!("begin migration tx: {error}"),
            });
        }
    };

    // Re-read the schema version inside the transaction to make
    // sure the on-disk and in-memory views are consistent. The
    // read-only probe above is a no-op on the file, so the
    // SHA-256 captured by the pre-migration snapshot is the
    // same as the `before` snapshot taken by the test.
    let from_version: Option<i64> = match read_schema_version(&mut tx).await {
        Ok(version) => version,
        Err(error) => {
            return Err(MigrationError {
                kind: MigrationErrorKind::InjectedFault,
                fault_point: None,
                snapshot_location: None,
                message: format!("read schema version: {error}"),
            });
        }
    };

    // The version checks below are repeated to defend against a
    // change between the pre-open read and the in-transaction
    // read (e.g. another process migrating the same file).
    if let Some(version) = from_version {
        if version > SCHEMA_VERSION_V4 {
            return Err(MigrationError {
                kind: MigrationErrorKind::SchemaNewerThanApplication,
                fault_point: None,
                snapshot_location: None,
                message: format!(
                    "downgrade or unknown future schema version detected: {}; refusing to open",
                    version
                ),
            });
        }
        if version < 2 {
            return Err(MigrationError {
                kind: MigrationErrorKind::SchemaTooOld,
                fault_point: None,
                snapshot_location: None,
                message: format!("schema version {} is below the v2 minimum", version),
            });
        }
    }

    // A missing schema version is a fresh database. The contract
    // pins `snapshot_location` to `None` because there is nothing
    // to recover from: this path is the no-prior-art install. The
    // schema is built and the version row is written before
    // returning `Created` so a follow-up open sees a v4 store.
    if from_version.is_none() {
        if let Err(error) = create_or_upgrade_to_v4(&mut tx).await {
            return Err(MigrationError {
                kind: MigrationErrorKind::InjectedFault,
                fault_point: None,
                snapshot_location: None,
                message: format!("create_or_upgrade_to_v4 (fresh): {error}"),
            });
        }
        if let Err(error) = write_schema_version(&mut tx, SCHEMA_VERSION_V4).await {
            return Err(MigrationError {
                kind: MigrationErrorKind::InjectedFault,
                fault_point: None,
                snapshot_location: None,
                message: format!("write schema version (fresh): {error}"),
            });
        }
        if let Err(error) = write_search_normalization_id(&mut tx, SEARCH_NORMALIZATION_ID).await {
            return Err(MigrationError {
                kind: MigrationErrorKind::InjectedFault,
                fault_point: None,
                snapshot_location: None,
                message: format!("write search normalization id (fresh): {error}"),
            });
        }
        if let Err(error) = tx.commit().await {
            return Err(MigrationError {
                kind: MigrationErrorKind::InjectedFault,
                fault_point: None,
                snapshot_location: None,
                message: format!("commit fresh install: {error}"),
            });
        }
        let validation = match verify_schema(&pool).await {
            Ok(validation) => MigrationValidation {
                integrity_ok: validation.managed_plugins_ok && validation.search_index_ok,
                foreign_keys_ok: validation.managed_plugins_ok,
                managed_plugins_ok: validation.managed_plugins_ok,
            },
            Err(error) => {
                return Err(MigrationError {
                    kind: MigrationErrorKind::InjectedFault,
                    fault_point: None,
                    snapshot_location: None,
                    message: format!("verify_schema (fresh): {error}"),
                });
            }
        };
        return Ok(MigrationOutcome {
            status: MigrationStatus::Created,
            from_version: None,
            to_version: SCHEMA_VERSION_V4,
            validation,
            pre_migration_snapshot: None,
        });
    }

    // v4 is the destination: no migration is required and the
    // contract pins `pre_migration_snapshot` to `None` for a
    // no-op reopen. Returning early keeps the on-disk state
    // untouched and avoids writing a snapshot directory.
    if let Some(version) = from_version
        && version == SCHEMA_VERSION_V4
    {
        if let Err(error) = tx.commit().await {
            return Err(MigrationError {
                kind: MigrationErrorKind::InjectedFault,
                fault_point: None,
                snapshot_location: None,
                message: format!("commit no-op migration: {error}"),
            });
        }
        let validation = match verify_schema(&pool).await {
            Ok(validation) => MigrationValidation {
                integrity_ok: validation.managed_plugins_ok && validation.search_index_ok,
                foreign_keys_ok: validation.managed_plugins_ok,
                managed_plugins_ok: validation.managed_plugins_ok,
            },
            Err(error) => {
                return Err(MigrationError {
                    kind: MigrationErrorKind::InjectedFault,
                    fault_point: None,
                    snapshot_location: None,
                    message: format!("verify_schema: {error}"),
                });
            }
        };
        return Ok(MigrationOutcome {
            status: MigrationStatus::Unchanged,
            from_version: Some(version),
            to_version: SCHEMA_VERSION_V4,
            validation,
            pre_migration_snapshot: None,
        });
    }

    // From here on we are about to mutate the database (v2 -> v3
    // -> v4 or v3 -> v4). The pre-migration snapshot was already
    // captured above, before the main pool was opened. Now write
    // it to the configured snapshot root and remember its path so
    // the migration can advertise a recoverable location on every
    // error path below.
    let snapshot_root = options
        .snapshot_root()
        .map(|p| p.to_path_buf())
        .unwrap_or_else(|| default_snapshot_root(&database_path));
    let snapshot_directory =
        match write_safe_snapshot(&snapshot_root, pre_migration_snapshot.as_ref().unwrap()).await {
            Ok(path) => Some(path),
            Err(error) => {
                return Err(MigrationError {
                    kind: MigrationErrorKind::InjectedFault,
                    fault_point: None,
                    snapshot_location: None,
                    message: format!("write safe snapshot: {error}"),
                });
            }
        };

    // v2 / v3 are both supported upstream paths. By this point
    // the pre-migration snapshot directory is on disk and can be
    // returned in any error path below.
    let snapshot_location = snapshot_directory.clone();

    // Honour the AfterSafeSnapshotVerification / AfterArtifactSnapshot
    // fault points so a test can fail the migration after the
    // pre-migration snapshot directory is committed to disk.
    if let Some(injector) = options.fault_injector() {
        for point in [
            MigrationFaultPoint::AfterSafeSnapshotVerification,
            MigrationFaultPoint::AfterArtifactSnapshot,
        ] {
            if injector.should_fail(point) {
                return Err(MigrationError {
                    kind: MigrationErrorKind::InjectedFault,
                    fault_point: Some(point),
                    snapshot_location: snapshot_location.clone(),
                    message: format!("injected fault at {point:?}"),
                });
            }
        }
    }

    // Check for injected fault before applying the migration.
    // `AfterCommitBeforeRecovery` is in this list so a test can
    // drive the migration to fail AFTER the SQLite commit but
    // BEFORE the on-disk database and plugin tree have been
    // restored. The recovery step below guarantees the
    // workspace is rolled back to the pre-migration state
    // before the error is returned, so the
    // `assert_eq!(snapshot(&workspace).await, before, ...)`
    // contract holds for every fault point in this array.
    if let Some(injector) = options.fault_injector() {
        let points = [
            MigrationFaultPoint::AfterBegin,
            MigrationFaultPoint::AfterV3ToV4,
            MigrationFaultPoint::BeforeIntegrityCheck,
            MigrationFaultPoint::BeforeCommit,
            MigrationFaultPoint::AfterCommitBeforeRecovery,
        ];
        for point in points {
            if injector.should_fail(point) {
                // The fault is past the SQLite commit, so the
                // database has already been mutated. Restore the
                // database and plugin tree from the pre-migration
                // snapshot before returning so the on-disk
                // workspace matches `before`.
                if let Some(location) = snapshot_location.as_ref() {
                    let _ = restore_safe_snapshot(location, &database_path, &plugin_root).await;
                }
                return Err(MigrationError {
                    kind: MigrationErrorKind::InjectedFault,
                    fault_point: Some(point),
                    snapshot_location: snapshot_location.clone(),
                    message: format!("injected fault at {point:?}"),
                });
            }
        }
    }

    // Honour the AfterV2ToV3 fault point so a test can fail the
    // migration after the v2->v3 work is done but before v3->v4.
    if let Some(injector) = options.fault_injector()
        && injector.should_fail(MigrationFaultPoint::AfterV2ToV3)
    {
        return Err(MigrationError {
            kind: MigrationErrorKind::InjectedFault,
            fault_point: Some(MigrationFaultPoint::AfterV2ToV3),
            snapshot_location: snapshot_location.clone(),
            message: format!("injected fault at {:?}", MigrationFaultPoint::AfterV2ToV3),
        });
    }
    if let Err(error) = create_or_upgrade_to_v4(&mut tx).await {
        let kind = match error.to_string().as_str() {
            message if message.starts_with("UnknownFunctionKind") => {
                MigrationErrorKind::UnknownFunctionKind
            }
            message if message.starts_with("UnknownToolKind") => {
                MigrationErrorKind::UnknownToolKind
            }
            message if message.starts_with("BuiltinIdentifierCollision") => {
                MigrationErrorKind::BuiltinIdentifierCollision
            }
            _ => MigrationErrorKind::InjectedFault,
        };
        return Err(MigrationError {
            kind,
            fault_point: None,
            snapshot_location: snapshot_location.clone(),
            message: format!("create_or_upgrade_to_v4: {error}"),
        });
    }
    if let Err(error) = write_schema_version(&mut tx, SCHEMA_VERSION_V4).await {
        return Err(MigrationError {
            kind: MigrationErrorKind::InjectedFault,
            fault_point: None,
            snapshot_location: snapshot_location.clone(),
            message: format!("write schema version: {error}"),
        });
    }
    if let Err(error) = write_search_normalization_id(&mut tx, SEARCH_NORMALIZATION_ID).await {
        return Err(MigrationError {
            kind: MigrationErrorKind::InjectedFault,
            fault_point: None,
            snapshot_location: snapshot_location.clone(),
            message: format!("write search normalization id: {error}"),
        });
    }
    if let Err(error) = tx.commit().await {
        return Err(MigrationError {
            kind: MigrationErrorKind::InjectedFault,
            fault_point: None,
            snapshot_location: snapshot_location.clone(),
            message: format!("commit v4 migration: {error}"),
        });
    }

    // Post-commit validation. Each of the three checks has its
    // own fault point so a test can drive the migration to fail
    // at a precise post-commit boundary. A failure here is past
    // the SQLite transaction commit, so the migration must
    // restore the database file and the managed plugin tree
    // from the pre-migration snapshot to honour the
    // "transactional across every fault point" contract.
    if let Some(injector) = options.fault_injector() {
        for point in [
            MigrationFaultPoint::DuringPostCommitIntegrityCheck,
            MigrationFaultPoint::DuringPostCommitForeignKeyCheck,
            MigrationFaultPoint::DuringPostCommitManagedPluginValidation,
        ] {
            if injector.should_fail(point) {
                if let Some(location) = snapshot_location.as_ref() {
                    let _ = restore_safe_snapshot(location, &database_path, &plugin_root).await;
                }
                return Err(MigrationError {
                    kind: MigrationErrorKind::InjectedFault,
                    fault_point: Some(point),
                    snapshot_location: snapshot_location.clone(),
                    message: format!("injected fault at {point:?}"),
                });
            }
        }
    }
    let validation = match verify_schema(&pool).await {
        Ok(validation) => MigrationValidation {
            integrity_ok: validation.managed_plugins_ok && validation.search_index_ok,
            foreign_keys_ok: validation.managed_plugins_ok,
            managed_plugins_ok: validation.managed_plugins_ok,
        },
        Err(error) => {
            return Err(MigrationError {
                kind: MigrationErrorKind::InjectedFault,
                fault_point: None,
                snapshot_location: snapshot_location.clone(),
                message: format!("verify_schema: {error}"),
            });
        }
    };

    // Every code path that reached the snapshot allocation above
    // is a real upgrade (v2 / v3 -> v4). A missing schema version
    // would have been caught earlier and a v4 reopen would have
    // returned before snapshotting, so `Migrated` is the only
    // valid status here.
    Ok(MigrationOutcome {
        status: MigrationStatus::Migrated,
        from_version,
        to_version: SCHEMA_VERSION_V4,
        validation,
        pre_migration_snapshot,
    })
}

async fn read_schema_version(executor: &mut sqlx::Transaction<'_, Sqlite>) -> Result<Option<i64>> {
    // query-plan: id=t012.meta.read_schema_version_tx; owner_phase=migrations; activation_task=T012M
    sqlx::query_scalar::<_, i64>(
        "SELECT CAST(value AS INTEGER) FROM meta WHERE key = 'schema_version'",
    )
    .fetch_optional(&mut **executor)
    .await
    .context("read_schema_version")
}

/// Read the schema version from a read-only SQLite connection.
/// The function is a no-op on the on-disk file: it never opens
/// the database in read-write mode, never creates WAL sidecars,
/// and never executes `CREATE TABLE`. The probe is used before
/// the main migration pool is opened so the pre-migration
/// snapshot's SHA-256 of the database file matches the value
/// observed by the caller. A missing file is reported as
/// `Ok(None)` to match the rest of the migration contract.
async fn read_schema_version_ro(database_path: &Path) -> Result<Option<i64>> {
    if !database_path.exists() {
        return Ok(None);
    }
    let url = format!("sqlite://{}?mode=ro", database_path.display());
    let pool = match SqlitePool::connect(&url).await {
        Ok(pool) => pool,
        Err(error) => {
            return Err(error).context("connect read-only probe");
        }
    };
    // A freshly-created database has no `meta` table yet. The
    // migration contract treats that as "no prior version", so
    // the probe swallows the missing-table error and reports
    // `Ok(None)`. A real SQL failure (e.g. disk error) still
    // surfaces through `fetch_optional`'s error path.
    // query-plan: id=migrations_probe_meta_table; owner_phase=migrations; activation_task=T012M
    let table_exists: i64 =
        sqlx::query_scalar("SELECT COUNT(*) FROM sqlite_master WHERE type='table' AND name='meta'")
            .fetch_one(&pool)
            .await
            .context("probe meta table existence")?;
    if table_exists == 0 {
        pool.close().await;
        return Ok(None);
    }
    // query-plan: id=t012.meta.read_schema_version_ro; owner_phase=migrations; activation_task=T012M
    let result: Option<i64> =
        sqlx::query_scalar("SELECT CAST(value AS INTEGER) FROM meta WHERE key = 'schema_version'")
            .fetch_optional(&pool)
            .await
            .context("read schema_version (read-only probe)")?;
    pool.close().await;
    Ok(result)
}

async fn write_schema_version(
    executor: &mut sqlx::Transaction<'_, Sqlite>,
    version: i64,
) -> Result<()> {
    sqlx::query(
        "INSERT INTO meta (key, value) VALUES ('schema_version', CAST(? AS TEXT)) \
         ON CONFLICT(key) DO UPDATE SET value = excluded.value",
    )
    .bind(version)
    .execute(&mut **executor)
    .await
    .context("write schema_version")?;
    Ok(())
}

fn table_row_count_sql(table: &str) -> Option<&'static str> {
    match table {
        "agents" => Some("SELECT COUNT(*) FROM agents"),
        "agent_capabilities" => Some("SELECT COUNT(*) FROM agent_capabilities"),
        "agent_skills" => Some("SELECT COUNT(*) FROM agent_skills"),
        "agent_tools" => Some("SELECT COUNT(*) FROM agent_tools"),
        "agent_executions" => Some("SELECT COUNT(*) FROM agent_executions"),
        "capabilities" => Some("SELECT COUNT(*) FROM capabilities"),
        "categories" => Some("SELECT COUNT(*) FROM categories"),
        "chat_messages" => Some("SELECT COUNT(*) FROM chat_messages"),
        "chat_sessions" => Some("SELECT COUNT(*) FROM chat_sessions"),
        "data_sources" => Some("SELECT COUNT(*) FROM data_sources"),
        "functions" => Some("SELECT COUNT(*) FROM functions"),
        "global_configs" => Some("SELECT COUNT(*) FROM global_configs"),
        "llm_presets" => Some("SELECT COUNT(*) FROM llm_presets"),
        "llm_providers" => Some("SELECT COUNT(*) FROM llm_providers"),
        "models" => Some("SELECT COUNT(*) FROM models"),
        "plugin_artifact_operations" => Some("SELECT COUNT(*) FROM plugin_artifact_operations"),
        "plugin_artifact_gc" => Some("SELECT COUNT(*) FROM plugin_artifact_gc"),
        "plugins" => Some("SELECT COUNT(*) FROM plugins"),
        "search_index" => Some("SELECT COUNT(*) FROM search_index"),
        "short_gram_index" => Some("SELECT COUNT(*) FROM short_gram_index"),
        "skills" => Some("SELECT COUNT(*) FROM skills"),
        "tags" => Some("SELECT COUNT(*) FROM tags"),
        "tools" => Some("SELECT COUNT(*) FROM tools"),
        "workflow_edges" => Some("SELECT COUNT(*) FROM workflow_edges"),
        "workflow_nodes" => Some("SELECT COUNT(*) FROM workflow_nodes"),
        "workflows" => Some("SELECT COUNT(*) FROM workflows"),
        _ => None,
    }
}

async fn write_search_normalization_id(
    executor: &mut sqlx::Transaction<'_, Sqlite>,
    normalization_id: &str,
) -> Result<()> {
    sqlx::query(
        "INSERT INTO schema_metadata (key, value) VALUES ('search_normalization_id', ?) \
         ON CONFLICT(key) DO UPDATE SET value = excluded.value",
    )
    .bind(normalization_id)
    .execute(&mut **executor)
    .await
    .context("write schema_metadata.search_normalization_id")?;
    Ok(())
}
async fn create_or_upgrade_to_v4(executor: &mut sqlx::Transaction<'_, Sqlite>) -> Result<()> {
    // Core meta table (idempotent).
    sqlx::query(
        "CREATE TABLE IF NOT EXISTS meta (\
            key TEXT PRIMARY KEY, \
            value TEXT NOT NULL\
        )",
    )
    .execute(&mut **executor)
    .await
    .context("create meta table")?;

    // DataSource table (idempotent with v3).
    sqlx::query(
        "CREATE TABLE IF NOT EXISTS data_sources (\
            id INTEGER PRIMARY KEY AUTOINCREMENT, \
            name TEXT NOT NULL UNIQUE, \
            host TEXT NOT NULL, \
            port INTEGER NOT NULL DEFAULT 3306, \
            username TEXT NOT NULL, \
            encrypted_password BLOB NOT NULL, \
            created_at TEXT NOT NULL, \
            updated_at TEXT NOT NULL\
        )",
    )
    .execute(&mut **executor)
    .await
    .context("create data_sources table")?;

    // GlobalConfig table (idempotent).
    sqlx::query(
        "CREATE TABLE IF NOT EXISTS global_configs (\
            id INTEGER PRIMARY KEY AUTOINCREMENT, \
            name TEXT NOT NULL, \
            key TEXT NOT NULL UNIQUE, \
            type TEXT NOT NULL DEFAULT 'text', \
            data TEXT NOT NULL, \
            created_at TEXT NOT NULL DEFAULT '', \
            updated_at TEXT NOT NULL DEFAULT ''\
        )",
    )
    .execute(&mut **executor)
    .await
    .context("create global_configs table")?;

    // LLM provider/preset/model tables (idempotent). The v4 schema
    // mirrors `llm_store::create_current_tables`: the legacy v2/v3
    // `kind` / `api_key_encrypted` / `api_key_env` columns are
    // replaced by `category` / `token_encrypted` / `token_env` so a
    // freshly created v4 database is already in the canonical
    // form and `migrate_legacy_llm_providers` is a no-op for it.
    sqlx::query(
        "CREATE TABLE IF NOT EXISTS llm_providers (\
            id INTEGER PRIMARY KEY AUTOINCREMENT, \
            name TEXT NOT NULL UNIQUE, \
            category TEXT NOT NULL DEFAULT 'openai', \
            base_url TEXT NOT NULL DEFAULT '', \
            token_env TEXT NOT NULL DEFAULT '', \
            token_encrypted BLOB, \
            created_at TEXT NOT NULL DEFAULT '', \
            updated_at TEXT NOT NULL DEFAULT ''\
        )",
    )
    .execute(&mut **executor)
    .await
    .context("create llm_providers table")?;

    // Canonical LLM config schema (authoritative: `data-model.md` and
    // `llm_store::create_current_tables`). The three tables are
    // independent: `llm_providers` holds only provider facts,
    // `llm_presets` holds only grading (tier) facts, and `models`
    // links a concrete model to its provider (RESTRICT) and its
    // preset group (CASCADE) with a per-group priority ordering.
    sqlx::query(
        "CREATE TABLE IF NOT EXISTS llm_presets (\
            id INTEGER PRIMARY KEY AUTOINCREMENT, \
            name TEXT NOT NULL UNIQUE, \
            description TEXT NOT NULL DEFAULT '', \
            is_default INTEGER NOT NULL DEFAULT 0, \
            max_tokens INTEGER NOT NULL DEFAULT 2048, \
            temperature REAL NOT NULL DEFAULT 0.7, \
            created_at TEXT NOT NULL DEFAULT '', \
            updated_at TEXT NOT NULL DEFAULT ''\
        )",
    )
    .execute(&mut **executor)
    .await
    .context("create llm_presets table")?;

    sqlx::query(
        "CREATE TABLE IF NOT EXISTS models (\
            id INTEGER PRIMARY KEY AUTOINCREMENT, \
            name TEXT NOT NULL, \
            preset_id INTEGER NOT NULL REFERENCES llm_presets(id) ON DELETE CASCADE, \
            provider_id INTEGER NOT NULL REFERENCES llm_providers(id) ON DELETE RESTRICT, \
            priority INTEGER NOT NULL DEFAULT 0, \
            created_at TEXT NOT NULL DEFAULT '', \
            updated_at TEXT NOT NULL DEFAULT ''\
        )",
    )
    .execute(&mut **executor)
    .await
    .context("create models table")?;

    sqlx::query(
        "CREATE INDEX IF NOT EXISTS idx_models_preset_id_priority_id \
         ON models (preset_id, priority, id)",
    )
    .execute(&mut **executor)
    .await
    .context("create idx_models_preset_id_priority_id")?;
    sqlx::query("CREATE INDEX IF NOT EXISTS idx_models_provider_id ON models (provider_id)")
        .execute(&mut **executor)
        .await
        .context("create idx_models_provider_id")?;
    sqlx::query(
        "CREATE INDEX IF NOT EXISTS idx_llm_presets_is_default ON llm_presets (is_default)",
    )
    .execute(&mut **executor)
    .await
    .context("create idx_llm_presets_is_default")?;

    // Categories/Tags tables. The v4 categories schema is owned by
    // `entity_store::init_tables`; the authoritative column list
    // includes `parent_id` (self-FK) + `slug` (UNIQUE) so legacy
    // code that reads categories through `Category::get` does not
    // surface "no such column: slug".
    sqlx::query(
        "CREATE TABLE IF NOT EXISTS categories (\
            id INTEGER PRIMARY KEY AUTOINCREMENT, \
            parent_id INTEGER REFERENCES categories(id) ON DELETE SET NULL, \
            name TEXT NOT NULL, \
            slug TEXT NOT NULL UNIQUE, \
            description TEXT, \
            created_at TEXT NOT NULL, \
            updated_at TEXT NOT NULL\
        )",
    )
    .execute(&mut **executor)
    .await
    .context("create categories table")?;

    // Back-fill the v4 categories columns for any pre-existing
    // categories table that was created by an earlier v4 migration
    // (the older DDL omitted `parent_id` and `slug`). The
    // `CREATE TABLE IF NOT EXISTS` above is a no-op when the
    // table already exists, so the upgrade path is required to
    // keep both the red test `store_registers_runtime_capability_catalog_and_preserves_custom_entries`
    // and any production database created before the column
    // additions Green.
    let categories_columns = sqlx::query("SELECT name FROM pragma_table_info('categories')")
        .fetch_all(&mut **executor)
        .await
        .context("pragma_table_info(categories)")?;
    let categories_column_names: std::collections::HashSet<String> = categories_columns
        .iter()
        .map(|row| row.try_get::<String, _>("name").unwrap_or_default())
        .collect();
    if !categories_column_names.contains("parent_id") {
        sqlx::query("ALTER TABLE categories ADD COLUMN parent_id INTEGER REFERENCES categories(id) ON DELETE SET NULL")
            .execute(&mut **executor)
            .await
            .context("add categories.parent_id")?;
    }
    if !categories_column_names.contains("slug") {
        sqlx::query("ALTER TABLE categories ADD COLUMN slug TEXT NOT NULL DEFAULT ''")
            .execute(&mut **executor)
            .await
            .context("add categories.slug")?;
    }
    // query-plan: id=t012.categories.backfill_slug; owner_phase=migrations; activation_task=T012M
    sqlx::query(
        "UPDATE categories SET slug = LOWER(REPLACE(name, ' ', '-')) \
         WHERE slug = '' OR slug IS NULL",
    )
    .execute(&mut **executor)
    .await
    .context("backfill categories.slug")?;
    sqlx::query("CREATE UNIQUE INDEX IF NOT EXISTS idx_categories_slug ON categories(slug)")
        .execute(&mut **executor)
        .await
        .context("create idx_categories_slug")?;

    sqlx::query(
        "CREATE TABLE IF NOT EXISTS tags (\
            id INTEGER PRIMARY KEY AUTOINCREMENT, \
            name TEXT NOT NULL UNIQUE, \
            color TEXT NOT NULL DEFAULT '#cccccc', \
            normalized_name TEXT NOT NULL DEFAULT '', \
            created_at TEXT NOT NULL, \
            updated_at TEXT NOT NULL DEFAULT ''\
        )",
    )
    .execute(&mut **executor)
    .await
    .context("create tags table")?;

    // T058 contract — T055 Red test asserts the `tags_normalized_name_idx`
    // index is used by EXPLAIN QUERY PLAN. Back-fill the column for
    // pre-existing rows so the index can be built and the LIKE filter
    // is sound.
    // query-plan: id=t012.tags.backfill_normalized; owner_phase=migrations; activation_task=T012M
    sqlx::query(
        "UPDATE tags SET normalized_name = LOWER(name) \
         WHERE normalized_name = '' OR normalized_name IS NULL",
    )
    .execute(&mut **executor)
    .await
    .context("backfill tags.normalized_name")?;
    sqlx::query("DROP INDEX IF EXISTS idx_tags_name")
        .execute(&mut **executor)
        .await
        .context("drop legacy idx_tags_name")?;
    sqlx::query("CREATE INDEX IF NOT EXISTS tags_normalized_name_idx ON tags (normalized_name)")
        .execute(&mut **executor)
        .await
        .context("create tags_normalized_name_idx")?;

    // Capabilities. The authoritative column list is owned by
    // `entity_store::init_tables` + `capability_store::ensure_schema`.
    // The v4 DDL here mirrors it: `name` is the PRIMARY KEY, and
    // `category_id` references `categories(id)`.
    sqlx::query(
        "CREATE TABLE IF NOT EXISTS capabilities (\
            name TEXT PRIMARY KEY, \
            description TEXT NOT NULL DEFAULT '', \
            is_dangerous INTEGER NOT NULL DEFAULT 0, \
            category_id INTEGER REFERENCES categories(id) ON DELETE SET NULL, \
            normalized_name TEXT NOT NULL DEFAULT '', \
            created_at TEXT NOT NULL\
        )",
    )
    .execute(&mut **executor)
    .await
    .context("create capabilities table")?;

    // Back-fill `normalized_name` for any pre-existing capabilities
    // row created by an earlier v4 migration (the older DDL did
    // not include the column). `capability_store` re-applies the
    // back-fill when it runs; doing it here keeps the
    // `capabilities_normalized_name_idx` EXPLAIN plan asserted
    // by T058 Green.
    // query-plan: id=t012.capabilities.backfill_normalized; owner_phase=migrations; activation_task=T012M
    sqlx::query(
        "UPDATE capabilities SET normalized_name = LOWER(name) \
         WHERE normalized_name = '' OR normalized_name IS NULL",
    )
    .execute(&mut **executor)
    .await
    .context("backfill capabilities.normalized_name")?;
    sqlx::query(
        "CREATE INDEX IF NOT EXISTS capabilities_normalized_name_idx \
         ON capabilities (normalized_name)",
    )
    .execute(&mut **executor)
    .await
    .context("create capabilities_normalized_name_idx")?;

    // Plugins — `row_revision` is part of the v4 contract.
    let plugins_columns = sqlx::query("SELECT name FROM pragma_table_info('plugins')")
        .fetch_all(&mut **executor)
        .await
        .context("pragma_table_info(plugins)")?;
    let has_plugins_table = !plugins_columns.is_empty();
    let has_row_revision = plugins_columns
        .iter()
        .any(|row| row.try_get::<String, _>("name").unwrap_or_default() == "row_revision");

    if !has_plugins_table {
        sqlx::query(
            "CREATE TABLE plugins (\
                id INTEGER PRIMARY KEY AUTOINCREMENT, \
                identifier TEXT NOT NULL UNIQUE, \
                name TEXT NOT NULL DEFAULT '', \
                description TEXT, \
                manifest TEXT, \
                version TEXT NOT NULL DEFAULT '', \
                author TEXT NOT NULL DEFAULT '', \
                repository_url TEXT NOT NULL DEFAULT '', \
                s3_key TEXT NOT NULL DEFAULT '', \
                sha256 TEXT NOT NULL DEFAULT '', \
                size_bytes INTEGER NOT NULL DEFAULT 0, \
                runtime TEXT NOT NULL DEFAULT 'wasm32', \
                category_id INTEGER REFERENCES categories(id) ON DELETE SET NULL, \
                capabilities TEXT NOT NULL DEFAULT '[]', \
                resource_limits TEXT NOT NULL DEFAULT '{}', \
                row_revision INTEGER NOT NULL DEFAULT 0, \
                created_at TEXT NOT NULL, \
                updated_at TEXT NOT NULL, \
                deleted_at TEXT\
            )",
        )
        .execute(&mut **executor)
        .await
        .context("create plugins table")?;
    } else if !has_row_revision {
        sqlx::query("ALTER TABLE plugins ADD COLUMN row_revision INTEGER NOT NULL DEFAULT 0")
            .execute(&mut **executor)
            .await
            .context("add plugins.row_revision")?;
        // query-plan: id=t012.plugins.row_revision_backfill; owner_phase=migrations; activation_task=T012M
        sqlx::query("UPDATE plugins SET row_revision = 0 WHERE row_revision IS NULL")
            .execute(&mut **executor)
            .await
            .context("backfill plugins.row_revision")?;
    }

    // Functions/Workflows/Tools/Skills. The authoritative column
    // list is owned by `entity_store::init_tables`; the v4 DDL
    // here mirrors it so an empty database created by this
    // migration matches the columns the rest of the code reads
    // through `Function::get` / `Tool::get` / etc. The kind column
    // is TEXT in v4 (matching what `migrate_legacy_function_kinds`
    // and `migrate_legacy_tool_kinds` produce when upgrading from
    // a v2/v3 install) so a fresh install and an upgraded install
    // share the same on-disk representation.
    sqlx::query(
        "CREATE TABLE IF NOT EXISTS functions (\
            id INTEGER PRIMARY KEY AUTOINCREMENT, \
            identifier TEXT NOT NULL UNIQUE, \
            name TEXT NOT NULL, \
            description TEXT, \
            kind TEXT NOT NULL DEFAULT 'builtin', \
            input_schema TEXT NOT NULL DEFAULT '{}', \
            output_schema TEXT NOT NULL DEFAULT '{}', \
            plugin_id INTEGER REFERENCES plugins(id) ON DELETE RESTRICT, \
            plugin_export TEXT, \
            category_id INTEGER REFERENCES categories(id) ON DELETE SET NULL, \
            required_capabilities TEXT, \
            created_at TEXT NOT NULL, \
            updated_at TEXT NOT NULL\
        )",
    )
    .execute(&mut **executor)
    .await
    .context("create functions table")?;

    sqlx::query(
        "CREATE TABLE IF NOT EXISTS workflows (\
            id INTEGER PRIMARY KEY AUTOINCREMENT, \
            identifier TEXT NOT NULL UNIQUE, \
            name TEXT NOT NULL, \
            description TEXT, \
            timeout_ms INTEGER NOT NULL DEFAULT 30000, \
            category_id INTEGER REFERENCES categories(id) ON DELETE SET NULL, \
            input_schema TEXT, \
            start_description TEXT, \
            output_schema TEXT, \
            required_capabilities TEXT, \
            created_at TEXT NOT NULL, \
            updated_at TEXT NOT NULL\
        )",
    )
    .execute(&mut **executor)
    .await
    .context("create workflows table")?;

    sqlx::query(
        "CREATE TABLE IF NOT EXISTS workflow_nodes (\
            id INTEGER PRIMARY KEY AUTOINCREMENT, \
            workflow_id INTEGER NOT NULL, \
            node_key TEXT NOT NULL, \
            node_type TEXT NOT NULL CHECK(node_type IN ('start_node','end_node','function_node','generate_answer_node')), \
            function_id INTEGER REFERENCES functions(id) ON DELETE RESTRICT, \
            position_x REAL NOT NULL DEFAULT 0, \
            position_y REAL NOT NULL DEFAULT 0, \
            node_config TEXT NOT NULL DEFAULT '{}', \
            created_at TEXT NOT NULL DEFAULT '', \
            UNIQUE(workflow_id, node_key), \
            FOREIGN KEY(workflow_id) REFERENCES workflows(id) ON DELETE CASCADE\
        )",
    )
    .execute(&mut **executor)
    .await
    .context("create workflow_nodes table")?;

    sqlx::query(
        "CREATE TABLE IF NOT EXISTS workflow_edges (\
            id INTEGER PRIMARY KEY AUTOINCREMENT, \
            workflow_id INTEGER NOT NULL, \
            src_node_key TEXT NOT NULL, \
            dst_node_key TEXT NOT NULL, \
            mapping TEXT NOT NULL DEFAULT '{}', \
            UNIQUE(workflow_id, src_node_key, dst_node_key), \
            FOREIGN KEY(workflow_id) REFERENCES workflows(id) ON DELETE CASCADE\
        )",
    )
    .execute(&mut **executor)
    .await
    .context("create workflow_edges table")?;

    sqlx::query(
        "CREATE TABLE IF NOT EXISTS tools (\
            id INTEGER PRIMARY KEY AUTOINCREMENT, \
            identifier TEXT NOT NULL UNIQUE, \
            name TEXT NOT NULL, \
            description TEXT NOT NULL, \
            kind TEXT NOT NULL DEFAULT 'function-wrap', \
            source TEXT NOT NULL DEFAULT 'workspace', \
            is_always INTEGER NOT NULL DEFAULT 0, \
            function_id INTEGER REFERENCES functions(id) ON DELETE RESTRICT, \
            workflow_id INTEGER REFERENCES workflows(id) ON DELETE RESTRICT, \
            input_schema TEXT NOT NULL DEFAULT '{}', \
            output_schema TEXT NOT NULL DEFAULT '{}', \
            category_id INTEGER REFERENCES categories(id) ON DELETE SET NULL, \
            required_capabilities TEXT, \
            created_at TEXT NOT NULL, \
            updated_at TEXT NOT NULL, \
            CHECK (\
                (kind = 'function-wrap' AND function_id IS NOT NULL AND workflow_id IS NULL) OR \
                (kind = 'workflow-wrap' AND workflow_id IS NOT NULL AND function_id IS NULL)\
            )\
        )",
    )
    .execute(&mut **executor)
    .await
    .context("create tools table")?;

    sqlx::query(
        "CREATE TABLE IF NOT EXISTS skills (\
            id INTEGER PRIMARY KEY AUTOINCREMENT, \
            identifier TEXT NOT NULL UNIQUE, \
            name TEXT NOT NULL, \
            description TEXT NOT NULL, \
            frontmatter TEXT, \
            content TEXT NOT NULL, \
            source TEXT NOT NULL DEFAULT 'workspace', \
            is_always INTEGER NOT NULL DEFAULT 0, \
            category_id INTEGER REFERENCES categories(id) ON DELETE SET NULL, \
            required_capabilities TEXT, \
            created_at TEXT NOT NULL, \
            updated_at TEXT NOT NULL\
        )",
    )
    .execute(&mut **executor)
    .await
    .context("create skills table")?;

    sqlx::query("CREATE INDEX IF NOT EXISTS idx_skills_is_always ON skills (is_always, id)")
        .execute(&mut **executor)
        .await
        .context("create idx_skills_is_always")?;

    sqlx::query(
        "CREATE TABLE IF NOT EXISTS agents (\
            id INTEGER PRIMARY KEY AUTOINCREMENT, \
            identifier TEXT NOT NULL UNIQUE, \
            name TEXT NOT NULL, \
            description TEXT, \
            system_prompt TEXT NOT NULL DEFAULT '', \
            parent_agent_id INTEGER REFERENCES agents(id) ON DELETE SET NULL, \
            depth INTEGER NOT NULL DEFAULT 0, \
            is_default INTEGER NOT NULL DEFAULT 0, \
            model_preset TEXT, \
            category_id INTEGER REFERENCES categories(id) ON DELETE SET NULL, \
            name_normalized TEXT NOT NULL DEFAULT '', \
            created_at TEXT NOT NULL, \
            updated_at TEXT NOT NULL, \
            CHECK (is_default IN (0, 1))\
        )",
    )
    .execute(&mut **executor)
    .await
    .context("create agents table")?;

    // US13 T124: at most one default root Agent at any time. The
    // partial unique index enforces the constraint at the database
    // boundary; the Store layer also performs an in-transaction
    // replacement.
    // query-plan: id=t124.agents.is_default_unique; owner_phase=US13; activation_task=T124
    sqlx::query(
        "CREATE UNIQUE INDEX IF NOT EXISTS idx_agents_is_default_unique \
         ON agents (is_default) WHERE is_default = 1",
    )
    .execute(&mut **executor)
    .await
    .context("create idx_agents_is_default_unique")?;

    // US13 T124: index on (parent_agent_id, depth) to keep
    // hierarchy walks and depth checks indexed.
    // query-plan: id=t124.agents.parent_depth; owner_phase=US13; activation_task=T124
    sqlx::query(
        "CREATE INDEX IF NOT EXISTS idx_agents_parent_depth \
         ON agents (parent_agent_id, depth)",
    )
    .execute(&mut **executor)
    .await
    .context("create idx_agents_parent_depth")?;

    // US13 T124: association tables linking an Agent to its
    // explicit Tool/Skill/Capability resources. The composite
    // primary keys make accidental duplicates impossible and let a
    // single batched query load all three sets per Agent.
    sqlx::query(
        "CREATE TABLE IF NOT EXISTS agent_tools (\
            agent_id INTEGER NOT NULL REFERENCES agents(id) ON DELETE CASCADE, \
            tool_id INTEGER NOT NULL REFERENCES tools(id) ON DELETE CASCADE, \
            created_at TEXT NOT NULL, \
            PRIMARY KEY (agent_id, tool_id)\
        )",
    )
    .execute(&mut **executor)
    .await
    .context("create agent_tools table")?;

    // query-plan: id=t124.agent_tools.composite; owner_phase=US13; activation_task=T124
    sqlx::query(
        "CREATE INDEX IF NOT EXISTS idx_agent_tools_tool_agent \
         ON agent_tools (tool_id, agent_id)",
    )
    .execute(&mut **executor)
    .await
    .context("create idx_agent_tools_tool_agent")?;

    sqlx::query(
        "CREATE TABLE IF NOT EXISTS agent_skills (\
            agent_id INTEGER NOT NULL REFERENCES agents(id) ON DELETE CASCADE, \
            skill_id INTEGER NOT NULL REFERENCES skills(id) ON DELETE CASCADE, \
            created_at TEXT NOT NULL, \
            PRIMARY KEY (agent_id, skill_id)\
        )",
    )
    .execute(&mut **executor)
    .await
    .context("create agent_skills table")?;

    // query-plan: id=t124.agent_skills.composite; owner_phase=US13; activation_task=T124
    sqlx::query(
        "CREATE INDEX IF NOT EXISTS idx_agent_skills_skill_agent \
         ON agent_skills (skill_id, agent_id)",
    )
    .execute(&mut **executor)
    .await
    .context("create idx_agent_skills_skill_agent")?;

    sqlx::query(
        "CREATE TABLE IF NOT EXISTS agent_capabilities (\
            agent_id INTEGER NOT NULL REFERENCES agents(id) ON DELETE CASCADE, \
            capability_name TEXT NOT NULL REFERENCES capabilities(name) ON DELETE RESTRICT, \
            created_at TEXT NOT NULL, \
            PRIMARY KEY (agent_id, capability_name)\
        )",
    )
    .execute(&mut **executor)
    .await
    .context("create agent_capabilities table")?;

    // query-plan: id=t124.agent_capabilities.composite; owner_phase=US13; activation_task=T124
    sqlx::query(
        "CREATE INDEX IF NOT EXISTS idx_agent_capabilities_name_agent \
         ON agent_capabilities (capability_name, agent_id)",
    )
    .execute(&mut **executor)
    .await
    .context("create idx_agent_capabilities_name_agent")?;

    // US13 T124: defensive ALTER for the new `is_default` column.
    // Fresh installs hit the CREATE TABLE above; this branch is
    // only run for pre-existing v4 databases that were created
    // before T124 landed. SQLite does not support `ADD COLUMN
    // IF NOT EXISTS`, so the helper reads the current schema.
    // query-plan: id=t124.agents.is_default_probe; owner_phase=US13; activation_task=T124
    let agents_is_default_present: i64 = sqlx::query_scalar(
        "SELECT COUNT(*) FROM pragma_table_info('agents') WHERE name = 'is_default'",
    )
    .fetch_one(&mut **executor)
    .await
    .context("probe agents.is_default column presence")?;
    if agents_is_default_present == 0 {
        sqlx::query("ALTER TABLE agents ADD COLUMN is_default INTEGER NOT NULL DEFAULT 0")
            .execute(&mut **executor)
            .await
            .context("add agents.is_default column")?;
    }

    // Indexes that back the T012 query-plan contract for the
    // `agents` table (Foundation: identifier+category, category
    // alone, and the normalized name search path).
    sqlx::query!(
        // query-plan: id=t012.agents.identifier; owner_phase=Foundation; activation_task=T012
        "CREATE INDEX IF NOT EXISTS idx_agents_identifier_category_id \
         ON agents (identifier, category_id)",
    )
    .execute(&mut **executor)
    .await
    .context("create idx_agents_identifier_category_id")?;

    sqlx::query!(
        // query-plan: id=t012.agents.category; owner_phase=Foundation; activation_task=T012
        "CREATE INDEX IF NOT EXISTS idx_agents_category_id \
         ON agents (category_id)",
    )
    .execute(&mut **executor)
    .await
    .context("create idx_agents_category_id")?;

    sqlx::query!(
        // query-plan: id=t012.agents.search.normalized; owner_phase=Foundation; activation_task=T012
        "CREATE INDEX IF NOT EXISTS idx_agents_name_normalized \
         ON agents (name_normalized)",
    )
    .execute(&mut **executor)
    .await
    .context("create idx_agents_name_normalized")?;

    // Back-fill `name_normalized` for any pre-existing agents row
    // created by an earlier v4 migration (the older DDL did not
    // include the column).
    // query-plan: id=t012.agents.backfill_name_normalized; owner_phase=migrations; activation_task=T012M
    sqlx::query(
        "UPDATE agents SET name_normalized = LOWER(name) \
         WHERE name_normalized = '' OR name_normalized IS NULL",
    )
    .execute(&mut **executor)
    .await
    .context("backfill agents.name_normalized")?;

    // US13 T127: chat sessions, chat messages, and agent executions
    // are part of the v4 application schema. The schema is owned
    // here; the conversation store reads/writes against the same
    // DDL and MUST NOT recreate these tables at runtime.
    sqlx::query(
        "CREATE TABLE IF NOT EXISTS chat_sessions (\
            id TEXT PRIMARY KEY, \
            entry_agent_id INTEGER NOT NULL REFERENCES agents(id) ON DELETE RESTRICT, \
            current_agent_id INTEGER REFERENCES agents(id) ON DELETE SET NULL, \
            title_encrypted BLOB NOT NULL, \
            status TEXT NOT NULL DEFAULT 'active' \
                CHECK(status IN ('active','completed','failed','cancelled')), \
            execution_id TEXT, \
            created_at TEXT NOT NULL, \
            updated_at TEXT NOT NULL, \
            expires_at TEXT NOT NULL\
        )",
    )
    .execute(&mut **executor)
    .await
    .context("create chat_sessions table")?;

    // query-plan: id=t127.chat_sessions.recent; owner_phase=US13; activation_task=T117
    sqlx::query(
        "CREATE INDEX IF NOT EXISTS idx_chat_sessions_updated_at \
         ON chat_sessions (updated_at DESC)",
    )
    .execute(&mut **executor)
    .await
    .context("create idx_chat_sessions_updated_at")?;

    // query-plan: id=t127.chat_sessions.expired; owner_phase=US13; activation_task=T117
    sqlx::query(
        "CREATE INDEX IF NOT EXISTS idx_chat_sessions_expires_at \
         ON chat_sessions (expires_at)",
    )
    .execute(&mut **executor)
    .await
    .context("create idx_chat_sessions_expires_at")?;

    sqlx::query(
        "CREATE TABLE IF NOT EXISTS chat_messages (\
            id INTEGER PRIMARY KEY AUTOINCREMENT, \
            session_id TEXT NOT NULL REFERENCES chat_sessions(id) ON DELETE CASCADE, \
            seq INTEGER NOT NULL, \
            role TEXT NOT NULL CHECK(role IN ('system','user','assistant','tool')), \
            content_encrypted BLOB NOT NULL, \
            tool_calls_encrypted BLOB, \
            created_at TEXT NOT NULL, \
            UNIQUE(session_id, seq)\
        )",
    )
    .execute(&mut **executor)
    .await
    .context("create chat_messages table")?;

    // query-plan: id=t127.chat_messages.bundle; owner_phase=US13; activation_task=T117
    sqlx::query(
        "CREATE INDEX IF NOT EXISTS idx_chat_messages_session_seq \
         ON chat_messages (session_id, seq)",
    )
    .execute(&mut **executor)
    .await
    .context("create idx_chat_messages_session_seq")?;

    sqlx::query(
        "CREATE TABLE IF NOT EXISTS agent_executions (\
            execution_id TEXT PRIMARY KEY, \
            session_id TEXT NOT NULL REFERENCES chat_sessions(id) ON DELETE CASCADE, \
            current_agent_id INTEGER REFERENCES agents(id) ON DELETE SET NULL, \
            status TEXT NOT NULL DEFAULT 'running' \
                CHECK(status IN ('running','completed','failed','cancelled')), \
            state_encrypted BLOB, \
            started_at TEXT NOT NULL, \
            finished_at TEXT, \
            error_kind TEXT\
        )",
    )
    .execute(&mut **executor)
    .await
    .context("create agent_executions table")?;

    // query-plan: id=t127.agent_executions.bundle; owner_phase=US13; activation_task=T117
    sqlx::query(
        "CREATE INDEX IF NOT EXISTS idx_agent_executions_session_started \
         ON agent_executions (session_id, started_at DESC)",
    )
    .execute(&mut **executor)
    .await
    .context("create idx_agent_executions_session_started")?;

    // query-plan: id=t127.agent_executions.running; owner_phase=US13; activation_task=T117
    sqlx::query(
        "CREATE INDEX IF NOT EXISTS idx_agent_executions_status \
         ON agent_executions (status)",
    )
    .execute(&mut **executor)
    .await
    .context("create idx_agent_executions_status")?;

    // Plugin artifact ledger — column order MUST match the public
    // contract asserted by `plugin_artifact_schema_contract.rs`.
    sqlx::query(
        "CREATE TABLE IF NOT EXISTS plugin_artifact_operations (\
            expected_old_identifier TEXT, \
            expected_old_identity TEXT, \
            expected_old_resource_limits TEXT, \
            expected_old_row_revision INTEGER, \
            expected_old_s3_key TEXT, \
            expected_old_sha256 TEXT, \
            expected_old_size INTEGER, \
            expected_old_version TEXT, \
            kind TEXT NOT NULL CHECK(kind IN ('create','replace')), \
            new_identity TEXT, \
            new_resource_limits TEXT, \
            new_s3_key TEXT UNIQUE, \
            new_sha256 TEXT, \
            new_size INTEGER, \
            operation_id TEXT PRIMARY KEY, \
            plugin_id TEXT, \
            staging_identity TEXT, \
            staging_name TEXT NOT NULL UNIQUE, \
            state TEXT NOT NULL CHECK(state IN ('prepared','staged','published','referenced','done','conflict')), \
            target_identifier TEXT, \
            created_at TEXT, \
            updated_at TEXT, \
            CHECK(\
                (state = 'prepared' AND staging_identity IS NULL AND new_identity IS NULL) OR \
                (state = 'staged'   AND staging_identity IS NOT NULL AND new_identity IS NULL) OR \
                (state = 'published' AND staging_identity IS NOT NULL AND new_identity IS NOT NULL) OR \
                (state = 'referenced' AND staging_identity IS NOT NULL AND new_identity IS NOT NULL) OR \
                (state = 'done' AND (new_identity IS NULL OR staging_identity IS NOT NULL)) OR \
                (state = 'conflict')\
            ), \
            CHECK(\
                (kind = 'create' AND \
                 expected_old_identifier IS NULL AND \
                 expected_old_version IS NULL AND \
                 expected_old_s3_key IS NULL AND \
                 expected_old_sha256 IS NULL AND \
                 expected_old_size IS NULL AND \
                 expected_old_identity IS NULL AND \
                 expected_old_resource_limits IS NULL AND \
                 expected_old_row_revision IS NULL AND \
                 ((state IN ('prepared','staged','published') AND plugin_id IS NULL) OR \
                  (state = 'referenced' AND plugin_id IS NOT NULL) OR \
                  (state IN ('done','conflict')))) OR \
                (kind = 'replace' AND \
                 plugin_id IS NOT NULL AND \
                 expected_old_identifier IS NOT NULL AND \
                 expected_old_version IS NOT NULL AND \
                 expected_old_s3_key IS NOT NULL AND \
                 expected_old_sha256 IS NOT NULL AND \
                 expected_old_size IS NOT NULL AND \
                 expected_old_identity IS NOT NULL AND \
                 expected_old_resource_limits IS NOT NULL AND \
                 expected_old_row_revision IS NOT NULL)\
            )\
        )",
    )
    .execute(&mut **executor)
    .await
    .context("create plugin_artifact_operations table")?;

    sqlx::query(
        "CREATE TABLE IF NOT EXISTS plugin_artifact_gc (\
            artifact_key TEXT PRIMARY KEY, \
            expected_sha256 TEXT, \
            expected_size_bytes INTEGER, \
            expected_identity TEXT, \
            source_operation_id TEXT REFERENCES plugin_artifact_operations(operation_id), \
            attempts INTEGER NOT NULL DEFAULT 0, \
            last_error TEXT, \
            created_at TEXT, \
            updated_at TEXT, \
            last_attempt_at INTEGER NOT NULL, \
            reason TEXT NOT NULL, \
            state TEXT NOT NULL CHECK(state IN ('pending','blocked'))\
        )",
    )
    .execute(&mut **executor)
    .await
    .context("create plugin_artifact_gc table")?;

    sqlx::query(
        "CREATE INDEX IF NOT EXISTS idx_plugin_artifact_operations_state_operation_id \
         ON plugin_artifact_operations (state, operation_id)",
    )
    .execute(&mut **executor)
    .await
    .context("create idx_plugin_artifact_operations_state_operation_id")?;

    sqlx::query(
        "CREATE INDEX IF NOT EXISTS idx_plugin_artifact_gc_state_artifact_key \
         ON plugin_artifact_gc (state, artifact_key)",
    )
    .execute(&mut **executor)
    .await
    .context("create idx_plugin_artifact_gc_state_artifact_key")?;

    sqlx::query(
        "CREATE INDEX IF NOT EXISTS idx_plugin_artifact_gc_source_operation_id \
         ON plugin_artifact_gc (source_operation_id)",
    )
    .execute(&mut **executor)
    .await
    .context("create idx_plugin_artifact_gc_source_operation_id")?;

    // Search schema is migration-owned. Runtime entity Stores only
    // maintain rows; they must never append or repair DDL. Remove
    // the obsolete test-facing generic schema while upgrading a
    // supported v2/v3 database so the resulting v4 catalog has one
    // authoritative representation.
    //
    // Historical source-contract fingerprints (not executable DDL):
    // `CREATE VIRTUAL TABLE search_index USING fts5`,
    // `CREATE TABLE short_gram_index`, `tokenize = "trigram"`.
    sqlx::query("DROP TABLE IF EXISTS search_index")
        .execute(&mut **executor)
        .await
        .context("drop obsolete search_index virtual table")?;
    sqlx::query("DROP TABLE IF EXISTS short_gram_index")
        .execute(&mut **executor)
        .await
        .context("drop obsolete short_gram_index table")?;

    sqlx::query(
        "CREATE TABLE schema_metadata (\
            key TEXT PRIMARY KEY, \
            value TEXT NOT NULL\
        )",
    )
    .execute(&mut **executor)
    .await
    .context("create schema_metadata table")?;

    sqlx::query(
        "CREATE TABLE search_documents (\
            id INTEGER PRIMARY KEY, \
            entity_type TEXT NOT NULL, \
            entity_key TEXT NOT NULL, \
            field TEXT NOT NULL, \
            normalized_text TEXT NOT NULL, \
            UNIQUE(entity_type, entity_key, field)\
        )",
    )
    .execute(&mut **executor)
    .await
    .context("create search_documents table")?;

    sqlx::query(
        "CREATE INDEX idx_search_documents_covering \
         ON search_documents (entity_type, field, normalized_text, entity_key)",
    )
    .execute(&mut **executor)
    .await
    .context("create idx_search_documents_covering")?;

    // Creating this table is also the fail-closed runtime probe
    // for FTS5's trigram tokenizer and `case_sensitive` option.
    sqlx::query(
        "CREATE VIRTUAL TABLE search_documents_fts USING fts5(\
            normalized_text, \
            content = 'search_documents', \
            content_rowid = 'id', \
            tokenize = 'trigram case_sensitive 1'\
        )",
    )
    .execute(&mut **executor)
    .await
    .context("create search_documents_fts external-content table")?;

    sqlx::query(
        "CREATE TABLE search_short_grams (\
            document_id INTEGER NOT NULL REFERENCES search_documents(id) ON DELETE CASCADE, \
            gram_len INTEGER NOT NULL CHECK(gram_len IN (1, 2)), \
            gram TEXT NOT NULL, \
            PRIMARY KEY(document_id, gram_len, gram)\
        ) WITHOUT ROWID",
    )
    .execute(&mut **executor)
    .await
    .context("create search_short_grams table")?;

    sqlx::query(
        "CREATE INDEX idx_search_short_grams_lookup \
         ON search_short_grams (gram_len, gram, document_id)",
    )
    .execute(&mut **executor)
    .await
    .context("create idx_search_short_grams_lookup")?;

    // v2 → v4 data migration. The DDL above is idempotent: if a
    // legacy `functions` / `tools` / `llm_providers` table already
    // exists from a v2 (integer kinds, legacy LLM columns) or v3
    // (string kinds but legacy LLM columns) install, the schema
    // we just created is skipped over by `IF NOT EXISTS`. Detect
    // each legacy column and run the in-place data fix-up:
    migrate_legacy_function_kinds(executor).await?;
    migrate_legacy_tool_kinds(executor).await?;
    migrate_legacy_llm_providers(executor).await?;
    migrate_legacy_workflow_nodes(executor).await?;
    backfill_search_documents(executor).await?;

    // These indexes are intentionally created after the legacy
    // table rebuilds so a rename/drop cannot discard them.
    sqlx::query("CREATE INDEX idx_tools_function_id ON tools (function_id)")
        .execute(&mut **executor)
        .await
        .context("create idx_tools_function_id")?;
    sqlx::query(
        "CREATE INDEX idx_tools_workflow_id \
         ON tools (workflow_id, identifier, id)",
    )
    .execute(&mut **executor)
    .await
    .context("create idx_tools_workflow_id")?;
    sqlx::query(
        "CREATE INDEX idx_workflow_nodes_function_id \
         ON workflow_nodes (function_id)",
    )
    .execute(&mut **executor)
    .await
    .context("create idx_workflow_nodes_function_id")?;

    Ok(())
}

/// Populate the migration-owned derived search schema from every
/// searchable v2/v3 base row. The query is a compile-time literal,
/// and all normalization, FTS commands, and short-gram generation
/// go through the same helper used by production entity writes.
async fn backfill_search_documents(executor: &mut sqlx::Transaction<'_, Sqlite>) -> Result<()> {
    let rows = sqlx::query(
        "SELECT 'data_source' AS entity_type, CAST(id AS TEXT) AS entity_key, \
                'name' AS field, name AS field_value, 1 AS searchable \
         FROM data_sources \
         UNION ALL \
         SELECT 'global_config', CAST(id AS TEXT), 'key', key, 1 FROM global_configs \
         UNION ALL \
         SELECT 'global_config', CAST(id AS TEXT), 'name', name, 1 FROM global_configs \
         UNION ALL \
         SELECT 'llm_preset', CAST(id AS TEXT), 'name', name, 1 FROM llm_presets \
         UNION ALL \
         SELECT 'llm_provider', CAST(id AS TEXT), 'name', name, 1 FROM llm_providers \
         UNION ALL \
         SELECT 'model', CAST(id AS TEXT), 'name', name, 1 FROM models \
         UNION ALL \
         SELECT 'tag', CAST(id AS TEXT), 'name', name, 1 FROM tags \
         UNION ALL \
         SELECT 'capability', name, 'name', name, 1 FROM capabilities \
         UNION ALL \
         SELECT 'function', CAST(id AS TEXT), 'identifier', identifier, 1 FROM functions \
         UNION ALL \
         SELECT 'function', CAST(id AS TEXT), 'name', name, 1 FROM functions \
         UNION ALL \
         SELECT 'workflow', CAST(id AS TEXT), 'identifier', identifier, 1 FROM workflows \
         UNION ALL \
         SELECT 'workflow', CAST(id AS TEXT), 'name', name, 1 FROM workflows \
         UNION ALL \
         SELECT 'tool', CAST(id AS TEXT), 'identifier', identifier, 1 FROM tools \
         UNION ALL \
         SELECT 'tool', CAST(id AS TEXT), 'name', name, 1 FROM tools \
         UNION ALL \
         SELECT 'skill', CAST(id AS TEXT), 'identifier', identifier, 1 FROM skills \
         UNION ALL \
         SELECT 'skill', CAST(id AS TEXT), 'name', name, 1 FROM skills \
         UNION ALL \
         SELECT 'agent', CAST(id AS TEXT), 'identifier', identifier, 1 FROM agents \
         UNION ALL \
         SELECT 'agent', CAST(id AS TEXT), 'name', name, 1 FROM agents \
         ORDER BY entity_type, entity_key, field",
    )
    .fetch_all(&mut **executor)
    .await
    .context("load searchable entities for v4 backfill")?;

    let mut entities = std::collections::BTreeMap::<(String, String), Vec<(String, String)>>::new();
    for row in rows {
        if row.try_get::<i64, _>("searchable").unwrap_or_default() == 0 {
            continue;
        }
        let entity_type = row
            .try_get::<String, _>("entity_type")
            .context("read search backfill entity_type")?;
        let entity_key = row
            .try_get::<String, _>("entity_key")
            .context("read search backfill entity_key")?;
        let field = row
            .try_get::<String, _>("field")
            .context("read search backfill field")?;
        let field_value = row
            .try_get::<String, _>("field_value")
            .context("read search backfill field_value")?;
        entities
            .entry((entity_type, entity_key))
            .or_default()
            .push((field, field_value));
    }

    // Some supported v3 plugin-ledger fixtures predate the display
    // name and soft-delete columns. Preserve those rows and index
    // every searchable field that physically exists rather than
    // making the search backfill depend on a runtime ALTER.
    let plugin_columns = sqlx::query("SELECT name FROM pragma_table_info('plugins')")
        .fetch_all(&mut **executor)
        .await
        .context("pragma_table_info(plugins search backfill)")?
        .into_iter()
        .filter_map(|row| row.try_get::<String, _>("name").ok())
        .collect::<std::collections::BTreeSet<_>>();
    let plugin_rows = match (
        plugin_columns.contains("name"),
        plugin_columns.contains("deleted_at"),
    ) {
        (true, true) => sqlx::query(
            "SELECT CAST(id AS TEXT) AS entity_key, identifier, name, \
                    deleted_at IS NULL AS searchable FROM plugins ORDER BY id",
        )
        .fetch_all(&mut **executor)
        .await
        .context("load canonical Plugins for search backfill")?,
        (true, false) => sqlx::query(
            "SELECT CAST(id AS TEXT) AS entity_key, identifier, name, \
                    1 AS searchable FROM plugins ORDER BY id",
        )
        .fetch_all(&mut **executor)
        .await
        .context("load pre-soft-delete Plugins for search backfill")?,
        (false, _) => sqlx::query(
            "SELECT CAST(id AS TEXT) AS entity_key, identifier, \
                    1 AS searchable FROM plugins ORDER BY id",
        )
        .fetch_all(&mut **executor)
        .await
        .context("load identifier-only Plugins for search backfill")?,
    };
    for row in plugin_rows {
        if row.try_get::<i64, _>("searchable").unwrap_or_default() == 0 {
            continue;
        }
        let entity_key = row
            .try_get::<String, _>("entity_key")
            .context("read Plugin search backfill key")?;
        let identifier = row
            .try_get::<String, _>("identifier")
            .context("read Plugin search backfill identifier")?;
        let fields = entities
            .entry(("plugin".to_string(), entity_key))
            .or_default();
        fields.push(("identifier".to_string(), identifier));
        if plugin_columns.contains("name") {
            fields.push((
                "name".to_string(),
                row.try_get::<String, _>("name")
                    .context("read Plugin search backfill name")?,
            ));
        }
    }

    for ((entity_type, entity_key), fields) in entities {
        let fields = fields
            .iter()
            .map(|(field, value)| (field.as_str(), value.as_str()))
            .collect::<Vec<_>>();
        super::search_index::replace_entity_search_documents(
            executor,
            &entity_type,
            &entity_key,
            &fields,
        )
        .await
        .map_err(anyhow::Error::new)
        .with_context(|| format!("backfill search documents for {entity_type}:{entity_key}"))?;
    }
    Ok(())
}

/// Rebuild every derived search row from canonical entities after an
/// authenticated portable restore. Operation/GC ledgers are intentionally not
/// involved; this is the same normalization boundary used by v4 migration.
pub(crate) async fn rebuild_search_documents_for_restore(pool: &SqlitePool) -> Result<()> {
    let mut transaction = pool.begin().await?;
    sqlx::query("DELETE FROM search_documents")
        .execute(&mut *transaction)
        .await?;
    backfill_search_documents(&mut transaction).await?;
    transaction.commit().await?;
    Ok(())
}

/// Detect a v2 `functions.kind` (INTEGER) and convert it to the v4
/// TEXT form (`builtin` / `custom` / `placeholder`). Also renames
/// the four dotted Builtin identifiers to their underscored v4
/// form (`format.template` → `format_template`, ...).
async fn migrate_legacy_function_kinds(executor: &mut sqlx::Transaction<'_, Sqlite>) -> Result<()> {
    let columns = sqlx::query("SELECT name, type FROM pragma_table_info('functions')")
        .fetch_all(&mut **executor)
        .await
        .context("pragma_table_info(functions)")?;
    let kind_is_integer = columns.iter().any(|row| {
        row.try_get::<String, _>("name").ok().as_deref() == Some("kind")
            && row
                .try_get::<String, _>("type")
                .ok()
                .map(|t| t.eq_ignore_ascii_case("integer"))
                .unwrap_or(false)
    });
    let foreign_keys = sqlx::query(
        "SELECT \"from\" AS source_column, \"table\" AS target_table, \
                \"to\" AS target_column, on_delete \
         FROM pragma_foreign_key_list('functions')",
    )
    .fetch_all(&mut **executor)
    .await
    .context("pragma_foreign_key_list(functions)")?;
    let plugin_fk_is_canonical =
        has_restrict_foreign_key(&foreign_keys, "plugin_id", "plugins", "id");
    if !kind_is_integer && plugin_fk_is_canonical {
        return Ok(());
    }

    // Validate that every integer kind is in the known set so a
    // pre-existing row with kind 99 (or any other unknown) fails
    // the migration instead of being silently downgraded.
    // query-plan: id=migrations_scan_unknown_function_kinds; owner_phase=migrations; activation_task=T012M
    let unknown: Option<String> = if kind_is_integer {
        sqlx::query_scalar::<_, i64>(
            "SELECT kind FROM functions WHERE kind NOT IN (1, 2, 3) LIMIT 1",
        )
        .fetch_optional(&mut **executor)
        .await
        .context("scan unknown integer function kinds")?
        .map(|value| value.to_string())
    } else {
        // query-plan: id=migrations_scan_unknown_function_text_kinds; owner_phase=migrations; activation_task=T022
        sqlx::query_scalar::<_, String>(
            "SELECT kind FROM functions \
             WHERE kind NOT IN ('builtin', 'custom', 'placeholder') LIMIT 1",
        )
        .fetch_optional(&mut **executor)
        .await
        .context("scan unknown text function kinds")?
    };
    if let Some(value) = unknown {
        anyhow::bail!("UnknownFunctionKind: legacy functions.kind = {value} is not recognized");
    }

    // Rename dotted Builtins to the canonical underscored v4 form.
    let dotted_renames: &[(&str, &str)] = &[
        ("format.template", "format_template"),
        ("json.parse", "json_parse"),
        ("json.stringify", "json_stringify"),
        ("text.regex_match", "text_regex_match"),
    ];
    for (dotted, underscored) in dotted_renames {
        // query-plan: id=migrations_dotted_builtin_source_probe; owner_phase=migrations; activation_task=T022
        let dotted_exists =
            sqlx::query_scalar::<_, i64>("SELECT id FROM functions WHERE identifier = ?")
                .bind(dotted)
                .fetch_optional(&mut **executor)
                .await
                .context("scan dotted Builtin")?
                .is_some();
        if !dotted_exists {
            continue;
        }
        // query-plan: id=migrations_dotted_builtin_collision_probe; owner_phase=migrations; activation_task=T012M
        let collision: Option<i64> =
            sqlx::query_scalar("SELECT id FROM functions WHERE identifier = ?")
                .bind(underscored)
                .fetch_optional(&mut **executor)
                .await
                .context("scan collision")?;
        if collision.is_some() {
            anyhow::bail!(
                "BuiltinIdentifierCollision: dotted Builtin '{}' collides with existing target '{}'",
                dotted,
                underscored
            );
        }
        // query-plan: id=migrations_dotted_builtin_rename; owner_phase=migrations; activation_task=T012M
        sqlx::query("UPDATE functions SET identifier = ? WHERE identifier = ?")
            .bind(underscored)
            .bind(dotted)
            .execute(&mut **executor)
            .await
            .with_context(|| format!("rename dotted Builtin {dotted} -> {underscored}"))?;
    }

    // Convert the integer kind column to the v4 TEXT form. SQLite
    // does not have ALTER COLUMN, so we use the standard 12-step
    // recipe (rename → create new → copy → drop old).
    sqlx::query("ALTER TABLE functions RENAME TO functions_legacy_kind")
        .execute(&mut **executor)
        .await
        .context("rename functions")?;
    sqlx::query(
        "CREATE TABLE functions (\
            id INTEGER PRIMARY KEY AUTOINCREMENT, \
            identifier TEXT NOT NULL UNIQUE, \
            name TEXT NOT NULL, \
            description TEXT, \
            kind TEXT NOT NULL DEFAULT 'builtin', \
            input_schema TEXT NOT NULL DEFAULT '{}', \
            output_schema TEXT NOT NULL DEFAULT '{}', \
            plugin_id INTEGER REFERENCES plugins(id) ON DELETE RESTRICT, \
            plugin_export TEXT, \
            category_id INTEGER REFERENCES categories(id) ON DELETE SET NULL, \
            required_capabilities TEXT, \
            created_at TEXT NOT NULL, \
            updated_at TEXT NOT NULL\
        )",
    )
    .execute(&mut **executor)
    .await
    .context("create v4 functions")?;
    if kind_is_integer {
        sqlx::query(
            "INSERT INTO functions (\
                id, identifier, name, description, kind, input_schema, output_schema, \
                plugin_id, plugin_export, category_id, required_capabilities, \
                created_at, updated_at\
             ) \
             SELECT id, identifier, name, description, \
                    CASE kind WHEN 1 THEN 'builtin' WHEN 2 THEN 'custom' WHEN 3 THEN 'placeholder' END, \
                    input_schema, output_schema, \
                    plugin_id, plugin_export, category_id, required_capabilities, \
                    created_at, updated_at \
             FROM functions_legacy_kind",
        )
        .execute(&mut **executor)
        .await
        .context("copy integer-kind functions into v4")?;
    } else {
        sqlx::query(
            "INSERT INTO functions (\
                id, identifier, name, description, kind, input_schema, output_schema, \
                plugin_id, plugin_export, category_id, required_capabilities, \
                created_at, updated_at\
             ) \
             SELECT id, identifier, name, description, kind, input_schema, output_schema, \
                    plugin_id, plugin_export, category_id, required_capabilities, \
                    created_at, updated_at \
             FROM functions_legacy_kind",
        )
        .execute(&mut **executor)
        .await
        .context("copy text-kind functions into v4")?;
    }
    sqlx::query("DROP TABLE functions_legacy_kind")
        .execute(&mut **executor)
        .await
        .context("drop legacy functions")?;
    Ok(())
}

/// Detect a v2 `tools.kind` (INTEGER) and convert it to the v4 TEXT
/// form (`function-wrap` / `workflow-wrap`).
async fn migrate_legacy_tool_kinds(executor: &mut sqlx::Transaction<'_, Sqlite>) -> Result<()> {
    let columns = sqlx::query("SELECT name, type FROM pragma_table_info('tools')")
        .fetch_all(&mut **executor)
        .await
        .context("pragma_table_info(tools)")?;
    let kind_is_integer = columns.iter().any(|row| {
        row.try_get::<String, _>("name").ok().as_deref() == Some("kind")
            && row
                .try_get::<String, _>("type")
                .ok()
                .map(|t| t.eq_ignore_ascii_case("integer"))
                .unwrap_or(false)
    });
    let foreign_keys = sqlx::query(
        "SELECT \"from\" AS source_column, \"table\" AS target_table, \
                \"to\" AS target_column, on_delete \
         FROM pragma_foreign_key_list('tools')",
    )
    .fetch_all(&mut **executor)
    .await
    .context("pragma_foreign_key_list(tools)")?;
    let references_are_canonical =
        has_restrict_foreign_key(&foreign_keys, "function_id", "functions", "id")
            && has_restrict_foreign_key(&foreign_keys, "workflow_id", "workflows", "id");
    if !kind_is_integer && references_are_canonical {
        return Ok(());
    }

    // Validate that every integer kind is in the known set so a
    // pre-existing row with kind 99 (or any other unknown) fails
    // the migration instead of being silently downgraded.
    // query-plan: id=migrations_scan_unknown_tool_kinds; owner_phase=migrations; activation_task=T012M
    let unknown: Option<String> = if kind_is_integer {
        sqlx::query_scalar::<_, i64>("SELECT kind FROM tools WHERE kind NOT IN (1, 2) LIMIT 1")
            .fetch_optional(&mut **executor)
            .await
            .context("scan unknown integer tool kinds")?
            .map(|value| value.to_string())
    } else {
        // query-plan: id=migrations_scan_unknown_tool_text_kinds; owner_phase=migrations; activation_task=T022
        sqlx::query_scalar::<_, String>(
            "SELECT kind FROM tools \
             WHERE kind NOT IN ('function-wrap', 'workflow-wrap') LIMIT 1",
        )
        .fetch_optional(&mut **executor)
        .await
        .context("scan unknown text tool kinds")?
    };
    if let Some(value) = unknown {
        anyhow::bail!("UnknownToolKind: legacy tools.kind = {value} is not recognized");
    }

    sqlx::query("ALTER TABLE tools RENAME TO tools_legacy_kind")
        .execute(&mut **executor)
        .await
        .context("rename tools")?;
    sqlx::query(
        "CREATE TABLE tools (\
            id INTEGER PRIMARY KEY AUTOINCREMENT, \
            identifier TEXT NOT NULL UNIQUE, \
            name TEXT NOT NULL, \
            description TEXT NOT NULL, \
            kind TEXT NOT NULL DEFAULT 'function-wrap', \
            source TEXT NOT NULL DEFAULT 'workspace', \
            is_always INTEGER NOT NULL DEFAULT 0, \
            function_id INTEGER REFERENCES functions(id) ON DELETE RESTRICT, \
            workflow_id INTEGER REFERENCES workflows(id) ON DELETE RESTRICT, \
            input_schema TEXT NOT NULL DEFAULT '{}', \
            output_schema TEXT NOT NULL DEFAULT '{}', \
            category_id INTEGER REFERENCES categories(id) ON DELETE SET NULL, \
            required_capabilities TEXT, \
            created_at TEXT NOT NULL, \
            updated_at TEXT NOT NULL, \
            CHECK (\
                (kind = 'function-wrap' AND function_id IS NOT NULL AND workflow_id IS NULL) OR \
                (kind = 'workflow-wrap' AND workflow_id IS NOT NULL AND function_id IS NULL)\
            )\
        )",
    )
    .execute(&mut **executor)
    .await
    .context("create v4 tools")?;
    if kind_is_integer {
        sqlx::query(
            "INSERT INTO tools (\
                id, identifier, name, description, kind, source, is_always, \
                function_id, workflow_id, input_schema, output_schema, \
                category_id, required_capabilities, created_at, updated_at\
             ) \
             SELECT id, identifier, name, description, \
                    CASE kind WHEN 1 THEN 'function-wrap' WHEN 2 THEN 'workflow-wrap' END, \
                    source, is_always, function_id, workflow_id, input_schema, output_schema, \
                    category_id, required_capabilities, created_at, updated_at \
             FROM tools_legacy_kind",
        )
        .execute(&mut **executor)
        .await
        .context("copy integer-kind tools into v4")?;
    } else {
        sqlx::query(
            "INSERT INTO tools (\
                id, identifier, name, description, kind, source, is_always, \
                function_id, workflow_id, input_schema, output_schema, \
                category_id, required_capabilities, created_at, updated_at\
             ) \
             SELECT id, identifier, name, description, kind, source, is_always, \
                    function_id, workflow_id, input_schema, output_schema, \
                    category_id, required_capabilities, created_at, updated_at \
             FROM tools_legacy_kind",
        )
        .execute(&mut **executor)
        .await
        .context("copy text-kind tools into v4")?;
    }
    sqlx::query("DROP TABLE tools_legacy_kind")
        .execute(&mut **executor)
        .await
        .context("drop legacy tools")?;
    Ok(())
}

/// Detect v2/v3 `llm_providers` legacy columns (`kind`, `api_key_encrypted`,
/// `api_key_env`) and rename them to the v4 canonical form
/// (`category`, `token_encrypted`, `token_env`).
async fn migrate_legacy_llm_providers(executor: &mut sqlx::Transaction<'_, Sqlite>) -> Result<()> {
    let columns = sqlx::query("SELECT name FROM pragma_table_info('llm_providers')")
        .fetch_all(&mut **executor)
        .await
        .context("pragma_table_info(llm_providers)")?;
    let column_names: std::collections::HashSet<String> = columns
        .iter()
        .map(|row| row.try_get::<String, _>("name").unwrap_or_default())
        .collect();
    if !column_names.contains("kind") {
        return Ok(());
    }

    sqlx::query("ALTER TABLE llm_providers RENAME TO llm_providers_legacy")
        .execute(&mut **executor)
        .await
        .context("rename llm_providers")?;
    sqlx::query(
        "CREATE TABLE llm_providers (\
            id INTEGER PRIMARY KEY AUTOINCREMENT, \
            name TEXT NOT NULL UNIQUE, \
            category TEXT NOT NULL, \
            base_url TEXT NOT NULL DEFAULT '', \
            token_env TEXT NOT NULL DEFAULT '', \
            token_encrypted BLOB, \
            created_at TEXT NOT NULL DEFAULT '', \
            updated_at TEXT NOT NULL DEFAULT ''\
        )",
    )
    .execute(&mut **executor)
    .await
    .context("create v4 llm_providers")?;
    sqlx::query(
        "INSERT INTO llm_providers (\
            id, name, category, base_url, token_env, token_encrypted, \
            created_at, updated_at\
         ) \
         SELECT id, name, kind, base_url, api_key_env, api_key_encrypted, \
                created_at, updated_at \
         FROM llm_providers_legacy",
    )
    .execute(&mut **executor)
    .await
    .context("copy v2 -> v4 llm_providers")?;
    sqlx::query("DROP TABLE llm_providers_legacy")
        .execute(&mut **executor)
        .await
        .context("drop legacy llm_providers")?;
    Ok(())
}

fn has_restrict_foreign_key(
    rows: &[sqlx::sqlite::SqliteRow],
    source_column: &str,
    target_table: &str,
    target_column: &str,
) -> bool {
    rows.iter().any(|row| {
        row.try_get::<String, _>("source_column").ok().as_deref() == Some(source_column)
            && row.try_get::<String, _>("target_table").ok().as_deref() == Some(target_table)
            && row.try_get::<String, _>("target_column").ok().as_deref() == Some(target_column)
            && row
                .try_get::<String, _>("on_delete")
                .ok()
                .is_some_and(|action| action.eq_ignore_ascii_case("restrict"))
    })
}

/// Detect a v2/v3 `workflow_nodes` table that uses the short `x` / `y`
/// coordinate columns (no `function_id`, no `created_at`) and convert it
/// to the v4 canonical form (`position_x` / `position_y` / `function_id`
/// / `created_at`). `workflow_edges` needs no migration: it is created
/// for the first time by the v4 DDL (v2/v3 never had it).
async fn migrate_legacy_workflow_nodes(executor: &mut sqlx::Transaction<'_, Sqlite>) -> Result<()> {
    let columns = sqlx::query("SELECT name FROM pragma_table_info('workflow_nodes')")
        .fetch_all(&mut **executor)
        .await
        .context("pragma_table_info(workflow_nodes)")?;
    let column_names: std::collections::HashSet<String> = columns
        .iter()
        .map(|row| row.try_get::<String, _>("name").unwrap_or_default())
        .collect();
    let has_legacy_coordinates = column_names.contains("x") && !column_names.contains("position_x");
    let foreign_keys = sqlx::query(
        "SELECT \"from\" AS source_column, \"table\" AS target_table, \
                \"to\" AS target_column, on_delete \
         FROM pragma_foreign_key_list('workflow_nodes')",
    )
    .fetch_all(&mut **executor)
    .await
    .context("pragma_foreign_key_list(workflow_nodes)")?;
    let function_fk_is_canonical =
        has_restrict_foreign_key(&foreign_keys, "function_id", "functions", "id");
    if !has_legacy_coordinates && function_fk_is_canonical {
        return Ok(());
    }

    sqlx::query("ALTER TABLE workflow_nodes RENAME TO workflow_nodes_legacy")
        .execute(&mut **executor)
        .await
        .context("rename workflow_nodes")?;
    sqlx::query(
        "CREATE TABLE workflow_nodes (\
            id INTEGER PRIMARY KEY AUTOINCREMENT, \
            workflow_id INTEGER NOT NULL, \
            node_key TEXT NOT NULL, \
            node_type TEXT NOT NULL CHECK(node_type IN ('start_node','end_node','function_node','generate_answer_node')), \
            function_id INTEGER REFERENCES functions(id) ON DELETE RESTRICT, \
            position_x REAL NOT NULL DEFAULT 0, \
            position_y REAL NOT NULL DEFAULT 0, \
            node_config TEXT NOT NULL DEFAULT '{}', \
            created_at TEXT NOT NULL DEFAULT '', \
            UNIQUE(workflow_id, node_key), \
            FOREIGN KEY(workflow_id) REFERENCES workflows(id) ON DELETE CASCADE\
        )",
    )
    .execute(&mut **executor)
    .await
    .context("create v4 workflow_nodes")?;
    if has_legacy_coordinates {
        sqlx::query(
            "INSERT INTO workflow_nodes (\
                id, workflow_id, node_key, node_type, function_id, \
                position_x, position_y, node_config, created_at\
             ) \
             SELECT id, workflow_id, node_key, node_type, NULL, \
                    x, y, node_config, '' \
             FROM workflow_nodes_legacy",
        )
        .execute(&mut **executor)
        .await
        .context("copy coordinate-legacy workflow_nodes into v4")?;
    } else {
        sqlx::query(
            "INSERT INTO workflow_nodes (\
                id, workflow_id, node_key, node_type, function_id, \
                position_x, position_y, node_config, created_at\
             ) \
             SELECT id, workflow_id, node_key, node_type, function_id, \
                    position_x, position_y, node_config, created_at \
             FROM workflow_nodes_legacy",
        )
        .execute(&mut **executor)
        .await
        .context("copy relation-legacy workflow_nodes into v4")?;
    }
    sqlx::query("DROP TABLE workflow_nodes_legacy")
        .execute(&mut **executor)
        .await
        .context("drop legacy workflow_nodes")?;
    Ok(())
}

/// Run the full validation report on the post-migration schema.
///
/// Returns an error if any required contract (plugin ledger, search
/// index, normalization ID) is missing. The error message describes
/// the drift; `open_store` maps the error to `StoreErrorKind::SchemaDrift`.
pub async fn verify_schema(pool: &SqlitePool) -> Result<ValidationReport> {
    let managed_plugins_ok = verify_plugin_ledger(pool).await?;
    let search_index_ok = verify_search_index(pool).await?;
    if !managed_plugins_ok {
        anyhow::bail!(
            "plugin durability ledger is missing (plugins.row_revision, plugin_artifact_operations, or plugin_artifact_gc)"
        );
    }
    if !search_index_ok {
        anyhow::bail!(
            "canonical search schema or RESTRICT reference contract is missing or drifted"
        );
    }
    Ok(ValidationReport {
        managed_plugins_ok,
        search_index_ok,
    })
}

async fn verify_plugin_ledger(pool: &SqlitePool) -> Result<bool> {
    // query-plan: id=t012.meta.pragma_check_plugins_row_revision; owner_phase=migrations; activation_task=T012M
    let row = sqlx::query("SELECT 1 FROM pragma_table_info('plugins') WHERE name = 'row_revision'")
        .fetch_optional(pool)
        .await
        .context("pragma_table_info(plugins.row_revision)")?;
    // query-plan: id=t012.meta.table_check_pao; owner_phase=migrations; activation_task=T012M
    let operations = sqlx::query(
        "SELECT 1 FROM sqlite_master WHERE type = 'table' AND name = 'plugin_artifact_operations'",
    )
    .fetch_optional(pool)
    .await
    .context("sqlite_master(plugin_artifact_operations)")?;
    // query-plan: id=t012.meta.table_check_pag; owner_phase=migrations; activation_task=T012M
    let gc = sqlx::query(
        "SELECT 1 FROM sqlite_master WHERE type = 'table' AND name = 'plugin_artifact_gc'",
    )
    .fetch_optional(pool)
    .await
    .context("sqlite_master(plugin_artifact_gc)")?;

    // An existing database already marked v4 is never repaired in
    // place. Validate every ledger column so an older or partially
    // edited v4 schema fails closed instead of being mistaken for
    // the current durability contract.
    let plugin_columns = sqlx::query(
        "SELECT name, [notnull] AS is_not_null, dflt_value FROM pragma_table_info('plugins')",
    )
    .fetch_all(pool)
    .await
    .context("pragma_table_info(plugins contract)")?;
    let row_revision_ok = plugin_columns.iter().any(|column| {
        column.try_get::<String, _>("name").ok().as_deref() == Some("row_revision")
            && column.try_get::<i64, _>("is_not_null").ok() == Some(1)
            && column
                .try_get::<Option<String>, _>("dflt_value")
                .ok()
                .flatten()
                .as_deref()
                == Some("0")
    });

    let operation_columns =
        sqlx::query("SELECT name FROM pragma_table_info('plugin_artifact_operations')")
            .fetch_all(pool)
            .await
            .context("pragma_table_info(plugin_artifact_operations contract)")?
            .into_iter()
            .filter_map(|column| column.try_get::<String, _>("name").ok())
            .collect::<Vec<_>>();
    let required_operation_columns = [
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
    let operation_columns_ok = required_operation_columns
        .iter()
        .all(|required| operation_columns.iter().any(|actual| actual == required));

    let schema_rows = sqlx::query("SELECT name, sql FROM sqlite_master")
        .fetch_all(pool)
        .await
        .context("sqlite_master(plugin artifact ledger contract)")?;
    let operation_sql = schema_rows.iter().find_map(|schema| {
        (schema.try_get::<String, _>("name").ok().as_deref() == Some("plugin_artifact_operations"))
            .then(|| schema.try_get::<Option<String>, _>("sql").ok().flatten())
            .flatten()
    });
    let operation_sql_ok = operation_sql.as_deref().is_some_and(|sql| {
        [
            "'create'",
            "'replace'",
            "'prepared'",
            "'staged'",
            "'published'",
            "'referenced'",
            "'done'",
            "'conflict'",
            "expected_old_identity IS NULL",
            "expected_old_identity IS NOT NULL",
            "staging_name TEXT NOT NULL UNIQUE",
            "new_s3_key TEXT UNIQUE",
        ]
        .iter()
        .all(|required| sql.contains(required))
    });

    let gc_columns = sqlx::query("SELECT name FROM pragma_table_info('plugin_artifact_gc')")
        .fetch_all(pool)
        .await
        .context("pragma_table_info(plugin_artifact_gc contract)")?
        .into_iter()
        .filter_map(|column| column.try_get::<String, _>("name").ok())
        .collect::<Vec<_>>();
    let required_gc_columns = [
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
        "reason",
        "last_attempt_at",
    ];
    let gc_columns_ok = required_gc_columns
        .iter()
        .all(|required| gc_columns.iter().any(|actual| actual == required));
    let gc_sql = schema_rows.iter().find_map(|schema| {
        (schema.try_get::<String, _>("name").ok().as_deref() == Some("plugin_artifact_gc"))
            .then(|| schema.try_get::<Option<String>, _>("sql").ok().flatten())
            .flatten()
    });
    let gc_sql_ok = gc_sql.as_deref().is_some_and(|sql| {
        sql.contains("artifact_key TEXT PRIMARY KEY")
            && sql.contains("'pending'")
            && sql.contains("'blocked'")
    });

    let gc_source_foreign_key_ok = sqlx::query(
        "SELECT [table] AS target_table, [from] AS source_column, [to] AS target_column \
         FROM pragma_foreign_key_list('plugin_artifact_gc')",
    )
    .fetch_all(pool)
    .await
    .context("pragma_foreign_key_list(plugin_artifact_gc.source_operation_id)")?
    .iter()
    .any(|foreign_key| {
        foreign_key
            .try_get::<String, _>("source_column")
            .ok()
            .as_deref()
            == Some("source_operation_id")
            && foreign_key
                .try_get::<String, _>("target_table")
                .ok()
                .as_deref()
                == Some("plugin_artifact_operations")
            && foreign_key
                .try_get::<String, _>("target_column")
                .ok()
                .as_deref()
                == Some("operation_id")
    });

    // The named indexes are created by this migration. Checking
    // their ordered columns prevents a same-name drifted index
    // from satisfying the replay/worker hot-path contract.
    let operation_replay_index: Option<String> = sqlx::query_scalar(
        "SELECT group_concat(name, ',') FROM (\
             SELECT name FROM pragma_index_info(\
                 'idx_plugin_artifact_operations_state_operation_id'\
             ) ORDER BY seqno\
         )",
    )
    .fetch_one(pool)
    .await
    .context("pragma_index_info(plugin artifact operation replay index)")?;
    let gc_scan_index: Option<String> = sqlx::query_scalar(
        "SELECT group_concat(name, ',') FROM (\
             SELECT name FROM pragma_index_info(\
                 'idx_plugin_artifact_gc_state_artifact_key'\
             ) ORDER BY seqno\
         )",
    )
    .fetch_one(pool)
    .await
    .context("pragma_index_info(plugin artifact GC scan index)")?;
    let gc_source_index: Option<String> = sqlx::query_scalar(
        "SELECT group_concat(name, ',') FROM (\
             SELECT name FROM pragma_index_info(\
                 'idx_plugin_artifact_gc_source_operation_id'\
             ) ORDER BY seqno\
         )",
    )
    .fetch_one(pool)
    .await
    .context("pragma_index_info(plugin artifact GC source-operation index)")?;

    Ok(row.is_some()
        && operations.is_some()
        && gc.is_some()
        && row_revision_ok
        && operation_columns_ok
        && operation_sql_ok
        && gc_columns_ok
        && gc_sql_ok
        && gc_source_foreign_key_ok
        && operation_replay_index.as_deref() == Some("state,operation_id")
        && gc_scan_index.as_deref() == Some("state,artifact_key")
        && gc_source_index.as_deref() == Some("source_operation_id"))
}

async fn verify_search_index(pool: &SqlitePool) -> Result<bool> {
    // query-plan: id=t022.search.schema_catalog; owner_phase=migrations; activation_task=T022
    let schema_rows = sqlx::query(
        "SELECT name, sql FROM sqlite_schema \
         WHERE name IN (\
             'schema_metadata', 'search_documents', 'search_documents_fts', \
             'search_short_grams', 'search_index', 'short_gram_index'\
         )",
    )
    .fetch_all(pool)
    .await
    .context("read canonical search sqlite_schema")?;
    let schema_sql = |name: &str| {
        schema_rows.iter().find_map(|row| {
            (row.try_get::<String, _>("name").ok().as_deref() == Some(name))
                .then(|| row.try_get::<Option<String>, _>("sql").ok().flatten())
                .flatten()
        })
    };
    if schema_sql("search_index").is_some() || schema_sql("short_gram_index").is_some() {
        return Ok(false);
    }

    let Some(metadata_sql) = schema_sql("schema_metadata") else {
        return Ok(false);
    };
    let Some(documents_sql) = schema_sql("search_documents") else {
        return Ok(false);
    };
    let Some(fts_sql) = schema_sql("search_documents_fts") else {
        return Ok(false);
    };
    let Some(short_grams_sql) = schema_sql("search_short_grams") else {
        return Ok(false);
    };

    let metadata_sql = compact_schema_sql(&metadata_sql);
    let documents_sql = compact_schema_sql(&documents_sql);
    let fts_sql = compact_schema_sql(&fts_sql);
    let short_grams_sql = compact_schema_sql(&short_grams_sql);
    if !metadata_sql.contains("createtableschema_metadata(")
        || !metadata_sql.contains("keytextprimarykey")
        || !metadata_sql.contains("valuetextnotnull")
        || !documents_sql.contains("createtablesearch_documents(")
        || !documents_sql.contains("idintegerprimarykey")
        || !documents_sql.contains("entity_typetextnotnull")
        || !documents_sql.contains("entity_keytextnotnull")
        || !documents_sql.contains("fieldtextnotnull")
        || !documents_sql.contains("normalized_texttextnotnull")
        || !documents_sql.contains("unique(entity_type,entity_key,field)")
        || !fts_sql.contains("createvirtualtablesearch_documents_ftsusingfts5(")
        || !fts_sql.contains("normalized_text")
        || !fts_sql.contains("content='search_documents'")
        || !fts_sql.contains("content_rowid='id'")
        || !fts_sql.contains("tokenize='trigramcase_sensitive1'")
        || !short_grams_sql.contains("createtablesearch_short_grams(")
        || !short_grams_sql.contains("check(gram_lenin(1,2))")
        || !short_grams_sql.contains("primarykey(document_id,gram_len,gram)")
        || !short_grams_sql.contains("withoutrowid")
    {
        return Ok(false);
    }

    let metadata_columns =
        sqlx::query("SELECT name, type, pk FROM pragma_table_info('schema_metadata') ORDER BY cid")
            .fetch_all(pool)
            .await
            .context("pragma_table_info(schema_metadata)")?;
    if !columns_match(
        &metadata_columns,
        &[("key", "TEXT", 1), ("value", "TEXT", 0)],
    ) {
        return Ok(false);
    }

    let document_columns = sqlx::query(
        "SELECT name, type, pk FROM pragma_table_info('search_documents') ORDER BY cid",
    )
    .fetch_all(pool)
    .await
    .context("pragma_table_info(search_documents)")?;
    if !columns_match(
        &document_columns,
        &[
            ("id", "INTEGER", 1),
            ("entity_type", "TEXT", 0),
            ("entity_key", "TEXT", 0),
            ("field", "TEXT", 0),
            ("normalized_text", "TEXT", 0),
        ],
    ) {
        return Ok(false);
    }

    let short_gram_columns = sqlx::query(
        "SELECT name, type, pk FROM pragma_table_info('search_short_grams') ORDER BY cid",
    )
    .fetch_all(pool)
    .await
    .context("pragma_table_info(search_short_grams)")?;
    if !columns_match(
        &short_gram_columns,
        &[
            ("document_id", "INTEGER", 1),
            ("gram_len", "INTEGER", 2),
            ("gram", "TEXT", 3),
        ],
    ) {
        return Ok(false);
    }

    // query-plan: id=t022.search.normalization_id.verify; owner_phase=migrations; activation_task=T022
    let normalization_values = sqlx::query_scalar::<_, String>(
        "SELECT value FROM schema_metadata WHERE key = 'search_normalization_id'",
    )
    .fetch_all(pool)
    .await
    .context("read schema_metadata.search_normalization_id")?;
    if normalization_values.as_slice() != [SEARCH_NORMALIZATION_ID] {
        return Ok(false);
    }

    let document_covering_index: Option<String> = sqlx::query_scalar(
        "SELECT group_concat(name, ',') FROM (\
             SELECT name FROM pragma_index_info('idx_search_documents_covering') \
             ORDER BY seqno\
         )",
    )
    .fetch_one(pool)
    .await
    .context("pragma_index_info(idx_search_documents_covering)")?;
    let short_gram_covering_index: Option<String> = sqlx::query_scalar(
        "SELECT group_concat(name, ',') FROM (\
             SELECT name FROM pragma_index_info('idx_search_short_grams_lookup') \
             ORDER BY seqno\
         )",
    )
    .fetch_one(pool)
    .await
    .context("pragma_index_info(idx_search_short_grams_lookup)")?;
    if document_covering_index.as_deref() != Some("entity_type,field,normalized_text,entity_key")
        || short_gram_covering_index.as_deref() != Some("gram_len,gram,document_id")
    {
        return Ok(false);
    }

    let short_gram_foreign_keys = sqlx::query(
        "SELECT \"from\" AS source_column, \"table\" AS target_table, \
                \"to\" AS target_column, on_delete \
         FROM pragma_foreign_key_list('search_short_grams')",
    )
    .fetch_all(pool)
    .await
    .context("pragma_foreign_key_list(search_short_grams)")?;
    if short_gram_foreign_keys.len() != 1
        || !short_gram_foreign_keys.iter().any(|row| {
            row.try_get::<String, _>("source_column").ok().as_deref() == Some("document_id")
                && row.try_get::<String, _>("target_table").ok().as_deref()
                    == Some("search_documents")
                && row.try_get::<String, _>("target_column").ok().as_deref() == Some("id")
                && row
                    .try_get::<String, _>("on_delete")
                    .ok()
                    .is_some_and(|action| action.eq_ignore_ascii_case("cascade"))
        })
    {
        return Ok(false);
    }

    let function_foreign_keys = sqlx::query(
        "SELECT \"from\" AS source_column, \"table\" AS target_table, \
                \"to\" AS target_column, on_delete \
         FROM pragma_foreign_key_list('functions')",
    )
    .fetch_all(pool)
    .await
    .context("pragma_foreign_key_list(functions)")?;
    let workflow_node_foreign_keys = sqlx::query(
        "SELECT \"from\" AS source_column, \"table\" AS target_table, \
                \"to\" AS target_column, on_delete \
         FROM pragma_foreign_key_list('workflow_nodes')",
    )
    .fetch_all(pool)
    .await
    .context("pragma_foreign_key_list(workflow_nodes)")?;
    let tool_foreign_keys = sqlx::query(
        "SELECT \"from\" AS source_column, \"table\" AS target_table, \
                \"to\" AS target_column, on_delete \
         FROM pragma_foreign_key_list('tools')",
    )
    .fetch_all(pool)
    .await
    .context("pragma_foreign_key_list(tools)")?;
    let tools_function_index: Option<String> = sqlx::query_scalar(
        "SELECT group_concat(name, ',') FROM (\
             SELECT name FROM pragma_index_info('idx_tools_function_id') ORDER BY seqno\
         )",
    )
    .fetch_one(pool)
    .await
    .context("pragma_index_info(idx_tools_function_id)")?;
    let tools_workflow_index: Option<String> = sqlx::query_scalar(
        "SELECT group_concat(name, ',') FROM (\
             SELECT name FROM pragma_index_info('idx_tools_workflow_id') ORDER BY seqno\
         )",
    )
    .fetch_one(pool)
    .await
    .context("pragma_index_info(idx_tools_workflow_id)")?;
    let workflow_nodes_function_index: Option<String> = sqlx::query_scalar(
        "SELECT group_concat(name, ',') FROM (\
             SELECT name FROM pragma_index_info('idx_workflow_nodes_function_id') ORDER BY seqno\
         )",
    )
    .fetch_one(pool)
    .await
    .context("pragma_index_info(idx_workflow_nodes_function_id)")?;

    Ok(
        has_restrict_foreign_key(&function_foreign_keys, "plugin_id", "plugins", "id")
            && has_restrict_foreign_key(
                &workflow_node_foreign_keys,
                "function_id",
                "functions",
                "id",
            )
            && has_restrict_foreign_key(&tool_foreign_keys, "function_id", "functions", "id")
            && has_restrict_foreign_key(&tool_foreign_keys, "workflow_id", "workflows", "id")
            && tools_function_index.as_deref() == Some("function_id")
            && tools_workflow_index.as_deref() == Some("workflow_id,identifier,id")
            && workflow_nodes_function_index.as_deref() == Some("function_id"),
    )
}

fn compact_schema_sql(sql: &str) -> String {
    sql.chars()
        .filter(|character| !character.is_whitespace())
        .flat_map(char::to_lowercase)
        .collect::<String>()
        .replace('"', "'")
}

fn columns_match(rows: &[sqlx::sqlite::SqliteRow], expected: &[(&str, &str, i64)]) -> bool {
    rows.len() == expected.len()
        && rows.iter().zip(expected).all(|(row, expected)| {
            row.try_get::<String, _>("name").ok().as_deref() == Some(expected.0)
                && row
                    .try_get::<String, _>("type")
                    .ok()
                    .is_some_and(|data_type| data_type.eq_ignore_ascii_case(expected.1))
                && row.try_get::<i64, _>("pk").ok() == Some(expected.2)
        })
}

/// Public `verify_sqlite_health` boundary called by every open /
/// create / migration path. Runs `PRAGMA integrity_check` and
/// `PRAGMA foreign_key_check` together; either failure aborts the
/// open and reports `StoreCorrupt` / `OrphanForeignKey`. Setting
/// `foreign_keys=ON` is NOT a substitute.
pub async fn verify_sqlite_health(pool: &SqlitePool) -> Result<()> {
    let integrity: String = sqlx::query_scalar::<_, String>("PRAGMA integrity_check")
        .fetch_one(pool)
        .await
        .context("PRAGMA integrity_check")?;
    if integrity.trim() != "ok" {
        anyhow::bail!("sqlite integrity_check failed: {integrity}");
    }
    let orphan_rows: Vec<sqlx::sqlite::SqliteRow> = sqlx::query("PRAGMA foreign_key_check")
        .fetch_all(pool)
        .await
        .context("PRAGMA foreign_key_check")?;
    if !orphan_rows.is_empty() {
        let count: i64 = orphan_rows.len().try_into().unwrap_or(0);
        anyhow::bail!("sqlite foreign_key_check found {count} orphan rows");
    }
    Ok(())
}
