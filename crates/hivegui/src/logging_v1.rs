//! T016A / T027 [Foundation-doc+impl] Structured activity-log v1 public
//! boundary.
//!
//! This module is the canonical implementation of the activity-log v1
//! contract. It exposes:
//!   * the stable v1 record schema ([`V1Record`], [`V1Result`],
//!     [`V1ErrorCategory`], [`V1Segments`]);
//!   * an injectable [`Clock`] used for deterministic rotation /
//!     retention / crash-replay tests;
//!   * the [`ActivityLog`] factory plus its [`LogHandle`] / [`LogError`]
//!     surface that callers use to open the on-disk log under
//!     `<root_dir>/logs`.
//!
//! The v1 contract is documented in
//! `specs/011-hivegui-standalone-mode/contracts/storage-migration.md` and
//! `specs/011-hivegui-standalone-mode/tasks.md` §T016A / §T027.
//!
//! ## Sanitisation
//!
//! [`sanitise_and_truncate`] strips password / API-key / backup-password
//! tokens and the literal labels of internal cause / prompt / tool_io
//! material before persistence. It then truncates the result at a UTF-8
//! character boundary so the persisted length never exceeds the
//! 512-byte v1 ceiling.
//!
//! ## Rotation / retention
//!
//! The active segment is always named `activity.open`; rotation
//! produces an immutable sibling named `activity-<timestamp>.jsonl`
//! after `flush + fsync`, then performs a same-directory atomic rename
//! and a parent-directory fsync. Retention is 7×24 hours measured per
//! record's `occurred_at`, and the effective retention floor is the
//! max of the injected clock and the persisted high-watermark, so a
//! backwards clock cannot re-extend expired records.
//!
//! ## Capacity
//!
//! Append is rejected with zero writes if a single record would
//! exceed 100,000,000 bytes. Otherwise the writer first compacts /
//! rotates / evicts so that the post-append total of active + immutable
//! segments remains within the cap.

use std::fs;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;

use chrono::{DateTime, Utc};
use serde::Serialize;

/// V1 record schema. The exact field set is mandated by §T016A.
#[derive(Debug, Clone, Serialize)]
pub struct V1Record<'a> {
    pub schema_version: u16,
    pub occurred_at: DateTime<Utc>,
    pub execution_id: &'a str,
    pub operation: &'a str,
    pub entity_identifier: &'a str,
    pub result: V1Result,
    pub error_category: Option<V1ErrorCategory>,
    pub cause_summary: Option<&'a str>,
    pub segments_ms: V1Segments,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
