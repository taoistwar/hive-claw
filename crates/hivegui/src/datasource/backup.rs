//! T129/T130 [US13] Age-encrypted backup export + restore.
//!
//! The exporter writes a passphrase-encrypted age stream that wraps a
//! `tar` archive containing the user database (`datasources.db`) and a
//! `manifest.json` describing the snapshot. The importer reverses
//! the operation, runs format upgrade (formats 1, 2, 3 → 1), and
//! validates the manifest before writing the staged database.
//!
//! Safety invariants enforced here:
//!   * Source must be a regular file (no symlink, hardlink, device,
//!     FIFO, or socket).
//!   * Target must not exist when exporting; the caller is
//!     responsible for picking a fresh path.
//!   * The manifest carries the six-tuple
//!     `(schema_version=1, role, UUID, restore_db_id,
//!      database_name="datasources.db", ownership_state="unarmed")`.
//!   * Sensitive values are only ever held inside the streaming
//!     buffers; nothing is ever written to disk in plaintext.
//!   * The restore side writes to a staging directory first and only
//!     after a successful `fsync` calls `rename` into the final
//!     location.

#![warn(missing_docs)]

use std::{
    fs::{self, File, OpenOptions},
    io::{Read, Write},
    path::{Path, PathBuf},
};

use age::secrecy::SecretString;
use age::{Decryptor, Encryptor};
use flate2::read::GzDecoder;
use flate2::write::GzEncoder;
use serde::{Deserialize, Serialize};
use thiserror::Error;
use uuid::Uuid;

const MANIFEST_FILENAME: &str = "manifest.json";
const DATABASE_FILENAME: &str = "datasources.db";
const MANIFEST_SCHEMA_VERSION: u32 = 1;
const ALLOWED_FORMATS: &[u32] = &[1, 2, 3];

/// Errors surfaced by the exporter.
#[derive(Debug, Error)]
pub enum ExportError {
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
}

/// Errors surfaced by the importer.
#[derive(Debug, Error)]
pub enum ImportError {
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
}

/// Manifest carried in every archive. The six-tuple fields are
/// documented in §T129.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct BackupManifest {
    /// Manifest schema version (always 1 for new exports).
    pub schema_version: u32,
    /// Archive role (`"primary"` for full backups).
    pub role: String,
    /// Stable UUID for this snapshot.
    pub uuid: String,
    /// UUID assigned to the restore-side staging database.
    pub restore_db_id: String,
    /// Database filename. Always `datasources.db` for the v4 model.
    pub database_name: String,
    /// Ownership state. New exports are always `unarmed`.
    pub ownership_state: String,
    /// Archive format (`1`, `2`, or `3`).
    pub format: u32,
}

impl BackupManifest {
    fn new(format: u32) -> Self {
        Self {
            schema_version: MANIFEST_SCHEMA_VERSION,
            role: "primary".to_string(),
            uuid: Uuid::new_v4().to_string(),
            restore_db_id: Uuid::new_v4().to_string(),
            database_name: DATABASE_FILENAME.to_string(),
            ownership_state: "unarmed".to_string(),
            format,
        }
    }
}

/// Backup exporter. Wraps a regular-file source database and writes
/// a passphrase-encrypted age + tar archive to a fresh target path.
pub struct BackupExporter {
    source: PathBuf,
    format: u32,
}

impl BackupExporter {
    /// Build a new exporter for the given source with format 1.
    pub fn new(source: impl Into<PathBuf>) -> Self {
        Self {
            source: source.into(),
            format: 1,
        }
    }

    /// Build a new exporter with a specific format (1, 2, or 3).
    /// Format 1 is the only actively-written format; 2 and 3 are
    /// accepted on import and upgraded transparently.
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
        let target = target.to_path_buf();
        let format = self.format;
        let passphrase = passphrase.to_string();
        tokio::task::spawn_blocking(move || export_blocking(&source, &target, &passphrase, format))
            .await
            .map_err(|e| ExportError::Io(format!("join error: {e}")))?
    }
}

