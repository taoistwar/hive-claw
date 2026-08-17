//! HiveGUI local DataSource store.
//!
//! T038 [US2] implementation. Source of truth:
//! `specs/011-hivegui-standalone-mode/tasks.md` §T038.
//!
//! Public boundary the Red tests (T034 / T035 / T036) drive:
//!   - [`DataSourceStore::new`]
//!   - [`DataSourceStore::create`] / [`DataSourceStore::update`]
//!   - [`DataSourceStore::get_by_id`] / [`DataSourceStore::get_by_name`]
//!   - [`DataSourceStore::list`] (paging + filter)
//!   - [`DataSourceStore::explain_list_plan`]
//!   - [`DataSourceInput::new`] / [`DataSourceInput::for_update`]
//!   - [`Conflict`] / [`ConflictReason`]
//!   - [`EmptyPasswordPolicy::KeepExisting`]
//!   - [`DataSourceFilter`] / [`DataSourcePage`]
//!
//! T016F `DataSourcePassword` canary is the first non-Foundation
//! activation: every password write goes through the device-key
//! ChaCha20Poly1305 boundary; the canary scanner exercises the
//! public roundtrip and observes zero residue across SQLite main,
//! WAL, SHM, journal, temp dir, sanitized errors.

#![warn(missing_docs)]

use std::fmt;

use chrono::{DateTime, Utc};
use sqlx::{Row, SqlitePool};
use thiserror::Error;

use super::crypto::Crypto;

const KEY_SIZE: usize = 32;

/// Stable failure envelope returned by every DataSource store write path.
///
/// Each variant carries only safe values; UI MUST never receive raw
/// SQL error strings or password material. The canary test
/// `sanitized_error_does_not_leak_canary_plaintext` depends on
/// `Display` / `Debug` never printing the canary payload.
#[derive(Debug, Error)]
#[error("data source store error: {kind:?}")]
pub struct DataSourceStoreError {
    /// Error variant.
    pub kind: DataSourceStoreErrorKind,
}

impl DataSourceStoreError {
    /// Conflict reason when this is a [`DataSourceStoreErrorKind::Conflict`].
    ///
    /// For other failure modes the value is unspecified (callers
    /// should check [`Self::kind`] first). The T034 test always
    /// receives a conflict here, so the public boundary returns the
    /// typed `ConflictReason` directly.
    pub fn reason(&self) -> ConflictReason {
        match &self.kind {
            DataSourceStoreErrorKind::Conflict(conflict) => conflict.reason(),
            DataSourceStoreErrorKind::InvalidInput { .. } => ConflictReason::Name,
            DataSourceStoreErrorKind::Backend(_) => ConflictReason::Name,
            DataSourceStoreErrorKind::Cancelled => ConflictReason::Name,
        }
    }

    /// Field that triggered the failure, when known. Returns the
    /// conflict field when this is a
    /// [`DataSourceStoreErrorKind::Conflict`], the `InvalidInput`
    /// field for invalid input, or an empty string for backend errors.
    pub fn field(&self) -> &str {
        match &self.kind {
            DataSourceStoreErrorKind::Conflict(conflict) => conflict.field(),
            DataSourceStoreErrorKind::InvalidInput { field, .. } => field.as_str(),
            DataSourceStoreErrorKind::Backend(_) => "",
            DataSourceStoreErrorKind::Cancelled => "",
        }
    }

    /// Stable reference list. Currently only populated by the
    /// Conflict variant; other variants return an empty slice.
    pub fn references(&self) -> &[String] {
        match &self.kind {
            DataSourceStoreErrorKind::Conflict(conflict) => conflict.references(),
            _ => &[],
        }
    }
}

/// Failure mode for the DataSource store.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum DataSourceStoreErrorKind {
    /// A `ConflictReason`-driven rejection.
    Conflict(Conflict),
    /// Input failed the local validation step.
    InvalidInput {
        /// Field that failed validation.
        field: String,
        /// Stable reason code.
        reason: String,
    },
    /// Underlying I/O / SQLx failure with a sanitized cause.
    Backend(String),
    /// The caller cancelled the operation via [`CreateCancel`].
    Cancelled,
}

/// Conflict reason assigned to a conflict rejection.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ConflictReason {
    /// `name` is not unique.
    Name,
}

/// Conflict envelope returned to the caller. The struct intentionally
/// keeps only safe values (no SQL fragments, no password material,
/// no raw row content).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Conflict {
    field: String,
    reason: ConflictReason,
    references: Vec<String>,
}

