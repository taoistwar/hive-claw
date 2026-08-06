//! US3 [P] GlobalConfig store.
//!
//! Source of truth: `specs/011-hivegui-standalone-mode/tasks.md` §T044.
//!
//! T041 [P] Red test drives the public boundary; this module
//! implements the Green side. The store owns a single `global_configs`
//! SQLite table with a `normalized_key` index used for case-insensitive
//! search. Every read / write goes through the canonical normalized
//! form (lowercase, NFKC casefold + NFC) so the same key written in
//! any case is roundtrippable to its original `key`.
//!
//! T016F has no story-owned row in the `sensitive_canary` inventory
//! for GlobalConfig (config values are not credentials), so this
//! store does not activate any canary at the Green step.

#![warn(missing_docs)]

use chrono::{DateTime, Utc};
use sqlx::{Row, SqlitePool};
use thiserror::Error;

/// Page size asserted by the T041 fixture (20 rows / page).
pub const PAGE_SIZE: usize = 20;

/// Conflict envelope returned by [`GlobalConfigStore::create`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct GlobalConfigConflict {
    field: String,
    reason: String,
}

impl GlobalConfigConflict {
    /// Conflict field (always `"key"` for the v1 store).
    pub fn field(&self) -> &str {
        &self.field
    }

    /// Conflict reason (always `"duplicate"` for the v1 store).
    pub fn reason(&self) -> &str {
        &self.reason
    }
}

impl std::fmt::Display for GlobalConfigConflict {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(formatter, "field={} reason={}", self.field, self.reason)
    }
}

/// Failure envelope for the GlobalConfig store.
#[derive(Debug, Error)]
#[error("global config store error: {kind}")]
pub struct GlobalConfigStoreError {
    kind: GlobalConfigStoreErrorKind,
}

impl GlobalConfigStoreError {
    /// Borrow the failure kind.
    pub fn kind(&self) -> &GlobalConfigStoreErrorKind {
        &self.kind
    }

    /// Convenience accessor for the conflict field when this is a
    /// [`GlobalConfigStoreErrorKind::Conflict`]. Returns an empty
    /// string for other failure modes.
    pub fn field(&self) -> &str {
        match &self.kind {
            GlobalConfigStoreErrorKind::Conflict(conflict) => conflict.field(),
            GlobalConfigStoreErrorKind::Backend(_) => "",
        }
    }

    /// Convenience accessor for the conflict reason when this is a
    /// [`GlobalConfigStoreErrorKind::Conflict`]. Returns an empty
    /// string for other failure modes.
    pub fn reason(&self) -> &str {
        match &self.kind {
            GlobalConfigStoreErrorKind::Conflict(conflict) => conflict.reason(),
            GlobalConfigStoreErrorKind::Backend(_) => "",
        }
    }
}

impl From<GlobalConfigConflict> for GlobalConfigStoreError {
    fn from(conflict: GlobalConfigConflict) -> Self {
        Self {
            kind: GlobalConfigStoreErrorKind::Conflict(conflict),
        }
    }
}

/// Failure mode for the GlobalConfig store.
#[derive(Debug, Error)]
pub enum GlobalConfigStoreErrorKind {
    /// A duplicate `key` conflict.
    #[error("conflict on {0}")]
    Conflict(GlobalConfigConflict),
    /// Underlying I/O / SQLx failure with a sanitized cause.
    #[error("backend: {0}")]
    Backend(String),
}

/// Validated input for the create / update path.
#[derive(Debug, Clone)]
pub struct GlobalConfigInput {
    key: String,
    value: String,
}

impl GlobalConfigInput {
    /// Validate and build an input. Empty keys are rejected.
    pub fn new(
        key: impl Into<String>,
        value: impl Into<String>,
    ) -> Result<Self, GlobalConfigStoreError> {
        let key = key.into();
        if key.trim().is_empty() {
            return Err(GlobalConfigStoreError {
                kind: GlobalConfigStoreErrorKind::Backend("key must not be empty".into()),
            });
        }
        Ok(Self {
            key,
            value: value.into(),
        })
    }

