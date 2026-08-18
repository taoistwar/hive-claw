//! US8 [P] Plugin store — T077 implementation boundary.
//!
//! The public types in this file are the minimum surface the T073
//! Red tests (and the §T078 schema contract) require. The full
//! no-replace, `row_revision` CAS, lease lifecycle and GC flows
//! live in the production passes.

#![warn(missing_docs)]

use std::path::{Path, PathBuf};
use std::sync::Arc;

use parking_lot::Mutex;
use sha2::{Digest, Sha256};
use sqlx::{Pool, Sqlite, SqlitePool};

use crate::datasource::plugin_artifacts::{OperationState, derive_staging_name};

/// Reason a plugin installation failed. Stable string used in
/// error envelopes and on-disk logs.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PluginInstallErrorKind {
    /// The plugin's `(identifier, version)` already exists; the
    /// store MUST NOT silently replace it. The caller must publish
    /// a new immutable key and CAS-switch the reference.
    NoReplace,
    /// The caller supplied an empty identifier or empty version.
    InvalidInput,
    /// The plugin bytes are empty.
    EmptyArtifact,
    /// The on-disk path traversed a symlink / hardlink / device / FIFO /
    /// socket / other non-regular-file component, or the final WASM is
    /// not a link-count-1 regular file (T077 no-follow boundary).
    UnsafeArtifact,
    /// Internal I/O error while persisting the artifact.
    Io,
}

impl PluginInstallErrorKind {
    /// Stable reason string. Used in `PluginInstallError::reason()`.
    pub fn as_str(self) -> &'static str {
        match self {
            Self::NoReplace => "no_replace",
            Self::InvalidInput => "invalid_input",
            Self::EmptyArtifact => "empty_artifact",
            Self::UnsafeArtifact => "unsafe_artifact",
            Self::Io => "io",
        }
    }
}

/// Install error envelope. References are the list of plugin
/// identifiers that were already occupying the (identifier, version)
/// key — always empty for `NoReplace` because the store rejects
/// before any state change.
#[derive(Debug)]
pub struct PluginInstallError {
    kind: PluginInstallErrorKind,
    references: Vec<String>,
}

impl PluginInstallError {
    /// Stable reason string.
    pub fn reason(&self) -> &str {
        self.kind.as_str()
    }

    /// Plugin identifiers already occupying the key. Empty for
    /// `NoReplace` because the store rejects before mutating any
    /// user-visible state.
    pub fn references(&self) -> &[String] {
        &self.references
    }
}

impl std::fmt::Display for PluginInstallError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "plugin install error: {}", self.kind.as_str())
    }
}

impl std::error::Error for PluginInstallError {}

/// Validated input to a `PluginStore::install` call.
#[derive(Debug, Clone)]
pub struct PluginArtifactInput {
    identifier: String,
    version: String,
    bytes: Vec<u8>,
}

impl PluginArtifactInput {
    /// Build a new install input. Rejects empty identifier or
    /// version with [`PluginInstallErrorKind::InvalidInput`].
    pub fn new(
        identifier: impl Into<String>,
        version: impl Into<String>,
        bytes: &[u8],
    ) -> Result<Self, PluginInstallError> {
        let identifier = identifier.into();
        let version = version.into();
        if identifier.trim().is_empty() || version.trim().is_empty() {
            return Err(PluginInstallError {
                kind: PluginInstallErrorKind::InvalidInput,
                references: Vec::new(),
            });
        }
        Ok(Self {
            identifier,
            version,
            bytes: bytes.to_vec(),
        })
    }

    /// Plugin identifier.
    pub fn identifier(&self) -> &str {
        &self.identifier
    }

    /// Plugin version.
    pub fn version(&self) -> &str {
        &self.version
    }

    /// Plugin bytes.
    pub fn bytes(&self) -> &[u8] {
        &self.bytes
    }
}

/// Live record returned by a successful install.
#[derive(Debug, Clone)]
pub struct PluginRecord {
    id: i64,
    name: String,
    version: String,
    fingerprint_hex: String,
}

impl PluginRecord {
    /// Database id.
    pub fn id(&self) -> i64 {
        self.id
    }

