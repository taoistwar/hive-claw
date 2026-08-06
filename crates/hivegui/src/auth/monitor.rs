//! `AgentExecution` and `ChatSession` status enums (T-AUTH-3, SC-034).
//!
//! Idle lock must transition them to `Cancelled` / `Locked` so the
//! runtime doesn't continue executing with a sealed keystore.

use std::path::Path;

use thiserror::Error;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AgentExecutionStatus {
    Running,
    Cancelled,
    Completed,
    Failed,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ChatSessionStatus {
    Active,
    Locked,
    Archived,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SensitiveFileKind {
    SqliteMain,
    SqliteWal,
    SqliteShm,
    BackupStaging,
    DiagnosticsBundle,
}

impl SensitiveFileKind {
    /// Returns the canonical on-disk path for this file kind under
    /// `root` (e.g. `SqliteMain` → `<root>/datasources.db`). Used by
    /// `SensitiveCanaryScanner` to locate the byte stream to scan.
    pub fn path_under(self, root: &Path) -> std::path::PathBuf {
        match self {
            SensitiveFileKind::SqliteMain => root.join("datasources.db"),
            SensitiveFileKind::SqliteWal => root.join("datasources.db-wal"),
            SensitiveFileKind::SqliteShm => root.join("datasources.db-shm"),
            SensitiveFileKind::BackupStaging => root.join("backups/staging"),
            SensitiveFileKind::DiagnosticsBundle => root.join("diagnostics/bundle.zip"),
        }
    }
}

#[derive(Debug, Error)]
#[error("canary scan io error: {0}")]
pub struct CanaryScanError(#[from] std::io::Error);

pub struct SensitiveCanaryScanner<'r> {
    root: &'r Path,
}

impl<'r> SensitiveCanaryScanner<'r> {
    /// Construct a scanner rooted at `root`. The root is borrowed for
    /// the scanner's lifetime; callers must ensure it outlives the
    /// scanner.
    pub fn new(root: &'r Path) -> Self {
        Self { root }
    }

    /// Read the file for `kind` and return `1` if `canary` appears as a
    /// byte substring in the file's contents, otherwise `0`. Returns `0`
    /// on I/O errors (the file may legitimately be missing during early
    /// startup or after a destructive operation).
    pub fn scan_known_canary(&self, kind: SensitiveFileKind, canary: &str) -> usize {
        let path = kind.path_under(self.root);
        let Ok(bytes) = std::fs::read(&path) else {
            return 0;
        };
        // Plain-text canary detection: byte substring.
        if bytes.windows(canary.len()).any(|w| w == canary.as_bytes()) {
            1
        } else {
            0
        }
    }
}
