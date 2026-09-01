//! US8 [P] Plugin store — T077 implementation boundary.
//!
//! The public types in this file are the minimum surface the T073
//! Red tests (and the §T078 schema contract) require. The full
//! no-replace, `row_revision` CAS, lease lifecycle and GC flows
//! live in the production passes.

#![warn(missing_docs)]

use std::collections::{BTreeSet, HashMap};
use std::path::{Component, Path, PathBuf};
use std::sync::{Arc, LazyLock, Weak};

use hive_runtime_core::wasm::validate_wasm_shape;
use parking_lot::Mutex;
use sha2::{Digest, Sha256};
use sqlx::{Pool, Sqlite, SqlitePool};

use crate::datasource::entity_store::{
    CreatePluginFields, NewArtifact, OperationGcTarget, OperationTransition, Plugin,
    PluginArtifactLedger, PluginLedgerError, PluginResourceLimits, PreparePluginOperation,
};
use crate::datasource::plugin_artifacts::{OperationKind, OperationState};
use crate::datasource::plugin_manifest::{
    build_v1_manifest, manifest_exports, manifest_required_capabilities, validate_manifest,
};
use crate::datasource::wasm_exports::extract_wasm_exports;

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
    /// The `row_revision` CAS failed: another writer changed the plugin
    /// row between read and write. The caller must re-read the tuple and
    /// retry.
    CasConflict,
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
            Self::CasConflict => "cas_conflict",
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

/// Plugin-row metadata persisted by [`PluginStore::install`].
///
/// The six-state install writes the artifact bytes durably and creates the
/// user-visible plugin row in a single flow. The caller supplies the full
/// metadata the UI collected so the row is correct from the moment it becomes
/// live — no follow-up `Plugin::update` is required.
#[derive(Debug, Clone)]
pub struct PluginMetadata {
    name: String,
    description: Option<String>,
    manifest: Option<String>,
    runtime: String,
    capabilities: String,
    resource_limits: String,
}

impl PluginMetadata {
    /// Build plugin metadata. `name` falls back to `identifier` (passed to
    /// [`PluginStore::install`] as [`PluginArtifactInput::identifier`]) when
    /// empty, matching the UI's optional display-name semantics.
    pub fn new(
        name: impl Into<String>,
        description: Option<String>,
        manifest: Option<String>,
        runtime: impl Into<String>,
        capabilities: impl Into<String>,
        resource_limits: impl Into<String>,
    ) -> Self {
        Self {
            name: name.into(),
            description,
            manifest,
            runtime: runtime.into(),
            capabilities: capabilities.into(),
            resource_limits: resource_limits.into(),
        }
    }

    /// Display name; empty means "use the identifier".
    pub fn name(&self) -> &str {
        &self.name
    }

    /// Runtime identifier (e.g. `extism`).
    pub fn runtime(&self) -> &str {
        &self.runtime
    }
}

impl Default for PluginMetadata {
    fn default() -> Self {
        Self {
            name: String::new(),
            description: None,
            manifest: None,
            runtime: "extism".to_string(),
            capabilities: "[]".to_string(),
            resource_limits: "{}".to_string(),
        }
    }
}

/// Live record returned by a successful install.
#[derive(Debug, Clone)]
pub struct PluginRecord {
    id: i64,
    name: String,
    version: String,
    fingerprint_hex: String,
    row_revision: i64,
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

    /// Current `row_revision` of the live plugin row (used for
    /// optimistic CAS on replace).
    pub fn row_revision(&self) -> i64 {
        self.row_revision
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
    _guard: Arc<PluginLeaseGuard>,
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
    lease_tracker: Arc<PluginLeaseTracker>,
}

#[derive(Debug, Default)]
struct PluginLeaseTracker {
    active_by_plugin: Mutex<HashMap<i64, usize>>,
}

impl PluginLeaseTracker {
    fn acquire(&self, plugin_id: i64) {
        *self.active_by_plugin.lock().entry(plugin_id).or_default() += 1;
    }

    fn release(&self, plugin_id: i64) {
        let mut active = self.active_by_plugin.lock();
        let Some(count) = active.get_mut(&plugin_id) else {
            return;
        };
        *count -= 1;
        if *count == 0 {
            active.remove(&plugin_id);
        }
    }

    fn has_active(&self, plugin_id: i64) -> bool {
        self.active_by_plugin
            .lock()
            .get(&plugin_id)
            .is_some_and(|count| *count > 0)
    }
}

#[derive(Debug)]
struct PluginLeaseGuard {
    tracker: Arc<PluginLeaseTracker>,
    plugin_id: i64,
}

impl Drop for PluginLeaseGuard {
    fn drop(&mut self) {
        self.tracker.release(self.plugin_id);
    }
}

static PLUGIN_LEASE_TRACKERS: LazyLock<Mutex<HashMap<PathBuf, Weak<PluginLeaseTracker>>>> =
    LazyLock::new(|| Mutex::new(HashMap::new()));

fn shared_lease_tracker(root: &Path) -> Arc<PluginLeaseTracker> {
    let mut trackers = PLUGIN_LEASE_TRACKERS.lock();
    trackers.retain(|_, tracker| tracker.strong_count() > 0);
    if let Some(tracker) = trackers.get(root).and_then(Weak::upgrade) {
        return tracker;
    }
    let tracker = Arc::new(PluginLeaseTracker::default());
    trackers.insert(root.to_path_buf(), Arc::downgrade(&tracker));
    tracker
}

#[derive(Debug, sqlx::FromRow)]
struct RecoveryOperationRow {
    operation_id: String,
    state: String,
    staging_name: String,
    staging_identity: Option<String>,
    new_identity: Option<String>,
    new_s3_key: Option<String>,
    new_sha256: Option<String>,
    new_size: Option<i64>,
}

#[derive(Debug, sqlx::FromRow)]
struct GcLedgerRow {
    artifact_key: String,
    expected_sha256: Option<String>,
    expected_size_bytes: Option<i64>,
    expected_identity: Option<String>,
    source_operation_id: Option<String>,
    state: String,
    attempts: i64,
    last_error: Option<String>,
    plugin_id: Option<i64>,
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
        let root = std::fs::canonicalize(&root).map_err(io_err)?;
        let lease_tracker = shared_lease_tracker(&root);
        Ok(Self {
            inner: Arc::new(PluginStoreInner {
                pool,
                root,
                lease_tracker,
            }),
        })
    }

