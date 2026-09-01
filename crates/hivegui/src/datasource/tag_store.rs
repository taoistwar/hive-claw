//! US5 [P] Tag management store — name-unique, NFKC + case-fold
//! normalized, paginated, with `EXPLAIN QUERY PLAN` index contract.
//!
//! Source of truth: `specs/011-hivegui-standalone-mode/tasks.md`
//! §T055 / §T058. The public boundary the T055 Red test drives:
//!
//!   - [`TagStore::new`]
//!   - [`TagStore::create`]
//!   - [`TagStore::search`]
//!   - [`TagStore::explain_search_plan`]
//!   - [`TagInput::new`]
//!   - [`TagFilter::new`], [`TagFilter::default`]
//!   - [`TagPage::first`]
//!   - [`TagRecord::name`]
//!   - [`TagStoreError::field`]
//!
//! T058 produces this module. T016E `TagList` scroll surface is
//! activated by T056 / T057 (T058 closes it). The store uses the
//! existing `tags` table from the global migrations; the only
//! schema delta the T055 contract requires is the
//! `tags_normalized_name_idx` index used by the EXPLAIN assertion.

#![warn(missing_docs)]

use chrono::{DateTime, Utc};
use sqlx::{Row, SqlitePool};
use thiserror::Error;

/// Stable failure envelope returned by every Tag write / search
/// path. Each variant carries only safe values; UI MUST never
/// receive raw SQL error strings.
#[derive(Debug, Error)]
#[error("tag store error: {kind:?}")]
pub struct TagStoreError {
    /// Error variant.
    pub kind: TagStoreErrorKind,
}

impl TagStoreError {
    /// Field that triggered the failure, when known. Returns the
    /// conflict field for [`TagStoreErrorKind::Conflict`], the
    /// validation field for invalid input, or an empty string for
    /// backend errors.
    pub fn field(&self) -> &str {
        match &self.kind {
            TagStoreErrorKind::Conflict(conflict) => conflict.field(),
            TagStoreErrorKind::InvalidInput { field, .. } => field.as_str(),
            TagStoreErrorKind::Backend(_) => "",
        }
    }
}

/// Failure mode for the Tag store.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TagStoreErrorKind {
    /// A `name` conflict rejection.
    Conflict(TagConflict),
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
pub struct TagConflict {
    field: String,
    reason: String,
}

impl TagConflict {
    /// Conflict field (e.g. `"name"`).
    pub fn field(&self) -> &str {
        &self.field
    }

    /// Conflict reason code (e.g. `"duplicate"`).
    pub fn reason(&self) -> &str {
        &self.reason
    }
}

impl From<TagConflict> for TagStoreError {
    fn from(conflict: TagConflict) -> Self {
        Self {
            kind: TagStoreErrorKind::Conflict(conflict),
        }
    }
}

/// Validated input for the create / update path.
#[derive(Debug, Clone)]
pub struct TagInput {
    name: String,
    color: Option<String>,
}

impl TagInput {
    /// Validate and build a new-record input. The `name` is
    /// required; the `color` is optional and when present must
    /// be a 7-character `#RRGGBB` hex literal.
    pub fn new(name: impl Into<String>, color: impl Into<String>) -> Result<Self, TagStoreError> {
        let name = name.into();
        let color = color.into();
        if name.trim().is_empty() {
            return Err(TagStoreError {
                kind: TagStoreErrorKind::InvalidInput {
                    field: "name".into(),
                    reason: "must not be empty".into(),
                },
            });
        }
        if !color.is_empty() && !is_valid_color(&color) {
            return Err(TagStoreError {
                kind: TagStoreErrorKind::InvalidInput {
                    field: "color".into(),
                    reason: "must be #RRGGBB".into(),
                },
            });
        }
        Ok(Self {
            name,
            color: if color.is_empty() { None } else { Some(color) },
        })
    }

    /// Borrow the tag display name.
    pub fn name(&self) -> &str {
        &self.name
    }

    /// Borrow the optional color.
    pub fn color(&self) -> Option<&str> {
        self.color.as_deref()
    }
}

/// Filter passed to list / search.
#[derive(Debug, Clone, Default)]
pub struct TagFilter {
    query: Option<String>,
}

impl TagFilter {
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
pub struct TagPage {
    offset: u64,
    limit: u64,
}

impl TagPage {
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

/// A persisted Tag record.
#[derive(Debug, Clone)]
pub struct TagRecord {
    id: i64,
    name: String,
    color: Option<String>,
    created_at: DateTime<Utc>,
    updated_at: DateTime<Utc>,
}

impl TagRecord {
    /// Row id.
    pub fn id(&self) -> i64 {
        self.id
    }

    /// Tag display name.
    pub fn name(&self) -> &str {
        &self.name
    }

    /// Optional color literal.
    pub fn color(&self) -> Option<&str> {
        self.color.as_deref()
    }