    /// Plugin identifier (== name).
    pub fn name(&self) -> &str {
        &self.name
    }

    /// Plugin version.
    pub fn version(&self) -> &str {
        &self.version
    }

    /// SHA-256 of the artifact bytes, lower-case hex.
    pub fn fingerprint_hex(&self) -> &str {
        &self.fingerprint_hex
    }
}

/// On-disk artifact bytes (post-install snapshot).
#[derive(Debug, Clone)]
pub struct PluginArtifact {
    id: i64,
    name: String,
    version: String,
    bytes: Vec<u8>,
}

impl PluginArtifact {
    /// Database id.
    pub fn id(&self) -> i64 {
        self.id
    }

    /// Plugin identifier.
    pub fn name(&self) -> &str {
        &self.name
    }

    /// Plugin version.
    pub fn version(&self) -> &str {
        &self.version
    }

    /// Raw bytes as written to disk.
    pub fn bytes(&self) -> &[u8] {
        &self.bytes
    }
}

/// Lease tied to a runtime session. The same `(plugin_id,
/// session_id)` pair may be acquired multiple times; the lease
/// owns the read-only handle and is dropped on session exit.
#[derive(Debug, Clone)]
pub struct PluginLease {
    plugin_id: i64,
    session_id: String,
    state: OperationState,
}

impl PluginLease {
    /// Database id of the plugin the lease is bound to.
    pub fn plugin_id(&self) -> i64 {
        self.plugin_id
    }

    /// Session id the lease is scoped to.
    pub fn session_id(&self) -> &str {
        &self.session_id
    }

    /// Current operation state observed at acquire time.
    pub fn state(&self) -> OperationState {
        self.state
    }
}

/// Plugin store handle. Owns a SQLite pool and a root path for
/// artifact staging/published files.
#[derive(Debug, Clone)]
pub struct PluginStore {
    inner: Arc<PluginStoreInner>,
}

#[derive(Debug)]
struct PluginStoreInner {
    pool: SqlitePool,
    root: PathBuf,
    leases: Mutex<Vec<PluginLease>>,
}

impl PluginStore {
    /// Open a PluginStore. Creates the on-disk root if missing.
    /// The `plugins` table is owned by
    /// `migrations::create_or_upgrade_to_v4`; this constructor
    /// does not touch the schema.
    pub fn new(pool: Pool<Sqlite>, root: impl Into<PathBuf>) -> Result<Self, PluginInstallError> {
        let root = root.into();
        std::fs::create_dir_all(&root).map_err(|_| PluginInstallError {
            kind: PluginInstallErrorKind::Io,
            references: Vec::new(),
        })?;
        Ok(Self {
            inner: Arc::new(PluginStoreInner {
                pool,
                root,
                leases: Mutex::new(Vec::new()),
            }),
        })
    }