    /// Install a fresh plugin with default metadata (name = identifier,
    /// runtime = `extism`, empty capabilities / resource limits). See
    /// [`PluginStore::install_with_metadata`] for the full-metadata path.
    pub async fn install(
        &self,
        input: PluginArtifactInput,
    ) -> Result<PluginRecord, PluginInstallError> {
        self.install_with_metadata(input, PluginMetadata::default())
            .await
    }

    /// Install a fresh plugin with explicit metadata. Rejects
    /// silently-replacing an existing `(identifier, version)` with
    /// `NoReplace`.
    ///
    /// T077: the install runs the full durability state machine:
    /// `prepared` → `staged` → `published` → `referenced` → `done`,
    /// recording each transition in `plugin_artifact_operations`. The
    /// artifact bytes are written to a staging file (exclusive create,
    /// flushed + fsynced, identity recomputed) before being published
    /// to the immutable key via an exclusive no-replace create. The
    /// user-visible plugin row becomes live with the supplied
    /// [`PluginMetadata`], so no follow-up `Plugin::update` is required.
    pub async fn install_with_metadata(
        &self,
        input: PluginArtifactInput,
        mut metadata: PluginMetadata,
    ) -> Result<PluginRecord, PluginInstallError> {
        if input.bytes().is_empty() {
            return Err(PluginInstallError {
                kind: PluginInstallErrorKind::EmptyArtifact,
                references: Vec::new(),
            });
        }

        // FR-031/FR-039: compatibility and field validation is strictly
        // pre-ledger. This helper performs only CPU work and read-only
        // capability lookups; on failure no Plugin row, operation/GC row, or
        // artifact byte has been created.
        let resource_limits = self.prevalidate_install(&input, &mut metadata).await?;

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

        let operation_id = uuid::Uuid::new_v4();
        let fingerprint = sha256_hex(input.bytes());
        // A create operation must know its immutable key before `prepared`,
        // while the database id is intentionally not allocated until the
        // atomic published->referenced transaction. The operation UUID is the
        // non-reused disambiguator; callers resolve the persisted s3_key.
        let artifact_key = format!(
            "{}/{}/{operation_id}/plugin.wasm",
            input.identifier(),
            input.version()
        );
        let ledger = PluginArtifactLedger::new(self.inner.pool.clone());
        let prepared = ledger
            .prepare(
                operation_id,
                PreparePluginOperation::Create {
                    target_identifier: input.identifier().to_string(),
                    new: NewArtifact {
                        s3_key: artifact_key.clone(),
                        sha256: fingerprint.clone(),
                        size_bytes: input.bytes().len() as i64,
                        resource_limits,
                    },
                },
            )
            .await
            .map_err(ledger_install_error)?;
        let staging_name = prepared.staging_name().to_string();

        // 1. prepared — the operation is recorded before any bytes hit
        //    disk, so a crash leaves a recoverable prepared row.
        // 2. staging write — exclusive create, flush + fsync, then
        //    re-read identity so the staged row carries the verified
        //    hash of the bytes actually on disk.
        let staging_identity = self.write_staging(&staging_name, input.bytes()).await?;

        // 3. staged — staging_identity becomes non-null.
        ledger
            .transition(
                operation_id,
                OperationState::Prepared,
                OperationTransition::Staged { staging_identity },
            )
            .await
            .map_err(ledger_install_error)?;

        // 4. Publish the verified bytes, then remove the staging name before
        //    persisting `published`. A crash that leaves both names while the
        //    operation is non-terminal is treated as an ownership conflict by
        //    startup replay.
        let published_identity = self.publish_artifact(&artifact_key, &staging_name).await?;

        // 5. published — new_identity becomes non-null.
        ledger
            .transition(
                operation_id,
                OperationState::Staged,
                OperationTransition::Published {
                    new_identity: published_identity,
                },
            )
            .await
            .map_err(ledger_install_error)?;

        // 6. Insert the user-visible row and move the exact operation to
        //    `referenced` in one SQLite transaction. No placeholder Plugin row
        //    and no latest-row scan is involved.
        let display_name = if metadata.name().is_empty() {
            input.identifier().to_string()
        } else {
            metadata.name().to_string()
        };
        let plugin = ledger
            .commit_published_create(
                operation_id,
                CreatePluginFields {
                    version: input.version().to_string(),
                    name: display_name,
                    description: metadata.description,
                    manifest: metadata.manifest,
                    runtime: metadata.runtime,
                    author: None,
                    repository_url: None,
                    category_id: None,
                    capabilities: metadata.capabilities,
                },
            )
            .await
            .map_err(ledger_install_error)?;

        // 7. Terminal success, addressed by the same operation UUID.
        ledger
            .transition(
                operation_id,
                OperationState::Referenced,
                OperationTransition::Done,
            )
            .await
            .map_err(ledger_install_error)?;

        Ok(PluginRecord {
            id: plugin.id,
            name: input.identifier().to_string(),
            version: input.version().to_string(),
            fingerprint_hex: fingerprint,
            row_revision: plugin.row_revision,
        })
    }

