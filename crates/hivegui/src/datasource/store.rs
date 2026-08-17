use anyhow::Result;
use chrono::{DateTime, Utc};
use sqlx::{
    Pool, Row, Sqlite,
    sqlite::{SqliteConnectOptions, SqlitePoolOptions},
};
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::str::FromStr;
use std::sync::Arc;
use thiserror::Error;

use super::crypto::Crypto;
use super::models::DataSource;
use super::query_count::QueryCountObserver;

const DB_FILENAME: &str = "datasources.db";
const KEY_SIZE: usize = 32;

/// In-process owner registry. Tracks the canonical database
/// path → owner-id of every live [`Store`] in this process. The
/// registry is consulted by [`Store::open_local`] and enforces
/// the "one process owns the local store write lock" contract
/// (T012). The corresponding entry is removed when the Store
/// is dropped.
static STORE_OWNERS: std::sync::OnceLock<std::sync::Mutex<HashMap<PathBuf, u64>>> =
    std::sync::OnceLock::new();

static NEXT_OWNER_ID: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(1);

/// Path of the inter-process advisory lock sidecar used by
/// [`Store::open_local`]. The sidecar lives next to the
/// database file; the OS-level `flock(2)` call is the
/// authoritative owner. On non-unix platforms the sidecar is
/// only used as a marker file.
fn lock_sidecar_path(database_path: &Path) -> PathBuf {
    let stem = database_path
        .file_name()
        .and_then(|name| name.to_str())
        .unwrap_or("datasources.db");
    let with_lock_suffix = format!("{stem}.lock");
    match database_path.parent() {
        Some(parent) => parent.join(with_lock_suffix),
        None => PathBuf::from(with_lock_suffix),
    }
}

/// Inter-process advisory lock implemented with `flock(2)` on
/// unix. The `Store` owns a `File` for the duration of the
/// open; dropping the `Store` releases the lock.
struct ProcessLock {
    _file: std::fs::File,
}

impl std::fmt::Debug for ProcessLock {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("ProcessLock")
            .finish_non_exhaustive()
    }
}

impl ProcessLock {
    /// Acquire an exclusive non-blocking lock on the lock file.
    /// Returns `Ok(Some(_))` when the lock is granted,
    /// `Ok(None)` when another process already holds it.
    fn try_acquire(path: &Path) -> std::io::Result<Option<Self>> {
        let file = std::fs::OpenOptions::new()
            .read(true)
            .write(true)
            .create(true)
            .truncate(false)
            .open(path)?;
        #[cfg(unix)]
        {
            use std::os::unix::io::AsRawFd;
            let fd = file.as_raw_fd();
            // LOCK_EX = 2, LOCK_NB = 4. Inline the constants to
            // avoid pulling a `libc` dependency.
            let operation: i32 = 2 | 4;
            let result = libc_flock(fd, operation);
            if result == 0 {
                Ok(Some(ProcessLock { _file: file }))
            } else {
                let errno = unsafe { *libc_errno_location() };
                // EWOULDBLOCK == EAGAIN == 11 on linux.
                if errno == 11 {
                    Ok(None)
                } else {
                    Err(std::io::Error::from_raw_os_error(errno))
                }
            }
        }
        #[cfg(not(unix))]
        {
            Ok(Some(ProcessLock { _file: file }))
        }
    }
}

#[cfg(unix)]
unsafe extern "C" {
    fn flock(fd: i32, operation: i32) -> i32;
    fn __errno_location() -> *mut i32;
}

#[cfg(unix)]
fn libc_flock(fd: i32, operation: i32) -> i32 {
    unsafe { flock(fd, operation) }
}

#[cfg(unix)]
fn libc_errno_location() -> *mut i32 {
    unsafe { __errno_location() }
}

/// In-process write-lock registry. Keyed by canonical database
/// path; each entry holds a `tokio::sync::Mutex` that every
/// concurrent [`Store::open_local`] call must acquire. The lock
/// is released when the [`Store`] is dropped.
static STORE_LOCKS: std::sync::OnceLock<
    std::sync::Mutex<HashMap<PathBuf, Arc<tokio::sync::Mutex<()>>>>,
> = std::sync::OnceLock::new();

fn lock_for_path(path: &Path) -> Arc<tokio::sync::Mutex<()>> {
    let canonical = std::fs::canonicalize(path).unwrap_or_else(|_| path.to_path_buf());
    let registry = STORE_LOCKS.get_or_init(|| std::sync::Mutex::new(HashMap::new()));
    let mut guard = registry.lock().expect("store lock registry poisoned");
    guard
        .entry(canonical)
        .or_insert_with(|| Arc::new(tokio::sync::Mutex::new(())))
        .clone()
}

fn register_store_owner(database_path: &Path) -> u64 {
    let canonical =
        std::fs::canonicalize(database_path).unwrap_or_else(|_| database_path.to_path_buf());
    let registry = STORE_OWNERS.get_or_init(|| std::sync::Mutex::new(HashMap::new()));
    let mut guard = registry.lock().expect("store owner registry poisoned");
    let id = NEXT_OWNER_ID.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
    guard.insert(canonical, id);
    id
}

fn unregister_store_owner(database_path: &Path) {
    let canonical =
        std::fs::canonicalize(database_path).unwrap_or_else(|_| database_path.to_path_buf());
    if let Some(registry) = STORE_OWNERS.get() {
        let mut guard = registry.lock().expect("store owner registry poisoned");
        guard.remove(&canonical);
    }
}

fn store_already_owned(database_path: &Path) -> bool {
    let canonical =
        std::fs::canonicalize(database_path).unwrap_or_else(|_| database_path.to_path_buf());
    if let Some(registry) = STORE_OWNERS.get() {
        let guard = registry.lock().expect("store owner registry poisoned");
        guard.contains_key(&canonical)
    } else {
        false
    }
}

/// Open-time configuration for [`Store::open_local`]. Mirrors the
/// shape that integration tests use to drive the storage layer in
/// isolation (one database file, one plugin artifact root).
#[derive(Debug, Clone)]
pub struct StoreOpenOptions {
    /// Path to the SQLite database file. The parent directory is
    /// created when missing.
    pub database_path: PathBuf,
    /// Directory used to materialise plugin artifacts (manifests,
    /// sha256 sidecars, etc.).
    pub plugin_root: PathBuf,
    /// Optional query-count observer wired into the new store.
    observer: Option<QueryCountObserver>,
    /// Optional fault injector for open-time failures.
    fault_injector: Option<Arc<dyn StoreOpenFaultInjector>>,
    /// Retry policy for the open call.
    retry_policy: Option<RetryPolicy>,
    /// Retry sleeper used by the open call.
    retry_sleeper: Option<Arc<dyn RetrySleeper>>,
}

impl Default for StoreOpenOptions {
    fn default() -> Self {
        Self {
            database_path: PathBuf::new(),
            plugin_root: PathBuf::new(),
            observer: None,
            fault_injector: None,
            retry_policy: None,
            retry_sleeper: None,
        }
    }
}

impl StoreOpenOptions {
    /// Build a new options bundle from a database path and plugin
    /// artifact root. Both paths are stored verbatim; directory
    /// creation is performed by [`Store::open_local`].
    pub fn new<P1: Into<PathBuf>, P2: Into<PathBuf>>(database_path: P1, plugin_root: P2) -> Self {
        Self {
            database_path: database_path.into(),
            plugin_root: plugin_root.into(),
            observer: None,
            fault_injector: None,
            retry_policy: None,
            retry_sleeper: None,
        }
    }

    /// Attach a query-count observer. The observer is cloned into
    /// the new store so callers can share it across tests.
    pub fn with_query_count_observer(mut self, observer: QueryCountObserver) -> Self {
        self.observer = Some(observer);
        self
    }