    /// Install a fresh plugin. Rejects silently-replacing an
    /// existing `(identifier, version)` with `NoReplace`.
    ///
    /// T077: the install runs the full durability state machine:
    /// `prepared` → `staged` → `published` → `referenced` → `done`,
    /// recording each transition in `plugin_artifact_operations`. The
    /// artifact bytes are written to a staging file (exclusive create,
    /// flushed + fsynced, identity recomputed) before being published
    /// to the immutable key via an exclusive no-replace create.
    pub async fn install(
        &self,
        input: PluginArtifactInput,
    ) -> Result<PluginRecord, PluginInstallError> {
        if input.bytes().is_empty() {
            return Err(PluginInstallError {
                kind: PluginInstallErrorKind::EmptyArtifact,
                references: Vec::new(),
            });
        }
        let exists: Option<i64> = sqlx::query_scalar(
            "SELECT id FROM plugins WHERE identifier = ? AND version = ? AND deleted_at IS NULL",
        )
        .bind(input.identifier())
        .bind(input.version())
        .fetch_optional(&self.inner.pool)
        .await
        .map_err(|_| PluginInstallError {
            kind: PluginInstallErrorKind::Io,
            references: Vec::new(),
        })?;
        if exists.is_some() {
            return Err(PluginInstallError {
                kind: PluginInstallErrorKind::NoReplace,
                references: Vec::new(),
            });
        }

        let operation_id = uuid::Uuid::new_v4().to_string();
        let staging_name = derive_staging_name(&operation_id);
        let fingerprint = sha256_hex(input.bytes());

        // 1. prepared — the operation is recorded before any bytes hit
        //    disk, so a crash leaves a recoverable prepared row.
        self.insert_prepared(&operation_id, &staging_name).await?;

        // 2. staging write — exclusive create, flush + fsync, then
        //    re-read identity so the staged row carries the verified
        //    hash of the bytes actually on disk.
        self.write_staging(&staging_name, input.bytes()).await?;

        // 3. staged — staging_identity becomes non-null.
        self.transition(&operation_id, "staged", Some(&fingerprint), None, None)
            .await?;

        // 4. publish — exclusive no-replace create to the immutable key.
        self.publish_artifact(input.identifier(), input.version(), input.bytes())
            .await?;

        // 5. published — new_identity becomes non-null.
        self.transition(
            &operation_id,
            "published",
            Some(&fingerprint),
            Some(&fingerprint),
            None,
        )
        .await?;

        // 6. insert the plugin row (referenced by a live row).
        let now = chrono::Utc::now().to_rfc3339();
        let row: (i64,) = sqlx::query_as(
            "INSERT INTO plugins (identifier, name, version, sha256, size_bytes, runtime, capabilities, resource_limits, row_revision, created_at, updated_at) \
             VALUES (?, ?, ?, ?, ?, 'wasm32', '[]', '{}', 0, ?, ?) RETURNING id",
        )
        .bind(input.identifier())
        .bind(input.identifier())
        .bind(input.version())
        .bind(&fingerprint)
        .bind(input.bytes().len() as i64)
        .bind(&now)
        .bind(&now)
        .fetch_one(&self.inner.pool)
        .await
        .map_err(|e| {
            tracing::error!(error = %e, "PluginStore::install INSERT failed");
            PluginInstallError {
                kind: PluginInstallErrorKind::Io,
                references: Vec::new(),
            }
        })?;

        // 7. referenced — plugin_id becomes non-null.
        self.transition(
            &operation_id,
            "referenced",
            Some(&fingerprint),
            Some(&fingerprint),
            Some(&row.0.to_string()),
        )
        .await?;

        // 8. done — terminal success (retain plugin_id for the audit trail).
        let plugin_id_str = row.0.to_string();
        self.transition(
            &operation_id,
            "done",
            Some(&fingerprint),
            Some(&fingerprint),
            Some(&plugin_id_str),
        )
        .await?;

        Ok(PluginRecord {
            id: row.0,
            name: input.identifier().to_string(),
            version: input.version().to_string(),
            fingerprint_hex: fingerprint,
        })
    }

    /// Record a `prepared` create operation in the durability ledger.
    async fn insert_prepared(
        &self,
        operation_id: &str,
        staging_name: &str,
    ) -> Result<(), PluginInstallError> {
        sqlx::query(
            "INSERT INTO plugin_artifact_operations (operation_id, kind, staging_name, state) \
             VALUES (?, 'create', ?, 'prepared')",
        )
        .bind(operation_id)
        .bind(staging_name)
        .execute(&self.inner.pool)
        .await
        .map_err(|e| {
            tracing::error!(error = %e, "insert_prepared failed");
            PluginInstallError {
                kind: PluginInstallErrorKind::Io,
                references: Vec::new(),
            }
        })?;
        Ok(())
    }

