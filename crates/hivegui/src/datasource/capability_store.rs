//! US7 [P] Capability management store — name-unique, NFKC + case-fold
//! normalized, paginated, with `EXPLAIN QUERY PLAN` index contract.
//!
//! Source of truth: `specs/011-hivegui-standalone-mode/tasks.md`
//! §T066 / §T069. The public boundary the T066 Red test drives:
//!
//!   - [`CapabilityStore::new`]
//!   - [`CapabilityStore::create`]
//!   - [`CapabilityStore::search`]
//!   - [`CapabilityStore::count`]
//!   - [`CapabilityStore::explain_search_plan`]
//!   - [`CapabilityInput::new`]
//!   - [`CapabilityFilter::new`], [`CapabilityFilter::default`]
//!   - [`CapabilityPage::first`]
//!   - [`CapabilityRecord::name`], [`CapabilityRecord::is_dangerous`]
//!   - [`CapabilityStoreError::field`]
//!
//! T069 produces this module. T016E `CapabilityList` scroll surface
//! is activated by T067A / T068 (T069 closes it). The store uses
//! the existing `capabilities` table; the only schema delta T066
//! requires is the `capabilities_normalized_name_idx` index used by
//! the EXPLAIN assertion.

#![warn(missing_docs)]

use chrono::{DateTime, Utc};
use sqlx::{Row, SqlitePool};
use thiserror::Error;

/// Stable failure envelope returned by every Capability write / search
/// path. Each variant carries only safe values; UI MUST never
/// receive raw SQL error strings.
#[derive(Debug, Error)]
#[error("capability store error: {kind:?}")]
pub struct CapabilityStoreError {
    /// Error variant.
    pub kind: CapabilityStoreErrorKind,
}

impl CapabilityStoreError {
    /// Field that triggered the failure, when known. Returns the
    /// conflict field for [`CapabilityStoreErrorKind::Conflict`], the
    /// validation field for invalid input, or an empty string for
    /// backend errors.
    pub fn field(&self) -> &str {
        match &self.kind {
            CapabilityStoreErrorKind::Conflict(conflict) => conflict.field(),
            CapabilityStoreErrorKind::InvalidInput { field, .. } => field.as_str(),
            CapabilityStoreErrorKind::Backend(_) => "",
        }
    }
}

/// Failure mode for the Capability store.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CapabilityStoreErrorKind {
    /// A `name` conflict rejection.
    Conflict(CapabilityConflict),
    /// Input failed the local validation step.
    InvalidInput {
        /// Field that failed validation.
        field: String,
        /// Stable reason code.
        reason: String,
    },
    /// Underlying I/O / SQLx failure with a sanitized cause.
    Backend(String),
}

/// Conflict envelope returned to the caller. The struct
/// intentionally keeps only safe values (no SQL fragments, no raw
/// row content).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CapabilityConflict {
    field: String,
    reason: String,
}

impl CapabilityConflict {
    /// Conflict field (e.g. `"name"`).
    pub fn field(&self) -> &str {
        &self.field
    }

    /// Conflict reason code (e.g. `"duplicate"`).
    pub fn reason(&self) -> &str {
        &self.reason
    }
}

impl From<CapabilityConflict> for CapabilityStoreError {
    fn from(conflict: CapabilityConflict) -> Self {
        Self {
            kind: CapabilityStoreErrorKind::Conflict(conflict),
        }
    }
}

/// Validated input for the create / update path.
#[derive(Debug, Clone)]
pub struct CapabilityInput {
    name: String,
    description: String,
    is_dangerous: bool,
    category_id: Option<i64>,
}

impl CapabilityInput {
    /// Validate and build a new-record input. The `name` MUST match
    /// `[a-z][a-z0-9._-]{1,63}` (lowercase identifier); the
    /// `description` must be non-empty; `is_dangerous` is a flag
    /// surfaced through the UI as a non-color marker.
    pub fn new(
        name: impl Into<String>,
        description: impl Into<String>,
        is_dangerous: bool,
    ) -> Result<Self, CapabilityStoreError> {
        let name = name.into();
        let description = description.into();
        if !is_valid_capability_name(&name) {
            return Err(CapabilityStoreError {
                kind: CapabilityStoreErrorKind::InvalidInput {
                    field: "name".into(),
                    reason: "must match [a-z][a-z0-9._-]{1,63}".into(),
                },
            });
        }
        if description.trim().is_empty() {
            return Err(CapabilityStoreError {
                kind: CapabilityStoreErrorKind::InvalidInput {
                    field: "description".into(),
                    reason: "must not be empty".into(),
                },
            });
        }
        Ok(Self {
            name,
            description,
            is_dangerous,
            category_id: None,
        })
    }