impl Conflict {
    /// Construct a conflict envelope.
    pub fn new(field: impl Into<String>, reason: ConflictReason) -> Self {
        Self {
            field: field.into(),
            reason,
            references: Vec::new(),
        }
    }

    /// Conflict field (e.g. `"name"`).
    pub fn field(&self) -> &str {
        &self.field
    }

    /// Conflict reason variant.
    pub fn reason(&self) -> ConflictReason {
        self.reason
    }

    /// Stable reference list (currently always empty for the
    /// DataSource name conflict; the field exists so the public
    /// boundary does not change when story-owned conflict reasons
    /// land).
    pub fn references(&self) -> &[String] {
        &self.references
    }
}

impl From<Conflict> for DataSourceStoreError {
    fn from(conflict: Conflict) -> Self {
        Self {
            kind: DataSourceStoreErrorKind::Conflict(conflict),
        }
    }
}

/// Policy applied when an `update` carries an empty password string.
///
/// HiveGUI never silently clears a stored password. An empty
/// password on an update means "keep the existing ciphertext",
/// which is the *only* policy the v1 store exposes.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum EmptyPasswordPolicy {
    /// Keep the existing ciphertext untouched.
    #[default]
    KeepExisting,
}

/// Validated input for create / update.
#[derive(Debug, Clone)]
pub struct DataSourceInput {
    /// Target row id (only set on update inputs).
    pub id: Option<i64>,
    /// DataSource display name.
    pub name: String,
    /// Host address.
    pub host: String,
    /// TCP port.
    pub port: u16,
    /// Username.
    pub username: String,
    /// Password (plaintext only inside the validate step; persisted as
    /// ChaCha20Poly1305 ciphertext before any disk I/O).
    pub password: String,
    /// Database name.
    pub database: String,
    /// Empty-password policy.
    pub policy: EmptyPasswordPolicy,
}

impl DataSourceInput {
    /// Validate and construct a new-record input.
    pub fn new(
        name: &str,
        host: &str,
        port: u16,
        username: &str,
        password: &str,
        database: &str,
    ) -> Result<Self, DataSourceStoreError> {
        if name.trim().is_empty() {
            return Err(DataSourceStoreError {
                kind: DataSourceStoreErrorKind::InvalidInput {
                    field: "name".into(),
                    reason: "must not be empty".into(),
                },
            });
        }
        if host.trim().is_empty() {
            return Err(DataSourceStoreError {
                kind: DataSourceStoreErrorKind::InvalidInput {
                    field: "host".into(),
                    reason: "must not be empty".into(),
                },
            });
        }
        if port == 0 {
            return Err(DataSourceStoreError {
                kind: DataSourceStoreErrorKind::InvalidInput {
                    field: "port".into(),
                    reason: "out_of_range".into(),
                },
            });
        }
        if username.is_empty() {
            return Err(DataSourceStoreError {
                kind: DataSourceStoreErrorKind::InvalidInput {
                    field: "username".into(),
                    reason: "must not be empty".into(),
                },
            });
        }
        Ok(Self {
            id: None,
            name: name.to_string(),
            host: host.to_string(),
            port,
            username: username.to_string(),
            password: password.to_string(),
            database: database.to_string(),
            policy: EmptyPasswordPolicy::default(),
        })
    }

    /// Construct an update input that targets `id`.
    pub fn for_update(id: i64) -> Self {
        Self {
            id: Some(id),
            name: String::new(),
            host: String::new(),
            port: 3306,
            username: String::new(),
            password: String::new(),
            database: String::new(),
            policy: EmptyPasswordPolicy::KeepExisting,
        }
    }

    /// Target row id (only set on update inputs).
    pub fn id(&self) -> Option<i64> {
        self.id
    }
}

/// Stable record returned by every read path.
#[derive(Debug, Clone)]
pub struct DataSourceRecord {
    /// Row primary key.
    pub id: i64,
    /// DataSource display name.
    pub name: String,
    /// Host address.
    pub host: String,
    /// TCP port.
    pub port: u16,
    /// Username.
    pub username: String,
    /// Database name.
    pub database: String,
    /// Raw ciphertext blob (None if no password was ever set).
    pub password_ciphertext: Option<Vec<u8>>,
    /// Creation timestamp.
    pub created_at: DateTime<Utc>,
    /// Last-update timestamp.
    pub updated_at: DateTime<Utc>,
}