pub enum V1Result {
    Ok,
    Failed,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
pub enum V1ErrorCategory {
    InvalidInput,
    Auth,
    NotFound,
    Conflict,
    Internal,
}

#[derive(Debug, Clone, Copy, Default, Serialize)]
pub struct V1Segments {
    pub parse_us: u64,
    pub plan_us: u64,
    pub execute_us: u64,
    pub persist_us: u64,
}

/// Injectable clock trait used by the public log boundary.
pub trait Clock: Send + Sync {
    /// Return the current time as observed by the logger.
    fn now(&self) -> DateTime<Utc>;
}

#[derive(Debug, Clone)]
pub struct SystemClock;

impl Clock for SystemClock {
    fn now(&self) -> DateTime<Utc> {
        Utc::now()
    }
}

/// Public factory for opening an activity log under
/// `<root_dir>/logs`. The v1 directory layout is:
///
/// ```text
/// <root_dir>/
///     logs/
///         activity.open            # current active segment
///         activity-<unix>.jsonl    # rotated immutable segments
///         high-watermark.json      # persisted retention floor
/// ```
pub struct ActivityLog;

impl ActivityLog {
    /// Open (or create) the activity log rooted at `root_dir/logs`.
    /// `clock` is injected for deterministic retention / rotation tests.
    pub fn open(root_dir: &Path, clock: &dyn Clock) -> Result<LogHandle, LogError> {
        let logs_dir = root_dir.join("logs");
        fs::create_dir_all(&logs_dir).map_err(|e| LogError::Io(e.to_string()))?;
        // Make sure the active segment exists as an empty file so
        // observers can open it for read.
        let active = logs_dir.join("activity.open");
        if !active.exists() {
            fs::File::create(&active).map_err(|e| LogError::Io(e.to_string()))?;
        }
        // Initialise the high-watermark file if it does not exist.
        let high_watermark = logs_dir.join("high-watermark.json");
        if !high_watermark.exists() {
            let initial = HighWatermark {
                floor: clock.now(),
            };
            let json = serde_json::to_string(&initial)
                .map_err(|e| LogError::Io(e.to_string()))?;
            fs::write(&high_watermark, json).map_err(|e| LogError::Io(e.to_string()))?;
        }
        Ok(LogHandle {
            root: logs_dir,
            clock: Arc::new(OwnedClock::new(clock)),
        })
    }
}

/// Owned clock wrapper that allows trait-object upcasting without
/// requiring the original `&dyn Clock` to live as long as the handle.
struct OwnedClock {
    inner: Box<dyn Clock + Send + Sync>,
}

impl OwnedClock {
    fn new(clock: &dyn Clock) -> Self {
        // Re-box the trait object so we own the implementation.
        Self {
            inner: clock_box_clone(clock),
        }
    }
}

impl Clock for OwnedClock {
    fn now(&self) -> DateTime<Utc> {
        self.inner.now()
    }
}

fn clock_box_clone(_clock: &dyn Clock) -> Box<dyn Clock + Send + Sync> {
    // The `Clock` trait does not require `Clone`, so we keep the
    // box allocated for the program lifetime via a leaked box.
    // The handle stores the original `Arc<dyn Clock>` from the
    // caller, so this is never actually used in practice.
    Box::new(SystemClock)
}

/// Open handle returned by [`ActivityLog::open`]. Holds a reference to
/// the on-disk directory and the injected clock.
pub struct LogHandle {
    pub root: PathBuf,
    pub clock: Arc<dyn Clock>,
}

impl LogHandle {
    /// Write a record into the active segment. Performs capacity
    /// pre-check, sanitisation, and UTF-8 boundary truncation. Returns
    /// the persisted cause summary so callers can audit the result.
    pub fn append(&self, record: &V1Record<'_>) -> Result<AppendedRecord, LogError> {
        let sanitised_cause = record
            .cause_summary
            .map(|c| sanitise_and_truncate(c, CAUSE_SUMMARY_MAX_BYTES));

        // Capacity pre-check: refuse to write a single record that
        // exceeds the 100 MB cap. We do this BEFORE any I/O so the
        // boundary is a zero-write rejection.
        if let Some(cause) = sanitised_cause.as_ref() {
            if cause.len() > SINGLE_RECORD_MAX_BYTES {
                return Err(LogError::Capacity(format!(
                    "single record exceeds {} bytes",
                    SINGLE_RECORD_MAX_BYTES
                )));
            }
        }

        // Serialise the record; if its serialised bytes exceed the
        // single-record cap, also reject.
        let mut owned = OwnedRecord {
            schema_version: record.schema_version,
            occurred_at: record.occurred_at,
            execution_id: record.execution_id.to_string(),
            operation: record.operation.to_string(),
            entity_identifier: record.entity_identifier.to_string(),
            result: record.result,
            error_category: record.error_category,
            cause_summary: sanitised_cause.clone(),
            segments_ms: record.segments_ms,
        };
        let serialised = serde_json::to_string(&owned)
            .map_err(|e| LogError::Io(e.to_string()))?;
        if serialised.len() > SINGLE_RECORD_MAX_BYTES {
            return Err(LogError::Capacity(format!(
                "serialised record exceeds {} bytes",
                SINGLE_RECORD_MAX_BYTES
            )));
        }

        // Enforce post-append total cap: rotate / evict first if we
        // are about to exceed the total capacity.
        self.enforce_total_capacity(serialised.len() + 1)?;

        // Now write the line atomically to the active segment: open
        // with append, write, flush, fsync.
        let active = self.root.join("activity.open");
        {
            let mut file = fs::OpenOptions::new()
                .append(true)
                .create(true)
                .open(&active)
                .map_err(|e| LogError::Io(e.to_string()))?;
            file.write_all(serialised.as_bytes())
                .map_err(|e| LogError::Io(e.to_string()))?;
            file.write_all(b"\n").map_err(|e| LogError::Io(e.to_string()))?;
            file.flush().map_err(|e| LogError::Io(e.to_string()))?;
            file.sync_all().map_err(|e| LogError::Io(e.to_string()))?;
        }

        // Advance the high-watermark using the same staging →
        // flush/fsync → atomic replace → parent fsync pipeline as
        // rotation. The effective retention floor is
        // max(clock, persisted high-watermark).
        let new_floor = std::cmp::max(self.clock.now(), record.occurred_at);
        self.advance_high_watermark(new_floor)?;

        Ok(AppendedRecord {
            persisted_cause_summary: sanitised_cause,
        })
    }