    /// Creation timestamp.
    pub fn created_at(&self) -> DateTime<Utc> {
        self.created_at
    }

    /// Last-update timestamp.
    pub fn updated_at(&self) -> DateTime<Utc> {
        self.updated_at
    }
}

/// Local Tag store. The constructor is async because it eagerly
/// ensures the `tags` table and the `tags_normalized_name_idx`
/// index on first use. The runtime never has to migrate the
/// schema from a cold start; an empty pool is enough.
#[derive(Debug, Clone)]
pub struct TagStore {
    pool: SqlitePool,
}

impl TagStore {
    /// Open (or migrate) the Tag store against the given pool.
    /// Creates the `tags` table, the `normalized_name` /
    /// `updated_at` columns (for legacy v1 databases) and the
    /// `tags_normalized_name_idx` index on first use.
    pub async fn new(pool: SqlitePool) -> Result<Self, TagStoreError> {
        let store = Self { pool };
        store.ensure_schema().await?;
        Ok(store)
    }

    async fn ensure_schema(&self) -> Result<(), TagStoreError> {
        sqlx::query(
            r#"
            CREATE TABLE IF NOT EXISTS tags (
                id INTEGER PRIMARY KEY AUTOINCREMENT,
                name TEXT NOT NULL UNIQUE,
                color TEXT,
                normalized_name TEXT NOT NULL DEFAULT '',
                created_at TEXT NOT NULL,
                updated_at TEXT NOT NULL DEFAULT ''
            )
            "#,
        )
        .execute(&self.pool)
        .await
        .map_err(|e| TagStoreError {
            kind: TagStoreErrorKind::Backend(format!("schema: {e}")),
        })?;

        // Idempotent column / index upgrades for legacy v1
        // databases. New databases skip these statements because
        // the table already carries the v2 columns.
        let columns = sqlx::query("SELECT name FROM pragma_table_info('tags')")
            .fetch_all(&self.pool)
            .await
            .map_err(|e| TagStoreError {
                kind: TagStoreErrorKind::Backend(format!("pragma: {e}")),
            })?;
        let has_normalized = columns
            .iter()
            .any(|row| row.try_get::<String, _>("name").unwrap_or_default() == "normalized_name");
        let has_updated_at = columns
            .iter()
            .any(|row| row.try_get::<String, _>("name").unwrap_or_default() == "updated_at");
        if !has_normalized {
            sqlx::query("ALTER TABLE tags ADD COLUMN normalized_name TEXT NOT NULL DEFAULT ''")
                .execute(&self.pool)
                .await
                .map_err(|e| TagStoreError {
                    kind: TagStoreErrorKind::Backend(format!("add normalized_name: {e}")),
                })?;
        }
        if !has_updated_at {
            sqlx::query("ALTER TABLE tags ADD COLUMN updated_at TEXT NOT NULL DEFAULT ''")
                .execute(&self.pool)
                .await
                .map_err(|e| TagStoreError {
                    kind: TagStoreErrorKind::Backend(format!("add updated_at: {e}")),
                })?;
        }
        sqlx::query(
            "UPDATE tags SET normalized_name = LOWER(name) \
             WHERE normalized_name = '' OR normalized_name IS NULL",
        )
        .execute(&self.pool)
        .await
        .map_err(|e| TagStoreError {
            kind: TagStoreErrorKind::Backend(format!("backfill normalized_name: {e}")),
        })?;
        sqlx::query("DROP INDEX IF EXISTS idx_tags_name")
            .execute(&self.pool)
            .await
            .map_err(|e| TagStoreError {
                kind: TagStoreErrorKind::Backend(format!("drop legacy idx: {e}")),
            })?;
        sqlx::query(
            "CREATE INDEX IF NOT EXISTS tags_normalized_name_idx ON tags (normalized_name)",
        )
        .execute(&self.pool)
        .await
        .map_err(|e| TagStoreError {
            kind: TagStoreErrorKind::Backend(format!("create index: {e}")),
        })?;
        Ok(())
    }

    /// Borrow the underlying pool. Reserved for callers that
    /// need to compose queries across stores without re-opening
    /// the database.
    pub fn pool(&self) -> &SqlitePool {
        &self.pool
    }