    /// Validate every import-controlled field and the shared ABI contract
    /// before the durability ledger or managed filesystem is mutated.
    async fn prevalidate_install(
        &self,
        input: &PluginArtifactInput,
        metadata: &mut PluginMetadata,
    ) -> Result<PluginResourceLimits, PluginInstallError> {
        if input.identifier().len() > 255
            || !input.identifier().chars().all(|character| {
                character.is_ascii_alphanumeric() || matches!(character, '_' | '-')
            })
        {
            return Err(invalid_install_input());
        }
        if input.version().is_empty()
            || input.version().len() > 64
            || matches!(input.version(), "." | "..")
            || input.version().contains(['/', '\\', '\0'])
        {
            return Err(invalid_install_input());
        }
        if metadata.runtime != "extism" {
            return Err(invalid_install_input());
        }
        let display_name = if metadata.name().is_empty() {
            input.identifier()
        } else {
            metadata.name()
        };
        if display_name.trim().is_empty() || display_name.len() > 255 {
            return Err(invalid_install_input());
        }
        if metadata
            .description
            .as_ref()
            .is_some_and(|description| description.len() > 2_000)
        {
            return Err(invalid_install_input());
        }

        let shape = validate_wasm_shape(input.bytes(), false);
        if !shape.is_ok() {
            tracing::warn!(rejection = ?shape.rejection_kind(), "plugin prevalidation rejected WASM shape");
            return Err(invalid_install_input());
        }
        let actual_exports = extract_wasm_exports(input.bytes()).map_err(|error| {
            tracing::warn!(error = %error, "plugin prevalidation could not inspect exports");
            invalid_install_input()
        })?;

        if metadata.capabilities.len() > 1024 * 1024 {
            return Err(invalid_install_input());
        }
        let metadata_capabilities = serde_json::from_str::<Vec<String>>(&metadata.capabilities)
            .map_err(|_| invalid_install_input())?;
        let mut unique_metadata_capabilities = BTreeSet::new();
        for capability in &metadata_capabilities {
            if capability.is_empty()
                || capability.trim() != capability
                || !unique_metadata_capabilities.insert(capability.clone())
            {
                return Err(invalid_install_input());
            }
        }

        let resource_limits = parse_resource_limits(&metadata.resource_limits)?;

        if metadata.manifest.is_none() {
            metadata.manifest = Some(build_v1_manifest(&actual_exports, &metadata_capabilities));
        }
        let manifest = metadata
            .manifest
            .as_deref()
            .ok_or_else(invalid_install_input)?;
        if manifest.len() > 1024 * 1024 {
            return Err(invalid_install_input());
        }

        // Capability availability is resolved exclusively from this local
        // HiveGUI database. Import validation never requests HiveWeb and never
        // mutates the capability catalog as a side effect.
        let available_capabilities = sqlx::query_scalar::<_, String>(
            "SELECT name FROM capabilities ORDER BY name",
        )
        .fetch_all(&self.inner.pool)
        .await
        .map_err(|error| {
            tracing::error!(error = %error, "load local capabilities for plugin prevalidation");
            io_err(std::io::Error::other("load local capabilities"))
        })?
        .into_iter()
        .collect::<BTreeSet<_>>();

        if let Err(issues) = validate_manifest(manifest, &available_capabilities) {
            tracing::warn!(issues = ?issues, "plugin prevalidation rejected manifest");
            return Err(invalid_install_input());
        }

        let manifest_capabilities = manifest_required_capabilities(Some(manifest));
        let manifest_capability_set = manifest_capabilities
            .iter()
            .cloned()
            .collect::<BTreeSet<_>>();
        if manifest_capabilities.len() != manifest_capability_set.len()
            || manifest_capability_set != unique_metadata_capabilities
        {
            return Err(invalid_install_input());
        }

        let declared_exports = manifest_exports(Some(manifest));
        let declared_export_set = declared_exports.iter().cloned().collect::<BTreeSet<_>>();
        let actual_export_set = actual_exports.iter().cloned().collect::<BTreeSet<_>>();
        if declared_exports.len() != declared_export_set.len()
            || declared_export_set != actual_export_set
        {
            return Err(invalid_install_input());
        }

        Ok(resource_limits)
    }

    /// Resolve one persisted artifact key under the controlled root without
    /// accepting absolute paths, traversal, prefixes, or platform separators.
    fn artifact_path_from_key(&self, artifact_key: &str) -> Result<PathBuf, PluginInstallError> {
        if artifact_key.is_empty() || artifact_key.contains(['\\', '\0']) {
            return Err(unsafe_artifact_error());
        }
        let relative = Path::new(artifact_key);
        if relative.is_absolute()
            || relative
                .components()
                .any(|component| !matches!(component, Component::Normal(_)))
        {
            return Err(unsafe_artifact_error());
        }
        Ok(self.inner.root.join(relative))
    }

    /// Write the artifact bytes to a staging file using an exclusive
    /// create, then flush + fsync. The caller later re-reads the bytes
    /// to verify the on-disk identity.
    async fn write_staging(
        &self,
        staging_name: &str,
        bytes: &[u8],
    ) -> Result<String, PluginInstallError> {
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
        let metadata = file.metadata().map_err(io_err)?;
        if !metadata.is_file() || metadata.len() != bytes.len() as u64 {
            return Err(unsafe_artifact_error());
        }
        #[cfg(unix)]
        {
            use std::os::unix::fs::MetadataExt;
            if metadata.nlink() != 1 {
                return Err(unsafe_artifact_error());
            }
        }
        Ok(file_identity(&metadata))
    }