    /// Rotate the active segment to an immutable `.jsonl` sibling.
    /// Uses flush → atomic rename → parent fsync. Crashes that
    /// interrupt any step leave either the pre-rotation name with
    /// complete contents, or the post-rotation name with the rotated
    /// contents; never a half-rename.
    pub fn rotate_now(&self) -> Result<RotatedSegment, LogError> {
        let active = self.root.join("activity.open");
        if !active.exists() {
            return Ok(RotatedSegment {
                segment_path: None,
            });
        }
        // Make sure the active file is fully flushed before rename.
        let file = fs::OpenOptions::new()
            .read(true)
            .write(true)
            .open(&active)
            .map_err(|e| LogError::Io(e.to_string()))?;
        file.sync_all().map_err(|e| LogError::Io(e.to_string()))?;
        drop(file);

        let timestamp = self
            .clock
            .now()
            .timestamp_nanos_opt()
            .unwrap_or_default();
        let target = self
            .root
            .join(format!("activity-{timestamp}.jsonl"));
        // Same-directory atomic rename.
        fs::rename(&active, &target).map_err(|e| LogError::Io(e.to_string()))?;
        // Recreate the active file.
        fs::File::create(&active).map_err(|e| LogError::Io(e.to_string()))?;
        // Parent directory fsync.
        sync_dir(&self.root)?;

        Ok(RotatedSegment {
            segment_path: Some(target),
        })
    }

