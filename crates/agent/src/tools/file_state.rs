//! Per-session file-read tracker used for read-before-edit warnings
//! and read deduplication. Port of `nanobot.agent.tools.file_state`.
//!
//! The original per-process global state has been replaced with a
//! `FileStates` instance that can be bound per async task via
//! `bind_file_states` / `reset_file_states`, mirroring Python's
//! `ContextVar` semantics.  Module-level convenience functions
//! (`record_read`, `check_read`, ...) delegate to the current task's
//! state, or fall back to a default instance for backward compat.

use std::collections::HashMap;
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::{SystemTime, UNIX_EPOCH};

use once_cell::sync::Lazy;
use sha2::{Digest, Sha256};

#[derive(Debug, Clone)]
pub struct ReadState {
    pub mtime: f64,
    pub offset: usize,
    pub limit: Option<usize>,
    pub content_hash: Option<String>,
    pub can_dedup: bool,
}

/// Per-session read/write tracker.
///
/// Owns its own state dict so read-dedup ("File unchanged since last read")
/// and read-before-edit warnings stay scoped to one agent session and do
/// not leak across sessions sharing this process.
pub struct FileStates {
    state: Mutex<HashMap<PathBuf, ReadState>>,
}

impl FileStates {
    pub fn new() -> Self {
        Self {
            state: Mutex::new(HashMap::new()),
        }
    }

    fn canonical(path: &Path) -> PathBuf {
        fs::canonicalize(path).unwrap_or_else(|_| path.to_path_buf())
    }

    fn file_mtime(path: &Path) -> Option<f64> {
        let meta = fs::metadata(path).ok()?;
        let mtime = meta.modified().ok()?;
        let dur = mtime.duration_since(UNIX_EPOCH).ok()?;
        Some(dur.as_secs_f64())
    }

    fn hash_file(path: &Path) -> Option<String> {
        let bytes = fs::read(path).ok()?;
        let mut hasher = Sha256::new();
        hasher.update(&bytes);
        Some(format!("{:x}", hasher.finalize()))
    }

    /// Record that a file was read (called after successful read).
    pub fn record_read<P: AsRef<Path>>(&self, path: P, offset: usize, limit: Option<usize>) {
        let p = Self::canonical(path.as_ref());
        let Some(mtime) = Self::file_mtime(&p) else {
            return;
        };
        let entry = ReadState {
            mtime,
            offset,
            limit,
            content_hash: Self::hash_file(&p),
            can_dedup: true,
        };
        self.state.lock().unwrap().insert(p, entry);
    }

    /// Record that a file was written (updates mtime in state).
    pub fn record_write<P: AsRef<Path>>(&self, path: P) {
        let p = Self::canonical(path.as_ref());
        let Some(mtime) = Self::file_mtime(&p) else {
            self.state.lock().unwrap().remove(&p);
            return;
        };
        let hash = Self::hash_file(&p);
        self.state.lock().unwrap().insert(
            p,
            ReadState {
                mtime,
                offset: 1,
                limit: None,
                content_hash: hash,
                can_dedup: false,
            },
        );
    }

    /// Check if a file has been read and is fresh.
    ///
    /// Returns `None` when OK, or a warning string.
    /// When mtime changed but file content is identical (e.g. touch, editor save),
    /// the check passes to avoid false-positive staleness warnings.
    pub fn check_read<P: AsRef<Path>>(&self, path: P) -> Option<String> {
        let p = Self::canonical(path.as_ref());
        let mut state = self.state.lock().unwrap();
        let Some(entry) = state.get_mut(&p).cloned() else {
            return Some(
                "Warning: file has not been read yet. Read it first to verify content before editing.".into(),
            );
        };
        let Some(current_mtime) = Self::file_mtime(&p) else {
            return None;
        };
        if (current_mtime - entry.mtime).abs() > f64::EPSILON {
            if let Some(h) = &entry.content_hash {
                if Self::hash_file(&p).as_ref() == Some(h) {
                    if let Some(e) = state.get_mut(&p) {
                        e.mtime = current_mtime;
                    }
                    return None;
                }
            }
            return Some(
                "Warning: file has been modified since last read. Re-read to verify content before editing.".into(),
            );
        }
        // mtime unchanged - still check content hash to detect quick modifications
        if let Some(h) = &entry.content_hash {
            if Self::hash_file(&p).as_ref() != Some(h) {
                return Some(
                    "Warning: file has been modified since last read. Re-read to verify content before editing.".into(),
                );
            }
        }
        None
    }