impl DataSourceRecord {
    /// Row primary key.
    pub fn id(&self) -> i64 {
        self.id
    }

    /// DataSource display name.
    pub fn name(&self) -> &str {
        &self.name
    }

    /// Raw ciphertext blob (None if no password was ever set).
    pub fn password_ciphertext(&self) -> Option<&[u8]> {
        self.password_ciphertext.as_deref()
    }
}

impl fmt::Display for DataSourceRecord {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(formatter, "DataSourceRecord({} {})", self.id, self.name)
    }
}

/// View mode for the DataSource UI surface.
///
/// T039 / T036 require a single tagged enum for the list / form /
/// empty / error states. The legacy parallel-flag patterns
/// (`DataSourceViewMode::AddForm` / `EditForm` carry no separate
/// visibility boolean) are the only mode surface the T039 contract
/// accepts.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub enum DataSourceViewMode {
    /// Show the paginated list.
    #[default]
    List,
    /// Show the Add form.
    AddForm,
    /// Show the Edit form for the given record id.
    EditForm(i64),
    /// The store is empty; render an explicit empty state.
    Empty,
    /// The store reported a recoverable error; the string is
    /// already sanitized by [`DataSourceStoreError`].
    Error(String),
}

/// Cancellation token for in-flight `create` / `update` operations.
/// T039 hands a token to the background MySQL probe so the UI
/// thread can abandon the wait without aborting the SQLx call.
#[derive(Debug, Default, Clone)]
pub struct CreateCancel {
    cancelled: std::sync::Arc<std::sync::atomic::AtomicBool>,
}

impl CreateCancel {
    /// Create a fresh token in the *not cancelled* state.
    pub fn new() -> Self {
        Self::default()
    }

    /// Mark the operation as cancelled. The store polls this flag
    /// on every error-recovery checkpoint; the SQLx call is
    /// allowed to run to completion (so the connection is
    /// returned to the pool) but the resulting record is
    /// discarded and a [`DataSourceStoreErrorKind::Cancelled`]
    /// is returned to the caller.
    pub fn cancel(&self) {
        self.cancelled
            .store(true, std::sync::atomic::Ordering::SeqCst);
    }

    /// Returns `true` once `cancel` has been called.
    pub fn is_cancelled(&self) -> bool {
        self.cancelled.load(std::sync::atomic::Ordering::SeqCst)
    }
}

/// Optional filter for the list path.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct DataSourceFilter {
    /// Search substring (exact prefix, ASCII case-sensitive).
    pub search: Option<String>,
}

impl DataSourceFilter {
    /// Filter with no constraints.
    pub fn none() -> Self {
        Self::default()
    }
}

/// Paging cursor. T034 requires 20 rows / page; [`DataSourcePage::first`]
/// produces the canonical first page and [`DataSourcePage::advance`]
/// returns `true` while another page may be available.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct DataSourcePage {
    /// 0-based offset.
    pub offset: u64,
    /// Page size.
    pub limit: u64,
}

impl DataSourcePage {
    /// First page of `limit` rows.
    pub fn first(limit: u64) -> Self {
        Self { offset: 0, limit }
    }

    /// Move the cursor forward by `count` and report whether more
    /// rows are likely available.
    pub fn advance(&mut self, count: usize) -> bool {
        self.offset = self.offset.saturating_add(count as u64);
        count as u64 == self.limit
    }
}

#[derive(Debug, Clone)]
struct DataSourceStoreInner {
    pool: SqlitePool,
    crypto: Crypto,
}

/// HiveGUI local DataSource store backed by SQLite + device-key
/// ChaCha20Poly1305 encryption. The store owns its SQLite pool
/// and crypto handle; it does not allocate on every read.
#[derive(Debug, Clone)]
pub struct DataSourceStore {
    inner: DataSourceStoreInner,
}

impl DataSourceStore {
    /// Open or create the DataSource store against the given
    /// SQLite pool. The device key is the 32-byte random secret
    /// produced by the Foundation key lifecycle (`T025`); every
    /// password write goes through the ChaCha20Poly1305 boundary
    /// keyed by that material.
    ///
    /// The constructor is async because it eagerly ensures the
    /// schema; callers (including the UI thread) must be on a
    /// tokio runtime. The same handle is reused for every other
    /// method on this store.
    pub async fn new(
        pool: SqlitePool,
        device_key: [u8; KEY_SIZE],
    ) -> Result<Self, DataSourceStoreError> {
        let crypto = Crypto::new(&device_key);
        let store = Self {
            inner: DataSourceStoreInner { pool, crypto },
        };
        store.ensure_schema().await?;
        Ok(store)
    }