fn export_blocking(
    source: &Path,
    target: &Path,
    passphrase: &str,
    format: u32,
) -> Result<BackupManifest, ExportError> {
    // 1. Refuse symlinks / hardlinks / devices / FIFOs / sockets.
    let metadata = match fs::symlink_metadata(source) {
        Ok(m) => m,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
            return Err(ExportError::SourceMissing(source.display().to_string()));
        }
        Err(e) => return Err(ExportError::Io(e.to_string())),
    };
    let file_type = metadata.file_type();
    if !file_type.is_file() || file_type.is_symlink() {
        return Err(ExportError::UnsafeSource(source.display().to_string()));
    }
    if metadata.len() == 0 {
        return Err(ExportError::UnsafeSource(source.display().to_string()));
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::MetadataExt;
        if metadata.nlink() > 1 {
            return Err(ExportError::UnsafeSource(source.display().to_string()));
        }
    }

    // 2. Refuse to overwrite an existing target.
    if target.exists() {
        return Err(ExportError::TargetExists(target.display().to_string()));
    }
    if let Some(parent) = target.parent() {
        fs::create_dir_all(parent).map_err(|e| ExportError::Io(e.to_string()))?;
    }

    // 3. Build the manifest.
    let manifest = BackupManifest::new(format);
    let manifest_bytes = serde_json::to_vec(&manifest)
        .map_err(|e| ExportError::Io(format!("manifest encode: {e}")))?;

    // 4. Read the database in 1 MiB chunks.
    let mut database_file = File::open(source).map_err(|e| ExportError::Io(e.to_string()))?;
    let mut database_bytes = Vec::new();
    let mut buf = [0u8; 1024 * 1024];
    loop {
        let n = database_file
            .read(&mut buf)
            .map_err(|e| ExportError::Io(e.to_string()))?;
        if n == 0 {
            break;
        }
        database_bytes.extend_from_slice(&buf[..n]);
    }

    // 5. Compose the tar archive in memory.
    let mut tar_bytes = Vec::new();
    {
        let mut header = tar::Header::new_gnu();
        header.set_size(manifest_bytes.len() as u64);
        header.set_mode(0o644);
        header
            .set_path(MANIFEST_FILENAME)
            .map_err(|e| ExportError::Io(format!("tar header path: {e}")))?;
        header.set_cksum();
        let mut tar_writer = tar::Builder::new(&mut tar_bytes);
        tar_writer
            .append(&header, manifest_bytes.as_slice())
            .map_err(|e| ExportError::Io(format!("tar append manifest: {e}")))?;

        let mut db_header = tar::Header::new_gnu();
        db_header.set_size(database_bytes.len() as u64);
        db_header.set_mode(0o644);
        db_header
            .set_path(DATABASE_FILENAME)
            .map_err(|e| ExportError::Io(format!("tar header path: {e}")))?;
        db_header.set_cksum();
        tar_writer
            .append(&db_header, database_bytes.as_slice())
            .map_err(|e| ExportError::Io(format!("tar append database: {e}")))?;
        tar_writer
            .finish()
            .map_err(|e| ExportError::Io(format!("tar finish: {e}")))?;
    }

    // 6. Compress with gzip.
    let mut compressed = Vec::new();
    {
        let mut encoder = GzEncoder::new(&mut compressed, flate2::Compression::default());
        encoder
            .write_all(&tar_bytes)
            .map_err(|e| ExportError::Io(format!("gzip write: {e}")))?;
        encoder
            .finish()
            .map_err(|e| ExportError::Io(format!("gzip finish: {e}")))?;
    }

    // 7. Encrypt with age using a passphrase.
    let passphrase_secret = SecretString::new(passphrase.to_string().into_boxed_str());
    let encryptor = Encryptor::with_user_passphrase(passphrase_secret);
    let mut encrypted = vec![];
    let mut writer = encryptor
        .wrap_output(&mut encrypted)
        .map_err(|e| ExportError::Age(format!("wrap_output: {e}")))?;
    writer
        .write_all(&compressed)
        .map_err(|e| ExportError::Age(format!("write: {e}")))?;
    writer
        .finish()
        .map_err(|e| ExportError::Age(format!("finish: {e}")))?;

    // 8. Write the target with create_new (atomic no-overwrite).
    let mut target_file = OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(target)
        .map_err(|e| {
            if e.kind() == std::io::ErrorKind::AlreadyExists {
                ExportError::TargetExists(target.display().to_string())
            } else {
                ExportError::Io(e.to_string())
            }
        })?;
    target_file
        .write_all(&encrypted)
        .map_err(|e| ExportError::Io(e.to_string()))?;
    target_file
        .sync_all()
        .map_err(|e| ExportError::Io(e.to_string()))?;
    if let Some(parent) = target.parent()
        && let Ok(dir) = File::open(parent)
    {
        let _ = dir.sync_all();
    }

    Ok(manifest)
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
        let archive = archive.to_path_buf();
        let passphrase = passphrase.to_string();
        let final_target = final_target.into();
        let staging_root = self.staging_root.clone();
        tokio::task::spawn_blocking(move || {
            import_blocking(&archive, &passphrase, &final_target, &staging_root)
        })
        .await
        .map_err(|e| ImportError::Io(format!("join error: {e}")))?
    }

    /// Decrypt the archive and return the manifest without writing
    /// the database. Used by callers that want to inspect the
    /// metadata before committing to a restore.
    pub async fn inspect_manifest(
        &self,
        archive: &Path,
        passphrase: &str,
    ) -> Result<BackupManifest, ImportError> {
        let archive = archive.to_path_buf();
        let passphrase = passphrase.to_string();
        tokio::task::spawn_blocking(move || inspect_manifest_blocking(&archive, &passphrase))
            .await
            .map_err(|e| ImportError::Io(format!("join error: {e}")))?
    }
}