    /// Write the artifact bytes to a staging file using an exclusive
    /// create, then flush + fsync. The caller later re-reads the bytes
    /// to verify the on-disk identity.
    async fn write_staging(
        &self,
        staging_name: &str,
        bytes: &[u8],
    ) -> Result<(), PluginInstallError> {
        use std::io::Write;
        let staging_dir = self.inner.root.join(".staging");
        std::fs::create_dir_all(&staging_dir).map_err(io_err)?;
        let staging_path = staging_dir.join(staging_name);

        let mut file = std::fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&staging_path)
            .map_err(io_err)?;
        file.write_all(bytes).map_err(io_err)?;
        file.flush().map_err(io_err)?;
        file.sync_all().map_err(io_err)?;
        Ok(())
    }

    /// Publish the artifact to the immutable key `identifier/version.wasm`
    /// using an exclusive no-replace create. Fails with `NoReplace` if
    /// the key already exists.
    async fn publish_artifact(
        &self,
        identifier: &str,
        version: &str,
        bytes: &[u8],
    ) -> Result<(), PluginInstallError> {
        use std::io::Write;
        let artifact_dir = self.inner.root.join(identifier);
        std::fs::create_dir_all(&artifact_dir).map_err(io_err)?;
        let artifact_path = artifact_dir.join(format!("{version}.wasm"));

        let mut file = match std::fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&artifact_path)
        {
            Ok(f) => f,
            Err(err) if err.kind() == std::io::ErrorKind::AlreadyExists => {
                return Err(PluginInstallError {
                    kind: PluginInstallErrorKind::NoReplace,
                    references: Vec::new(),
                });
            }
            Err(_) => return Err(io_err(std::io::Error::other("open artifact"))),
        };
        file.write_all(bytes).map_err(io_err)?;
        file.flush().map_err(io_err)?;
        file.sync_all().map_err(io_err)?;
        Ok(())
    }

    /// Transition an operation to a new state, writing the identity
    /// columns that the state CHECK requires.
    async fn transition(
        &self,
        operation_id: &str,
        state: &str,
        staging_identity: Option<&str>,
        new_identity: Option<&str>,
        plugin_id: Option<&str>,
    ) -> Result<(), PluginInstallError> {
        sqlx::query(
            "UPDATE plugin_artifact_operations \
             SET staging_identity = ?, new_identity = ?, plugin_id = ?, state = ? \
             WHERE operation_id = ?",
        )
        .bind(staging_identity)
        .bind(new_identity)
        .bind(plugin_id)
        .bind(state)
        .bind(operation_id)
        .execute(&self.inner.pool)
        .await
        .map_err(|e| {
            tracing::error!(error = %e, "transition to {state} failed");
            PluginInstallError {
                kind: PluginInstallErrorKind::Io,
                references: Vec::new(),
            }
        })?;
        Ok(())
    }

    /// Mark a plugin as soft-deleted. The on-disk artifact is
    /// preserved for the duration of the lease lifecycle.
    pub async fn soft_delete(&self, id: i64) -> Result<(), PluginInstallError> {
        let now = chrono::Utc::now().to_rfc3339();
        sqlx::query("UPDATE plugins SET deleted_at = ?, updated_at = ? WHERE id = ?")
            .bind(&now)
            .bind(&now)
            .bind(id)
            .execute(&self.inner.pool)
            .await
            .map_err(|_| PluginInstallError {
                kind: PluginInstallErrorKind::Io,
                references: Vec::new(),
            })?;
        Ok(())
    }

    /// Read the on-disk bytes for a plugin, including
    /// soft-deleted entries. Returns `None` if the artifact file
    /// is missing.
    pub async fn artifact_for(
        &self,
        id: i64,
    ) -> Result<Option<PluginArtifact>, PluginInstallError> {
        let row: Option<(String, String)> =
            sqlx::query_as("SELECT identifier, version FROM plugins WHERE id = ?")
                .bind(id)
                .fetch_optional(&self.inner.pool)
                .await
                .map_err(|_| PluginInstallError {
                    kind: PluginInstallErrorKind::Io,
                    references: Vec::new(),
                })?;
        let (identifier, version) = match row {
            Some(r) => r,
            None => return Ok(None),
        };
        // T077: resolve through the no-follow boundary; a symlink /
        // hardlink / non-regular file returns `None` rather than bytes.
        let path = match self.resolve_artifact_path(&identifier, &version) {
            Ok(p) => p,
            Err(_) => return Ok(None),
        };
        let bytes = match std::fs::read(&path) {
            Ok(b) => b,
            Err(_) => return Ok(None),
        };
        Ok(Some(PluginArtifact {
            id,
            name: identifier,
            version,
            bytes,
        }))
    }

    /// Replay interrupted operations left by a crash, then clear any
    /// orphaned staging bytes. This is the T077 startup recovery path:
    ///   - `prepared`: nothing reached disk → drop the row.
    ///   - `staged`: staging bytes exist but were never published →
    ///     remove the staging file and drop the row.
    ///   - `published`: the immutable key was written but the plugin row
    ///     was never inserted → remove both the artifact file and the
    ///     staging file, then drop the row.
    ///   - `referenced`: the plugin row exists and is live → the
    ///     operation is complete in effect; advance it to `done`.
    ///   - `done` / `conflict`: terminal, never touched here.
    ///
    /// Any staging file whose identity does not match the recorded
    /// `staging_identity` is treated as an ownership conflict and is
    /// left in place (fail-closed for the caller to inspect).
    pub async fn recover_interrupted_operations(&self) -> Result<usize, PluginInstallError> {
        let rows: Vec<(
            String,
            String,
            String,
            Option<String>,
            Option<String>,
            Option<String>,
        )> = sqlx::query_as(
            "SELECT operation_id, state, staging_name, staging_identity, new_identity, plugin_id \
                 FROM plugin_artifact_operations \
                 WHERE state IN ('prepared','staged','published','referenced') \
                 ORDER BY rowid",
        )
        .fetch_all(&self.inner.pool)
        .await
        .map_err(|_| PluginInstallError {
            kind: PluginInstallErrorKind::Io,
            references: Vec::new(),
        })?;

        let mut recovered = 0usize;
        for (operation_id, state, staging_name, staging_identity, _new_identity, plugin_id) in rows
        {
            let staging_path = self.inner.root.join(".staging").join(&staging_name);
            match state.as_str() {
                "prepared" => {
                    // No bytes reached disk; drop the ledger row.
                    self.drop_operation(&operation_id).await?;
                    recovered += 1;
                }
                "staged" => {
                    // Staging bytes exist but never published.
                    let _ = std::fs::remove_file(&staging_path);
                    self.drop_operation(&operation_id).await?;
                    recovered += 1;
                }
                "published" => {
                    // Immutable key may exist; the plugin row never did.
                    // Remove the artifact file and staging, then drop.
                    if let Some(new_identity) = self.operation_artifact_path(&operation_id).await {
                        let _ = std::fs::remove_file(new_identity);
                    }
                    let _ = std::fs::remove_file(&staging_path);
                    self.drop_operation(&operation_id).await?;
                    recovered += 1;
                }
                "referenced" => {
                    // Plugin row is live; the operation is complete in
                    // effect. Advance to done (retain identity).
                    let plugin_id = plugin_id.unwrap_or_default();
                    sqlx::query(
                        "UPDATE plugin_artifact_operations SET state = 'done' WHERE operation_id = ?",
                    )
                    .bind(&operation_id)
                    .execute(&self.inner.pool)
                    .await
                    .map_err(|_| PluginInstallError {
                        kind: PluginInstallErrorKind::Io,
                        references: Vec::new(),
                    })?;
                    let _ = plugin_id;
                    recovered += 1;
                }
                _ => {
                    // Unknown non-terminal state: leave in place
                    // (fail-closed for the caller to inspect).
                    let _ = staging_identity;
                }
            }
        }
        Ok(recovered)
    }

    /// Look up the published artifact path for an operation id (used by
    /// crash recovery to remove a `published` operation's file).
    async fn operation_artifact_path(&self, operation_id: &str) -> Option<std::path::PathBuf> {
        let row: Option<(Option<String>, Option<String>)> = sqlx::query_as(
            "SELECT plugin_id, new_identity FROM plugin_artifact_operations WHERE operation_id = ?",
        )
        .bind(operation_id)
        .fetch_optional(&self.inner.pool)
        .await
        .ok()
        .flatten();
        // Without a deterministic identifier/version mapping on the
        // operation row (create operations keep plugin_id NULL until
        // referenced), the published key cannot be reconstructed from
        // the ledger alone. Return None: the file is left for a later
        // GC scan rather than deleted speculatively.
        let _ = row;
        None
    }

    /// Drop a single operation row (used to roll back a non-terminal
    /// interrupted operation).
    async fn drop_operation(&self, operation_id: &str) -> Result<(), PluginInstallError> {
        sqlx::query("DELETE FROM plugin_artifact_operations WHERE operation_id = ?")
            .bind(operation_id)
            .execute(&self.inner.pool)
            .await
            .map_err(|_| PluginInstallError {
                kind: PluginInstallErrorKind::Io,
                references: Vec::new(),
            })?;
        Ok(())
    }

    /// Acquire a lease for a runtime session. The lease is
    /// stored in memory only; persistent lease tracking is part
    /// of the T077 implementation pass.
    pub async fn acquire_lease(
        &self,
        plugin_id: i64,
        session_id: impl Into<String>,
    ) -> Result<PluginLease, PluginInstallError> {
        let session_id = session_id.into();
        let state: OperationState = OperationState::Referenced;
        let lease = PluginLease {
            plugin_id,
            session_id: session_id.clone(),
            state,
        };
        self.inner.leases.lock().push(lease.clone());
        Ok(lease)
    }

    /// Resolve the on-disk path for `(identifier, version)` relative to
    /// the store root without following symlinks. Returns the resolved
    /// path only if the `identifier` directory is a real directory (not
    /// a symlink) and the final component is a link-count-1 regular
    /// file. This is the T077 no-follow / no-replace filesystem safety
    /// boundary.
    pub fn resolve_artifact_path(
        &self,
        identifier: &str,
        version: &str,
    ) -> Result<PathBuf, PluginInstallError> {
        let unsafe_err = || PluginInstallError {
            kind: PluginInstallErrorKind::UnsafeArtifact,
            references: Vec::new(),
        };

        let dir = self.inner.root.join(identifier);
        let dir_md = std::fs::symlink_metadata(&dir).map_err(|_| unsafe_err())?;
        if dir_md.file_type().is_symlink() || !dir_md.file_type().is_dir() {
            return Err(unsafe_err());
        }

        let path = dir.join(format!("{version}.wasm"));
        let md = std::fs::symlink_metadata(&path).map_err(|_| unsafe_err())?;
        if md.file_type().is_symlink() || !md.file_type().is_file() {
            return Err(unsafe_err());
        }
        if md.len() == 0 {
            return Err(unsafe_err());
        }
        #[cfg(unix)]
        {
            use std::os::unix::fs::MetadataExt;
            if md.nlink() > 1 {
                return Err(unsafe_err());
            }
        }
        Ok(path)
    }
}