    /// Attach a fault injector for open-time failures.
    pub fn with_open_fault_injector(mut self, injector: Arc<dyn StoreOpenFaultInjector>) -> Self {
        self.fault_injector = Some(injector);
        self
    }

    /// Configure the retry policy used by the open call.
    pub fn with_retry_policy(mut self, policy: RetryPolicy) -> Self {
        self.retry_policy = Some(policy);
        self
    }

    /// Configure the retry sleeper used by the open call.
    pub fn with_retry_sleeper(mut self, sleeper: Arc<dyn RetrySleeper>) -> Self {
        self.retry_sleeper = Some(sleeper);
        self
    }

    /// Returns the configured observer, if any.
    pub fn query_count_observer(&self) -> Option<&QueryCountObserver> {
        self.observer.as_ref()
    }

    /// Returns the configured fault injector, if any.
    pub fn open_fault_injector(&self) -> Option<&Arc<dyn StoreOpenFaultInjector>> {
        self.fault_injector.as_ref()
    }

    /// Returns the configured retry policy.
    pub fn retry_policy(&self) -> RetryPolicy {
        self.retry_policy.unwrap_or_default()
    }

    /// Returns the configured retry sleeper.
    pub fn retry_sleeper(&self) -> Option<&Arc<dyn RetrySleeper>> {
        self.retry_sleeper.as_ref()
    }
}

/// Error kinds returned by [`Store::open_local`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum StoreOpenErrorKind {
    /// The database file is held by another live owner.
    AlreadyLocked,
    /// The local database is corrupt; recovery requires an explicit
    /// user decision (restore backup, rebuild, exit, ...).
    CorruptDatabase,
    /// A recovery path requires explicit user confirmation before it
    /// proceeds (e.g. rebuild of a corrupt local database).
    ConfirmationRequired,
    /// Schema drift detected on open (missing column / table / CHECK).
    SchemaDrift,
    /// The committed high-watermark has not been advanced yet.
    CommittedHighWatermarkMissing,
    /// Underlying I/O error.
    Io,
}

/// Stable Store-open error envelope.
#[derive(Debug, Clone)]
pub struct StoreOpenError {
    kind: StoreOpenErrorKind,
    database_path: Option<PathBuf>,
    failure_class: Option<DatabaseFailureClass>,
    retry_count: usize,
}

impl StoreOpenError {
    /// Build a new open error.
    pub fn new(kind: StoreOpenErrorKind, database_path: Option<PathBuf>) -> Self {
        Self {
            kind,
            database_path,
            failure_class: None,
            retry_count: 0,
        }
    }

    /// Attach a failure class describing the underlying cause.
    pub fn with_failure_class(mut self, class: DatabaseFailureClass) -> Self {
        self.failure_class = Some(class);
        self
    }

    /// Attach a retry counter (number of retries that ran before
    /// the open call gave up).
    pub fn with_retry_count(mut self, retry_count: usize) -> Self {
        self.retry_count = retry_count;
        self
    }

    /// Returns the error kind.
    pub fn kind(&self) -> StoreOpenErrorKind {
        self.kind.clone()
    }

    /// Returns the database path, when known.
    pub fn database_path(&self) -> Option<&Path> {
        self.database_path.as_deref()
    }

    /// Returns the failure class, when known.
    pub fn failure_class(&self) -> Option<DatabaseFailureClass> {
        self.failure_class
    }

    /// Returns the number of retries that ran before the open call
    /// gave up.
    pub fn retry_count(&self) -> usize {
        self.retry_count
    }
}

impl std::fmt::Display for StoreOpenError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        // Redact the absolute path to its file name so the
        // error message never leaks the workspace root or any
        // other host-specific directory tree.
        let path_label = self
            .database_path
            .as_ref()
            .and_then(|path| path.file_name())
            .and_then(|name| name.to_str())
            .unwrap_or("<redacted>");
        match &self.database_path {
            Some(_) => write!(
                formatter,
                "store open error: {:?} (file: {})",
                self.kind, path_label
            ),
            None => write!(formatter, "store open error: {:?}", self.kind),
        }
    }
}

impl std::error::Error for StoreOpenError {}

/// Decision returned by [`StoreOpenFaultInjector`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CorruptionDecision {
    /// Caller must reject the open and surface the error.
    Reject,
    /// Caller must repair the corruption and re-open.
    Repair,
    /// Caller must quarantine the affected file and continue.
    Quarantine,
    /// Caller must rebuild the local database from a known-good
    /// fixture.
    RebuildLocalDatabase,
    /// Caller must request a fresh device key.
    RequestDeviceKeyRotation,
    /// Caller must restore the database from a previously written
    /// backup.
    RestoreBackup,
    /// Caller must give up and surface the error to the user.
    Exit,
}

/// Classification of a database failure surfaced during open.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DatabaseFailureClass {
    /// Transient I/O error (lock contention, disk full, etc.).
    Transient,
    /// Persistent I/O error (file missing, path inaccessible).
    Persistent,
    /// Logical corruption (bad page, missing row, FK violation).
    Corruption,
    /// Another process holds the database lock.
    DatabaseLocked,
    /// File is busy (rename, fsync, antivirus scan).
    FileBusy,
    /// Logical corruption (alias of [`DatabaseFailureClass::Corruption`]).
    Corrupt,
    /// Permission denied (file mode, SELinux, etc.).
    PermissionDenied,
    /// Schema is not understood by the application.
    UnsupportedSchema,
    /// Caller supplied an invalid input.
    InvalidInput,
    /// `PRAGMA integrity_check` failed.
    IntegrityCheckFailed,
}

/// Retry policy attached to one open call.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct RetryPolicy {
    /// Maximum number of attempts (including the first one).
    pub max_attempts: usize,
    /// Base backoff between attempts.
    pub backoff: std::time::Duration,
}

impl Default for RetryPolicy {
    fn default() -> Self {
        Self {
            max_attempts: 4,
            backoff: std::time::Duration::from_secs(1),
        }
    }
}

impl RetryPolicy {
    /// The Foundation-default retry policy: 4 total attempts (1
    /// initial + 3 retries) using the exact 1s / 2s / 4s backoff
    /// ladder for [`DatabaseFailureClass::DatabaseLocked`] and
    /// [`DatabaseFailureClass::FileBusy`]. Every other failure
    /// class is non-retryable and surfaces immediately on the
    /// first attempt.
    pub fn foundation_default() -> Self {
        Self::default()
    }

    /// Number of retry attempts (excluding the initial attempt).
    pub fn maximum_retries(&self) -> usize {
        self.max_attempts.saturating_sub(1)
    }

    /// Total number of attempts (initial + retries).
    pub fn maximum_attempts(&self) -> usize {
        self.max_attempts
    }

    /// Returns the exact backoff ladder for `class`. Non-retryable
    /// classes return an empty slice.
    pub fn delays(&self, class: DatabaseFailureClass) -> Vec<std::time::Duration> {
        match class {
            DatabaseFailureClass::DatabaseLocked | DatabaseFailureClass::FileBusy => vec![
                std::time::Duration::from_secs(1),
                std::time::Duration::from_secs(2),
                std::time::Duration::from_secs(4),
            ],
            _ => Vec::new(),
        }
    }

    /// Returns the sum of [`Self::delays`] for one retryable
    /// failure class. The Foundation policy only retries
    /// [`DatabaseFailureClass::DatabaseLocked`] /
    /// [`DatabaseFailureClass::FileBusy`]; both classes share
    /// the same ladder, so the sum is computed once.
    pub fn total_delay(&self) -> std::time::Duration {
        let mut total = std::time::Duration::ZERO;
        for delay in self.delays(DatabaseFailureClass::DatabaseLocked) {
            total += delay;
        }
        total
    }