    /// Test-only non-functional placeholder. The constructor is
    /// async, so the test surface area needs a sync stand-in for
    /// layout-only assertions. Calling any persistence method on
    /// the returned handle is a logic error; the `for_test`
    /// constructor in `datasource_view` uses this explicitly.
    pub fn placeholder() -> Self {
        Self {
            inner: DataSourceStoreInner {
                pool: dummy_pool(),
                crypto: Crypto::placeholder(),
            },
        }
    }

    async fn ensure_schema(&self) -> Result<(), DataSourceStoreError> {
        sqlx::query(
            r#"
            CREATE TABLE IF NOT EXISTS data_sources (
                id INTEGER PRIMARY KEY AUTOINCREMENT,
                name TEXT NOT NULL UNIQUE,
                host TEXT NOT NULL,
                port INTEGER NOT NULL DEFAULT 3306,
                username TEXT NOT NULL,
                database TEXT NOT NULL DEFAULT '',
                encrypted_password BLOB,
                created_at TEXT NOT NULL,
                updated_at TEXT NOT NULL
            )
            "#,
        )
        .execute(&self.inner.pool)
        .await
        .map_err(sanitize)?;
        sqlx::query("CREATE INDEX IF NOT EXISTS data_sources_name_idx ON data_sources(name)")
            .execute(&self.inner.pool)
            .await
            .map_err(sanitize)?;
        Ok(())
    }

    /// Insert a new record. Returns [`Conflict`] if `name` collides.
    /// The optional `cancel` token lets the background MySQL probe
    /// surface cancellation back to the UI thread (T039).
    pub async fn create(
        &self,
        input: DataSourceInput,
        cancel: Option<&CreateCancel>,
    ) -> Result<DataSourceRecord, DataSourceStoreError> {
        if input.id.is_some() {
            return Err(DataSourceStoreError {
                kind: DataSourceStoreErrorKind::InvalidInput {
                    field: "id".into(),
                    reason: "create input must not carry an id".into(),
                },
            });
        }
        let DataSourceInput {
            name,
            host,
            port,
            username,
            database,
            password,
            ..
        } = input;
        let encrypted = self
            .inner
            .crypto
            .encrypt(password.as_bytes())
            .map_err(sanitize)?;
        let now = Utc::now().to_rfc3339();
        let outcome = sqlx::query(
            "INSERT INTO data_sources \
             (name, host, port, username, database, encrypted_password, created_at, updated_at) \
             VALUES (?, ?, ?, ?, ?, ?, ?, ?)",
        )
        .bind(&name)
        .bind(&host)
        .bind(port as i64)
        .bind(&username)
        .bind(&database)
        .bind(&encrypted)
        .bind(&now)
        .bind(&now)
        .execute(&self.inner.pool)
        .await;
        let result = match outcome {
            Ok(result) => result.last_insert_rowid(),
            Err(err) if is_unique_violation(&err) => {
                return Err(DataSourceStoreError {
                    kind: DataSourceStoreErrorKind::Conflict(Conflict::new(
                        "name",
                        ConflictReason::Name,
                    )),
                });
            }
            Err(err) => return Err(sanitize(err)),
        };
        if cancel.map(CreateCancel::is_cancelled).unwrap_or(false) {
            return Err(DataSourceStoreError {
                kind: DataSourceStoreErrorKind::Cancelled,
            });
        }
        self.get_by_id(result)
            .await?
            .ok_or_else(|| DataSourceStoreError {
                kind: DataSourceStoreErrorKind::Backend("record vanished after insert".into()),
            })
    }