    /// Set the optional `category_id` (used for `category_id` SET
    /// NULL on parent delete).
    pub fn with_category_id(mut self, category_id: Option<i64>) -> Self {
        self.category_id = category_id;
        self
    }

    /// Borrow the capability name.
    pub fn name(&self) -> &str {
        &self.name
    }

    /// Borrow the description.
    pub fn description(&self) -> &str {
        &self.description
    }

    /// Borrow the dangerous flag.
    pub fn is_dangerous(&self) -> bool {
        self.is_dangerous
    }

    /// Borrow the category id, when set.
    pub fn category_id(&self) -> Option<i64> {
        self.category_id
    }
}

/// Filter passed to list / search.
#[derive(Debug, Clone, Default)]
pub struct CapabilityFilter {
    query: Option<String>,
}

impl CapabilityFilter {
    /// Build a search filter from the raw query string. The
    /// query is normalized with the same NFKC + lower-case
    /// pipeline used for the `name` column.
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
pub struct CapabilityPage {
    offset: u64,
    limit: u64,
}

impl CapabilityPage {
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

/// A persisted Capability record.
#[derive(Debug, Clone)]
pub struct CapabilityRecord {
    name: String,
    description: String,
    is_dangerous: bool,
    category_id: Option<i64>,
    created_at: DateTime<Utc>,
}

impl CapabilityRecord {
    /// Capability primary key (the unique name).
    pub fn name(&self) -> &str {
        &self.name
    }

    /// Borrow the description.
    pub fn description(&self) -> &str {
        &self.description
    }

    /// Whether the capability is flagged dangerous.
    pub fn is_dangerous(&self) -> bool {
        self.is_dangerous
    }

    /// Optional owning category id.
    pub fn category_id(&self) -> Option<i64> {
        self.category_id
    }

    /// Creation timestamp.
    pub fn created_at(&self) -> DateTime<Utc> {
        self.created_at
    }
}

/// Local Capability store. The constructor is async because it
/// eagerly ensures the `capabilities` table and the
/// `capabilities_normalized_name_idx` index on first use. The
/// runtime never has to migrate the schema from a cold start; an
/// empty pool is enough.
#[derive(Debug, Clone)]
pub struct CapabilityStore {
    pool: SqlitePool,
}

impl CapabilityStore {
    /// Open (or migrate) the Capability store against the given
    /// pool. Creates the `capabilities` table, the
    /// `normalized_name` column (for legacy v1 databases) and the
    /// `capabilities_normalized_name_idx` index on first use.
    pub async fn new(pool: SqlitePool) -> Result<Self, CapabilityStoreError> {
        let store = Self { pool };
        store.ensure_schema().await?;
        Ok(store)
    }

    /// Open the store synchronously from an already-migrated
    /// pool. Used by the [`crate::datasource::Store`] constructor
    /// after the global migrations have run; production callers
    /// should use [`CapabilityStore::new`].
    pub fn from_pool(pool: SqlitePool) -> Self {
        Self { pool }
    }

    async fn ensure_schema(&self) -> Result<(), CapabilityStoreError> {
        sqlx::query(
            r#"
            CREATE TABLE IF NOT EXISTS capabilities (
                name TEXT PRIMARY KEY,
                description TEXT NOT NULL DEFAULT '',
                is_dangerous INTEGER NOT NULL DEFAULT 0,
                category_id INTEGER,
                normalized_name TEXT NOT NULL DEFAULT '',
                created_at TEXT NOT NULL
            )
            "#,
        )
        .execute(&self.pool)
        .await
        .map_err(|e| CapabilityStoreError {
            kind: CapabilityStoreErrorKind::Backend(format!("schema: {e}")),
        })?;

        let columns = sqlx::query("SELECT name FROM pragma_table_info('capabilities')")
            .fetch_all(&self.pool)
            .await
            .map_err(|e| CapabilityStoreError {
                kind: CapabilityStoreErrorKind::Backend(format!("pragma: {e}")),
            })?;
        let has_normalized = columns
            .iter()
            .any(|row| row.try_get::<String, _>("name").unwrap_or_default() == "normalized_name");
        if !has_normalized {
            sqlx::query(
                "ALTER TABLE capabilities ADD COLUMN normalized_name TEXT NOT NULL DEFAULT ''",
            )
            .execute(&self.pool)
            .await
            .map_err(|e| CapabilityStoreError {
                kind: CapabilityStoreErrorKind::Backend(format!("add normalized_name: {e}")),
            })?;
        }

        sqlx::query(
            "UPDATE capabilities SET normalized_name = LOWER(name) \
             WHERE normalized_name = '' OR normalized_name IS NULL",
        )
        .execute(&self.pool)
        .await
        .map_err(|e| CapabilityStoreError {
            kind: CapabilityStoreErrorKind::Backend(format!("backfill normalized_name: {e}")),
        })?;

        sqlx::query("DROP INDEX IF EXISTS idx_capabilities_name")
            .execute(&self.pool)
            .await
            .map_err(|e| CapabilityStoreError {
                kind: CapabilityStoreErrorKind::Backend(format!("drop legacy idx: {e}")),
            })?;
        sqlx::query(
            "CREATE INDEX IF NOT EXISTS capabilities_normalized_name_idx \
             ON capabilities (normalized_name)",
        )
        .execute(&self.pool)
        .await
        .map_err(|e| CapabilityStoreError {
            kind: CapabilityStoreErrorKind::Backend(format!("create index: {e}")),
        })?;
        Ok(())
    }