    /// Always `false`. The Foundation retry policy never retries
    /// after a successful open; the test contract pins this
    /// behaviour to avoid hidden extra attempts.
    pub fn retries_after_success(&self) -> bool {
        false
    }
}

/// Sleep policy used by the open retry loop.
pub trait RetrySleeper: std::fmt::Debug {
    /// Sleep for `delay`. Returns a future that resolves when the
    /// sleep completes.
    fn sleep(
        &self,
        delay: std::time::Duration,
    ) -> std::pin::Pin<Box<dyn std::future::Future<Output = ()> + Send + '_>>;
}

/// Default [`RetrySleeper`] that delegates to `tokio::time::sleep`.
#[derive(Debug, Clone, Copy, Default)]
pub struct TokioRetrySleeper;

impl RetrySleeper for TokioRetrySleeper {
    fn sleep(
        &self,
        delay: std::time::Duration,
    ) -> std::pin::Pin<Box<dyn std::future::Future<Output = ()> + Send + '_>> {
        Box::pin(async move {
            tokio::time::sleep(delay).await;
        })
    }
}

/// Test seam for forcing open-time failures at a known point.
pub trait StoreOpenFaultInjector: std::fmt::Debug {
    /// Hook called before each open attempt. Implementations may
    /// return a [`DatabaseFailureClass`] to force the open path
    /// to surface a typed failure; returning `None` lets the
    /// normal open path run.
    fn before_open_attempt(&self, attempt: usize) -> Option<DatabaseFailureClass>;
}

/// `true` when the file at `path` looks like a SQLite database
/// (header is `SQLite format 3\0`).
fn is_sqlite_file(path: &Path) -> bool {
    match std::fs::read(path) {
        Ok(bytes) if bytes.len() >= 16 => bytes.starts_with(b"SQLite format 3"),
        Ok(_) => false,
        Err(_) => false,
    }
}

/// Copy the corrupt database to a quarantined location and return
/// the destination path. The quarantine file preserves the
/// original bytes; the caller is responsible for replacing or
/// rebuilding the active database.
async fn quarantine_corrupt_database(database_path: &Path) -> Result<PathBuf, StoreOpenError> {
    let parent = database_path.parent().unwrap_or_else(|| Path::new("."));
    let file_name = database_path
        .file_name()
        .and_then(|name| name.to_str())
        .unwrap_or("datasources.db");
    let counter = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|duration| duration.as_nanos())
        .unwrap_or(0);
    let quarantine_dir = parent.join("quarantine");
    std::fs::create_dir_all(&quarantine_dir).map_err(|_error| {
        StoreOpenError::new(StoreOpenErrorKind::Io, Some(database_path.to_path_buf()))
            .with_failure_class(DatabaseFailureClass::Persistent)
    })?;
    let destination = quarantine_dir.join(format!("{file_name}.corrupt-{counter}"));
    std::fs::copy(database_path, &destination).map_err(|_error| {
        StoreOpenError::new(StoreOpenErrorKind::Io, Some(database_path.to_path_buf()))
            .with_failure_class(DatabaseFailureClass::Persistent)
    })?;
    Ok(destination)
}

/// Read-only classification of the database file at startup time.
/// The store uses this to decide whether to create a fresh
/// database, refuse the open, or surface a dedicated corruption
/// gate. The classification is intentionally side-effect free: the
/// caller is expected to inspect the gate, then call
/// [`Store::open_local`] or [`Store::recover_corrupt_database`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum StoreStartupGate {
    /// The database file does not exist; a successful
    /// [`Store::open_local`] will create it at the current
    /// schema version.
    MissingDatabase,
    /// The database file is corrupt. The inner report enumerates
    /// the recovery decisions the caller may take.
    Corrupt(CorruptionReport),
    /// The database file is healthy and openable.
    Healthy,
}

/// Report returned by [`StoreStartupGate::Corrupt`]. It pins the
/// recovery decisions the test contract locks in: a corrupt
/// database must be classified, must not be silently repaired,
/// and must offer a fixed set of explicit recovery decisions.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CorruptionReport {
    /// True when `PRAGMA integrity_check` failed.
    integrity_check_failed: bool,
    /// The set of explicit recovery decisions the caller may
    /// take. The order is part of the public contract.
    allowed_decisions: Vec<CorruptionDecision>,
}

impl CorruptionReport {
    /// Build a new report.
    pub fn new(integrity_check_failed: bool, allowed_decisions: Vec<CorruptionDecision>) -> Self {
        Self {
            integrity_check_failed,
            allowed_decisions,
        }
    }

    /// True when `PRAGMA integrity_check` failed.
    pub fn integrity_check_failed(&self) -> bool {
        self.integrity_check_failed
    }

    /// Returns the explicit recovery decisions the caller may
    /// take. The order is part of the public contract.
    pub fn allowed_decisions(&self) -> Vec<CorruptionDecision> {
        self.allowed_decisions.clone()
    }
}

/// Outcome of a successful corruption recovery. The new database
/// is created at the current schema version; the corrupt bytes
/// are quarantined to `quarantined_database` so the original
/// evidence is preserved.
#[derive(Debug, Clone)]
pub struct CorruptionRecoveryReport {
    /// Schema version of the freshly created database.
    pub schema_version: i64,
    /// Path to the quarantined copy of the corrupt database.
    pub quarantined_database: PathBuf,
}

/// Stable error envelope returned by every Store open / write path.
#[derive(Debug, Error)]
#[error("hivegui store error: {kind:?}")]
pub struct StoreError {
    /// Error variant. Each variant carries only safe values; the
    /// UI MUST never receive raw SQL error strings.
    pub kind: StoreErrorKind,
    /// Optional database path (e.g. when the error originates from
    /// a specific [`StoreOpenOptions`]).
    pub database_path: Option<PathBuf>,
}

impl StoreError {
    /// Build a new error with no database path.
    pub fn new(kind: StoreErrorKind) -> Self {
        Self {
            kind,
            database_path: None,
        }
    }

    /// Attach a database path to the error.
    pub fn with_database_path(mut self, path: PathBuf) -> Self {
        self.database_path = Some(path);
        self
    }

    /// Returns the error kind. Alias of [`Self::error_kind`] used
    /// by tests that follow the standard `kind()` accessor
    /// convention.
    pub fn kind(&self) -> StoreErrorKind {
        self.kind.clone()
    }

    /// Returns the error kind.
    pub fn error_kind(&self) -> StoreErrorKind {
        self.kind.clone()
    }

    /// Returns the database path, when known.
    pub fn database_path(&self) -> Option<&Path> {
        self.database_path.as_deref()
    }
}

/// Failure mode for [`Store::open`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum StoreErrorKind {
    /// `PRAGMA integrity_check` failed or the file is not a SQLite database.
    StoreCorrupt {
        /// Medium carrying the corruption.
        medium: &'static str,
    },
    /// `PRAGMA foreign_key_check` reported orphan rows.
    OrphanForeignKey {
        /// Number of orphan rows.
        count: i64,
    },
    /// Schema drift detected on open (missing column / table / CHECK).
    SchemaDrift {
        /// Description of the drift.
        description: String,
    },
    /// The committed high-watermark has not been advanced yet.
    CommittedHighWatermarkMissing,
    /// Underlying I/O error.
    Io(String),
}

/// Outcome of [`open_store`].
#[derive(Debug)]
pub enum OpenOutcome {
    /// Store opened with the listed sidecars (canonical names only).
    Opened {
        /// The opened Store handle.
        handle: Store,
        /// Sidecar files observed during open.
        sidecars: Vec<SidecarRecord>,
    },
    /// Open was rejected with the given error.
    Rejected(StoreError),
}

/// Canonical classification of one sidecar file.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum SidecarKind {
    /// Currently in use (WAL / SHM of the live primary).
    Hot,
    /// Owner cannot be determined.
    Unknown,
    /// Recoverable sidecar that requires explicit handling.
    Recoverable,
}