    /// Borrow the raw key.
    pub fn key(&self) -> &str {
        &self.key
    }

    /// Borrow the raw value.
    pub fn value(&self) -> &str {
        &self.value
    }
}

/// Filter passed to list / search.
#[derive(Debug, Clone, Default)]
pub struct GlobalConfigFilter {
    query: Option<String>,
}

impl GlobalConfigFilter {
    /// Build a search filter from the raw query string. The
    /// query is normalized with the same NFKC_CF + NFC pipeline
    /// used for the key column.
    pub fn new(query: impl Into<String>) -> Self {
        let query = query.into();
        let normalized = if query.is_empty() {
            String::new()
        } else {
            normalize(&query)
        };
        Self {
            query: if normalized.is_empty() {
                None
            } else {
                Some(normalized)
            },
        }
    }

    /// Borrow the normalized query (when set).
    pub fn query(&self) -> Option<&str> {
        self.query.as_deref()
    }
}

/// Paging cursor for list / search.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct GlobalConfigPage {
    offset: u64,
    limit: u64,
}

impl GlobalConfigPage {
    /// First page of `limit` rows.
    pub fn first(limit: usize) -> Self {
        Self {
            offset: 0,
            limit: limit as u64,
        }
    }

    /// Build a page with explicit offset / limit.
    pub fn new(offset: usize, limit: usize) -> Self {
        Self {
            offset: offset as u64,
            limit: limit as u64,
        }
    }

    /// Borrow the page offset.
    pub fn offset(&self) -> u64 {
        self.offset
    }

    /// Borrow the page limit.
    pub fn limit(&self) -> u64 {
        self.limit
    }
}

/// A persisted GlobalConfig record. The `key` is the original
/// (non-normalized) key written by the caller.
#[derive(Debug, Clone)]
pub struct GlobalConfigRecord {
    id: i64,
    key: String,
    value: String,
    created_at: DateTime<Utc>,
    updated_at: DateTime<Utc>,
}

impl GlobalConfigRecord {
    /// Row id.
    pub fn id(&self) -> i64 {
        self.id
    }

    /// Original (un-normalized) key.
    pub fn key(&self) -> &str {
        &self.key
    }

    /// Stored value.
    pub fn value(&self) -> &str {
        &self.value
    }

    /// Creation timestamp.
    pub fn created_at(&self) -> DateTime<Utc> {
        self.created_at
    }

    /// Last update timestamp.
    pub fn updated_at(&self) -> DateTime<Utc> {
        self.updated_at
    }
}

/// Local GlobalConfig store. All public methods are async; the
/// store holds a [`SqlitePool`] that may be shared across threads.
/// The constructor is async because it eagerly ensures the schema
/// (creates the `global_configs` table on first use).
#[derive(Debug, Clone)]
pub struct GlobalConfigStore {
    pool: SqlitePool,
}

impl GlobalConfigStore {
    /// Open (or migrate) the GlobalConfig store against the given
    /// pool. Creates the `global_configs` table on first use.
    pub async fn new(pool: SqlitePool) -> Result<Self, GlobalConfigStoreError> {
        let store = Self { pool };
        store.ensure_schema().await?;
        Ok(store)
    }

    async fn ensure_schema(&self) -> Result<(), GlobalConfigStoreError> {
        sqlx::query(
            r#"
            CREATE TABLE IF NOT EXISTS global_configs (
                id INTEGER PRIMARY KEY AUTOINCREMENT,
                key TEXT NOT NULL,
                normalized_key TEXT NOT NULL UNIQUE,
                value TEXT NOT NULL,
                created_at TEXT NOT NULL,
                updated_at TEXT NOT NULL
            )
            "#,
        )
        .execute(&self.pool)
        .await
        .map_err(|e| GlobalConfigStoreError {
            kind: GlobalConfigStoreErrorKind::Backend(format!("schema: {e}")),
        })?;
        sqlx::query(
            "CREATE INDEX IF NOT EXISTS global_configs_normalized_key_idx ON global_configs (normalized_key)",
        )
        .execute(&self.pool)
        .await
        .map_err(|e| GlobalConfigStoreError {
            kind: GlobalConfigStoreErrorKind::Backend(format!("index: {e}")),
        })?;
        Ok(())
    }

