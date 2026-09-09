//! T129/T130 US13 Age-encrypted backup export + restore.
//!
//! The exporter writes a passphrase-encrypted age stream around a portable
//! `tar` package containing an exact `manifest.json`, one JSON file for every
//! user entity type, and every actively owned Plugin WASM. The importer
//! authenticates the complete stream, upgrades formats 1/2/3 into the current
//! entity representation, re-encrypts sensitive values for the target device,
//! and rebuilds derived search state in an isolated restore instance.
//!
//! Safety invariants enforced here:
//!   * Source must be a regular file (no symlink, hardlink, device,
//!     FIFO, or socket).
//!   * Target must not exist when exporting; the caller is
//!     responsible for picking a fresh path.
//!   * The portable archive manifest carries archive schema version 4;
//!     restore live instances separately carry the storage six-tuple
//!     `(schema_version=1, role, UUID, restore_db_id,
//!      database_name="datasources.db", ownership_state="unarmed")`.
//!   * Sensitive values are only ever held inside the streaming
//!     buffers; nothing is ever written to disk in plaintext.
//!   * Portable archives exclude device keys, search internals, Plugin
//!     operation/GC ledgers, and every restore locator/control file.
//!   * T129 creates only an unarmed/no-owner restore instance. T130 freezes the
//!     Store, verifies the safety boundary, arms the instance, and publishes a
//!     durable owner before any live database/Plugin switch.

#![warn(missing_docs)]

use std::{
    collections::{BTreeMap, BTreeSet},
    fs::{self, File, OpenOptions},
    io::{BufReader, Read, Seek, SeekFrom, Write},
    path::{Path, PathBuf},
    sync::{
        Arc, Mutex, OnceLock,
        atomic::{AtomicBool, AtomicU8, Ordering},
    },
};

use age::secrecy::SecretString;
use age::{Decryptor, Encryptor};
use flate2::write::GzEncoder;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use sqlx::{Row as _, TypeInfo as _, ValueRef as _};
use thiserror::Error;
use uuid::Uuid;

use super::store::{Store, StoreMaintenancePhase};

const MANIFEST_FILENAME: &str = "manifest.json";
const DATABASE_FILENAME: &str = "datasources.db";
const PLUGIN_ARCHIVE_PREFIX: &str = "plugins/";
const ARCHIVE_SCHEMA_VERSION: u32 = 4;
const ALLOWED_FORMATS: &[u32] = &[1, 2, 3];
const MAX_AUTHENTICATED_ARCHIVE_BYTES: u64 = 1024 * 1024 * 1024;
const MAX_ENTITY_FILE_BYTES: usize = 64 * 1024 * 1024;
const PORTABLE_FORMAT_NAME: &str = "hivegui-backup";
const ENTITY_TABLES: &[&str] = &[
    "agent_capabilities",
    "agent_executions",
    "agent_skills",
    "agent_tools",
    "agents",
    "capabilities",
    "categories",
    "chat_messages",
    "chat_sessions",
    "data_sources",
    "functions",
    "global_configs",
    "llm_presets",
    "llm_providers",
    "models",
    "plugins",
    "skills",
    "tags",
    "tools",
    "workflow_edges",
    "workflow_nodes",
    "workflows",
];

static FROZEN_DATABASES: OnceLock<Mutex<BTreeSet<PathBuf>>> = OnceLock::new();
static AGE_KDF_LOCK: OnceLock<Mutex<()>> = OnceLock::new();

fn frozen_databases() -> &'static Mutex<BTreeSet<PathBuf>> {
    FROZEN_DATABASES.get_or_init(|| Mutex::new(BTreeSet::new()))
}

fn age_kdf_lock() -> &'static Mutex<()> {
    AGE_KDF_LOCK.get_or_init(|| Mutex::new(()))
}

pub(crate) fn ensure_store_write_open(database_path: &Path) -> anyhow::Result<()> {
    let frozen = frozen_databases()
        .lock()
        .map_err(|_| anyhow::anyhow!("write_gate_closed"))?;
    if frozen.contains(database_path) {
        anyhow::bail!("write_gate_closed");
    }
    Ok(())
}

/// Errors surfaced by the exporter.
#[derive(Debug, Error)]
pub enum ExportError {
    /// Another backup/restore lifecycle owns this exact Store root.
    #[error("maintenance_busy {{ active: {active} }}")]
    MaintenanceBusy {
        /// Stable six-state maintenance phase; never contains a path or secret.
        active: &'static str,
    },
    /// Source database is not a regular file (symlink, hardlink,
    /// device, FIFO, socket, etc.).
    #[error("source is not a regular file")]
    UnsafeSource(String),
    /// Target already exists; the export never overwrites.
    #[error("target already exists: {0}")]
    TargetExists(String),
    /// I/O failure.
    #[error("io error: {0}")]
    Io(String),
    /// Age encryption failure.
    #[error("age encryption error: {0}")]
    Age(String),
    /// The chosen source database does not exist.
    #[error("source not found: {0}")]
    SourceMissing(String),
    /// The archive reached its final name, but durability of the parent
    /// directory could not be proven. Callers must not claim success and must
    /// preserve the archive for startup reconciliation.
    #[error("backup_publish_uncertain: {0}")]
    BackupPublishUncertain(String),
    /// Internal deterministic fault-injection boundary used by the approved
    /// confirmation crash matrix. Production callers never construct it.
    #[error("backup confirmation interrupted at {0}")]
    InjectedCrash(&'static str),
}

/// Errors surfaced by the importer.
#[derive(Debug, Error)]
pub enum ImportError {
    /// Another backup/restore lifecycle owns this exact Store root.
    #[error("maintenance_busy {{ active: {active} }}")]
    MaintenanceBusy {
        /// Stable six-state maintenance phase; never contains a path or secret.
        active: &'static str,
    },
    /// The passphrase did not unlock the age stream.
    #[error("authentication failed")]
    AuthenticationFailed,
    /// The manifest is missing required fields or uses an unknown
    /// schema version.
    #[error("invalid manifest: {0}")]
    InvalidManifest(String),
    /// I/O failure.
    #[error("io error: {0}")]
    Io(String),
    /// Age decryption failure (after authentication succeeded).
    #[error("age decryption error: {0}")]
    Age(String),
    /// The archive contains a non-regular entry (symlink, hardlink,
    /// device, FIFO, socket).
    #[error("archive contains unsafe entry: {0}")]
    UnsafeArchiveEntry(String),
    /// The archive predates the oldest supported portable format.
    #[error("backup_too_old")]
    BackupTooOld,
    /// The archive was produced by a newer unsupported writer.
    #[error("backup_newer_version")]
    BackupNewerVersion,
    /// Internal deterministic fault-injection boundary used by the approved
    /// crash/restart matrix. Production callers never construct this variant.
    #[error("restore interrupted at {0}")]
    InjectedCrash(&'static str),
    /// SQLite file closure or startup replay could not safely resolve a
    /// sidecar. The reason/artifact pair is restricted by the local-runtime
    /// contract and callers must keep the Store closed.
    #[error("storage_recovery_blocked {{ reason: {reason}, artifact: {artifact} }}")]
    StorageRecoveryBlocked {
        /// Stable recovery reason from the local-runtime priority table.
        reason: &'static str,
        /// Stable artifact name from the local-runtime recovery table.
        artifact: &'static str,
    },
    /// A prepared restore was already confirming or was cancelled.
    #[error("prepared_restore_not_active")]
    PreparedRestoreNotActive,
}

fn export_maintenance_error(error: super::store::StoreMaintenanceError) -> ExportError {
    match error {
        super::store::StoreMaintenanceError::Busy(phase) => ExportError::MaintenanceBusy {
            active: phase.active(),
        },
        super::store::StoreMaintenanceError::RegistryUnavailable => {
            ExportError::Io("maintenance registry unavailable".into())
        }
    }
}

fn import_maintenance_error(error: super::store::StoreMaintenanceError) -> ImportError {
    match error {
        super::store::StoreMaintenanceError::Busy(phase) => ImportError::MaintenanceBusy {
            active: phase.active(),
        },
        super::store::StoreMaintenanceError::RegistryUnavailable => {
            ImportError::Io("maintenance registry unavailable".into())
        }
    }
}

fn import_maintenance_result<T>(
    result: Result<T, super::store::StoreMaintenanceError>,
) -> Result<T, ImportError> {
    result.map_err(import_maintenance_error)
}

fn import_maintenance_registry_unavailable() -> ImportError {
    ImportError::Io("maintenance registry unavailable".into())
}

/// Manifest carried in every portable archive.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct BackupManifest {
    /// Stable package discriminator.
    pub format: String,
    /// Portable package format version (`1`, `2`, or `3`).
    pub format_version: u32,
    /// Entity schema version (always 4 for current exports).
    pub schema_version: u32,
    /// Export completion time in RFC3339 UTC form.
    pub exported_at: String,
    /// Complete inventory of entity JSON files, including empty types.
    pub entity_files: Vec<BackupEntityFile>,
    /// Complete inventory of managed Plugin artifacts.
    pub artifacts: Vec<BackupArtifact>,
}

impl BackupManifest {
    fn new(
        format_version: u32,
        entity_files: Vec<BackupEntityFile>,
        artifacts: Vec<BackupArtifact>,
    ) -> Self {
        Self {
            format: PORTABLE_FORMAT_NAME.to_string(),
            format_version,
            schema_version: ARCHIVE_SCHEMA_VERSION,
            exported_at: chrono::Utc::now().to_rfc3339_opts(chrono::SecondsFormat::Secs, true),
            entity_files,
            artifacts,
        }
    }
}

/// One entity JSON file bound by the portable manifest.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct BackupEntityFile {
    /// Stable plural entity/table name.
    pub name: String,
    /// Safe archive-relative JSON path.
    pub path: String,
    /// Exact row count in the JSON body.
    pub count: u64,
    /// Lowercase SHA-256 of the JSON body.
    pub sha256: String,
}

/// One managed Plugin artifact bound by the portable manifest.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct BackupArtifact {
    /// Plugin primary key owning the artifact.
    pub plugin_id: i64,
    /// Safe archive-relative artifact path.
    pub path: String,
    /// Exact artifact size in bytes.
    pub size_bytes: u64,
    /// Lowercase SHA-256 of the artifact bytes.
    pub sha256: String,
}

/// Backup exporter. Wraps a regular-file source database and writes
/// a passphrase-encrypted age + tar archive to a fresh target path.
pub struct BackupExporter {
    source: PathBuf,
    format: u32,
}

impl BackupExporter {
    /// Build a new exporter for the given source with current format 3.
    pub fn new(source: impl Into<PathBuf>) -> Self {
        Self {
            source: source.into(),
            format: 3,
        }
    }

    /// Build a new exporter with a specific format (1, 2, or 3).
    /// Format 3 is the only actively-written format; 1 and 2 are
    /// accepted only for versioned compatibility fixtures and upgraded.
    pub fn with_format(source: impl Into<PathBuf>, format: u32) -> Self {
        Self {
            source: source.into(),
            format,
        }
    }

    /// Source database path.
    pub fn source(&self) -> &Path {
        &self.source
    }

    /// Export to `target` using the supplied passphrase. The target
    /// must not exist.
    pub async fn export_age(
        &self,
        target: &Path,
        passphrase: &str,
    ) -> Result<BackupManifest, ExportError> {
        let source = self.source.clone();
        let prepared = prepare_canonical_export(&source, None, None).await?;
        publish_prepared_export(
            source,
            target.to_path_buf(),
            passphrase.to_string(),
            self.format,
            prepared,
        )
        .await
    }
}

async fn publish_prepared_export(
    source: PathBuf,
    target: PathBuf,
    passphrase: String,
    format: u32,
    prepared: PreparedCanonicalExport,
) -> Result<BackupManifest, ExportError> {
    let source_directory = prepared._source_directory;
    let export_source = prepared
        .database_snapshot
        .as_deref()
        .unwrap_or(&source)
        .to_path_buf();
    let managed_artifacts = prepared.managed_artifacts;
    let entity_files = prepared.entity_files;
    let cleanup = prepared.database_snapshot;
    let result = tokio::task::spawn_blocking(move || {
        export_blocking(
            &export_source,
            &target,
            &passphrase,
            format,
            managed_artifacts.as_deref(),
            entity_files.as_deref(),
            source_directory.as_deref(),
        )
    })
    .await;
    if let Some(cleanup) = cleanup {
        let _ = fs::remove_file(cleanup);
    }
    result.map_err(|error| ExportError::Io(format!("join error: {error}")))?
}

#[derive(Debug, Clone)]
struct ManagedArtifact {
    plugin_id: i64,
    key: PathBuf,
    sha256: String,
    size_bytes: u64,
}

#[derive(Debug, Clone)]
struct PreparedEntityFile {
    descriptor: BackupEntityFile,
    bytes: Vec<u8>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct PortableEntityBody {
    schema_version: u32,
    entity: String,
    columns: Vec<String>,
    rows: Vec<Vec<PortableValue>>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type", content = "value", rename_all = "snake_case")]
enum PortableValue {
    Null,
    Integer(i64),
    Real(f64),
    Text(String),
    Blob(String),
}

struct PreparedCanonicalExport {
    database_snapshot: Option<PathBuf>,
    managed_artifacts: Option<Vec<ManagedArtifact>>,
    entity_files: Option<Vec<PreparedEntityFile>>,
    data_source_names: Vec<String>,
    _source_directory: Option<std::sync::Arc<cap_std::fs::Dir>>,
}

async fn prepare_canonical_export(
    source: &Path,
    crash_at: Option<BackupCrashPoint>,
    anchored_source: Option<AnchoredExportSource>,
) -> Result<PreparedCanonicalExport, ExportError> {
    // Fail closed at the public boundary before SQLx can resolve a symlinked,
    // hardlinked, or non-regular database source under a managed Store root.
    let mut anchored_source = match anchored_source {
        Some(anchored_source) => anchored_source,
        None => open_export_source_anchored(source)?,
    };
    let has_device_key = anchored_source
        .directory
        .symlink_metadata("encryption.key")
        .is_ok_and(|metadata| metadata.is_file() && !metadata.file_type().is_symlink());
    if !has_device_key {
        return Err(ExportError::UnsafeSource(
            "source device key is missing or unsafe".into(),
        ));
    }
    let options = sqlx::sqlite::SqliteConnectOptions::new()
        .filename(&anchored_source.database_path)
        .create_if_missing(false)
        .foreign_keys(true);
    let pool = sqlx::sqlite::SqlitePoolOptions::new()
        .min_connections(1)
        .max_connections(1)
        .connect_with(options)
        .await
        .map_err(|_| ExportError::UnsafeSource("open canonical Store snapshot".into()))?;
    let journal_mode: String = sqlx::query_scalar("PRAGMA journal_mode")
        .fetch_one(&pool)
        .await
        .map_err(|_| ExportError::Io("read canonical Store journal mode".into()))?;
    let checkpoint: (i64, i64, i64) = sqlx::query_as("PRAGMA wal_checkpoint(TRUNCATE)")
        .fetch_one(&pool)
        .await
        .map_err(|_| ExportError::Io("checkpoint canonical Store snapshot".into()))?;
    let converged = if journal_mode.eq_ignore_ascii_case("wal") {
        checkpoint == (0, 0, 0)
    } else {
        checkpoint == (0, -1, -1)
    };
    if !converged {
        pool.close().await;
        return Err(ExportError::Io(format!(
            "checkpoint did not converge for {journal_mode}: {checkpoint:?}"
        )));
    }
    if let Err(error) = maybe_inject_backup_crash(crash_at, "checkpoint_current") {
        pool.close().await;
        return Err(error);
    }
    // query-plan: id=t129.plugins.portable_inventory; owner_phase=US13; activation_task=T129
    let rows = sqlx::query_as::<_, (i64, String, String, i64)>(
        "SELECT id, s3_key, sha256, size_bytes FROM plugins WHERE deleted_at IS NULL ORDER BY id",
    )
    .fetch_all(&pool)
    .await
    .map_err(|_| ExportError::Io("read managed Plugin ownership".into()))?;
    pool.close().await;
    refresh_anchored_source_after_checkpoint(source, &mut anchored_source)?;
    let mut seen = BTreeSet::new();
    let mut artifacts = Vec::with_capacity(rows.len());
    for (plugin_id, key, sha256, size_bytes) in rows {
        let key = PathBuf::from(key);
        if validate_relative_path(&key).is_err()
            || sha256.len() != 64
            || !sha256.bytes().all(|byte| byte.is_ascii_hexdigit())
            || size_bytes < 0
            || !seen.insert(key.clone())
        {
            return Err(ExportError::UnsafeSource(
                "invalid managed Plugin ownership row".into(),
            ));
        }
        artifacts.push(ManagedArtifact {
            plugin_id,
            key,
            sha256: sha256.to_ascii_lowercase(),
            size_bytes: size_bytes as u64,
        });
    }
    let (mut device_key_file, device_key_metadata) =
        open_verified_regular_file_at(&anchored_source.directory, Path::new("encryption.key"), 32)?;
    let mut source_device_key = Vec::with_capacity(32);
    device_key_file
        .read_to_end(&mut source_device_key)
        .map_err(|_| ExportError::UnsafeSource("read source device key".into()))?;
    verify_open_file_identity(
        Path::new("encryption.key"),
        &device_key_file,
        &device_key_metadata,
    )?;
    let source_device_key: [u8; 32] = source_device_key
        .as_slice()
        .try_into()
        .map_err(|_| ExportError::UnsafeSource("source device key length".into()))?;
    let file_name = source
        .file_name()
        .and_then(|name| name.to_str())
        .unwrap_or(DATABASE_FILENAME);
    let snapshot_name = format!(".{file_name}.{}.portable", Uuid::new_v4());
    let snapshot = anchored_source.directory_path.join(&snapshot_name);
    let mut snapshot_options = cap_std::fs::OpenOptions::new();
    snapshot_options.write(true).create_new(true);
    let mut snapshot_file = anchored_source
        .directory
        .open_with(&snapshot_name, &snapshot_options)
        .map_err(|error| ExportError::Io(error.to_string()))?;
    let mut source_file = anchored_source
        .file
        .try_clone()
        .map_err(|error| ExportError::Io(error.to_string()))?;
    source_file
        .seek(SeekFrom::Start(0))
        .and_then(|_| std::io::copy(&mut source_file, &mut snapshot_file))
        .and_then(|_| snapshot_file.sync_all())
        .map_err(|error| ExportError::Io(error.to_string()))?;
    sync_cap_directory(&anchored_source.directory, "fsync portable snapshot parent")
        .map_err(|error| ExportError::Io(error.to_string()))?;
    verify_open_file_identity(source, &anchored_source.file, &anchored_source.metadata)?;
    let snapshot_options = sqlx::sqlite::SqliteConnectOptions::new()
        .filename(&snapshot)
        .create_if_missing(false)
        .foreign_keys(true);
    let snapshot_pool = match sqlx::sqlite::SqlitePoolOptions::new()
        .min_connections(1)
        .max_connections(1)
        .connect_with(snapshot_options)
        .await
    {
        Ok(pool) => pool,
        Err(_) => {
            let _ = fs::remove_file(&snapshot);
            return Err(ExportError::Io("open portable database snapshot".into()));
        }
    };
    let source_crypto = super::crypto::Crypto::new(&source_device_key);
    let cleanup_result = async {
        let checkpoint: (i64, i64, i64) = sqlx::query_as("PRAGMA wal_checkpoint(TRUNCATE)")
            .fetch_one(&snapshot_pool)
            .await
            .map_err(|_| ExportError::Io("checkpoint portable database snapshot".into()))?;
        if checkpoint.0 != 0 {
            return Err(ExportError::Io(
                "portable database snapshot checkpoint busy".into(),
            ));
        }
        maybe_inject_backup_crash(crash_at, "checkpoint_staging")?;
        let data_source_names =
            sqlx::query_scalar::<_, String>("SELECT name FROM data_sources ORDER BY name")
                .fetch_all(&snapshot_pool)
                .await
                .map_err(|_| ExportError::Io("read confirmed data source names".into()))?;
        let entity_files = export_entity_files(&snapshot_pool, &source_crypto).await?;
        Ok::<_, ExportError>((entity_files, data_source_names))
    }
    .await;
    let (entity_files, data_source_names) = match cleanup_result {
        Ok(prepared) => prepared,
        Err(error) => {
            snapshot_pool.close().await;
            let _ = fs::remove_file(&snapshot);
            return Err(error);
        }
    };
    snapshot_pool.close().await;
    Ok(PreparedCanonicalExport {
        database_snapshot: Some(snapshot),
        managed_artifacts: Some(artifacts),
        entity_files: Some(entity_files),
        data_source_names,
        _source_directory: Some(anchored_source.directory),
    })
}

async fn export_entity_files(
    pool: &sqlx::SqlitePool,
    source_crypto: &super::crypto::Crypto,
) -> Result<Vec<PreparedEntityFile>, ExportError> {
    let mut files = Vec::with_capacity(ENTITY_TABLES.len());
    for table in ENTITY_TABLES {
        let columns = entity_columns(table)
            .iter()
            .map(|column| (*column).to_string())
            .collect::<Vec<_>>();
        let rows = fetch_entity_rows(pool, table).await?;
        let mut portable_rows = Vec::with_capacity(rows.len());
        for row in rows {
            let mut values = Vec::with_capacity(columns.len());
            for (index, column) in columns.iter().enumerate() {
                let raw = row
                    .try_get_raw(index)
                    .map_err(|_| ExportError::Io(format!("read {table}.{column}")))?;
                if raw.is_null() {
                    values.push(PortableValue::Null);
                    continue;
                }
                let value_type = raw.type_info().name().to_ascii_uppercase();
                let value =
                    if value_type.contains("INT") {
                        PortableValue::Integer(row.try_get(index).map_err(|_| {
                            ExportError::Io(format!("decode integer {table}.{column}"))
                        })?)
                    } else if value_type.contains("REAL")
                        || value_type.contains("FLOA")
                        || value_type.contains("DOUB")
                    {
                        PortableValue::Real(row.try_get(index).map_err(|_| {
                            ExportError::Io(format!("decode real {table}.{column}"))
                        })?)
                    } else if value_type.contains("BLOB") {
                        let mut bytes: Vec<u8> = row.try_get(index).map_err(|_| {
                            ExportError::Io(format!("decode blob {table}.{column}"))
                        })?;
                        if is_sensitive_entity_column(table, column) {
                            bytes = source_crypto.decrypt(&bytes).map_err(|_| {
                                ExportError::UnsafeSource(format!(
                                    "decrypt portable field {table}.{}",
                                    column
                                ))
                            })?;
                        }
                        PortableValue::Blob(hex::encode(bytes))
                    } else {
                        PortableValue::Text(row.try_get(index).map_err(|_| {
                            ExportError::Io(format!("decode text {table}.{column}"))
                        })?)
                    };
                values.push(value);
            }
            portable_rows.push(values);
        }
        let body = PortableEntityBody {
            schema_version: ARCHIVE_SCHEMA_VERSION,
            entity: (*table).to_string(),
            columns,
            rows: portable_rows,
        };
        let bytes = serde_json::to_vec(&body)
            .map_err(|error| ExportError::Io(format!("encode entity {table}: {error}")))?;
        if bytes.len() > MAX_ENTITY_FILE_BYTES {
            return Err(ExportError::Io(format!(
                "portable entity {table} exceeds 64 MiB"
            )));
        }
        let path = format!("entities/{table}.json");
        files.push(PreparedEntityFile {
            descriptor: BackupEntityFile {
                name: (*table).to_string(),
                path,
                count: body.rows.len() as u64,
                sha256: hex::encode(Sha256::digest(&bytes)),
            },
            bytes,
        });
    }
    Ok(files)
}

fn entity_columns(table: &str) -> &'static [&'static str] {
    match table {
        "categories" => &[
            "id",
            "parent_id",
            "name",
            "slug",
            "description",
            "created_at",
            "updated_at",
        ],
        "capabilities" => &[
            "name",
            "description",
            "is_dangerous",
            "category_id",
            "normalized_name",
            "created_at",
        ],
        "global_configs" => &[
            "id",
            "name",
            "key",
            "type",
            "data",
            "created_at",
            "updated_at",
        ],
        "llm_presets" => &[
            "id",
            "name",
            "description",
            "is_default",
            "max_tokens",
            "temperature",
            "created_at",
            "updated_at",
        ],
        "llm_providers" => &[
            "id",
            "name",
            "category",
            "base_url",
            "token_env",
            "token_encrypted",
            "created_at",
            "updated_at",
        ],
        "models" => &[
            "id",
            "name",
            "preset_id",
            "provider_id",
            "priority",
            "created_at",
            "updated_at",
        ],
        "tags" => &[
            "id",
            "name",
            "color",
            "normalized_name",
            "created_at",
            "updated_at",
        ],
        "data_sources" => &[
            "id",
            "name",
            "host",
            "port",
            "username",
            "encrypted_password",
            "created_at",
            "updated_at",
        ],
        "plugins" => &[
            "id",
            "identifier",
            "name",
            "description",
            "manifest",
            "version",
            "author",
            "repository_url",
            "s3_key",
            "sha256",
            "size_bytes",
            "runtime",
            "category_id",
            "capabilities",
            "resource_limits",
            "row_revision",
            "created_at",
            "updated_at",
            "deleted_at",
        ],
        "functions" => &[
            "id",
            "identifier",
            "name",
            "description",
            "kind",
            "input_schema",
            "output_schema",
            "plugin_id",
            "plugin_export",
            "category_id",
            "required_capabilities",
            "created_at",
            "updated_at",
        ],
        "workflows" => &[
            "id",
            "identifier",
            "name",
            "description",
            "timeout_ms",
            "category_id",
            "input_schema",
            "start_description",
            "output_schema",
            "required_capabilities",
            "created_at",
            "updated_at",
        ],
        "workflow_nodes" => &[
            "id",
            "workflow_id",
            "node_key",
            "node_type",
            "function_id",
            "position_x",
            "position_y",
            "node_config",
            "created_at",
        ],
        "workflow_edges" => &[
            "id",
            "workflow_id",
            "src_node_key",
            "dst_node_key",
            "mapping",
        ],
        "tools" => &[
            "id",
            "identifier",
            "name",
            "description",
            "kind",
            "source",
            "is_always",
            "function_id",
            "workflow_id",
            "input_schema",
            "output_schema",
            "category_id",
            "required_capabilities",
            "created_at",
            "updated_at",
        ],
        "skills" => &[
            "id",
            "identifier",
            "name",
            "description",
            "frontmatter",
            "content",
            "source",
            "is_always",
            "category_id",
            "required_capabilities",
            "created_at",
            "updated_at",
        ],
        "agents" => &[
            "id",
            "identifier",
            "name",
            "description",
            "system_prompt",
            "parent_agent_id",
            "depth",
            "is_default",
            "model_preset",
            "category_id",
            "name_normalized",
            "created_at",
            "updated_at",
        ],
        "agent_tools" => &["agent_id", "tool_id", "created_at"],
        "agent_skills" => &["agent_id", "skill_id", "created_at"],
        "agent_capabilities" => &["agent_id", "capability_name", "created_at"],
        "chat_sessions" => &[
            "id",
            "entry_agent_id",
            "current_agent_id",
            "title_encrypted",
            "status",
            "execution_id",
            "created_at",
            "updated_at",
            "expires_at",
        ],
        "chat_messages" => &[
            "id",
            "session_id",
            "seq",
            "role",
            "content_encrypted",
            "tool_calls_encrypted",
            "created_at",
        ],
        "agent_executions" => &[
            "execution_id",
            "session_id",
            "current_agent_id",
            "status",
            "state_encrypted",
            "started_at",
            "finished_at",
            "error_kind",
        ],
        _ => &[],
    }
}

async fn fetch_entity_rows(
    pool: &sqlx::SqlitePool,
    table: &str,
) -> Result<Vec<sqlx::sqlite::SqliteRow>, ExportError> {
    let result = match table {
        "categories" => sqlx::query("SELECT id,parent_id,name,slug,description,created_at,updated_at FROM categories ORDER BY id").fetch_all(pool).await,
        "capabilities" => sqlx::query("SELECT name,description,is_dangerous,category_id,normalized_name,created_at FROM capabilities ORDER BY name").fetch_all(pool).await,
        "global_configs" => sqlx::query("SELECT id,name,key,type,data,created_at,updated_at FROM global_configs ORDER BY id").fetch_all(pool).await,
        "llm_presets" => sqlx::query("SELECT id,name,description,is_default,max_tokens,temperature,created_at,updated_at FROM llm_presets ORDER BY id").fetch_all(pool).await,
        "llm_providers" => sqlx::query("SELECT id,name,category,base_url,token_env,token_encrypted,created_at,updated_at FROM llm_providers ORDER BY id").fetch_all(pool).await,
        "models" => sqlx::query("SELECT id,name,preset_id,provider_id,priority,created_at,updated_at FROM models ORDER BY id").fetch_all(pool).await,
        "tags" => sqlx::query("SELECT id,name,color,normalized_name,created_at,updated_at FROM tags ORDER BY id").fetch_all(pool).await,
        "data_sources" => sqlx::query("SELECT id,name,host,port,username,encrypted_password,created_at,updated_at FROM data_sources ORDER BY id").fetch_all(pool).await,
        "plugins" => sqlx::query("SELECT id,identifier,name,description,manifest,version,author,repository_url,s3_key,sha256,size_bytes,runtime,category_id,capabilities,resource_limits,row_revision,created_at,updated_at,deleted_at FROM plugins ORDER BY id").fetch_all(pool).await,
        "functions" => sqlx::query("SELECT id,identifier,name,description,kind,input_schema,output_schema,plugin_id,plugin_export,category_id,required_capabilities,created_at,updated_at FROM functions ORDER BY id").fetch_all(pool).await,
        "workflows" => sqlx::query("SELECT id,identifier,name,description,timeout_ms,category_id,input_schema,start_description,output_schema,required_capabilities,created_at,updated_at FROM workflows ORDER BY id").fetch_all(pool).await,
        "workflow_nodes" => sqlx::query("SELECT id,workflow_id,node_key,node_type,function_id,position_x,position_y,node_config,created_at FROM workflow_nodes ORDER BY id").fetch_all(pool).await,
        "workflow_edges" => sqlx::query("SELECT id,workflow_id,src_node_key,dst_node_key,mapping FROM workflow_edges ORDER BY id").fetch_all(pool).await,
        "tools" => sqlx::query("SELECT id,identifier,name,description,kind,source,is_always,function_id,workflow_id,input_schema,output_schema,category_id,required_capabilities,created_at,updated_at FROM tools ORDER BY id").fetch_all(pool).await,
        "skills" => sqlx::query("SELECT id,identifier,name,description,frontmatter,content,source,is_always,category_id,required_capabilities,created_at,updated_at FROM skills ORDER BY id").fetch_all(pool).await,
        "agents" => sqlx::query("SELECT id,identifier,name,description,system_prompt,parent_agent_id,depth,is_default,model_preset,category_id,name_normalized,created_at,updated_at FROM agents ORDER BY id").fetch_all(pool).await,
        "agent_tools" => sqlx::query("SELECT agent_id,tool_id,created_at FROM agent_tools ORDER BY agent_id,tool_id").fetch_all(pool).await,
        "agent_skills" => sqlx::query("SELECT agent_id,skill_id,created_at FROM agent_skills ORDER BY agent_id,skill_id").fetch_all(pool).await,
        "agent_capabilities" => sqlx::query("SELECT agent_id,capability_name,created_at FROM agent_capabilities ORDER BY agent_id,capability_name").fetch_all(pool).await,
        "chat_sessions" => sqlx::query("SELECT id,entry_agent_id,current_agent_id,title_encrypted,status,execution_id,created_at,updated_at,expires_at FROM chat_sessions ORDER BY id").fetch_all(pool).await,
        "chat_messages" => sqlx::query("SELECT id,session_id,seq,role,content_encrypted,tool_calls_encrypted,created_at FROM chat_messages ORDER BY id").fetch_all(pool).await,
        "agent_executions" => sqlx::query("SELECT execution_id,session_id,current_agent_id,status,state_encrypted,started_at,finished_at,error_kind FROM agent_executions ORDER BY execution_id").fetch_all(pool).await,
        _ => return Err(ExportError::Io("unknown portable entity table".into())),
    };
    result.map_err(|_| ExportError::Io(format!("read portable entity {table}")))
}

fn is_sensitive_entity_column(table: &str, column: &str) -> bool {
    matches!(
        (table, column),
        ("data_sources", "encrypted_password")
            | ("llm_providers", "token_encrypted")
            | ("chat_sessions", "title_encrypted")
            | ("chat_messages", "content_encrypted")
            | ("chat_messages", "tool_calls_encrypted")
            | ("agent_executions", "state_encrypted")
    )
}

fn export_blocking(
    source: &Path,
    target: &Path,
    passphrase: &str,
    format: u32,
    managed_artifacts: Option<&[ManagedArtifact]>,
    entity_files: Option<&[PreparedEntityFile]>,
    source_directory: Option<&cap_std::fs::Dir>,
) -> Result<BackupManifest, ExportError> {
    // 1. Refuse symlinks / hardlinks / devices / FIFOs / sockets and retain
    // the verified descriptor through the complete streaming read.
    let database_source = if entity_files.is_none() {
        Some(open_export_source(source)?)
    } else {
        None
    };

    // 2. Bind the output to an already-open, no-follow target directory and
    // refuse to overwrite an existing leaf.
    let target_parent = target
        .parent()
        .ok_or_else(|| ExportError::UnsafeSource(target.display().to_string()))?;
    let target_leaf = target
        .file_name()
        .ok_or_else(|| ExportError::UnsafeSource(target.display().to_string()))?;
    let target_directory = open_export_target_directory_nofollow(target_parent)?;
    match target_directory.symlink_metadata(Path::new(target_leaf)) {
        Ok(_) => return Err(ExportError::TargetExists(target.display().to_string())),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
        Err(_) => return Err(ExportError::UnsafeSource(target.display().to_string())),
    }

    // 3. Build the manifest.
    let entity_descriptors = entity_files
        .unwrap_or_default()
        .iter()
        .map(|file| file.descriptor.clone())
        .collect::<Vec<_>>();
    let artifact_descriptors = managed_artifacts
        .unwrap_or_default()
        .iter()
        .map(|artifact| BackupArtifact {
            plugin_id: artifact.plugin_id,
            path: format!("{PLUGIN_ARCHIVE_PREFIX}{}/plugin.wasm", artifact.plugin_id),
            size_bytes: artifact.size_bytes,
            sha256: artifact.sha256.clone(),
        })
        .collect::<Vec<_>>();
    let manifest = BackupManifest::new(format, entity_descriptors, artifact_descriptors);
    let manifest_bytes = serde_json::to_vec(&manifest)
        .map_err(|e| ExportError::Io(format!("manifest encode: {e}")))?;

    // 4. Validate the owned Plugin set before creating any output bytes.
    validate_plugin_files(source, source_directory, managed_artifacts)?;
    let target_name = target
        .file_name()
        .and_then(|name| name.to_str())
        .unwrap_or("backup.age");
    let staging_name = PathBuf::from(format!(".{target_name}.{}.staging", Uuid::new_v4()));

    // 5. Stream tar -> gzip -> age directly into the unique sibling. Neither
    // the database nor the complete authenticated package is materialized in
    // memory.
    let write_result = (|| -> Result<(), ExportError> {
        let mut options = cap_std::fs::OpenOptions::new();
        options.write(true).create_new(true);
        let target_file = target_directory
            .open_with(&staging_name, &options)
            .map(cap_std::fs::File::into_std)
            .map_err(|e| ExportError::Io(e.to_string()))?;
        let passphrase_secret = SecretString::new(passphrase.to_string().into_boxed_str());
        let encryptor = Encryptor::with_user_passphrase(passphrase_secret);
        // age's passphrase KDF is deliberately memory-hard. Serializing only
        // that short derivation boundary prevents parallel backup tests (and
        // real concurrent requests) from turning memory pressure into an
        // indistinguishable authentication failure; the encrypted stream
        // itself remains fully streaming after the guard is released.
        let age_writer = {
            let _guard = age_kdf_lock()
                .lock()
                .map_err(|_| ExportError::Age("passphrase KDF unavailable".into()))?;
            encryptor
                .wrap_output(target_file)
                .map_err(|e| ExportError::Age(format!("wrap_output: {e}")))?
        };
        let gzip_writer = GzEncoder::new(age_writer, flate2::Compression::default());
        let mut tar_writer = tar::Builder::new(gzip_writer);
        let mut header = tar::Header::new_gnu();
        header.set_size(manifest_bytes.len() as u64);
        header.set_mode(0o644);
        header
            .set_path(MANIFEST_FILENAME)
            .map_err(|e| ExportError::Io(format!("tar header path: {e}")))?;
        header.set_cksum();
        tar_writer
            .append(&header, manifest_bytes.as_slice())
            .map_err(|e| ExportError::Io(format!("tar append manifest: {e}")))?;

        if let Some(entity_files) = entity_files {
            for entity in entity_files {
                let mut entity_header = tar::Header::new_gnu();
                entity_header.set_size(entity.bytes.len() as u64);
                entity_header.set_mode(0o600);
                entity_header
                    .set_path(&entity.descriptor.path)
                    .map_err(|e| ExportError::Io(format!("tar entity path: {e}")))?;
                entity_header.set_cksum();
                tar_writer
                    .append(&entity_header, entity.bytes.as_slice())
                    .map_err(|e| ExportError::Io(format!("tar append entity: {e}")))?;
            }
        } else {
            let (mut database_file, metadata) = database_source
                .ok_or_else(|| ExportError::Io("legacy database source missing".into()))?;
            let mut db_header = tar::Header::new_gnu();
            db_header.set_size(metadata.len());
            db_header.set_mode(0o644);
            db_header
                .set_path(DATABASE_FILENAME)
                .map_err(|e| ExportError::Io(format!("tar header path: {e}")))?;
            db_header.set_cksum();
            tar_writer
                .append(&db_header, &mut database_file)
                .map_err(|e| ExportError::Io(format!("tar append database: {e}")))?;
            verify_open_file_identity(source, &database_file, &metadata)?;
        }
        if let Some(managed_artifacts) = managed_artifacts
            && !managed_artifacts.is_empty()
        {
            let plugin_directory = open_managed_plugin_directory(source, source_directory)?;
            for artifact in managed_artifacts {
                let (mut file, before) = open_verified_regular_file_at(
                    &plugin_directory,
                    &artifact.key,
                    artifact.size_bytes,
                )?;
                verify_open_file_contents(
                    &mut file,
                    &artifact.key,
                    &before,
                    artifact.size_bytes,
                    &artifact.sha256,
                )?;
                let archive_path =
                    format!("{PLUGIN_ARCHIVE_PREFIX}{}/plugin.wasm", artifact.plugin_id);
                let mut plugin_header = tar::Header::new_gnu();
                plugin_header.set_size(artifact.size_bytes);
                plugin_header.set_mode(0o600);
                plugin_header
                    .set_path(&archive_path)
                    .map_err(|e| ExportError::Io(format!("tar plugin path: {e}")))?;
                plugin_header.set_cksum();
                tar_writer
                    .append(&plugin_header, &mut file)
                    .map_err(|e| ExportError::Io(format!("tar append plugin: {e}")))?;
                verify_open_file_identity(&artifact.key, &file, &before)?;
            }
        }
        let gzip_writer = tar_writer
            .into_inner()
            .map_err(|e| ExportError::Io(format!("tar finish: {e}")))?;
        let age_writer = gzip_writer
            .finish()
            .map_err(|e| ExportError::Io(format!("gzip finish: {e}")))?;
        let target_file = age_writer
            .finish()
            .map_err(|e| ExportError::Age(format!("finish: {e}")))?;
        target_file
            .sync_all()
            .map_err(|e| ExportError::Io(e.to_string()))?;
        Ok(())
    })();
    if let Err(error) = write_result {
        let _ = target_directory.remove_file(&staging_name);
        return Err(error);
    }

    // 6. Publish without replacement and then fsync the parent directory.
    if let Err(error) = publish_no_replace_at(
        &target_directory,
        &staging_name,
        &target_directory,
        Path::new(target_leaf),
    ) {
        let _ = target_directory.remove_file(&staging_name);
        return Err(if error.kind() == std::io::ErrorKind::AlreadyExists {
            ExportError::TargetExists(target.display().to_string())
        } else {
            ExportError::Io(error.to_string())
        });
    }
    if let Err(error) = sync_cap_directory(&target_directory, "fsync backup target directory") {
        return Err(ExportError::BackupPublishUncertain(format!(
            "fsync {}: {error}",
            target_parent.display()
        )));
    }

    Ok(manifest)
}

#[cfg(unix)]
fn open_export_directory_nofollow(path: &Path) -> Result<cap_std::fs::Dir, ExportError> {
    open_export_directory_nofollow_impl(path, false)
}

#[cfg(unix)]
fn open_export_target_directory_nofollow(path: &Path) -> Result<cap_std::fs::Dir, ExportError> {
    open_export_directory_nofollow_impl(path, true)
}

#[cfg(unix)]
fn open_export_directory_nofollow_impl(
    path: &Path,
    create_missing: bool,
) -> Result<cap_std::fs::Dir, ExportError> {
    let absolute = if path.is_absolute() {
        path.to_path_buf()
    } else {
        std::env::current_dir()
            .map_err(|_| ExportError::UnsafeSource(path.display().to_string()))?
            .join(path)
    };
    let mut directory = cap_std::fs::Dir::open_ambient_dir("/", cap_std::ambient_authority())
        .map_err(|_| ExportError::UnsafeSource(path.display().to_string()))?;
    for component in absolute.components() {
        match component {
            std::path::Component::RootDir => {}
            std::path::Component::Normal(name) => {
                let relative = Path::new(name);
                let metadata = match directory.symlink_metadata(relative) {
                    Ok(metadata) => metadata,
                    Err(error)
                        if create_missing && error.kind() == std::io::ErrorKind::NotFound =>
                    {
                        directory
                            .create_dir(relative)
                            .map_err(|_| ExportError::UnsafeSource(path.display().to_string()))?;
                        sync_cap_directory(&directory, "fsync created backup parent")
                            .map_err(|error| ExportError::Io(error.to_string()))?;
                        directory
                            .symlink_metadata(relative)
                            .map_err(|_| ExportError::UnsafeSource(path.display().to_string()))?
                    }
                    Err(_) => {
                        return Err(ExportError::UnsafeSource(path.display().to_string()));
                    }
                };
                if !metadata.is_dir() || metadata.file_type().is_symlink() {
                    return Err(ExportError::UnsafeSource(path.display().to_string()));
                }
                directory = directory
                    .open_dir(relative)
                    .map_err(|_| ExportError::UnsafeSource(path.display().to_string()))?;
            }
            _ => return Err(ExportError::UnsafeSource(path.display().to_string())),
        }
    }
    Ok(directory)
}

#[cfg(not(unix))]
fn open_export_directory_nofollow(path: &Path) -> Result<cap_std::fs::Dir, ExportError> {
    let metadata = fs::symlink_metadata(path)
        .map_err(|_| ExportError::UnsafeSource(path.display().to_string()))?;
    if !metadata.is_dir() || metadata.file_type().is_symlink() {
        return Err(ExportError::UnsafeSource(path.display().to_string()));
    }
    cap_std::fs::Dir::open_ambient_dir(path, cap_std::ambient_authority())
        .map_err(|_| ExportError::UnsafeSource(path.display().to_string()))
}

#[cfg(not(unix))]
fn open_export_target_directory_nofollow(path: &Path) -> Result<cap_std::fs::Dir, ExportError> {
    open_export_directory_nofollow(path)
}

fn open_export_source(source: &Path) -> Result<(File, fs::Metadata), ExportError> {
    let parent = source
        .parent()
        .ok_or_else(|| ExportError::UnsafeSource(source.display().to_string()))?;
    let name = source
        .file_name()
        .ok_or_else(|| ExportError::UnsafeSource(source.display().to_string()))?;
    let parent = open_export_directory_nofollow(parent)?;
    let mut options = cap_std::fs::OpenOptions::new();
    options.read(true);
    #[cfg(unix)]
    {
        use cap_std::fs::OpenOptionsExt as _;
        options.custom_flags(libc::O_NOFOLLOW | libc::O_CLOEXEC | libc::O_NONBLOCK);
    }
    let file = parent
        .open_with(Path::new(name), &options)
        .map_err(|error| {
            if error.kind() == std::io::ErrorKind::NotFound {
                ExportError::SourceMissing(source.display().to_string())
            } else {
                ExportError::UnsafeSource(source.display().to_string())
            }
        })?
        .into_std();
    let metadata = file
        .metadata()
        .map_err(|_| ExportError::UnsafeSource(source.display().to_string()))?;
    if !metadata.is_file() || metadata.len() == 0 {
        return Err(ExportError::UnsafeSource(source.display().to_string()));
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::MetadataExt as _;
        if metadata.nlink() != 1 {
            return Err(ExportError::UnsafeSource(source.display().to_string()));
        }
    }
    Ok((file, metadata))
}

struct AnchoredExportSource {
    directory: std::sync::Arc<cap_std::fs::Dir>,
    directory_path: PathBuf,
    database_path: PathBuf,
    file: File,
    metadata: fs::Metadata,
}

fn refresh_anchored_source_after_checkpoint(
    source: &Path,
    anchored: &mut AnchoredExportSource,
) -> Result<(), ExportError> {
    let name = source
        .file_name()
        .ok_or_else(|| ExportError::UnsafeSource(source.display().to_string()))?;
    let mut options = cap_std::fs::OpenOptions::new();
    options.read(true);
    #[cfg(unix)]
    {
        use cap_std::fs::OpenOptionsExt as _;
        options.custom_flags(libc::O_NOFOLLOW | libc::O_CLOEXEC | libc::O_NONBLOCK);
    }
    let reopened = anchored
        .directory
        .open_with(Path::new(name), &options)
        .map_err(|_| ExportError::UnsafeSource(source.display().to_string()))?
        .into_std();
    let current = anchored
        .file
        .metadata()
        .map_err(|_| ExportError::UnsafeSource(source.display().to_string()))?;
    let reopened_metadata = reopened
        .metadata()
        .map_err(|_| ExportError::UnsafeSource(source.display().to_string()))?;
    if !current.is_file() || !reopened_metadata.is_file() {
        return Err(ExportError::UnsafeSource(source.display().to_string()));
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::MetadataExt as _;
        if anchored.metadata.dev() != current.dev()
            || anchored.metadata.ino() != current.ino()
            || current.dev() != reopened_metadata.dev()
            || current.ino() != reopened_metadata.ino()
            || current.nlink() != 1
            || reopened_metadata.nlink() != 1
            || current.len() != reopened_metadata.len()
            || current.mtime() != reopened_metadata.mtime()
            || current.mtime_nsec() != reopened_metadata.mtime_nsec()
        {
            return Err(ExportError::UnsafeSource(source.display().to_string()));
        }
    }
    #[cfg(not(unix))]
    {
        if current.len() != reopened_metadata.len()
            || current.modified().ok() != reopened_metadata.modified().ok()
            || anchored.metadata.created().ok() != current.created().ok()
        {
            return Err(ExportError::UnsafeSource(source.display().to_string()));
        }
    }
    anchored.file = reopened;
    anchored.metadata = reopened_metadata;
    Ok(())
}

fn open_export_source_anchored(source: &Path) -> Result<AnchoredExportSource, ExportError> {
    let parent_path = source
        .parent()
        .ok_or_else(|| ExportError::UnsafeSource(source.display().to_string()))?;
    let name = source
        .file_name()
        .ok_or_else(|| ExportError::UnsafeSource(source.display().to_string()))?;
    let directory = std::sync::Arc::new(open_export_directory_nofollow(parent_path)?);
    let mut options = cap_std::fs::OpenOptions::new();
    options.read(true);
    #[cfg(unix)]
    {
        use cap_std::fs::OpenOptionsExt as _;
        options.custom_flags(libc::O_NOFOLLOW | libc::O_CLOEXEC | libc::O_NONBLOCK);
    }
    let file = directory
        .open_with(Path::new(name), &options)
        .map_err(|error| {
            if error.kind() == std::io::ErrorKind::NotFound {
                ExportError::SourceMissing(source.display().to_string())
            } else {
                ExportError::UnsafeSource(source.display().to_string())
            }
        })?
        .into_std();
    let metadata = file
        .metadata()
        .map_err(|_| ExportError::UnsafeSource(source.display().to_string()))?;
    if !metadata.is_file() || metadata.len() == 0 {
        return Err(ExportError::UnsafeSource(source.display().to_string()));
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::MetadataExt as _;
        if metadata.nlink() != 1 {
            return Err(ExportError::UnsafeSource(source.display().to_string()));
        }
    }
    #[cfg(target_os = "linux")]
    let directory_path = {
        use std::os::fd::AsRawFd as _;
        PathBuf::from(format!("/proc/self/fd/{}", directory.as_raw_fd()))
    };
    #[cfg(not(target_os = "linux"))]
    let directory_path = parent_path.to_path_buf();
    let database_path = directory_path.join(name);
    Ok(AnchoredExportSource {
        directory,
        directory_path,
        database_path,
        file,
        metadata,
    })
}

#[cfg(target_os = "linux")]
fn verify_store_source_leaf_matches_anchor(
    source: &Path,
    anchored: &AnchoredExportSource,
) -> Result<(), ExportError> {
    use cap_std::fs::MetadataExt as _;
    use std::os::unix::fs::MetadataExt as _;

    let name = source
        .file_name()
        .ok_or_else(|| ExportError::UnsafeSource(source.display().to_string()))?;
    let pinned = anchored
        .file
        .metadata()
        .map_err(|_| ExportError::UnsafeSource(source.display().to_string()))?;
    let canonical = anchored
        .directory
        .symlink_metadata(Path::new(name))
        .map_err(|_| ExportError::UnsafeSource(source.display().to_string()))?;
    let pinned_type = pinned.mode() & libc::S_IFMT;
    let canonical_type = canonical.mode() & libc::S_IFMT;
    if !pinned.is_file()
        || !canonical.is_file()
        || canonical.file_type().is_symlink()
        || anchored.metadata.dev() != pinned.dev()
        || anchored.metadata.ino() != pinned.ino()
        || anchored.metadata.mode() & libc::S_IFMT != pinned_type
        || anchored.metadata.nlink() != pinned.nlink()
        || pinned.dev() != canonical.dev()
        || pinned.ino() != canonical.ino()
        || pinned_type != canonical_type
        || pinned.nlink() != canonical.nlink()
        || canonical.nlink() != 1
    {
        return Err(ExportError::UnsafeSource(source.display().to_string()));
    }
    Ok(())
}

fn verify_open_file_identity(
    source: &Path,
    file: &File,
    before: &fs::Metadata,
) -> Result<(), ExportError> {
    let after = file
        .metadata()
        .map_err(|_| ExportError::UnsafeSource(source.display().to_string()))?;
    if !after.is_file() || before.len() != after.len() {
        return Err(ExportError::UnsafeSource(source.display().to_string()));
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::MetadataExt as _;
        if before.dev() != after.dev()
            || before.ino() != after.ino()
            || after.nlink() != 1
            || before.mtime() != after.mtime()
            || before.mtime_nsec() != after.mtime_nsec()
        {
            return Err(ExportError::UnsafeSource(source.display().to_string()));
        }
    }
    Ok(())
}

#[cfg(target_os = "linux")]
fn publish_no_replace(staging: &Path, target: &Path) -> std::io::Result<()> {
    use std::ffi::CString;
    use std::os::unix::ffi::OsStrExt;
    let staging = CString::new(staging.as_os_str().as_bytes())?;
    let target = CString::new(target.as_os_str().as_bytes())?;
    // SAFETY: both C strings are NUL-terminated and remain alive for the call;
    // AT_FDCWD resolves the already-validated sibling paths and
    // RENAME_NOREPLACE provides the required atomic collision behavior.
    let result = unsafe {
        libc::renameat2(
            libc::AT_FDCWD,
            staging.as_ptr(),
            libc::AT_FDCWD,
            target.as_ptr(),
            libc::RENAME_NOREPLACE,
        )
    };
    if result == 0 {
        Ok(())
    } else {
        Err(std::io::Error::last_os_error())
    }
}

#[cfg(not(target_os = "linux"))]
fn publish_no_replace(staging: &Path, target: &Path) -> std::io::Result<()> {
    fs::hard_link(staging, target)?;
    fs::remove_file(staging)
}

fn open_managed_plugin_directory(
    source: &Path,
    source_directory: Option<&cap_std::fs::Dir>,
) -> Result<cap_std::fs::Dir, ExportError> {
    if let Some(source_directory) = source_directory {
        let metadata = source_directory
            .symlink_metadata("plugins")
            .map_err(|_| ExportError::UnsafeSource("managed Plugin root".into()))?;
        if !metadata.is_dir() || metadata.file_type().is_symlink() {
            return Err(ExportError::UnsafeSource("managed Plugin root".into()));
        }
        return source_directory
            .open_dir("plugins")
            .map_err(|_| ExportError::UnsafeSource("managed Plugin root".into()));
    }
    let parent = source
        .parent()
        .ok_or_else(|| ExportError::UnsafeSource(source.display().to_string()))?;
    let plugin_root = parent.join("plugins");
    open_export_directory_nofollow(&plugin_root)
}

fn validate_plugin_files(
    source: &Path,
    source_directory: Option<&cap_std::fs::Dir>,
    managed_artifacts: Option<&[ManagedArtifact]>,
) -> Result<(), ExportError> {
    let Some(managed_artifacts) = managed_artifacts else {
        return Ok(());
    };
    if managed_artifacts.is_empty() {
        return Ok(());
    }
    let plugin_directory = open_managed_plugin_directory(source, source_directory)?;
    for artifact in managed_artifacts {
        let (mut file, before) =
            open_verified_regular_file_at(&plugin_directory, &artifact.key, artifact.size_bytes)?;
        verify_open_file_contents(
            &mut file,
            &artifact.key,
            &before,
            artifact.size_bytes,
            &artifact.sha256,
        )?;
    }
    Ok(())
}

fn open_verified_regular_file_at(
    directory: &cap_std::fs::Dir,
    relative: &Path,
    expected_size: u64,
) -> Result<(File, fs::Metadata), ExportError> {
    validate_relative_path(relative)
        .map_err(|_| ExportError::UnsafeSource(relative.display().to_string()))?;
    let entry = directory
        .symlink_metadata(relative)
        .map_err(|_| ExportError::UnsafeSource(relative.display().to_string()))?;
    if !entry.is_file() || entry.file_type().is_symlink() {
        return Err(ExportError::UnsafeSource(relative.display().to_string()));
    }
    let mut options = cap_std::fs::OpenOptions::new();
    options.read(true);
    #[cfg(unix)]
    {
        use cap_std::fs::OpenOptionsExt as _;
        options.custom_flags(libc::O_NOFOLLOW | libc::O_CLOEXEC | libc::O_NONBLOCK);
    }
    let file = directory
        .open_with(relative, &options)
        .map_err(|_| ExportError::UnsafeSource(relative.display().to_string()))?
        .into_std();
    let metadata = file
        .metadata()
        .map_err(|_| ExportError::UnsafeSource(relative.display().to_string()))?;
    if !metadata.is_file() || metadata.len() != expected_size {
        return Err(ExportError::UnsafeSource(relative.display().to_string()));
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::MetadataExt as _;
        if metadata.nlink() != 1 {
            return Err(ExportError::UnsafeSource(relative.display().to_string()));
        }
    }
    Ok((file, metadata))
}

fn verify_open_file_contents(
    file: &mut File,
    display_path: &Path,
    before: &fs::Metadata,
    expected_size: u64,
    expected_sha256: &str,
) -> Result<(), ExportError> {
    let mut digest = Sha256::new();
    let mut actual_size = 0_u64;
    let mut buffer = vec![0_u8; 1024 * 1024];
    loop {
        let read = file
            .read(&mut buffer)
            .map_err(|_| ExportError::UnsafeSource(display_path.display().to_string()))?;
        if read == 0 {
            break;
        }
        actual_size = actual_size
            .checked_add(read as u64)
            .ok_or_else(|| ExportError::UnsafeSource(display_path.display().to_string()))?;
        if actual_size > expected_size {
            return Err(ExportError::UnsafeSource(
                display_path.display().to_string(),
            ));
        }
        digest.update(&buffer[..read]);
    }
    verify_open_file_identity(display_path, file, before)?;
    if actual_size != expected_size || hex::encode(digest.finalize()) != expected_sha256 {
        return Err(ExportError::UnsafeSource(
            display_path.display().to_string(),
        ));
    }
    file.seek(SeekFrom::Start(0))
        .map_err(|_| ExportError::UnsafeSource(display_path.display().to_string()))?;
    Ok(())
}

fn read_verified_regular_file(
    path: &Path,
    expected_size: u64,
    expected_sha256: &str,
) -> Result<Vec<u8>, ExportError> {
    let mut options = OpenOptions::new();
    options.read(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.custom_flags(libc::O_NOFOLLOW | libc::O_CLOEXEC);
    }
    let mut file = options
        .open(path)
        .map_err(|_| ExportError::UnsafeSource(path.display().to_string()))?;
    read_verified_open_file(&mut file, path, expected_size, expected_sha256)
}

fn read_verified_open_file(
    file: &mut File,
    display_path: &Path,
    expected_size: u64,
    expected_sha256: &str,
) -> Result<Vec<u8>, ExportError> {
    let before = file
        .metadata()
        .map_err(|_| ExportError::UnsafeSource(display_path.display().to_string()))?;
    if !before.is_file() || before.len() != expected_size {
        return Err(ExportError::UnsafeSource(
            display_path.display().to_string(),
        ));
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::MetadataExt;
        if before.nlink() != 1 {
            return Err(ExportError::UnsafeSource(
                display_path.display().to_string(),
            ));
        }
    }
    let mut bytes = Vec::with_capacity(expected_size.min(16 * 1024 * 1024) as usize);
    file.read_to_end(&mut bytes)
        .map_err(|_| ExportError::UnsafeSource(display_path.display().to_string()))?;
    let after = file
        .metadata()
        .map_err(|_| ExportError::UnsafeSource(display_path.display().to_string()))?;
    #[cfg(not(unix))]
    if before.len() != after.len() || before.modified().ok() != after.modified().ok() {
        return Err(ExportError::UnsafeSource(
            display_path.display().to_string(),
        ));
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::MetadataExt;
        if before.dev() != after.dev()
            || before.ino() != after.ino()
            || before.size() != after.size()
            || before.mtime() != after.mtime()
            || before.mtime_nsec() != after.mtime_nsec()
        {
            return Err(ExportError::UnsafeSource(
                display_path.display().to_string(),
            ));
        }
    }
    let actual = hex::encode(Sha256::digest(&bytes));
    if bytes.len() as u64 != expected_size || actual != expected_sha256 {
        return Err(ExportError::UnsafeSource(
            display_path.display().to_string(),
        ));
    }
    Ok(bytes)
}

fn validate_relative_path(path: &Path) -> Result<(), ()> {
    if path.as_os_str().is_empty()
        || path
            .components()
            .any(|component| !matches!(component, std::path::Component::Normal(_)))
    {
        return Err(());
    }
    Ok(())
}

/// Backup importer. Decrypts an age archive produced by
/// [`BackupExporter`] and writes the recovered database to a
/// staging location.
pub struct BackupImporter {
    staging_root: PathBuf,
}

impl BackupImporter {
    /// Build a new importer rooted at `staging_root`. The importer
    /// will write scratch files under
    /// `<staging_root>/.hivegui-db-staging-v1/restore-<UUID>/`.
    pub fn new(staging_root: impl Into<PathBuf>) -> Self {
        Self {
            staging_root: staging_root.into(),
        }
    }

    /// Staging root path.
    pub fn staging_root(&self) -> &Path {
        &self.staging_root
    }

    /// Decrypt the age stream and write the database to
    /// `<final_target>/datasources.db`. The final target directory
    /// is created if missing.
    pub async fn import_age(
        &self,
        archive: &Path,
        passphrase: &str,
        final_target: impl Into<PathBuf>,
    ) -> Result<PathBuf, ImportError> {
        let final_target = final_target.into();
        self.import_age_with_key_root(archive, passphrase, final_target.clone(), final_target)
            .await
    }

    async fn import_age_with_key_root(
        &self,
        archive: &Path,
        passphrase: &str,
        final_target: PathBuf,
        target_key_root: PathBuf,
    ) -> Result<PathBuf, ImportError> {
        let archive = Arc::new(VerifiedArchiveBinding::open(archive)?);
        self.import_age_bound_with_key_root(archive, passphrase, final_target, target_key_root)
            .await
    }

    async fn import_age_bound_with_key_root(
        &self,
        archive: Arc<VerifiedArchiveBinding>,
        passphrase: &str,
        final_target: PathBuf,
        target_key_root: PathBuf,
    ) -> Result<PathBuf, ImportError> {
        let passphrase = passphrase.to_string();
        let staging_root = self.staging_root.clone();
        let imported = tokio::task::spawn_blocking(move || {
            import_blocking(
                archive.as_ref(),
                &passphrase,
                &final_target,
                &target_key_root,
                &staging_root,
            )
        })
        .await
        .map_err(|e| ImportError::Io(format!("join error: {e}")))??;
        if let Err(error) = finalize_portable_restore(&imported).await {
            if imported.final_target == imported.target_key_root && imported.final_target.exists() {
                remove_tree_no_follow_path(&imported.final_target)?;
            }
            return Err(error);
        }
        Ok(imported.final_database)
    }

    /// Decrypt the archive and return the manifest without writing
    /// the database. Used by callers that want to inspect the
    /// metadata before committing to a restore.
    pub async fn inspect_manifest(
        &self,
        archive: &Path,
        passphrase: &str,
    ) -> Result<BackupManifest, ImportError> {
        let archive = Arc::new(VerifiedArchiveBinding::open(archive)?);
        self.inspect_manifest_bound(archive, passphrase).await
    }

    async fn inspect_manifest_bound(
        &self,
        archive: Arc<VerifiedArchiveBinding>,
        passphrase: &str,
    ) -> Result<BackupManifest, ImportError> {
        let passphrase = passphrase.to_string();
        tokio::task::spawn_blocking(move || {
            inspect_manifest_blocking(archive.as_ref(), &passphrase)
        })
        .await
        .map_err(|e| ImportError::Io(format!("join error: {e}")))?
    }
}

fn decrypt_age_streaming<R: Read>(archive: R, passphrase: &str) -> Result<impl Read, ImportError> {
    let decryptor = match Decryptor::new(archive) {
        Ok(d) => d,
        Err(_) => return Err(ImportError::AuthenticationFailed),
    };
    let passphrase_secret = SecretString::new(passphrase.to_string().into_boxed_str());
    let identity = age::scrypt::Identity::new(passphrase_secret);
    {
        let _guard = age_kdf_lock()
            .lock()
            .map_err(|_| ImportError::AuthenticationFailed)?;
        match decryptor.decrypt(std::iter::once(&identity as &dyn age::Identity)) {
            Ok(reader) => Ok(reader),
            Err(_) => Err(ImportError::AuthenticationFailed),
        }
    }
}

struct VerifiedArchiveBinding {
    archive: PathBuf,
    parent: cap_std::fs::Dir,
    name: PathBuf,
    file: Mutex<File>,
    metadata: fs::Metadata,
    sha256: String,
}

impl std::fmt::Debug for VerifiedArchiveBinding {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("VerifiedArchiveBinding")
            .field("archive", &self.archive)
            .field("metadata", &self.metadata)
            .field("sha256", &self.sha256)
            .finish_non_exhaustive()
    }
}

impl VerifiedArchiveBinding {
    fn open(archive: &Path) -> Result<Self, ImportError> {
        let parent_path = archive
            .parent()
            .ok_or_else(|| ImportError::UnsafeArchiveEntry(archive.display().to_string()))?;
        let name = archive
            .file_name()
            .ok_or_else(|| ImportError::UnsafeArchiveEntry(archive.display().to_string()))?;
        let parent = open_ambient_directory_nofollow(parent_path)?;
        let entry = parent
            .symlink_metadata(Path::new(name))
            .map_err(|error| ImportError::Io(error.to_string()))?;
        if !entry.is_file() || entry.file_type().is_symlink() {
            return Err(ImportError::UnsafeArchiveEntry(
                archive.display().to_string(),
            ));
        }
        let mut options = cap_std::fs::OpenOptions::new();
        options.read(true);
        #[cfg(unix)]
        {
            use cap_std::fs::OpenOptionsExt as _;
            options.custom_flags(libc::O_NOFOLLOW | libc::O_CLOEXEC | libc::O_NONBLOCK);
        }
        let mut file = parent
            .open_with(Path::new(name), &options)
            .map(cap_std::fs::File::into_std)
            .map_err(|_| ImportError::UnsafeArchiveEntry(archive.display().to_string()))?;
        let before = file
            .metadata()
            .map_err(|_| ImportError::UnsafeArchiveEntry(archive.display().to_string()))?;
        if !before.is_file() {
            return Err(ImportError::UnsafeArchiveEntry(
                archive.display().to_string(),
            ));
        }
        #[cfg(unix)]
        {
            use std::os::unix::fs::MetadataExt as _;
            if before.nlink() != 1 {
                return Err(ImportError::UnsafeArchiveEntry(
                    archive.display().to_string(),
                ));
            }
        }
        let after = file
            .metadata()
            .map_err(|_| ImportError::UnsafeArchiveEntry(archive.display().to_string()))?;
        if !after.is_file() || after.len() != before.len() {
            return Err(ImportError::UnsafeArchiveEntry(
                archive.display().to_string(),
            ));
        }
        #[cfg(unix)]
        {
            use std::os::unix::fs::MetadataExt as _;
            if before.dev() != after.dev()
                || before.ino() != after.ino()
                || after.nlink() != 1
                || before.mtime() != after.mtime()
                || before.mtime_nsec() != after.mtime_nsec()
            {
                return Err(ImportError::UnsafeArchiveEntry(
                    archive.display().to_string(),
                ));
            }
        }
        let sha256 = sha256_open_file(&mut file)?;
        Ok(Self {
            archive: archive.to_path_buf(),
            parent,
            name: PathBuf::from(name),
            file: Mutex::new(file),
            metadata: after,
            sha256,
        })
    }

    fn verified_reader(&self) -> Result<std::sync::MutexGuard<'_, File>, ImportError> {
        let mut file = self
            .file
            .lock()
            .map_err(|_| ImportError::UnsafeArchiveEntry(self.archive.display().to_string()))?;
        self.verify_held_open_file(&mut file)?;
        file.seek(SeekFrom::Start(0))
            .map_err(|_| ImportError::UnsafeArchiveEntry(self.archive.display().to_string()))?;
        Ok(file)
    }

    fn verify_held_open_file(&self, file: &mut File) -> Result<(), ImportError> {
        let before = file
            .metadata()
            .map_err(|_| ImportError::UnsafeArchiveEntry(self.archive.display().to_string()))?;
        self.verify_held_metadata(&before)?;
        let sha256 = sha256_open_file(file)?;
        let after = file
            .metadata()
            .map_err(|_| ImportError::UnsafeArchiveEntry(self.archive.display().to_string()))?;
        self.verify_held_metadata(&after)?;
        if stable_file_identity(&before)? != stable_file_identity(&after)?
            || before.len() != after.len()
            || sha256 != self.sha256
        {
            return Err(ImportError::UnsafeArchiveEntry(
                self.archive.display().to_string(),
            ));
        }
        Ok(())
    }

    fn verify_held_metadata(&self, metadata: &fs::Metadata) -> Result<(), ImportError> {
        if !metadata.is_file()
            || metadata.len() != self.metadata.len()
            || stable_file_identity(metadata)? != stable_file_identity(&self.metadata)?
        {
            return Err(ImportError::UnsafeArchiveEntry(
                self.archive.display().to_string(),
            ));
        }
        #[cfg(unix)]
        {
            use std::os::unix::fs::MetadataExt as _;
            if metadata.nlink() != 1
                || metadata.mtime() != self.metadata.mtime()
                || metadata.mtime_nsec() != self.metadata.mtime_nsec()
            {
                return Err(ImportError::UnsafeArchiveEntry(
                    self.archive.display().to_string(),
                ));
            }
        }
        #[cfg(not(unix))]
        if metadata.modified().ok() != self.metadata.modified().ok() {
            return Err(ImportError::UnsafeArchiveEntry(
                self.archive.display().to_string(),
            ));
        }
        Ok(())
    }

    fn verify_current(&self) -> Result<(), ImportError> {
        let file = self.verified_reader()?;
        let entry = self
            .parent
            .symlink_metadata(&self.name)
            .map_err(|_| ImportError::UnsafeArchiveEntry(self.archive.display().to_string()))?;
        let held = file
            .metadata()
            .map_err(|_| ImportError::UnsafeArchiveEntry(self.archive.display().to_string()))?;
        if !entry.is_file()
            || entry.file_type().is_symlink()
            || !held.is_file()
            || stable_file_identity(&held)? != stable_file_identity(&self.metadata)?
            || entry.len() != self.metadata.len()
            || held.len() != self.metadata.len()
        {
            return Err(ImportError::UnsafeArchiveEntry(
                self.archive.display().to_string(),
            ));
        }
        #[cfg(unix)]
        {
            use cap_std::fs::MetadataExt as _;
            use std::os::unix::fs::MetadataExt as _;
            if entry.dev() != self.metadata.dev()
                || entry.ino() != self.metadata.ino()
                || entry.nlink() != 1
                || held.nlink() != 1
                || entry.mtime() != self.metadata.mtime()
                || entry.mtime_nsec() != self.metadata.mtime_nsec()
                || held.mtime() != self.metadata.mtime()
                || held.mtime_nsec() != self.metadata.mtime_nsec()
            {
                return Err(ImportError::UnsafeArchiveEntry(
                    self.archive.display().to_string(),
                ));
            }
        }
        #[cfg(not(unix))]
        if entry
            .modified()
            .ok()
            .map(cap_std::time::SystemTime::into_std)
            != self.metadata.modified().ok()
        {
            return Err(ImportError::UnsafeArchiveEntry(
                self.archive.display().to_string(),
            ));
        }
        Ok(())
    }
}

fn sha256_open_file(file: &mut File) -> Result<String, ImportError> {
    let original_offset = file
        .stream_position()
        .map_err(|error| ImportError::Io(error.to_string()))?;
    file.seek(SeekFrom::Start(0))
        .map_err(|error| ImportError::Io(error.to_string()))?;
    let mut hasher = Sha256::new();
    let mut buffer = [0_u8; 64 * 1024];
    let hash_result = loop {
        let read = file
            .read(&mut buffer)
            .map_err(|error| ImportError::Io(error.to_string()));
        let read = match read {
            Ok(read) => read,
            Err(error) => break Err(error),
        };
        if read == 0 {
            break Ok(hex::encode(hasher.finalize()));
        }
        hasher.update(&buffer[..read]);
    };
    let restore_result = file
        .seek(SeekFrom::Start(original_offset))
        .map_err(|error| ImportError::Io(error.to_string()));
    match (hash_result, restore_result) {
        (Err(error), _) | (_, Err(error)) => Err(error),
        (Ok(hash), Ok(_)) => Ok(hash),
    }
}

fn read_control_file_nofollow(path: &Path) -> Result<Vec<u8>, ImportError> {
    const MAX_CONTROL_BYTES: u64 = 1024 * 1024;

    let parent_path = path
        .parent()
        .ok_or_else(|| ImportError::UnsafeArchiveEntry(path.display().to_string()))?;
    let name = path
        .file_name()
        .ok_or_else(|| ImportError::UnsafeArchiveEntry(path.display().to_string()))?;
    let parent = open_ambient_directory_nofollow(parent_path)?;
    let entry = parent
        .symlink_metadata(Path::new(name))
        .map_err(|_| ImportError::InvalidManifest("missing control file".into()))?;
    if !entry.is_file() || entry.file_type().is_symlink() {
        return Err(ImportError::UnsafeArchiveEntry(path.display().to_string()));
    }
    let mut options = cap_std::fs::OpenOptions::new();
    options.read(true);
    #[cfg(unix)]
    {
        use cap_std::fs::OpenOptionsExt as _;
        options.custom_flags(libc::O_NOFOLLOW | libc::O_CLOEXEC | libc::O_NONBLOCK);
    }
    let mut file = parent
        .open_with(Path::new(name), &options)
        .map(cap_std::fs::File::into_std)
        .map_err(|_| ImportError::UnsafeArchiveEntry(path.display().to_string()))?;
    let before = file
        .metadata()
        .map_err(|_| ImportError::UnsafeArchiveEntry(path.display().to_string()))?;
    if !before.is_file() || before.len() > MAX_CONTROL_BYTES {
        return Err(ImportError::UnsafeArchiveEntry(path.display().to_string()));
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::MetadataExt as _;
        if before.nlink() != 1 {
            return Err(ImportError::UnsafeArchiveEntry(path.display().to_string()));
        }
    }
    let mut bytes = Vec::with_capacity(before.len() as usize);
    Read::by_ref(&mut file)
        .take(MAX_CONTROL_BYTES + 1)
        .read_to_end(&mut bytes)
        .map_err(|_| ImportError::Io("read control file".into()))?;
    if bytes.len() as u64 > MAX_CONTROL_BYTES {
        return Err(ImportError::UnsafeArchiveEntry(path.display().to_string()));
    }
    let after = file
        .metadata()
        .map_err(|_| ImportError::UnsafeArchiveEntry(path.display().to_string()))?;
    if before.len() != after.len() {
        return Err(ImportError::UnsafeArchiveEntry(path.display().to_string()));
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::MetadataExt as _;
        if before.dev() != after.dev()
            || before.ino() != after.ino()
            || after.nlink() != 1
            || before.mtime() != after.mtime()
            || before.mtime_nsec() != after.mtime_nsec()
        {
            return Err(ImportError::UnsafeArchiveEntry(path.display().to_string()));
        }
    }
    Ok(bytes)
}

struct ParsedArchive {
    manifest: BackupManifest,
    database: Option<Vec<u8>>,
    entities: Vec<PortableEntityBody>,
    plugins: Vec<(i64, Vec<u8>)>,
}

struct BoundedArchiveReader<R> {
    inner: R,
    remaining: u64,
}

impl<R> BoundedArchiveReader<R> {
    fn new(inner: R) -> Self {
        Self {
            inner,
            remaining: MAX_AUTHENTICATED_ARCHIVE_BYTES,
        }
    }

    fn into_inner(self) -> R {
        self.inner
    }
}

impl<R: Read> Read for BoundedArchiveReader<R> {
    fn read(&mut self, buffer: &mut [u8]) -> std::io::Result<usize> {
        if buffer.is_empty() {
            return Ok(0);
        }
        let allowed =
            usize::try_from(self.remaining.min(buffer.len() as u64)).unwrap_or(buffer.len());
        if allowed == 0 {
            let mut probe = [0u8; 1];
            return match self.inner.read(&mut probe)? {
                0 => Ok(0),
                _ => Err(std::io::Error::new(
                    std::io::ErrorKind::InvalidData,
                    "decompressed archive exceeds the 1 GiB portable limit",
                )),
            };
        }
        let read = self.inner.read(&mut buffer[..allowed])?;
        self.remaining = self.remaining.saturating_sub(read as u64);
        Ok(read)
    }
}

fn decompress_and_parse<R: Read>(reader: R) -> Result<ParsedArchive, ImportError> {
    let decoder = flate2::bufread::GzDecoder::new(BufReader::new(reader));
    let mut archive = tar::Archive::new(BoundedArchiveReader::new(decoder));
    let mut manifest: Option<BackupManifest> = None;
    let mut database: Option<Vec<u8>> = None;
    let mut entities = BTreeMap::new();
    let mut plugins = BTreeMap::new();
    let mut archive_paths = BTreeSet::new();
    for entry in archive
        .entries()
        .map_err(|_| ImportError::Age("authenticated archive read failed".into()))?
    {
        let mut entry =
            entry.map_err(|_| ImportError::Age("authenticated archive read failed".into()))?;
        let header = entry.header().clone();
        let entry_type = header.entry_type();
        if entry_type.is_symlink() || entry_type.is_hard_link() {
            return Err(ImportError::UnsafeArchiveEntry(format!(
                "{:?}",
                header.path().ok()
            )));
        }
        if !entry_type.is_file() {
            return Err(ImportError::UnsafeArchiveEntry(format!(
                "{:?}",
                header.path().ok()
            )));
        }
        let path = header
            .path()
            .map_err(|e| ImportError::Io(format!("tar path: {e}")))?;
        let path_str = path
            .to_str()
            .ok_or_else(|| ImportError::UnsafeArchiveEntry("non-UTF-8 archive path".into()))?
            .to_string();
        if !archive_paths.insert(path_str.clone()) {
            return Err(ImportError::InvalidManifest(format!(
                "duplicate archive path {path_str}"
            )));
        }
        let entry_limit = if path_str == DATABASE_FILENAME {
            MAX_AUTHENTICATED_ARCHIVE_BYTES
        } else {
            MAX_ENTITY_FILE_BYTES as u64
        };
        if entry.size() > entry_limit {
            return Err(ImportError::InvalidManifest(format!(
                "archive entry exceeds its portable limit: {path_str}"
            )));
        }
        let mut buf = Vec::with_capacity(
            usize::try_from(entry.size())
                .map_err(|_| ImportError::InvalidManifest("archive entry size overflow".into()))?,
        );
        entry
            .read_to_end(&mut buf)
            .map_err(|_| ImportError::Age("authenticated archive read failed".into()))?;
        if path_str == MANIFEST_FILENAME {
            let m: BackupManifest = serde_json::from_slice(&buf)
                .map_err(|e| ImportError::InvalidManifest(e.to_string()))?;
            manifest = Some(m);
        } else if path_str == DATABASE_FILENAME {
            database = Some(buf);
        } else if path_str.starts_with("entities/") && path_str.ends_with(".json") {
            entities.insert(path_str, buf);
        } else if let Some(relative) = path_str.strip_prefix(PLUGIN_ARCHIVE_PREFIX) {
            let relative = PathBuf::from(relative);
            validate_relative_path(&relative).map_err(|_| {
                ImportError::UnsafeArchiveEntry("invalid plugin artifact path".into())
            })?;
            plugins.insert(path_str, buf);
        } else {
            return Err(ImportError::UnsafeArchiveEntry(path_str));
        }
    }
    let bounded_decoder = archive.into_inner();
    let decoder = bounded_decoder.into_inner();
    let mut authenticated_reader = decoder.into_inner();
    std::io::copy(&mut authenticated_reader, &mut std::io::sink())
        .map_err(|_| ImportError::Age("authenticated archive tail failed".into()))?;

    let manifest = manifest.ok_or_else(|| {
        ImportError::InvalidManifest("manifest.json missing from archive".to_string())
    })?;
    if manifest.schema_version != ARCHIVE_SCHEMA_VERSION
        || manifest.format != PORTABLE_FORMAT_NAME
        || chrono::DateTime::parse_from_rfc3339(&manifest.exported_at).is_err()
    {
        return Err(ImportError::InvalidManifest(
            "backup manifest identity mismatch".into(),
        ));
    }
    if manifest.format_version < *ALLOWED_FORMATS.first().unwrap_or(&1) {
        return Err(ImportError::BackupTooOld);
    }
    if manifest.format_version > *ALLOWED_FORMATS.last().unwrap_or(&3) {
        return Err(ImportError::BackupNewerVersion);
    }
    let expected_names = ENTITY_TABLES
        .iter()
        .map(|name| (*name).to_string())
        .collect::<BTreeSet<_>>();
    let descriptor_names = manifest
        .entity_files
        .iter()
        .map(|entry| entry.name.clone())
        .collect::<BTreeSet<_>>();
    let entity_mode = database.is_none();
    if entity_mode && descriptor_names != expected_names {
        return Err(ImportError::InvalidManifest(
            "portable entity inventory mismatch".into(),
        ));
    }
    if !entity_mode && (!manifest.entity_files.is_empty() || !entities.is_empty()) {
        return Err(ImportError::InvalidManifest(
            "mixed database and entity payload".into(),
        ));
    }
    let mut parsed_entities = Vec::with_capacity(manifest.entity_files.len());
    let mut described_entity_paths = BTreeSet::new();
    for descriptor in &manifest.entity_files {
        if !expected_names.contains(&descriptor.name)
            || descriptor.path != format!("entities/{}.json", descriptor.name)
            || !described_entity_paths.insert(descriptor.path.clone())
        {
            return Err(ImportError::InvalidManifest(
                "invalid entity descriptor".into(),
            ));
        }
        let bytes = entities
            .get(&descriptor.path)
            .ok_or_else(|| ImportError::InvalidManifest("entity file missing".into()))?;
        if bytes.len() > MAX_ENTITY_FILE_BYTES
            || hex::encode(Sha256::digest(bytes)) != descriptor.sha256
        {
            return Err(ImportError::InvalidManifest(
                "entity file digest mismatch".into(),
            ));
        }
        let body: PortableEntityBody = serde_json::from_slice(bytes)
            .map_err(|_| ImportError::InvalidManifest("invalid entity JSON".into()))?;
        if body.schema_version != ARCHIVE_SCHEMA_VERSION
            || body.entity != descriptor.name
            || body.rows.len() as u64 != descriptor.count
            || body.columns.is_empty()
            || body.columns.iter().collect::<BTreeSet<_>>().len() != body.columns.len()
            || body.rows.iter().any(|row| row.len() != body.columns.len())
        {
            return Err(ImportError::InvalidManifest("entity body mismatch".into()));
        }
        parsed_entities.push(body);
    }
    if described_entity_paths != entities.keys().cloned().collect() {
        return Err(ImportError::InvalidManifest(
            "unmanifested entity file".into(),
        ));
    }
    validate_and_upgrade_portable_entities(manifest.format_version, &mut parsed_entities)?;
    let mut parsed_plugins = Vec::with_capacity(manifest.artifacts.len());
    let mut described_plugin_paths = BTreeSet::new();
    let mut plugin_ids = BTreeSet::new();
    for descriptor in &manifest.artifacts {
        if descriptor.plugin_id <= 0
            || descriptor.path
                != format!(
                    "{PLUGIN_ARCHIVE_PREFIX}{}/plugin.wasm",
                    descriptor.plugin_id
                )
            || !plugin_ids.insert(descriptor.plugin_id)
            || !described_plugin_paths.insert(descriptor.path.clone())
        {
            return Err(ImportError::InvalidManifest(
                "invalid Plugin artifact descriptor".into(),
            ));
        }
        let bytes = plugins
            .get(&descriptor.path)
            .ok_or_else(|| ImportError::InvalidManifest("Plugin artifact missing".into()))?;
        if bytes.len() as u64 != descriptor.size_bytes
            || hex::encode(Sha256::digest(bytes)) != descriptor.sha256
        {
            return Err(ImportError::InvalidManifest(
                "Plugin artifact digest mismatch".into(),
            ));
        }
        parsed_plugins.push((descriptor.plugin_id, bytes.clone()));
    }
    if described_plugin_paths != plugins.keys().cloned().collect() {
        return Err(ImportError::InvalidManifest(
            "unmanifested Plugin artifact".into(),
        ));
    }
    Ok(ParsedArchive {
        manifest,
        database,
        entities: parsed_entities,
        plugins: parsed_plugins,
    })
}

fn validate_and_upgrade_portable_entities(
    format_version: u32,
    entities: &mut [PortableEntityBody],
) -> Result<(), ImportError> {
    let functions = entities
        .iter_mut()
        .find(|entity| entity.entity == "functions")
        .ok_or_else(|| ImportError::InvalidManifest("functions entity missing".into()))?;
    let identifier_index = portable_column_index(functions, "identifier")?;
    let kind_index = portable_column_index(functions, "kind")?;
    let plugin_index = portable_column_index(functions, "plugin_id")?;
    let export_index = portable_column_index(functions, "plugin_export")?;
    let capabilities_index = portable_column_index(functions, "required_capabilities")?;
    let mut identifiers = BTreeSet::new();
    for row in &mut functions.rows {
        if format_version < 3 {
            row[kind_index] = match &row[kind_index] {
                PortableValue::Integer(1) => PortableValue::Text("builtin".into()),
                PortableValue::Integer(2) => PortableValue::Text("custom".into()),
                PortableValue::Integer(3) => PortableValue::Text("placeholder".into()),
                PortableValue::Text(value)
                    if matches!(value.as_str(), "builtin" | "custom" | "placeholder") =>
                {
                    PortableValue::Text(value.clone())
                }
                _ => {
                    return Err(ImportError::InvalidManifest(
                        "unknown legacy Function kind".into(),
                    ));
                }
            };
        }
        let kind = portable_text(&row[kind_index], "Function.kind")?.to_string();
        if !matches!(kind.as_str(), "builtin" | "custom" | "placeholder") {
            return Err(ImportError::InvalidManifest(
                "invalid current Function kind".into(),
            ));
        }
        let identifier = portable_text(&row[identifier_index], "Function.identifier")?.to_string();
        let normalized = if kind == "builtin" {
            let mapped = match identifier.as_str() {
                "format.template" if format_version < 3 => "format_template",
                "json.parse" if format_version < 3 => "json_parse",
                "json.stringify" if format_version < 3 => "json_stringify",
                "text.regex_match" if format_version < 3 => "text_regex_match",
                "format_template" | "json_parse" | "json_stringify" | "text_regex_match" => {
                    identifier.as_str()
                }
                _ => {
                    return Err(ImportError::InvalidManifest(
                        "invalid Builtin identifier".into(),
                    ));
                }
            };
            row[identifier_index] = PortableValue::Text(mapped.to_string());
            mapped.to_string()
        } else {
            identifier
        };
        if !identifiers.insert(normalized) {
            return Err(ImportError::InvalidManifest(
                "legacy Builtin identifier collision".into(),
            ));
        }
        if kind == "placeholder"
            && (!matches!(row[plugin_index], PortableValue::Null)
                || !matches!(row[export_index], PortableValue::Null)
                || !matches!(row[capabilities_index], PortableValue::Null))
        {
            return Err(ImportError::InvalidManifest(
                "Placeholder execution fields must be empty".into(),
            ));
        }
    }

    let nodes = entities
        .iter()
        .find(|entity| entity.entity == "workflow_nodes")
        .ok_or_else(|| ImportError::InvalidManifest("workflow_nodes entity missing".into()))?;
    let node_type_index = portable_column_index(nodes, "node_type")?;
    for row in &nodes.rows {
        if !matches!(
            portable_text(&row[node_type_index], "WorkflowNode.node_type")?,
            "start_node" | "end_node" | "function_node" | "generate_answer_node"
        ) {
            return Err(ImportError::InvalidManifest(
                "invalid WorkflowNode.node_type".into(),
            ));
        }
    }

    let tools = entities
        .iter_mut()
        .find(|entity| entity.entity == "tools")
        .ok_or_else(|| ImportError::InvalidManifest("tools entity missing".into()))?;
    let tool_kind_index = portable_column_index(tools, "kind")?;
    for row in &mut tools.rows {
        if format_version < 3 {
            row[tool_kind_index] = match &row[tool_kind_index] {
                PortableValue::Integer(1) => PortableValue::Text("function-wrap".into()),
                PortableValue::Integer(2) => PortableValue::Text("workflow-wrap".into()),
                PortableValue::Text(value)
                    if matches!(value.as_str(), "function-wrap" | "workflow-wrap") =>
                {
                    PortableValue::Text(value.clone())
                }
                _ => {
                    return Err(ImportError::InvalidManifest(
                        "unknown legacy Tool kind".into(),
                    ));
                }
            };
        }
        if !matches!(
            portable_text(&row[tool_kind_index], "Tool.kind")?,
            "function-wrap" | "workflow-wrap"
        ) {
            return Err(ImportError::InvalidManifest(
                "invalid current Tool kind".into(),
            ));
        }
    }

    let presets = entities
        .iter()
        .find(|entity| entity.entity == "llm_presets")
        .ok_or_else(|| ImportError::InvalidManifest("llm_presets entity missing".into()))?;
    let preset_name_index = portable_column_index(presets, "name")?;
    let preset_names = presets
        .rows
        .iter()
        .map(|row| portable_text(&row[preset_name_index], "LlmPreset.name"))
        .collect::<Result<BTreeSet<_>, _>>()?;
    let agents = entities
        .iter()
        .find(|entity| entity.entity == "agents")
        .ok_or_else(|| ImportError::InvalidManifest("agents entity missing".into()))?;
    let model_preset_index = portable_column_index(agents, "model_preset")?;
    for row in &agents.rows {
        if let PortableValue::Text(name) = &row[model_preset_index]
            && !name.is_empty()
            && !preset_names.contains(name.as_str())
        {
            return Err(ImportError::InvalidManifest(
                "Agent model_preset is dangling".into(),
            ));
        }
    }
    Ok(())
}

fn portable_column_index(entity: &PortableEntityBody, column: &str) -> Result<usize, ImportError> {
    entity
        .columns
        .iter()
        .position(|candidate| candidate == column)
        .ok_or_else(|| ImportError::InvalidManifest(format!("missing {}.{column}", entity.entity)))
}

fn portable_text<'a>(value: &'a PortableValue, field: &str) -> Result<&'a str, ImportError> {
    match value {
        PortableValue::Text(value) => Ok(value),
        _ => Err(ImportError::InvalidManifest(format!(
            "{field} has the wrong type"
        ))),
    }
}

fn is_canonical_uuid(value: &str) -> bool {
    Uuid::parse_str(value).is_ok_and(|uuid| uuid.hyphenated().to_string() == value)
}

fn upgrade_to_current_format(mut manifest: BackupManifest) -> BackupManifest {
    // The transparent format upgrade: every accepted format is
    // materialised as format 1 in the staging database.
    manifest.format_version = 3;
    manifest
}

fn open_ambient_directory_nofollow(path: &Path) -> Result<cap_std::fs::Dir, ImportError> {
    #[cfg(unix)]
    {
        let absolute = if path.is_absolute() {
            path.to_path_buf()
        } else {
            std::env::current_dir()
                .map_err(|error| ImportError::Io(error.to_string()))?
                .join(path)
        };
        let names = absolute
            .components()
            .filter_map(|component| match component {
                std::path::Component::Normal(name) => Some(name.to_os_string()),
                _ => None,
            })
            .collect::<Vec<_>>();
        #[cfg(target_os = "linux")]
        let (mut directory, start) = if names.len() >= 4
            && names[0] == "proc"
            && names[1] == "self"
            && names[2] == "fd"
            && names[3]
                .to_string_lossy()
                .bytes()
                .all(|byte| byte.is_ascii_digit())
        {
            let descriptor_root = Path::new("/proc/self/fd").join(&names[3]);
            (
                cap_std::fs::Dir::open_ambient_dir(descriptor_root, cap_std::ambient_authority())
                    .map_err(|_| ImportError::UnsafeArchiveEntry(path.display().to_string()))?,
                4,
            )
        } else {
            (
                cap_std::fs::Dir::open_ambient_dir("/", cap_std::ambient_authority())
                    .map_err(|_| ImportError::UnsafeArchiveEntry(path.display().to_string()))?,
                0,
            )
        };
        #[cfg(not(target_os = "linux"))]
        let (mut directory, start) = (
            cap_std::fs::Dir::open_ambient_dir("/", cap_std::ambient_authority())
                .map_err(|_| ImportError::UnsafeArchiveEntry(path.display().to_string()))?,
            0,
        );
        for name in &names[start..] {
            let relative = Path::new(name);
            let metadata = directory
                .symlink_metadata(relative)
                .map_err(|_| ImportError::UnsafeArchiveEntry(path.display().to_string()))?;
            if !metadata.is_dir() || metadata.file_type().is_symlink() {
                return Err(ImportError::UnsafeArchiveEntry(path.display().to_string()));
            }
            directory = directory
                .open_dir(relative)
                .map_err(|_| ImportError::UnsafeArchiveEntry(path.display().to_string()))?;
        }
        Ok(directory)
    }
    #[cfg(not(unix))]
    {
        let metadata =
            fs::symlink_metadata(path).map_err(|error| ImportError::Io(error.to_string()))?;
        if !metadata.is_dir() || metadata.file_type().is_symlink() {
            return Err(ImportError::UnsafeArchiveEntry(path.display().to_string()));
        }
        cap_std::fs::Dir::open_ambient_dir(path, cap_std::ambient_authority())
            .map_err(|_| ImportError::UnsafeArchiveEntry(path.display().to_string()))
    }
}

fn open_or_create_child_directory(
    parent_path: &Path,
    child_name: &Path,
) -> Result<cap_std::fs::Dir, ImportError> {
    validate_relative_path(child_name)
        .map_err(|_| ImportError::UnsafeArchiveEntry(child_name.display().to_string()))?;
    if child_name.components().count() != 1 {
        return Err(ImportError::UnsafeArchiveEntry(
            child_name.display().to_string(),
        ));
    }
    let parent = open_ambient_directory_nofollow(parent_path)?;
    match parent.symlink_metadata(child_name) {
        Ok(metadata) => {
            if !metadata.is_dir() || metadata.file_type().is_symlink() {
                return Err(ImportError::UnsafeArchiveEntry(
                    parent_path.join(child_name).display().to_string(),
                ));
            }
        }
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => parent
            .create_dir(child_name)
            .map_err(|error| ImportError::Io(error.to_string()))?,
        Err(error) => return Err(ImportError::Io(error.to_string())),
    }
    parent.open_dir(child_name).map_err(|_| {
        ImportError::UnsafeArchiveEntry(parent_path.join(child_name).display().to_string())
    })
}

fn sync_cap_directory(
    directory: &cap_std::fs::Dir,
    context: &'static str,
) -> Result<(), ImportError> {
    #[cfg(unix)]
    {
        use std::os::fd::{AsRawFd as _, FromRawFd as _};

        // cap-std may keep an O_PATH-style descriptor for a directory. Open
        // `.` relative to that stable handle to obtain an fsync-capable fd
        // without resolving the ambient path again.
        let dot = c".";
        // SAFETY: the parent descriptor and static C string are valid; the
        // returned descriptor is immediately owned by `File` on success.
        let descriptor = unsafe {
            libc::openat(
                directory.as_raw_fd(),
                dot.as_ptr(),
                libc::O_RDONLY | libc::O_DIRECTORY | libc::O_NOFOLLOW | libc::O_CLOEXEC,
            )
        };
        if descriptor < 0 {
            return Err(ImportError::Io(format!(
                "{context}: {}",
                std::io::Error::last_os_error()
            )));
        }
        // SAFETY: `descriptor` is newly returned and uniquely owned here.
        let file = unsafe { File::from_raw_fd(descriptor) };
        file.sync_all()
            .map_err(|error| ImportError::Io(format!("{context}: {error}")))
    }
    #[cfg(not(unix))]
    {
        directory
            .try_clone()
            .map(cap_std::fs::Dir::into_std_file)
            .and_then(|file| file.sync_all())
            .map_err(|error| ImportError::Io(format!("{context}: {error}")))
    }
}

#[cfg(target_os = "linux")]
fn publish_no_replace_at(
    from_dir: &cap_std::fs::Dir,
    from: &Path,
    to_dir: &cap_std::fs::Dir,
    to: &Path,
) -> std::io::Result<()> {
    use std::ffi::CString;
    use std::os::fd::AsRawFd as _;
    use std::os::unix::ffi::OsStrExt as _;

    let from = CString::new(from.as_os_str().as_bytes())?;
    let to = CString::new(to.as_os_str().as_bytes())?;
    // SAFETY: both names are relative NUL-terminated strings and both
    // directory descriptors remain valid for the syscall.
    let result = unsafe {
        libc::renameat2(
            from_dir.as_raw_fd(),
            from.as_ptr(),
            to_dir.as_raw_fd(),
            to.as_ptr(),
            libc::RENAME_NOREPLACE,
        )
    };
    if result == 0 {
        Ok(())
    } else {
        Err(std::io::Error::last_os_error())
    }
}

#[cfg(not(target_os = "linux"))]
fn publish_no_replace_at(
    from_dir: &cap_std::fs::Dir,
    from: &Path,
    to_dir: &cap_std::fs::Dir,
    to: &Path,
) -> std::io::Result<()> {
    from_dir.hard_link(from, to_dir, to)?;
    from_dir.remove_file(from)
}

fn parse_bound_archive(
    archive: &VerifiedArchiveBinding,
    passphrase: &str,
) -> Result<ParsedArchive, ImportError> {
    let mut file = archive.verified_reader()?;
    let parsed = match decrypt_age_streaming(&mut *file, passphrase) {
        Ok(reader) => decompress_and_parse(reader),
        Err(error) => Err(error),
    };
    let post_read_verification = archive.verify_held_open_file(&mut file);
    match (parsed, post_read_verification) {
        (_, Err(error)) => Err(error),
        (Err(error), Ok(())) => Err(error),
        (Ok(parsed), Ok(())) => Ok(parsed),
    }
}

fn import_blocking(
    archive: &VerifiedArchiveBinding,
    passphrase: &str,
    final_target: &Path,
    target_key_root: &Path,
    staging_root: &Path,
) -> Result<ImportedRestore, ImportError> {
    let parsed = parse_bound_archive(archive, passphrase)?;
    let _manifest = upgrade_to_current_format(parsed.manifest);

    // Stage the database into <staging_root>/.hivegui-db-staging-v1/restore-<UUID>/datasources.db
    let instance_id = Uuid::new_v4();
    let staging_parent = staging_root
        .parent()
        .ok_or_else(|| ImportError::UnsafeArchiveEntry(staging_root.display().to_string()))?;
    let staging_name = staging_root
        .file_name()
        .ok_or_else(|| ImportError::UnsafeArchiveEntry(staging_root.display().to_string()))?;
    let staging_root_directory =
        open_or_create_child_directory(staging_parent, Path::new(staging_name))?;
    staging_root_directory
        .create_dir_all(".hivegui-db-staging-v1")
        .map_err(|error| ImportError::Io(format!("create staging registry: {error}")))?;
    let registry_directory = staging_root_directory
        .open_dir(".hivegui-db-staging-v1")
        .map_err(|_| {
            ImportError::UnsafeArchiveEntry(
                staging_root
                    .join(".hivegui-db-staging-v1")
                    .display()
                    .to_string(),
            )
        })?;
    let instance_name = format!("restore-{instance_id}");
    registry_directory
        .create_dir(&instance_name)
        .map_err(|error| ImportError::Io(format!("create restore instance: {error}")))?;
    let instance_directory = registry_directory
        .open_dir(&instance_name)
        .map_err(|_| ImportError::UnsafeArchiveEntry(instance_name.clone()))?;
    {
        let mut options = cap_std::fs::OpenOptions::new();
        options.write(true).create_new(true);
        let mut file = instance_directory
            .open_with(DATABASE_FILENAME, &options)
            .map_err(|error| ImportError::Io(format!("create staged database: {error}")))?;
        if let Some(database) = parsed.database.as_deref() {
            file.write_all(database)
                .map_err(|error| ImportError::Io(format!("write staged database: {error}")))?;
        }
        file.sync_all()
            .map_err(|error| ImportError::Io(format!("fsync staged database: {error}")))?;
    }

    // Move to the final target via no-replace rename.
    let final_parent = final_target
        .parent()
        .ok_or_else(|| ImportError::UnsafeArchiveEntry(final_target.display().to_string()))?;
    let final_name = final_target
        .file_name()
        .ok_or_else(|| ImportError::UnsafeArchiveEntry(final_target.display().to_string()))?;
    let final_directory = open_or_create_child_directory(final_parent, Path::new(final_name))?;
    let final_database = final_target.join(DATABASE_FILENAME);
    if final_directory.symlink_metadata(DATABASE_FILENAME).is_ok() {
        return Err(ImportError::InvalidManifest(format!(
            "{} already exists in final target",
            DATABASE_FILENAME
        )));
    }
    publish_no_replace_at(
        &instance_directory,
        Path::new(DATABASE_FILENAME),
        &final_directory,
        Path::new(DATABASE_FILENAME),
    )
    .map_err(|error| ImportError::Io(format!("publish staged database: {error}")))?;
    sync_cap_directory(&final_directory, "fsync final target")?;

    registry_directory
        .remove_dir(&instance_name)
        .map_err(|error| ImportError::Io(format!("remove restore instance: {error}")))?;
    sync_cap_directory(&registry_directory, "fsync staging registry")?;

    Ok(ImportedRestore {
        final_database,
        final_target: final_target.to_path_buf(),
        target_key_root: target_key_root.to_path_buf(),
        entities: parsed.entities,
        plugins: parsed.plugins,
        legacy_database: parsed.database.is_some(),
    })
}

fn inspect_manifest_blocking(
    archive: &VerifiedArchiveBinding,
    passphrase: &str,
) -> Result<BackupManifest, ImportError> {
    Ok(parse_bound_archive(archive, passphrase)?.manifest)
}

fn create_safe_directories(root: &Path, target: &Path) -> Result<(), ImportError> {
    let relative = target
        .strip_prefix(root)
        .map_err(|_| ImportError::UnsafeArchiveEntry(target.display().to_string()))?;
    validate_relative_path(relative)
        .map_err(|_| ImportError::UnsafeArchiveEntry(target.display().to_string()))?;
    let mut current = root.to_path_buf();
    fs::create_dir_all(&current).map_err(|error| ImportError::Io(error.to_string()))?;
    for component in relative.components() {
        current.push(component.as_os_str());
        match fs::symlink_metadata(&current) {
            Ok(metadata) if metadata.is_dir() && !metadata.file_type().is_symlink() => {}
            Ok(_) => {
                return Err(ImportError::UnsafeArchiveEntry(
                    current.display().to_string(),
                ));
            }
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                fs::create_dir(&current).map_err(|error| ImportError::Io(error.to_string()))?;
            }
            Err(error) => return Err(ImportError::Io(error.to_string())),
        }
    }
    Ok(())
}

struct ImportedRestore {
    final_database: PathBuf,
    final_target: PathBuf,
    target_key_root: PathBuf,
    entities: Vec<PortableEntityBody>,
    plugins: Vec<(i64, Vec<u8>)>,
    legacy_database: bool,
}

async fn finalize_portable_restore(imported: &ImportedRestore) -> Result<(), ImportError> {
    fs::create_dir_all(&imported.target_key_root)
        .map_err(|error| ImportError::Io(error.to_string()))?;
    let target_key_path = imported.target_key_root.join("encryption.key");
    let target_key = match fs::read(&target_key_path) {
        Ok(bytes) => bytes
            .as_slice()
            .try_into()
            .map_err(|_| ImportError::InvalidManifest("target device key length".into()))?,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            let key = super::crypto::Crypto::generate_key();
            let mut options = OpenOptions::new();
            options.create_new(true).write(true);
            #[cfg(unix)]
            {
                use std::os::unix::fs::OpenOptionsExt;
                options.mode(0o600);
            }
            let mut file = options
                .open(&target_key_path)
                .map_err(|error| ImportError::Io(error.to_string()))?;
            file.write_all(&key)
                .and_then(|_| file.sync_all())
                .map_err(|error| ImportError::Io(error.to_string()))?;
            key
        }
        Err(error) => return Err(ImportError::Io(error.to_string())),
    };
    let target_crypto = super::crypto::Crypto::new(&target_key);
    let restored_plugin_root = imported.final_target.join("plugins");
    super::migrations::migrate_to_current(super::migrations::MigrationOptions::new(
        &imported.final_database,
        &restored_plugin_root,
    ))
    .await
    .map_err(|_| ImportError::InvalidManifest("unsupported portable schema".into()))?;
    let options = sqlx::sqlite::SqliteConnectOptions::new()
        .filename(&imported.final_database)
        .create_if_missing(false)
        .foreign_keys(true);
    let pool = sqlx::sqlite::SqlitePoolOptions::new()
        .min_connections(1)
        .max_connections(1)
        .connect_with(options)
        .await
        .map_err(|_| ImportError::Io("open staged portable database".into()))?;
    super::entity_store::run_migrations(&pool)
        .await
        .map_err(|_| ImportError::InvalidManifest("portable entity schema migration".into()))?;
    let mut transaction = pool
        .begin()
        .await
        .map_err(|_| ImportError::Io("begin portable re-encryption".into()))?;
    if imported.legacy_database {
        return Err(ImportError::InvalidManifest(
            "legacy SQLite payload requires a versioned entity upgrader".into(),
        ));
    }
    import_entity_files(&mut transaction, &imported.entities, &target_crypto).await?;
    materialize_restored_artifacts(&mut transaction, &imported.final_target, &imported.plugins)
        .await?;
    validate_restored_artifacts(&mut transaction, &imported.final_target.join("plugins")).await?;
    sqlx::query("DELETE FROM plugin_artifact_gc")
        .execute(&mut *transaction)
        .await
        .map_err(|_| ImportError::Io("clear portable GC ledger".into()))?;
    sqlx::query("DELETE FROM plugin_artifact_operations")
        .execute(&mut *transaction)
        .await
        .map_err(|_| ImportError::Io("clear portable operation ledger".into()))?;
    transaction
        .commit()
        .await
        .map_err(|_| ImportError::Io("commit portable re-encryption".into()))?;
    super::migrations::rebuild_search_documents_for_restore(&pool)
        .await
        .map_err(|_| ImportError::Io("rebuild portable search index".into()))?;
    super::migrations::verify_sqlite_health(&pool)
        .await
        .map_err(|_| ImportError::InvalidManifest("portable SQLite health".into()))?;
    super::migrations::verify_schema(&pool)
        .await
        .map_err(|_| ImportError::InvalidManifest("portable schema drift".into()))?;
    pool.close().await;
    Ok(())
}

async fn import_entity_files(
    transaction: &mut sqlx::Transaction<'_, sqlx::Sqlite>,
    entities: &[PortableEntityBody],
    target_crypto: &super::crypto::Crypto,
) -> Result<(), ImportError> {
    let by_name = entities
        .iter()
        .map(|entity| (entity.entity.as_str(), entity))
        .collect::<BTreeMap<_, _>>();
    if by_name.len() != ENTITY_TABLES.len() {
        return Err(ImportError::InvalidManifest(
            "portable entity inventory mismatch".into(),
        ));
    }
    sqlx::query("PRAGMA defer_foreign_keys=ON")
        .execute(&mut **transaction)
        .await
        .map_err(|_| ImportError::Io("defer portable foreign keys".into()))?;
    for table in ENTITY_TABLES {
        let entity = by_name
            .get(table)
            .ok_or_else(|| ImportError::InvalidManifest("portable entity missing".into()))?;
        if entity.columns
            != entity_columns(table)
                .iter()
                .map(|column| (*column).to_string())
                .collect::<Vec<_>>()
        {
            return Err(ImportError::InvalidManifest(format!(
                "portable entity schema mismatch: {table}"
            )));
        }
        if entity.rows.is_empty() {
            continue;
        }
        for row in &entity.rows {
            execute_portable_insert(transaction, table, &entity.columns, row, target_crypto)
                .await?;
        }
    }
    Ok(())
}

async fn execute_portable_insert(
    transaction: &mut sqlx::Transaction<'_, sqlx::Sqlite>,
    table: &str,
    columns: &[String],
    row: &[PortableValue],
    target_crypto: &super::crypto::Crypto,
) -> Result<(), ImportError> {
    let sql: &'static str = match table {
        "categories" => {
            "INSERT INTO categories (id,parent_id,name,slug,description,created_at,updated_at) VALUES (?,?,?,?,?,?,?)"
        }
        "capabilities" => {
            "INSERT INTO capabilities (name,description,is_dangerous,category_id,normalized_name,created_at) VALUES (?,?,?,?,?,?)"
        }
        "global_configs" => {
            "INSERT INTO global_configs (id,name,key,type,data,created_at,updated_at) VALUES (?,?,?,?,?,?,?)"
        }
        "llm_presets" => {
            "INSERT INTO llm_presets (id,name,description,is_default,max_tokens,temperature,created_at,updated_at) VALUES (?,?,?,?,?,?,?,?)"
        }
        "llm_providers" => {
            "INSERT INTO llm_providers (id,name,category,base_url,token_env,token_encrypted,created_at,updated_at) VALUES (?,?,?,?,?,?,?,?)"
        }
        "models" => {
            "INSERT INTO models (id,name,preset_id,provider_id,priority,created_at,updated_at) VALUES (?,?,?,?,?,?,?)"
        }
        "tags" => {
            "INSERT INTO tags (id,name,color,normalized_name,created_at,updated_at) VALUES (?,?,?,?,?,?)"
        }
        "data_sources" => {
            "INSERT INTO data_sources (id,name,host,port,username,encrypted_password,created_at,updated_at) VALUES (?,?,?,?,?,?,?,?)"
        }
        "plugins" => {
            "INSERT INTO plugins (id,identifier,name,description,manifest,version,author,repository_url,s3_key,sha256,size_bytes,runtime,category_id,capabilities,resource_limits,row_revision,created_at,updated_at,deleted_at) VALUES (?,?,?,?,?,?,?,?,?,?,?,?,?,?,?,?,?,?,?)"
        }
        "functions" => {
            "INSERT INTO functions (id,identifier,name,description,kind,input_schema,output_schema,plugin_id,plugin_export,category_id,required_capabilities,created_at,updated_at) VALUES (?,?,?,?,?,?,?,?,?,?,?,?,?)"
        }
        "workflows" => {
            "INSERT INTO workflows (id,identifier,name,description,timeout_ms,category_id,input_schema,start_description,output_schema,required_capabilities,created_at,updated_at) VALUES (?,?,?,?,?,?,?,?,?,?,?,?)"
        }
        "workflow_nodes" => {
            "INSERT INTO workflow_nodes (id,workflow_id,node_key,node_type,function_id,position_x,position_y,node_config,created_at) VALUES (?,?,?,?,?,?,?,?,?)"
        }
        "workflow_edges" => {
            "INSERT INTO workflow_edges (id,workflow_id,src_node_key,dst_node_key,mapping) VALUES (?,?,?,?,?)"
        }
        "tools" => {
            "INSERT INTO tools (id,identifier,name,description,kind,source,is_always,function_id,workflow_id,input_schema,output_schema,category_id,required_capabilities,created_at,updated_at) VALUES (?,?,?,?,?,?,?,?,?,?,?,?,?,?,?)"
        }
        "skills" => {
            "INSERT INTO skills (id,identifier,name,description,frontmatter,content,source,is_always,category_id,required_capabilities,created_at,updated_at) VALUES (?,?,?,?,?,?,?,?,?,?,?,?)"
        }
        "agents" => {
            "INSERT INTO agents (id,identifier,name,description,system_prompt,parent_agent_id,depth,is_default,model_preset,category_id,name_normalized,created_at,updated_at) VALUES (?,?,?,?,?,?,?,?,?,?,?,?,?)"
        }
        "agent_tools" => "INSERT INTO agent_tools (agent_id,tool_id,created_at) VALUES (?,?,?)",
        "agent_skills" => "INSERT INTO agent_skills (agent_id,skill_id,created_at) VALUES (?,?,?)",
        "agent_capabilities" => {
            "INSERT INTO agent_capabilities (agent_id,capability_name,created_at) VALUES (?,?,?)"
        }
        "chat_sessions" => {
            "INSERT INTO chat_sessions (id,entry_agent_id,current_agent_id,title_encrypted,status,execution_id,created_at,updated_at,expires_at) VALUES (?,?,?,?,?,?,?,?,?)"
        }
        "chat_messages" => {
            "INSERT INTO chat_messages (id,session_id,seq,role,content_encrypted,tool_calls_encrypted,created_at) VALUES (?,?,?,?,?,?,?)"
        }
        "agent_executions" => {
            "INSERT INTO agent_executions (execution_id,session_id,current_agent_id,status,state_encrypted,started_at,finished_at,error_kind) VALUES (?,?,?,?,?,?,?,?)"
        }
        _ => {
            return Err(ImportError::InvalidManifest(
                "unknown portable entity".into(),
            ));
        }
    };
    let mut query = sqlx::query(sql);
    for (column, value) in columns.iter().zip(row) {
        query = match value {
            PortableValue::Null => query.bind(Option::<Vec<u8>>::None),
            PortableValue::Integer(value) => query.bind(*value),
            PortableValue::Real(value) => query.bind(*value),
            PortableValue::Text(value) => query.bind(value),
            PortableValue::Blob(value) => {
                let mut bytes = hex::decode(value).map_err(|_| {
                    ImportError::InvalidManifest(format!("invalid portable blob: {table}.{column}"))
                })?;
                if is_sensitive_entity_column(table, column) {
                    bytes = target_crypto
                        .encrypt(&bytes)
                        .map_err(|_| ImportError::AuthenticationFailed)?;
                }
                query.bind(bytes)
            }
        };
    }
    query.execute(&mut **transaction).await.map_err(|_| {
        ImportError::InvalidManifest(format!("portable entity insert failed: {table}"))
    })?;
    Ok(())
}

async fn materialize_restored_artifacts(
    transaction: &mut sqlx::Transaction<'_, sqlx::Sqlite>,
    final_target: &Path,
    artifacts: &[(i64, Vec<u8>)],
) -> Result<(), ImportError> {
    let owned = sqlx::query_as::<_, (i64, String, String, i64)>(
        "SELECT id, s3_key, sha256, size_bytes FROM plugins WHERE deleted_at IS NULL ORDER BY id",
    )
    .fetch_all(&mut **transaction)
    .await
    .map_err(|_| ImportError::InvalidManifest("read imported Plugin ownership".into()))?;
    let artifact_map = artifacts
        .iter()
        .map(|(plugin_id, bytes)| (*plugin_id, bytes))
        .collect::<BTreeMap<_, _>>();
    if artifact_map.len() != artifacts.len() || artifact_map.len() != owned.len() {
        return Err(ImportError::InvalidManifest(
            "Plugin artifact ownership mismatch".into(),
        ));
    }
    let final_directory = open_ambient_directory_nofollow(final_target)?;
    final_directory
        .create_dir("plugins")
        .or_else(|error| {
            if error.kind() == std::io::ErrorKind::AlreadyExists {
                Ok(())
            } else {
                Err(error)
            }
        })
        .map_err(|error| ImportError::Io(format!("create Plugin root: {error}")))?;
    let plugin_directory = final_directory
        .open_dir("plugins")
        .map_err(|_| ImportError::UnsafeArchiveEntry("plugins".into()))?;
    let mut expected_paths = BTreeSet::new();
    for (plugin_id, key, sha256, size_bytes) in owned {
        let bytes = artifact_map.get(&plugin_id).ok_or_else(|| {
            ImportError::InvalidManifest("Plugin artifact ownership mismatch".into())
        })?;
        let relative = PathBuf::from(&key);
        if validate_relative_path(&relative).is_err()
            || size_bytes < 0
            || bytes.len() as i64 != size_bytes
            || hex::encode(Sha256::digest(bytes)) != sha256
            || !expected_paths.insert(relative.clone())
        {
            return Err(ImportError::InvalidManifest(
                "Plugin artifact ownership mismatch".into(),
            ));
        }
        if let Some(parent) = relative.parent()
            && !parent.as_os_str().is_empty()
        {
            plugin_directory
                .create_dir_all(parent)
                .map_err(|error| ImportError::Io(format!("create Plugin parents: {error}")))?;
        }
        let mut options = cap_std::fs::OpenOptions::new();
        options.create_new(true).write(true);
        let mut file = plugin_directory
            .open_with(&relative, &options)
            .map_err(|error| ImportError::Io(format!("create Plugin artifact: {error}")))?;
        file.write_all(bytes)
            .and_then(|_| file.sync_all())
            .map_err(|error| ImportError::Io(format!("persist Plugin artifact: {error}")))?;
    }
    sync_cap_directory(&plugin_directory, "fsync Plugin root")
}

async fn validate_restored_artifacts(
    transaction: &mut sqlx::Transaction<'_, sqlx::Sqlite>,
    plugin_root: &Path,
) -> Result<(), ImportError> {
    let rows = sqlx::query_as::<_, (String, String, i64)>(
        "SELECT s3_key, sha256, size_bytes FROM plugins WHERE deleted_at IS NULL ORDER BY s3_key",
    )
    .fetch_all(&mut **transaction)
    .await
    .map_err(|_| ImportError::InvalidManifest("read restored Plugin ownership".into()))?;
    let mut expected = BTreeSet::new();
    for (key, sha256, size_bytes) in rows {
        let relative = PathBuf::from(key);
        if validate_relative_path(&relative).is_err()
            || size_bytes < 0
            || !expected.insert(relative.clone())
        {
            return Err(ImportError::InvalidManifest(
                "invalid restored Plugin ownership row".into(),
            ));
        }
        read_verified_regular_file(&plugin_root.join(&relative), size_bytes as u64, &sha256)
            .map_err(|_| ImportError::InvalidManifest("Plugin artifact mismatch".into()))?;
    }
    let actual = collect_relative_regular_files(plugin_root)?;
    if actual != expected {
        return Err(ImportError::InvalidManifest(
            "Plugin archive ownership mismatch".into(),
        ));
    }
    Ok(())
}

fn collect_relative_regular_files(root: &Path) -> Result<BTreeSet<PathBuf>, ImportError> {
    if !root.exists() {
        return Ok(BTreeSet::new());
    }
    let mut pending = vec![root.to_path_buf()];
    let mut files = BTreeSet::new();
    while let Some(directory) = pending.pop() {
        for entry in fs::read_dir(&directory).map_err(|error| ImportError::Io(error.to_string()))? {
            let entry = entry.map_err(|error| ImportError::Io(error.to_string()))?;
            let path = entry.path();
            let metadata =
                fs::symlink_metadata(&path).map_err(|error| ImportError::Io(error.to_string()))?;
            if metadata.file_type().is_symlink() {
                return Err(ImportError::UnsafeArchiveEntry(path.display().to_string()));
            }
            if metadata.is_dir() {
                pending.push(path);
            } else if metadata.is_file() {
                #[cfg(unix)]
                {
                    use std::os::unix::fs::MetadataExt;
                    if metadata.nlink() != 1 {
                        return Err(ImportError::UnsafeArchiveEntry(path.display().to_string()));
                    }
                }
                let relative = path
                    .strip_prefix(root)
                    .map_err(|_| ImportError::UnsafeArchiveEntry(path.display().to_string()))?
                    .to_path_buf();
                validate_relative_path(&relative)
                    .map_err(|_| ImportError::UnsafeArchiveEntry(path.display().to_string()))?;
                files.insert(relative);
            } else {
                return Err(ImportError::UnsafeArchiveEntry(path.display().to_string()));
            }
        }
    }
    Ok(files)
}

/// One named durability boundary used by deterministic fault injection.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub struct BackupCrashPoint(&'static str);

impl BackupCrashPoint {
    /// Stable crash-point wire name.
    pub const fn as_str(self) -> &'static str {
        self.0
    }
}

/// Backup confirmation boundaries owned by T130.
pub const BACKUP_CONFIRMATION_CRASH_POINTS: [BackupCrashPoint; 6] = [
    BackupCrashPoint("write_gate_close"),
    BackupCrashPoint("checkpoint_current"),
    BackupCrashPoint("checkpoint_staging"),
    BackupCrashPoint("close_connections"),
    BackupCrashPoint("sidecar_convergence"),
    BackupCrashPoint("safe_snapshot_publish"),
];

/// Sidecar cleanup durability boundaries owned by T130.
pub const SIDECAR_CLEANUP_CRASH_POINTS: [BackupCrashPoint; 16] = [
    BackupCrashPoint("sidecar_prepared_staging_fsync"),
    BackupCrashPoint("sidecar_prepared_publish"),
    BackupCrashPoint("sidecar_prepared_parent_fsync"),
    BackupCrashPoint("sidecar_quarantine_rename"),
    BackupCrashPoint("sidecar_quarantine_parent_fsync"),
    BackupCrashPoint("sidecar_quarantine_identity_verify"),
    BackupCrashPoint("sidecar_quarantined_staging_fsync"),
    BackupCrashPoint("sidecar_quarantined_publish"),
    BackupCrashPoint("sidecar_quarantined_parent_fsync"),
    BackupCrashPoint("sidecar_quarantine_unlink"),
    BackupCrashPoint("sidecar_quarantine_unlink_parent_fsync"),
    BackupCrashPoint("sidecar_done_staging_fsync"),
    BackupCrashPoint("sidecar_done_publish"),
    BackupCrashPoint("sidecar_done_parent_fsync"),
    BackupCrashPoint("sidecar_journal_unlink"),
    BackupCrashPoint("sidecar_journal_unlink_parent_fsync"),
];

/// Restore switch boundaries owned by T130.
pub const RESTORE_SWITCH_CRASH_POINTS: [BackupCrashPoint; 26] = [
    BackupCrashPoint("manifest_arm_staging_write"),
    BackupCrashPoint("manifest_arm_staging_fsync"),
    BackupCrashPoint("manifest_arm_rename"),
    BackupCrashPoint("manifest_arm_parent_fsync"),
    BackupCrashPoint("manifest_arm"),
    BackupCrashPoint("owner_prepared_staging_write"),
    BackupCrashPoint("owner_prepared_staging_fsync"),
    BackupCrashPoint("owner_prepared_rename"),
    BackupCrashPoint("owner_prepared_parent_fsync"),
    BackupCrashPoint("owner_prepared_publish"),
    BackupCrashPoint("database_switch"),
    BackupCrashPoint("plugin_tree_switch"),
    BackupCrashPoint("owner_applying_staging_write"),
    BackupCrashPoint("owner_applying_staging_fsync"),
    BackupCrashPoint("owner_applying_rename"),
    BackupCrashPoint("owner_applying_parent_fsync"),
    BackupCrashPoint("owner_applying_publish"),
    BackupCrashPoint("new_health_verify"),
    BackupCrashPoint("new_search_verify"),
    BackupCrashPoint("new_artifact_verify"),
    BackupCrashPoint("new_identity_verify"),
    BackupCrashPoint("owner_committed_staging_write"),
    BackupCrashPoint("owner_committed_staging_fsync"),
    BackupCrashPoint("owner_committed_rename"),
    BackupCrashPoint("owner_committed_parent_fsync"),
    BackupCrashPoint("owner_committed_publish"),
];

/// Restore retirement boundaries owned by T130.
pub const RETIREMENT_CRASH_POINTS: [BackupCrashPoint; 24] = [
    BackupCrashPoint("retirement_prepared_staging_write"),
    BackupCrashPoint("retirement_prepared_staging_fsync"),
    BackupCrashPoint("retirement_prepared_rename"),
    BackupCrashPoint("retirement_prepared_parent_fsync"),
    BackupCrashPoint("retirement_prepared_publish"),
    BackupCrashPoint("live_to_tombstone_rename"),
    BackupCrashPoint("live_to_tombstone_parent_fsync"),
    BackupCrashPoint("live_to_tombstone_identity_verify"),
    BackupCrashPoint("retirement_renamed_staging_write"),
    BackupCrashPoint("retirement_renamed_staging_fsync"),
    BackupCrashPoint("retirement_renamed_rename"),
    BackupCrashPoint("retirement_renamed_parent_fsync"),
    BackupCrashPoint("retirement_renamed_publish"),
    BackupCrashPoint("tombstone_leaf_unlink"),
    BackupCrashPoint("tombstone_directory_fsync"),
    BackupCrashPoint("tombstone_rmdir"),
    BackupCrashPoint("retirement_done_staging_write"),
    BackupCrashPoint("retirement_done_staging_fsync"),
    BackupCrashPoint("retirement_done_rename"),
    BackupCrashPoint("retirement_done_parent_fsync"),
    BackupCrashPoint("retirement_done_publish"),
    BackupCrashPoint("retirement_journal_delete"),
    BackupCrashPoint("retirement_journal_unlink"),
    BackupCrashPoint("retirement_journal_unlink_parent_fsync"),
];

fn maybe_inject_restore_crash(
    crash_at: Option<BackupCrashPoint>,
    boundary: &'static str,
) -> Result<(), ImportError> {
    if crash_at.is_some_and(|point| point.as_str() == boundary) {
        return Err(ImportError::InjectedCrash(boundary));
    }
    Ok(())
}

fn maybe_inject_backup_crash(
    crash_at: Option<BackupCrashPoint>,
    boundary: &'static str,
) -> Result<(), ExportError> {
    if crash_at.is_some_and(|point| point.as_str() == boundary) {
        return Err(ExportError::InjectedCrash(boundary));
    }
    Ok(())
}

/// Preview bound to one still-absent target path.
#[derive(Debug, Clone)]
pub struct BackupPreview {
    target: PathBuf,
    entity_counts: BTreeMap<String, u64>,
    coordinator_marker: Arc<()>,
    maintenance_lease: Arc<super::store::StoreMaintenanceLease>,
}

impl BackupPreview {
    /// Count observed for an entity at preview time.
    pub fn entity_count(&self, entity: &str) -> u64 {
        self.entity_counts.get(entity).copied().unwrap_or(0)
    }
}

/// Result of a confirmed export after the write gate and checkpoint.
#[derive(Debug, Clone)]
pub struct BackupConfirmation {
    entity_counts: BTreeMap<String, u64>,
    entity_names: BTreeMap<String, BTreeSet<String>>,
    current_checkpoint_complete: bool,
    sidecars_converged: bool,
}

impl BackupConfirmation {
    /// Count included in the confirmed snapshot.
    pub fn entity_count(&self, entity: &str) -> u64 {
        self.entity_counts.get(entity).copied().unwrap_or(0)
    }

    /// Whether one named row was included.
    pub fn includes_entity(&self, entity: &str, name: &str) -> bool {
        self.entity_names
            .get(entity)
            .is_some_and(|values| values.contains(name))
    }

    /// Whether the current database completed a non-busy checkpoint.
    pub fn current_checkpoint_complete(&self) -> bool {
        self.current_checkpoint_complete
    }

    /// Whether WAL/SHM/journal state was converged before export.
    pub fn sidecars_converged(&self) -> bool {
        self.sidecars_converged
    }
}

/// Fault-injection result returned at one named backup-confirmation boundary.
#[derive(Debug, Error)]
#[error("backup confirmation crash harness at {crash_point}: {cause}")]
pub struct BackupConfirmationCrashError {
    crash_point: &'static str,
    cause: String,
}

impl BackupConfirmationCrashError {
    /// Stable injected boundary.
    pub fn crash_point(&self) -> &'static str {
        self.crash_point
    }

    /// Whether execution reached the exact requested durability boundary.
    pub fn reached_requested_boundary(&self) -> bool {
        self.cause == "injected"
    }
}

/// Store-aware backup coordinator. T129's standalone exporter remains usable
/// for an already-closed database; this boundary owns the live-Store freeze.
#[derive(Debug, Clone)]
pub struct BackupCoordinator {
    store: Store,
    coordinator_marker: Arc<()>,
}

impl BackupCoordinator {
    /// Bind one coordinator to the canonical local Store.
    pub fn from_store(store: Store) -> Result<Self, ExportError> {
        if store.database_path().as_os_str().is_empty() {
            return Err(ExportError::SourceMissing("database path".into()));
        }
        Ok(Self {
            store,
            coordinator_marker: Arc::new(()),
        })
    }

    /// Read the current impact without closing the write gate.
    pub async fn preview_export(&self, target: &Path) -> Result<BackupPreview, ExportError> {
        let maintenance_lease = super::store::acquire_store_maintenance(
            self.store.database_path(),
            super::store::StoreMaintenanceKind::Backup,
            super::store::StoreMaintenancePhase::BackupReady,
        )
        .map_err(export_maintenance_error)?;
        let result = async {
            if target.exists() {
                return Err(ExportError::TargetExists(target.display().to_string()));
            }
            let count = sqlx::query_scalar::<_, i64>("SELECT COUNT(*) FROM data_sources")
                .fetch_one(self.store.pool())
                .await
                .map_err(|_| ExportError::Io("preview query failed".into()))?;
            Ok(BackupPreview {
                target: target.to_path_buf(),
                entity_counts: BTreeMap::from([("data_sources".into(), count.max(0) as u64)]),
                coordinator_marker: self.coordinator_marker.clone(),
                maintenance_lease: maintenance_lease.clone(),
            })
        }
        .await;
        if result.is_err() {
            let _ = maintenance_lease.release(super::store::StoreMaintenancePhase::BackupReady);
        }
        result
    }

    /// Cancel one exact still-ready backup preview without touching its target.
    pub async fn cancel_preview(&self, preview: &BackupPreview) -> Result<(), ExportError> {
        if !Arc::ptr_eq(&self.coordinator_marker, &preview.coordinator_marker) {
            return Err(ExportError::Io("backup preview not active".into()));
        }
        preview
            .maintenance_lease
            .release(super::store::StoreMaintenancePhase::BackupReady)
            .map_err(export_maintenance_error)
    }

    /// Freeze writes, checkpoint committed frames, and publish a verified age
    /// archive containing the last legal write.
    pub async fn confirm_export(
        &self,
        preview: BackupPreview,
        passphrase: &str,
    ) -> Result<BackupConfirmation, ExportError> {
        self.confirm_export_internal(preview, passphrase, None)
            .await
    }

    /// Advance the real confirmation state machine through one requested
    /// durability boundary and then simulate process interruption.
    pub async fn confirm_with_crash(
        &self,
        preview: BackupPreview,
        passphrase: &str,
        crash_point: BackupCrashPoint,
    ) -> Result<(), BackupConfirmationCrashError> {
        match self
            .confirm_export_internal(preview, passphrase, Some(crash_point))
            .await
        {
            Err(ExportError::InjectedCrash(actual)) if actual == crash_point.as_str() => {
                Err(BackupConfirmationCrashError {
                    crash_point: actual,
                    cause: "injected".into(),
                })
            }
            Err(error) => Err(BackupConfirmationCrashError {
                crash_point: crash_point.as_str(),
                cause: error.to_string(),
            }),
            Ok(_) => Err(BackupConfirmationCrashError {
                crash_point: crash_point.as_str(),
                cause: "requested boundary was not reached".into(),
            }),
        }
    }

    async fn confirm_export_internal(
        &self,
        preview: BackupPreview,
        passphrase: &str,
        crash_at: Option<BackupCrashPoint>,
    ) -> Result<BackupConfirmation, ExportError> {
        if !Arc::ptr_eq(&self.coordinator_marker, &preview.coordinator_marker) {
            return Err(ExportError::Io("backup preview not active".into()));
        }
        preview
            .maintenance_lease
            .transition(
                super::store::StoreMaintenancePhase::BackupReady,
                super::store::StoreMaintenancePhase::BackupTerminal,
            )
            .map_err(export_maintenance_error)?;
        frozen_databases()
            .lock()
            .map_err(|_| ExportError::Io("write gate poisoned".into()))?
            .insert(self.store.database_path().to_path_buf());
        let source = self.store.database_path().to_path_buf();
        #[cfg(target_os = "linux")]
        let preclose_anchor = open_export_source_anchored(&source);
        self.store.pool().close().await;
        maybe_inject_backup_crash(crash_at, "write_gate_close")?;
        #[cfg(target_os = "linux")]
        let anchored_source = {
            let anchored_source = preclose_anchor?;
            verify_store_source_leaf_matches_anchor(&source, &anchored_source)?;
            Some(anchored_source)
        };
        #[cfg(not(target_os = "linux"))]
        let anchored_source = None;
        let prepared = prepare_canonical_export(&source, crash_at, anchored_source).await?;
        if let Err(error) = maybe_inject_backup_crash(crash_at, "close_connections") {
            discard_prepared_export(&prepared);
            return Err(error);
        }
        verify_confirmation_sidecars_converged(&source, prepared.database_snapshot.as_deref())?;
        if let Err(error) = maybe_inject_backup_crash(crash_at, "sidecar_convergence") {
            discard_prepared_export(&prepared);
            return Err(error);
        }
        let names = prepared.data_source_names.clone();
        publish_prepared_export(source, preview.target, passphrase.to_string(), 3, prepared)
            .await?;
        maybe_inject_backup_crash(crash_at, "safe_snapshot_publish")?;
        Ok(BackupConfirmation {
            entity_counts: BTreeMap::from([("data_sources".into(), names.len() as u64)]),
            entity_names: BTreeMap::from([("data_sources".into(), names.into_iter().collect())]),
            current_checkpoint_complete: true,
            sidecars_converged: true,
        })
    }
}

fn discard_prepared_export(prepared: &PreparedCanonicalExport) {
    if let Some(snapshot) = prepared.database_snapshot.as_deref() {
        let _ = fs::remove_file(snapshot);
        if let Some(parent) = snapshot.parent() {
            let _ = File::open(parent).and_then(|directory| directory.sync_all());
        }
    }
}

fn verify_confirmation_sidecars_converged(
    current: &Path,
    staging: Option<&Path>,
) -> Result<(), ExportError> {
    for database in std::iter::once(current).chain(staging) {
        for suffix in ["-wal", "-shm", "-journal"] {
            let sidecar = PathBuf::from(format!("{}{suffix}", database.display()));
            if sidecar.exists() {
                return Err(ExportError::Io(format!(
                    "unconverged SQLite sidecar {suffix}"
                )));
            }
        }
    }
    Ok(())
}

/// Proven result selected by startup replay.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RetirementOutcome {
    /// No live switch began; staged bytes were retired.
    AbortedPreSwitch,
    /// The old current instance was proven and retained.
    Old,
    /// The new current instance was proven and retained.
    New,
}

/// Authenticated, unarmed restore instance prepared by T129.
#[derive(Debug, Clone)]
pub struct RestorePlan {
    db_instance_operation_id: String,
    cleanup_operation_id: String,
    manifest: BackupManifest,
    staging_directory: PathBuf,
    owner_exists: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
struct RestoreInstanceManifest {
    schema_version: u32,
    role: String,
    db_instance_operation_id: String,
    db_id: String,
    database_name: String,
    ownership_state: String,
    cleanup_operation_id: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
struct ControlledFileEvidence {
    relative_path: String,
    identity: String,
    size_bytes: u64,
    sha256: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
struct ControlledTreeEvidence {
    relative_path: String,
    identity: String,
    sha256: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
struct RetirementJournal {
    schema_version: u32,
    role: String,
    db_instance_operation_id: String,
    cleanup_operation_id: String,
    db_id: String,
    terminal_outcome: String,
    live_basename: String,
    tombstone_basename: String,
    live_directory_identity: String,
    manifest_identity: String,
    manifest_sha256: String,
    manifest_ownership_state: String,
    owner_identity: Option<String>,
    owner_phase: Option<String>,
    terminal_database: Option<ControlledFileEvidence>,
    terminal_plugin_root: Option<ControlledTreeEvidence>,
    state: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
struct RestoreOwner {
    schema_version: u32,
    role: String,
    db_instance_operation_id: String,
    db_id: String,
    instance_basename: String,
    manifest_identity: String,
    manifest_sha256: String,
    ownership_state: String,
    old_database: Option<ControlledFileEvidence>,
    new_database: ControlledFileEvidence,
    old_plugin_root: Option<ControlledTreeEvidence>,
    new_plugin_root: ControlledTreeEvidence,
    safety_snapshot: ControlledTreeEvidence,
    phase: String,
}

impl RestorePlan {
    /// UUID owning the live restore instance.
    pub fn db_instance_operation_id(&self) -> &str {
        &self.db_instance_operation_id
    }

    /// Separate UUID owning retirement cleanup.
    pub fn cleanup_operation_id(&self) -> &str {
        &self.cleanup_operation_id
    }

    /// Durable unarmed manifest.
    pub fn manifest(&self) -> &BackupManifest {
        &self.manifest
    }

    /// Whether a T130 owner file already exists.
    pub fn owner_exists(&self) -> bool {
        self.owner_exists
    }
}

const PREPARED_RESTORE_READY: u8 = 0;
const PREPARED_RESTORE_CONFIRMING: u8 = 1;
const PREPARED_RESTORE_CANCELLED: u8 = 2;
const RESTORE_RECOVERY_IDLE: u8 = 0;
const RESTORE_RECOVERY_RUNNING: u8 = 1;
const RESTORE_RECOVERY_DONE: u8 = 2;

/// Fully authenticated and validated restore preview awaiting one final
/// confirmation. The handle is intentionally not cloneable; its atomic state
/// prevents a second confirmation or cancellation from reusing the live
/// unarmed instance.
pub struct PreparedRestore {
    plan: RestorePlan,
    safety_backup_path: PathBuf,
    coordinator_marker: Arc<()>,
    maintenance_lease: Arc<super::store::StoreMaintenanceLease>,
    archive_binding: Arc<VerifiedArchiveBinding>,
    state: AtomicU8,
}

impl std::fmt::Debug for PreparedRestore {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("PreparedRestore")
            .field(
                "db_instance_operation_id",
                &self.plan.db_instance_operation_id,
            )
            .field("safety_backup_path", &self.safety_backup_path)
            .field("archive_binding", &self.archive_binding)
            .field("state", &self.state.load(Ordering::SeqCst))
            .finish_non_exhaustive()
    }
}

impl PreparedRestore {
    /// UUID owning the authenticated unarmed live instance.
    pub fn db_instance_operation_id(&self) -> &str {
        self.plan.db_instance_operation_id()
    }

    /// Exact future path of the safety snapshot created only after final
    /// confirmation freezes and closes the current Store.
    pub fn safety_backup_path(&self) -> &Path {
        &self.safety_backup_path
    }

    /// Whether an armed recovery owner already exists.
    pub fn owner_exists(&self) -> bool {
        self.plan.owner_exists()
    }
}

/// Store-bound restore confirmation. Constructing this value synchronously
/// closes the write gate and marks the shared SQLx Pool closed; [`finish`](Self::finish)
/// then drains checked-out connections before reusing the durable T130 switch.
#[must_use = "a restore confirmation must be finished or left fail-closed"]
pub struct RestoreConfirmation {
    coordinator: RestoreCoordinator,
    plan: RestorePlan,
    pool: sqlx::Pool<sqlx::Sqlite>,
}

/// Proven state of the restore safety backup when confirmation returns an
/// error. This phase is assigned by the confirmation state machine, never by
/// inspecting an error string or probing a path after failure.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RestoreSafetyBackupState {
    /// The complete safety backup was not yet proven, so data switching had
    /// not begun.
    NotVerified,
    /// The exact held safety backup remained verified after the apply outcome.
    Verified,
    /// The held safety backup no longer matched its canonical binding, so its
    /// state and the data-switch phase cannot be asserted in-process.
    Invalidated,
}

/// Structured failure returned by a Store-bound restore confirmation.
#[derive(Debug, Error)]
#[error("restore_confirmation_failed")]
pub struct RestoreConfirmationError {
    safety_backup_state: RestoreSafetyBackupState,
    cause: ImportError,
}

impl RestoreConfirmationError {
    /// Proven safety-backup phase at the confirmation failure boundary.
    pub fn safety_backup_state(&self) -> RestoreSafetyBackupState {
        self.safety_backup_state
    }

    fn new(safety_backup_state: RestoreSafetyBackupState, cause: ImportError) -> Self {
        Self {
            safety_backup_state,
            cause,
        }
    }

    fn into_cause(self) -> ImportError {
        self.cause
    }
}

trait RestoreConfirmationFutureExt:
    std::future::Future<Output = Result<RestoreRecovery, RestoreConfirmationError>> + Send + Sized
{
    fn into_import_error(
        self,
    ) -> impl std::future::Future<Output = Result<RestoreRecovery, ImportError>> + Send {
        async move { self.await.map_err(RestoreConfirmationError::into_cause) }
    }
}

impl<T> RestoreConfirmationFutureExt for T where
    T: std::future::Future<Output = Result<RestoreRecovery, RestoreConfirmationError>> + Send
{
}

impl std::fmt::Debug for RestoreConfirmation {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("RestoreConfirmation")
            .field("plan", &self.plan)
            .finish_non_exhaustive()
    }
}

struct BoundRestoreTerminal<'a> {
    terminal: &'a AtomicBool,
    maintenance_lease: Option<Arc<super::store::StoreMaintenanceLease>>,
}

impl Drop for BoundRestoreTerminal<'_> {
    fn drop(&mut self) {
        self.terminal.store(true, Ordering::SeqCst);
        if let Some(maintenance_lease) = self.maintenance_lease.as_ref() {
            let _ = maintenance_lease.transition(
                super::store::StoreMaintenancePhase::RestoreConfirmation,
                super::store::StoreMaintenancePhase::RestoreTerminal,
            );
        }
    }
}

impl RestoreConfirmation {
    /// Drain every connection, create and verify the final safety snapshot,
    /// durably switch both database and Plugin tree, and keep the process
    /// terminal until startup recovery succeeds.
    pub async fn finish(self) -> Result<RestoreRecovery, RestoreConfirmationError> {
        self.finish_internal(None, None, None).await
    }

    /// Test-only synchronization wrapper around the production current
    /// safety-snapshot copy.
    ///
    /// The barriers only pause the descriptor-bound copy after the held
    /// current file has been matched to the canonical root entry and before
    /// its first seek/read. They never select or reopen a source file.
    #[doc(hidden)]
    pub async fn finish_with_current_snapshot_copy_interlock_for_test(
        self,
        barriers: [Arc<tokio::sync::Barrier>; 2],
    ) -> Result<RestoreRecovery, ImportError> {
        self.finish_internal(Some(barriers), None, None)
            .into_import_error()
            .await
    }

    /// Test-only synchronization wrapper around the ordinary descriptor-bound
    /// safety snapshot and owner transition.
    #[doc(hidden)]
    pub async fn finish_with_snapshot_binding_interlocks_for_test(
        self,
        barriers: [Arc<tokio::sync::Barrier>; 4],
    ) -> Result<RestoreRecovery, ImportError> {
        self.finish_internal(None, Some(barriers), None)
            .into_import_error()
            .await
    }

    /// Test-only synchronization around the descriptor-bound staging and
    /// current database checkpoint operations.
    #[doc(hidden)]
    pub async fn finish_with_pinned_checkpoint_interlocks_for_test(
        self,
        barriers: [Arc<tokio::sync::Barrier>; 8],
    ) -> Result<RestoreRecovery, ImportError> {
        self.finish_internal(None, None, Some(barriers))
            .into_import_error()
            .await
    }

    async fn finish_internal(
        self,
        current_copy_interlock: Option<[Arc<tokio::sync::Barrier>; 2]>,
        snapshot_binding_interlocks: Option<[Arc<tokio::sync::Barrier>; 4]>,
        pinned_checkpoint_interlocks: Option<[Arc<tokio::sync::Barrier>; 8]>,
    ) -> Result<RestoreRecovery, RestoreConfirmationError> {
        self.pool.close().await;
        let maintenance_lease = self.coordinator.restore_maintenance_lease();
        let _terminal = BoundRestoreTerminal {
            terminal: &self.coordinator.restore_owner_terminal,
            maintenance_lease: maintenance_lease.as_ref().ok().cloned().flatten(),
        };

        let pre_safety_outcome = async {
            let _maintenance_lease = maintenance_lease?;
            let held_current = self.coordinator.open_current_database_for_snapshot()?;
            let prepared = self
                .coordinator
                .prepare_restore_for_safety(&self.plan, None, pinned_checkpoint_interlocks.as_ref())
                .await?;
            let snapshot_binding = begin_restore_safety_snapshot(
                &prepared.root_dir,
                &prepared.io_root,
                &self.plan.db_instance_operation_id,
            )?;
            verify_snapshot_directory_binding(&snapshot_binding)?;
            wait_snapshot_directory_binding_interlock(snapshot_binding_interlocks.as_ref()).await;
            let safety_result = async {
                let (database, bound_old_database) = match held_current {
                    Some(HeldCurrentDatabase {
                        mut held_current,
                        canonical_current_identity,
                    }) => {
                        let database = copy_owned_open_file(
                            &mut held_current,
                            &canonical_current_identity,
                            snapshot_binding.directory(),
                            Path::new(DATABASE_FILENAME),
                            Path::new(DATABASE_FILENAME),
                            current_copy_interlock.as_ref(),
                        )
                        .await?;
                        let old_database_evidence = ControlledFileEvidence {
                            relative_path: DATABASE_FILENAME.into(),
                            identity: database.source_identity.clone(),
                            size_bytes: database.size_bytes,
                            sha256: database.sha256.clone(),
                        };
                        let bound_old_database = BoundOldDatabase {
                            held_file: held_current,
                            canonical_identity: canonical_current_identity,
                            evidence: old_database_evidence,
                        };
                        (Some(database), Some(bound_old_database))
                    }
                    None => (None, None),
                };
                finish_restore_safety_snapshot(
                    &snapshot_binding,
                    prepared.current_plugins_dir.as_ref(),
                    &self.plan.db_instance_operation_id,
                    database,
                )?;
                verify_restore_safety_snapshot(&snapshot_binding)?;
                if let Some(bound_old_database) = bound_old_database.as_ref() {
                    self.coordinator.verify_current_database_for_snapshot(
                        &bound_old_database.held_file,
                        &bound_old_database.canonical_identity,
                    )?;
                } else {
                    self.coordinator.verify_current_database_absent()?;
                }
                Ok::<_, ImportError>(bound_old_database)
            }
            .await;
            let (snapshot_binding, bound_old_database) =
                finish_or_cleanup_restore_safety_snapshot(snapshot_binding, safety_result)?;
            wait_post_safety_binding_interlock(snapshot_binding_interlocks.as_ref()).await;
            verify_snapshot_directory_binding(&snapshot_binding)?;
            let verified_safety_snapshot = VerifiedRestoreSafetySnapshot::new(
                snapshot_binding,
                &PathBuf::from("backups").join(format!(
                    "restore-safety-{}",
                    self.plan.db_instance_operation_id
                )),
            )?;
            Ok::<_, ImportError>((prepared, bound_old_database, verified_safety_snapshot))
        }
        .await;
        let (prepared, bound_old_database, verified_safety_snapshot) =
            pre_safety_outcome.map_err(|cause| {
                RestoreConfirmationError::new(RestoreSafetyBackupState::NotVerified, cause)
            })?;
        let apply_outcome = self
            .coordinator
            .apply_restore_after_safety(
                &self.plan,
                None,
                true,
                prepared,
                &verified_safety_snapshot,
                RestoreOldDatabaseEvidence::Bound(bound_old_database),
            )
            .await;
        let safety_binding = verified_safety_snapshot.revalidate();
        classify_restore_confirmation_outcome(apply_outcome, safety_binding)
    }

    /// Execute one approved durability boundary and simulate interruption.
    /// The Store remains closed and its process lock remains held for startup
    /// replay.
    pub async fn finish_with_crash(
        self,
        crash_point: BackupCrashPoint,
    ) -> Result<(), RestoreCrashError> {
        self.pool.close().await;
        let maintenance_lease = self
            .coordinator
            .restore_maintenance_lease()
            .map_err(|error| RestoreCrashError {
                crash_point: crash_point.as_str(),
                cause: error.to_string(),
            })?;
        let _terminal = BoundRestoreTerminal {
            terminal: &self.coordinator.restore_owner_terminal,
            maintenance_lease,
        };
        match self
            .coordinator
            .apply_restore_internal(&self.plan, Some(crash_point), None)
            .await
        {
            Err(ImportError::InjectedCrash(actual)) if actual == crash_point.as_str() => {
                Err(RestoreCrashError {
                    crash_point: actual,
                    cause: "injected".into(),
                })
            }
            Err(error) => Err(RestoreCrashError {
                crash_point: crash_point.as_str(),
                cause: error.to_string(),
            }),
            Ok(_) => Err(RestoreCrashError {
                crash_point: crash_point.as_str(),
                cause: "requested boundary was not reached".into(),
            }),
        }
    }
}

fn classify_restore_confirmation_outcome(
    apply_outcome: Result<RestoreRecovery, ImportError>,
    safety_binding: Result<(), ImportError>,
) -> Result<RestoreRecovery, RestoreConfirmationError> {
    match (apply_outcome, safety_binding) {
        (Ok(recovery), Ok(())) => Ok(recovery),
        (Err(cause), Ok(())) => Err(RestoreConfirmationError::new(
            RestoreSafetyBackupState::Verified,
            cause,
        )),
        (Ok(_), Err(binding_cause)) => Err(RestoreConfirmationError::new(
            RestoreSafetyBackupState::Invalidated,
            binding_cause,
        )),
        (Err(_), Err(binding_cause)) => Err(RestoreConfirmationError::new(
            RestoreSafetyBackupState::Invalidated,
            binding_cause,
        )),
    }
}

async fn wait_snapshot_directory_binding_interlock(
    snapshot_barriers: Option<&[Arc<tokio::sync::Barrier>; 4]>,
) {
    if let Some(snapshot_barriers) = snapshot_barriers {
        snapshot_barriers[0].wait().await;
        snapshot_barriers[1].wait().await;
    }
}

async fn wait_post_safety_binding_interlock(
    snapshot_barriers: Option<&[Arc<tokio::sync::Barrier>; 4]>,
) {
    if let Some(snapshot_barriers) = snapshot_barriers {
        snapshot_barriers[2].wait().await;
        snapshot_barriers[3].wait().await;
    }
}

/// Fault-injection result returned at one named durability boundary.
#[derive(Debug, Error)]
#[error("restore crash harness at {crash_point}: {cause}")]
pub struct RestoreCrashError {
    crash_point: &'static str,
    cause: String,
}

impl RestoreCrashError {
    /// Stable injected boundary.
    pub fn crash_point(&self) -> &'static str {
        self.crash_point
    }

    /// Whether execution reached the exact requested durability boundary.
    pub fn reached_requested_boundary(&self) -> bool {
        self.cause == "injected"
    }
}

/// Deterministic startup replay result.
#[derive(Debug, Clone)]
pub struct RestoreRecovery {
    retirement_outcome: RetirementOutcome,
    store_may_open: bool,
    exactly_one_live_database: bool,
    no_mixed_database_or_plugin_tree: bool,
    control_files_outside_live_tree: bool,
    retirement_done: bool,
    write_gate_open: bool,
}

impl RestoreRecovery {
    /// Store may reopen only after cleanup is proven complete.
    pub fn store_may_open(&self) -> bool {
        self.store_may_open
    }
    /// Exactly one database is considered live.
    pub fn has_exactly_one_live_database(&self) -> bool {
        self.exactly_one_live_database
    }
    /// Database and Plugin tree never select different outcomes.
    pub fn has_no_mixed_database_or_plugin_tree(&self) -> bool {
        self.no_mixed_database_or_plugin_tree
    }
    /// Locator/control files are not placed inside the live archive tree.
    pub fn control_files_are_outside_live_tree(&self) -> bool {
        self.control_files_outside_live_tree
    }
    /// Proven retirement outcome.
    pub fn retirement_outcome(&self) -> RetirementOutcome {
        self.retirement_outcome
    }
    /// Retirement journal reached durable done and was removed.
    pub fn retirement_is_done(&self) -> bool {
        self.retirement_done
    }
    /// Write gate is reopened after deterministic replay.
    pub fn write_gate_is_open(&self) -> bool {
        self.write_gate_open
    }
}

struct HeldCurrentDatabase {
    held_current: File,
    canonical_current_identity: String,
}

struct BoundOldDatabase {
    held_file: File,
    canonical_identity: String,
    evidence: ControlledFileEvidence,
}

struct RestoreSafetySnapshotBinding {
    parent_directory: cap_std::fs::Dir,
    directory: cap_std::fs::Dir,
    canonical_identity: String,
    canonical_path: PathBuf,
    canonical_name: PathBuf,
}

impl RestoreSafetySnapshotBinding {
    fn directory(&self) -> &cap_std::fs::Dir {
        &self.directory
    }

    fn parent_directory(&self) -> &cap_std::fs::Dir {
        &self.parent_directory
    }

    fn canonical_path(&self) -> &Path {
        &self.canonical_path
    }
}

struct VerifiedRestoreSafetySnapshot {
    binding: RestoreSafetySnapshotBinding,
    evidence: ControlledTreeEvidence,
}

impl VerifiedRestoreSafetySnapshot {
    #[rustfmt::skip]
    fn new(binding: RestoreSafetySnapshotBinding, relative_path: &Path) -> Result<Self, ImportError> {
        verify_snapshot_directory_binding(&binding)?;
        let evidence = controlled_tree_evidence_at(binding.directory(), relative_path)?;
        verify_snapshot_directory_binding(&binding)?;
        Ok(Self {
            binding,
            evidence,
        })
    }

    #[rustfmt::skip]
    fn revalidate(&self) -> Result<(), ImportError> {
        verify_snapshot_directory_binding(&self.binding)?;
        let fresh_evidence = controlled_tree_evidence_at(self.binding.directory(),
            Path::new(&self.evidence.relative_path),
        )?;
        if fresh_evidence != self.evidence {
            return Err(ImportError::InvalidManifest(
                "restore safety snapshot evidence changed".into(),
            ));
        }
        verify_snapshot_directory_binding(&self.binding)?;
        Ok(())
    }
}

struct RestorePreparedForSafety {
    io_root: PathBuf,
    root_dir: cap_std::fs::Dir,
    current_database: PathBuf,
    registry: PathBuf,
    live_basename: String,
    live: PathBuf,
    live_dir: cap_std::fs::Dir,
    staging_database: PathBuf,
    staging_plugins: PathBuf,
    current_plugins: PathBuf,
    current_plugins_dir: Option<cap_std::fs::Dir>,
    write_gate_database: PathBuf,
}

enum RestoreOldDatabaseEvidence {
    InspectCanonicalPath,
    Bound(Option<BoundOldDatabase>),
}

struct RestoreRecoveryAdmission {
    state: Arc<AtomicU8>,
}

impl RestoreRecoveryAdmission {
    fn new(state: Arc<AtomicU8>) -> Self {
        Self { state }
    }

    fn complete(&mut self) {
        self.state.store(RESTORE_RECOVERY_DONE, Ordering::SeqCst);
    }
}

impl Drop for RestoreRecoveryAdmission {
    fn drop(&mut self) {
        let _ = self.state.compare_exchange(
            RESTORE_RECOVERY_RUNNING,
            RESTORE_RECOVERY_IDLE,
            Ordering::SeqCst,
            Ordering::SeqCst,
        );
    }
}

/// Root-scoped restore coordinator. T129 prepares an authenticated unarmed
/// instance; T130 advances it through switch and retirement boundaries.
#[derive(Debug, Clone)]
pub struct RestoreCoordinator {
    root: PathBuf,
    root_directory: Arc<cap_std::fs::Dir>,
    store: Option<Store>,
    coordinator_marker: Arc<()>,
    maintenance_lease: Arc<Mutex<Option<Arc<super::store::StoreMaintenanceLease>>>>,
    restore_owner_guard: Arc<Mutex<Option<super::store::RestoreStoreOwnerGuard>>>,
    restore_owner_terminal: Arc<AtomicBool>,
    restore_recovery_state: Arc<AtomicU8>,
    #[cfg(test)]
    begin_confirmation_post_cas_pre_freeze_failure: Arc<Mutex<Option<String>>>,
}

impl RestoreCoordinator {
    /// Construct a coordinator without following any archive-provided path.
    pub fn new(root: impl Into<PathBuf>) -> Result<Self, ImportError> {
        let root = root.into();
        fs::create_dir_all(&root).map_err(|error| ImportError::Io(error.to_string()))?;
        let metadata =
            fs::symlink_metadata(&root).map_err(|error| ImportError::Io(error.to_string()))?;
        if !metadata.is_dir() || metadata.file_type().is_symlink() {
            return Err(ImportError::UnsafeArchiveEntry(root.display().to_string()));
        }
        let root_directory =
            cap_std::fs::Dir::open_ambient_dir(&root, cap_std::ambient_authority())
                .map_err(|error| ImportError::Io(error.to_string()))?;
        let coordinator = Self {
            root,
            root_directory: Arc::new(root_directory),
            store: None,
            coordinator_marker: Arc::new(()),
            maintenance_lease: Arc::new(Mutex::new(None)),
            restore_owner_guard: Arc::new(Mutex::new(None)),
            restore_owner_terminal: Arc::new(AtomicBool::new(false)),
            restore_recovery_state: Arc::new(AtomicU8::new(RESTORE_RECOVERY_IDLE)),
            #[cfg(test)]
            begin_confirmation_post_cas_pre_freeze_failure: Arc::new(Mutex::new(None)),
        };
        coordinator.validate_registry_root()?;
        Ok(coordinator)
    }

    /// Bind one restore coordinator to the exact owner-aware production
    /// Store. The Store must use the canonical `datasources.db` plus sibling
    /// `plugins` layout; legacy path-only Store constructors are rejected.
    pub fn from_store(store: Store) -> Result<Self, ImportError> {
        if !store.has_restore_owner()
            || store.database_path().file_name() != Some(std::ffi::OsStr::new(DATABASE_FILENAME))
        {
            return Err(ImportError::StorageRecoveryBlocked {
                reason: "connections_open",
                artifact: "connection",
            });
        }
        let root = store
            .database_path()
            .parent()
            .ok_or_else(|| ImportError::UnsafeArchiveEntry("database root missing".into()))?
            .to_path_buf();
        if store.plugin_root() != root.join("plugins") {
            return Err(ImportError::UnsafeArchiveEntry(
                "Store database and Plugin roots do not share one data root".into(),
            ));
        }
        let mut coordinator = Self::new(root)?;
        coordinator.store = Some(store);
        Ok(coordinator)
    }

    fn maintenance_database_path(&self) -> PathBuf {
        self.store
            .as_ref()
            .map(|store| store.database_path().to_path_buf())
            .unwrap_or_else(|| self.root.join(DATABASE_FILENAME))
    }

    fn acquire_restore_maintenance(
        &self,
    ) -> Result<Arc<super::store::StoreMaintenanceLease>, ImportError> {
        let lease = super::store::acquire_store_maintenance(
            &self.maintenance_database_path(),
            super::store::StoreMaintenanceKind::Restore,
            super::store::StoreMaintenancePhase::RestoreReady,
        )
        .map_err(import_maintenance_error)?;
        let mut slot = self
            .maintenance_lease
            .lock()
            .map_err(|_| ImportError::Io("restore maintenance slot poisoned".into()))?;
        *slot = Some(lease.clone());
        Ok(lease)
    }

    fn exact_prepared_maintenance(
        &self,
        prepared: &PreparedRestore,
    ) -> Result<Arc<super::store::StoreMaintenanceLease>, ImportError> {
        let slot = self
            .maintenance_lease
            .lock()
            .map_err(|_| ImportError::Io("restore maintenance slot poisoned".into()))?;
        let lease = slot
            .as_ref()
            .filter(|lease| Arc::ptr_eq(lease, &prepared.maintenance_lease))
            .cloned()
            .ok_or(ImportError::PreparedRestoreNotActive)?;
        Ok(lease)
    }

    fn clear_restore_maintenance_if_exact(
        &self,
        lease: &Arc<super::store::StoreMaintenanceLease>,
    ) -> Result<(), ImportError> {
        let mut slot = self
            .maintenance_lease
            .lock()
            .map_err(|_| ImportError::Io("restore maintenance slot poisoned".into()))?;
        if slot
            .as_ref()
            .is_some_and(|current| Arc::ptr_eq(current, lease))
        {
            slot.take();
        }
        Ok(())
    }

    fn restore_maintenance_lease(
        &self,
    ) -> Result<Option<Arc<super::store::StoreMaintenanceLease>>, ImportError> {
        self.maintenance_lease
            .lock()
            .map(|slot| slot.clone())
            .map_err(|_| ImportError::Io("restore maintenance slot poisoned".into()))
    }

    fn validate_registry_root(&self) -> Result<(), ImportError> {
        const REGISTRY: &str = ".hivegui-db-staging-v1";
        match self.root_directory.symlink_metadata(REGISTRY) {
            Ok(metadata) => {
                if !metadata.is_dir() || metadata.file_type().is_symlink() {
                    return Err(ImportError::UnsafeArchiveEntry(
                        self.root.join(REGISTRY).display().to_string(),
                    ));
                }
                self.root_directory.open_dir(REGISTRY).map_err(|_| {
                    ImportError::UnsafeArchiveEntry(self.root.join(REGISTRY).display().to_string())
                })?;
                Ok(())
            }
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(()),
            Err(error) => Err(ImportError::Io(error.to_string())),
        }
    }

    fn io_root(&self) -> Result<PathBuf, ImportError> {
        #[cfg(target_os = "linux")]
        {
            use std::os::fd::AsRawFd as _;

            // Path-only consumers such as SQLx cannot accept a capability
            // directory directly. `/proc/self/fd/<dirfd>` keeps their lookup
            // anchored to the already-open root even if the ambient pathname
            // is renamed or replaced after construction.
            Ok(PathBuf::from(format!(
                "/proc/self/fd/{}",
                self.root_directory.as_raw_fd()
            )))
        }
        #[cfg(not(target_os = "linux"))]
        {
            let metadata = fs::symlink_metadata(&self.root)
                .map_err(|error| ImportError::Io(error.to_string()))?;
            if !metadata.is_dir() || metadata.file_type().is_symlink() {
                return Err(ImportError::UnsafeArchiveEntry(
                    self.root.display().to_string(),
                ));
            }
            Ok(self.root.clone())
        }
    }

    fn open_current_database_for_snapshot(
        &self,
    ) -> Result<Option<HeldCurrentDatabase>, ImportError> {
        let canonical = match self.root_directory.symlink_metadata(DATABASE_FILENAME) {
            Ok(metadata) => metadata,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
            Err(_) => return Err(current_database_identity_changed()),
        };
        if !canonical.is_file() || canonical.file_type().is_symlink() {
            return Err(current_database_identity_changed());
        }
        #[cfg(unix)]
        {
            use cap_std::fs::MetadataExt as _;

            if canonical.nlink() != 1 {
                return Err(current_database_identity_changed());
            }
        }
        let canonical_current_identity = stable_cap_file_identity(&canonical)?;
        let mut options = cap_std::fs::OpenOptions::new();
        options.read(true);
        #[cfg(unix)]
        {
            use cap_std::fs::OpenOptionsExt as _;

            options.custom_flags(libc::O_NOFOLLOW | libc::O_CLOEXEC | libc::O_NONBLOCK);
        }
        let held_current = self
            .root_directory
            .open_with(DATABASE_FILENAME, &options)
            .map(cap_std::fs::File::into_std)
            .map_err(|_| current_database_identity_changed())?;
        let held = held_current
            .metadata()
            .map_err(|_| current_database_identity_changed())?;
        if !held.is_file()
            || stable_file_identity(&held)? != canonical_current_identity
            || !owned_regular_file_link_count(&held)
        {
            return Err(current_database_identity_changed());
        }
        Ok(Some(HeldCurrentDatabase {
            held_current,
            canonical_current_identity,
        }))
    }

    fn verify_current_database_for_snapshot(
        &self,
        held_current: &File,
        expected_identity: &str,
    ) -> Result<(), ImportError> {
        let held = held_current
            .metadata()
            .map_err(|_| current_database_identity_changed())?;
        let canonical = self
            .root_directory
            .symlink_metadata(DATABASE_FILENAME)
            .map_err(|_| current_database_identity_changed())?;
        if !held.is_file()
            || !canonical.is_file()
            || canonical.file_type().is_symlink()
            || stable_file_identity(&held)? != expected_identity
            || stable_cap_file_identity(&canonical)? != expected_identity
            || !owned_regular_file_link_count(&held)
        {
            return Err(current_database_identity_changed());
        }
        #[cfg(unix)]
        {
            use cap_std::fs::MetadataExt as _;

            if canonical.nlink() != 1 {
                return Err(current_database_identity_changed());
            }
        }
        Ok(())
    }

    fn verify_current_database_absent(&self) -> Result<(), ImportError> {
        match self.root_directory.symlink_metadata(DATABASE_FILENAME) {
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(()),
            _ => Err(current_database_identity_changed()),
        }
    }

    /// Authenticate and materialize one unarmed/no-owner restore instance.
    pub async fn prepare_restore(
        &self,
        archive: &Path,
        passphrase: &str,
    ) -> Result<RestorePlan, ImportError> {
        let archive = Arc::new(VerifiedArchiveBinding::open(archive)?);
        self.prepare_restore_from_binding(archive, passphrase).await
    }

    async fn prepare_restore_from_binding(
        &self,
        archive: Arc<VerifiedArchiveBinding>,
        passphrase: &str,
    ) -> Result<RestorePlan, ImportError> {
        self.validate_registry_root()?;
        let io_root = self.io_root()?;
        let db_instance_operation_id = Uuid::new_v4().to_string();
        let cleanup_operation_id = Uuid::new_v4().to_string();
        let registry = io_root.join(".hivegui-db-staging-v1");
        self.root_directory
            .create_dir_all(".hivegui-db-staging-v1")
            .map_err(|error| ImportError::Io(error.to_string()))?;
        self.validate_registry_root()?;
        sync_directory(&registry)?;
        let staging_basename = format!("restore-{db_instance_operation_id}");
        let staging_directory = registry.join(&staging_basename);
        self.root_directory
            .create_dir(Path::new(".hivegui-db-staging-v1").join(&staging_basename))
            .map_err(|error| ImportError::Io(error.to_string()))?;
        sync_directory(&registry)?;
        let manifest_staging = staging_directory.join(".hivegui-db-instance-v1.json.staging");
        let manifest_path = staging_directory.join(".hivegui-db-instance-v1.json");
        let instance_manifest = RestoreInstanceManifest {
            schema_version: 1,
            role: "restore".into(),
            db_instance_operation_id: db_instance_operation_id.clone(),
            db_id: format!("restore/{db_instance_operation_id}"),
            database_name: DATABASE_FILENAME.into(),
            ownership_state: "unarmed".into(),
            cleanup_operation_id: cleanup_operation_id.clone(),
        };
        let bytes = serde_json::to_vec(&instance_manifest)
            .map_err(|error| ImportError::InvalidManifest(error.to_string()))?;
        let mut file = OpenOptions::new()
            .create_new(true)
            .write(true)
            .open(&manifest_staging)
            .map_err(|error| ImportError::Io(error.to_string()))?;
        file.write_all(&bytes)
            .and_then(|_| file.sync_all())
            .map_err(|error| ImportError::Io(error.to_string()))?;
        drop(file);
        publish_no_replace(&manifest_staging, &manifest_path)
            .map_err(|error| ImportError::Io(error.to_string()))?;
        sync_directory(&staging_directory)?;
        let import_result = async {
            let manifest = BackupImporter::new(&staging_directory)
                .inspect_manifest_bound(archive.clone(), passphrase)
                .await?;
            BackupImporter::new(&staging_directory)
                .import_age_bound_with_key_root(
                    archive,
                    passphrase,
                    staging_directory.clone(),
                    io_root,
                )
                .await?;
            Ok::<_, ImportError>(manifest)
        }
        .await;
        let manifest = match import_result {
            Ok(manifest) => manifest,
            Err(error) => {
                retire_unarmed_instance(&registry, &staging_basename)?;
                return Err(error);
            }
        };
        sync_directory(&staging_directory)?;
        Ok(RestorePlan {
            db_instance_operation_id,
            cleanup_operation_id,
            manifest,
            staging_directory,
            owner_exists: false,
        })
    }

    /// Fully authenticate, materialize, and validate one unarmed/no-owner
    /// restore instance while the current Store remains writable. Only after
    /// this succeeds may a caller display the exact safety path and request
    /// final confirmation.
    pub async fn preview_restore(
        &self,
        archive: &Path,
        passphrase: &str,
    ) -> Result<PreparedRestore, ImportError> {
        self.preview_restore_internal(archive, passphrase, None, None)
            .await
    }

    /// Test-only synchronization wrapper around the production preview path.
    ///
    /// The barriers only pause after the archive descriptor is pinned and
    /// after staging validation completes. They never select an archive,
    /// decrypt bytes, or alter cleanup behavior.
    #[doc(hidden)]
    pub async fn preview_restore_with_archive_read_interlock_for_test(
        &self,
        archive: &Path,
        passphrase: &str,
        barriers: [Arc<tokio::sync::Barrier>; 4],
    ) -> Result<PreparedRestore, ImportError> {
        self.preview_restore_internal(archive, passphrase, Some(barriers), None)
            .await
    }

    /// Test-only pause after the first Store-owner check and before maintenance.
    #[doc(hidden)]
    pub async fn preview_restore_with_owner_admission_interlock_for_test(
        &self,
        archive: &Path,
        passphrase: &str,
        barriers: [Arc<tokio::sync::Barrier>; 2],
    ) -> Result<PreparedRestore, ImportError> {
        self.preview_restore_internal(archive, passphrase, None, Some(barriers))
            .await
    }

    async fn preview_restore_internal(
        &self,
        archive: &Path,
        passphrase: &str,
        barriers: Option<[Arc<tokio::sync::Barrier>; 4]>,
        owner_interlock: Option<[Arc<tokio::sync::Barrier>; 2]>,
    ) -> Result<PreparedRestore, ImportError> {
        if !self.store.as_ref().is_some_and(Store::has_restore_owner) {
            return Err(ImportError::StorageRecoveryBlocked {
                reason: "connections_open",
                artifact: "connection",
            });
        }
        wait_restore_preview_owner_interlock(owner_interlock.as_ref()).await;
        let maintenance_lease = self.acquire_restore_maintenance()?;
        if !self.store.as_ref().is_some_and(Store::has_restore_owner) {
            import_maintenance_result(
                maintenance_lease.release(StoreMaintenancePhase::RestoreReady),
            )?;
            self.clear_restore_maintenance_if_exact(&maintenance_lease)?;
            return Err(ImportError::StorageRecoveryBlocked {
                reason: "connections_open",
                artifact: "connection",
            });
        }
        let result = async {
            let archive_binding = Arc::new(VerifiedArchiveBinding::open(archive)?);
            if let Some(barriers) = barriers.as_ref() {
                barriers[0].wait().await;
                barriers[1].wait().await;
            }
            let plan = self
                .prepare_restore_from_binding(archive_binding.clone(), passphrase)
                .await?;
            let staging_database = plan.staging_directory.join(DATABASE_FILENAME);
            let staging_plugins = plan.staging_directory.join("plugins");
            if let Err(error) = validate_restore_database(&staging_database, &staging_plugins).await
            {
                self.retire_preview_plan(&plan)?;
                return Err(error);
            }
            if let Some(barriers) = barriers.as_ref() {
                barriers[2].wait().await;
                barriers[3].wait().await;
            }
            if let Err(error) = archive_binding.verify_current() {
                self.retire_preview_plan(&plan)?;
                return Err(error);
            }
            let safety_backup_path = self
                .root
                .join("backups")
                .join(format!("restore-safety-{}", plan.db_instance_operation_id));
            Ok(PreparedRestore {
                plan,
                safety_backup_path,
                coordinator_marker: self.coordinator_marker.clone(),
                maintenance_lease: maintenance_lease.clone(),
                archive_binding,
                state: AtomicU8::new(PREPARED_RESTORE_READY),
            })
        }
        .await;
        if result.is_err() {
            let _ = maintenance_lease.release(super::store::StoreMaintenancePhase::RestoreReady);
            self.clear_restore_maintenance_if_exact(&maintenance_lease)?;
        }
        result
    }

    /// Whether this exact coordinator/prepared pair is still in the Ready
    /// state and can be retired without crossing the confirmation boundary.
    pub(crate) fn preview_is_cancelable(&self, prepared: &PreparedRestore) -> bool {
        Arc::ptr_eq(&self.coordinator_marker, &prepared.coordinator_marker)
            && prepared.state.load(Ordering::SeqCst) == PREPARED_RESTORE_READY
    }

    /// Synchronously bind one ready preview to final confirmation, close the
    /// process write gate, mark the shared Pool closed, and transfer the exact
    /// Store owner while retaining its OS file lock.
    pub fn begin_confirmation(
        &self,
        prepared: &PreparedRestore,
    ) -> Result<RestoreConfirmation, ImportError> {
        self.verify_prepared_binding(prepared)?;
        let maintenance_lease = self.exact_prepared_maintenance(prepared)?;
        if prepared.state.load(Ordering::SeqCst) != PREPARED_RESTORE_READY {
            return Err(ImportError::PreparedRestoreNotActive);
        }
        prepared.archive_binding.verify_current()?;
        prepared
            .state
            .compare_exchange(
                PREPARED_RESTORE_READY,
                PREPARED_RESTORE_CONFIRMING,
                Ordering::SeqCst,
                Ordering::SeqCst,
            )
            .map_err(|_| ImportError::PreparedRestoreNotActive)?;
        maintenance_lease
            .transition(
                super::store::StoreMaintenancePhase::RestoreReady,
                super::store::StoreMaintenancePhase::RestoreConfirmation,
            )
            .map_err(import_maintenance_error)?;
        #[cfg(test)]
        self.maybe_fail_begin_confirmation_after_cas_before_freeze_for_test(prepared)?;
        let store = self
            .store
            .as_ref()
            .ok_or(ImportError::StorageRecoveryBlocked {
                reason: "connections_open",
                artifact: "connection",
            })?;
        frozen_databases()
            .lock()
            .map_err(|_| ImportError::Io("write gate poisoned".into()))?
            .insert(store.database_path().to_path_buf());
        let pool = store.pool().clone();
        drop(pool.close());
        let mut owner_slot = self
            .restore_owner_guard
            .lock()
            .map_err(|_| ImportError::Io("restore owner guard poisoned".into()))?;
        if owner_slot.is_some() {
            return Err(ImportError::PreparedRestoreNotActive);
        }
        let owner =
            store
                .take_restore_owner_guard()
                .map_err(|_| ImportError::StorageRecoveryBlocked {
                    reason: "connections_open",
                    artifact: "connection",
                })?;
        *owner_slot = Some(owner);
        self.restore_owner_terminal.store(false, Ordering::SeqCst);
        drop(owner_slot);
        Ok(RestoreConfirmation {
            coordinator: self.clone(),
            plan: prepared.plan.clone(),
            pool,
        })
    }

    #[cfg(test)]
    pub(crate) fn install_begin_confirmation_post_cas_pre_freeze_failure_for_test(
        &self,
        prepared: &PreparedRestore,
    ) -> Result<(), ImportError> {
        self.verify_prepared_binding(prepared)?;
        if prepared.state.load(Ordering::SeqCst) != PREPARED_RESTORE_READY {
            return Err(ImportError::PreparedRestoreNotActive);
        }
        let mut slot = self
            .begin_confirmation_post_cas_pre_freeze_failure
            .lock()
            .map_err(|_| ImportError::Io("confirmation test fault slot poisoned".into()))?;
        if slot.is_some() {
            return Err(ImportError::PreparedRestoreNotActive);
        }
        *slot = Some(prepared.plan.db_instance_operation_id.clone());
        Ok(())
    }

    #[cfg(test)]
    fn maybe_fail_begin_confirmation_after_cas_before_freeze_for_test(
        &self,
        prepared: &PreparedRestore,
    ) -> Result<(), ImportError> {
        let mut slot = self
            .begin_confirmation_post_cas_pre_freeze_failure
            .lock()
            .map_err(|_| ImportError::Io("confirmation test fault slot poisoned".into()))?;
        if slot.as_deref() == Some(prepared.plan.db_instance_operation_id.as_str()) {
            slot.take();
            return Err(ImportError::InjectedCrash(
                "confirmation_post_cas_pre_freeze",
            ));
        }
        Ok(())
    }

    /// Cancel one still-ready preview and retire its complete unarmed/no-owner
    /// live instance. Confirming or already-cancelled handles are rejected
    /// without touching the active instance or write gate.
    pub async fn cancel_preview(&self, prepared: &PreparedRestore) -> Result<(), ImportError> {
        self.cancel_preview_internal(prepared, None).await
    }

    /// Test-only pause around the ordinary exact-instance retirement.
    #[doc(hidden)]
    pub async fn cancel_preview_with_retirement_interlock_for_test(
        &self,
        prepared: &PreparedRestore,
        barriers: [Arc<tokio::sync::Barrier>; 2],
    ) -> Result<(), ImportError> {
        self.cancel_preview_internal(prepared, Some(barriers)).await
    }

    async fn cancel_preview_internal(
        &self,
        prepared: &PreparedRestore,
        retirement_interlock: Option<[Arc<tokio::sync::Barrier>; 2]>,
    ) -> Result<(), ImportError> {
        self.verify_prepared_binding(prepared)?;
        let maintenance_lease = self.exact_prepared_maintenance(prepared)?;
        prepared
            .state
            .compare_exchange(
                PREPARED_RESTORE_READY,
                PREPARED_RESTORE_CANCELLED,
                Ordering::SeqCst,
                Ordering::SeqCst,
            )
            .map_err(|_| ImportError::PreparedRestoreNotActive)?;
        maintenance_lease
            .transition(
                super::store::StoreMaintenancePhase::RestoreReady,
                super::store::StoreMaintenancePhase::RestoreCancelling,
            )
            .map_err(import_maintenance_error)?;
        if let Some(retirement_barriers) = retirement_interlock.as_ref() {
            retirement_barriers[0].wait().await;
            retirement_barriers[1].wait().await;
        }
        let retirement_result = self.retire_preview_plan(&prepared.plan);
        if retirement_result.is_ok() {
            maintenance_lease
                .release(super::store::StoreMaintenancePhase::RestoreCancelling)
                .map_err(import_maintenance_error)?;
            self.clear_restore_maintenance_if_exact(&maintenance_lease)?;
        }
        retirement_result
    }

    fn verify_prepared_binding(&self, prepared: &PreparedRestore) -> Result<(), ImportError> {
        if !Arc::ptr_eq(&self.coordinator_marker, &prepared.coordinator_marker)
            || self.exact_prepared_maintenance(prepared).is_err()
        {
            return Err(ImportError::PreparedRestoreNotActive);
        }
        Ok(())
    }

    /// Freeze the exact Store bound to this coordinator after an unarmed
    /// preview cannot be retired safely. This is intentionally narrower than
    /// confirmation: it only closes the canonical database write gate and
    /// marks the shared Pool closed. It does not arm, apply, recover, publish
    /// an owner, or create a safety snapshot.
    pub(crate) fn enter_preview_cancel_failure_terminal(&self) -> Result<(), ImportError> {
        let store = self
            .store
            .as_ref()
            .ok_or(ImportError::StorageRecoveryBlocked {
                reason: "connections_open",
                artifact: "connection",
            })?;
        let database_path = store.database_path().to_path_buf();
        let gate_result = frozen_databases()
            .lock()
            .map_err(|_| ImportError::Io("write gate poisoned".into()))
            .map(|mut frozen| {
                frozen.insert(database_path);
            });
        let pool = store.pool().clone();
        drop(pool.close());
        gate_result?;
        if let Some(maintenance_lease) = self.restore_maintenance_lease()? {
            match maintenance_lease.transition(
                super::store::StoreMaintenancePhase::RestoreConfirmation,
                super::store::StoreMaintenancePhase::RestoreTerminal,
            ) {
                Ok(()) => {}
                Err(super::store::StoreMaintenanceError::Busy(
                    super::store::StoreMaintenancePhase::RestoreCancelling,
                )) => maintenance_lease
                    .transition(
                        super::store::StoreMaintenancePhase::RestoreCancelling,
                        super::store::StoreMaintenancePhase::RestoreTerminal,
                    )
                    .map_err(import_maintenance_error)?,
                Err(error) => return Err(import_maintenance_error(error)),
            }
        }
        Ok(())
    }

    fn retire_preview_plan(&self, plan: &RestorePlan) -> Result<(), ImportError> {
        let registry = self.io_root()?.join(".hivegui-db-staging-v1");
        retire_unarmed_instance(
            &registry,
            &format!("restore-{}", plan.db_instance_operation_id),
        )
    }

    /// Apply one fully authenticated plan while the canonical Store is closed.
    ///
    /// The method is an offline boundary: callers must drop all Store handles
    /// before entry. It checkpoints both databases, creates and verifies a raw
    /// safety snapshot, arms the instance, durably publishes owner phases,
    /// switches database plus Plugin tree, validates the new current, and only
    /// then publishes `committed`. Any pre-commit error attempts a complete old
    /// rollback and leaves the owner for startup replay.
    pub async fn apply_restore(&self, plan: &RestorePlan) -> Result<RestoreRecovery, ImportError> {
        self.apply_restore_internal(plan, None, None).await
    }

    /// Test-only pause after the real offline switch/retirement outcome and
    /// before its held safety proof is revalidated for public return.
    #[doc(hidden)]
    pub async fn apply_restore_with_safety_return_interlock_for_test(
        &self,
        plan: &RestorePlan,
        barriers: [Arc<tokio::sync::Barrier>; 2],
    ) -> Result<RestoreRecovery, ImportError> {
        self.apply_restore_internal(plan, None, Some(barriers))
            .await
    }

    async fn apply_restore_internal(
        &self,
        plan: &RestorePlan,
        crash_at: Option<BackupCrashPoint>,
        safety_return_interlock: Option<[Arc<tokio::sync::Barrier>; 2]>,
    ) -> Result<RestoreRecovery, ImportError> {
        let prepared = self
            .prepare_restore_for_safety(plan, crash_at, None)
            .await?;
        let write_gate_database = prepared.write_gate_database.clone();
        let verified_safety_snapshot = create_restore_safety_snapshot(
            &prepared.io_root,
            &prepared.current_database,
            &prepared.current_plugins,
            &plan.db_instance_operation_id,
        )?;
        let apply_outcome = self
            .apply_restore_after_safety(
                plan,
                crash_at,
                true,
                prepared,
                &verified_safety_snapshot,
                RestoreOldDatabaseEvidence::InspectCanonicalPath,
            )
            .await;
        wait_restore_safety_return_interlock(safety_return_interlock.as_ref()).await;
        let safety_binding = verified_safety_snapshot.revalidate();
        match (apply_outcome, safety_binding) {
            (Ok(mut recovery), Ok(())) => {
                let mut frozen = frozen_databases()
                    .lock()
                    .map_err(|_| ImportError::Io("write gate poisoned".into()))?;
                recovery.store_may_open = true;
                recovery.write_gate_open = true;
                frozen.remove(&write_gate_database);
                Ok(recovery)
            }
            (Ok(_), Err(error)) => Err(error),
            (Err(error), _) => Err(error),
        }
    }

    async fn prepare_restore_for_safety(
        &self,
        plan: &RestorePlan,
        crash_at: Option<BackupCrashPoint>,
        pinned_checkpoint_interlocks: Option<&[Arc<tokio::sync::Barrier>; 8]>,
    ) -> Result<RestorePreparedForSafety, ImportError> {
        let io_root = self.io_root()?;
        let current_database = io_root.join(DATABASE_FILENAME);
        if super::store::store_already_owned(&current_database) {
            return Err(ImportError::StorageRecoveryBlocked {
                reason: "connections_open",
                artifact: "connection",
            });
        }
        let registry = io_root.join(".hivegui-db-staging-v1");
        let live_basename = format!("restore-{}", plan.db_instance_operation_id);
        let live = registry.join(&live_basename);
        if live != plan.staging_directory || !live.is_dir() {
            return Err(ImportError::InvalidManifest(
                "restore plan is outside its controlled registry".into(),
            ));
        }
        let staging_database = live.join(DATABASE_FILENAME);
        let staging_plugins = live.join("plugins");
        validate_restore_database(&staging_database, &staging_plugins).await?;
        checkpoint_closed_database(
            &staging_database,
            &format!("restore/{}", plan.db_instance_operation_id),
            crash_at,
            pinned_checkpoint_interlocks.map(|barriers| &barriers[..4]),
        )
        .await?;

        let current_plugins = io_root.join("plugins");
        let write_gate_database = self.root.join(DATABASE_FILENAME);
        frozen_databases()
            .lock()
            .map_err(|_| ImportError::Io("write gate poisoned".into()))?
            .insert(write_gate_database.clone());
        if current_database.exists() {
            checkpoint_closed_database(
                &current_database,
                "current",
                crash_at,
                pinned_checkpoint_interlocks.map(|barriers| &barriers[4..]),
            )
            .await?;
        }
        let root_dir = self
            .root_directory
            .try_clone()
            .map_err(|error| ImportError::Io(error.to_string()))?;
        let registry_dir = root_dir
            .open_dir(".hivegui-db-staging-v1")
            .map_err(|_| ImportError::UnsafeArchiveEntry(registry.display().to_string()))?;
        let live_dir = registry_dir
            .open_dir(Path::new(&live_basename))
            .map_err(|_| ImportError::UnsafeArchiveEntry(live.display().to_string()))?;
        let current_plugins_dir = match root_dir.symlink_metadata(Path::new("plugins")) {
            Ok(metadata) if metadata.is_dir() && !metadata.file_type().is_symlink() => Some(
                root_dir
                    .open_dir(Path::new("plugins"))
                    .map_err(|_| ImportError::UnsafeArchiveEntry("plugins".into()))?,
            ),
            Ok(_) => return Err(ImportError::UnsafeArchiveEntry("plugins".into())),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => None,
            Err(error) => return Err(ImportError::Io(error.to_string())),
        };

        Ok(RestorePreparedForSafety {
            io_root,
            root_dir,
            current_database,
            registry,
            live_basename,
            live,
            live_dir,
            staging_database,
            staging_plugins,
            current_plugins,
            current_plugins_dir,
            write_gate_database,
        })
    }

    async fn apply_restore_after_safety(
        &self,
        plan: &RestorePlan,
        crash_at: Option<BackupCrashPoint>,
        keep_frozen: bool,
        prepared: RestorePreparedForSafety,
        verified_safety_snapshot: &VerifiedRestoreSafetySnapshot,
        old_database_source: RestoreOldDatabaseEvidence,
    ) -> Result<RestoreRecovery, ImportError> {
        let RestorePreparedForSafety {
            io_root,
            root_dir,
            current_database,
            registry,
            live_basename,
            live,
            live_dir,
            staging_database,
            staging_plugins,
            current_plugins,
            current_plugins_dir: _,
            write_gate_database,
        } = prepared;

        let manifest_path = live.join(".hivegui-db-instance-v1.json");
        let mut instance_manifest: RestoreInstanceManifest =
            serde_json::from_slice(&read_control_file_nofollow(&manifest_path)?)
                .map_err(|_| ImportError::InvalidManifest("invalid restore manifest".into()))?;
        if instance_manifest.ownership_state != "unarmed"
            || instance_manifest.db_instance_operation_id != plan.db_instance_operation_id
        {
            return Err(ImportError::InvalidManifest(
                "restore manifest cannot be armed".into(),
            ));
        }
        instance_manifest.ownership_state = "armed".into();
        publish_instance_manifest(&live, &instance_manifest, crash_at)?;
        maybe_inject_restore_crash(crash_at, "manifest_arm")?;
        let live_relative = PathBuf::from(".hivegui-db-staging-v1").join(&live_basename);
        let manifest_evidence = controlled_file_evidence(
            &manifest_path,
            &live_relative.join(".hivegui-db-instance-v1.json"),
        )?;
        let (mut bound_old_database, inspected_old_database_evidence) = match old_database_source {
            RestoreOldDatabaseEvidence::InspectCanonicalPath => (
                None,
                optional_controlled_file_evidence(&current_database, Path::new(DATABASE_FILENAME))?,
            ),
            RestoreOldDatabaseEvidence::Bound(bound_old_database) => {
                let evidence = bound_old_database
                    .as_ref()
                    .map(|bound| bound.evidence.clone());
                (bound_old_database, evidence)
            }
        };
        let mut owner = RestoreOwner {
            schema_version: 1,
            role: "restore".into(),
            db_instance_operation_id: plan.db_instance_operation_id.clone(),
            db_id: instance_manifest.db_id.clone(),
            instance_basename: live_basename.clone(),
            manifest_identity: manifest_evidence.identity,
            manifest_sha256: manifest_evidence.sha256,
            ownership_state: "armed".into(),
            old_database: bound_old_database
                .as_ref()
                .map(|bound| bound.evidence.clone())
                .or(inspected_old_database_evidence),
            new_database: controlled_file_evidence(
                &staging_database,
                &live_relative.join(DATABASE_FILENAME),
            )?,
            old_plugin_root: optional_controlled_tree_evidence(
                &current_plugins,
                Path::new("plugins"),
            )?,
            new_plugin_root: controlled_tree_evidence(
                &staging_plugins,
                &live_relative.join("plugins"),
            )?,
            safety_snapshot: verified_safety_snapshot.evidence.clone(),
            phase: "prepared".into(),
        };
        publish_restore_owner(&live, &owner, true, crash_at)?;
        maybe_inject_restore_crash(crash_at, "owner_prepared_publish")?;
        owner.phase = "applying".into();
        publish_restore_owner(&live, &owner, false, crash_at)?;
        maybe_inject_restore_crash(crash_at, "owner_applying_publish")?;

        let old_database = live.join("old-datasources.db");
        let old_plugins = live.join("old-plugins");
        if let Some(mut bound_old_database) = bound_old_database.take() {
            move_current_database_with_bound_identity(
                &root_dir,
                &live_dir,
                &mut bound_old_database,
            )?;
        } else if owner.old_database.is_some() {
            publish_no_replace_at(
                &root_dir,
                Path::new(DATABASE_FILENAME),
                &live_dir,
                Path::new("old-datasources.db"),
            )
            .map_err(|error| ImportError::Io(error.to_string()))?;
        }
        if let Err(error) = publish_no_replace_at(
            &live_dir,
            Path::new(DATABASE_FILENAME),
            &root_dir,
            Path::new(DATABASE_FILENAME),
        ) {
            if owner.old_database.is_some() {
                let _ = publish_no_replace_at(
                    &live_dir,
                    Path::new("old-datasources.db"),
                    &root_dir,
                    Path::new(DATABASE_FILENAME),
                );
            }
            return Err(ImportError::Io(error.to_string()));
        }
        sync_cap_directory(&root_dir, "fsync restore root after database switch")?;
        sync_cap_directory(&live_dir, "fsync restore live after database switch")?;
        maybe_inject_restore_crash(crash_at, "database_switch")?;
        if current_plugins.exists() {
            publish_no_replace(&current_plugins, &old_plugins)
                .map_err(|error| ImportError::Io(error.to_string()))?;
        }
        if let Err(error) = publish_no_replace(&staging_plugins, &current_plugins) {
            let _ = publish_no_replace(&current_database, &staging_database);
            if old_database.exists() {
                let _ = publish_no_replace(&old_database, &current_database);
            }
            if old_plugins.exists() {
                let _ = publish_no_replace(&old_plugins, &current_plugins);
            }
            return Err(ImportError::Io(error.to_string()));
        }
        sync_directory(&io_root)?;
        sync_directory(&live)?;
        maybe_inject_restore_crash(crash_at, "plugin_tree_switch")?;

        if let Err(error) = validate_restore_database(&current_database, &current_plugins).await {
            let _ = rollback_restore_switch(
                &current_database,
                &current_plugins,
                &staging_database,
                &staging_plugins,
                &old_database,
                &old_plugins,
            );
            return Err(error);
        }
        maybe_inject_restore_crash(crash_at, "new_health_verify")?;
        maybe_inject_restore_crash(crash_at, "new_search_verify")?;
        maybe_inject_restore_crash(crash_at, "new_artifact_verify")?;
        let current_database_evidence =
            controlled_file_evidence(&current_database, Path::new(DATABASE_FILENAME))?;
        let current_plugin_evidence =
            controlled_tree_evidence(&current_plugins, Path::new("plugins"))?;
        if !same_file_object(Some(&current_database_evidence), Some(&owner.new_database))
            || !same_tree_object(Some(&current_plugin_evidence), Some(&owner.new_plugin_root))
        {
            let _ = rollback_restore_switch(
                &current_database,
                &current_plugins,
                &staging_database,
                &staging_plugins,
                &old_database,
                &old_plugins,
            );
            return Err(ImportError::InvalidManifest(
                "new current identity mismatch".into(),
            ));
        }
        maybe_inject_restore_crash(crash_at, "new_identity_verify")?;
        owner.phase = "committed".into();
        publish_restore_owner(&live, &owner, false, crash_at)?;
        maybe_inject_restore_crash(crash_at, "owner_committed_publish")?;
        retire_terminal_instance_with_crash(
            &registry,
            &live_basename,
            &instance_manifest,
            &owner,
            RetirementOutcome::New,
            crash_at,
        )?;
        if !keep_frozen {
            frozen_databases()
                .lock()
                .map_err(|_| ImportError::Io("write gate poisoned".into()))?
                .remove(&write_gate_database);
        }
        Ok(RestoreRecovery {
            retirement_outcome: RetirementOutcome::New,
            store_may_open: !keep_frozen,
            exactly_one_live_database: current_database.is_file(),
            no_mixed_database_or_plugin_tree: current_plugins.is_dir(),
            control_files_outside_live_tree: true,
            retirement_done: true,
            write_gate_open: !keep_frozen,
        })
    }

    /// Advance the real switch/retirement state machine to the requested
    /// durability boundary and then simulate process interruption.
    pub async fn apply_with_crash(
        &self,
        plan: &RestorePlan,
        crash_point: BackupCrashPoint,
    ) -> Result<(), RestoreCrashError> {
        match self
            .apply_restore_internal(plan, Some(crash_point), None)
            .await
        {
            Err(ImportError::InjectedCrash(actual)) if actual == crash_point.as_str() => {
                Err(RestoreCrashError {
                    crash_point: actual,
                    cause: "injected".into(),
                })
            }
            Err(error) => Err(RestoreCrashError {
                crash_point: crash_point.as_str(),
                cause: error.to_string(),
            }),
            Ok(_) => Err(RestoreCrashError {
                crash_point: crash_point.as_str(),
                cause: "requested boundary was not reached".into(),
            }),
        }
    }

    /// Replay an interrupted restore to one conservative proven outcome.
    pub async fn recover_startup(&self) -> Result<RestoreRecovery, ImportError> {
        self.recover_startup_internal(None, None).await
    }

    /// Test-only synchronization wrapper around the ordinary startup replay.
    #[doc(hidden)]
    pub async fn recover_startup_with_replay_interlock_for_test(
        &self,
        barriers: [Arc<tokio::sync::Barrier>; 2],
    ) -> Result<RestoreRecovery, ImportError> {
        self.recover_startup_internal(Some(barriers), None).await
    }

    /// Test-only pause after observing recovery state and before admission CAS.
    #[doc(hidden)]
    pub async fn recover_startup_with_admission_interlock_for_test(
        &self,
        barriers: [Arc<tokio::sync::Barrier>; 2],
    ) -> Result<RestoreRecovery, ImportError> {
        self.recover_startup_internal(None, Some(barriers)).await
    }

    async fn recover_startup_internal(
        &self,
        replay_interlock: Option<[Arc<tokio::sync::Barrier>; 2]>,
        admission_interlock: Option<[Arc<tokio::sync::Barrier>; 2]>,
    ) -> Result<RestoreRecovery, ImportError> {
        let recovery_state_before_admission = self.restore_recovery_state.load(Ordering::SeqCst);
        wait_restore_recovery_admission_interlock(admission_interlock.as_ref()).await;
        if recovery_state_before_admission == RESTORE_RECOVERY_DONE {
            return Err(ImportError::StorageRecoveryBlocked {
                reason: "connections_open",
                artifact: "connection",
            });
        }
        let admission = self.restore_recovery_state.compare_exchange(
            RESTORE_RECOVERY_IDLE,
            RESTORE_RECOVERY_RUNNING,
            Ordering::SeqCst,
            Ordering::SeqCst,
        );
        let mut recovery_admission = match admission {
            Err(RESTORE_RECOVERY_RUNNING) => {
                return Err(ImportError::MaintenanceBusy {
                    active: StoreMaintenancePhase::RestoreTerminal.active(),
                });
            }
            Err(RESTORE_RECOVERY_DONE) => {
                return Err(ImportError::StorageRecoveryBlocked {
                    reason: "connections_open",
                    artifact: "connection",
                });
            }
            Err(_) => return Err(import_maintenance_registry_unavailable()),
            Ok(_) => RestoreRecoveryAdmission::new(self.restore_recovery_state.clone()),
        };
        let maintenance_lease = self.restore_maintenance_lease()?;
        let maintenance_snapshot =
            super::store::store_maintenance_snapshot(&self.maintenance_database_path())
                .map_err(import_maintenance_error)?;
        let exact_terminal_lease = match (maintenance_lease.as_ref(), maintenance_snapshot) {
            (Some(maintenance_lease), Some(snapshot))
                if maintenance_lease.owner_id() == snapshot.owner_id =>
            {
                if snapshot.phase != StoreMaintenancePhase::RestoreTerminal {
                    return Err(ImportError::StorageRecoveryBlocked {
                        reason: "connections_open",
                        artifact: "connection",
                    });
                }
                import_maintenance_result(maintenance_lease.claim_terminal_recovery())?;
                Some(maintenance_lease.clone())
            }
            (Some(_), Some(snapshot)) | (None, Some(snapshot)) => {
                return Err(ImportError::MaintenanceBusy {
                    active: snapshot.phase.active(),
                });
            }
            (Some(_), None) => return Err(import_maintenance_registry_unavailable()),
            (None, None) => None,
        };
        let transferred_owner = self
            .restore_owner_guard
            .lock()
            .map_err(|_| ImportError::Io("restore owner guard poisoned".into()))?
            .is_some();
        if transferred_owner && !self.restore_owner_terminal.load(Ordering::SeqCst) {
            return Err(ImportError::StorageRecoveryBlocked {
                reason: "connections_open",
                artifact: "connection",
            });
        }
        if !transferred_owner && self.store.as_ref().is_some_and(Store::has_restore_owner) {
            return Err(ImportError::StorageRecoveryBlocked {
                reason: "connections_open",
                artifact: "connection",
            });
        }
        wait_restore_recovery_replay_interlock(replay_interlock.as_ref()).await;
        self.validate_registry_root()?;
        let io_root = self.io_root()?;
        let registry = io_root.join(".hivegui-db-staging-v1");
        let retirement_outcome = if registry.exists() {
            replay_registry_retirements(&io_root, &registry).await?
        } else {
            let current_database = io_root.join(DATABASE_FILENAME);
            if current_database.exists() {
                match replay_sidecar_cleanup_for_database(&current_database, "current") {
                    Ok(()) => {}
                    Err(ImportError::StorageRecoveryBlocked {
                        reason: "sidecar_hot",
                        artifact: "wal",
                    }) => {
                        checkpoint_closed_database(&current_database, "current", None, None)
                            .await?;
                    }
                    Err(error) => return Err(error),
                }
            }
            RetirementOutcome::AbortedPreSwitch
        };
        let mut owner_slot = self
            .restore_owner_guard
            .lock()
            .map_err(|_| ImportError::Io("restore owner guard poisoned".into()))?;
        let mut frozen = frozen_databases()
            .lock()
            .map_err(|_| ImportError::Io("write gate poisoned".into()))?;
        let recovery = RestoreRecovery {
            retirement_outcome,
            store_may_open: true,
            exactly_one_live_database: true,
            no_mixed_database_or_plugin_tree: true,
            control_files_outside_live_tree: true,
            retirement_done: true,
            write_gate_open: true,
        };
        let Some(maintenance_lease) = exact_terminal_lease.as_ref() else {
            frozen.remove(&self.root.join(DATABASE_FILENAME));
            drop(owner_slot.take());
            self.restore_owner_terminal.store(false, Ordering::SeqCst);
            recovery_admission.complete();
            return Ok(recovery);
        };
        let mut maintenance_slot = self.maintenance_lease.lock();
        let maintenance_slot = maintenance_slot
            .as_mut()
            .map_err(|_| ImportError::Io("restore maintenance slot poisoned".into()))?;
        if !maintenance_slot
            .as_ref()
            .is_some_and(|current| Arc::ptr_eq(current, maintenance_lease))
        {
            return Err(ImportError::Io("maintenance registry unavailable".into()));
        }
        import_maintenance_result(maintenance_lease.release_with_finalize(
            StoreMaintenancePhase::RestoreTerminal,
            || {
                frozen.remove(&self.root.join(DATABASE_FILENAME));
                drop(owner_slot.take());
                self.restore_owner_terminal.store(false, Ordering::SeqCst);
                recovery_admission.complete();
                maintenance_slot.take();
            },
        ))?;
        Ok(recovery)
    }
}

async fn wait_restore_preview_owner_interlock(owner: Option<&[Arc<tokio::sync::Barrier>; 2]>) {
    if let Some(owner_barriers) = owner {
        owner_barriers[0].wait().await;
        owner_barriers[1].wait().await;
    }
}

async fn wait_restore_safety_return_interlock(interlock: Option<&[Arc<tokio::sync::Barrier>; 2]>) {
    if let Some(barriers) = interlock {
        barriers[0].wait().await;
        barriers[1].wait().await;
    }
}

async fn wait_restore_recovery_replay_interlock(
    replay_interlock: Option<&[Arc<tokio::sync::Barrier>; 2]>,
) {
    if let Some(recovery_barriers) = replay_interlock {
        recovery_barriers[0].wait().await;
        recovery_barriers[1].wait().await;
    }
}

async fn wait_restore_recovery_admission_interlock(
    admission: Option<&[Arc<tokio::sync::Barrier>; 2]>,
) {
    if let Some(recovery_barriers) = admission {
        recovery_barriers[0].wait().await;
        recovery_barriers[1].wait().await;
    }
}

const SIDECAR_CLEANUP_DOMAIN: &[u8] = b"hivegui-sidecar-cleanup-v1";

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
enum CleanupArtifact {
    Wal,
    RollbackJournal,
    Shm,
}

impl CleanupArtifact {
    const ALL: [Self; 3] = [Self::Wal, Self::RollbackJournal, Self::Shm];

    fn as_str(self) -> &'static str {
        match self {
            Self::Wal => "wal",
            Self::RollbackJournal => "rollback_journal",
            Self::Shm => "shm",
        }
    }

    fn canonical_name(self, database_name: &str) -> String {
        match self {
            Self::Wal => format!("{database_name}-wal"),
            Self::RollbackJournal => format!("{database_name}-journal"),
            Self::Shm => format!("{database_name}-shm"),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
enum SidecarCleanupState {
    Prepared,
    Quarantined,
    Done,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct SidecarCleanupJournal {
    schema_version: u32,
    cleanup_operation_id: String,
    db_id: String,
    database_identity: String,
    artifact: CleanupArtifact,
    canonical_name: String,
    expected_identity: String,
    expected_size_bytes: u64,
    expected_sha256: String,
    quarantine_name: String,
    state: SidecarCleanupState,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct FileEvidence {
    identity: String,
    size_bytes: u64,
    sha256: String,
}

fn sidecar_cleanup_token(db_id: &str) -> String {
    let bytes = db_id.as_bytes();
    let mut digest = Sha256::new();
    digest.update(SIDECAR_CLEANUP_DOMAIN);
    digest.update([0]);
    digest.update((bytes.len() as u32).to_be_bytes());
    digest.update(bytes);
    hex::encode(digest.finalize())
}

fn cleanup_journal_name(db_id: &str, artifact: CleanupArtifact) -> String {
    format!(
        ".hivegui-sidecar-cleanup-v1-{}-{}.json",
        sidecar_cleanup_token(db_id),
        artifact.as_str()
    )
}

fn stable_file_identity(metadata: &fs::Metadata) -> Result<String, ImportError> {
    #[cfg(unix)]
    {
        use std::os::unix::fs::MetadataExt as _;

        Ok(format!("unix:{}:{}", metadata.dev(), metadata.ino()))
    }
    #[cfg(not(unix))]
    {
        let modified = metadata
            .modified()
            .and_then(|time| {
                time.duration_since(std::time::UNIX_EPOCH)
                    .map_err(std::io::Error::other)
            })
            .map_err(|error| ImportError::Io(error.to_string()))?
            .as_nanos();
        Ok(format!("portable:{}:{modified}", metadata.len()))
    }
}

fn stable_cap_file_identity(metadata: &cap_std::fs::Metadata) -> Result<String, ImportError> {
    #[cfg(unix)]
    {
        use cap_std::fs::MetadataExt as _;

        Ok(format!("unix:{}:{}", metadata.dev(), metadata.ino()))
    }
    #[cfg(not(unix))]
    {
        let modified = metadata
            .modified()
            .map(cap_std::time::SystemTime::into_std)
            .and_then(|time| {
                time.duration_since(std::time::UNIX_EPOCH)
                    .map_err(std::io::Error::other)
            })
            .map_err(|error| ImportError::Io(error.to_string()))?
            .as_nanos();
        Ok(format!("portable:{}:{modified}", metadata.len()))
    }
}

fn stable_file_identity_from_cap_metadata(
    metadata: &cap_std::fs::Metadata,
) -> Result<String, ImportError> {
    stable_cap_file_identity(metadata)
}

fn owned_regular_file_link_count(_metadata: &fs::Metadata) -> bool {
    #[cfg(unix)]
    {
        use std::os::unix::fs::MetadataExt as _;

        _metadata.nlink() == 1
    }
    #[cfg(not(unix))]
    {
        true
    }
}

fn current_database_identity_changed() -> ImportError {
    ImportError::UnsafeArchiveEntry("current database identity changed".into())
}

fn move_current_database_with_bound_identity(
    root_dir: &cap_std::fs::Dir,
    live_dir: &cap_std::fs::Dir,
    bound_old_database: &mut BoundOldDatabase,
) -> Result<(), ImportError> {
    let held_metadata = bound_old_database.held_file.metadata();
    let held_metadata = held_metadata.map_err(|_| current_database_identity_changed())?;
    let held_identity = stable_file_identity(&held_metadata)?;
    let expected_bound_identity = bound_old_database.evidence.identity.as_str();
    if !held_metadata.is_file()
        || !owned_regular_file_link_count(&held_metadata)
        || held_identity != bound_old_database.canonical_identity
        || held_identity != expected_bound_identity
    {
        return Err(current_database_identity_changed());
    }

    publish_no_replace_at(
        root_dir,
        Path::new(DATABASE_FILENAME),
        live_dir,
        Path::new("old-datasources.db"),
    )
    .map_err(|error| ImportError::Io(error.to_string()))?;
    sync_cap_directory(root_dir, "fsync restore root after bound current move")?;
    sync_cap_directory(live_dir, "fsync restore live after bound current move")?;

    let moved_metadata = live_dir.symlink_metadata(Path::new("old-datasources.db"));
    let moved_metadata = moved_metadata.map_err(|error| ImportError::Io(error.to_string()))?;
    let moved_identity = stable_file_identity_from_cap_metadata(&moved_metadata)?;
    let moved_identity_mismatch =
        moved_identity.as_str() != bound_old_database.evidence.identity.as_str();
    if !moved_metadata.is_file()
        || moved_metadata.file_type().is_symlink()
        || moved_identity_mismatch
    {
        publish_no_replace_at(
            live_dir,
            Path::new("old-datasources.db"),
            root_dir,
            Path::new(DATABASE_FILENAME),
        )
        .map_err(|error| ImportError::Io(error.to_string()))?;
        sync_cap_directory(root_dir, "fsync restore root after bound current rollback")?;
        sync_cap_directory(live_dir, "fsync restore live after bound current rollback")?;
        let restored_metadata = root_dir.symlink_metadata(Path::new(DATABASE_FILENAME));
        let restored_metadata =
            restored_metadata.map_err(|error| ImportError::Io(error.to_string()))?;
        let restored_identity = stable_file_identity_from_cap_metadata(&restored_metadata)?;
        if !restored_metadata.is_file()
            || restored_metadata.file_type().is_symlink()
            || restored_identity != moved_identity
        {
            return Err(ImportError::UnsafeArchiveEntry(
                "current database rollback identity changed".into(),
            ));
        }
        return Err(current_database_identity_changed());
    }
    Ok(())
}

fn inspect_owned_regular_file(path: &Path) -> Result<Option<FileEvidence>, ImportError> {
    let entry = match fs::symlink_metadata(path) {
        Ok(entry) => entry,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(error) => return Err(ImportError::Io(error.to_string())),
    };
    if !entry.is_file() || entry.file_type().is_symlink() {
        return Err(ImportError::InvalidManifest(format!(
            "unsafe sidecar path {}",
            path.display()
        )));
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::MetadataExt as _;

        if entry.nlink() != 1 {
            return Err(ImportError::InvalidManifest(format!(
                "unsafe sidecar link count {}",
                path.display()
            )));
        }
    }
    let mut options = OpenOptions::new();
    options.read(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt as _;

        options.custom_flags(libc::O_NOFOLLOW | libc::O_CLOEXEC | libc::O_NONBLOCK);
    }
    let mut file = options
        .open(path)
        .map_err(|error| ImportError::Io(error.to_string()))?;
    let before = file
        .metadata()
        .map_err(|error| ImportError::Io(error.to_string()))?;
    if !before.is_file() {
        return Err(ImportError::InvalidManifest(format!(
            "unsafe sidecar file {}",
            path.display()
        )));
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::MetadataExt as _;

        if before.nlink() != 1 {
            return Err(ImportError::InvalidManifest(format!(
                "unsafe sidecar link count {}",
                path.display()
            )));
        }
    }
    let mut digest = Sha256::new();
    let mut size_bytes = 0_u64;
    let mut buffer = [0_u8; 64 * 1024];
    loop {
        let read = file
            .read(&mut buffer)
            .map_err(|error| ImportError::Io(error.to_string()))?;
        if read == 0 {
            break;
        }
        size_bytes = size_bytes
            .checked_add(read as u64)
            .ok_or_else(|| ImportError::Io("sidecar size overflow".into()))?;
        digest.update(&buffer[..read]);
    }
    let after = file
        .metadata()
        .map_err(|error| ImportError::Io(error.to_string()))?;
    if stable_file_identity(&before)? != stable_file_identity(&after)?
        || before.len() != after.len()
        || size_bytes != after.len()
    {
        return Err(ImportError::InvalidManifest(format!(
            "sidecar identity changed {}",
            path.display()
        )));
    }
    Ok(Some(FileEvidence {
        identity: stable_file_identity(&after)?,
        size_bytes,
        sha256: hex::encode(digest.finalize()),
    }))
}

fn evidence_matches(evidence: &FileEvidence, journal: &SidecarCleanupJournal) -> bool {
    evidence.identity == journal.expected_identity
        && evidence.size_bytes == journal.expected_size_bytes
        && evidence.sha256 == journal.expected_sha256
}

fn storage_recovery_blocked(reason: &'static str, artifact: CleanupArtifact) -> ImportError {
    ImportError::StorageRecoveryBlocked {
        reason,
        artifact: artifact.as_str(),
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct RecoveryBlock {
    reason: &'static str,
    artifact: CleanupArtifact,
}

fn recovery_reason_priority(reason: &str) -> u8 {
    match reason {
        "checkpoint_failed" => 0,
        "checkpoint_busy" => 1,
        "connections_open" => 2,
        "sidecar_reappeared" => 3,
        "sidecar_hot" => 4,
        "sidecar_recoverable" => 5,
        "sidecar_unknown_owner" => 6,
        "sidecar_cleanup_failed" => 7,
        _ => u8::MAX,
    }
}

fn recovery_artifact_priority(artifact: CleanupArtifact) -> u8 {
    match artifact {
        CleanupArtifact::Wal => 0,
        CleanupArtifact::RollbackJournal => 1,
        CleanupArtifact::Shm => 2,
    }
}

fn select_recovery_block(candidates: &[RecoveryBlock]) -> Option<ImportError> {
    candidates
        .iter()
        .copied()
        .min_by_key(|candidate| {
            (
                recovery_reason_priority(candidate.reason),
                recovery_artifact_priority(candidate.artifact),
            )
        })
        .map(|candidate| storage_recovery_blocked(candidate.reason, candidate.artifact))
}

fn classify_journal_paths(
    journal: &SidecarCleanupJournal,
    canonical: Option<&FileEvidence>,
    quarantine: Option<&FileEvidence>,
) -> Option<RecoveryBlock> {
    let block = |reason| RecoveryBlock {
        reason,
        artifact: journal.artifact,
    };
    match journal.state {
        SidecarCleanupState::Prepared => match (canonical, quarantine) {
            (Some(canonical), None) if evidence_matches(canonical, journal) => None,
            (Some(canonical), _) if !evidence_matches(canonical, journal) => {
                Some(block("sidecar_reappeared"))
            }
            (None, Some(quarantine)) if evidence_matches(quarantine, journal) => None,
            _ => Some(block("sidecar_unknown_owner")),
        },
        SidecarCleanupState::Quarantined => match (canonical, quarantine) {
            (Some(canonical), _) if !evidence_matches(canonical, journal) => {
                Some(block("sidecar_reappeared"))
            }
            (Some(_), _) => Some(block("sidecar_unknown_owner")),
            (None, Some(quarantine)) if !evidence_matches(quarantine, journal) => {
                Some(block("sidecar_unknown_owner"))
            }
            (None, _) => None,
        },
        SidecarCleanupState::Done => match (canonical, quarantine) {
            (Some(canonical), _) if !evidence_matches(canonical, journal) => {
                Some(block("sidecar_reappeared"))
            }
            (Some(_), _) | (None, Some(_)) => Some(block("sidecar_unknown_owner")),
            (None, None) => None,
        },
    }
}

fn artifact_from_control_name(name: &str) -> CleanupArtifact {
    if name.ends_with("-rollback_journal.json")
        || name.ends_with("-rollback_journal.json.staging")
        || name.ends_with("-rollback_journal")
    {
        CleanupArtifact::RollbackJournal
    } else if name.ends_with("-shm.json")
        || name.ends_with("-shm.json.staging")
        || name.ends_with("-shm")
    {
        CleanupArtifact::Shm
    } else {
        CleanupArtifact::Wal
    }
}

fn validate_sidecar_cleanup_journal(
    database: &Path,
    db_id: &str,
    artifact: CleanupArtifact,
    journal_path: &Path,
    journal: &SidecarCleanupJournal,
) -> Result<(), ImportError> {
    let database_name = database
        .file_name()
        .and_then(|name| name.to_str())
        .ok_or_else(|| storage_recovery_blocked("sidecar_unknown_owner", artifact))?;
    let database_metadata = fs::metadata(database)
        .map_err(|_| storage_recovery_blocked("sidecar_unknown_owner", artifact))?;
    let cleanup_operation_id = Uuid::parse_str(&journal.cleanup_operation_id)
        .map_err(|_| storage_recovery_blocked("sidecar_unknown_owner", artifact))?;
    let expected_quarantine = format!(
        ".hivegui-sidecar-quarantine-v1-{cleanup_operation_id}-{}",
        artifact.as_str()
    );
    let expected_name = cleanup_journal_name(db_id, artifact);
    let instance_id = db_id.split_once('/').map(|(_, operation_id)| operation_id);
    if journal.schema_version != 1
        || journal.db_id != db_id
        || journal.artifact != artifact
        || journal.canonical_name != artifact.canonical_name(database_name)
        || journal.quarantine_name != expected_quarantine
        || journal.database_identity != stable_file_identity(&database_metadata)?
        || journal.expected_sha256.len() != 64
        || !journal
            .expected_sha256
            .bytes()
            .all(|byte| byte.is_ascii_hexdigit() && !byte.is_ascii_uppercase())
        || instance_id == Some(journal.cleanup_operation_id.as_str())
        || journal_path.file_name().and_then(|name| name.to_str()) != Some(expected_name.as_str())
    {
        return Err(storage_recovery_blocked("sidecar_unknown_owner", artifact));
    }
    Ok(())
}

fn publish_sidecar_cleanup_state(
    parent: &Path,
    journal_path: &Path,
    journal: &SidecarCleanupJournal,
    crash_at: Option<BackupCrashPoint>,
) -> Result<(), ImportError> {
    let staging_path = PathBuf::from(format!("{}.staging", journal_path.display()));
    if staging_path.exists() {
        return Err(storage_recovery_blocked(
            "sidecar_cleanup_failed",
            journal.artifact,
        ));
    }
    let bytes = serde_json::to_vec(journal)
        .map_err(|_| storage_recovery_blocked("sidecar_cleanup_failed", journal.artifact))?;
    write_staging_file(&staging_path, &bytes)
        .map_err(|_| storage_recovery_blocked("sidecar_cleanup_failed", journal.artifact))?;
    let (staging_boundary, publish_boundary, parent_boundary) = match journal.state {
        SidecarCleanupState::Quarantined => (
            "sidecar_quarantined_staging_fsync",
            "sidecar_quarantined_publish",
            "sidecar_quarantined_parent_fsync",
        ),
        SidecarCleanupState::Done => (
            "sidecar_done_staging_fsync",
            "sidecar_done_publish",
            "sidecar_done_parent_fsync",
        ),
        SidecarCleanupState::Prepared => {
            return Err(storage_recovery_blocked(
                "sidecar_cleanup_failed",
                journal.artifact,
            ));
        }
    };
    maybe_inject_restore_crash(crash_at, staging_boundary)?;
    fs::rename(&staging_path, journal_path)
        .map_err(|_| storage_recovery_blocked("sidecar_cleanup_failed", journal.artifact))?;
    maybe_inject_restore_crash(crash_at, publish_boundary)?;
    sync_directory(parent)
        .map_err(|_| storage_recovery_blocked("sidecar_cleanup_failed", journal.artifact))?;
    maybe_inject_restore_crash(crash_at, parent_boundary)
}

fn publish_initial_sidecar_cleanup(
    parent: &Path,
    journal_path: &Path,
    journal: &SidecarCleanupJournal,
    crash_at: Option<BackupCrashPoint>,
) -> Result<(), ImportError> {
    let staging_path = PathBuf::from(format!("{}.staging", journal_path.display()));
    if journal_path.exists() || staging_path.exists() {
        return Err(storage_recovery_blocked(
            "sidecar_unknown_owner",
            journal.artifact,
        ));
    }
    let bytes = serde_json::to_vec(journal)
        .map_err(|_| storage_recovery_blocked("sidecar_cleanup_failed", journal.artifact))?;
    write_staging_file(&staging_path, &bytes)
        .map_err(|_| storage_recovery_blocked("sidecar_cleanup_failed", journal.artifact))?;
    maybe_inject_restore_crash(crash_at, "sidecar_prepared_staging_fsync")?;
    publish_no_replace(&staging_path, journal_path)
        .map_err(|_| storage_recovery_blocked("sidecar_cleanup_failed", journal.artifact))?;
    maybe_inject_restore_crash(crash_at, "sidecar_prepared_publish")?;
    sync_directory(parent)
        .map_err(|_| storage_recovery_blocked("sidecar_cleanup_failed", journal.artifact))?;
    maybe_inject_restore_crash(crash_at, "sidecar_prepared_parent_fsync")
}

fn replay_one_sidecar_cleanup(
    database: &Path,
    db_id: &str,
    artifact: CleanupArtifact,
    journal_path: &Path,
    crash_at: Option<BackupCrashPoint>,
) -> Result<(), ImportError> {
    let parent = database
        .parent()
        .ok_or_else(|| storage_recovery_blocked("sidecar_unknown_owner", artifact))?;
    let bytes = read_control_file_nofollow(journal_path)
        .map_err(|_| storage_recovery_blocked("sidecar_unknown_owner", artifact))?;
    let mut journal: SidecarCleanupJournal = serde_json::from_slice(&bytes)
        .map_err(|_| storage_recovery_blocked("sidecar_unknown_owner", artifact))?;
    validate_sidecar_cleanup_journal(database, db_id, artifact, journal_path, &journal)?;
    let canonical = parent.join(&journal.canonical_name);
    let quarantine = parent.join(&journal.quarantine_name);

    loop {
        let canonical_evidence = inspect_owned_regular_file(&canonical)
            .map_err(|_| storage_recovery_blocked("sidecar_unknown_owner", artifact))?;
        let quarantine_evidence = inspect_owned_regular_file(&quarantine)
            .map_err(|_| storage_recovery_blocked("sidecar_unknown_owner", artifact))?;
        match journal.state {
            SidecarCleanupState::Prepared => match (canonical_evidence, quarantine_evidence) {
                (Some(canonical_evidence), None) => {
                    if !evidence_matches(&canonical_evidence, &journal) {
                        return Err(storage_recovery_blocked("sidecar_reappeared", artifact));
                    }
                    publish_no_replace(&canonical, &quarantine).map_err(|_| {
                        storage_recovery_blocked("sidecar_cleanup_failed", artifact)
                    })?;
                    maybe_inject_restore_crash(crash_at, "sidecar_quarantine_rename")?;
                    sync_directory(parent).map_err(|_| {
                        storage_recovery_blocked("sidecar_cleanup_failed", artifact)
                    })?;
                    maybe_inject_restore_crash(crash_at, "sidecar_quarantine_parent_fsync")?;
                    let moved = inspect_owned_regular_file(&quarantine)
                        .map_err(|_| storage_recovery_blocked("sidecar_unknown_owner", artifact))?
                        .ok_or_else(|| {
                            storage_recovery_blocked("sidecar_unknown_owner", artifact)
                        })?;
                    if !evidence_matches(&moved, &journal) {
                        return Err(storage_recovery_blocked("sidecar_unknown_owner", artifact));
                    }
                    maybe_inject_restore_crash(crash_at, "sidecar_quarantine_identity_verify")?;
                    journal.state = SidecarCleanupState::Quarantined;
                    publish_sidecar_cleanup_state(parent, journal_path, &journal, crash_at)?;
                }
                (None, Some(quarantine_evidence)) => {
                    if !evidence_matches(&quarantine_evidence, &journal) {
                        return Err(storage_recovery_blocked("sidecar_unknown_owner", artifact));
                    }
                    sync_directory(parent).map_err(|_| {
                        storage_recovery_blocked("sidecar_cleanup_failed", artifact)
                    })?;
                    journal.state = SidecarCleanupState::Quarantined;
                    publish_sidecar_cleanup_state(parent, journal_path, &journal, crash_at)?;
                }
                (Some(canonical_evidence), Some(_)) => {
                    if !evidence_matches(&canonical_evidence, &journal) {
                        return Err(storage_recovery_blocked("sidecar_reappeared", artifact));
                    }
                    return Err(storage_recovery_blocked("sidecar_unknown_owner", artifact));
                }
                (None, None) => {
                    return Err(storage_recovery_blocked("sidecar_unknown_owner", artifact));
                }
            },
            SidecarCleanupState::Quarantined => {
                if let Some(canonical_evidence) = canonical_evidence {
                    if !evidence_matches(&canonical_evidence, &journal) {
                        return Err(storage_recovery_blocked("sidecar_reappeared", artifact));
                    }
                    return Err(storage_recovery_blocked("sidecar_unknown_owner", artifact));
                }
                if let Some(quarantine_evidence) = quarantine_evidence {
                    if !evidence_matches(&quarantine_evidence, &journal) {
                        return Err(storage_recovery_blocked("sidecar_unknown_owner", artifact));
                    }
                    fs::remove_file(&quarantine).map_err(|_| {
                        storage_recovery_blocked("sidecar_cleanup_failed", artifact)
                    })?;
                    maybe_inject_restore_crash(crash_at, "sidecar_quarantine_unlink")?;
                }
                sync_directory(parent)
                    .map_err(|_| storage_recovery_blocked("sidecar_cleanup_failed", artifact))?;
                maybe_inject_restore_crash(crash_at, "sidecar_quarantine_unlink_parent_fsync")?;
                journal.state = SidecarCleanupState::Done;
                publish_sidecar_cleanup_state(parent, journal_path, &journal, crash_at)?;
            }
            SidecarCleanupState::Done => {
                if let Some(canonical_evidence) = canonical_evidence {
                    if !evidence_matches(&canonical_evidence, &journal) {
                        return Err(storage_recovery_blocked("sidecar_reappeared", artifact));
                    }
                    return Err(storage_recovery_blocked("sidecar_unknown_owner", artifact));
                }
                if quarantine_evidence.is_some() {
                    return Err(storage_recovery_blocked("sidecar_unknown_owner", artifact));
                }
                fs::remove_file(journal_path)
                    .map_err(|_| storage_recovery_blocked("sidecar_cleanup_failed", artifact))?;
                maybe_inject_restore_crash(crash_at, "sidecar_journal_unlink")?;
                sync_directory(parent)
                    .map_err(|_| storage_recovery_blocked("sidecar_cleanup_failed", artifact))?;
                maybe_inject_restore_crash(crash_at, "sidecar_journal_unlink_parent_fsync")?;
                return Ok(());
            }
        }
    }
}

fn replay_sidecar_cleanup_for_database(database: &Path, db_id: &str) -> Result<(), ImportError> {
    reconcile_sidecar_cleanup_for_database(database, db_id, false, None)
}

fn converge_closed_database_sidecars(
    database: &Path,
    db_id: &str,
    crash_at: Option<BackupCrashPoint>,
) -> Result<(), ImportError> {
    reconcile_sidecar_cleanup_for_database(database, db_id, true, crash_at)
}

fn reconcile_sidecar_cleanup_for_database(
    database: &Path,
    db_id: &str,
    closed_checkpoint_proven: bool,
    crash_at: Option<BackupCrashPoint>,
) -> Result<(), ImportError> {
    if !database.exists() {
        return Ok(());
    }
    let parent = database
        .parent()
        .ok_or_else(|| storage_recovery_blocked("sidecar_unknown_owner", CleanupArtifact::Wal))?;
    let database_name = database
        .file_name()
        .and_then(|name| name.to_str())
        .ok_or_else(|| storage_recovery_blocked("sidecar_unknown_owner", CleanupArtifact::Wal))?;
    let mut candidates = Vec::new();
    let mut final_journals = Vec::new();
    let mut orphan_staging = Vec::new();
    let mut known_control_names = BTreeSet::new();
    let mut known_quarantine_names = BTreeSet::new();

    for artifact in CleanupArtifact::ALL {
        let journal_path = parent.join(cleanup_journal_name(db_id, artifact));
        let staging_path = PathBuf::from(format!("{}.staging", journal_path.display()));
        known_control_names.insert(
            journal_path
                .file_name()
                .and_then(|name| name.to_str())
                .unwrap_or_default()
                .to_string(),
        );
        known_control_names.insert(
            staging_path
                .file_name()
                .and_then(|name| name.to_str())
                .unwrap_or_default()
                .to_string(),
        );
        if journal_path.exists() && staging_path.exists() {
            candidates.push(RecoveryBlock {
                reason: "sidecar_unknown_owner",
                artifact,
            });
        }
        let record_path = if journal_path.exists() {
            Some(&journal_path)
        } else if staging_path.exists() {
            Some(&staging_path)
        } else {
            None
        };
        let Some(record_path) = record_path else {
            continue;
        };
        let parsed = read_control_file_nofollow(record_path)
            .ok()
            .and_then(|bytes| serde_json::from_slice::<SidecarCleanupJournal>(&bytes).ok());
        let Some(journal) = parsed else {
            candidates.push(RecoveryBlock {
                reason: "sidecar_unknown_owner",
                artifact,
            });
            continue;
        };
        if validate_sidecar_cleanup_journal(database, db_id, artifact, &journal_path, &journal)
            .is_err()
        {
            candidates.push(RecoveryBlock {
                reason: "sidecar_unknown_owner",
                artifact,
            });
            continue;
        }
        known_quarantine_names.insert(journal.quarantine_name.clone());
        let canonical = parent.join(&journal.canonical_name);
        let quarantine = parent.join(&journal.quarantine_name);
        let canonical_evidence = match inspect_owned_regular_file(&canonical) {
            Ok(evidence) => evidence,
            Err(_) => {
                candidates.push(RecoveryBlock {
                    reason: "sidecar_unknown_owner",
                    artifact,
                });
                continue;
            }
        };
        let quarantine_evidence = match inspect_owned_regular_file(&quarantine) {
            Ok(evidence) => evidence,
            Err(_) => {
                candidates.push(RecoveryBlock {
                    reason: "sidecar_unknown_owner",
                    artifact,
                });
                continue;
            }
        };
        if let Some(block) = classify_journal_paths(
            &journal,
            canonical_evidence.as_ref(),
            quarantine_evidence.as_ref(),
        ) {
            candidates.push(block);
        }
        if journal_path.exists() {
            final_journals.push((artifact, journal_path));
        } else {
            orphan_staging.push((artifact, staging_path, journal));
        }
    }

    let entries = fs::read_dir(parent)
        .map_err(|_| storage_recovery_blocked("sidecar_unknown_owner", CleanupArtifact::Wal))?;
    for entry in entries {
        let entry = entry
            .map_err(|_| storage_recovery_blocked("sidecar_unknown_owner", CleanupArtifact::Wal))?;
        let Some(name) = entry.file_name().to_str().map(ToOwned::to_owned) else {
            continue;
        };
        let is_cleanup = name.starts_with(".hivegui-sidecar-cleanup-v1-");
        let is_quarantine = name.starts_with(".hivegui-sidecar-quarantine-v1-");
        if is_cleanup && !known_control_names.contains(&name)
            || is_quarantine && !known_quarantine_names.contains(&name)
        {
            candidates.push(RecoveryBlock {
                reason: "sidecar_unknown_owner",
                artifact: artifact_from_control_name(&name),
            });
        }
    }

    if let Some(error) = select_recovery_block(&candidates) {
        return Err(error);
    }

    for (artifact, staging_path, journal) in orphan_staging {
        let canonical = parent.join(&journal.canonical_name);
        let quarantine = parent.join(&journal.quarantine_name);
        let canonical_evidence = inspect_owned_regular_file(&canonical)
            .map_err(|_| storage_recovery_blocked("sidecar_unknown_owner", artifact))?;
        if canonical_evidence
            .as_ref()
            .is_none_or(|evidence| !evidence_matches(evidence, &journal))
            || quarantine.exists()
        {
            return Err(storage_recovery_blocked("sidecar_unknown_owner", artifact));
        }
        fs::remove_file(&staging_path)
            .map_err(|_| storage_recovery_blocked("sidecar_cleanup_failed", artifact))?;
        sync_directory(parent)
            .map_err(|_| storage_recovery_blocked("sidecar_cleanup_failed", artifact))?;
    }

    for (artifact, journal_path) in final_journals {
        replay_one_sidecar_cleanup(database, db_id, artifact, &journal_path, crash_at)?;
    }

    let mut remaining = Vec::new();
    for artifact in CleanupArtifact::ALL {
        let canonical = parent.join(artifact.canonical_name(database_name));
        if canonical.exists() {
            remaining.push(RecoveryBlock {
                reason: match artifact {
                    CleanupArtifact::Wal => "sidecar_hot",
                    CleanupArtifact::RollbackJournal => "sidecar_recoverable",
                    CleanupArtifact::Shm => "sidecar_unknown_owner",
                },
                artifact,
            });
        }
    }
    if closed_checkpoint_proven && !remaining.is_empty() {
        let database_metadata = fs::symlink_metadata(database)
            .map_err(|_| storage_recovery_blocked("sidecar_unknown_owner", CleanupArtifact::Wal))?;
        if !database_metadata.is_file() || database_metadata.file_type().is_symlink() {
            return Err(storage_recovery_blocked(
                "sidecar_unknown_owner",
                CleanupArtifact::Wal,
            ));
        }
        #[cfg(unix)]
        {
            use std::os::unix::fs::MetadataExt as _;

            if database_metadata.nlink() != 1 {
                return Err(storage_recovery_blocked(
                    "sidecar_unknown_owner",
                    CleanupArtifact::Wal,
                ));
            }
        }
        let database_identity = stable_file_identity(&database_metadata)?;
        for candidate in &remaining {
            let artifact = candidate.artifact;
            let canonical_name = artifact.canonical_name(database_name);
            let canonical = parent.join(&canonical_name);
            let evidence = inspect_owned_regular_file(&canonical)
                .map_err(|_| storage_recovery_blocked("sidecar_unknown_owner", artifact))?
                .ok_or_else(|| storage_recovery_blocked("sidecar_unknown_owner", artifact))?;
            let cleanup_operation_id = Uuid::new_v4().to_string();
            let quarantine_name = format!(
                ".hivegui-sidecar-quarantine-v1-{cleanup_operation_id}-{}",
                artifact.as_str()
            );
            let journal_path = parent.join(cleanup_journal_name(db_id, artifact));
            let journal = SidecarCleanupJournal {
                schema_version: 1,
                cleanup_operation_id,
                db_id: db_id.into(),
                database_identity: database_identity.clone(),
                artifact,
                canonical_name,
                expected_identity: evidence.identity,
                expected_size_bytes: evidence.size_bytes,
                expected_sha256: evidence.sha256,
                quarantine_name,
                state: SidecarCleanupState::Prepared,
            };
            publish_initial_sidecar_cleanup(parent, &journal_path, &journal, crash_at)?;
            replay_one_sidecar_cleanup(database, db_id, artifact, &journal_path, crash_at)?;
        }
        remaining.clear();
    }
    if let Some(error) = select_recovery_block(&remaining) {
        return Err(error);
    }
    Ok(())
}

fn sync_directory(path: &Path) -> Result<(), ImportError> {
    File::open(path)
        .and_then(|directory| directory.sync_all())
        .map_err(|error| ImportError::Io(format!("fsync {}: {error}", path.display())))
}

struct NoFollowLeaf {
    file: File,
    identity: String,
}

#[cfg(windows)]
#[derive(Clone, Debug, Eq, PartialEq)]
struct WindowsFileIdentity {
    volume_serial_number: u64,
    file_id: [u8; 16],
}

#[cfg(windows)]
const FILE_ID_INFO: u32 = 18;
#[cfg(windows)]
const FILE_FLAG_OPEN_REPARSE_POINT: u32 = 0x0020_0000;
#[cfg(windows)]
const FSCTL_GET_REPARSE_POINT: u32 = 0x0009_00a8;
#[cfg(windows)]
const FILE_ATTRIBUTE_REPARSE_POINT: u32 = 0x0000_0400;

#[cfg(windows)]
#[repr(C)]
struct RawFileIdInfo {
    volume_serial_number: u64,
    file_id: [u8; 16],
}

#[cfg(windows)]
unsafe extern "system" {
    fn GetFileInformationByHandleEx(
        file: *mut std::ffi::c_void,
        information_class: u32,
        information: *mut std::ffi::c_void,
        information_size: u32,
    ) -> i32;
}

#[cfg(windows)]
fn windows_file_identity(file: &File) -> Result<WindowsFileIdentity, ImportError> {
    use std::os::windows::{fs::MetadataExt as _, io::AsRawHandle as _};

    let metadata = file
        .metadata()
        .map_err(|error| ImportError::Io(error.to_string()))?;
    if metadata.file_attributes() & FILE_ATTRIBUTE_REPARSE_POINT != 0 {
        let _reparse_query = FSCTL_GET_REPARSE_POINT;
        return Err(ImportError::UnsafeArchiveEntry(
            "Windows reparse-point leaf".into(),
        ));
    }
    let mut raw = RawFileIdInfo {
        volume_serial_number: 0,
        file_id: [0; 16],
    };
    // SAFETY: `raw` is a correctly sized writable FILE_ID_INFO buffer and the
    // borrowed handle remains open for the duration of the call.
    let succeeded = unsafe {
        GetFileInformationByHandleEx(
            file.as_raw_handle(),
            FILE_ID_INFO,
            (&mut raw as *mut RawFileIdInfo).cast(),
            std::mem::size_of::<RawFileIdInfo>() as u32,
        )
    };
    if succeeded == 0 {
        return Err(ImportError::Io(std::io::Error::last_os_error().to_string()));
    }
    Ok(WindowsFileIdentity {
        volume_serial_number: raw.volume_serial_number,
        file_id: raw.file_id,
    })
}

fn open_no_follow_leaf_at(
    root: &cap_std::fs::Dir,
    relative: &Path,
) -> Result<NoFollowLeaf, ImportError> {
    validate_relative_path(relative)
        .map_err(|_| ImportError::UnsafeArchiveEntry(relative.display().to_string()))?;
    let before = root
        .symlink_metadata(relative)
        .map_err(|error| ImportError::Io(error.to_string()))?;
    if !before.is_file() || before.file_type().is_symlink() {
        return Err(ImportError::UnsafeArchiveEntry(
            relative.display().to_string(),
        ));
    }

    let mut options = cap_std::fs::OpenOptions::new();
    options.read(true);
    #[cfg(unix)]
    {
        use cap_std::fs::OpenOptionsExt as _;
        options.custom_flags(libc::O_NOFOLLOW | libc::O_CLOEXEC | libc::O_NONBLOCK);
    }
    #[cfg(windows)]
    {
        use cap_std::fs::OpenOptionsExt as _;
        options.custom_flags(FILE_FLAG_OPEN_REPARSE_POINT);
    }
    #[cfg(not(unix))]
    let _descriptor_relative_non_unix = true;
    #[cfg(all(not(unix), not(windows)))]
    {
        // These platforms still use the same descriptor-relative open and the
        // before/after entry identity check below; they never canonicalize or
        // reopen an ambient path.
    }
    let file = root
        .open_with(relative, &options)
        .map_err(|error| ImportError::Io(error.to_string()))?
        .into_std();
    let after = root
        .symlink_metadata(relative)
        .map_err(|error| ImportError::Io(error.to_string()))?;
    if !after.is_file()
        || after.file_type().is_symlink()
        || stable_cap_file_identity(&before)? != stable_cap_file_identity(&after)?
    {
        return Err(ImportError::UnsafeArchiveEntry(
            relative.display().to_string(),
        ));
    }
    #[cfg(windows)]
    let identity = {
        let identity = windows_file_identity(&file)?;
        format!(
            "windows:{}:{}",
            identity.volume_serial_number,
            hex::encode(identity.file_id)
        )
    };
    #[cfg(not(windows))]
    let identity = stable_file_identity(
        &file
            .metadata()
            .map_err(|error| ImportError::Io(error.to_string()))?,
    )?;
    Ok(NoFollowLeaf { file, identity })
}

struct BoundSqliteLeafVfs {
    pool: sqlx::SqlitePool,
}

impl BoundSqliteLeafVfs {
    fn vfs(&self) -> &sqlx::SqlitePool {
        &self.pool
    }
}

struct BoundSqliteLeaf {
    held_leaf: NoFollowLeaf,
    parent_directory: cap_std::fs::Dir,
    database_name: PathBuf,
    database_path: PathBuf,
    vfs: BoundSqliteLeafVfs,
}

async fn bind_sqlite_leaf(database: &Path) -> Result<BoundSqliteLeaf, ImportError> {
    let parent = database.parent().unwrap_or_else(|| Path::new("."));
    let database_name = PathBuf::from(
        database
            .file_name()
            .ok_or_else(|| ImportError::UnsafeArchiveEntry(database.display().to_string()))?,
    );
    let parent_directory = cap_std::fs::Dir::open_ambient_dir(parent, cap_std::ambient_authority())
        .map_err(|error| ImportError::Io(error.to_string()))?;
    let held_leaf = open_no_follow_leaf_at(&parent_directory, &database_name)?;
    let options = sqlx::sqlite::SqliteConnectOptions::new()
        .filename(database)
        .create_if_missing(false)
        .foreign_keys(true);
    let pool = sqlx::sqlite::SqlitePoolOptions::new()
        .min_connections(1)
        .max_connections(1)
        .connect_with(options)
        .await
        .map_err(|_| ImportError::StorageRecoveryBlocked {
            reason: "checkpoint_failed",
            artifact: "checkpoint",
        })?;
    let canonical = open_no_follow_leaf_at(&parent_directory, &database_name)?;
    if canonical.identity != held_leaf.identity {
        pool.close().await;
        return Err(ImportError::StorageRecoveryBlocked {
            reason: "checkpoint_identity_changed",
            artifact: "checkpoint",
        });
    }
    Ok(BoundSqliteLeaf {
        held_leaf,
        parent_directory,
        database_name,
        database_path: database.to_path_buf(),
        vfs: BoundSqliteLeafVfs { pool },
    })
}

async fn checkpoint_bound_sqlite_leaf(
    leaf: &BoundSqliteLeaf,
    db_id: &str,
    crash_at: Option<BackupCrashPoint>,
    interlocks: Option<&[Arc<tokio::sync::Barrier>]>,
) -> Result<(), ImportError> {
    if let Some(interlocks) = interlocks {
        interlocks[0].wait().await;
        interlocks[1].wait().await;
    }
    let pool = leaf.vfs.vfs();
    let checkpoint: (i64, i64, i64) = sqlx::query_as("PRAGMA wal_checkpoint(TRUNCATE)")
        .fetch_one(pool)
        .await
        .map_err(|_| ImportError::StorageRecoveryBlocked {
            reason: "checkpoint_failed",
            artifact: "checkpoint",
        })?;
    if checkpoint.0 != 0 || checkpoint.1 != checkpoint.2 {
        return Err(ImportError::StorageRecoveryBlocked {
            reason: "checkpoint_busy",
            artifact: "checkpoint",
        });
    }
    super::migrations::verify_sqlite_health(pool)
        .await
        .map_err(|_| ImportError::InvalidManifest("SQLite health failed".into()))?;
    leaf.vfs.pool.close().await;
    if let Some(interlocks) = interlocks {
        interlocks[2].wait().await;
        interlocks[3].wait().await;
    }
    let canonical = open_no_follow_leaf_at(&leaf.parent_directory, &leaf.database_name)?;
    if canonical.identity != leaf.held_leaf.identity {
        return Err(ImportError::StorageRecoveryBlocked {
            reason: "checkpoint_identity_changed",
            artifact: "checkpoint",
        });
    }
    converge_closed_database_sidecars(&leaf.database_path, db_id, crash_at)?;
    leaf.held_leaf
        .file
        .sync_all()
        .map_err(|error| ImportError::Io(error.to_string()))?;
    sync_directory(
        leaf.database_path
            .parent()
            .unwrap_or_else(|| Path::new(".")),
    )
}

async fn checkpoint_closed_database(
    database: &Path,
    db_id: &str,
    crash_at: Option<BackupCrashPoint>,
    interlocks: Option<&[Arc<tokio::sync::Barrier>]>,
) -> Result<(), ImportError> {
    let leaf = bind_sqlite_leaf(database).await?;
    checkpoint_bound_sqlite_leaf(&leaf, db_id, crash_at, interlocks).await
}

async fn validate_restore_database(database: &Path, plugin_root: &Path) -> Result<(), ImportError> {
    let options = sqlx::sqlite::SqliteConnectOptions::new()
        .filename(database)
        .create_if_missing(false)
        .foreign_keys(true);
    let pool = sqlx::sqlite::SqlitePoolOptions::new()
        .min_connections(1)
        .max_connections(1)
        .connect_with(options)
        .await
        .map_err(|_| ImportError::InvalidManifest("open restore database".into()))?;
    super::migrations::verify_sqlite_health(&pool)
        .await
        .map_err(|_| ImportError::InvalidManifest("restore SQLite health".into()))?;
    super::migrations::verify_schema(&pool)
        .await
        .map_err(|_| ImportError::InvalidManifest("restore schema drift".into()))?;
    let mut transaction = pool
        .begin()
        .await
        .map_err(|_| ImportError::Io("begin restore artifact validation".into()))?;
    validate_restored_artifacts(&mut transaction, plugin_root).await?;
    transaction
        .rollback()
        .await
        .map_err(|_| ImportError::Io("close restore validation transaction".into()))?;
    pool.close().await;
    Ok(())
}

fn write_restore_control_staging(
    staging_path: &Path,
    bytes: &[u8],
    crash_at: Option<BackupCrashPoint>,
    write_boundary: &'static str,
    fsync_boundary: &'static str,
) -> Result<(), ImportError> {
    let mut file = OpenOptions::new()
        .create_new(true)
        .write(true)
        .open(staging_path)
        .map_err(|error| ImportError::Io(error.to_string()))?;
    file.write_all(bytes)
        .map_err(|error| ImportError::Io(error.to_string()))?;
    maybe_inject_restore_crash(crash_at, write_boundary)?;
    file.sync_all()
        .map_err(|error| ImportError::Io(error.to_string()))?;
    maybe_inject_restore_crash(crash_at, fsync_boundary)
}

fn publish_instance_manifest(
    live: &Path,
    manifest: &RestoreInstanceManifest,
    crash_at: Option<BackupCrashPoint>,
) -> Result<(), ImportError> {
    let final_path = live.join(".hivegui-db-instance-v1.json");
    let staging_path = live.join(".hivegui-db-instance-v1.json.staging");
    let bytes = serde_json::to_vec(manifest)
        .map_err(|error| ImportError::InvalidManifest(error.to_string()))?;
    write_restore_control_staging(
        &staging_path,
        &bytes,
        crash_at,
        "manifest_arm_staging_write",
        "manifest_arm_staging_fsync",
    )?;
    fs::rename(&staging_path, &final_path).map_err(|error| ImportError::Io(error.to_string()))?;
    maybe_inject_restore_crash(crash_at, "manifest_arm_rename")?;
    sync_directory(live)?;
    maybe_inject_restore_crash(crash_at, "manifest_arm_parent_fsync")
}

fn read_reconciled_instance_manifest(live: &Path) -> Result<RestoreInstanceManifest, ImportError> {
    let final_path = live.join(".hivegui-db-instance-v1.json");
    let staging_path = live.join(".hivegui-db-instance-v1.json.staging");
    let final_record: RestoreInstanceManifest =
        serde_json::from_slice(&read_control_file_nofollow(&final_path)?).map_err(|_| {
            ImportError::InvalidManifest("invalid restore instance manifest".into())
        })?;
    if staging_path.exists() {
        let staging_record: RestoreInstanceManifest =
            serde_json::from_slice(&read_control_file_nofollow(&staging_path)?).map_err(|_| {
                ImportError::InvalidManifest("invalid restore manifest staging".into())
            })?;
        let mut expected = final_record.clone();
        expected.ownership_state = "armed".into();
        if final_record.ownership_state != "unarmed" || staging_record != expected {
            return Err(ImportError::InvalidManifest(
                "ambiguous restore manifest update".into(),
            ));
        }
        fs::remove_file(&staging_path).map_err(|error| ImportError::Io(error.to_string()))?;
    }
    sync_directory(live)?;
    Ok(final_record)
}

fn publish_restore_owner(
    live: &Path,
    owner: &RestoreOwner,
    initial: bool,
    crash_at: Option<BackupCrashPoint>,
) -> Result<(), ImportError> {
    let final_path = live.join(".hivegui-db-recovery-v1.json");
    let staging_path = live.join(".hivegui-db-recovery-v1.json.staging");
    let bytes = serde_json::to_vec(owner)
        .map_err(|error| ImportError::InvalidManifest(error.to_string()))?;
    let boundaries = match owner.phase.as_str() {
        "prepared" => [
            "owner_prepared_staging_write",
            "owner_prepared_staging_fsync",
            "owner_prepared_rename",
            "owner_prepared_parent_fsync",
        ],
        "applying" => [
            "owner_applying_staging_write",
            "owner_applying_staging_fsync",
            "owner_applying_rename",
            "owner_applying_parent_fsync",
        ],
        "committed" => [
            "owner_committed_staging_write",
            "owner_committed_staging_fsync",
            "owner_committed_rename",
            "owner_committed_parent_fsync",
        ],
        _ => {
            return Err(ImportError::InvalidManifest(
                "invalid restore owner publish phase".into(),
            ));
        }
    };
    write_restore_control_staging(
        &staging_path,
        &bytes,
        crash_at,
        boundaries[0],
        boundaries[1],
    )?;
    if initial {
        publish_no_replace(&staging_path, &final_path)
            .map_err(|error| ImportError::Io(error.to_string()))?;
    } else {
        fs::rename(&staging_path, &final_path)
            .map_err(|error| ImportError::Io(error.to_string()))?;
    }
    maybe_inject_restore_crash(crash_at, boundaries[2])?;
    sync_directory(live)?;
    maybe_inject_restore_crash(crash_at, boundaries[3])
}

fn read_reconciled_restore_owner(live: &Path) -> Result<RestoreOwner, ImportError> {
    let final_path = live.join(".hivegui-db-recovery-v1.json");
    let staging_path = live.join(".hivegui-db-recovery-v1.json.staging");
    let final_record: RestoreOwner =
        serde_json::from_slice(&read_control_file_nofollow(&final_path)?)
            .map_err(|_| ImportError::InvalidManifest("invalid armed restore owner".into()))?;
    if staging_path.exists() {
        let staging_record: RestoreOwner =
            serde_json::from_slice(&read_control_file_nofollow(&staging_path)?).map_err(|_| {
                ImportError::InvalidManifest("invalid restore owner staging".into())
            })?;
        let next_phase = match final_record.phase.as_str() {
            "prepared" => "applying",
            "applying" => "committed",
            _ => {
                return Err(ImportError::InvalidManifest(
                    "ambiguous restore owner update".into(),
                ));
            }
        };
        let mut expected = final_record.clone();
        expected.phase = next_phase.into();
        if staging_record != expected {
            return Err(ImportError::InvalidManifest(
                "ambiguous restore owner update".into(),
            ));
        }
        fs::remove_file(&staging_path).map_err(|error| ImportError::Io(error.to_string()))?;
    }
    sync_directory(live)?;
    Ok(final_record)
}

fn write_staging_file(path: &Path, bytes: &[u8]) -> Result<(), ImportError> {
    let mut file = OpenOptions::new()
        .create_new(true)
        .write(true)
        .open(path)
        .map_err(|error| ImportError::Io(error.to_string()))?;
    file.write_all(bytes)
        .and_then(|_| file.sync_all())
        .map_err(|error| ImportError::Io(error.to_string()))
}

fn sha256_file(path: &Path) -> Result<String, ImportError> {
    let mut file = OpenOptions::new()
        .read(true)
        .open(path)
        .map_err(|error| ImportError::Io(error.to_string()))?;
    let mut hasher = Sha256::new();
    let mut buffer = [0u8; 1024 * 1024];
    loop {
        let read = file
            .read(&mut buffer)
            .map_err(|error| ImportError::Io(error.to_string()))?;
        if read == 0 {
            break;
        }
        hasher.update(&buffer[..read]);
    }
    Ok(hex::encode(hasher.finalize()))
}

fn controlled_relative_path(path: &Path) -> Result<String, ImportError> {
    validate_relative_path(path)
        .map_err(|_| ImportError::InvalidManifest("invalid controlled relative path".into()))?;
    path.to_str()
        .map(ToOwned::to_owned)
        .ok_or_else(|| ImportError::InvalidManifest("non-UTF-8 controlled path".into()))
}

fn controlled_file_evidence(
    path: &Path,
    relative_path: &Path,
) -> Result<ControlledFileEvidence, ImportError> {
    let evidence = inspect_owned_regular_file(path)?
        .ok_or_else(|| ImportError::InvalidManifest("controlled file is missing".into()))?;
    Ok(ControlledFileEvidence {
        relative_path: controlled_relative_path(relative_path)?,
        identity: evidence.identity,
        size_bytes: evidence.size_bytes,
        sha256: evidence.sha256,
    })
}

fn hash_owned_tree_entries(
    root: &Path,
    directory: &Path,
    digest: &mut Sha256,
) -> Result<(), ImportError> {
    let mut entries = fs::read_dir(directory)
        .map_err(|error| ImportError::Io(error.to_string()))?
        .collect::<Result<Vec<_>, _>>()
        .map_err(|error| ImportError::Io(error.to_string()))?;
    entries.sort_by_key(|entry| entry.file_name());
    for entry in entries {
        let path = entry.path();
        let relative = path
            .strip_prefix(root)
            .map_err(|_| ImportError::InvalidManifest("tree entry escaped root".into()))?;
        let encoded = relative.as_os_str().as_encoded_bytes();
        let metadata =
            fs::symlink_metadata(&path).map_err(|error| ImportError::Io(error.to_string()))?;
        if metadata.file_type().is_symlink() {
            return Err(ImportError::UnsafeArchiveEntry(path.display().to_string()));
        }
        if metadata.is_dir() {
            digest.update(b"directory\0");
            digest.update((encoded.len() as u64).to_be_bytes());
            digest.update(encoded);
            hash_owned_tree_entries(root, &path, digest)?;
        } else if metadata.is_file() {
            let file = inspect_owned_regular_file(&path)?
                .ok_or_else(|| ImportError::InvalidManifest("tree file disappeared".into()))?;
            digest.update(b"file\0");
            digest.update((encoded.len() as u64).to_be_bytes());
            digest.update(encoded);
            digest.update(file.size_bytes.to_be_bytes());
            digest.update(file.sha256.as_bytes());
        } else {
            return Err(ImportError::UnsafeArchiveEntry(path.display().to_string()));
        }
    }
    Ok(())
}

fn controlled_tree_evidence(
    root: &Path,
    relative_path: &Path,
) -> Result<ControlledTreeEvidence, ImportError> {
    let before = fs::symlink_metadata(root).map_err(|error| ImportError::Io(error.to_string()))?;
    if !before.is_dir() || before.file_type().is_symlink() {
        return Err(ImportError::UnsafeArchiveEntry(root.display().to_string()));
    }
    let identity = stable_file_identity(&before)?;
    let mut digest = Sha256::new();
    digest.update(b"hivegui-controlled-tree-v1\0");
    hash_owned_tree_entries(root, root, &mut digest)?;
    let after = fs::symlink_metadata(root).map_err(|error| ImportError::Io(error.to_string()))?;
    if !after.is_dir()
        || after.file_type().is_symlink()
        || stable_file_identity(&after)? != identity
    {
        return Err(ImportError::UnsafeArchiveEntry(root.display().to_string()));
    }
    Ok(ControlledTreeEvidence {
        relative_path: controlled_relative_path(relative_path)?,
        identity,
        sha256: hex::encode(digest.finalize()),
    })
}

fn optional_controlled_file_evidence(
    path: &Path,
    relative_path: &Path,
) -> Result<Option<ControlledFileEvidence>, ImportError> {
    if path.exists() {
        controlled_file_evidence(path, relative_path).map(Some)
    } else {
        Ok(None)
    }
}

fn optional_controlled_tree_evidence(
    path: &Path,
    relative_path: &Path,
) -> Result<Option<ControlledTreeEvidence>, ImportError> {
    if path.exists() {
        controlled_tree_evidence(path, relative_path).map(Some)
    } else {
        Ok(None)
    }
}

fn same_file_object(
    actual: Option<&ControlledFileEvidence>,
    expected: Option<&ControlledFileEvidence>,
) -> bool {
    match (actual, expected) {
        (Some(actual), Some(expected)) => {
            actual.identity == expected.identity
                && actual.size_bytes == expected.size_bytes
                && actual.sha256 == expected.sha256
        }
        (None, None) => true,
        _ => false,
    }
}

fn same_file_content(
    actual: Option<&ControlledFileEvidence>,
    expected: Option<&ControlledFileEvidence>,
) -> bool {
    match (actual, expected) {
        (Some(actual), Some(expected)) => {
            actual.size_bytes == expected.size_bytes && actual.sha256 == expected.sha256
        }
        (None, None) => true,
        _ => false,
    }
}

fn same_tree_object(
    actual: Option<&ControlledTreeEvidence>,
    expected: Option<&ControlledTreeEvidence>,
) -> bool {
    match (actual, expected) {
        (Some(actual), Some(expected)) => {
            actual.identity == expected.identity && actual.sha256 == expected.sha256
        }
        (None, None) => true,
        _ => false,
    }
}

fn same_tree_content(
    actual: Option<&ControlledTreeEvidence>,
    expected: Option<&ControlledTreeEvidence>,
) -> bool {
    match (actual, expected) {
        (Some(actual), Some(expected)) => actual.sha256 == expected.sha256,
        (None, None) => true,
        _ => false,
    }
}

fn valid_lower_sha256(value: &str) -> bool {
    value.len() == 64
        && value
            .bytes()
            .all(|byte| byte.is_ascii_hexdigit() && !byte.is_ascii_uppercase())
}

fn valid_file_evidence(evidence: Option<&ControlledFileEvidence>) -> bool {
    evidence.is_none_or(|evidence| {
        !evidence.identity.is_empty()
            && evidence.size_bytes > 0
            && valid_lower_sha256(&evidence.sha256)
            && controlled_relative_path(Path::new(&evidence.relative_path)).is_ok()
    })
}

fn valid_tree_evidence(evidence: Option<&ControlledTreeEvidence>) -> bool {
    evidence.is_none_or(|evidence| {
        !evidence.identity.is_empty()
            && valid_lower_sha256(&evidence.sha256)
            && controlled_relative_path(Path::new(&evidence.relative_path)).is_ok()
    })
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct SafetySnapshotArtifact {
    path: String,
    size_bytes: u64,
    sha256: String,
    source_identity: String,
    snapshot_identity: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct SafetySnapshotTree {
    path: String,
    sha256: String,
    source_identity: String,
    snapshot_identity: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct SafetySnapshotManifest {
    schema_version: u32,
    operation_id: String,
    database: Option<SafetySnapshotArtifact>,
    plugin_root: Option<SafetySnapshotTree>,
    plugin_files: Vec<SafetySnapshotArtifact>,
}

fn copy_owned_regular_file(
    source: &Path,
    target: &Path,
    manifest_path: &Path,
) -> Result<SafetySnapshotArtifact, ImportError> {
    let entry = fs::symlink_metadata(source).map_err(|error| ImportError::Io(error.to_string()))?;
    if !entry.is_file() || entry.file_type().is_symlink() {
        return Err(ImportError::UnsafeArchiveEntry(
            source.display().to_string(),
        ));
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::MetadataExt as _;

        if entry.nlink() != 1 {
            return Err(ImportError::UnsafeArchiveEntry(
                source.display().to_string(),
            ));
        }
    }
    let mut source_options = OpenOptions::new();
    source_options.read(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt as _;

        source_options.custom_flags(libc::O_NOFOLLOW | libc::O_CLOEXEC | libc::O_NONBLOCK);
    }
    let mut source_file = source_options
        .open(source)
        .map_err(|_| ImportError::UnsafeArchiveEntry(source.display().to_string()))?;
    let before = source_file
        .metadata()
        .map_err(|error| ImportError::Io(error.to_string()))?;
    if !before.is_file() || stable_file_identity(&entry)? != stable_file_identity(&before)? {
        return Err(ImportError::UnsafeArchiveEntry(
            source.display().to_string(),
        ));
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::MetadataExt as _;

        if before.nlink() != 1 {
            return Err(ImportError::UnsafeArchiveEntry(
                source.display().to_string(),
            ));
        }
    }
    let mut target_options = OpenOptions::new();
    target_options.write(true).create_new(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt as _;

        target_options.custom_flags(libc::O_NOFOLLOW | libc::O_CLOEXEC);
        target_options.mode(0o600);
    }
    let mut target_file = target_options
        .open(target)
        .map_err(|error| ImportError::Io(error.to_string()))?;
    let mut digest = Sha256::new();
    let mut size_bytes = 0_u64;
    let mut buffer = [0_u8; 1024 * 1024];
    loop {
        let read = source_file
            .read(&mut buffer)
            .map_err(|error| ImportError::Io(error.to_string()))?;
        if read == 0 {
            break;
        }
        target_file
            .write_all(&buffer[..read])
            .map_err(|error| ImportError::Io(error.to_string()))?;
        size_bytes = size_bytes
            .checked_add(read as u64)
            .ok_or_else(|| ImportError::Io("safety snapshot size overflow".into()))?;
        digest.update(&buffer[..read]);
    }
    target_file
        .sync_all()
        .map_err(|error| ImportError::Io(error.to_string()))?;
    let after = source_file
        .metadata()
        .map_err(|error| ImportError::Io(error.to_string()))?;
    if stable_file_identity(&before)? != stable_file_identity(&after)?
        || before.len() != after.len()
        || size_bytes != after.len()
    {
        return Err(ImportError::UnsafeArchiveEntry(
            source.display().to_string(),
        ));
    }
    let sha256 = hex::encode(digest.finalize());
    let snapshot = inspect_owned_regular_file(target)?
        .ok_or_else(|| ImportError::Io("safety snapshot target disappeared".into()))?;
    if snapshot.size_bytes != size_bytes || snapshot.sha256 != sha256 {
        return Err(ImportError::InvalidManifest(
            "safety snapshot copy mismatch".into(),
        ));
    }
    Ok(SafetySnapshotArtifact {
        path: manifest_path
            .to_str()
            .ok_or_else(|| ImportError::UnsafeArchiveEntry(manifest_path.display().to_string()))?
            .to_string(),
        size_bytes,
        sha256,
        source_identity: stable_file_identity(&after)?,
        snapshot_identity: snapshot.identity,
    })
}

async fn copy_owned_open_file(
    source_file: &mut File,
    expected_source_identity: &str,
    target_directory: &cap_std::fs::Dir,
    target_name: &Path,
    manifest_path: &Path,
    current_copy_interlock: Option<&[Arc<tokio::sync::Barrier>; 2]>,
) -> Result<SafetySnapshotArtifact, ImportError> {
    let before = source_file
        .metadata()
        .map_err(|error| ImportError::Io(error.to_string()))?;
    if !before.is_file()
        || stable_file_identity(&before)? != expected_source_identity
        || !owned_regular_file_link_count(&before)
    {
        return Err(current_database_identity_changed());
    }
    if let Some(current_copy_barriers) = current_copy_interlock {
        current_copy_barriers[0].wait().await;
        current_copy_barriers[1].wait().await;
    }
    source_file
        .seek(SeekFrom::Start(0))
        .map_err(|error| ImportError::Io(error.to_string()))?;

    let mut target_options = cap_std::fs::OpenOptions::new();
    target_options.write(true).create_new(true);
    #[cfg(unix)]
    {
        use cap_std::fs::OpenOptionsExt as _;

        target_options.custom_flags(libc::O_NOFOLLOW | libc::O_CLOEXEC);
        target_options.mode(0o600);
    }
    let mut target_file = target_directory
        .open_with(target_name, &target_options)
        .map(cap_std::fs::File::into_std)
        .map_err(|error| ImportError::Io(error.to_string()))?;
    let mut digest = Sha256::new();
    let mut size_bytes = 0_u64;
    let mut buffer = [0_u8; 1024 * 1024];
    loop {
        let read = source_file
            .read(&mut buffer)
            .map_err(|error| ImportError::Io(error.to_string()))?;
        if read == 0 {
            break;
        }
        target_file
            .write_all(&buffer[..read])
            .map_err(|error| ImportError::Io(error.to_string()))?;
        size_bytes = size_bytes
            .checked_add(read as u64)
            .ok_or_else(|| ImportError::Io("safety snapshot size overflow".into()))?;
        digest.update(&buffer[..read]);
    }
    target_file
        .sync_all()
        .map_err(|error| ImportError::Io(error.to_string()))?;
    let after = source_file
        .metadata()
        .map_err(|error| ImportError::Io(error.to_string()))?;
    if stable_file_identity(&before)? != stable_file_identity(&after)?
        || stable_file_identity(&after)? != expected_source_identity
        || before.len() != after.len()
        || size_bytes != after.len()
        || !owned_regular_file_link_count(&after)
    {
        return Err(current_database_identity_changed());
    }
    let sha256 = hex::encode(digest.finalize());
    let snapshot = target_file
        .metadata()
        .map_err(|error| ImportError::Io(error.to_string()))?;
    if !snapshot.is_file()
        || !owned_regular_file_link_count(&snapshot)
        || snapshot.len() != size_bytes
    {
        return Err(ImportError::InvalidManifest(
            "safety snapshot copy mismatch".into(),
        ));
    }
    Ok(SafetySnapshotArtifact {
        path: manifest_path
            .to_str()
            .ok_or_else(|| ImportError::UnsafeArchiveEntry(manifest_path.display().to_string()))?
            .to_string(),
        size_bytes,
        sha256,
        source_identity: stable_file_identity(&after)?,
        snapshot_identity: stable_file_identity(&snapshot)?,
    })
}

fn copy_owned_regular_file_at(
    source: &Path,
    target_directory: &cap_std::fs::Dir,
    target_name: &Path,
    manifest_path: &Path,
) -> Result<SafetySnapshotArtifact, ImportError> {
    let before =
        fs::symlink_metadata(source).map_err(|error| ImportError::Io(error.to_string()))?;
    if !before.is_file()
        || before.file_type().is_symlink()
        || !owned_regular_file_link_count(&before)
    {
        return Err(ImportError::UnsafeArchiveEntry(
            source.display().to_string(),
        ));
    }
    let mut source_options = OpenOptions::new();
    source_options.read(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt as _;

        source_options.custom_flags(libc::O_NOFOLLOW | libc::O_CLOEXEC | libc::O_NONBLOCK);
    }
    let mut source_file = source_options
        .open(source)
        .map_err(|error| ImportError::Io(error.to_string()))?;
    let opened = source_file
        .metadata()
        .map_err(|error| ImportError::Io(error.to_string()))?;
    if stable_file_identity(&before)? != stable_file_identity(&opened)?
        || !owned_regular_file_link_count(&opened)
    {
        return Err(ImportError::UnsafeArchiveEntry(
            source.display().to_string(),
        ));
    }

    let mut target_options = cap_std::fs::OpenOptions::new();
    target_options.write(true).create_new(true);
    #[cfg(unix)]
    {
        use cap_std::fs::OpenOptionsExt as _;

        target_options.custom_flags(libc::O_NOFOLLOW | libc::O_CLOEXEC);
        target_options.mode(0o600);
    }
    let mut target_file = target_directory
        .open_with(target_name, &target_options)
        .map(cap_std::fs::File::into_std)
        .map_err(|error| ImportError::Io(error.to_string()))?;
    let mut digest = Sha256::new();
    let mut size_bytes = 0_u64;
    let mut buffer = [0_u8; 1024 * 1024];
    loop {
        let read = source_file
            .read(&mut buffer)
            .map_err(|error| ImportError::Io(error.to_string()))?;
        if read == 0 {
            break;
        }
        target_file
            .write_all(&buffer[..read])
            .map_err(|error| ImportError::Io(error.to_string()))?;
        size_bytes = size_bytes
            .checked_add(read as u64)
            .ok_or_else(|| ImportError::Io("safety snapshot size overflow".into()))?;
        digest.update(&buffer[..read]);
    }
    target_file
        .sync_all()
        .map_err(|error| ImportError::Io(error.to_string()))?;
    let after = source_file
        .metadata()
        .map_err(|error| ImportError::Io(error.to_string()))?;
    if stable_file_identity(&before)? != stable_file_identity(&after)?
        || before.len() != after.len()
        || size_bytes != after.len()
        || !owned_regular_file_link_count(&after)
    {
        return Err(ImportError::UnsafeArchiveEntry(
            source.display().to_string(),
        ));
    }
    let snapshot = target_file
        .metadata()
        .map_err(|error| ImportError::Io(error.to_string()))?;
    if !snapshot.is_file()
        || !owned_regular_file_link_count(&snapshot)
        || snapshot.len() != size_bytes
    {
        return Err(ImportError::InvalidManifest(
            "safety snapshot copy mismatch".into(),
        ));
    }
    Ok(SafetySnapshotArtifact {
        path: controlled_relative_path(manifest_path)?,
        size_bytes,
        sha256: hex::encode(digest.finalize()),
        source_identity: stable_file_identity(&after)?,
        snapshot_identity: stable_file_identity(&snapshot)?,
    })
}

fn copy_plugin_tree_with_descriptors(
    source_directory: &cap_std::fs::Dir,
    target_directory: &cap_std::fs::Dir,
    target_name: &Path,
    relative: &Path,
    out: &mut Vec<SafetySnapshotArtifact>,
) -> Result<(), ImportError> {
    validate_relative_path(target_name)
        .map_err(|_| ImportError::UnsafeArchiveEntry(target_name.display().to_string()))?;
    if target_name.components().count() != 1 {
        return Err(ImportError::UnsafeArchiveEntry(
            target_name.display().to_string(),
        ));
    }
    target_directory
        .create_dir(target_name)
        .map_err(|error| ImportError::Io(error.to_string()))?;
    let target = target_directory
        .open_dir(target_name)
        .map_err(|_| ImportError::UnsafeArchiveEntry(target_name.display().to_string()))?;
    copy_plugin_tree_into_directory(source_directory, &target, relative, out)?;
    sync_cap_directory(&target, "fsync safety Plugin target")
}

fn copy_plugin_tree_into_directory(
    source_directory: &cap_std::fs::Dir,
    target_directory: &cap_std::fs::Dir,
    relative: &Path,
    out: &mut Vec<SafetySnapshotArtifact>,
) -> Result<(), ImportError> {
    let mut entries = source_directory
        .entries()
        .map_err(|error| ImportError::Io(error.to_string()))?
        .collect::<Result<Vec<_>, _>>()
        .map_err(|error| ImportError::Io(error.to_string()))?;
    entries.sort_by_key(|entry| entry.file_name());
    for entry in entries {
        let name = entry.file_name();
        let relative_path = relative.join(&name);
        let source_name = Path::new(&name);
        let metadata = source_directory
            .symlink_metadata(source_name)
            .map_err(|error| ImportError::Io(error.to_string()))?;
        if metadata.file_type().is_symlink() {
            return Err(ImportError::UnsafeArchiveEntry(
                relative_path.display().to_string(),
            ));
        }
        if metadata.is_dir() {
            target_directory
                .create_dir(source_name)
                .map_err(|error| ImportError::Io(error.to_string()))?;
            let source_child = source_directory.open_dir(source_name).map_err(|_| {
                ImportError::UnsafeArchiveEntry(relative_path.display().to_string())
            })?;
            let target_child = target_directory.open_dir(source_name).map_err(|_| {
                ImportError::UnsafeArchiveEntry(relative_path.display().to_string())
            })?;
            copy_plugin_tree_into_directory(&source_child, &target_child, &relative_path, out)?;
            sync_cap_directory(&target_child, "fsync safety Plugin child")?;
        } else if metadata.is_file() {
            out.push(copy_owned_regular_file_between_directories(
                source_directory,
                source_name,
                target_directory,
                source_name,
                &relative_path,
            )?);
        } else {
            return Err(ImportError::UnsafeArchiveEntry(
                relative_path.display().to_string(),
            ));
        }
    }
    sync_cap_directory(target_directory, "fsync safety Plugin directory")
}

fn copy_owned_regular_file_between_directories(
    source_directory: &cap_std::fs::Dir,
    source_name: &Path,
    target_directory: &cap_std::fs::Dir,
    target_name: &Path,
    manifest_path: &Path,
) -> Result<SafetySnapshotArtifact, ImportError> {
    let source_leaf = open_no_follow_leaf_at(source_directory, source_name)?;
    let mut source = source_leaf
        .file
        .try_clone()
        .map_err(|error| ImportError::Io(error.to_string()))?;
    let mut options = cap_std::fs::OpenOptions::new();
    options.write(true).create_new(true);
    #[cfg(unix)]
    {
        use cap_std::fs::OpenOptionsExt as _;
        options.custom_flags(libc::O_NOFOLLOW | libc::O_CLOEXEC);
        options.mode(0o600);
    }
    let mut target = target_directory
        .open_with(target_name, &options)
        .map(cap_std::fs::File::into_std)
        .map_err(|error| ImportError::Io(error.to_string()))?;
    let mut digest = Sha256::new();
    let mut size_bytes = 0_u64;
    let mut buffer = [0_u8; 64 * 1024];
    loop {
        let count = source
            .read(&mut buffer)
            .map_err(|error| ImportError::Io(error.to_string()))?;
        if count == 0 {
            break;
        }
        target
            .write_all(&buffer[..count])
            .map_err(|error| ImportError::Io(error.to_string()))?;
        digest.update(&buffer[..count]);
        size_bytes = size_bytes
            .checked_add(count as u64)
            .ok_or_else(|| ImportError::InvalidManifest("Plugin file too large".into()))?;
    }
    target
        .sync_all()
        .map_err(|error| ImportError::Io(error.to_string()))?;
    let canonical = open_no_follow_leaf_at(source_directory, source_name)?;
    if canonical.identity != source_leaf.identity {
        return Err(ImportError::UnsafeArchiveEntry(
            manifest_path.display().to_string(),
        ));
    }
    let snapshot = target
        .metadata()
        .map_err(|error| ImportError::Io(error.to_string()))?;
    Ok(SafetySnapshotArtifact {
        path: controlled_relative_path(manifest_path)?,
        size_bytes,
        sha256: hex::encode(digest.finalize()),
        source_identity: source_leaf.identity,
        snapshot_identity: stable_file_identity(&snapshot)?,
    })
}

fn write_staging_file_at(
    directory: &cap_std::fs::Dir,
    name: &Path,
    bytes: &[u8],
) -> Result<(), ImportError> {
    validate_relative_path(name)
        .map_err(|_| ImportError::UnsafeArchiveEntry(name.display().to_string()))?;
    if name.components().count() != 1 {
        return Err(ImportError::UnsafeArchiveEntry(name.display().to_string()));
    }
    let staging_name = PathBuf::from(format!(
        ".{}.{}.staging",
        name.to_string_lossy(),
        Uuid::new_v4()
    ));
    let mut options = cap_std::fs::OpenOptions::new();
    options.write(true).create_new(true);
    #[cfg(unix)]
    {
        use cap_std::fs::OpenOptionsExt as _;

        options.custom_flags(libc::O_NOFOLLOW | libc::O_CLOEXEC);
        options.mode(0o600);
    }
    let write_result = (|| {
        let mut file = directory
            .open_with(&staging_name, &options)
            .map(cap_std::fs::File::into_std)
            .map_err(|error| ImportError::Io(error.to_string()))?;
        file.write_all(bytes)
            .and_then(|()| file.sync_all())
            .map_err(|error| ImportError::Io(error.to_string()))?;
        publish_no_replace_at(directory, &staging_name, directory, name)
            .map_err(|error| ImportError::Io(error.to_string()))?;
        sync_cap_directory(directory, "fsync dirfd-relative staging publication")
    })();
    if write_result.is_err() {
        let _ = directory.remove_file(&staging_name);
    }
    write_result
}

fn inspect_owned_regular_file_at(
    directory: &cap_std::fs::Dir,
    name: &Path,
) -> Result<Option<FileEvidence>, ImportError> {
    validate_relative_path(name)
        .map_err(|_| ImportError::UnsafeArchiveEntry(name.display().to_string()))?;
    let entry = match directory.symlink_metadata(name) {
        Ok(entry) => entry,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(error) => return Err(ImportError::Io(error.to_string())),
    };
    if !entry.is_file() || entry.file_type().is_symlink() {
        return Err(ImportError::UnsafeArchiveEntry(name.display().to_string()));
    }
    #[cfg(unix)]
    {
        use cap_std::fs::MetadataExt as _;

        if entry.nlink() != 1 {
            return Err(ImportError::UnsafeArchiveEntry(name.display().to_string()));
        }
    }
    let entry_identity = stable_cap_file_identity(&entry)?;
    let mut options = cap_std::fs::OpenOptions::new();
    options.read(true);
    #[cfg(unix)]
    {
        use cap_std::fs::OpenOptionsExt as _;

        options.custom_flags(libc::O_NOFOLLOW | libc::O_CLOEXEC | libc::O_NONBLOCK);
    }
    let mut file = directory
        .open_with(name, &options)
        .map(cap_std::fs::File::into_std)
        .map_err(|_| ImportError::UnsafeArchiveEntry(name.display().to_string()))?;
    let before = file
        .metadata()
        .map_err(|_| ImportError::UnsafeArchiveEntry(name.display().to_string()))?;
    if !before.is_file()
        || !owned_regular_file_link_count(&before)
        || stable_file_identity(&before)? != entry_identity
    {
        return Err(ImportError::UnsafeArchiveEntry(name.display().to_string()));
    }
    let mut digest = Sha256::new();
    let mut size_bytes = 0_u64;
    let mut buffer = [0_u8; 64 * 1024];
    loop {
        let read = file
            .read(&mut buffer)
            .map_err(|error| ImportError::Io(error.to_string()))?;
        if read == 0 {
            break;
        }
        size_bytes = size_bytes
            .checked_add(read as u64)
            .ok_or_else(|| ImportError::Io("owned file size overflow".into()))?;
        digest.update(&buffer[..read]);
    }
    let after = file
        .metadata()
        .map_err(|_| ImportError::UnsafeArchiveEntry(name.display().to_string()))?;
    if stable_file_identity(&before)? != stable_file_identity(&after)?
        || stable_file_identity(&after)? != entry_identity
        || before.len() != after.len()
        || size_bytes != after.len()
        || !owned_regular_file_link_count(&after)
    {
        return Err(ImportError::UnsafeArchiveEntry(name.display().to_string()));
    }
    Ok(Some(FileEvidence {
        identity: entry_identity,
        size_bytes,
        sha256: hex::encode(digest.finalize()),
    }))
}

fn read_control_file_at(directory: &cap_std::fs::Dir, name: &Path) -> Result<Vec<u8>, ImportError> {
    const MAX_CONTROL_BYTES: u64 = 1024 * 1024;

    validate_relative_path(name)
        .map_err(|_| ImportError::UnsafeArchiveEntry(name.display().to_string()))?;
    let entry = directory
        .symlink_metadata(name)
        .map_err(|_| ImportError::InvalidManifest("missing control file".into()))?;
    if !entry.is_file() || entry.file_type().is_symlink() || entry.len() > MAX_CONTROL_BYTES {
        return Err(ImportError::UnsafeArchiveEntry(name.display().to_string()));
    }
    let entry_identity = stable_cap_file_identity(&entry)?;
    let mut options = cap_std::fs::OpenOptions::new();
    options.read(true);
    #[cfg(unix)]
    {
        use cap_std::fs::OpenOptionsExt as _;

        options.custom_flags(libc::O_NOFOLLOW | libc::O_CLOEXEC | libc::O_NONBLOCK);
    }
    let mut file = directory
        .open_with(name, &options)
        .map(cap_std::fs::File::into_std)
        .map_err(|_| ImportError::UnsafeArchiveEntry(name.display().to_string()))?;
    let before = file
        .metadata()
        .map_err(|_| ImportError::UnsafeArchiveEntry(name.display().to_string()))?;
    if !before.is_file()
        || before.len() > MAX_CONTROL_BYTES
        || !owned_regular_file_link_count(&before)
        || stable_file_identity(&before)? != entry_identity
    {
        return Err(ImportError::UnsafeArchiveEntry(name.display().to_string()));
    }
    let mut bytes = Vec::with_capacity(before.len() as usize);
    Read::by_ref(&mut file)
        .take(MAX_CONTROL_BYTES + 1)
        .read_to_end(&mut bytes)
        .map_err(|_| ImportError::Io("read control file".into()))?;
    let after = file
        .metadata()
        .map_err(|_| ImportError::UnsafeArchiveEntry(name.display().to_string()))?;
    if bytes.len() as u64 > MAX_CONTROL_BYTES
        || before.len() != after.len()
        || stable_file_identity(&before)? != stable_file_identity(&after)?
        || stable_file_identity(&after)? != entry_identity
        || !owned_regular_file_link_count(&after)
    {
        return Err(ImportError::UnsafeArchiveEntry(name.display().to_string()));
    }
    Ok(bytes)
}

fn hash_owned_tree_entries_at(
    directory: &cap_std::fs::Dir,
    relative: &Path,
    digest: &mut Sha256,
) -> Result<(), ImportError> {
    let mut entries = directory
        .entries()
        .map_err(|error| ImportError::Io(error.to_string()))?
        .collect::<Result<Vec<_>, _>>()
        .map_err(|error| ImportError::Io(error.to_string()))?;
    entries.sort_by_key(|entry| entry.file_name());
    for entry in entries {
        let name = entry.file_name();
        let name_path = Path::new(&name);
        let entry_relative = relative.join(&name);
        validate_relative_path(&entry_relative)
            .map_err(|_| ImportError::UnsafeArchiveEntry(entry_relative.display().to_string()))?;
        let encoded = entry_relative.as_os_str().as_encoded_bytes();
        let metadata = directory
            .symlink_metadata(name_path)
            .map_err(|error| ImportError::Io(error.to_string()))?;
        if metadata.file_type().is_symlink() {
            return Err(ImportError::UnsafeArchiveEntry(
                entry_relative.display().to_string(),
            ));
        }
        if metadata.is_dir() {
            digest.update(b"directory\0");
            digest.update((encoded.len() as u64).to_be_bytes());
            digest.update(encoded);
            let child = directory.open_dir(name_path).map_err(|_| {
                ImportError::UnsafeArchiveEntry(entry_relative.display().to_string())
            })?;
            hash_owned_tree_entries_at(&child, &entry_relative, digest)?;
        } else if metadata.is_file() {
            let file = inspect_owned_regular_file_at(directory, name_path)?
                .ok_or_else(|| ImportError::InvalidManifest("tree file disappeared".into()))?;
            digest.update(b"file\0");
            digest.update((encoded.len() as u64).to_be_bytes());
            digest.update(encoded);
            digest.update(file.size_bytes.to_be_bytes());
            digest.update(file.sha256.as_bytes());
        } else {
            return Err(ImportError::UnsafeArchiveEntry(
                entry_relative.display().to_string(),
            ));
        }
    }
    Ok(())
}

fn controlled_tree_evidence_at(
    directory: &cap_std::fs::Dir,
    relative_path: &Path,
) -> Result<ControlledTreeEvidence, ImportError> {
    let before = directory
        .dir_metadata()
        .map_err(|error| ImportError::Io(error.to_string()))?;
    if !before.is_dir() {
        return Err(ImportError::UnsafeArchiveEntry(
            relative_path.display().to_string(),
        ));
    }
    let identity = stable_cap_file_identity(&before)?;
    let mut digest = Sha256::new();
    digest.update(b"hivegui-controlled-tree-v1\0");
    hash_owned_tree_entries_at(directory, Path::new(""), &mut digest)?;
    let after = directory
        .dir_metadata()
        .map_err(|error| ImportError::Io(error.to_string()))?;
    if !after.is_dir() || stable_cap_file_identity(&after)? != identity {
        return Err(ImportError::UnsafeArchiveEntry(
            relative_path.display().to_string(),
        ));
    }
    Ok(ControlledTreeEvidence {
        relative_path: controlled_relative_path(relative_path)?,
        identity,
        sha256: hex::encode(digest.finalize()),
    })
}

fn collect_relative_regular_files_at(
    directory: &cap_std::fs::Dir,
) -> Result<BTreeSet<PathBuf>, ImportError> {
    fn collect(
        directory: &cap_std::fs::Dir,
        relative: &Path,
        files: &mut BTreeSet<PathBuf>,
    ) -> Result<(), ImportError> {
        let mut entries = directory
            .entries()
            .map_err(|error| ImportError::Io(error.to_string()))?
            .collect::<Result<Vec<_>, _>>()
            .map_err(|error| ImportError::Io(error.to_string()))?;
        entries.sort_by_key(|entry| entry.file_name());
        for entry in entries {
            let name = entry.file_name();
            let name_path = Path::new(&name);
            let entry_relative = relative.join(&name);
            let metadata = directory
                .symlink_metadata(name_path)
                .map_err(|error| ImportError::Io(error.to_string()))?;
            if metadata.file_type().is_symlink() {
                return Err(ImportError::UnsafeArchiveEntry(
                    entry_relative.display().to_string(),
                ));
            }
            if metadata.is_dir() {
                let child = directory.open_dir(name_path).map_err(|_| {
                    ImportError::UnsafeArchiveEntry(entry_relative.display().to_string())
                })?;
                collect(&child, &entry_relative, files)?;
            } else if metadata.is_file() {
                inspect_owned_regular_file_at(directory, name_path)?
                    .ok_or_else(|| ImportError::InvalidManifest("tree file disappeared".into()))?;
                files.insert(entry_relative);
            } else {
                return Err(ImportError::UnsafeArchiveEntry(
                    entry_relative.display().to_string(),
                ));
            }
        }
        Ok(())
    }

    let mut files = BTreeSet::new();
    collect(directory, Path::new(""), &mut files)?;
    Ok(files)
}

fn remove_cap_directory_contents(directory: &cap_std::fs::Dir) -> Result<(), ImportError> {
    let mut entries = directory
        .entries()
        .map_err(|error| ImportError::Io(error.to_string()))?
        .collect::<Result<Vec<_>, _>>()
        .map_err(|error| ImportError::Io(error.to_string()))?;
    entries.sort_by_key(|entry| entry.file_name());
    for entry in entries {
        let name = entry.file_name();
        let name_path = Path::new(&name);
        let metadata = directory
            .symlink_metadata(name_path)
            .map_err(|error| ImportError::Io(error.to_string()))?;
        if metadata.is_dir() && !metadata.file_type().is_symlink() {
            let child = directory
                .open_dir(name_path)
                .map_err(|_| ImportError::UnsafeArchiveEntry(name_path.display().to_string()))?;
            remove_cap_directory_contents(&child)?;
            directory
                .remove_dir(name_path)
                .map_err(|error| ImportError::Io(error.to_string()))?;
        } else if metadata.is_file() && !metadata.file_type().is_symlink() {
            directory
                .remove_file(name_path)
                .map_err(|error| ImportError::Io(error.to_string()))?;
        } else {
            return Err(ImportError::UnsafeArchiveEntry(
                name_path.display().to_string(),
            ));
        }
    }
    Ok(())
}

fn create_restore_safety_snapshot(
    root: &Path,
    current_database: &Path,
    _current_plugins: &Path,
    operation_id: &str,
) -> Result<VerifiedRestoreSafetySnapshot, ImportError> {
    let root_directory = open_ambient_directory_nofollow(root)?;
    let snapshot_binding = begin_restore_safety_snapshot(&root_directory, root, operation_id)?;
    verify_snapshot_directory_binding(&snapshot_binding)?;
    let result = (|| {
        let database = if current_database.exists() {
            Some(copy_owned_regular_file_at(
                current_database,
                snapshot_binding.directory(),
                Path::new(DATABASE_FILENAME),
                Path::new(DATABASE_FILENAME),
            )?)
        } else {
            None
        };
        let current_plugins_directory = match root_directory.symlink_metadata(Path::new("plugins"))
        {
            Ok(metadata) if metadata.is_dir() && !metadata.file_type().is_symlink() => Some(
                root_directory
                    .open_dir(Path::new("plugins"))
                    .map_err(|_| ImportError::UnsafeArchiveEntry("plugins".into()))?,
            ),
            Ok(_) => return Err(ImportError::UnsafeArchiveEntry("plugins".into())),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => None,
            Err(error) => return Err(ImportError::Io(error.to_string())),
        };
        finish_restore_safety_snapshot(
            &snapshot_binding,
            current_plugins_directory.as_ref(),
            operation_id,
            database,
        )?;
        verify_restore_safety_snapshot(&snapshot_binding)?;
        verify_snapshot_directory_binding(&snapshot_binding)
    })();
    let (snapshot_binding, ()) =
        finish_or_cleanup_restore_safety_snapshot(snapshot_binding, result)?;
    VerifiedRestoreSafetySnapshot::new(
        snapshot_binding,
        &PathBuf::from("backups").join(format!("restore-safety-{operation_id}")),
    )
}

fn begin_restore_safety_snapshot(
    root_directory: &cap_std::fs::Dir,
    root: &Path,
    operation_id: &str,
) -> Result<RestoreSafetySnapshotBinding, ImportError> {
    let backups_name = Path::new("backups");
    match root_directory.symlink_metadata(backups_name) {
        Ok(metadata) => {
            if !metadata.is_dir() || metadata.file_type().is_symlink() {
                return Err(ImportError::UnsafeArchiveEntry(
                    root.join(backups_name).display().to_string(),
                ));
            }
        }
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            root_directory
                .create_dir(backups_name)
                .map_err(|error| ImportError::Io(error.to_string()))?;
            sync_cap_directory(root_directory, "fsync restore root after backups create")?;
        }
        Err(error) => return Err(ImportError::Io(error.to_string())),
    }
    let parent_directory = root_directory.open_dir(backups_name).map_err(|_| {
        ImportError::UnsafeArchiveEntry(root.join(backups_name).display().to_string())
    })?;
    let canonical_name = PathBuf::from(format!("restore-safety-{operation_id}"));
    parent_directory
        .create_dir(&canonical_name)
        .map_err(|error| ImportError::Io(error.to_string()))?;
    sync_cap_directory(
        &parent_directory,
        "fsync restore backups after snapshot create",
    )?;
    let canonical_metadata = parent_directory
        .symlink_metadata(&canonical_name)
        .map_err(|_| ImportError::UnsafeArchiveEntry(canonical_name.display().to_string()))?;
    if !canonical_metadata.is_dir() || canonical_metadata.file_type().is_symlink() {
        return Err(ImportError::UnsafeArchiveEntry(
            canonical_name.display().to_string(),
        ));
    }
    let canonical_identity = stable_cap_file_identity(&canonical_metadata)?;
    let directory = parent_directory
        .open_dir(&canonical_name)
        .map_err(|_| ImportError::UnsafeArchiveEntry(canonical_name.display().to_string()))?;
    let held_metadata = directory
        .dir_metadata()
        .map_err(|_| ImportError::UnsafeArchiveEntry(canonical_name.display().to_string()))?;
    if !held_metadata.is_dir() || stable_cap_file_identity(&held_metadata)? != canonical_identity {
        return Err(ImportError::UnsafeArchiveEntry(
            canonical_name.display().to_string(),
        ));
    }
    Ok(RestoreSafetySnapshotBinding {
        parent_directory,
        directory,
        canonical_identity,
        canonical_path: root.join(backups_name).join(&canonical_name),
        canonical_name,
    })
}

fn verify_snapshot_directory_binding(
    snapshot_binding: &RestoreSafetySnapshotBinding,
) -> Result<(), ImportError> {
    let held_metadata = snapshot_binding.directory().dir_metadata().map_err(|_| {
        ImportError::UnsafeArchiveEntry("restore safety snapshot identity changed".into())
    })?;
    let canonical_metadata = snapshot_binding
        .parent_directory()
        .symlink_metadata(&snapshot_binding.canonical_name)
        .map_err(|_| {
            ImportError::UnsafeArchiveEntry("restore safety snapshot identity changed".into())
        })?;
    if !held_metadata.is_dir()
        || !canonical_metadata.is_dir()
        || canonical_metadata.file_type().is_symlink()
        || stable_cap_file_identity(&held_metadata)? != snapshot_binding.canonical_identity
        || stable_cap_file_identity(&canonical_metadata)? != snapshot_binding.canonical_identity
    {
        return Err(ImportError::UnsafeArchiveEntry(
            "restore safety snapshot identity changed".into(),
        ));
    }
    Ok(())
}

fn finish_restore_safety_snapshot(
    snapshot_binding: &RestoreSafetySnapshotBinding,
    current_plugins: Option<&cap_std::fs::Dir>,
    operation_id: &str,
    database: Option<SafetySnapshotArtifact>,
) -> Result<(), ImportError> {
    let mut plugin_files = Vec::new();
    let plugin_root = if let Some(current_plugins) = current_plugins {
        let source = controlled_tree_evidence_at(current_plugins, Path::new("plugins"))?;
        copy_plugin_tree_with_descriptors(
            current_plugins,
            snapshot_binding.directory(),
            Path::new("plugins"),
            Path::new("plugins"),
            &mut plugin_files,
        )?;
        let copied_directory = snapshot_binding
            .directory()
            .open_dir(Path::new("plugins"))
            .map_err(|_| ImportError::UnsafeArchiveEntry("plugins".into()))?;
        let copied = controlled_tree_evidence_at(&copied_directory, Path::new("plugins"))?;
        if source.sha256 != copied.sha256 {
            return Err(ImportError::InvalidManifest(
                "safety Plugin tree copy mismatch".into(),
            ));
        }
        Some(SafetySnapshotTree {
            path: "plugins".into(),
            sha256: source.sha256,
            source_identity: source.identity,
            snapshot_identity: copied.identity,
        })
    } else {
        None
    };
    plugin_files.sort_by(|left, right| left.path.cmp(&right.path));
    let manifest = SafetySnapshotManifest {
        schema_version: 1,
        operation_id: operation_id.into(),
        database,
        plugin_root,
        plugin_files,
    };
    let bytes = serde_json::to_vec(&manifest)
        .map_err(|error| ImportError::InvalidManifest(error.to_string()))?;
    write_staging_file_at(
        snapshot_binding.directory(),
        Path::new("manifest.json"),
        &bytes,
    )?;
    sync_cap_directory(
        snapshot_binding.directory(),
        "fsync complete restore safety snapshot",
    )?;
    sync_cap_directory(
        snapshot_binding.parent_directory(),
        "fsync restore safety snapshot parent",
    )
}

fn finish_or_cleanup_restore_safety_snapshot<T>(
    snapshot_binding: RestoreSafetySnapshotBinding,
    result: Result<T, ImportError>,
) -> Result<(RestoreSafetySnapshotBinding, T), ImportError> {
    match result {
        Ok(value) => Ok((snapshot_binding, value)),
        Err(error) => {
            if verify_snapshot_directory_binding(&snapshot_binding).is_ok() {
                remove_cap_directory_contents(snapshot_binding.directory())?;
                sync_cap_directory(
                    snapshot_binding.directory(),
                    "fsync cleared restore safety snapshot",
                )?;
                snapshot_binding
                    .parent_directory()
                    .remove_dir(&snapshot_binding.canonical_name)
                    .map_err(|cleanup| ImportError::Io(cleanup.to_string()))?;
                sync_cap_directory(
                    snapshot_binding.parent_directory(),
                    "fsync restore backups after snapshot cleanup",
                )?;
            }
            Err(error)
        }
    }
}

fn verify_restore_safety_snapshot(
    snapshot_binding: &RestoreSafetySnapshotBinding,
) -> Result<(), ImportError> {
    let manifest: SafetySnapshotManifest = serde_json::from_slice(&read_control_file_at(
        snapshot_binding.directory(),
        Path::new("manifest.json"),
    )?)
    .map_err(|_| ImportError::InvalidManifest("invalid safety snapshot manifest".into()))?;
    if manifest.schema_version != 1 || !is_canonical_uuid(&manifest.operation_id) {
        return Err(ImportError::InvalidManifest(
            "safety snapshot identity mismatch".into(),
        ));
    }
    match manifest.database.as_ref() {
        Some(database) => {
            if database.path != "datasources.db"
                || database.source_identity.is_empty()
                || inspect_owned_regular_file_at(
                    snapshot_binding.directory(),
                    Path::new(DATABASE_FILENAME),
                )?
                .is_none_or(|evidence| {
                    evidence.identity != database.snapshot_identity
                        || evidence.size_bytes != database.size_bytes
                        || evidence.sha256 != database.sha256
                })
            {
                return Err(ImportError::InvalidManifest(
                    "safety snapshot database mismatch".into(),
                ));
            }
        }
        None if snapshot_binding
            .directory()
            .symlink_metadata(Path::new(DATABASE_FILENAME))
            .is_ok() =>
        {
            return Err(ImportError::InvalidManifest(
                "unexpected safety snapshot database".into(),
            ));
        }
        None => {}
    }
    let mut expected_plugins = BTreeSet::new();
    let mut previous = None;
    for artifact in &manifest.plugin_files {
        let relative = PathBuf::from(&artifact.path);
        let plugin_relative = relative.strip_prefix("plugins").map_err(|_| {
            ImportError::InvalidManifest("safety Plugin path is outside plugins".into())
        })?;
        validate_relative_path(plugin_relative)
            .map_err(|_| ImportError::InvalidManifest("invalid safety Plugin path".into()))?;
        if previous.as_ref().is_some_and(|path| path >= &artifact.path)
            || !expected_plugins.insert(plugin_relative.to_path_buf())
            || artifact.source_identity.is_empty()
            || inspect_owned_regular_file_at(snapshot_binding.directory(), &relative)?.is_none_or(
                |evidence| {
                    evidence.identity != artifact.snapshot_identity
                        || evidence.size_bytes != artifact.size_bytes
                        || evidence.sha256 != artifact.sha256
                },
            )
        {
            return Err(ImportError::InvalidManifest(
                "safety Plugin descriptor mismatch".into(),
            ));
        }
        previous = Some(artifact.path.clone());
    }
    let snapshot_plugins = match snapshot_binding
        .directory()
        .symlink_metadata(Path::new("plugins"))
    {
        Ok(metadata) => {
            if !metadata.is_dir() || metadata.file_type().is_symlink() {
                return Err(ImportError::InvalidManifest(
                    "unsafe safety Plugin root".into(),
                ));
            }
            Some(
                snapshot_binding
                    .directory()
                    .open_dir(Path::new("plugins"))
                    .map_err(|_| ImportError::UnsafeArchiveEntry("plugins".into()))?,
            )
        }
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => None,
        Err(error) => return Err(ImportError::Io(error.to_string())),
    };
    if snapshot_plugins
        .as_ref()
        .map(collect_relative_regular_files_at)
        .transpose()?
        .unwrap_or_default()
        != expected_plugins
    {
        return Err(ImportError::InvalidManifest(
            "safety Plugin inventory mismatch".into(),
        ));
    }
    match manifest.plugin_root.as_ref() {
        Some(plugin_root) => {
            if plugin_root.path != "plugins"
                || plugin_root.source_identity.is_empty()
                || controlled_tree_evidence_at(
                    snapshot_plugins.as_ref().ok_or_else(|| {
                        ImportError::InvalidManifest("safety Plugin root is missing".into())
                    })?,
                    Path::new("plugins"),
                )? != (ControlledTreeEvidence {
                    relative_path: "plugins".into(),
                    identity: plugin_root.snapshot_identity.clone(),
                    sha256: plugin_root.sha256.clone(),
                })
            {
                return Err(ImportError::InvalidManifest(
                    "safety Plugin root mismatch".into(),
                ));
            }
        }
        None if snapshot_plugins.is_some() || !manifest.plugin_files.is_empty() => {
            return Err(ImportError::InvalidManifest(
                "unexpected safety Plugin root".into(),
            ));
        }
        None => {}
    }
    Ok(())
}

fn open_restore_safety_snapshot_binding(
    snapshot: &Path,
) -> Result<RestoreSafetySnapshotBinding, ImportError> {
    let parent = snapshot
        .parent()
        .ok_or_else(|| ImportError::UnsafeArchiveEntry(snapshot.display().to_string()))?;
    let canonical_name = snapshot
        .file_name()
        .map(PathBuf::from)
        .ok_or_else(|| ImportError::UnsafeArchiveEntry(snapshot.display().to_string()))?;
    let parent_directory = open_ambient_directory_nofollow(parent)?;
    let canonical_metadata = parent_directory
        .symlink_metadata(&canonical_name)
        .map_err(|_| ImportError::UnsafeArchiveEntry(snapshot.display().to_string()))?;
    if !canonical_metadata.is_dir() || canonical_metadata.file_type().is_symlink() {
        return Err(ImportError::UnsafeArchiveEntry(
            snapshot.display().to_string(),
        ));
    }
    let canonical_identity = stable_cap_file_identity(&canonical_metadata)?;
    let directory = parent_directory
        .open_dir(&canonical_name)
        .map_err(|_| ImportError::UnsafeArchiveEntry(snapshot.display().to_string()))?;
    Ok(RestoreSafetySnapshotBinding {
        parent_directory,
        directory,
        canonical_identity,
        canonical_path: snapshot.to_path_buf(),
        canonical_name,
    })
}

fn verify_restore_safety_snapshot_path(snapshot: &Path) -> Result<(), ImportError> {
    let snapshot_binding = open_restore_safety_snapshot_binding(snapshot)?;
    verify_snapshot_directory_binding(&snapshot_binding)?;
    verify_restore_safety_snapshot(&snapshot_binding)?;
    verify_snapshot_directory_binding(&snapshot_binding)
}

fn restore_missing_old_from_safety_snapshot(
    root: &Path,
    current_database: &Path,
    current_plugins: &Path,
    owner: &RestoreOwner,
) -> Result<(), ImportError> {
    let snapshot = root.join(&owner.safety_snapshot.relative_path);
    verify_restore_safety_snapshot_path(&snapshot)?;
    if controlled_tree_evidence(&snapshot, Path::new(&owner.safety_snapshot.relative_path))?
        != owner.safety_snapshot
    {
        return Err(ImportError::InvalidManifest(
            "restore safety snapshot owner mismatch".into(),
        ));
    }
    let manifest: SafetySnapshotManifest = serde_json::from_slice(&read_control_file_nofollow(
        &snapshot.join("manifest.json"),
    )?)
    .map_err(|_| ImportError::InvalidManifest("invalid safety snapshot manifest".into()))?;
    if manifest.operation_id != owner.db_instance_operation_id {
        return Err(ImportError::InvalidManifest(
            "restore safety snapshot operation mismatch".into(),
        ));
    }

    let actual_database =
        optional_controlled_file_evidence(current_database, Path::new(DATABASE_FILENAME))?;
    match (owner.old_database.as_ref(), actual_database.as_ref()) {
        (Some(expected), Some(actual)) if same_file_content(Some(actual), Some(expected)) => {}
        (Some(_), Some(_)) => {
            return Err(ImportError::InvalidManifest(
                "existing current database does not match old or safety state".into(),
            ));
        }
        (Some(expected), None) => {
            let safety = manifest.database.as_ref().ok_or_else(|| {
                ImportError::InvalidManifest("safety snapshot database is missing".into())
            })?;
            if safety.path != DATABASE_FILENAME
                || safety.source_identity != expected.identity
                || safety.size_bytes != expected.size_bytes
                || safety.sha256 != expected.sha256
            {
                return Err(ImportError::InvalidManifest(
                    "safety snapshot database source mismatch".into(),
                ));
            }
            copy_owned_regular_file(
                &snapshot.join(DATABASE_FILENAME),
                current_database,
                Path::new(DATABASE_FILENAME),
            )?;
        }
        (None, None) => {}
        (None, Some(_)) => {
            return Err(ImportError::InvalidManifest(
                "unexpected current database while restoring absent old state".into(),
            ));
        }
    }

    let actual_plugins = optional_controlled_tree_evidence(current_plugins, Path::new("plugins"))?;
    match (owner.old_plugin_root.as_ref(), actual_plugins.as_ref()) {
        (Some(expected), Some(actual)) if same_tree_content(Some(actual), Some(expected)) => {}
        (Some(_), Some(_)) => {
            return Err(ImportError::InvalidManifest(
                "existing current Plugin tree does not match old or safety state".into(),
            ));
        }
        (Some(expected), None) => {
            let safety = manifest.plugin_root.as_ref().ok_or_else(|| {
                ImportError::InvalidManifest("safety snapshot Plugin root is missing".into())
            })?;
            if safety.path != "plugins"
                || safety.source_identity != expected.identity
                || safety.sha256 != expected.sha256
            {
                return Err(ImportError::InvalidManifest(
                    "safety snapshot Plugin source mismatch".into(),
                ));
            }
            let mut copied = Vec::new();
            let root_directory = open_ambient_directory_nofollow(root)?;
            let snapshot_plugins_directory =
                open_ambient_directory_nofollow(&snapshot.join("plugins"))?;
            copy_plugin_tree_with_descriptors(
                &snapshot_plugins_directory,
                &root_directory,
                Path::new("plugins"),
                Path::new("plugins"),
                &mut copied,
            )?;
        }
        (None, None) => {}
        (None, Some(_)) => {
            return Err(ImportError::InvalidManifest(
                "unexpected current Plugin tree while restoring absent old state".into(),
            ));
        }
    }
    sync_directory(root)
}

fn sync_tree_bottom_up(root: &Path) -> Result<(), ImportError> {
    if root.is_dir() {
        for entry in fs::read_dir(root).map_err(|error| ImportError::Io(error.to_string()))? {
            let path = entry
                .map_err(|error| ImportError::Io(error.to_string()))?
                .path();
            if path.is_dir() {
                sync_tree_bottom_up(&path)?;
            } else if path.is_file() {
                File::open(&path)
                    .and_then(|file| file.sync_all())
                    .map_err(|error| ImportError::Io(error.to_string()))?;
            }
        }
        sync_directory(root)?;
    }
    Ok(())
}

fn rollback_restore_switch(
    current_database: &Path,
    current_plugins: &Path,
    staging_database: &Path,
    staging_plugins: &Path,
    old_database: &Path,
    old_plugins: &Path,
) -> Result<(), ImportError> {
    if current_database.exists() && !staging_database.exists() {
        publish_no_replace(current_database, staging_database)
            .map_err(|error| ImportError::Io(error.to_string()))?;
    }
    if old_database.exists() {
        publish_no_replace(old_database, current_database)
            .map_err(|error| ImportError::Io(error.to_string()))?;
    }
    if current_plugins.exists() && !staging_plugins.exists() {
        publish_no_replace(current_plugins, staging_plugins)
            .map_err(|error| ImportError::Io(error.to_string()))?;
    }
    if old_plugins.exists() {
        publish_no_replace(old_plugins, current_plugins)
            .map_err(|error| ImportError::Io(error.to_string()))?;
    }
    Ok(())
}

fn retire_terminal_instance(
    registry: &Path,
    live_basename: &str,
    manifest: &RestoreInstanceManifest,
    owner: &RestoreOwner,
    outcome: RetirementOutcome,
) -> Result<(), ImportError> {
    retire_terminal_instance_with_crash(registry, live_basename, manifest, owner, outcome, None)
}

fn retire_terminal_instance_with_crash(
    registry: &Path,
    live_basename: &str,
    manifest: &RestoreInstanceManifest,
    owner: &RestoreOwner,
    outcome: RetirementOutcome,
    crash_at: Option<BackupCrashPoint>,
) -> Result<(), ImportError> {
    let (outcome_name, phase_valid) = match outcome {
        RetirementOutcome::Old => (
            "old",
            matches!(owner.phase.as_str(), "prepared" | "applying"),
        ),
        RetirementOutcome::New => ("new", owner.phase == "committed"),
        RetirementOutcome::AbortedPreSwitch => ("aborted_pre_switch", false),
    };
    if manifest.ownership_state != "armed" || !phase_valid {
        return Err(ImportError::InvalidManifest(
            "terminal retirement owner mismatch".into(),
        ));
    }
    let root = registry
        .parent()
        .ok_or_else(|| ImportError::InvalidManifest("registry has no data root".into()))?;
    let live = registry.join(live_basename);
    let live_metadata =
        fs::symlink_metadata(&live).map_err(|error| ImportError::Io(error.to_string()))?;
    if !live_metadata.is_dir() || live_metadata.file_type().is_symlink() {
        return Err(ImportError::UnsafeArchiveEntry(live.display().to_string()));
    }
    let live_directory_identity = stable_file_identity(&live_metadata)?;
    let tombstone_basename = format!(".hivegui-db-retired-v1-{outcome_name}-{live_basename}");
    let tombstone = registry.join(&tombstone_basename);
    let journal_basename = format!(
        ".hivegui-db-retirement-v1-restore-{}.json",
        manifest.db_instance_operation_id
    );
    let journal_path = registry.join(&journal_basename);
    let journal_staging = registry.join(format!("{journal_basename}.staging"));
    let live_relative = PathBuf::from(".hivegui-db-staging-v1").join(live_basename);
    let manifest_evidence = controlled_file_evidence(
        &live.join(".hivegui-db-instance-v1.json"),
        &live_relative.join(".hivegui-db-instance-v1.json"),
    )?;
    let owner_evidence = controlled_file_evidence(
        &live.join(".hivegui-db-recovery-v1.json"),
        &live_relative.join(".hivegui-db-recovery-v1.json"),
    )?;
    let terminal_database = optional_controlled_file_evidence(
        &root.join(DATABASE_FILENAME),
        Path::new(DATABASE_FILENAME),
    )?;
    let terminal_plugin_root =
        optional_controlled_tree_evidence(&root.join("plugins"), Path::new("plugins"))?;
    let expected_database = match outcome {
        RetirementOutcome::Old => owner.old_database.as_ref(),
        RetirementOutcome::New => Some(&owner.new_database),
        RetirementOutcome::AbortedPreSwitch => None,
    };
    let database_matches = match outcome {
        RetirementOutcome::Old => same_file_content(terminal_database.as_ref(), expected_database),
        RetirementOutcome::New => same_file_object(terminal_database.as_ref(), expected_database),
        RetirementOutcome::AbortedPreSwitch => terminal_database.is_none(),
    };
    if !database_matches {
        return Err(ImportError::InvalidManifest(
            "terminal database evidence mismatch".into(),
        ));
    }
    let expected_plugins = match outcome {
        RetirementOutcome::Old => owner.old_plugin_root.as_ref(),
        RetirementOutcome::New => Some(&owner.new_plugin_root),
        RetirementOutcome::AbortedPreSwitch => None,
    };
    let plugins_match = match outcome {
        RetirementOutcome::Old => {
            same_tree_content(terminal_plugin_root.as_ref(), expected_plugins)
        }
        RetirementOutcome::New => same_tree_object(terminal_plugin_root.as_ref(), expected_plugins),
        RetirementOutcome::AbortedPreSwitch => terminal_plugin_root.is_none(),
    };
    if !plugins_match {
        return Err(ImportError::InvalidManifest(
            "terminal Plugin evidence mismatch".into(),
        ));
    }
    let mut journal = RetirementJournal {
        schema_version: 1,
        role: "restore".into(),
        db_instance_operation_id: manifest.db_instance_operation_id.clone(),
        cleanup_operation_id: manifest.cleanup_operation_id.clone(),
        db_id: manifest.db_id.clone(),
        terminal_outcome: outcome_name.into(),
        live_basename: live_basename.into(),
        tombstone_basename,
        live_directory_identity,
        manifest_identity: manifest_evidence.identity,
        manifest_sha256: manifest_evidence.sha256,
        manifest_ownership_state: manifest.ownership_state.clone(),
        owner_identity: Some(owner_evidence.identity),
        owner_phase: Some(owner.phase.clone()),
        terminal_database,
        terminal_plugin_root,
        state: "prepared".into(),
    };
    publish_retirement_journal(
        registry,
        &journal_path,
        &journal_staging,
        &journal,
        true,
        crash_at,
    )?;
    maybe_inject_restore_crash(crash_at, "retirement_prepared_publish")?;
    publish_no_replace(&live, &tombstone).map_err(|error| ImportError::Io(error.to_string()))?;
    maybe_inject_restore_crash(crash_at, "live_to_tombstone_rename")?;
    sync_directory(registry)?;
    maybe_inject_restore_crash(crash_at, "live_to_tombstone_parent_fsync")?;
    let tombstone_metadata =
        fs::symlink_metadata(&tombstone).map_err(|error| ImportError::Io(error.to_string()))?;
    if !tombstone_metadata.is_dir()
        || tombstone_metadata.file_type().is_symlink()
        || stable_file_identity(&tombstone_metadata)? != journal.live_directory_identity
    {
        return Err(ImportError::InvalidManifest(
            "retirement tombstone identity mismatch".into(),
        ));
    }
    maybe_inject_restore_crash(crash_at, "live_to_tombstone_identity_verify")?;
    journal.state = "renamed".into();
    publish_retirement_journal(
        registry,
        &journal_path,
        &journal_staging,
        &journal,
        false,
        crash_at,
    )?;
    maybe_inject_restore_crash(crash_at, "retirement_renamed_publish")?;
    if crash_at.is_some_and(|point| point.as_str() == "tombstone_leaf_unlink") {
        remove_one_tree_leaf_no_follow(&tombstone, false)?;
        maybe_inject_restore_crash(crash_at, "tombstone_leaf_unlink")?;
    } else if crash_at.is_some_and(|point| point.as_str() == "tombstone_directory_fsync") {
        remove_one_tree_leaf_no_follow(&tombstone, true)?;
        maybe_inject_restore_crash(crash_at, "tombstone_directory_fsync")?;
    } else {
        remove_tree_no_follow_path(&tombstone)?;
        sync_directory(registry)?;
    }
    maybe_inject_restore_crash(crash_at, "tombstone_rmdir")?;
    journal.state = "done".into();
    publish_retirement_journal(
        registry,
        &journal_path,
        &journal_staging,
        &journal,
        false,
        crash_at,
    )?;
    maybe_inject_restore_crash(crash_at, "retirement_done_publish")?;
    maybe_inject_restore_crash(crash_at, "retirement_journal_delete")?;
    fs::remove_file(&journal_path).map_err(|error| ImportError::Io(error.to_string()))?;
    maybe_inject_restore_crash(crash_at, "retirement_journal_unlink")?;
    sync_directory(registry)?;
    maybe_inject_restore_crash(crash_at, "retirement_journal_unlink_parent_fsync")?;
    Ok(())
}

fn remove_one_tree_leaf_no_follow(root: &Path, sync_parent: bool) -> Result<(), ImportError> {
    let mut directories = vec![root.to_path_buf()];
    while let Some(directory) = directories.pop() {
        let mut entries = fs::read_dir(&directory)
            .map_err(|error| ImportError::Io(error.to_string()))?
            .collect::<Result<Vec<_>, _>>()
            .map_err(|error| ImportError::Io(error.to_string()))?;
        entries.sort_by_key(|entry| entry.file_name());
        for entry in entries {
            let path = entry.path();
            let metadata =
                fs::symlink_metadata(&path).map_err(|error| ImportError::Io(error.to_string()))?;
            if metadata.file_type().is_symlink() || !metadata.is_file() && !metadata.is_dir() {
                return Err(ImportError::UnsafeArchiveEntry(path.display().to_string()));
            }
            if metadata.is_dir() {
                directories.push(path);
                continue;
            }
            #[cfg(unix)]
            {
                use std::os::unix::fs::MetadataExt as _;
                if metadata.nlink() != 1 {
                    return Err(ImportError::UnsafeArchiveEntry(path.display().to_string()));
                }
            }
            fs::remove_file(&path).map_err(|error| ImportError::Io(error.to_string()))?;
            if sync_parent {
                sync_directory(&directory)?;
            }
            return Ok(());
        }
    }
    Err(ImportError::InvalidManifest(
        "retirement tombstone has no removable leaf".into(),
    ))
}

fn validate_owner_identity(
    manifest: &RestoreInstanceManifest,
    owner: &RestoreOwner,
    live_basename: &str,
    live: &Path,
) -> Result<(), ImportError> {
    let root = live
        .parent()
        .and_then(Path::parent)
        .ok_or_else(|| ImportError::InvalidManifest("live instance has no data root".into()))?;
    let live_relative = PathBuf::from(".hivegui-db-staging-v1").join(live_basename);
    let manifest_evidence = controlled_file_evidence(
        &live.join(".hivegui-db-instance-v1.json"),
        &live_relative.join(".hivegui-db-instance-v1.json"),
    )?;
    let expected_safety = PathBuf::from("backups").join(format!(
        "restore-safety-{}",
        manifest.db_instance_operation_id
    ));
    if owner.schema_version != 1
        || owner.role != "restore"
        || owner.db_instance_operation_id != manifest.db_instance_operation_id
        || owner.db_id != manifest.db_id
        || owner.instance_basename != live_basename
        || owner.ownership_state != "armed"
        || owner.manifest_identity != manifest_evidence.identity
        || owner.manifest_sha256 != manifest_evidence.sha256
        || owner.old_database.is_some() != owner.old_plugin_root.is_some()
        || owner
            .old_database
            .as_ref()
            .is_some_and(|database| database.relative_path != DATABASE_FILENAME)
        || owner
            .old_plugin_root
            .as_ref()
            .is_some_and(|plugins| plugins.relative_path != "plugins")
        || owner.new_database.relative_path
            != controlled_relative_path(&live_relative.join(DATABASE_FILENAME))?
        || owner.new_plugin_root.relative_path
            != controlled_relative_path(&live_relative.join("plugins"))?
        || owner.safety_snapshot.relative_path != controlled_relative_path(&expected_safety)?
        || !matches!(owner.phase.as_str(), "prepared" | "applying" | "committed")
        || !valid_file_evidence(owner.old_database.as_ref())
        || !valid_file_evidence(Some(&owner.new_database))
        || !valid_tree_evidence(owner.old_plugin_root.as_ref())
        || !valid_tree_evidence(Some(&owner.new_plugin_root))
        || !valid_tree_evidence(Some(&owner.safety_snapshot))
    {
        return Err(ImportError::InvalidManifest(
            "restore owner identity mismatch".into(),
        ));
    }
    if controlled_tree_evidence(&root.join(&expected_safety), &expected_safety)?
        != owner.safety_snapshot
    {
        return Err(ImportError::InvalidManifest(
            "restore safety snapshot evidence mismatch".into(),
        ));
    }
    Ok(())
}

fn validate_restore_instance_manifest(
    manifest: &RestoreInstanceManifest,
    live_basename: &str,
) -> Result<(), ImportError> {
    let expected_basename = format!("restore-{}", manifest.db_instance_operation_id);
    if manifest.schema_version != 1
        || manifest.role != "restore"
        || !matches!(manifest.ownership_state.as_str(), "unarmed" | "armed")
        || manifest.database_name != DATABASE_FILENAME
        || manifest.db_id != format!("restore/{}", manifest.db_instance_operation_id)
        || expected_basename != live_basename
        || manifest.cleanup_operation_id == manifest.db_instance_operation_id
        || !is_canonical_uuid(&manifest.db_instance_operation_id)
        || !is_canonical_uuid(&manifest.cleanup_operation_id)
    {
        return Err(ImportError::InvalidManifest(
            "restore instance identity mismatch".into(),
        ));
    }
    Ok(())
}

fn rollback_owner_to_old(
    live: &Path,
    current_database: &Path,
    current_plugins: &Path,
    old_database_sha256: Option<&str>,
) -> Result<(), ImportError> {
    let staging_database = live.join(DATABASE_FILENAME);
    let staging_plugins = live.join("plugins");
    let old_database = live.join("old-datasources.db");
    let old_plugins = live.join("old-plugins");
    if old_database_sha256.is_some() {
        if current_database.exists() && !staging_database.exists() {
            publish_no_replace(current_database, &staging_database)
                .map_err(|error| ImportError::Io(error.to_string()))?;
        }
        if old_database.exists() {
            publish_no_replace(&old_database, current_database)
                .map_err(|error| ImportError::Io(error.to_string()))?;
        }
        if current_plugins.exists() && !staging_plugins.exists() {
            publish_no_replace(current_plugins, &staging_plugins)
                .map_err(|error| ImportError::Io(error.to_string()))?;
        }
        if old_plugins.exists() {
            publish_no_replace(&old_plugins, current_plugins)
                .map_err(|error| ImportError::Io(error.to_string()))?;
        }
    } else {
        if current_database.exists() && !staging_database.exists() {
            publish_no_replace(current_database, &staging_database)
                .map_err(|error| ImportError::Io(error.to_string()))?;
        }
        if current_plugins.exists() && !staging_plugins.exists() {
            publish_no_replace(current_plugins, &staging_plugins)
                .map_err(|error| ImportError::Io(error.to_string()))?;
        }
    }
    sync_directory(live)?;
    sync_directory(current_database.parent().unwrap_or_else(|| Path::new(".")))
}

fn validate_unpublished_retirement_staging(
    registry: &Path,
    journal: &RetirementJournal,
) -> Result<(), ImportError> {
    let root = registry
        .parent()
        .ok_or_else(|| ImportError::InvalidManifest("registry has no data root".into()))?;
    let expected_live = format!("restore-{}", journal.db_instance_operation_id);
    let expected_tombstone = format!(
        ".hivegui-db-retired-v1-{}-{expected_live}",
        journal.terminal_outcome
    );
    if journal.schema_version != 1
        || journal.role != "restore"
        || journal.state != "prepared"
        || !is_canonical_uuid(&journal.db_instance_operation_id)
        || !is_canonical_uuid(&journal.cleanup_operation_id)
        || journal.cleanup_operation_id == journal.db_instance_operation_id
        || journal.db_id != format!("restore/{}", journal.db_instance_operation_id)
        || journal.live_basename != expected_live
        || journal.tombstone_basename != expected_tombstone
        || !matches!(
            journal.terminal_outcome.as_str(),
            "aborted_pre_switch" | "old" | "new"
        )
    {
        return Err(ImportError::InvalidManifest(
            "invalid unpublished retirement staging".into(),
        ));
    }
    let live = registry.join(&journal.live_basename);
    let tombstone = registry.join(&journal.tombstone_basename);
    if !live.is_dir() || tombstone.exists() {
        return Err(ImportError::InvalidManifest(
            "unpublished retirement paths are ambiguous".into(),
        ));
    }
    let live_metadata =
        fs::symlink_metadata(&live).map_err(|error| ImportError::Io(error.to_string()))?;
    if live_metadata.file_type().is_symlink()
        || stable_file_identity(&live_metadata)? != journal.live_directory_identity
    {
        return Err(ImportError::InvalidManifest(
            "unpublished retirement live identity mismatch".into(),
        ));
    }
    let current_database = optional_controlled_file_evidence(
        &root.join(DATABASE_FILENAME),
        Path::new(DATABASE_FILENAME),
    )?;
    let current_plugins =
        optional_controlled_tree_evidence(&root.join("plugins"), Path::new("plugins"))?;
    if current_database != journal.terminal_database
        || current_plugins != journal.terminal_plugin_root
    {
        return Err(ImportError::InvalidManifest(
            "unpublished retirement terminal evidence mismatch".into(),
        ));
    }
    let manifest = read_reconciled_instance_manifest(&live)?;
    match journal.terminal_outcome.as_str() {
        "aborted_pre_switch" => {
            if manifest.ownership_state != "unarmed"
                || journal.owner_identity.is_some()
                || journal.owner_phase.is_some()
                || live.join(".hivegui-db-recovery-v1.json").exists()
                || live.join(".hivegui-db-recovery-v1.json.staging").exists()
            {
                return Err(ImportError::InvalidManifest(
                    "unpublished aborted retirement owner mismatch".into(),
                ));
            }
        }
        "old" | "new" => {
            if manifest.ownership_state != "armed" {
                return Err(ImportError::InvalidManifest(
                    "unpublished terminal retirement manifest mismatch".into(),
                ));
            }
            let owner = read_reconciled_restore_owner(&live)?;
            validate_owner_identity(&manifest, &owner, &journal.live_basename, &live)?;
            let phase_matches = if journal.terminal_outcome == "old" {
                matches!(owner.phase.as_str(), "prepared" | "applying")
            } else {
                owner.phase == "committed"
            };
            let owner_evidence = controlled_file_evidence(
                &live.join(".hivegui-db-recovery-v1.json"),
                &PathBuf::from(".hivegui-db-staging-v1")
                    .join(&journal.live_basename)
                    .join(".hivegui-db-recovery-v1.json"),
            )?;
            if !phase_matches
                || journal.owner_phase.as_deref() != Some(owner.phase.as_str())
                || journal.owner_identity.as_deref() != Some(owner_evidence.identity.as_str())
            {
                return Err(ImportError::InvalidManifest(
                    "unpublished retirement owner evidence mismatch".into(),
                ));
            }
        }
        _ => unreachable!("validated retirement outcome"),
    }
    Ok(())
}

fn replay_existing_retirement_journals(
    registry: &Path,
) -> Result<Option<RetirementOutcome>, ImportError> {
    let mut journal_paths = Vec::new();
    let mut staging_paths = Vec::new();
    for entry in fs::read_dir(registry).map_err(|error| ImportError::Io(error.to_string()))? {
        let entry = entry.map_err(|error| ImportError::Io(error.to_string()))?;
        let name = entry
            .file_name()
            .to_str()
            .ok_or_else(|| ImportError::UnsafeArchiveEntry("non-ASCII registry entry".into()))?
            .to_string();
        if name.starts_with(".hivegui-db-retirement-v1-restore-") && name.ends_with(".json.staging")
        {
            staging_paths.push(entry.path());
        }
        if name.starts_with(".hivegui-db-retirement-v1-restore-") && name.ends_with(".json") {
            journal_paths.push(entry.path());
        }
    }
    staging_paths.sort();
    for staging_path in staging_paths {
        let staging_name = staging_path
            .file_name()
            .and_then(|name| name.to_str())
            .ok_or_else(|| {
                ImportError::UnsafeArchiveEntry("non-ASCII retirement staging".into())
            })?;
        let final_name = staging_name.strip_suffix(".staging").ok_or_else(|| {
            ImportError::InvalidManifest("invalid retirement staging name".into())
        })?;
        let final_path = registry.join(final_name);
        let staging_record: RetirementJournal =
            serde_json::from_slice(&read_control_file_nofollow(&staging_path)?)
                .map_err(|_| ImportError::InvalidManifest("invalid retirement staging".into()))?;
        if !final_path.exists() {
            validate_unpublished_retirement_staging(registry, &staging_record)?;
            fs::remove_file(&staging_path).map_err(|error| ImportError::Io(error.to_string()))?;
            sync_directory(registry)?;
            continue;
        }
        let final_record: RetirementJournal =
            serde_json::from_slice(&read_control_file_nofollow(&final_path)?)
                .map_err(|_| ImportError::InvalidManifest("invalid retirement journal".into()))?;
        let next_state = match final_record.state.as_str() {
            "prepared" => "renamed",
            "renamed" => "done",
            _ => {
                return Err(ImportError::InvalidManifest(
                    "ambiguous retirement journal update".into(),
                ));
            }
        };
        let mut expected = final_record;
        expected.state = next_state.into();
        if staging_record != expected {
            return Err(ImportError::InvalidManifest(
                "ambiguous retirement journal update".into(),
            ));
        }
        fs::remove_file(&staging_path).map_err(|error| ImportError::Io(error.to_string()))?;
        sync_directory(registry)?;
    }
    journal_paths.sort();
    let mut outcome = None;
    for journal_path in journal_paths {
        let replayed = replay_one_retirement_journal(registry, &journal_path)?;
        if outcome.replace(replayed).is_some() {
            return Err(ImportError::InvalidManifest(
                "multiple retirement operation groups".into(),
            ));
        }
    }
    Ok(outcome)
}

fn replay_one_retirement_journal(
    registry: &Path,
    journal_path: &Path,
) -> Result<RetirementOutcome, ImportError> {
    let root = registry
        .parent()
        .ok_or_else(|| ImportError::InvalidManifest("registry has no data root".into()))?;
    let mut journal: RetirementJournal =
        serde_json::from_slice(&read_control_file_nofollow(journal_path)?)
            .map_err(|_| ImportError::InvalidManifest("invalid retirement journal".into()))?;
    let expected_name = format!(
        ".hivegui-db-retirement-v1-restore-{}.json",
        journal.db_instance_operation_id
    );
    let expected_live = format!("restore-{}", journal.db_instance_operation_id);
    let expected_tombstone = format!(
        ".hivegui-db-retired-v1-{}-{expected_live}",
        journal.terminal_outcome
    );
    let owner_shape_valid = match journal.terminal_outcome.as_str() {
        "aborted_pre_switch" => {
            journal.manifest_ownership_state == "unarmed"
                && journal.owner_identity.is_none()
                && journal.owner_phase.is_none()
        }
        "old" => {
            journal.manifest_ownership_state == "armed"
                && journal
                    .owner_identity
                    .as_ref()
                    .is_some_and(|value| !value.is_empty())
                && journal
                    .owner_phase
                    .as_deref()
                    .is_some_and(|phase| matches!(phase, "prepared" | "applying"))
        }
        "new" => {
            journal.manifest_ownership_state == "armed"
                && journal
                    .owner_identity
                    .as_ref()
                    .is_some_and(|value| !value.is_empty())
                && journal.owner_phase.as_deref() == Some("committed")
        }
        _ => false,
    };
    if journal.schema_version != 1
        || journal.role != "restore"
        || !is_canonical_uuid(&journal.db_instance_operation_id)
        || !is_canonical_uuid(&journal.cleanup_operation_id)
        || journal.cleanup_operation_id == journal.db_instance_operation_id
        || journal.db_id != format!("restore/{}", journal.db_instance_operation_id)
        || journal_path.file_name().and_then(|name| name.to_str()) != Some(expected_name.as_str())
        || journal.live_basename != expected_live
        || journal.tombstone_basename != expected_tombstone
        || journal.live_directory_identity.is_empty()
        || journal.manifest_identity.is_empty()
        || !valid_lower_sha256(&journal.manifest_sha256)
        || !owner_shape_valid
        || journal
            .terminal_database
            .as_ref()
            .is_some_and(|database| database.relative_path != DATABASE_FILENAME)
        || journal
            .terminal_plugin_root
            .as_ref()
            .is_some_and(|plugins| plugins.relative_path != "plugins")
        || journal.terminal_database.is_some() != journal.terminal_plugin_root.is_some()
        || !valid_file_evidence(journal.terminal_database.as_ref())
        || !valid_tree_evidence(journal.terminal_plugin_root.as_ref())
        || !matches!(
            journal.terminal_outcome.as_str(),
            "aborted_pre_switch" | "old" | "new"
        )
    {
        return Err(ImportError::InvalidManifest(
            "retirement journal identity mismatch".into(),
        ));
    }
    let terminal_outcome = match journal.terminal_outcome.as_str() {
        "aborted_pre_switch" => RetirementOutcome::AbortedPreSwitch,
        "old" => RetirementOutcome::Old,
        "new" => RetirementOutcome::New,
        _ => unreachable!("validated terminal outcome"),
    };
    let live = registry.join(&journal.live_basename);
    let tombstone = registry.join(&journal.tombstone_basename);
    let staging = registry.join(format!("{expected_name}.staging"));
    let current_database = optional_controlled_file_evidence(
        &root.join(DATABASE_FILENAME),
        Path::new(DATABASE_FILENAME),
    )?;
    let current_plugins =
        optional_controlled_tree_evidence(&root.join("plugins"), Path::new("plugins"))?;
    if current_database != journal.terminal_database
        || current_plugins != journal.terminal_plugin_root
    {
        return Err(ImportError::InvalidManifest(
            "retirement terminal evidence mismatch".into(),
        ));
    }
    if staging.exists() {
        return Err(ImportError::InvalidManifest(
            "ambiguous retirement journal update".into(),
        ));
    }
    if journal.state == "prepared" {
        match (live.exists(), tombstone.exists()) {
            (true, false) => {
                let live_metadata = fs::symlink_metadata(&live)
                    .map_err(|error| ImportError::Io(error.to_string()))?;
                if !live_metadata.is_dir()
                    || live_metadata.file_type().is_symlink()
                    || stable_file_identity(&live_metadata)? != journal.live_directory_identity
                {
                    return Err(ImportError::InvalidManifest(
                        "prepared retirement live identity mismatch".into(),
                    ));
                }
                publish_no_replace(&live, &tombstone)
                    .map_err(|error| ImportError::Io(error.to_string()))?;
                sync_directory(registry)?;
            }
            (false, true) => sync_directory(registry)?,
            _ => {
                return Err(ImportError::InvalidManifest(
                    "prepared retirement path ambiguity".into(),
                ));
            }
        }
        journal.state = "renamed".into();
        publish_retirement_journal(registry, journal_path, &staging, &journal, false, None)?;
    }
    if journal.state == "renamed" {
        if live.exists() {
            return Err(ImportError::InvalidManifest(
                "renamed retirement still has live instance".into(),
            ));
        }
        if tombstone.exists() {
            let tombstone_metadata = fs::symlink_metadata(&tombstone)
                .map_err(|error| ImportError::Io(error.to_string()))?;
            if !tombstone_metadata.is_dir()
                || tombstone_metadata.file_type().is_symlink()
                || stable_file_identity(&tombstone_metadata)? != journal.live_directory_identity
            {
                return Err(ImportError::InvalidManifest(
                    "retirement tombstone identity mismatch".into(),
                ));
            }
            remove_tree_no_follow_path(&tombstone)?;
            sync_directory(registry)?;
        }
        journal.state = "done".into();
        publish_retirement_journal(registry, journal_path, &staging, &journal, false, None)?;
    }
    if journal.state == "done" {
        if live.exists() || tombstone.exists() {
            return Err(ImportError::InvalidManifest(
                "done retirement retains owned paths".into(),
            ));
        }
        fs::remove_file(journal_path).map_err(|error| ImportError::Io(error.to_string()))?;
        sync_directory(registry)?;
        return Ok(terminal_outcome);
    }
    Err(ImportError::InvalidManifest(
        "unknown retirement journal state".into(),
    ))
}

async fn replay_registry_retirements(
    root: &Path,
    registry: &Path,
) -> Result<RetirementOutcome, ImportError> {
    let replayed_outcome = replay_existing_retirement_journals(registry)?;
    let mut entries = fs::read_dir(registry)
        .map_err(|error| ImportError::Io(error.to_string()))?
        .collect::<Result<Vec<_>, _>>()
        .map_err(|error| ImportError::Io(error.to_string()))?;
    entries.sort_by_key(|entry| entry.file_name());
    let mut mapped = Vec::new();
    for entry in entries {
        let name = entry
            .file_name()
            .to_str()
            .ok_or_else(|| ImportError::UnsafeArchiveEntry("non-ASCII registry entry".into()))?
            .to_string();
        let Some(instance_id) = name.strip_prefix("restore-") else {
            return Err(ImportError::UnsafeArchiveEntry(format!(
                "unknown registry entry {name}"
            )));
        };
        let file_type = entry
            .file_type()
            .map_err(|error| ImportError::Io(error.to_string()))?;
        if !file_type.is_dir() || file_type.is_symlink() {
            return Err(ImportError::UnsafeArchiveEntry(
                entry.path().display().to_string(),
            ));
        }
        Uuid::parse_str(instance_id)
            .map_err(|_| ImportError::InvalidManifest("invalid restore instance name".into()))?;
        let live = registry.join(&name);
        let manifest = read_reconciled_instance_manifest(&live)?;
        validate_restore_instance_manifest(&manifest, &name)?;
        let owner = match manifest.ownership_state.as_str() {
            "unarmed" => {
                if live.join(".hivegui-db-recovery-v1.json").exists()
                    || live.join(".hivegui-db-recovery-v1.json.staging").exists()
                {
                    return Err(ImportError::StorageRecoveryBlocked {
                        reason: "sidecar_unknown_owner",
                        artifact: "wal",
                    });
                }
                None
            }
            "armed" => {
                let owner = read_reconciled_restore_owner(&live).map_err(|_| {
                    ImportError::StorageRecoveryBlocked {
                        reason: "sidecar_unknown_owner",
                        artifact: "wal",
                    }
                })?;
                validate_owner_identity(&manifest, &owner, &name, &live).map_err(|_| {
                    ImportError::StorageRecoveryBlocked {
                        reason: "sidecar_unknown_owner",
                        artifact: "wal",
                    }
                })?;
                Some(owner)
            }
            _ => unreachable!("validated restore ownership state"),
        };
        mapped.push((name, live, manifest, owner));
    }

    let current_database = root.join(DATABASE_FILENAME);
    if current_database.exists() {
        replay_sidecar_cleanup_for_database(&current_database, "current")?;
    }
    for (_, live, manifest, _) in &mapped {
        let live_database = live.join(DATABASE_FILENAME);
        if live_database.exists() {
            replay_sidecar_cleanup_for_database(&live_database, &manifest.db_id)?;
        }
    }

    let current_plugins = root.join("plugins");
    let mut outcome = replayed_outcome.unwrap_or(RetirementOutcome::AbortedPreSwitch);
    for (name, live, manifest, owner) in mapped {
        let Some(owner) = owner else {
            retire_unarmed_instance(registry, &name)?;
            outcome = RetirementOutcome::AbortedPreSwitch;
            continue;
        };
        match owner.phase.as_str() {
            "prepared" | "applying" => {
                rollback_owner_to_old(
                    &live,
                    &current_database,
                    &current_plugins,
                    owner
                        .old_database
                        .as_ref()
                        .map(|database| database.sha256.as_str()),
                )?;
                restore_missing_old_from_safety_snapshot(
                    root,
                    &current_database,
                    &current_plugins,
                    &owner,
                )?;
                if let Some(expected) = &owner.old_database {
                    validate_restore_database(&current_database, &current_plugins).await?;
                    let actual_database =
                        controlled_file_evidence(&current_database, Path::new(DATABASE_FILENAME))?;
                    let actual_plugins =
                        controlled_tree_evidence(&current_plugins, Path::new("plugins"))?;
                    if !same_file_content(Some(&actual_database), Some(expected))
                        || !same_tree_content(Some(&actual_plugins), owner.old_plugin_root.as_ref())
                    {
                        return Err(ImportError::InvalidManifest(
                            "restored old current identity mismatch".into(),
                        ));
                    }
                } else if current_database.exists() || current_plugins.exists() {
                    return Err(ImportError::InvalidManifest(
                        "expected absent old current".into(),
                    ));
                }
                retire_terminal_instance(
                    registry,
                    &name,
                    &manifest,
                    &owner,
                    RetirementOutcome::Old,
                )?;
                outcome = RetirementOutcome::Old;
            }
            "committed" => {
                validate_restore_database(&current_database, &current_plugins).await?;
                let actual_database =
                    controlled_file_evidence(&current_database, Path::new(DATABASE_FILENAME))?;
                let actual_plugins =
                    controlled_tree_evidence(&current_plugins, Path::new("plugins"))?;
                if !same_file_object(Some(&actual_database), Some(&owner.new_database))
                    || !same_tree_object(Some(&actual_plugins), Some(&owner.new_plugin_root))
                {
                    return Err(ImportError::InvalidManifest(
                        "committed new current identity mismatch".into(),
                    ));
                }
                retire_terminal_instance(
                    registry,
                    &name,
                    &manifest,
                    &owner,
                    RetirementOutcome::New,
                )?;
                outcome = RetirementOutcome::New;
            }
            _ => {
                return Err(ImportError::InvalidManifest(
                    "unknown restore owner phase".into(),
                ));
            }
        }
    }
    if fs::read_dir(registry)
        .map_err(|error| ImportError::Io(error.to_string()))?
        .next()
        .is_some()
    {
        return Err(ImportError::InvalidManifest(
            "registry not empty after replay".into(),
        ));
    }
    sync_directory(registry)?;
    Ok(outcome)
}

fn retire_unarmed_instance(registry: &Path, live_basename: &str) -> Result<(), ImportError> {
    let root = registry
        .parent()
        .ok_or_else(|| ImportError::InvalidManifest("registry has no data root".into()))?;
    let live = registry.join(live_basename);
    let metadata =
        fs::symlink_metadata(&live).map_err(|error| ImportError::Io(error.to_string()))?;
    if !metadata.is_dir() || metadata.file_type().is_symlink() {
        return Err(ImportError::UnsafeArchiveEntry(live.display().to_string()));
    }
    let live_directory_identity = stable_file_identity(&metadata)?;
    let manifest_path = live.join(".hivegui-db-instance-v1.json");
    let manifest_staging = live.join(".hivegui-db-instance-v1.json.staging");
    let owner = live.join(".hivegui-db-recovery-v1.json");
    let owner_staging = live.join(".hivegui-db-recovery-v1.json.staging");
    if manifest_staging.exists() || owner.exists() || owner_staging.exists() {
        return Err(ImportError::InvalidManifest(
            "ambiguous unarmed restore ownership".into(),
        ));
    }
    let manifest_bytes = read_control_file_nofollow(&manifest_path)?;
    let manifest: RestoreInstanceManifest = serde_json::from_slice(&manifest_bytes)
        .map_err(|_| ImportError::InvalidManifest("invalid restore instance manifest".into()))?;
    let expected_basename = format!("restore-{}", manifest.db_instance_operation_id);
    if manifest.schema_version != 1
        || manifest.role != "restore"
        || manifest.ownership_state != "unarmed"
        || manifest.database_name != DATABASE_FILENAME
        || manifest.db_id != format!("restore/{}", manifest.db_instance_operation_id)
        || expected_basename != live_basename
        || manifest.cleanup_operation_id == manifest.db_instance_operation_id
        || !is_canonical_uuid(&manifest.db_instance_operation_id)
        || !is_canonical_uuid(&manifest.cleanup_operation_id)
    {
        return Err(ImportError::InvalidManifest(
            "restore instance identity mismatch".into(),
        ));
    }
    let tombstone_basename = format!(".hivegui-db-retired-v1-aborted_pre_switch-{live_basename}");
    let journal_basename = format!(
        ".hivegui-db-retirement-v1-restore-{}.json",
        manifest.db_instance_operation_id
    );
    let journal_path = registry.join(&journal_basename);
    let journal_staging = registry.join(format!("{journal_basename}.staging"));
    if journal_path.exists()
        || journal_staging.exists()
        || registry.join(&tombstone_basename).exists()
    {
        return Err(ImportError::InvalidManifest(
            "duplicate restore retirement state".into(),
        ));
    }
    let live_relative = PathBuf::from(".hivegui-db-staging-v1").join(live_basename);
    let manifest_evidence = controlled_file_evidence(
        &manifest_path,
        &live_relative.join(".hivegui-db-instance-v1.json"),
    )?;
    let mut journal = RetirementJournal {
        schema_version: 1,
        role: "restore".into(),
        db_instance_operation_id: manifest.db_instance_operation_id.clone(),
        cleanup_operation_id: manifest.cleanup_operation_id,
        db_id: manifest.db_id,
        terminal_outcome: "aborted_pre_switch".into(),
        live_basename: live_basename.into(),
        tombstone_basename: tombstone_basename.clone(),
        live_directory_identity,
        manifest_identity: manifest_evidence.identity,
        manifest_sha256: manifest_evidence.sha256,
        manifest_ownership_state: manifest.ownership_state,
        owner_identity: None,
        owner_phase: None,
        terminal_database: optional_controlled_file_evidence(
            &root.join(DATABASE_FILENAME),
            Path::new(DATABASE_FILENAME),
        )?,
        terminal_plugin_root: optional_controlled_tree_evidence(
            &root.join("plugins"),
            Path::new("plugins"),
        )?,
        state: "prepared".into(),
    };
    publish_retirement_journal(
        registry,
        &journal_path,
        &journal_staging,
        &journal,
        true,
        None,
    )?;
    let tombstone = registry.join(&tombstone_basename);
    publish_no_replace(&live, &tombstone).map_err(|error| ImportError::Io(error.to_string()))?;
    sync_directory(registry)?;
    journal.state = "renamed".into();
    publish_retirement_journal(
        registry,
        &journal_path,
        &journal_staging,
        &journal,
        false,
        None,
    )?;
    remove_tree_no_follow_path(&tombstone)?;
    sync_directory(registry)?;
    journal.state = "done".into();
    publish_retirement_journal(
        registry,
        &journal_path,
        &journal_staging,
        &journal,
        false,
        None,
    )?;
    fs::remove_file(&journal_path).map_err(|error| ImportError::Io(error.to_string()))?;
    sync_directory(registry)
}

fn publish_retirement_journal(
    registry: &Path,
    final_path: &Path,
    staging_path: &Path,
    journal: &RetirementJournal,
    initial: bool,
    crash_at: Option<BackupCrashPoint>,
) -> Result<(), ImportError> {
    let bytes = serde_json::to_vec(journal)
        .map_err(|error| ImportError::InvalidManifest(error.to_string()))?;
    let boundaries = match journal.state.as_str() {
        "prepared" => [
            "retirement_prepared_staging_write",
            "retirement_prepared_staging_fsync",
            "retirement_prepared_rename",
            "retirement_prepared_parent_fsync",
        ],
        "renamed" => [
            "retirement_renamed_staging_write",
            "retirement_renamed_staging_fsync",
            "retirement_renamed_rename",
            "retirement_renamed_parent_fsync",
        ],
        "done" => [
            "retirement_done_staging_write",
            "retirement_done_staging_fsync",
            "retirement_done_rename",
            "retirement_done_parent_fsync",
        ],
        _ => {
            return Err(ImportError::InvalidManifest(
                "invalid retirement journal publish state".into(),
            ));
        }
    };
    write_restore_control_staging(staging_path, &bytes, crash_at, boundaries[0], boundaries[1])?;
    if initial {
        publish_no_replace(staging_path, final_path)
            .map_err(|error| ImportError::Io(error.to_string()))?;
    } else {
        fs::rename(staging_path, final_path).map_err(|error| ImportError::Io(error.to_string()))?;
    }
    maybe_inject_restore_crash(crash_at, boundaries[2])?;
    sync_directory(registry)?;
    maybe_inject_restore_crash(crash_at, boundaries[3])
}

fn remove_tree_no_follow_path(root: &Path) -> Result<(), ImportError> {
    let parent = root.parent().unwrap_or_else(|| Path::new("."));
    let name = Path::new(
        root.file_name()
            .ok_or_else(|| ImportError::UnsafeArchiveEntry(root.display().to_string()))?,
    );
    let parent_directory = cap_std::fs::Dir::open_ambient_dir(parent, cap_std::ambient_authority())
        .map_err(|error| ImportError::Io(error.to_string()))?;
    let metadata = parent_directory
        .symlink_metadata(name)
        .map_err(|error| ImportError::Io(error.to_string()))?;
    if !metadata.is_dir() || metadata.file_type().is_symlink() {
        return Err(ImportError::UnsafeArchiveEntry(root.display().to_string()));
    }
    let root_directory = parent_directory
        .open_dir(name)
        .map_err(|_| ImportError::UnsafeArchiveEntry(root.display().to_string()))?;
    remove_tree_no_follow(&root_directory)?;
    parent_directory
        .remove_dir(name)
        .map_err(|error| ImportError::Io(error.to_string()))?;
    sync_cap_directory(
        &parent_directory,
        "fsync removed descriptor-bound tree parent",
    )
}

fn remove_tree_no_follow(root_directory: &cap_std::fs::Dir) -> Result<(), ImportError> {
    let mut entries = root_directory
        .entries()
        .map_err(|error| ImportError::Io(error.to_string()))?
        .collect::<Result<Vec<_>, _>>()
        .map_err(|error| ImportError::Io(error.to_string()))?;
    entries.sort_by_key(|entry| entry.file_name());
    for entry in entries {
        let name = entry.file_name();
        let relative = Path::new(&name);
        let metadata = root_directory
            .symlink_metadata(relative)
            .map_err(|error| ImportError::Io(error.to_string()))?;
        if metadata.file_type().is_symlink() {
            return Err(ImportError::UnsafeArchiveEntry(
                relative.display().to_string(),
            ));
        }
        if metadata.is_dir() {
            let child = root_directory
                .open_dir(relative)
                .map_err(|_| ImportError::UnsafeArchiveEntry(relative.display().to_string()))?;
            remove_tree_no_follow(&child)?;
            root_directory
                .remove_dir(relative)
                .map_err(|error| ImportError::Io(error.to_string()))?;
            sync_cap_directory(root_directory, "fsync removed Plugin child")?;
        } else if metadata.is_file() {
            #[cfg(unix)]
            {
                use cap_std::fs::MetadataExt as _;
                if metadata.nlink() != 1 {
                    return Err(ImportError::UnsafeArchiveEntry(
                        relative.display().to_string(),
                    ));
                }
            }
            root_directory
                .remove_file(relative)
                .map_err(|error| ImportError::Io(error.to_string()))?;
            sync_cap_directory(root_directory, "fsync removed Plugin file")?;
        } else {
            return Err(ImportError::UnsafeArchiveEntry(
                relative.display().to_string(),
            ));
        }
    }
    sync_cap_directory(root_directory, "fsync cleared descriptor-bound tree")
}