    /// Borrow the underlying pool. Reserved for callers that
    /// need to compose queries across stores without re-opening
    /// the database.
    pub fn pool(&self) -> &SqlitePool {
        &self.pool
    }

    /// Insert a new Capability record. Returns
    /// [`CapabilityStoreErrorKind::Conflict`] when the `name`
    /// collides with an existing row (the `name` PRIMARY KEY fires).
    pub async fn create(
        &self,
        input: CapabilityInput,
    ) -> Result<CapabilityRecord, CapabilityStoreError> {
        let now = Utc::now();
        let now_str = now.to_rfc3339();
        let normalized_name = normalize(input.name());
        let outcome = sqlx::query(
            "INSERT INTO capabilities \
                (name, description, is_dangerous, category_id, normalized_name, created_at) \
             VALUES (?, ?, ?, ?, ?, ?)",
        )
        .bind(input.name())
        .bind(input.description())
        .bind(input.is_dangerous() as i64)
        .bind(input.category_id())
        .bind(&normalized_name)
        .bind(&now_str)
        .execute(&self.pool)
        .await;

        if let Err(err) = outcome {
            if is_unique_violation(&err) {
                return Err(CapabilityStoreError {
                    kind: CapabilityStoreErrorKind::Conflict(CapabilityConflict {
                        field: "name".into(),
                        reason: "duplicate".into(),
                    }),
                });
            }
            return Err(CapabilityStoreError {
                kind: CapabilityStoreErrorKind::Backend(format!("create: {err}")),
            });
        }

        let row = sqlx::query(
            "SELECT name, description, is_dangerous, category_id, normalized_name, created_at \
             FROM capabilities WHERE name = ?",
        )
        .bind(input.name())
        .fetch_one(&self.pool)
        .await
        .map_err(|e| CapabilityStoreError {
            kind: CapabilityStoreErrorKind::Backend(format!("read after insert: {e}")),
        })?;
        Ok(row_to_record(row))
    }

    /// Page through all records, optionally filtered by a search
    /// query. When `filter.query()` is `Some`, the records whose
    /// `normalized_name` starts with the normalized query are
    /// returned.
    pub async fn search(
        &self,
        filter: CapabilityFilter,
        page: CapabilityPage,
    ) -> Result<Vec<CapabilityRecord>, CapabilityStoreError> {
        let rows = match filter.query() {
            Some(query) => {
                let like = format!("{query}%");
                sqlx::query(
                    "SELECT name, description, is_dangerous, category_id, normalized_name, created_at \
                     FROM capabilities \
                     WHERE normalized_name LIKE ? \
                     ORDER BY normalized_name \
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
                    "SELECT name, description, is_dangerous, category_id, normalized_name, created_at \
                     FROM capabilities \
                     ORDER BY normalized_name \
                     LIMIT ? OFFSET ?",
                )
                .bind(page.limit as i64)
                .bind(page.offset as i64)
                .fetch_all(&self.pool)
                .await
            }
        }
        .map_err(|e| CapabilityStoreError {
            kind: CapabilityStoreErrorKind::Backend(format!("search: {e}")),
        })?;
        Ok(rows.into_iter().map(row_to_record).collect())
    }