    /// Publish an artifact at its ledger-owned relative key using an
    /// exclusive no-replace create. The staging name is removed only after
    /// the final file has been flushed and fsynced.
    async fn publish_artifact(
        &self,
        artifact_key: &str,
        staging_name: &str,
    ) -> Result<String, PluginInstallError> {
        let artifact_path = self.artifact_path_from_key(artifact_key)?;
        let artifact_dir = artifact_path.parent().ok_or_else(unsafe_artifact_error)?;
        std::fs::create_dir_all(artifact_dir).map_err(io_err)?;

        let staging_dir = self.inner.root.join(".staging");
        let staging_path = staging_dir.join(staging_name);
        match std::fs::hard_link(&staging_path, &artifact_path) {
            Ok(()) => {}
            Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => {
                return Err(PluginInstallError {
                    kind: PluginInstallErrorKind::NoReplace,
                    references: Vec::new(),
                });
            }
            Err(error) => return Err(io_err(error)),
        }
        // `hard_link` is the same-filesystem no-replace publish primitive:
        // both names temporarily identify the already-fsynced staging inode.
        // The final parent is durable before the staging name is removed;
        // after the unlink the published file has link-count one.
        fsync_dir(artifact_dir);
        std::fs::remove_file(&staging_path).map_err(io_err)?;
        fsync_dir(&staging_dir);
        let file = open_nofollow(&artifact_path).map_err(io_err)?;
        let metadata = file.metadata().map_err(io_err)?;
        if !metadata.is_file() {
            return Err(unsafe_artifact_error());
        }
        #[cfg(unix)]
        {
            use std::os::unix::fs::MetadataExt;
            if metadata.nlink() != 1 {
                return Err(unsafe_artifact_error());
            }
        }
        Ok(file_identity(&metadata))
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
        let row: Option<(String, String, String)> =
            sqlx::query_as("SELECT identifier, version, s3_key FROM plugins WHERE id = ?")
                .bind(id)
                .fetch_optional(&self.inner.pool)
                .await
                .map_err(|_| PluginInstallError {
                    kind: PluginInstallErrorKind::Io,
                    references: Vec::new(),
                })?;
        let (identifier, version, artifact_key) = match row {
            Some(r) => r,
            None => return Ok(None),
        };
        let bytes = match self.read_verified_artifact_key(&artifact_key) {
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

    /// Replay interrupted operations before opening the Plugin Store.
    /// Ownership ambiguity is durable and fail-closed: the exact operation
    /// becomes `conflict`, every observed object remains byte-for-byte in
    /// place, and the caller receives an error.
    pub async fn recover_interrupted_operations(&self) -> Result<usize, PluginInstallError> {
        let rows = sqlx::query_as::<_, RecoveryOperationRow>(
            "SELECT operation_id, state, staging_name, staging_identity, new_identity, \
                    new_s3_key, new_sha256, new_size \
                 FROM plugin_artifact_operations \
                 WHERE state IN ('prepared','staged','published','referenced') \
                 ORDER BY created_at, operation_id",
        )
        .fetch_all(&self.inner.pool)
        .await
        .map_err(|_| PluginInstallError {
            kind: PluginInstallErrorKind::Io,
            references: Vec::new(),
        })?;

        let mut recovered = 0usize;
        let ledger = PluginArtifactLedger::new(self.inner.pool.clone());
        for row in rows {
            let staging_dir = self.inner.root.join(".staging");
            let staging_path = staging_dir.join(&row.staging_name);
            let staging_exists = path_entry_exists(&staging_path);
            match row.state.as_str() {
                "prepared" => {
                    if staging_exists {
                        self.mark_operation_conflict(&row.operation_id, "prepared")
                            .await?;
                        return Err(unsafe_artifact_error());
                    }
                    self.mark_operation_done(&row.operation_id, "prepared")
                        .await?;
                    recovered += 1;
                }
                "staged" => {
                    let final_exists = row
                        .new_s3_key
                        .as_deref()
                        .and_then(|key| self.artifact_path_from_key(key).ok())
                        .is_some_and(|path| path_entry_exists(&path));
                    if staging_exists && final_exists {
                        self.mark_operation_conflict(&row.operation_id, "staged")
                            .await?;
                        return Err(unsafe_artifact_error());
                    }
                    if staging_exists {
                        if !artifact_file_matches(
                            &staging_path,
                            row.new_sha256.as_deref(),
                            row.new_size,
                            row.staging_identity.as_deref(),
                        ) {
                            self.mark_operation_conflict(&row.operation_id, "staged")
                                .await?;
                            return Err(unsafe_artifact_error());
                        }
                        std::fs::remove_file(&staging_path).map_err(io_err)?;
                        fsync_dir(&staging_dir);
                    } else if final_exists {
                        // A final-only staged shape needs an identity-bound
                        // publish replay. Until all ownership fields verify,
                        // retain the object and block rather than adopting it.
                        self.mark_operation_conflict(&row.operation_id, "staged")
                            .await?;
                        return Err(unsafe_artifact_error());
                    }
                    self.mark_operation_done(&row.operation_id, "staged")
                        .await?;
                    recovered += 1;
                }
                "published" => {
                    // Published accepts exactly one physical shape: staging is
                    // absent and final is the ledger-owned object. Double
                    // presence is always ambiguous even when bytes match.
                    let final_path = row
                        .new_s3_key
                        .as_deref()
                        .and_then(|key| self.artifact_path_from_key(key).ok());
                    let final_matches = final_path.as_deref().is_some_and(|path| {
                        artifact_file_matches(
                            path,
                            row.new_sha256.as_deref(),
                            row.new_size,
                            row.new_identity.as_deref(),
                        )
                    });
                    if staging_exists || !final_matches {
                        self.mark_operation_conflict(&row.operation_id, "published")
                            .await?;
                        return Err(unsafe_artifact_error());
                    }

                    let operation_id = match uuid::Uuid::parse_str(&row.operation_id) {
                        Ok(operation_id) => operation_id,
                        Err(_) => {
                            self.mark_operation_conflict(&row.operation_id, "published")
                                .await?;
                            return Err(unsafe_artifact_error());
                        }
                    };
                    let operation = match ledger.get(operation_id).await {
                        Ok(Some(operation)) => operation,
                        Ok(None) | Err(_) => {
                            self.mark_operation_conflict(&row.operation_id, "published")
                                .await?;
                            return Err(unsafe_artifact_error());
                        }
                    };

                    // Replaying a published operation never recreates user
                    // intent or redoes a live replace CAS. If no atomic
                    // referenced fact exists, the operation-owned new object
                    // enters GC and the operation completes transactionally.
                    if operation.kind() == OperationKind::Replace
                        && self.plugin_references_operation_new(&operation).await?
                    {
                        self.mark_operation_conflict(&row.operation_id, "published")
                            .await?;
                        return Err(unsafe_artifact_error());
                    }
                    ledger
                        .finish_with_gc(
                            operation_id,
                            OperationState::Published,
                            OperationGcTarget::PublishedNew,
                        )
                        .await
                        .map_err(ledger_install_error)?;
                    recovered += 1;
                }
                "referenced" => {
                    let operation_id = uuid::Uuid::parse_str(&row.operation_id)
                        .map_err(|_| unsafe_artifact_error())?;
                    let operation = ledger
                        .get(operation_id)
                        .await
                        .map_err(ledger_install_error)?
                        .ok_or_else(unsafe_artifact_error)?;
                    match operation.kind() {
                        OperationKind::Create => {
                            ledger
                                .transition(
                                    operation_id,
                                    OperationState::Referenced,
                                    OperationTransition::Done,
                                )
                                .await
                                .map_err(ledger_install_error)?;
                        }
                        OperationKind::Replace => {
                            ledger
                                .finish_with_gc(
                                    operation_id,
                                    OperationState::Referenced,
                                    OperationGcTarget::ExpectedOld,
                                )
                                .await
                                .map_err(ledger_install_error)?;
                        }
                    }
                    recovered += 1;
                }
                _ => unreachable!("query restricts operation states"),
            }
        }
        Ok(recovered)
    }

    async fn plugin_references_operation_new(
        &self,
        operation: &crate::datasource::entity_store::PluginArtifactOperation,
    ) -> Result<bool, PluginInstallError> {
        let Some(plugin_id) = operation.plugin_id() else {
            return Ok(false);
        };
        sqlx::query_scalar::<_, i64>(
            "SELECT COUNT(*) FROM plugins \
             WHERE id = ? AND identifier = ? AND s3_key = ? AND sha256 = ? \
               AND size_bytes = ? AND deleted_at IS NULL",
        )
        .bind(plugin_id)
        .bind(operation.target_identifier())
        .bind(operation.new_s3_key())
        .bind(operation.new_sha256())
        .bind(operation.new_size_bytes())
        .fetch_one(&self.inner.pool)
        .await
        .map(|count| count == 1)
        .map_err(|_| io_err(std::io::Error::other("query plugin reference")))
    }

    async fn mark_operation_conflict(
        &self,
        operation_id: &str,
        expected_state: &str,
    ) -> Result<(), PluginInstallError> {
        let updated = sqlx::query(
            "UPDATE plugin_artifact_operations \
             SET state = 'conflict', updated_at = ? \
             WHERE operation_id = ? AND state = ?",
        )
        .bind(chrono::Utc::now().to_rfc3339())
        .bind(operation_id)
        .bind(expected_state)
        .execute(&self.inner.pool)
        .await
        .map_err(|_| io_err(std::io::Error::other("mark operation conflict")))?;
        if updated.rows_affected() != 1 {
            return Err(unsafe_artifact_error());
        }
        Ok(())
    }

    async fn mark_operation_done(
        &self,
        operation_id: &str,
        expected_state: &str,
    ) -> Result<(), PluginInstallError> {
        let updated = sqlx::query(
            "UPDATE plugin_artifact_operations SET state = 'done', updated_at = ? \
             WHERE operation_id = ? AND state = ?",
        )
        .bind(chrono::Utc::now().to_rfc3339())
        .bind(operation_id)
        .bind(expected_state)
        .execute(&self.inner.pool)
        .await
        .map_err(|_| io_err(std::io::Error::other("mark operation done")))?;
        if updated.rows_affected() != 1 {
            return Err(unsafe_artifact_error());
        }
        Ok(())
    }

    /// Soft-delete a plugin and garbage-collect its on-disk artifact through
    /// the protected ledger path. Unlike the legacy `remove_dir_all` flow,
    /// this records a `pending` GC row first (crash-recoverable) and only
    /// then deletes the file, so a crash between the soft-delete and the
    /// unlink leaves a `pending` row that startup replay drains.
    ///
    /// T079 ③: the artifact is removed via [`PluginStore::drain_pending_gc`],
    /// which resolves the `artifact_key` with strict path-traversal checks
    /// and never deletes speculatively.
    pub async fn delete(&self, id: i64) -> Result<(), PluginInstallError> {
        let artifact_key: Option<String> =
            sqlx::query_scalar("SELECT s3_key FROM plugins WHERE id = ?")
                .bind(id)
                .fetch_optional(&self.inner.pool)
                .await
                .map_err(|_| io_err(std::io::Error::other("resolve plugin")))?;
        let artifact_key = artifact_key.ok_or_else(|| PluginInstallError {
            kind: PluginInstallErrorKind::Io,
            references: Vec::new(),
        })?;

        self.soft_delete(id).await?;

        // Prevent an idle managed runtime instance from keeping the artifact
        // alive indefinitely. Active calls remain tracked and make the GC
        // scan retryable; invalidation also prevents an in-flight checkout
        // from being reinserted after it returns.
        let _ = crate::runtime::plugin_executor::invalidate_artifact_instances(
            &self.inner.root,
            id,
            &artifact_key,
        );

        Plugin::register_gc_artifact(&self.inner.pool, artifact_key, "deleted".to_string())
            .await
            .map_err(|_| io_err(std::io::Error::other("register gc")))?;

        self.drain_pending_gc().await?;
        Ok(())
    }

    /// Drain protected GC entries in stable key order. Live metadata, runtime
    /// leases, and active runtime instances remain retryable. Incomplete or
    /// mismatched ownership stays blocked and competitor bytes are preserved.
    pub async fn drain_pending_gc(&self) -> Result<usize, PluginInstallError> {
        let rows = sqlx::query_as::<_, GcLedgerRow>(
            "SELECT g.artifact_key, g.expected_sha256, g.expected_size_bytes, \
                    g.expected_identity, g.source_operation_id, g.state, \
                    g.attempts, g.last_error, CAST(o.plugin_id AS INTEGER) AS plugin_id \
             FROM plugin_artifact_gc g \
             LEFT JOIN plugin_artifact_operations o \
               ON o.operation_id = g.source_operation_id \
             WHERE g.state IN ('pending','blocked') \
             ORDER BY g.artifact_key",
        )
        .fetch_all(&self.inner.pool)
        .await
        .map_err(|_| io_err(std::io::Error::other("query protected gc")))?;

        let mut drained = 0usize;
        for row in rows {
            let key = row.artifact_key.as_str();
            tracing::debug!(
                artifact_key = key,
                previous_state = %row.state,
                attempts = row.attempts,
                "scan protected plugin artifact GC row"
            );
            // Identity/ownership conflicts require operator intervention.
            // Never let a later scan adopt a path merely because its current
            // bytes happen to match the originally recorded tuple again.
            if gc_error_is_terminal(row.last_error.as_deref()) {
                continue;
            }
            let Some(path) = self.resolve_gc_artifact_path(key) else {
                self.record_gc_outcome(key, "blocked", "unsafe_artifact_key")
                    .await?;
                continue;
            };
            let path_exists = path_entry_exists(&path);

            // The marker means an earlier worker had already passed the last
            // ownership check and may have unlinked the owned inode. If a
            // name is present on replay, it is a reappearance and therefore
            // competitor-owned even when its tuple happens to match.
            if row.last_error.as_deref() == Some(GC_UNLINK_MARKER) && path_exists {
                self.record_gc_outcome(key, "blocked", "identity_reappeared_after_unlink_marker")
                    .await?;
                continue;
            }

            let live_references: i64 = sqlx::query_scalar(
                "SELECT COUNT(*) FROM plugins WHERE s3_key = ? AND deleted_at IS NULL",
            )
            .bind(key)
            .fetch_one(&self.inner.pool)
            .await
            .map_err(|_| io_err(std::io::Error::other("query live plugin references")))?;
            if live_references > 0 {
                if row.last_error.as_deref() != Some(GC_UNLINK_MARKER) {
                    self.record_gc_outcome(key, "pending", "metadata_referenced")
                        .await?;
                }
                continue;
            }

            let plugin_id = row.plugin_id.filter(|plugin_id| *plugin_id > 0);
            if plugin_id.is_some_and(|plugin_id| self.inner.lease_tracker.has_active(plugin_id)) {
                if row.last_error.as_deref() != Some(GC_UNLINK_MARKER) {
                    self.record_gc_outcome(key, "pending", "runtime_lease_active")
                        .await?;
                }
                continue;
            }

            if let Some(plugin_id) = plugin_id {
                let active_after_invalidation =
                    crate::runtime::plugin_executor::invalidate_artifact_instances(
                        &self.inner.root,
                        plugin_id,
                        key,
                    );
                if active_after_invalidation
                    || crate::runtime::plugin_executor::artifact_has_runtime_references(
                        &self.inner.root,
                        plugin_id,
                        key,
                    )
                {
                    if row.last_error.as_deref() != Some(GC_UNLINK_MARKER) {
                        self.record_gc_outcome(key, "pending", "runtime_reference_active")
                            .await?;
                    }
                    continue;
                }
            }

            let ownership = match (
                row.expected_sha256.as_deref(),
                row.expected_size_bytes,
                row.expected_identity.as_deref(),
                row.source_operation_id.as_deref(),
                plugin_id,
            ) {
                (Some(sha256), Some(size), Some(identity), Some(_), Some(_))
                    if size > 0 && sha256.len() == 64 && !identity.is_empty() =>
                {
                    (sha256, size, identity)
                }
                _ => {
                    self.record_gc_outcome(key, "blocked", "ownership_incomplete")
                        .await?;
                    continue;
                }
            };

            if !path_exists {
                if row.last_error.as_deref() == Some(GC_UNLINK_MARKER) {
                    if let Some(parent) = path.parent() {
                        fsync_dir(parent);
                    }
                    if self.remove_gc_row(&row).await? {
                        drained += 1;
                    }
                } else {
                    self.record_gc_outcome(key, "blocked", "owned_artifact_missing")
                        .await?;
                }
                continue;
            }

            if !artifact_file_matches(
                &path,
                Some(ownership.0),
                Some(ownership.1),
                Some(ownership.2),
            ) {
                self.record_gc_outcome(key, "blocked", "identity_or_content_mismatch")
                    .await?;
                continue;
            }

            // Persist an unlink marker after complete verification. If the
            // process stops after unlink+fsync but before ledger deletion,
            // absence is converged only when this marker is present.
            if !self.mark_gc_unlinking(&row).await? {
                continue;
            }
            if !artifact_file_matches(
                &path,
                Some(ownership.0),
                Some(ownership.1),
                Some(ownership.2),
            ) {
                self.record_gc_outcome(key, "blocked", "identity_changed_before_unlink")
                    .await?;
                continue;
            }
            match std::fs::remove_file(&path) {
                Ok(()) => {}
                Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
                Err(_) => {
                    self.record_gc_outcome(key, "blocked", "unlink_failed")
                        .await?;
                    continue;
                }
            }
            self.prune_empty_ancestors(&path);
            if self.remove_gc_row(&row).await? {
                drained += 1;
            }
        }
        Ok(drained)
    }

    /// Resolve a GC `artifact_key` to the on-disk path it owns, with strict
    /// path-traversal protection. A key has four normal relative components
    /// ending in `plugin.wasm`; its third immutable component is opaque.
    fn resolve_gc_artifact_path(&self, artifact_key: &str) -> Option<PathBuf> {
        let segments: Vec<&str> = artifact_key.split('/').collect();
        if segments.len() != 4 || segments[3] != "plugin.wasm" {
            return None;
        }
        if segments[..3]
            .iter()
            .any(|segment| segment.is_empty() || matches!(*segment, "." | ".."))
        {
            return None;
        }
        let mut parent = self.inner.root.clone();
        for segment in &segments[..3] {
            parent.push(segment);
            match std::fs::symlink_metadata(&parent) {
                Ok(metadata)
                    if !metadata.file_type().is_symlink() && metadata.file_type().is_dir() => {}
                Ok(_) => return None,
                Err(error) if error.kind() == std::io::ErrorKind::NotFound => break,
                Err(_) => return None,
            }
        }
        self.artifact_path_from_key(artifact_key).ok()
    }

    /// Remove now-empty ancestor directories of a deleted artifact file,
    /// walking upward from `{id}` → `{version}` → `{identifier}`. The store
    /// root is never removed.
    fn prune_empty_ancestors(&self, path: &Path) {
        // path = root/{identifier}/{version}/{id}/plugin.wasm. The file has
        // already been unlinked by the caller; fsync its directory to make the
        // unlink durable, then prune now-empty ancestor dirs upward, fsyncing
        // each parent after its child is removed (T079 persistent-GC contract:
        // delete + fsync parent directory).
        let Some(id_dir) = path.parent() else {
            return;
        };
        fsync_dir(id_dir);

        let Some(version_dir) = id_dir.parent() else {
            return;
        };
        if std::fs::remove_dir(id_dir).is_ok() {
            fsync_dir(version_dir);
        }

        let Some(identifier_dir) = version_dir.parent() else {
            return;
        };
        if std::fs::remove_dir(version_dir).is_ok() {
            fsync_dir(identifier_dir);
        }

        if identifier_dir != self.inner.root {
            let _ = std::fs::remove_dir(identifier_dir);
        }
    }

    /// Persist the pre-unlink marker only if the complete ownership snapshot
    /// selected by this worker is still current.
    async fn mark_gc_unlinking(&self, row: &GcLedgerRow) -> Result<bool, PluginInstallError> {
        let (Some(sha256), Some(size), Some(identity), Some(source_operation_id)) = (
            row.expected_sha256.as_deref(),
            row.expected_size_bytes,
            row.expected_identity.as_deref(),
            row.source_operation_id.as_deref(),
        ) else {
            return Ok(false);
        };
        let now = chrono::Utc::now().to_rfc3339();
        let result = sqlx::query(
            "UPDATE plugin_artifact_gc \
             SET state = 'blocked', attempts = attempts + 1, last_error = ?, \
                 updated_at = ?, last_attempt_at = unixepoch() \
             WHERE artifact_key = ? AND expected_sha256 = ? \
               AND expected_size_bytes = ? AND expected_identity = ? \
               AND source_operation_id = ? AND state = ? \
               AND ((last_error IS NULL AND ? IS NULL) OR last_error = ?)",
        )
        .bind(GC_UNLINK_MARKER)
        .bind(now)
        .bind(&row.artifact_key)
        .bind(sha256)
        .bind(size)
        .bind(identity)
        .bind(source_operation_id)
        .bind(&row.state)
        .bind(row.last_error.as_deref())
        .bind(row.last_error.as_deref())
        .execute(&self.inner.pool)
        .await
        .map_err(|_| io_err(std::io::Error::other("mark gc unlinking")))?;
        Ok(result.rows_affected() == 1)
    }

    /// Remove a drained GC ledger row only while its complete ownership tuple,
    /// source operation, and pre-unlink marker still match this scan.
    async fn remove_gc_row(&self, row: &GcLedgerRow) -> Result<bool, PluginInstallError> {
        let (Some(sha256), Some(size), Some(identity), Some(source_operation_id)) = (
            row.expected_sha256.as_deref(),
            row.expected_size_bytes,
            row.expected_identity.as_deref(),
            row.source_operation_id.as_deref(),
        ) else {
            return Ok(false);
        };
        let result = sqlx::query(
            "DELETE FROM plugin_artifact_gc \
             WHERE artifact_key = ? AND expected_sha256 = ? \
               AND expected_size_bytes = ? AND expected_identity = ? \
               AND source_operation_id = ? AND state = 'blocked' \
               AND last_error = ?",
        )
        .bind(&row.artifact_key)
        .bind(sha256)
        .bind(size)
        .bind(identity)
        .bind(source_operation_id)
        .bind(GC_UNLINK_MARKER)
        .execute(&self.inner.pool)
        .await
        .map_err(|_| io_err(std::io::Error::other("remove gc row")))?;
        Ok(result.rows_affected() == 1)
    }

    /// Persist one classified GC attempt without changing its ownership
    /// tuple. `pending` is reserved for retryable references; ownership or
    /// identity uncertainty is `blocked`.
    async fn record_gc_outcome(
        &self,
        key: &str,
        state: &str,
        last_error: &str,
    ) -> Result<(), PluginInstallError> {
        if !matches!(state, "pending" | "blocked") {
            return Err(invalid_install_input());
        }
        sqlx::query(
            "UPDATE plugin_artifact_gc \
             SET state = ?, attempts = attempts + 1, last_error = ?, \
                 updated_at = ?, last_attempt_at = unixepoch() \
             WHERE artifact_key = ?",
        )
        .bind(state)
        .bind(last_error)
        .bind(chrono::Utc::now().to_rfc3339())
        .bind(key)
        .execute(&self.inner.pool)
        .await
        .map_err(|_| io_err(std::io::Error::other("record gc outcome")))?;
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
        self.inner.lease_tracker.acquire(plugin_id);
        Ok(PluginLease {
            plugin_id,
            session_id,
            state,
            _guard: Arc::new(PluginLeaseGuard {
                tracker: self.inner.lease_tracker.clone(),
                plugin_id,
            }),
        })
    }

    /// Resolve the on-disk path for `(identifier, version, id)` relative to
    /// the store root without following symlinks. Returns the resolved
    /// path only if every directory component is a real directory (not a
    /// symlink) and the final component is a link-count-1 regular file.
    /// This is the T077 no-follow / no-replace filesystem safety boundary.
    pub fn resolve_artifact_path(
        &self,
        identifier: &str,
        version: &str,
        id: i64,
    ) -> Result<PathBuf, PluginInstallError> {
        let unsafe_err = || PluginInstallError {
            kind: PluginInstallErrorKind::UnsafeArtifact,
            references: Vec::new(),
        };

        let dir = self
            .inner
            .root
            .join(Plugin::artifact_dir(identifier, version, id));
        let dir_md = std::fs::symlink_metadata(&dir).map_err(|_| unsafe_err())?;
        if dir_md.file_type().is_symlink() || !dir_md.file_type().is_dir() {
            return Err(unsafe_err());
        }

        let path = dir.join("plugin.wasm");
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

    /// Read a persisted opaque artifact key through a no-follow descriptor.
    /// New create operations use an operation UUID in the third key segment;
    /// legacy numeric-id keys remain valid through the same boundary.
    pub fn read_verified_artifact_key(
        &self,
        artifact_key: &str,
    ) -> Result<Vec<u8>, PluginInstallError> {
        let path = self.artifact_path_from_key(artifact_key)?;
        let file = open_nofollow(&path).map_err(|_| unsafe_artifact_error())?;
        let metadata = file.metadata().map_err(|_| unsafe_artifact_error())?;
        if !metadata.is_file() || metadata.len() == 0 {
            return Err(unsafe_artifact_error());
        }
        #[cfg(unix)]
        {
            use std::os::unix::fs::MetadataExt;
            if metadata.nlink() != 1 {
                return Err(unsafe_artifact_error());
            }
        }
        use std::io::Read;
        let mut bytes = Vec::with_capacity(metadata.len() as usize);
        file.take(MAX_ARTIFACT_BYTES)
            .read_to_end(&mut bytes)
            .map_err(|_| unsafe_artifact_error())?;
        Ok(bytes)
    }

    /// Read artifact bytes through a single no-follow descriptor.
    ///
    /// T079: unlike [`PluginStore::resolve_artifact_path`] followed by
    /// `std::fs::read` (which validates a path and then re-opens it, leaving
    /// a replace window between check and use), this opens the final
    /// component with `O_NOFOLLOW` and validates the *opened descriptor*
    /// via `fstat` (regular file, link count 1, non-empty) before reading a
    /// single byte. The artifact directory
    /// (`{identifier}/{version}/{id}`) is still checked with
    /// `symlink_metadata`: it is store-managed and must never be a symlink.
    pub fn read_verified_artifact(
        &self,
        identifier: &str,
        version: &str,
        id: i64,
    ) -> Result<Vec<u8>, PluginInstallError> {
        let unsafe_err = || PluginInstallError {
            kind: PluginInstallErrorKind::UnsafeArtifact,
            references: Vec::new(),
        };

        let dir = self
            .inner
            .root
            .join(Plugin::artifact_dir(identifier, version, id));
        let dir_md = std::fs::symlink_metadata(&dir).map_err(|_| unsafe_err())?;
        if dir_md.file_type().is_symlink() || !dir_md.file_type().is_dir() {
            return Err(unsafe_err());
        }

        let path = dir.join("plugin.wasm");

        // Open the final component with O_NOFOLLOW: a symlink in the last
        // path segment is rejected at open time rather than "checked, then
        // re-opened".
        let file = open_nofollow(&path).map_err(|_| unsafe_err())?;

        // Validate the *opened descriptor* (fstat), so the bytes we read are
        // guaranteed to come from exactly one regular, non-empty file.
        let md = file.metadata().map_err(|_| unsafe_err())?;
        if !md.is_file() || md.len() == 0 {
            return Err(unsafe_err());
        }
        #[cfg(unix)]
        {
            use std::os::unix::fs::MetadataExt;
            if md.nlink() > 1 {
                return Err(unsafe_err());
            }
        }

        use std::io::Read;
        let mut bytes = Vec::with_capacity(md.len() as usize);
        file.take(MAX_ARTIFACT_BYTES)
            .read_to_end(&mut bytes)
            .map_err(|_| unsafe_err())?;
        Ok(bytes)
    }
}

/// Upper bound on a single artifact read, as a DoS guard against a
/// store-managed file that grew unexpectedly after publish (128 MiB).
const MAX_ARTIFACT_BYTES: u64 = 128 * 1024 * 1024;

/// Durable marker proving the ledger had verified its complete ownership
/// tuple immediately before attempting unlink.
const GC_UNLINK_MARKER: &str = "unlinking_verified_identity";

/// Outcomes that require explicit operator repair. Retrying these rows must
/// never adopt a later filesystem entry, even if it happens to match the old
/// digest/size/identity tuple.
fn gc_error_is_terminal(error: Option<&str>) -> bool {
    matches!(
        error,
        Some(
            "unsafe_artifact_key"
                | "ownership_unproven"
                | "ownership_incomplete"
                | "owned_artifact_missing"
                | "identity_or_content_mismatch"
                | "identity_changed_before_unlink"
                | "identity_reappeared_after_unlink_marker"
        )
    )
}

/// Open a path with `O_NOFOLLOW` so a symlink in the final component is
/// rejected at open time. Non-Unix platforms fall back to a plain read
/// open (the T077 no-follow boundary is a Unix-only concern).
#[cfg(unix)]
fn open_nofollow(path: &Path) -> std::io::Result<std::fs::File> {
    use std::os::unix::fs::OpenOptionsExt;
    std::fs::OpenOptions::new()
        .read(true)
        .custom_flags(libc::O_NOFOLLOW)
        .open(path)
}

#[cfg(not(unix))]
fn open_nofollow(path: &Path) -> std::io::Result<std::fs::File> {
    std::fs::File::open(path)
}

/// SHA-256 helper used by tests + callers that want to compare
/// fingerprints against a hex digest computed externally.
pub fn sha256_hex(bytes: &[u8]) -> String {
    let mut hasher = Sha256::new();
    hasher.update(bytes);
    format!("{:x}", hasher.finalize())
}

fn path_entry_exists(path: &Path) -> bool {
    match std::fs::symlink_metadata(path) {
        Ok(_) => true,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => false,
        // An entry whose ownership cannot be inspected is conservatively
        // treated as present, forcing the caller into durable conflict.
        Err(_) => true,
    }
}

#[cfg(unix)]
fn file_identity(metadata: &std::fs::Metadata) -> String {
    use std::os::unix::fs::MetadataExt;
    format!(
        "unix-v1:{}:{}:{}:{}",
        metadata.dev(),
        metadata.ino(),
        metadata.ctime(),
        metadata.ctime_nsec()
    )
}

#[cfg(not(unix))]
fn file_identity(metadata: &std::fs::Metadata) -> String {
    use std::time::UNIX_EPOCH;
    let modified = metadata
        .modified()
        .ok()
        .and_then(|value| value.duration_since(UNIX_EPOCH).ok())
        .map_or(0, |duration| duration.as_nanos());
    format!("portable-v1:{}:{modified}", metadata.len())
}

fn artifact_file_matches(
    path: &Path,
    expected_sha256: Option<&str>,
    expected_size: Option<i64>,
    expected_identity: Option<&str>,
) -> bool {
    let (Some(expected_sha256), Some(expected_size), Some(expected_identity)) =
        (expected_sha256, expected_size, expected_identity)
    else {
        return false;
    };
    if expected_size <= 0 {
        return false;
    }
    let Ok(file) = open_nofollow(path) else {
        return false;
    };
    let Ok(metadata) = file.metadata() else {
        return false;
    };
    if !metadata.is_file() || metadata.len() != expected_size as u64 {
        return false;
    }
    if file_identity(&metadata) != expected_identity {
        return false;
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::MetadataExt;
        if metadata.nlink() != 1 {
            return false;
        }
    }
    use std::io::Read;
    let mut bytes = Vec::with_capacity(metadata.len() as usize);
    if file
        .take(MAX_ARTIFACT_BYTES)
        .read_to_end(&mut bytes)
        .is_err()
    {
        return false;
    }
    bytes.len() == expected_size as usize && sha256_hex(&bytes) == expected_sha256
}

fn parse_resource_limits(value: &str) -> Result<PluginResourceLimits, PluginInstallError> {
    if value.len() > 1024 * 1024 {
        return Err(invalid_install_input());
    }
    let limits =
        serde_json::from_str::<PluginResourceLimits>(value).map_err(|_| invalid_install_input())?;
    let within = |value: Option<i64>, minimum: i64, maximum: i64| {
        value.is_none_or(|value| (minimum..=maximum).contains(&value))
    };
    if !within(limits.timeout_ms, 1, 120_000)
        || !within(limits.memory_limit_mb, 1, 512)
        || !within(limits.output_limit_bytes, 1, 52_428_800)
    {
        return Err(invalid_install_input());
    }
    Ok(limits)
}

fn invalid_install_input() -> PluginInstallError {
    PluginInstallError {
        kind: PluginInstallErrorKind::InvalidInput,
        references: Vec::new(),
    }
}

fn unsafe_artifact_error() -> PluginInstallError {
    PluginInstallError {
        kind: PluginInstallErrorKind::UnsafeArtifact,
        references: Vec::new(),
    }
}

fn ledger_install_error(error: PluginLedgerError) -> PluginInstallError {
    let kind = match &error {
        PluginLedgerError::InvalidInput { .. } => PluginInstallErrorKind::InvalidInput,
        PluginLedgerError::KindMismatch { .. } | PluginLedgerError::StateConflict { .. } => {
            PluginInstallErrorKind::CasConflict
        }
        PluginLedgerError::NotFound { .. }
        | PluginLedgerError::CorruptLedger(_)
        | PluginLedgerError::Backend(_) => PluginInstallErrorKind::Io,
    };
    tracing::error!(error = %error, "plugin artifact ledger operation failed");
    PluginInstallError {
        kind,
        references: Vec::new(),
    }
}

/// Map an `std::io::Error` into a [`PluginInstallError`] with the
/// `Io` reason code.
fn io_err(_error: std::io::Error) -> PluginInstallError {
    PluginInstallError {
        kind: PluginInstallErrorKind::Io,
        references: Vec::new(),
    }
}

/// fsync a directory so a completed unlink/rmdir is durable across a crash
/// (best-effort; on platforms where directories cannot be opened this is a
/// no-op). Used by the persistent-GC path after removing an artifact file.
#[cfg(unix)]
fn fsync_dir(path: &Path) {
    if let Ok(dir) = std::fs::File::open(path) {
        let _ = dir.sync_all();
    }
}

#[cfg(not(unix))]
fn fsync_dir(_path: &Path) {}

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
    let path = root
        .join(Plugin::artifact_dir(&identifier, &version, id))
        .join("plugin.wasm");
    let bytes = match std::fs::read(&path) {
        Ok(b) => b,
        Err(_) => return Ok(None),
    };
    Ok(Some((identifier, version, bytes)))
}
