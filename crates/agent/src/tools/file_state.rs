//! Per-process file-read tracker used for read-before-edit warnings
//! and read deduplication. Port of `nanobot.agent.tools.file_state`.

use std::collections::HashMap;
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::Mutex;
use std::time::{SystemTime, UNIX_EPOCH};

use once_cell::sync::Lazy;
use sha2::{Digest, Sha256};

#[derive(Debug, Clone)]
struct ReadState {
    mtime: f64,
    offset: usize,
    limit: Option<usize>,
    content_hash: Option<String>,
    can_dedup: bool,
}

static STATE: Lazy<Mutex<HashMap<PathBuf, ReadState>>> =
    Lazy::new(|| Mutex::new(HashMap::new()));

fn canonical(path: &Path) -> PathBuf {
    std::fs::canonicalize(path).unwrap_or_else(|_| path.to_path_buf())
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

/// Record that a file was read (call after successful read).
pub fn record_read<P: AsRef<Path>>(path: P, offset: usize, limit: Option<usize>) {
    let p = canonical(path.as_ref());
    let Some(mtime) = file_mtime(&p) else {
        return;
    };
    let entry = ReadState {
        mtime,
        offset,
        limit,
        content_hash: hash_file(&p),
        can_dedup: true,
    };
    STATE.lock().unwrap().insert(p, entry);
}

/// Record that a file was written (updates mtime in state).
pub fn record_write<P: AsRef<Path>>(path: P) {
    let p = canonical(path.as_ref());
    let Some(mtime) = file_mtime(&p) else {
        STATE.lock().unwrap().remove(&p);
        return;
    };
    let hash = hash_file(&p);
    STATE.lock().unwrap().insert(
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
pub fn check_read<P: AsRef<Path>>(path: P) -> Option<String> {
    let p = canonical(path.as_ref());
    let mut state = STATE.lock().unwrap();
    let Some(entry) = state.get_mut(&p).cloned() else {
        return Some(
            "Warning: file has not been read yet. Read it first to verify content before editing.".into(),
        );
    };
    let Some(current_mtime) = file_mtime(&p) else {
        return None;
    };
    if (current_mtime - entry.mtime).abs() > f64::EPSILON {
        if let Some(h) = &entry.content_hash {
            if hash_file(&p).as_ref() == Some(h) {
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
    if let Some(h) = &entry.content_hash {
        if hash_file(&p).as_ref() != Some(h) {
            return Some(
                "Warning: file has been modified since last read. Re-read to verify content before editing.".into(),
            );
        }
    }
    None
}

/// `true` if the file was previously read with identical params and is unchanged.
pub fn is_unchanged<P: AsRef<Path>>(path: P, offset: usize, limit: Option<usize>) -> bool {
    let p = canonical(path.as_ref());
    let mut state = STATE.lock().unwrap();
    let Some(entry) = state.get_mut(&p) else {
        return false;
    };
    if !entry.can_dedup {
        return false;
    }
    if entry.offset != offset || entry.limit != limit {
        return false;
    }
    let Some(current_mtime) = file_mtime(&p) else {
        return false;
    };
    if (current_mtime - entry.mtime).abs() > f64::EPSILON {
        let current_hash = hash_file(&p);
        if current_hash != entry.content_hash {
            entry.can_dedup = false;
            return false;
        }
        entry.can_dedup = false;
        return true;
    }
    true
}

/// Return the canonicalized mtime and hash for a recorded file, if any.
pub fn touch_modified<P: AsRef<Path>>(path: P) {
    let p = canonical(path.as_ref());
    if let Some(mtime) = file_mtime(&p) {
        if let Some(entry) = STATE.lock().unwrap().get_mut(&p) {
            entry.mtime = mtime;
            entry.content_hash = hash_file(&p);
            entry.can_dedup = false;
        }
    }
}

/// Clear all tracked state (useful for tests).
pub fn clear() {
    STATE.lock().unwrap().clear();
}

/// Compute the sha256 hex of *path*'s bytes if readable.
pub fn sha256_hex(path: &Path) -> Option<String> {
    hash_file(path)
}

/// Convenience accessor for state-aware adapters needing the system time.
pub fn now_epoch() -> f64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs_f64())
        .unwrap_or_default()
}