/// One sidecar file observed during [`open_store`].
#[derive(Debug, Clone)]
pub struct SidecarRecord {
    /// Path of the sidecar.
    pub path: PathBuf,
    /// Canonical classification.
    pub kind: SidecarKind,
}

/// Reason assigned to a quarantine record.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum QuarantineReason {
    /// Sidecar is identity-bound and would replace an existing record.
    IdentityBound,
    /// Sidecar is recoverable (e.g. uncommitted WAL).
    Recoverable,
}

/// Stable quarantine record schema.
#[derive(Debug, Clone)]
pub struct QuarantineRecord {
    /// Reason variant.
    pub reason: QuarantineReason,
    /// Stable string describing the legal reason.
    pub legal_reason: String,
    /// Numeric priority of this record.
    pub priority: i64,
}

impl QuarantineRecord {
    /// Construct a quarantine record (test helper).
    pub fn new_for_test(
        reason: QuarantineReason,
        legal_reason: impl Into<String>,
        priority: i64,
    ) -> Self {
        Self {
            reason,
            legal_reason: legal_reason.into(),
            priority,
        }
    }

    /// Persist the record. Identity-bound, no-replace: the file
    /// name is derived from the priority + reason; identical
    /// records still get a unique basename.
    pub fn save(&self, workspace_root: &Path) -> std::io::Result<PathBuf> {
        let dir = workspace_root
            .join("data")
            .join("hivegui")
            .join("quarantine");
        std::fs::create_dir_all(&dir)?;
        let counter = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_nanos())
            .unwrap_or(0);
        let basename = format!(
            "q-{}-{}-{}.json",
            self.priority,
            self.legal_reason.replace(['/', '\\', ' '], "_"),
            counter
        );
        let path = dir.join(basename);
        std::fs::write(&path, format!("{}|{}", self.legal_reason, self.priority))?;
        Ok(path)
    }

    /// Legal reason (stable string).
    pub fn legal_reason(&self) -> &str {
        &self.legal_reason
    }

    /// Numeric priority of this record.
    pub fn priority(&self) -> i64 {
        self.priority
    }

    /// Reason variant.
    pub fn reason(&self) -> QuarantineReason {
        self.reason
    }
}

/// Committed high-watermark gate. The Store refuses to start a
/// new write transaction until the open call has confirmed the
/// committed high-watermark.
pub struct WriteGate {
    committed: std::sync::atomic::AtomicBool,
}

impl Default for WriteGate {
    fn default() -> Self {
        Self::open()
    }
}

impl WriteGate {
    /// Open a fresh gate. The gate starts closed.
    pub fn open() -> Self {
        Self {
            committed: std::sync::atomic::AtomicBool::new(false),
        }
    }

    /// Mark the committed high-watermark as advanced.
    pub fn mark_committed(&self) {
        self.committed
            .store(true, std::sync::atomic::Ordering::SeqCst);
    }

    /// Test helper: try to acquire the gate.
    pub fn try_acquire_for_test(&self) -> Result<(), StoreError> {
        if self.committed.load(std::sync::atomic::Ordering::SeqCst) {
            Ok(())
        } else {
            Err(StoreError::new(
                StoreErrorKind::CommittedHighWatermarkMissing,
            ))
        }
    }
}

/// Open the Store at the given path with crash-safe sidecar
/// classification. Returns [`OpenOutcome::Rejected`] on any
/// integrity / orphan / schema-drift failure.
pub async fn open_store(
    database_path: &Path,
    workspace_root: &Path,
) -> Result<OpenOutcome, StoreError> {
    let _ = workspace_root;
    if !database_path.exists() {
        return Err(StoreError::new(StoreErrorKind::Io(
            "database file does not exist".into(),
        )));
    }
    let bytes = std::fs::read(database_path)
        .map_err(|e| StoreError::new(StoreErrorKind::Io(e.to_string())))?;
    if !bytes.starts_with(b"SQLite format 3") {
        return Err(StoreError::new(StoreErrorKind::StoreCorrupt {
            medium: "sqlite",
        }));
    }
    if bytes.len() < 16 {
        return Err(StoreError::new(StoreErrorKind::StoreCorrupt {
            medium: "sqlite",
        }));
    }
    // Open in read-write mode for the integrity check; this does
    // NOT apply schema migrations. A connection failure with
    // "file is not a database" means the header is valid but the
    // body is corrupted; treat that as StoreCorrupt so the
    // caller never sees a raw SQLite error string.
    let url = format!("sqlite://{}?mode=rwc", database_path.display());
    let pool = match sqlx::SqlitePool::connect(&url).await {
        Ok(pool) => pool,
        Err(err) => {
            let message = err.to_string();
            if message.contains("file is not a database") || message.contains("not a database") {
                return Err(StoreError::new(StoreErrorKind::StoreCorrupt {
                    medium: "sqlite",
                }));
            }
            return Err(StoreError::new(StoreErrorKind::Io(message)));
        }
    };
    let integrity: Result<String, sqlx::Error> = sqlx::query_scalar("PRAGMA integrity_check")
        .fetch_one(&pool)
        .await;
    let integrity = match integrity {
        Ok(value) => value,
        Err(err) => {
            // SQLite cannot run `integrity_check` on a file that
            // is no longer a valid database (header says SQLite
            // but the body is not). Treat the error as a
            // structural corruption, not an opaque I/O error.
            let message = err.to_string();
            if message.contains("file is not a database")
                || message.contains("not a database")
                || message.contains("database disk image is malformed")
            {
                return Err(StoreError::new(StoreErrorKind::StoreCorrupt {
                    medium: "sqlite",
                }));
            }
            return Err(StoreError::new(StoreErrorKind::Io(message)));
        }
    };
    if integrity.trim() != "ok" {
        return Err(StoreError::new(StoreErrorKind::StoreCorrupt {
            medium: "sqlite",
        }));
    }
    let orphan_rows = sqlx::query("PRAGMA foreign_key_check")
        .fetch_all(&pool)
        .await
        .map_err(|e| StoreError::new(StoreErrorKind::Io(e.to_string())))?;
    if !orphan_rows.is_empty() {
        return Err(StoreError::new(StoreErrorKind::OrphanForeignKey {
            count: orphan_rows.len() as i64,
        }));
    }
    // Verify v4 schema drift: required columns / tables / checks.
    if let Err(message) = crate::datasource::migrations::verify_schema(&pool).await {
        return Err(StoreError::new(StoreErrorKind::SchemaDrift {
            description: message.to_string(),
        }));
    }
    let sidecars = scan_sidecars(database_path);
    let store = Store::open_existing(database_path)
        .await
        .map_err(|e| StoreError::new(StoreErrorKind::Io(e.to_string())))?;
    Ok(OpenOutcome::Opened {
        handle: store,
        sidecars,
    })
}

fn scan_sidecars(database_path: &Path) -> Vec<SidecarRecord> {
    let mut sidecars = Vec::new();
    let parent = database_path.parent().unwrap_or_else(|| Path::new("."));
    let stem = database_path
        .file_name()
        .and_then(|n| n.to_str())
        .unwrap_or("datasources.db");
    let wal = parent.join(format!("{}-wal", stem));
    let shm = parent.join(format!("{}-shm", stem));
    let journal = parent.join(format!("{}-journal", stem));
    for (path, kind) in [
        (wal, SidecarKind::Hot),
        (shm, SidecarKind::Hot),
        (journal, SidecarKind::Recoverable),
    ] {
        if path.exists() {
            sidecars.push(SidecarRecord { path, kind });
        }
    }
    // Also include any unknown `.db-*` siblings.
    if let Ok(entries) = std::fs::read_dir(parent) {
        for entry in entries.flatten() {
            let name = entry
                .file_name()
                .to_str()
                .map(|s| s.to_string())
                .unwrap_or_default();
            if name.starts_with(&format!("{stem}-"))
                && !name.ends_with("-wal")
                && !name.ends_with("-shm")
                && !name.ends_with("-journal")
            {
                sidecars.push(SidecarRecord {
                    path: entry.path(),
                    kind: SidecarKind::Unknown,
                });
            }
        }
    }
    sidecars
}