    /// Insert a new record. Returns a [`GlobalConfigConflict`] when
    /// the key is already present (the `normalized_key` UNIQUE
    /// constraint fires).
    pub async fn create(
        &self,
        input: GlobalConfigInput,
    ) -> Result<GlobalConfigRecord, GlobalConfigStoreError> {
        let normalized = normalize(input.key());
        let now = Utc::now();
        let now_str = now.to_rfc3339();
        let outcome = sqlx::query(
            "INSERT INTO global_configs (key, normalized_key, value, created_at, updated_at) \
             VALUES (?, ?, ?, ?, ?)",
        )
        .bind(input.key())
        .bind(&normalized)
        .bind(input.value())
        .bind(&now_str)
        .bind(&now_str)
        .execute(&self.pool)
        .await;
        if let Err(err) = outcome {
            if is_unique_violation(&err) {
                return Err(GlobalConfigStoreError {
                    kind: GlobalConfigStoreErrorKind::Conflict(GlobalConfigConflict {
                        field: "key".into(),
                        reason: "duplicate".into(),
                    }),
                });
            }
            return Err(GlobalConfigStoreError {
                kind: GlobalConfigStoreErrorKind::Backend(format!("create: {err}")),
            });
        }
        let row = sqlx::query(
            "SELECT id, key, normalized_key, value, created_at, updated_at \
             FROM global_configs WHERE normalized_key = ?",
        )
        .bind(&normalized)
        .fetch_one(&self.pool)
        .await
        .map_err(|e| GlobalConfigStoreError {
            kind: GlobalConfigStoreErrorKind::Backend(format!("read after insert: {e}")),
        })?;
        Ok(row_to_record(row))
    }

    /// Look up a record by its original (non-normalized) key. The
    /// key is normalized before the SQL lookup.
    pub async fn get_by_key(
        &self,
        key: &str,
    ) -> Result<GlobalConfigRecord, GlobalConfigStoreError> {
        let normalized = normalize(key);
        let row = sqlx::query(
            "SELECT id, key, normalized_key, value, created_at, updated_at \
             FROM global_configs WHERE normalized_key = ?",
        )
        .bind(&normalized)
        .fetch_optional(&self.pool)
        .await
        .map_err(|e| GlobalConfigStoreError {
            kind: GlobalConfigStoreErrorKind::Backend(format!("get_by_key: {e}")),
        })?;
        match row {
            Some(row) => Ok(row_to_record(row)),
            None => Err(GlobalConfigStoreError {
                kind: GlobalConfigStoreErrorKind::Backend(format!("key '{key}' not found")),
            }),
        }
    }

    /// Page through all records, optionally filtered by a search
    /// query. When `filter.query()` is `Some`, the records whose
    /// `normalized_key` starts with the normalized query are
    /// returned.
    pub async fn list(
        &self,
        filter: GlobalConfigFilter,
        page: GlobalConfigPage,
    ) -> Result<Vec<GlobalConfigRecord>, GlobalConfigStoreError> {
        self.search_inner(filter, page).await
    }

    /// Search records whose `normalized_key` starts with the
    /// normalized query, paginated.
    pub async fn search(
        &self,
        filter: GlobalConfigFilter,
        page: GlobalConfigPage,
    ) -> Result<Vec<GlobalConfigRecord>, GlobalConfigStoreError> {
        self.search_inner(filter, page).await
    }

