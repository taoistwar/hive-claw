//! Backup bundle and required-notice types (T-AUTH-4, FR-051 / SC-035).

use std::path::{Path, PathBuf};

use sha2::{Digest, Sha256};
use thiserror::Error;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BackupRequiredNotice {
    NoPriorBackup,
}

impl BackupRequiredNotice {
    /// Returns the underlying variant. The single-variant enum
    /// currently exposes `NoPriorBackup`; the indirection leaves room
    /// for future variants without changing the public surface.
    pub fn reason(&self) -> Self {
        *self
    }
}

#[derive(Debug, Clone)]
pub struct BackupBundle {
    pub export_path: PathBuf,
    pub wrapped_device_key: Vec<u8>,
    pub manifest_sha256: String,
}

impl BackupBundle {
    /// Returns the on-disk path of the exported backup file.
    pub fn export_path(&self) -> &Path {
        &self.export_path
    }

    /// Returns the hex-encoded SHA-256 manifest the backup file is
    /// expected to match. Used by `verify_sha256` and `read_and_verify`.
    pub fn manifest_sha256(&self) -> &str {
        &self.manifest_sha256
    }

    /// Verify that the SHA-256 of `contents` matches the bundle's stored
    /// `manifest_sha256`. Returns `true` iff the digests are equal byte-for-byte.
    pub fn verify_sha256(&self, contents: &[u8]) -> bool {
        let mut hasher = Sha256::new();
        hasher.update(contents);
        let digest = hasher.finalize();
        hex::encode(digest) == self.manifest_sha256
    }

    /// Read the bundle file from disk and verify its SHA-256 manifest.
    /// Returns `Err(TamperDetected)` if the digest mismatches; otherwise
    /// returns the raw bytes. I/O errors propagate as `Err(Io(_))`.
    pub fn read_and_verify(&self) -> Result<Vec<u8>, BackupError> {
        let bytes = std::fs::read(&self.export_path)?;
        if !self.verify_sha256(&bytes) {
            return Err(BackupError::TamperDetected);
        }
        Ok(bytes)
    }

    /// Returns `true` iff the in-memory `wrapped_device_key` is non-empty
    /// and starts with the `AUTHV1` magic. Used as a fast-path tamper
    /// signal before invoking the full AES-GCM unwrap.
    pub fn wrapped_device_key_matches(&self) -> bool {
        !self.wrapped_device_key.is_empty() && self.wrapped_device_key.starts_with(b"AUTHV1")
    }

    /// Test-only: overwrite both the in-memory `wrapped_device_key` and
    /// the on-disk bundle file with `bytes`. Used by the tamper-injection
    /// test to drive a fail-closed restore outcome.
    pub fn overwrite_wrapped_device_key_for_test(&mut self, bytes: Vec<u8>) {
        self.wrapped_device_key = bytes.clone();
        // Also overwrite the file on disk so the restore path
        // (which reads the file) sees the tampered bytes.
        if let Err(err) = std::fs::write(&self.export_path, &bytes) {
            eprintln!(
                "warning: failed to overwrite tampered bundle at {:?}: {err}",
                self.export_path
            );
        }
    }
}

#[derive(Debug, Error)]
pub enum BackupError {
    #[error("io error: {0}")]
    Io(#[from] std::io::Error),
    #[error("backup tamper detected")]
    TamperDetected,
}

#[derive(Debug, Clone)]
pub enum BackupExportOutcome {
    Exported { bundle: BackupBundle },
}

pub trait BackupExporter: Send + Sync {
    fn export(
        &self,
        workspace_root: &Path,
        password: &str,
    ) -> Result<BackupExportOutcome, BackupError>;
}

/// In-memory test exporter. Real exporter is T129.
pub struct TestBackupExporter {
    pub export_path: PathBuf,
}

impl BackupExporter for TestBackupExporter {
    fn export(
        &self,
        workspace_root: &Path,
        _password: &str,
    ) -> Result<BackupExportOutcome, BackupError> {
        use std::io::Write;
        let path = workspace_root.join(&self.export_path);
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)?;
        }

        // Pull the wrapped_device_key bytes from the workspace if any
        let wrapped_path = workspace_root.join("keystore/wrapped_device_key.v1");
        let wrapped_device_key = std::fs::read(&wrapped_path).unwrap_or_default();

        // Write the wrapped_device_key to the file as the bundle
        // payload. The restore path reads the file as the
        // wrapped_device_key; this is what makes the tamper-injection
        // test (`tampered_backup_wrapped_key_yields_fail_closed_on_new_device`)
        // able to drive a fail-closed outcome by overwriting the
        // file contents.
        let mut file = std::fs::File::create(&path)?;
        file.write_all(&wrapped_device_key)?;
        file.sync_all()?;
        let mut hasher = Sha256::new();
        hasher.update(&wrapped_device_key);
        let manifest = hex::encode(hasher.finalize());

        Ok(BackupExportOutcome::Exported {
            bundle: BackupBundle {
                export_path: path,
                wrapped_device_key,
                manifest_sha256: manifest,
            },
        })
    }
}