#[derive(Debug)]
struct StoreInner {
    pool: Pool<Sqlite>,
    crypto: Crypto,
    /// Process-level write lock, held for the lifetime of the
    /// `Store`. Dropping the `Store` releases the lock so the
    /// next [`Store::open_local`] call can succeed.
    process_lock: Option<ProcessLock>,
    /// Canonical database path (used by the in-process owner
    /// registry cleanup on drop).
    database_path: PathBuf,
    /// Owner id assigned by the in-process owner registry.
    owner_id: u64,
    /// Optional query-count observer. When wired, every public
    /// store method that runs a checked SQL query routes its
    /// query-id through the observer so the query-count contract
    /// can verify that production operations stay within their
    /// declared budget.
    observer: Option<QueryCountObserver>,
}

#[derive(Clone, Debug)]
pub struct Store {
    inner: Arc<StoreInner>,
}

impl Drop for Store {
    fn drop(&mut self) {
        unregister_store_owner(&self.inner.database_path);
    }
}

impl Store {
    pub async fn new(db_dir: &Path) -> Result<Self> {
        std::fs::create_dir_all(db_dir)?;

        let db_path = db_dir.join(DB_FILENAME);
        let conn_opts =
            SqliteConnectOptions::from_str(&db_path.to_string_lossy())?.create_if_missing(true);

        let pool = SqlitePoolOptions::new().connect_with(conn_opts).await?;

        // Run v4 migration (idempotent).
        let _ = super::migrations::migrate_to_current(super::migrations::MigrationOptions::new(
            &db_path, db_dir,
        ))
        .await?;

        sqlx::query(
            r#"
            CREATE TABLE IF NOT EXISTS data_sources (
                id INTEGER PRIMARY KEY AUTOINCREMENT,
                name TEXT NOT NULL UNIQUE,
                host TEXT NOT NULL,
                port INTEGER NOT NULL DEFAULT 3306,
                username TEXT NOT NULL,
                encrypted_password BLOB NOT NULL,
                created_at TEXT NOT NULL,
                updated_at TEXT NOT NULL
            )
            "#,
        )
        .execute(&pool)
        .await?;

        sqlx::query("CREATE INDEX IF NOT EXISTS idx_data_sources_name ON data_sources(name)")
            .execute(&pool)
            .await?;

        // global_configs table
        sqlx::query(
            "CREATE TABLE IF NOT EXISTS global_configs (\
             id INTEGER PRIMARY KEY AUTOINCREMENT, \
             name TEXT NOT NULL, key TEXT NOT NULL UNIQUE, \
             type TEXT NOT NULL DEFAULT 'text', data TEXT NOT NULL, \
             created_at TEXT NOT NULL DEFAULT '', updated_at TEXT NOT NULL DEFAULT '')",
        )
        .execute(&pool)
        .await?;

        // FR-027: 运行数据库迁移（包括初始化实体表）
        super::entity_store::run_migrations(&pool).await?;
        super::entity_store::register_runtime_capabilities(&pool).await?;

        // 执行数据库完整性检查
        let integrity_result = sqlx::query_scalar::<_, String>("PRAGMA integrity_check")
            .fetch_one(&pool)
            .await;

        match integrity_result {
            Ok(result) if result == "ok" => {
                // 数据库完整性正常
            }
            Ok(result) => {
                tracing::error!("数据库完整性检查失败: {}", result);
                // 这里可以添加恢复逻辑，但目前仅记录错误
            }
            Err(e) => {
                tracing::error!("执行完整性检查时出错: {}", e);
            }
        }

        let key = Self::load_or_generate_key(db_dir)?;
        let crypto = Crypto::new(&key);

        Ok(Self {
            inner: Arc::new(StoreInner {
                pool,
                crypto,
                process_lock: None,
                database_path: db_dir.join(DB_FILENAME),
                owner_id: 0,
                observer: None,
            }),
        })
    }

    /// Open an existing v4 Store without applying migrations.
    pub async fn open_existing(db_path: &Path) -> Result<Self> {
        let conn_opts = SqliteConnectOptions::from_str(&db_path.to_string_lossy())?;
        let pool = SqlitePoolOptions::new().connect_with(conn_opts).await?;
        let key = Self::load_or_generate_key(db_path.parent().unwrap_or(Path::new(".")))?;
        let crypto = Crypto::new(&key);
        Ok(Self {
            inner: Arc::new(StoreInner {
                pool,
                crypto,
                process_lock: None,
                database_path: db_path.to_path_buf(),
                owner_id: 0,
                observer: None,
            }),
        })
    }

    /// Open a [`Store`] from explicit [`StoreOpenOptions`].
    ///
    /// The integration suite uses this entry point to drive the
    /// storage layer against a known database file and a known
    /// plugin artifact root. Migrations are applied
    /// idempotently before the connection pool is returned.
    ///
    /// The call respects the Foundation retry policy: failures
    /// classified as [`DatabaseFailureClass::DatabaseLocked`] or
    /// [`DatabaseFailureClass::FileBusy`] are retried with the
    /// exact 1s / 2s / 4s backoff ladder; every other class
    /// surfaces immediately. Corrupt databases are NOT repaired
    /// implicitly — the caller must drive the recovery path via
    /// [`Store::inspect_startup`] and
    /// [`Store::recover_corrupt_database`].
    pub async fn open_local(options: StoreOpenOptions) -> Result<Self, StoreOpenError> {
        let StoreOpenOptions {
            database_path,
            plugin_root,
            observer,
            fault_injector,
            retry_policy,
            retry_sleeper,
        } = options;

        let database_path_buf = database_path.clone();
        let policy = retry_policy.unwrap_or_default();
        let sleeper: Arc<dyn RetrySleeper> =
            retry_sleeper.unwrap_or_else(|| Arc::new(TokioRetrySleeper));

        if let Some(parent) = database_path.parent() {
            std::fs::create_dir_all(parent).map_err(|_error| {
                StoreOpenError::new(StoreOpenErrorKind::Io, Some(database_path_buf.clone()))
                    .with_failure_class(DatabaseFailureClass::Persistent)
            })?;
        }
        std::fs::create_dir_all(&plugin_root).map_err(|_error| {
            StoreOpenError::new(StoreOpenErrorKind::Io, Some(database_path_buf.clone()))
                .with_failure_class(DatabaseFailureClass::Persistent)
        })?;

        // Reject corruption before running migrations: a corrupt
        // local database must surface a dedicated gate, never a
        // silent rebuild.
        if database_path.exists() && !is_sqlite_file(&database_path) {
            return Err(StoreOpenError::new(
                StoreOpenErrorKind::CorruptDatabase,
                Some(database_path_buf),
            )
            .with_failure_class(DatabaseFailureClass::Corrupt));
        }

        // In-process lock check. Another live Store in this
        // process already owns the same database file; refuse
        // the second open. The in-process owner registry is
        // updated when the Store is dropped.
        if store_already_owned(&database_path) {
            return Err(StoreOpenError::new(
                StoreOpenErrorKind::AlreadyLocked,
                Some(database_path_buf),
            ));
        }

        // Inter-process lock: try to acquire an exclusive
        // `flock(2)` on the lock sidecar. A second process
        // holding the lock surfaces `AlreadyLocked`.
        let lock_path = lock_sidecar_path(&database_path);
        if let Some(parent) = lock_path.parent() {
            let _ = std::fs::create_dir_all(parent);
        }
        let process_lock = match ProcessLock::try_acquire(&lock_path) {
            Ok(Some(lock)) => Some(lock),
            Ok(None) => {
                return Err(StoreOpenError::new(
                    StoreOpenErrorKind::AlreadyLocked,
                    Some(database_path_buf),
                ));
            }
            Err(_error) => None,
        };

        // Register the in-process owner BEFORE returning so the
        // second open in the same process surfaces
        // `AlreadyLocked`. The Drop impl unregisters.
        let owner_id = register_store_owner(&database_path);

        // Retry loop — Foundation policy: 1 initial attempt + 3
        // retries for transient lock/busy failures, no retries
        // for any other class.
        let max_attempts = policy.maximum_attempts();
        let mut last_error: Option<StoreOpenError> = None;
        for attempt in 0..max_attempts {
            // Test-only fault injector hook.
            if let Some(injector) = fault_injector.as_ref()
                && let Some(class) = injector.before_open_attempt(attempt)
            {
                let delays = policy.delays(class);
                if delays.is_empty() || attempt + 1 >= max_attempts {
                    unregister_store_owner(&database_path);
                    return Err(StoreOpenError::new(
                        StoreOpenErrorKind::Io,
                        Some(database_path_buf.clone()),
                    )
                    .with_failure_class(class)
                    .with_retry_count(attempt));
                }
                if let Some(delay) = delays.get(attempt).copied() {
                    sleeper.sleep(delay).await;
                }
                last_error = Some(
                    StoreOpenError::new(StoreOpenErrorKind::Io, Some(database_path_buf.clone()))
                        .with_failure_class(class)
                        .with_retry_count(attempt),
                );
                continue;
            }

            match Self::try_open_once(&database_path, &plugin_root).await {
                Ok(mut inner) => {
                    inner.process_lock = process_lock;
                    inner.owner_id = owner_id;
                    inner.observer = observer.clone();
                    return Ok(Self {
                        inner: Arc::new(inner),
                    });
                }
                Err(error) => {
                    let class = error
                        .failure_class()
                        .unwrap_or(DatabaseFailureClass::Transient);
                    let delays = policy.delays(class);
                    if delays.is_empty() || attempt + 1 >= max_attempts {
                        unregister_store_owner(&database_path);
                        return Err(error);
                    }
                    if let Some(delay) = delays.get(attempt).copied() {
                        sleeper.sleep(delay).await;
                    }
                    last_error = Some(error);
                }
            }
        }
        unregister_store_owner(&database_path);
        Err(last_error.unwrap_or_else(|| {
            StoreOpenError::new(StoreOpenErrorKind::Io, Some(database_path_buf))
        }))
    }

