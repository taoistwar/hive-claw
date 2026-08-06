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

use crate::datasource::plugin_artifacts::OperationState;

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
        let artifact_dir = self.inner.root.join(input.identifier());
        std::fs::create_dir_all(&artifact_dir).map_err(|_| PluginInstallError {
            kind: PluginInstallErrorKind::Io,
            references: Vec::new(),
        })?;
        let artifact_path = artifact_dir.join(format!("{}.wasm", input.version()));
        std::fs::write(&artifact_path, input.bytes()).map_err(|_| PluginInstallError {
            kind: PluginInstallErrorKind::Io,
            references: Vec::new(),
        })?;
        let now = chrono::Utc::now().to_rfc3339();
        let fingerprint = {
            let mut hasher = Sha256::new();
            hasher.update(input.bytes());
            format!("{:x}", hasher.finalize())
        };
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
        Ok(PluginRecord {
            id: row.0,
            name: input.identifier().to_string(),
            version: input.version().to_string(),
            fingerprint_hex: fingerprint,
        })
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
        let path = self
            .inner
            .root
            .join(&identifier)
            .join(format!("{}.wasm", version));
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
}

/// SHA-256 helper used by tests + callers that want to compare
/// fingerprints against a hex digest computed externally.
pub fn sha256_hex(bytes: &[u8]) -> String {
    let mut hasher = Sha256::new();
    hasher.update(bytes);
    format!("{:x}", hasher.finalize())
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