    /// Return `true` if file was previously read with same params and content is unchanged.
    pub fn is_unchanged<P: AsRef<Path>>(
        &self,
        path: P,
        offset: usize,
        limit: Option<usize>,
    ) -> bool {
        let p = Self::canonical(path.as_ref());
        let mut state = self.state.lock().unwrap();
        let Some(entry) = state.get_mut(&p) else {
            return false;
        };
        if !entry.can_dedup {
            return false;
        }
        if entry.offset != offset || entry.limit != limit {
            return false;
        }
        let Some(current_mtime) = Self::file_mtime(&p) else {
            return false;
        };
        if (current_mtime - entry.mtime).abs() > f64::EPSILON {
            let current_hash = Self::hash_file(&p);
            if current_hash != entry.content_hash {
                // Content actually changed - don't dedup
                entry.can_dedup = false;
                return false;
            }
            // Content identical despite mtime change (e.g. touch) - mark as not dedupable
            entry.can_dedup = false;
            return true;
        }
        // mtime unchanged - content must be identical
        true
    }

    /// Return the raw `ReadState` entry for a path, or `None`.
    pub fn get<P: AsRef<Path>>(&self, path: P) -> Option<ReadState> {
        let p = Self::canonical(path.as_ref());
        self.state.lock().unwrap().get(&p).cloned()
    }

    /// Clear all tracked state (useful for testing).
    pub fn clear(&self) {
        self.state.lock().unwrap().clear();
    }
}

impl Default for FileStates {
    fn default() -> Self {
        Self::new()
    }
}

/// Lookup table for per-session file read/write state.
///
/// Maps session keys to their own `FileStates` instances.
pub struct FileStateStore {
    states: Mutex<HashMap<String, Arc<FileStates>>>,
}

impl FileStateStore {
    pub fn new() -> Self {
        Self {
            states: Mutex::new(HashMap::new()),
        }
    }

    pub fn for_session(&self, session_key: Option<&str>) -> Arc<FileStates> {
        let key = session_key.unwrap_or("__default__");
        let mut states = self.states.lock().unwrap();
        states
            .entry(key.to_string())
            .or_insert_with(|| Arc::new(FileStates::new()))
            .clone()
    }

    pub fn clear(&self) {
        self.states.lock().unwrap().clear();
    }
}

impl Default for FileStateStore {
    fn default() -> Self {
        Self::new()
    }
}

// ---------------------------------------------------------------------------
// ContextVar simulation via thread-local for per-async-task binding.
// ---------------------------------------------------------------------------

use std::cell::RefCell;

thread_local! {
    static CURRENT_FILE_STATES: RefCell<Option<Arc<FileStates>>> = const { RefCell::new(None) };
}

/// Return the `FileStates` bound to the current async task, or a fallback.
pub fn current_file_states(fallback: &Arc<FileStates>) -> Arc<FileStates> {
    CURRENT_FILE_STATES.with(|cell| cell.borrow().clone().unwrap_or_else(|| fallback.clone()))
}

/// Bind `file_states` for the current async task. Returns a token for reset.
pub fn bind_file_states(states: Arc<FileStates>) {
    CURRENT_FILE_STATES.with(|cell| {
        *cell.borrow_mut() = Some(states);
    });
}

/// Reset the current task's binding (clear it).
pub fn reset_file_states() {
    CURRENT_FILE_STATES.with(|cell| {
        *cell.borrow_mut() = None;
    });
}

// ---------------------------------------------------------------------------
// Module-level default instance, retained for backward compatibility.
// ---------------------------------------------------------------------------

static DEFAULT: Lazy<FileStates> = Lazy::new(FileStates::new);

/// Record that a file was read (convenience, delegates to default).
pub fn record_read<P: AsRef<Path>>(path: P, offset: usize, limit: Option<usize>) {
    DEFAULT.record_read(path, offset, limit);
}

/// Record that a file was written (convenience, delegates to default).
pub fn record_write<P: AsRef<Path>>(path: P) {
    DEFAULT.record_write(path);
}

/// Check if a file has been read and is fresh (convenience, delegates to default).
pub fn check_read<P: AsRef<Path>>(path: P) -> Option<String> {
    DEFAULT.check_read(path)
}

/// `true` if the file was previously read with identical params and is unchanged.
pub fn is_unchanged<P: AsRef<Path>>(path: P, offset: usize, limit: Option<usize>) -> bool {
    DEFAULT.is_unchanged(path, offset, limit)
}

/// Return the canonicalized mtime and hash for a recorded file, if any.
pub fn touch_modified<P: AsRef<Path>>(path: P) {
    DEFAULT.record_read(&path, 1, None);
}

/// Clear all tracked state in the default instance (useful for tests).
pub fn clear() {
    DEFAULT.clear();
}

/// Compute the sha256 hex of *path*'s bytes if readable.
pub fn sha256_hex(path: &Path) -> Option<String> {
    FileStates::hash_file(path)
}

/// Convenience accessor for state-aware adapters needing the system time.
pub fn now_epoch() -> f64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs_f64())
        .unwrap_or_default()
}