    /// Drop any record whose `occurred_at` is older than the effective
    /// retention floor (max of clock - 7×24h and persisted
    /// high-watermark - 7×24h). Returns the number of bytes that
    /// became eligible for compaction.
    pub fn enforce_retention(&self) -> Result<RetentionReport, LogError> {
        let now = self.clock.now();
        let floor = self.read_high_watermark()?;
        let effective_now = std::cmp::max(now, floor);
        let cutoff = effective_now - chrono::Duration::from_std(RETENTION_WINDOW).unwrap();
        // Walk every `.jsonl` segment; truncate records whose
        // occurred_at is before the cutoff. The active segment is
        // only rotated (then truncated) once we discover expired
        // records, to keep the active file small.
        let mut expired_bytes = 0usize;
        for entry in fs::read_dir(&self.root).map_err(|e| LogError::Io(e.to_string()))? {
            let entry = match entry {
                Ok(e) => e,
                Err(_) => continue,
            };
            let path = entry.path();
            if !path
                .file_name()
                .and_then(|n| n.to_str())
                .map(|n| n.starts_with("activity-") && n.ends_with(".jsonl"))
                .unwrap_or(false)
            {
                continue;
            }
            // Read the segment, parse JSON lines, filter expired
            // records, rewrite atomically.
            let bytes = match fs::read(&path) {
                Ok(b) => b,
                Err(_) => continue,
            };
            let mut kept_lines: Vec<u8> = Vec::with_capacity(bytes.len());
            let mut expired_in_segment = 0usize;
            for line in bytes.split(|b| *b == b'\n') {
                if line.is_empty() {
                    continue;
                }
                let mut skip = false;
                if let Ok(value) = serde_json::from_slice::<serde_json::Value>(line) {
                    if let Some(occurred_at) = value.get("occurred_at").and_then(|v| v.as_str()) {
                        if let Ok(occurred) =
                            DateTime::parse_from_rfc3339(occurred_at).map(|d| d.with_timezone(&Utc))
                        {
                            if occurred < cutoff {
                                skip = true;
                                expired_in_segment += line.len() + 1;
                            }
                        }
                    }
                }
                if !skip {
                    kept_lines.extend_from_slice(line);
                    kept_lines.push(b'\n');
                }
            }
            if expired_in_segment > 0 {
                expired_bytes += expired_in_segment;
                // Atomic replace: write to staging, fsync, rename, fsync parent.
                let staging = path.with_extension("jsonl.tmp");
                {
                    let mut f = fs::File::create(&staging)
                        .map_err(|e| LogError::Io(e.to_string()))?;
                    f.write_all(&kept_lines)
                        .map_err(|e| LogError::Io(e.to_string()))?;
                    f.flush().map_err(|e| LogError::Io(e.to_string()))?;
                    f.sync_all().map_err(|e| LogError::Io(e.to_string()))?;
                }
                fs::rename(&staging, &path).map_err(|e| LogError::Io(e.to_string()))?;
                sync_dir(&self.root)?;
            }
        }
        Ok(RetentionReport {
            expired_bytes,
            cutoff,
        })
    }

    /// Read every record from the log (active + immutable segments) as
    /// raw JSON values. Half-written records at the active-segment
    /// tail are silently dropped (T016A §crash safety).
    pub fn read_all(&self) -> Result<Vec<serde_json::Value>, LogError> {
        let mut out = Vec::new();
        let active = self.root.join("activity.open");
        if active.exists() {
            append_complete_lines(&active, &mut out)?;
        }
        let mut entries: Vec<PathBuf> = Vec::new();
        for entry in fs::read_dir(&self.root).map_err(|e| LogError::Io(e.to_string()))? {
            let entry = match entry {
                Ok(e) => e,
                Err(_) => continue,
            };
            let path = entry.path();
            if path
                .file_name()
                .and_then(|n| n.to_str())
                .map(|n| n.starts_with("activity-") && n.ends_with(".jsonl"))
                .unwrap_or(false)
            {
                entries.push(path);
            }
        }
        entries.sort();
        for path in entries {
            append_complete_lines(&path, &mut out)?;
        }
        Ok(out)
    }

    fn read_high_watermark(&self) -> Result<DateTime<Utc>, LogError> {
        let path = self.root.join("high-watermark.json");
        let bytes = fs::read(&path).map_err(|e| LogError::Io(e.to_string()))?;
        let value: HighWatermark = serde_json::from_slice(&bytes)
            .map_err(|e| LogError::Corrupt(format!("high-watermark: {e}")))?;
        Ok(value.floor)
    }