    /// Run a single open attempt. The retry loop in
    /// [`Store::open_local`] drives this; tests rarely call it
    /// directly. The function never panics on a corrupt
    /// database — it returns
    /// [`StoreOpenErrorKind::CorruptDatabase`] instead.
    async fn try_open_once(
        database_path: &Path,
        plugin_root: &Path,
    ) -> Result<StoreInner, StoreOpenError> {
        let database_path_buf = database_path.to_path_buf();
        let conn_opts = SqliteConnectOptions::from_str(&database_path.to_string_lossy())
            .map_err(|_error| {
                StoreOpenError::new(StoreOpenErrorKind::Io, Some(database_path_buf.clone()))
                    .with_failure_class(DatabaseFailureClass::InvalidInput)
            })?
            .create_if_missing(true);
        let pool = SqlitePoolOptions::new()
            .connect_with(conn_opts)
            .await
            .map_err(|error| {
                let message = error.to_string();
                let class = if message.contains("database is locked") {
                    DatabaseFailureClass::DatabaseLocked
                } else if message.contains("database disk image is malformed")
                    || message.contains("file is not a database")
                    || message.contains("not a database")
                {
                    DatabaseFailureClass::Corrupt
                } else {
                    DatabaseFailureClass::Persistent
                };
                let kind = if matches!(class, DatabaseFailureClass::Corrupt) {
                    StoreOpenErrorKind::CorruptDatabase
                } else {
                    StoreOpenErrorKind::Io
                };
                StoreOpenError::new(kind, Some(database_path_buf.clone())).with_failure_class(class)
            })?;

        // Run v4 migration (idempotent) against the chosen path.
        let migration_options =
            super::migrations::MigrationOptions::new(database_path, plugin_root);
        if let Err(_migration_error) =
            super::migrations::migrate_to_current(migration_options).await
        {
            return Err(
                StoreOpenError::new(StoreOpenErrorKind::Io, Some(database_path_buf))
                    .with_failure_class(DatabaseFailureClass::Corrupt),
            );
        }

        if let Err(_entity_error) = super::entity_store::run_migrations(&pool).await {
            return Err(StoreOpenError::new(
                StoreOpenErrorKind::Io,
                Some(database_path_buf),
            ));
        }
        if let Err(_register_error) =
            super::entity_store::register_runtime_capabilities(&pool).await
        {
            return Err(StoreOpenError::new(
                StoreOpenErrorKind::Io,
                Some(database_path_buf),
            ));
        }

        let key_dir = database_path.parent().unwrap_or(Path::new("."));
        let key = Self::load_or_generate_key(key_dir).map_err(|_error| {
            StoreOpenError::new(StoreOpenErrorKind::Io, Some(database_path_buf.clone()))
                .with_failure_class(DatabaseFailureClass::Persistent)
        })?;
        let crypto = Crypto::new(&key);

        Ok(StoreInner {
            pool,
            crypto,
            process_lock: None,
            database_path: database_path_buf,
            owner_id: 0,
            observer: None,
        })
    }

    /// Inspect the database file without opening it. The
    /// classification is purely read-only: the file is never
    /// modified, never replaced, and never repaired implicitly.
    pub async fn inspect_startup(
        options: StoreOpenOptions,
    ) -> Result<StoreStartupGate, StoreOpenError> {
        let StoreOpenOptions { database_path, .. } = options;
        if !database_path.exists() {
            return Ok(StoreStartupGate::MissingDatabase);
        }
        if !is_sqlite_file(&database_path) {
            return Ok(StoreStartupGate::Corrupt(CorruptionReport::new(
                true,
                vec![
                    CorruptionDecision::RestoreBackup,
                    CorruptionDecision::RebuildLocalDatabase,
                    CorruptionDecision::Exit,
                ],
            )));
        }
        Ok(StoreStartupGate::Healthy)
    }

    /// Recover a corrupt local database. `confirmed` must be
    /// `true` for any non-trivial recovery (e.g. rebuild) to
    /// proceed; the function rejects unconfirmed rebuilds with
    /// [`StoreOpenErrorKind::ConfirmationRequired`].
    pub async fn recover_corrupt_database(
        options: StoreOpenOptions,
        decision: CorruptionDecision,
        confirmed: bool,
    ) -> Result<CorruptionRecoveryReport, StoreOpenError> {
        let StoreOpenOptions {
            database_path,
            plugin_root,
            ..
        } = options;
        let database_path_buf = database_path.clone();
        let plugin_root_buf = plugin_root.clone();

        // Rebuild requires explicit confirmation.
        if matches!(decision, CorruptionDecision::RebuildLocalDatabase) && !confirmed {
            return Err(StoreOpenError::new(
                StoreOpenErrorKind::ConfirmationRequired,
                Some(database_path_buf),
            ));
        }

        // Quarantine the corrupt bytes first, so the original
        // evidence is preserved even if the new database fails
        // to open.
        let quarantined_database = quarantine_corrupt_database(&database_path).await?;
        std::fs::remove_file(&database_path).map_err(|_error| {
            StoreOpenError::new(StoreOpenErrorKind::Io, Some(database_path_buf.clone()))
                .with_failure_class(DatabaseFailureClass::Persistent)
        })?;

        // Recreate the database at the current schema version.
        // Pass-through the original `plugin_root` so the v4
        // migration can find the managed plugin tree.
        let new_options = StoreOpenOptions::new(&database_path, &plugin_root_buf);
        let new_store = Self::open_local(new_options).await?;

        // The new `Store` registered itself as the new in-process
        // owner; drop it so the test can re-open the database
        // itself if it wants to. The recovery report is returned
        // before drop.
        let report = CorruptionRecoveryReport {
            schema_version: super::migrations::CURRENT_SCHEMA_VERSION,
            quarantined_database,
        };
        drop(new_store);
        Ok(report)
    }