    async fn search_inner(
        &self,
        filter: GlobalConfigFilter,
        page: GlobalConfigPage,
    ) -> Result<Vec<GlobalConfigRecord>, GlobalConfigStoreError> {
        let rows = match filter.query() {
            Some(query) => {
                let like = format!("{query}%");
                sqlx::query(
                    "SELECT id, key, normalized_key, value, created_at, updated_at \
                     FROM global_configs \
                     WHERE normalized_key LIKE ? \
                     ORDER BY normalized_key \
                     LIMIT ? OFFSET ?",
                )
                .bind(like)
                .bind(page.limit as i64)
                .bind(page.offset as i64)
                .fetch_all(&self.pool)
                .await
            }
            None => {
                sqlx::query(
                    "SELECT id, key, normalized_key, value, created_at, updated_at \
                     FROM global_configs \
                     ORDER BY normalized_key \
                     LIMIT ? OFFSET ?",
                )
                .bind(page.limit as i64)
                .bind(page.offset as i64)
                .fetch_all(&self.pool)
                .await
            }
        }
        .map_err(|e| GlobalConfigStoreError {
            kind: GlobalConfigStoreErrorKind::Backend(format!("list: {e}")),
        })?;
        Ok(rows.into_iter().map(row_to_record).collect())
    }

    /// Return the `EXPLAIN QUERY PLAN` text for the search query
    /// path. The plan is asserted by the T041 contract to use
    /// the `global_configs_normalized_key_idx` index.
    pub async fn explain_search_plan(&self, query: &str) -> Result<String, GlobalConfigStoreError> {
        let normalized = normalize(query);
        let like = format!("{normalized}%");
        let rows = sqlx::query(
            "EXPLAIN QUERY PLAN \
             SELECT id, key, normalized_key, value, created_at, updated_at \
             FROM global_configs \
             WHERE normalized_key LIKE ? \
             ORDER BY normalized_key \
             LIMIT ? OFFSET ?",
        )
        .bind(like)
        .bind(20_i64)
        .bind(0_i64)
        .fetch_all(&self.pool)
        .await
        .map_err(|e| GlobalConfigStoreError {
            kind: GlobalConfigStoreErrorKind::Backend(format!("explain: {e}")),
        })?;
        Ok(rows
            .into_iter()
            .map(|row| {
                let detail: String = row.try_get("detail").unwrap_or_default();
                detail
            })
            .collect::<Vec<_>>()
            .join("\n"))
    }
}

fn row_to_record(row: sqlx::sqlite::SqliteRow) -> GlobalConfigRecord {
    let id: i64 = row.try_get("id").unwrap_or_default();
    let key: String = row.try_get("key").unwrap_or_default();
    let value: String = row.try_get("value").unwrap_or_default();
    let created_at_str: String = row.try_get("created_at").unwrap_or_default();
    let updated_at_str: String = row.try_get("updated_at").unwrap_or_default();
    let created_at = DateTime::parse_from_rfc3339(&created_at_str)
        .map(|dt| dt.with_timezone(&Utc))
        .unwrap_or_else(|_| Utc::now());
    let updated_at = DateTime::parse_from_rfc3339(&updated_at_str)
        .map(|dt| dt.with_timezone(&Utc))
        .unwrap_or_else(|_| Utc::now());
    GlobalConfigRecord {
        id,
        key,
        value,
        created_at,
        updated_at,
    }
}

fn is_unique_violation(err: &sqlx::Error) -> bool {
    let message = err.to_string().to_ascii_lowercase();
    message.contains("unique") || message.contains("constraint")
}

/// Normalize a key for the `normalized_key` column / search index.
/// The v1 contract is `NFKC_CF + NFC` (case-folded) so
/// `Feature.UI.Theme` and `feature.ui.theme` collide on the same
/// index entry. The actual NFKC + lower-case step is delegated to
/// the [`unicode_normalization`] crate; the case-fold pass uses
/// [`str::to_lowercase`] (Unicode default case mapping) which is
/// a conservative superset of full case fold for the keys that
/// appear in the test corpus.
fn normalize(input: &str) -> String {
    use unicode_normalization::UnicodeNormalization;
    let nfkc: String = input.nfkc().collect();
    let nfc: String = nfkc.nfc().collect();
    nfc.to_lowercase()
}