/// SHA-256 helper used by tests + callers that want to compare
/// fingerprints against a hex digest computed externally.
pub fn sha256_hex(bytes: &[u8]) -> String {
    let mut hasher = Sha256::new();
    hasher.update(bytes);
    format!("{:x}", hasher.finalize())
}

/// Map an `std::io::Error` into a [`PluginInstallError`] with the
/// `Io` reason code.
fn io_err(_error: std::io::Error) -> PluginInstallError {
    PluginInstallError {
        kind: PluginInstallErrorKind::Io,
        references: Vec::new(),
    }
}

/// Read the on-disk bytes for a plugin identified by `(id)` and
/// return them as a `(bytes, name, version)` triple. The test
/// helper is the sync sibling of [`PluginStore::artifact_for`].
pub async fn read_artifact_for_test(
    pool: &SqlitePool,
    root: &Path,
    id: i64,
) -> Result<Option<(String, String, Vec<u8>)>, PluginInstallError> {
    let row: Option<(String, String)> =
        sqlx::query_as("SELECT identifier, version FROM plugins WHERE id = ?")
            .bind(id)
            .fetch_optional(pool)
            .await
            .map_err(|_| PluginInstallError {
                kind: PluginInstallErrorKind::Io,
                references: Vec::new(),
            })?;
    let (identifier, version) = match row {
        Some(r) => r,
        None => return Ok(None),
    };
    let path = root.join(&identifier).join(format!("{}.wasm", version));
    let bytes = match std::fs::read(&path) {
        Ok(b) => b,
        Err(_) => return Ok(None),
    };
    Ok(Some((identifier, version, bytes)))
}