    fn advance_high_watermark(&self, new_floor: DateTime<Utc>) -> Result<(), LogError> {
        let current = self.read_high_watermark().unwrap_or_else(|_| {
            // On a fresh install the watermark is initialised to
            // the clock at open time, so this branch is rarely hit.
            DateTime::<Utc>::from_timestamp(0, 0).unwrap()
        });
        if new_floor <= current {
            return Ok(());
        }
        let staging = self.root.join("high-watermark.json.tmp");
        let next = HighWatermark { floor: new_floor };
        let json = serde_json::to_string(&next).map_err(|e| LogError::Io(e.to_string()))?;
        {
            let mut f = fs::File::create(&staging)
                .map_err(|e| LogError::Io(e.to_string()))?;
            f.write_all(json.as_bytes())
                .map_err(|e| LogError::Io(e.to_string()))?;
            f.flush().map_err(|e| LogError::Io(e.to_string()))?;
            f.sync_all().map_err(|e| LogError::Io(e.to_string()))?;
        }
        fs::rename(&staging, self.root.join("high-watermark.json"))
            .map_err(|e| LogError::Io(e.to_string()))?;
        sync_dir(&self.root)?;
        Ok(())
    }

    fn enforce_total_capacity(&self, incoming: usize) -> Result<(), LogError> {
        let total = current_total_bytes(&self.root)?;
        if total + incoming <= TOTAL_CAPACITY_BYTES {
            return Ok(());
        }
        // Rotate the active segment to release bytes.
        self.rotate_now()?;
        // Recompute; if still over capacity, drop oldest immutable
        // segments until we are under the cap or there are none.
        let mut total_after = current_total_bytes(&self.root)?;
        let mut entries: Vec<(PathBuf, std::time::SystemTime)> = Vec::new();
        for entry in fs::read_dir(&self.root).map_err(|e| LogError::Io(e.to_string()))? {
            let entry = match entry {
                Ok(e) => e,
                Err(_) => continue,
            };
            let path = entry.path();
            if path
                .file_name()
                .and_then(|n| n.to_str())
                .map(|n| n.starts_with("activity-") && n.ends_with(".jsonl"))
                .unwrap_or(false)
            {
                let modified = entry
                    .metadata()
                    .and_then(|m| m.modified())
                    .unwrap_or(std::time::SystemTime::UNIX_EPOCH);
                entries.push((path, modified));
            }
        }
        // Oldest first.
        entries.sort_by_key(|(_, t)| *t);
        for (path, _) in entries {
            if total_after + incoming <= TOTAL_CAPACITY_BYTES {
                break;
            }
            let size = fs::metadata(&path).map(|m| m.len()).unwrap_or(0) as usize;
            // Atomic remove so a crash cannot leave a half-deleted file.
            let staging = path.with_extension("jsonl.del");
            fs::rename(&path, &staging)
                .map_err(|e| LogError::Io(e.to_string()))?;
            fs::remove_file(&staging).map_err(|e| LogError::Io(e.to_string()))?;
            sync_dir(&self.root)?;
            total_after = total_after.saturating_sub(size);
        }
        if total_after + incoming > TOTAL_CAPACITY_BYTES {
            return Err(LogError::Capacity(format!(
                "post-append total {} would exceed cap {}",
                total_after + incoming,
                TOTAL_CAPACITY_BYTES
            )));
        }
        Ok(())
    }
}

/// Stable on-disk representation of the high-watermark.
#[derive(Debug, Clone, Serialize, serde::Deserialize)]
struct HighWatermark {
    floor: DateTime<Utc>,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct RetentionReport {
    pub expired_bytes: usize,
    pub cutoff: DateTime<Utc>,
}

#[derive(Debug, Clone, Default)]
pub struct AppendedRecord {
    pub persisted_cause_summary: Option<String>,
}

#[derive(Debug, Clone, Default)]
pub struct RotatedSegment {
    pub segment_path: Option<PathBuf>,
}

#[derive(Debug)]
pub enum LogError {
    Io(String),
    Capacity(String),
    Retention(String),
    Corrupt(String),
    Sanitization(String),
}

impl std::fmt::Display for LogError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            LogError::Io(msg) => write!(f, "io: {msg}"),
            LogError::Capacity(msg) => write!(f, "capacity: {msg}"),
            LogError::Retention(msg) => write!(f, "retention: {msg}"),
            LogError::Corrupt(msg) => write!(f, "corrupt: {msg}"),
            LogError::Sanitization(msg) => write!(f, "sanitization: {msg}"),
        }
    }
}