    /// Insert a new Tag record. Returns
    /// [`TagStoreErrorKind::Conflict`] when the `name` collides
    /// with an existing row (the `name` UNIQUE constraint fires).
    pub async fn create(&self, input: TagInput) -> Result<TagRecord, TagStoreError> {
        let now = Utc::now();
        let now_str = now.to_rfc3339();
        let normalized_name = normalize(input.name());
        let outcome = sqlx::query(
            "INSERT INTO tags (name, color, normalized_name, created_at, updated_at) \
             VALUES (?, ?, ?, ?, ?)",
        )
        .bind(input.name())
        .bind(input.color())
        .bind(&normalized_name)
        .bind(&now_str)
        .bind(&now_str)
        .execute(&self.pool)
        .await;

        if let Err(err) = outcome {
            if is_unique_violation(&err) {
                return Err(TagStoreError {
                    kind: TagStoreErrorKind::Conflict(TagConflict {
                        field: "name".into(),
                        reason: "duplicate".into(),
                    }),
                });
            }
            return Err(TagStoreError {
                kind: TagStoreErrorKind::Backend(format!("create: {err}")),
            });
        }

        let row = sqlx::query(
            "SELECT id, name, color, created_at, updated_at \
             FROM tags WHERE name = ?",
        )
        .bind(input.name())
        .fetch_one(&self.pool)
        .await
        .map_err(|e| TagStoreError {
            kind: TagStoreErrorKind::Backend(format!("read after insert: {e}")),
        })?;
        Ok(row_to_record(row))
    }

    /// Page through all records, optionally filtered by a search
    /// query. When `filter.query()` is `Some`, the records whose
    /// `normalized_name` starts with the normalized query are
    /// returned.
    pub async fn search(
        &self,
        filter: TagFilter,
        page: TagPage,
    ) -> Result<Vec<TagRecord>, TagStoreError> {
        let rows = match filter.query() {
            Some(query) => {
                let like = format!("{query}%");
                sqlx::query(
                    "SELECT id, name, color, created_at, updated_at \
                     FROM tags \
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
                    "SELECT id, name, color, created_at, updated_at \
                     FROM tags \
                     ORDER BY normalized_name \
                     LIMIT ? OFFSET ?",
                )
                .bind(page.limit as i64)
                .bind(page.offset as i64)
                .fetch_all(&self.pool)
                .await
            }
        }
        .map_err(|e| TagStoreError {
            kind: TagStoreErrorKind::Backend(format!("search: {e}")),
        })?;
        Ok(rows.into_iter().map(row_to_record).collect())
    }

    /// Return the `EXPLAIN QUERY PLAN` text for the search query
    /// path. The T055 contract asserts the plan references the
    /// `tags_normalized_name_idx` index.
    pub async fn explain_search_plan(&self, query: &str) -> Result<String, TagStoreError> {
        let normalized = normalize(query);
        let like = format!("{normalized}%");
        let rows = sqlx::query(
            "EXPLAIN QUERY PLAN \
             SELECT id, name, color, created_at, updated_at \
             FROM tags \
             WHERE normalized_name LIKE ? \
             ORDER BY normalized_name \
             LIMIT 20 OFFSET 0",
        )
        .bind(like)
        .fetch_all(&self.pool)
        .await
        .map_err(|e| TagStoreError {
            kind: TagStoreErrorKind::Backend(format!("explain: {e}")),
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

fn row_to_record(row: sqlx::sqlite::SqliteRow) -> TagRecord {
    let id: i64 = row.try_get("id").unwrap_or_default();
    let name: String = row.try_get("name").unwrap_or_default();
    let color: Option<String> = row.try_get("color").ok().flatten();
    let created_at_str: String = row.try_get("created_at").unwrap_or_default();
    let updated_at_str: String = row.try_get("updated_at").unwrap_or_default();
    let created_at = DateTime::parse_from_rfc3339(&created_at_str)
        .map(|dt| dt.with_timezone(&Utc))
        .unwrap_or_else(|_| Utc::now());
    let updated_at = DateTime::parse_from_rfc3339(&updated_at_str)
        .map(|dt| dt.with_timezone(&Utc))
        .unwrap_or_else(|_| Utc::now());
    TagRecord {
        id,
        name,
        color,
        created_at,
        updated_at,
    }
}

fn is_unique_violation(err: &sqlx::Error) -> bool {
    let message = err.to_string().to_ascii_lowercase();
    message.contains("unique") || message.contains("constraint")
}

/// Normalize a Tag name (or query) for the `normalized_name`
/// index lookup. The pipeline matches the T017G / T022 contract:
/// NFKC + lower-case. Empty input returns an empty string so the
/// caller can decide whether to skip the filter.
fn normalize(input: &str) -> String {
    use unicode_normalization::UnicodeNormalization;
    let nfkc: String = input.nfkc().collect();
    let nfc: String = nfkc.nfc().collect();
    nfc.to_lowercase()
}

/// Validate a `#RRGGBB` or `#RGB` color literal. An empty
/// string is accepted (the field is optional); any other
/// non-empty value MUST start with `#` followed by either
/// three or six hex digits.
fn is_valid_color(input: &str) -> bool {
    let rest = if let Some(stripped) = input.strip_prefix('#') {
        stripped
    } else {
        return false;
    };
    match rest.len() {
        3 | 6 => rest.chars().all(|c| c.is_ascii_hexdigit()),
        _ => false,
    }
}