    /// Test-only non-functional placeholder. The real constructor
    /// is async; this lets the UI mount a stub form for layout
    /// assertions without a backing database.
    pub fn placeholder() -> Self {
        // Build a minimal in-memory pool via a private tokio
        // runtime so the legacy `DataSourceForm` constructor can
        // be invoked from a sync context.
        use sqlx::sqlite::{SqliteConnectOptions, SqlitePoolOptions};
        let options = SqliteConnectOptions::new()
            .filename(":memory:")
            .create_if_missing(true)
            .foreign_keys(false)
            .journal_mode(sqlx::sqlite::SqliteJournalMode::Memory);
        let runtime = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .expect("placeholder store: tokio runtime");
        let pool = runtime
            .block_on(async {
                SqlitePoolOptions::new()
                    .min_connections(1)
                    .max_connections(1)
                    .connect_with(options)
                    .await
            })
            .expect("placeholder store: connect");
        Self {
            inner: Arc::new(StoreInner {
                pool,
                crypto: crate::datasource::Crypto::placeholder(),
                process_lock: None,
                database_path: PathBuf::new(),
                owner_id: 0,
                observer: None,
            }),
        }
    }

    fn load_or_generate_key(db_dir: &Path) -> Result<[u8; KEY_SIZE]> {
        let key_path = db_dir.join("encryption.key");
        if key_path.exists() {
            let data = std::fs::read(&key_path)?;
            if data.len() != KEY_SIZE {
                anyhow::bail!("Invalid key file size");
            }
            let mut key = [0u8; KEY_SIZE];
            key.copy_from_slice(&data);
            Ok(key)
        } else {
            let key = Crypto::generate_key();
            std::fs::write(&key_path, key)?;
            Ok(key)
        }
    }

    pub async fn list(&self) -> Result<Vec<DataSource>> {
        let rows = sqlx::query(
            "SELECT id, name, host, port, username, encrypted_password, created_at, updated_at FROM data_sources ORDER BY name",
        )
        .fetch_all(&self.inner.pool)
        .await?;

        let mut sources = Vec::new();
        for row in rows {
            let created_at = row
                .try_get::<String, _>("created_at")?
                .parse::<DateTime<Utc>>()
                .unwrap_or_else(|_| Utc::now());
            let updated_at = row
                .try_get::<String, _>("updated_at")?
                .parse::<DateTime<Utc>>()
                .unwrap_or_else(|_| Utc::now());
            sources.push(DataSource {
                id: row.try_get("id")?,
                name: row.try_get("name")?,
                host: row.try_get("host")?,
                port: row.try_get("port")?,
                username: row.try_get("username")?,
                encrypted_password: row.try_get("encrypted_password")?,
                created_at,
                updated_at,
            });
        }
        Ok(sources)
    }

    pub async fn get(&self, id: i64) -> Result<Option<DataSource>> {
        // query-plan: id=t012.data_sources.by_id; owner_phase=US1; activation_task=T019
        let row = sqlx::query("SELECT id, name, host, port, username, encrypted_password, created_at, updated_at FROM data_sources WHERE id = ?")
            .bind(id).fetch_optional(&self.inner.pool).await?;
        match row {
            Some(row) => {
                let created_at = row
                    .try_get::<String, _>("created_at")?
                    .parse::<DateTime<Utc>>()
                    .unwrap_or_else(|_| Utc::now());
                let updated_at = row
                    .try_get::<String, _>("updated_at")?
                    .parse::<DateTime<Utc>>()
                    .unwrap_or_else(|_| Utc::now());
                Ok(Some(DataSource {
                    id: row.try_get("id")?,
                    name: row.try_get("name")?,
                    host: row.try_get("host")?,
                    port: row.try_get("port")?,
                    username: row.try_get("username")?,
                    encrypted_password: row.try_get("encrypted_password")?,
                    created_at,
                    updated_at,
                }))
            }
            None => Ok(None),
        }
    }

    pub async fn create(
        &self,
        name: &str,
        host: &str,
        port: u16,
        username: &str,
        password: &[u8],
    ) -> Result<DataSource> {
        let encrypted = self.inner.crypto.encrypt(password)?;
        let now = Utc::now();
        let result = sqlx::query("INSERT INTO data_sources (name, host, port, username, encrypted_password, created_at, updated_at) VALUES (?, ?, ?, ?, ?, ?, ?)")
            .bind(name).bind(host).bind(port as i64).bind(username).bind(&encrypted).bind(now.to_rfc3339()).bind(now.to_rfc3339()).execute(&self.inner.pool).await?;
        self.get(result.last_insert_rowid())
            .await?
            .ok_or_else(|| anyhow::anyhow!("Failed to retrieve newly created data source"))
    }

    pub async fn update(
        &self,
        id: i64,
        name: &str,
        host: &str,
        port: u16,
        username: &str,
        password: Option<&[u8]>,
    ) -> Result<Option<DataSource>> {
        if self.get(id).await?.is_none() {
            return Ok(None);
        }
        let now = Utc::now();
        if let Some(pwd) = password {
            // query-plan: id=t012.data_sources.update; owner_phase=US1; activation_task=T020
            let encrypted = self.inner.crypto.encrypt(pwd)?;
            sqlx::query("UPDATE data_sources SET name = ?, host = ?, port = ?, username = ?, encrypted_password = ?, updated_at = ? WHERE id = ?")
                .bind(name).bind(host).bind(port as i64).bind(username).bind(&encrypted).bind(now.to_rfc3339()).bind(id).execute(&self.inner.pool).await?;
        } else {
            // query-plan: id=t012.data_sources.update_no_pwd; owner_phase=US1; activation_task=T020
            sqlx::query("UPDATE data_sources SET name = ?, host = ?, port = ?, username = ?, updated_at = ? WHERE id = ?")
                .bind(name).bind(host).bind(port as i64).bind(username).bind(now.to_rfc3339()).bind(id).execute(&self.inner.pool).await?;
        }
        self.get(id).await
    }

    pub async fn delete(&self, id: i64) -> Result<bool> {
        // query-plan: id=t012.data_sources.delete; owner_phase=US1; activation_task=T021
        Ok(sqlx::query("DELETE FROM data_sources WHERE id = ?")
            .bind(id)
            .execute(&self.inner.pool)
            .await?
            .rows_affected()
            > 0)
    }

    pub fn decrypt_password(&self, encrypted: &[u8]) -> Result<Vec<u8>> {
        self.inner.crypto.decrypt(encrypted)
    }

    pub fn default_db_path() -> PathBuf {
        dirs::data_local_dir()
            .unwrap_or_else(|| dirs::home_dir().unwrap_or_default())
            .join("hivegui")
    }

    pub fn pool(&self) -> &Pool<Sqlite> {
        &self.inner.pool
    }

    /// Returns the absolute database path used by this store.
    pub fn database_path(&self) -> &Path {
        &self.inner.database_path
    }

    pub fn crypto(&self) -> &Crypto {
        &self.inner.crypto
    }