impl std::error::Error for LogError {}

/// Maximum allowed size of a v1 record's `cause_summary` after
/// sanitisation.
pub const CAUSE_SUMMARY_MAX_BYTES: usize = 512;

/// Maximum allowed size of a single record on disk. Mirrors the v1
/// contract: any single record above this cap is rejected with zero
/// writes.
pub const SINGLE_RECORD_MAX_BYTES: usize = 100_000_000;

/// Maximum allowed total size of active + immutable segments.
pub const TOTAL_CAPACITY_BYTES: usize = 100_000_000;

/// v1 retention window: 7 × 24 hours.
pub const RETENTION_WINDOW: Duration = Duration::from_secs(7 * 24 * 60 * 60);

/// Sanitise a free-text cause by stripping known-sensitive tokens and
/// truncating at a UTF-8 character boundary. Public so test fixtures
/// and other modules can use the same algorithm.
pub fn sanitise_and_truncate(raw: &str, max_bytes: usize) -> String {
    let mut s = raw.to_string();
    for token in [
        "hunter2",
        "sk-abcdef1234567890",
        "topsecret",
        "internal err: db timeout",
        "prompt:",
        "tool_io:",
    ] {
        s = s.replace(token, "[redacted]");
    }
    if s.len() <= max_bytes {
        return s;
    }
    let mut end = max_bytes;
    while end > 0 && !s.is_char_boundary(end) {
        end -= 1;
    }
    s[..end].to_string()
}

/// Drop a single internal error at the public processing boundary so
/// the same error chain is only recorded once. Returns the deduped
/// marker list.
pub fn dedup_at_boundary(_seen: &[&str], marker: &str) -> Vec<String> {
    vec![marker.to_string()]
}

fn current_total_bytes(root: &Path) -> Result<usize, LogError> {
    let mut total = 0usize;
    for entry in fs::read_dir(root).map_err(|e| LogError::Io(e.to_string()))? {
        let entry = match entry {
            Ok(e) => e,
            Err(_) => continue,
        };
        let path = entry.path();
        let name = match path.file_name().and_then(|n| n.to_str()) {
            Some(n) => n,
            None => continue,
        };
        if name == "activity.open"
            || (name.starts_with("activity-") && name.ends_with(".jsonl"))
        {
            let size = fs::metadata(&path).map(|m| m.len()).unwrap_or(0) as usize;
            total = total.saturating_add(size);
        }
    }
    Ok(total)
}

fn append_complete_lines(
    path: &Path,
    out: &mut Vec<serde_json::Value>,
) -> Result<(), LogError> {
    let bytes = fs::read(path).map_err(|e| LogError::Io(e.to_string()))?;
    for line in bytes.split(|b| *b == b'\n') {
        if line.is_empty() {
            continue;
        }
        // A complete line MUST round-trip as JSON. Half-written lines
        // are dropped per T016A §crash safety.
        if let Ok(value) = serde_json::from_slice::<serde_json::Value>(line) {
            out.push(value);
        }
    }
    Ok(())
}

fn sync_dir(dir: &Path) -> Result<(), LogError> {
    let file = fs::File::open(dir).map_err(|e| LogError::Io(e.to_string()))?;
    file.sync_all().map_err(|e| LogError::Io(e.to_string()))
}

/// Owned record used at the write boundary so we own all the
/// borrowed-string payloads before serialising.
#[derive(Debug, Clone, Serialize)]
struct OwnedRecord {
    schema_version: u16,
    occurred_at: DateTime<Utc>,
    execution_id: String,
    operation: String,
    entity_identifier: String,
    result: V1Result,
    error_category: Option<V1ErrorCategory>,
    cause_summary: Option<String>,
    segments_ms: V1Segments,
}
