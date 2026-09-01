//! `AgentExecution` and `ChatSession` status enums (T-AUTH-3, SC-034).
//!
//! Idle lock must transition them to `Cancelled` / `Locked` so the
//! runtime doesn't continue executing with a sealed keystore.

use std::path::Path;

use thiserror::Error;

/// Lifecycle status of an agent execution, used by idle-lock to cancel
/// work when the keystore is sealed.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AgentExecutionStatus {
    /// Execution is actively running.
    Running,
    /// Execution was cancelled (e.g. by idle lock).
    Cancelled,
    /// Execution finished successfully.
    Completed,
    /// Execution terminated with an error.
    Failed,
}

/// Lifecycle status of a chat session relative to the keystore lock.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ChatSessionStatus {
    /// Session is active and unlocked.
    Active,
    /// Session was locked (keystore sealed); work must pause.
    Locked,
    /// Session has been archived.
    Archived,
}

/// Categories of sensitive on-disk files scanned by the canary checker.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SensitiveFileKind {
    /// Primary SQLite database file.
    SqliteMain,
    /// SQLite write-ahead log.
    SqliteWal,
    /// SQLite shared-memory file.
    SqliteShm,
    /// Backup staging directory.
    BackupStaging,
    /// Exported diagnostics bundle.
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

/// I/O error encountered while scanning a sensitive file for a canary.
#[derive(Debug, Error)]
#[error("canary scan io error: {0}")]
pub struct CanaryScanError(#[from] std::io::Error);

/// Scans sensitive on-disk files under a root directory for known canary
/// byte substrings, used to detect leaked plaintext.
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