    /// Return the current schema version recorded in the `meta`
    /// table. Used by the query-count contract to pin the
    /// production SQL boundary to a single round trip.
    pub async fn schema_version(&self) -> Result<i64> {
        // query-plan: id=t012.meta.read_schema_version; owner_phase=Foundation; activation_task=T028
        self.record_query("foundation.store.schema_version");
        let row: Option<String> =
            sqlx::query_scalar!("SELECT value FROM meta WHERE key = 'schema_version'") // owner_phase=Foundation
                .fetch_optional(&self.inner.pool)
                .await?;
        match row {
            Some(value) => Ok(value
                .parse::<i64>()
                .unwrap_or(super::migrations::SCHEMA_VERSION_V4)),
            None => Ok(super::migrations::SCHEMA_VERSION_V4),
        }
    }

    /// Forward a checked-query identifier to the store's
    /// query-count observer, when one is wired. Used by the
    /// query-count contract tests to verify that production
    /// methods keep their round-trip budget fixed.
    fn record_query(&self, query_id: &'static str) {
        if let Some(observer) = self.inner.observer.as_ref() {
            observer.record_checked_query(query_id);
        }
    }
}

// ── GlobalConfig CRUD ──
#[derive(Debug, Clone, sqlx::FromRow)]
pub struct GlobalConfig {
    pub id: i64,
    pub name: String,
    pub key: String,
    #[sqlx(rename = "type")]
    pub config_type: String,
    pub data: String,
    pub created_at: String,
    pub updated_at: String,
}

impl Store {
    /// Load the stored payload for `key` if present.
    pub async fn get_global_config(&self, key: &str) -> Result<Option<String>> {
        Ok(
            // query-plan: id=t012.global_configs.by_key; owner_phase=US1; activation_task=T019
            sqlx::query_scalar::<_, String>("SELECT data FROM global_configs WHERE key = ?")
                .bind(key)
                .fetch_optional(&self.inner.pool)
                .await?,
        )
    }

    /// Insert or replace a config record by unique key.
    pub async fn upsert_global_config(
        &self,
        name: &str,
        key: &str,
        config_type: &str,
        data: &str,
    ) -> Result<GlobalConfig> {
        let now = Utc::now().to_rfc3339();
        sqlx::query(
            "INSERT INTO global_configs (name, key, type, data, created_at, updated_at) \
             VALUES (?, ?, ?, ?, ?, ?) \
             ON CONFLICT(key) DO UPDATE SET name=excluded.name, type=excluded.type, \
             data=excluded.data, updated_at=excluded.updated_at",
        )
        .bind(name)
        .bind(key)
        .bind(config_type)
        .bind(data)
        .bind(&now)
        .bind(&now)
        .execute(&self.inner.pool)
        .await?;

        // query-plan: id=t012.global_configs.by_key_after_upsert; owner_phase=US1; activation_task=T019
        sqlx::query_as::<_, GlobalConfig>("SELECT * FROM global_configs WHERE key = ?")
            .bind(key)
            .fetch_one(&self.inner.pool)
            .await
            .map_err(|e| e.into())
    }

    pub async fn create_global_config(
        &self,
        name: &str,
        key: &str,
        config_type: &str,
        data: &str,
    ) -> Result<GlobalConfig> {
        let now = Utc::now().to_rfc3339();
        sqlx::query("INSERT INTO global_configs (name, key, type, data, created_at, updated_at) VALUES (?, ?, ?, ?, ?, ?)")
            .bind(name).bind(key).bind(config_type).bind(data).bind(&now).bind(&now).execute(&self.inner.pool).await?;
        // query-plan: id=t012.global_configs.by_id_inserted; owner_phase=US1; activation_task=T019
        Ok(sqlx::query_as::<_, GlobalConfig>(
            "SELECT * FROM global_configs WHERE id = last_insert_rowid()",
        )
        .fetch_one(&self.inner.pool)
        .await?)
    }
    pub async fn list_global_configs(
        &self,
        search: &str,
        page: i64,
        page_size: i64,
    ) -> Result<(Vec<GlobalConfig>, i64)> {
        // Public-boundary validation: page must be >= 1 and page_size
        // must be exactly 20 (the contract says the Foundation list
        // path uses a fixed page size). Long searches and control
        // characters are rejected before they reach SQLite.
        if page < 1 {
            return Err(super::validation::PublicBoundaryError::new(
                super::validation::PublicErrorEnvelope::InvalidInput {
                    field: "page".to_string(),
                    reason: "out_of_range".to_string(),
                },
            )
            .into());
        }
        if page_size != 20 {
            return Err(super::validation::PublicBoundaryError::new(
                super::validation::PublicErrorEnvelope::InvalidInput {
                    field: "page_size".to_string(),
                    reason: "fixed_value_required".to_string(),
                },
            )
            .into());
        }
        if search.chars().count() > 128 {
            return Err(super::validation::PublicBoundaryError::new(
                super::validation::PublicErrorEnvelope::InvalidInput {
                    field: "search".to_string(),
                    reason: "too_long".to_string(),
                },
            )
            .into());
        }
        if search.chars().any(|c| c.is_control()) {
            return Err(super::validation::PublicBoundaryError::new(
                super::validation::PublicErrorEnvelope::InvalidInput {
                    field: "search".to_string(),
                    reason: "control_character".to_string(),
                },
            )
            .into());
        }
        let total: (i64,) = if search.is_empty() {
            // query-plan: id=t012.global_configs.count_all; owner_phase=US1; activation_task=T019
            sqlx::query_as("SELECT COUNT(*) FROM global_configs")
                .fetch_one(&self.inner.pool)
                .await?
        } else {
            // query-plan: id=t012.global_configs.search_count; owner_phase=US1; activation_task=T019
            sqlx::query_as("SELECT COUNT(*) FROM global_configs WHERE name = ? OR key = ?")
                .bind(search)
                .bind(search)
                .fetch_one(&self.inner.pool)
                .await?
        };
        let rows = if search.is_empty() {
            // query-plan: id=t012.global_configs.list_all; owner_phase=US1; activation_task=T019
            sqlx::query_as("SELECT * FROM global_configs ORDER BY id LIMIT ? OFFSET ?")
                .bind(page_size)
                .bind((page - 1) * page_size)
                .fetch_all(&self.inner.pool)
                .await?
        } else {
            // query-plan: id=t012.global_configs.search_list; owner_phase=US1; activation_task=T019
            sqlx::query_as(
                "SELECT * FROM global_configs WHERE name = ? OR key = ? ORDER BY id LIMIT ? OFFSET ?",
            )
            .bind(search)
            .bind(search)
            .bind(page_size)
            .bind((page - 1) * page_size)
            .fetch_all(&self.inner.pool)
            .await?
        };
        Ok((rows, total.0))
    }
    pub async fn update_global_config(
        &self,
        id: i64,
        name: &str,
        key: &str,
        config_type: &str,
        data: &str,
    ) -> Result<bool> {
        let now = Utc::now();
        Ok({
            // query-plan: id=t012.global_configs.update; owner_phase=US1; activation_task=T020
            sqlx::query(
                "UPDATE global_configs SET name=?, key=?, type=?, data=?, updated_at=? WHERE id=?",
            )
            .bind(name)
            .bind(key)
            .bind(config_type)
            .bind(data)
            .bind(now)
            .bind(id)
            .execute(&self.inner.pool)
            .await?
            .rows_affected()
                > 0
        })
    }
    pub async fn delete_global_config(&self, id: i64) -> Result<bool> {
        // query-plan: id=t012.global_configs.delete; owner_phase=US1; activation_task=T021
        Ok(sqlx::query("DELETE FROM global_configs WHERE id=?")
            .bind(id)
            .execute(&self.inner.pool)
            .await?
            .rows_affected()
            > 0)
    }
}