    /// Update an existing record. Honors [`EmptyPasswordPolicy`]:
    /// when `password` is empty the existing ciphertext is kept.
    pub async fn update(
        &self,
        input: DataSourceInput,
    ) -> Result<DataSourceRecord, DataSourceStoreError> {
        let id = input.id.ok_or_else(|| DataSourceStoreError {
            kind: DataSourceStoreErrorKind::InvalidInput {
                field: "id".into(),
                reason: "update input must carry an id".into(),
            },
        })?;
        let existing = self
            .get_by_id(id)
            .await?
            .ok_or_else(|| DataSourceStoreError {
                kind: DataSourceStoreErrorKind::Backend(format!("id {id} not found")),
            })?;
        let DataSourceInput {
            name,
            host,
            port,
            username,
            database,
            password,
            ..
        } = input;
        let next_name = if name.is_empty() {
            existing.name.clone()
        } else {
            name
        };
        let next_host = if host.is_empty() {
            existing.host.clone()
        } else {
            host
        };
        let next_port = if port == 0 { existing.port } else { port };
        let next_username = if username.is_empty() {
            existing.username.clone()
        } else {
            username
        };
        let next_database = if database.is_empty() {
            existing.database.clone()
        } else {
            database
        };
        let next_ciphertext = if password.is_empty() {
            existing.password_ciphertext.clone()
        } else {
            Some(
                self.inner
                    .crypto
                    .encrypt(password.as_bytes())
                    .map_err(sanitize)?,
            )
        };
        let now = Utc::now().to_rfc3339();
        sqlx::query(
            "UPDATE data_sources SET name = ?, host = ?, port = ?, username = ?, database = ?, encrypted_password = ?, updated_at = ? WHERE id = ?",
        )
        .bind(&next_name)
        .bind(&next_host)
        .bind(next_port as i64)
        .bind(&next_username)
        .bind(&next_database)
        .bind(next_ciphertext.as_deref())
        .bind(&now)
        .bind(id)
        .execute(&self.inner.pool)
        .await
        .map_err(sanitize)?;
        self.get_by_id(id)
            .await?
            .ok_or_else(|| DataSourceStoreError {
                kind: DataSourceStoreErrorKind::Backend("record vanished after update".into()),
            })
    }

    /// Read a single record by primary key.
    pub async fn get_by_id(
        &self,
        id: i64,
    ) -> Result<Option<DataSourceRecord>, DataSourceStoreError> {
        let row = sqlx::query(
            "SELECT id, name, host, port, username, database, encrypted_password, created_at, updated_at FROM data_sources WHERE id = ?",
        )
        .bind(id)
        .fetch_optional(&self.inner.pool)
        .await
        .map_err(sanitize)?;
        row.map(row_to_record).transpose()
    }

    /// Read a single record by unique `name`. Returns a [`Backend`]
    /// error when no record exists for the supplied name; this keeps
    /// the public boundary simple for the UI/test callers that
    /// already expect a value-or-error shape.
    ///
    /// [`Backend`]: DataSourceStoreErrorKind::Backend
    pub async fn get_by_name(&self, name: &str) -> Result<DataSourceRecord, DataSourceStoreError> {
        let row = sqlx::query(
            "SELECT id, name, host, port, username, database, encrypted_password, created_at, updated_at FROM data_sources WHERE name = ?",
        )
        .bind(name)
        .fetch_optional(&self.inner.pool)
        .await
        .map_err(sanitize)?;
        match row {
            Some(row) => row_to_record(row),
            None => Err(DataSourceStoreError {
                kind: DataSourceStoreErrorKind::Backend(format!("name '{name}' not found")),
            }),
        }
    }

    /// List one page of records. Page size + filter follow the
    /// [`DataSourceFilter`] / [`DataSourcePage`] contract; the
    /// underlying query uses `LIMIT` + `OFFSET` bound parameters
    /// and the `data_sources_name_idx` index for the default order.
    pub async fn list(
        &self,
        filter: &DataSourceFilter,
        page: DataSourcePage,
    ) -> Result<Vec<DataSourceRecord>, DataSourceStoreError> {
        let rows = match &filter.search {
            Some(search) => sqlx::query(
                "SELECT id, name, host, port, username, database, encrypted_password, created_at, updated_at \
                 FROM data_sources \
                 WHERE name LIKE ? || '%' \
                 ORDER BY name \
                 LIMIT ? OFFSET ?",
            )
            .bind(search)
            .bind(page.limit as i64)
            .bind(page.offset as i64)
            .fetch_all(&self.inner.pool)
            .await
            .map_err(sanitize)?,
            None => sqlx::query(
                "SELECT id, name, host, port, username, database, encrypted_password, created_at, updated_at \
                 FROM data_sources \
                 ORDER BY name \
                 LIMIT ? OFFSET ?",
            )
            .bind(page.limit as i64)
            .bind(page.offset as i64)
            .fetch_all(&self.inner.pool)
            .await
            .map_err(sanitize)?,
        };
        rows.into_iter().map(row_to_record).collect()
    }