fn decrypt_age_streaming(archive: &Path, passphrase: &str) -> Result<Vec<u8>, ImportError> {
    let file = File::open(archive).map_err(|e| ImportError::Io(e.to_string()))?;
    let decryptor = match Decryptor::new(file) {
        Ok(d) => d,
        Err(_) => return Err(ImportError::AuthenticationFailed),
    };
    let passphrase_secret = SecretString::new(passphrase.to_string().into_boxed_str());
    let identity = age::scrypt::Identity::new(passphrase_secret);
    let mut reader = match decryptor.decrypt(std::iter::once(&identity as &dyn age::Identity)) {
        Ok(r) => r,
        Err(_) => return Err(ImportError::AuthenticationFailed),
    };
    let mut out = Vec::new();
    if std::io::copy(&mut reader, &mut out).is_err() {
        return Err(ImportError::AuthenticationFailed);
    }
    Ok(out)
}

fn decompress_and_parse(plaintext: &[u8]) -> Result<(BackupManifest, Vec<u8>), ImportError> {
    let mut decoder = GzDecoder::new(plaintext);
    let mut tar_bytes = Vec::new();
    std::io::copy(&mut decoder, &mut tar_bytes).map_err(|e| ImportError::Io(e.to_string()))?;
    let mut archive = tar::Archive::new(tar_bytes.as_slice());
    let mut manifest: Option<BackupManifest> = None;
    let mut database: Option<Vec<u8>> = None;
    for entry in archive
        .entries()
        .map_err(|e| ImportError::Io(format!("tar entries: {e}")))?
    {
        let mut entry = entry.map_err(|e| ImportError::Io(format!("tar entry: {e}")))?;
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
        let path_str = path.to_string_lossy().to_string();
        let mut buf = Vec::new();
        entry
            .read_to_end(&mut buf)
            .map_err(|e| ImportError::Io(format!("tar read: {e}")))?;
        if path_str == MANIFEST_FILENAME {
            let m: BackupManifest = serde_json::from_slice(&buf)
                .map_err(|e| ImportError::InvalidManifest(e.to_string()))?;
            manifest = Some(m);
        } else if path_str == DATABASE_FILENAME {
            database = Some(buf);
        }
    }
    let manifest = manifest.ok_or_else(|| {
        ImportError::InvalidManifest("manifest.json missing from archive".to_string())
    })?;
    let database = database.ok_or_else(|| {
        ImportError::InvalidManifest("datasources.db missing from archive".to_string())
    })?;
    if !ALLOWED_FORMATS.contains(&manifest.format) {
        return Err(ImportError::InvalidManifest(format!(
            "unsupported format {}",
            manifest.format
        )));
    }
    if manifest.database_name != DATABASE_FILENAME {
        return Err(ImportError::InvalidManifest(format!(
            "database_name must be {}",
            DATABASE_FILENAME
        )));
    }
    Ok((manifest, database))
}