    /// Total number of records that match the given filter. The
    /// T066 contract uses this for the page-1-of-N indicator.
    pub async fn count(&self, filter: CapabilityFilter) -> Result<i64, CapabilityStoreError> {
        let count: i64 = match filter.query() {
            Some(query) => {
                let like = format!("{query}%");
                sqlx::query_scalar("SELECT COUNT(*) FROM capabilities WHERE normalized_name LIKE ?")
                    .bind(like)
                    .fetch_one(&self.pool)
                    .await
            }
            None => {
                sqlx::query_scalar("SELECT COUNT(*) FROM capabilities")
                    .fetch_one(&self.pool)
                    .await
            }
        }
        .map_err(|e| CapabilityStoreError {
            kind: CapabilityStoreErrorKind::Backend(format!("count: {e}")),
        })?;
        Ok(count)
    }

    /// Return the `EXPLAIN QUERY PLAN` text for the search query
    /// path. The T066 contract asserts the plan references the
    /// `capabilities_normalized_name_idx` index.
    pub async fn explain_search_plan(&self, query: &str) -> Result<String, CapabilityStoreError> {
        let normalized = normalize(query);
        let like = format!("{normalized}%");
        let rows = sqlx::query(
            "EXPLAIN QUERY PLAN \
             SELECT name, description, is_dangerous, category_id, normalized_name, created_at \
             FROM capabilities \
             WHERE normalized_name LIKE ? \
             ORDER BY normalized_name \
             LIMIT 20 OFFSET 0",
        )
        .bind(like)
        .fetch_all(&self.pool)
        .await
        .map_err(|e| CapabilityStoreError {
            kind: CapabilityStoreErrorKind::Backend(format!("explain: {e}")),
        })?;

        let mut plan = String::new();
        for row in rows {
            let detail: String = row.try_get("detail").unwrap_or_default();
            if !plan.is_empty() {
                plan.push('\n');
            }
            plan.push_str(&detail);
        }
        Ok(plan)
    }
}

fn row_to_record(row: sqlx::sqlite::SqliteRow) -> CapabilityRecord {
    let name: String = row.try_get("name").unwrap_or_default();
    let description: String = row.try_get("description").unwrap_or_default();
    let is_dangerous: i64 = row.try_get("is_dangerous").unwrap_or_default();
    let category_id: Option<i64> = row.try_get("category_id").ok().flatten();
    let created_at_str: String = row.try_get("created_at").unwrap_or_default();
    let created_at = DateTime::parse_from_rfc3339(&created_at_str)
        .map(|dt| dt.with_timezone(&Utc))
        .unwrap_or_else(|_| Utc::now());
    CapabilityRecord {
        name,
        description,
        is_dangerous: is_dangerous != 0,
        category_id,
        created_at,
    }
}

fn is_unique_violation(err: &sqlx::Error) -> bool {
    let message = err.to_string().to_ascii_lowercase();
    message.contains("unique") || message.contains("constraint")
}

/// Normalize a Capability name (or query) for the
/// `normalized_name` index lookup. The pipeline matches the
/// T017G / T022 contract: NFKC + lower-case. Empty input returns
/// an empty string so the caller can decide whether to skip the
/// filter.
fn normalize(input: &str) -> String {
    use unicode_normalization::UnicodeNormalization;
    let nfkc: String = input.nfkc().collect();
    let nfc: String = nfkc.nfc().collect();
    nfc.to_lowercase()
}

/// Validate a Capability `name` against the T066 contract:
/// `[a-z][a-z0-9._-]{1,63}`. The first character must be a
/// lowercase letter; subsequent characters may include digits,
/// `.`, `_` or `-`. Length is between 2 and 64 characters.
fn is_valid_capability_name(name: &str) -> bool {
    if name.len() < 2 || name.len() > 64 {
        return false;
    }
    let mut chars = name.chars();
    let first = chars.next().expect("len checked above");
    if !first.is_ascii_lowercase() {
        return false;
    }
    chars.all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '.' || c == '_' || c == '-')
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn normalize_passes_through_ascii_lower_case() {
        assert_eq!(normalize("HTTP_GET"), "http_get");
        assert_eq!(normalize("net.HTTP"), "net.http");
    }

    #[test]
    fn is_valid_capability_name_accepts_lowercase_identifier() {
        assert!(is_valid_capability_name("http"));
        assert!(is_valid_capability_name("fs.read"));
        assert!(is_valid_capability_name("net_http-get.v2"));
    }

    #[test]
    fn is_valid_capability_name_rejects_invalid_forms() {
        assert!(!is_valid_capability_name(""));
        assert!(!is_valid_capability_name("HTTP")); // uppercase
        assert!(!is_valid_capability_name(".dot")); // leading dot
        assert!(!is_valid_capability_name("space name")); // contains space
        assert!(!is_valid_capability_name("中文")); // non-ASCII
    }
}