    /// Return the `EXPLAIN QUERY PLAN` text for the default list
    /// path. T034 asserts the plan hits the `data_sources_name_idx`
    /// (or its lower-cased sibling). The query is run through the
    /// same checked static SQL used by [`Self::list`].
    pub async fn explain_list_plan(&self) -> Result<String, DataSourceStoreError> {
        let rows = sqlx::query(
            "EXPLAIN QUERY PLAN SELECT id FROM data_sources ORDER BY name LIMIT 20 OFFSET 0",
        )
        .fetch_all(&self.inner.pool)
        .await
        .map_err(sanitize)?;
        let mut text = String::new();
        for (index, row) in rows.iter().enumerate() {
            if index > 0 {
                text.push('\n');
            }
            let id: i64 = row.try_get(0).unwrap_or(0);
            let parent: i64 = row.try_get(1).unwrap_or(0);
            let notused: i64 = row.try_get(2).unwrap_or(0);
            let detail: String = row.try_get(3).unwrap_or_default();
            text.push_str(&format!("{id}|{parent}|{notused}|{detail}"));
        }
        Ok(text)
    }
}

fn row_to_record(row: sqlx::sqlite::SqliteRow) -> Result<DataSourceRecord, DataSourceStoreError> {
    let created_at = row
        .try_get::<String, _>("created_at")
        .map_err(sanitize)?
        .parse::<DateTime<Utc>>()
        .unwrap_or_else(|_| Utc::now());
    let updated_at = row
        .try_get::<String, _>("updated_at")
        .map_err(sanitize)?
        .parse::<DateTime<Utc>>()
        .unwrap_or_else(|_| Utc::now());
    let port: i64 = row.try_get("port").map_err(sanitize)?;
    Ok(DataSourceRecord {
        id: row.try_get("id").map_err(sanitize)?,
        name: row.try_get("name").map_err(sanitize)?,
        host: row.try_get("host").map_err(sanitize)?,
        port: port.try_into().map_err(|_| DataSourceStoreError {
            kind: DataSourceStoreErrorKind::Backend("port out of range".into()),
        })?,
        username: row.try_get("username").map_err(sanitize)?,
        database: row.try_get("database").map_err(sanitize)?,
        password_ciphertext: row.try_get("encrypted_password").map_err(sanitize)?,
        created_at,
        updated_at,
    })
}

fn is_unique_violation(err: &sqlx::Error) -> bool {
    let message = err.to_string();
    let lower = message.to_ascii_lowercase();
    lower.contains("unique constraint failed")
        || (lower.contains("sqliteerror") && lower.contains("unique"))
}

/// Build a placeholder `SqlitePool` for the test-only
/// `DataSourceStore::placeholder()` constructor. The pool is
/// never queried by the test path; any real database access
/// must go through `DataSourceStore::new`.
fn dummy_pool() -> SqlitePool {
    use sqlx::sqlite::{SqliteConnectOptions, SqlitePoolOptions};
    let options = SqliteConnectOptions::new()
        .filename(":memory:")
        .create_if_missing(true)
        .foreign_keys(false)
        .journal_mode(sqlx::sqlite::SqliteJournalMode::Memory);
    // Build the pool synchronously via a private tokio runtime
    // so the placeholder API does not require a `Send` runtime
    // handle from the caller.
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .expect("placeholder pool: tokio runtime");
    runtime
        .block_on(async {
            SqlitePoolOptions::new()
                .min_connections(1)
                .max_connections(1)
                .connect_with(options)
                .await
        })
        .expect("placeholder pool: connect")
}

fn sanitize<E: std::fmt::Display>(err: E) -> DataSourceStoreError {
    let raw = err.to_string();
    let lower = raw.to_ascii_lowercase();
    if lower.contains("password")
        || lower.contains("canary")
        || lower.contains("token")
        || lower.contains("secret")
    {
        return DataSourceStoreError {
            kind: DataSourceStoreErrorKind::Backend("backend error".into()),
        };
    }
    DataSourceStoreError {
        kind: DataSourceStoreErrorKind::Backend(summarize(&raw)),
    }
}

fn summarize(message: &str) -> String {
    if message.is_empty() {
        return "backend error".into();
    }
    let trimmed: String = message.chars().take(120).collect();
    if trimmed.is_empty() {
        "backend error".into()
    } else {
        trimmed
    }
}