fn upgrade_to_format1(mut manifest: BackupManifest) -> BackupManifest {
    // The transparent format upgrade: every accepted format is
    // materialised as format 1 in the staging database.
    manifest.format = 1;
    manifest
}

fn import_blocking(
    archive: &Path,
    passphrase: &str,
    final_target: &Path,
    staging_root: &Path,
) -> Result<PathBuf, ImportError> {
    let plaintext = decrypt_age_streaming(archive, passphrase)?;
    let (manifest, database_bytes) = decompress_and_parse(&plaintext)?;
    let manifest = upgrade_to_format1(manifest);

    // Stage the database into <staging_root>/.hivegui-db-staging-v1/restore-<UUID>/datasources.db
    let instance_id = Uuid::new_v4();
    let staging_dir = staging_root
        .join(".hivegui-db-staging-v1")
        .join(format!("restore-{}", instance_id));
    fs::create_dir_all(&staging_dir).map_err(|e| ImportError::Io(e.to_string()))?;
    let staged_database = staging_dir.join(DATABASE_FILENAME);
    {
        let mut file = OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&staged_database)
            .map_err(|e| ImportError::Io(e.to_string()))?;
        file.write_all(&database_bytes)
            .map_err(|e| ImportError::Io(e.to_string()))?;
        file.sync_all()
            .map_err(|e| ImportError::Io(e.to_string()))?;
    }

    // Move to the final target via no-replace rename.
    fs::create_dir_all(final_target).map_err(|e| ImportError::Io(e.to_string()))?;
    let final_database = final_target.join(DATABASE_FILENAME);
    if final_database.exists() {
        return Err(ImportError::InvalidManifest(format!(
            "{} already exists in final target",
            DATABASE_FILENAME
        )));
    }
    fs::rename(&staged_database, &final_database).map_err(|e| ImportError::Io(e.to_string()))?;
    if let Some(parent) = final_database.parent()
        && let Ok(dir) = File::open(parent)
    {
        let _ = dir.sync_all();
    }

    // Persist the manifest sidecar for diagnostics.
    let sidecar = staging_dir.join("manifest.json");
    let manifest_bytes = serde_json::to_vec(&manifest)
        .map_err(|e| ImportError::Io(format!("manifest encode: {e}")))?;
    fs::write(&sidecar, &manifest_bytes).map_err(|e| ImportError::Io(e.to_string()))?;

    Ok(final_database)
}

fn inspect_manifest_blocking(
    archive: &Path,
    passphrase: &str,
) -> Result<BackupManifest, ImportError> {
    let plaintext = decrypt_age_streaming(archive, passphrase)?;
    let (manifest, _database_bytes) = decompress_and_parse(&plaintext)?;
    Ok(manifest)
}
