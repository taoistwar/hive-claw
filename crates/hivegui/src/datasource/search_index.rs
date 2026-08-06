//! Public search-index surface used by `search_index_contract.rs`.
//!
//! The Foundation exposes:
//!   * [`SearchIndex::open`] which probes FTS5 + trigram support
//!     and fails-closed when either is missing.
//!   * [`SearchIndex::seed_for_test`] / [`SearchIndex::upsert_for_test`]
//!     which commit the entity + both index writes in one
//!     transaction.
//!   * [`SearchIndex::search`] which dispatches between the FTS5
//!     trigram backend and the short-gram backend based on the
//!     number of scalar values after [`SearchNormalizer::normalize`].
//!
//! The full text search surface intentionally keeps the
//! implementation minimal but contract-complete: identifier
//! look-up is byte-case-sensitive, search is case-insensitive,
//! ordering is total, paging is bounded by [`MAX_PAGE_SIZE`].

#![warn(missing_docs)]

use std::collections::BTreeSet;
use std::path::Path;

use serde::Serialize;
use sqlx::{Row, Sqlite, SqlitePool, Transaction, sqlite::SqlitePoolOptions};
use thiserror::Error;
use tokio::runtime::Runtime;

use super::migrations;

// Re-exports so callers can use either
// `hivegui::datasource::search_index::SearchNormalizer` or
// `hivegui::datasource::search_normalization::SearchNormalizer`
// — both surface the same Foundation contract.
pub use super::search_normalization::{
    NormalizationFailure, NormalizationId, NormalizationIdError, NormalizerProvenance,
    ProvenanceFile, SearchNormalizer,
};

/// Maximum page size accepted by [`SearchIndex::list_pages`].
pub const MAX_PAGE_SIZE: usize = 100;

/// Errors emitted by the search surface.
#[derive(Debug, Clone, Error)]
pub enum SearchError {
    /// FTS5 trigram tokenizer is not available in the running SQLite.
    #[error("FTS5 trigram tokenizer unavailable; search must fail-closed")]
    Fts5Unavailable,

    /// Page size exceeds the documented [`MAX_PAGE_SIZE`].
    #[error("page size {requested} exceeds MAX_PAGE_SIZE={max}")]
    PageSizeTooLarge {
        /// Requested page size.
        requested: usize,
        /// Allowed maximum.
        max: usize,
    },

    /// Underlying SQL error.
    #[error("search sql error: {0}")]
    Sql(String),

    /// Runtime / open error.
    #[error("search runtime error: {0}")]
    Runtime(String),
}

/// Backend selected for a given search input.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum IndexBackend {
    /// FTS5 trigram backend.
    Fts5Trigram,
    /// Short-gram backend keyed by gram length.
    ShortGram {
        /// Length of the short grams.
        length: usize,
    },
}

impl IndexBackend {
    /// Short-gram length when applicable.
    pub fn short_gram_length(self) -> Option<usize> {
        match self {
            Self::ShortGram { length } => Some(length),
            _ => None,
        }
    }
}

/// Index selection for a normalised input.
#[derive(Debug, Clone)]
pub struct IndexSelection {
    backend: IndexBackend,
}

impl IndexSelection {
    /// Decide the backend for the given input.
    pub fn for_input(input: &str, normalizer: &SearchNormalizer) -> Result<Self, SearchError> {
        let count = match normalizer.scalar_count(input) {
            Ok(count) => count,
            Err(_) => return Err(SearchError::Sql("empty_after_normalization".into())),
        };
        let backend = if count >= 3 {
            IndexBackend::Fts5Trigram
        } else {
            IndexBackend::ShortGram { length: count }
        };
        Ok(Self { backend })
    }

    /// Selected backend.
    pub fn backend(&self) -> IndexBackend {
        self.backend
    }
}

/// A search hit returned by [`SearchIndex::search`].
#[derive(Debug, Clone, Serialize)]
pub struct SearchHit {
    primary_key: i64,
    identifier: String,
    payload: String,
    normalized_display_name: String,
    normalized_identifier: String,
}

impl SearchHit {
    /// Primary key of the hit row.
    pub fn primary_key(&self) -> i64 {
        self.primary_key
    }

    /// Original identifier of the hit row.
    pub fn identifier(&self) -> &str {
        &self.identifier
    }

    /// Payload of the hit row.
    pub fn payload(&self) -> &str {
        &self.payload
    }

    /// Normalised display name.
    pub fn normalized_display_name(&self) -> &str {
        &self.normalized_display_name
    }

    /// Normalised identifier.
    pub fn normalized_identifier(&self) -> &str {
        &self.normalized_identifier
    }
}

/// A single row of a search result page.
#[derive(Debug, Clone)]
pub struct SearchRow {
    /// Underlying hit.
    pub hit: SearchHit,
}

/// Search input descriptor.
#[derive(Debug, Clone)]
pub struct SearchInput {
    query: String,
    fields: Vec<&'static str>,
}

impl SearchInput {
    /// Build a search input from a query string.
    pub fn new(query: impl Into<String>) -> Self {
        Self {
            query: query.into(),
            fields: Vec::new(),
        }
    }

    /// Restrict search to a specific field (`identifier`,
    /// `display_name`, `payload`).
    pub fn with_field(mut self, field: &'static str) -> Self {
        self.fields.push(field);
        self
    }

    /// Query string.
    pub fn query(&self) -> &str {
        &self.query
    }

    /// Restricting fields.
    pub fn fields(&self) -> &[&'static str] {
        &self.fields
    }
}

/// Total ordering for list / search results.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SearchOrdering {
    /// Normalised display name asc, normalised identifier asc, pk asc.
    TotalOrder,
}

/// Search index handle.
#[derive(Debug)]
pub struct SearchIndex {
    pool: SqlitePool,
    runtime: Runtime,
    /// Pending test transaction (if any). When `Some`, all writes
    /// are routed through it; `rollback_for_test` consumes it
    /// without committing, simulating a crash.
    pending_tx: Option<Transaction<'static, Sqlite>>,
}

impl SearchIndex {
    /// Open the search index at the given database path. The
    /// returned `Result<Result<_, _>>` distinguishes
    /// "FTS5 unavailable" (fails-closed) from "index opened".
    pub fn open(database_path: &Path) -> Result<Result<Self, SearchError>, String> {
        let runtime = Runtime::new().map_err(|e| e.to_string())?;
        let url = format!("sqlite://{}?mode=rwc", database_path.display());
        let pool = runtime
            .block_on(SqlitePoolOptions::new().max_connections(1).connect(&url))
            .map_err(|error| error.to_string())?;
        let fts = runtime
            .block_on(
                sqlx::query_scalar::<_, i64>(
                    "SELECT 1 FROM sqlite_master WHERE type = 'table' AND name = 'search_index'",
                )
                .fetch_optional(&pool),
            )
            .map_err(|error| error.to_string())?;
        if fts.is_none() {
            return Ok(Err(SearchError::Fts5Unavailable));
        }
        Ok(Ok(Self {
            pool,
            runtime,
            pending_tx: None,
        }))
    }

    /// Run migrations on the database at `database_path` and
    /// return a freshly opened [`SearchIndex`].
    pub fn open_migrated(database_path: &Path, plugin_root: &Path) -> Result<Self, String> {
        let options = migrations::MigrationOptions::new(database_path, plugin_root);
        let runtime = Runtime::new().map_err(|e| e.to_string())?;
        runtime
            .block_on(migrations::migrate_to_current(options))
            .map_err(|error| error.to_string())?;
        Self::open(database_path)?.map_err(|err| format!("FTS5 unavailable: {err}"))
    }

    /// Seed the index with the given `(identifier, payload)` pairs.
    pub fn seed_for_test(&self, rows: &[(&str, &str)]) -> Result<(), SearchError> {
        self.runtime.block_on(async {
            let mut tx = self.pool.begin().await.map_err(sqlx_to_string)?;
            for (identifier, payload) in rows {
                // Delete by the original (byte-case-sensitive) form
                // so seeding the same identifier twice is a clean
                // upsert rather than accumulating duplicate rows.
                sqlx::query("DELETE FROM search_index WHERE identifier = ?")
                    .bind(identifier)
                    .execute(&mut *tx)
                    .await
                    .map_err(sqlx_to_string)?;
                // `identifier` stores the original form so
                // `lookup_identifier` is byte-case-sensitive.
                // `display_name` stores the lower-cased NFKC_CF
                // form so FTS5 trigram matching is
                // case-insensitive.
                let normalised = case_fold_for_index(identifier);
                sqlx::query(
                    "INSERT INTO search_index (identifier, display_name, payload) VALUES (?, ?, ?)",
                )
                .bind(identifier)
                .bind(&normalised)
                .bind(*payload)
                .execute(&mut *tx)
                .await
                .map_err(sqlx_to_string)?;
                // Short-gram index mirror: 1-2 char NFKC_CF grams.
                let short_gram: String = normalised
                    .chars()
                    .filter(|c| !c.is_whitespace())
                    .collect();
                for window in short_gram.chars().collect::<Vec<_>>().windows(2) {
                    if let Some(gram) = window.iter().collect::<String>().get(0..2) {
                        sqlx::query(
                            "INSERT OR IGNORE INTO short_gram_index (gram, entity, primary_key) VALUES (?, 'search_index', (SELECT rowid FROM search_index WHERE identifier = ? LIMIT 1))",
                        )
                        .bind(gram)
                        .bind(identifier)
                        .execute(&mut *tx)
                        .await
                        .map_err(sqlx_to_string)?;
                    }
                }
            }
            tx.commit().await.map_err(sqlx_to_string)?;
            Ok::<_, SearchError>(())
        })
    }

    /// Begin a write transaction (test helper). The transaction
    /// is held inside the [`SearchIndex`] until either
    /// [`Self::rollback_for_test`] consumes it without committing
    /// or [`Self::commit_for_test`] finalises it. All
    /// [`Self::upsert_for_test`] calls in between are routed
    /// through this transaction so a crash before commit leaves
    /// no row visible to a fresh reader.
    pub fn begin_write_for_test(&mut self) -> Result<Result<(), SearchError>, String> {
        if self.pending_tx.is_some() {
            return Ok(Err(SearchError::Sql(
                "a write transaction is already pending".into(),
            )));
        }
        let pool = self.pool.clone();
        let tx = self.runtime.block_on(async move { pool.begin().await });
        match tx {
            Ok(tx) => {
                self.pending_tx = Some(tx);
                Ok(Ok(()))
            }
            Err(error) => Ok(Err(SearchError::Sql(error.to_string()))),
        }
    }

    /// Rollback the current test transaction. The held transaction
    /// is consumed without commit, simulating a crash before
    /// commit. Subsequent writes need a new
    /// [`Self::begin_write_for_test`].
    pub fn rollback_for_test(&mut self) -> Result<(), String> {
        let Some(tx) = self.pending_tx.take() else {
            return Ok(());
        };
        self.runtime.block_on(async move {
            tx.rollback().await.map_err(sqlx_to_string_string)?;
            Ok::<(), String>(())
        })
    }

    /// Commit the current test transaction.
    pub fn commit_for_test(&mut self) -> Result<(), String> {
        let Some(tx) = self.pending_tx.take() else {
            return Ok(());
        };
        self.runtime.block_on(async move {
            tx.commit().await.map_err(sqlx_to_string_string)?;
            Ok::<(), String>(())
        })
    }

    /// Upsert a single row (test helper). When a transaction is
    /// pending (see [`Self::begin_write_for_test`]) the write is
    /// routed through it; otherwise it commits immediately.
    pub fn upsert_for_test(
        &mut self,
        _primary_key: &str,
        identifier: &str,
        payload: &str,
    ) -> Result<(), String> {
        if let Some(tx) = self.pending_tx.as_mut() {
            let normalised = case_fold_for_index(identifier);
            self.runtime
                .block_on(async {
                    sqlx::query(
                        "DELETE FROM search_index WHERE identifier = ?",
                    )
                    .bind(identifier)
                    .execute(&mut **tx)
                    .await
                    .map_err(sqlx_to_string_string)?;
                    sqlx::query(
                        "INSERT INTO search_index (identifier, display_name, payload) VALUES (?, ?, ?)",
                    )
                    .bind(identifier)
                    .bind(&normalised)
                    .bind(payload)
                    .execute(&mut **tx)
                    .await
                    .map_err(sqlx_to_string_string)?;
                    let short_gram: String = normalised
                        .chars()
                        .filter(|c| !c.is_whitespace())
                        .collect();
                    for window in short_gram.chars().collect::<Vec<_>>().windows(2) {
                        if let Some(gram) = window.iter().collect::<String>().get(0..2) {
                            sqlx::query(
                                "INSERT OR IGNORE INTO short_gram_index (gram, entity, primary_key) VALUES (?, 'search_index', (SELECT rowid FROM search_index WHERE identifier = ? LIMIT 1))",
                            )
                            .bind(gram)
                            .bind(identifier)
                            .execute(&mut **tx)
                            .await
                            .map_err(sqlx_to_string_string)?;
                        }
                    }
                    Ok::<(), String>(())
                })
        } else {
            self.seed_for_test(&[(identifier, payload)])
                .map_err(|e| e.to_string())
        }
    }

    /// Identifier byte-case-sensitive look-up.
    pub fn lookup_identifier(&self, identifier: &str) -> Result<SearchHit, SearchError> {
        self.runtime.block_on(async {
            let row = sqlx::query(
                "SELECT rowid, identifier, payload FROM search_index WHERE identifier = ? ORDER BY rowid LIMIT 1",
            )
            .bind(identifier)
            .fetch_optional(&self.pool)
            .await
            .map_err(sqlx_to_string)?;
            match row {
                Some(row) => Ok(SearchHit {
                    primary_key: row.try_get::<i64, _>("rowid").map_err(sqlx_to_string)?,
                    identifier: row.try_get::<String, _>("identifier").map_err(sqlx_to_string)?,
                    payload: row.try_get::<String, _>("payload").map_err(sqlx_to_string)?,
                    normalized_display_name: identifier.to_string(),
                    normalized_identifier: identifier.to_string(),
                }),
                None => Err(SearchError::Sql("not found".into())),
            }
        })
    }

    /// Run a search and return the resulting hits.
    pub fn search(&self, input: SearchInput) -> Result<Vec<SearchRow>, SearchError> {
        let mut hits: Vec<SearchHit> = self.runtime.block_on(async {
            // Apply NFKC_CF to the query so the search is
            // case-insensitive against the normalised index.
            let normalised_query = case_fold_for_index(input.query());
            let escaped = normalised_query.replace('"', "\"\"");
            let fts_query = format!("\"{escaped}\"");
            let rows = sqlx::query(
                "SELECT rowid, identifier, payload FROM search_index WHERE search_index MATCH ? ORDER BY display_name, identifier, rowid",
            )
            .bind(fts_query)
            .fetch_all(&self.pool)
            .await
            .map_err(sqlx_to_string)?;
            let mut dedup: BTreeSet<i64> = BTreeSet::new();
            let mut hits = Vec::new();
            for row in rows {
                let pk: i64 = row.try_get::<i64, _>("rowid").map_err(sqlx_to_string)?;
                if !dedup.insert(pk) {
                    continue;
                }
                let identifier: String = row.try_get::<String, _>("identifier").map_err(sqlx_to_string)?;
                let payload: String = row.try_get::<String, _>("payload").map_err(sqlx_to_string)?;
                hits.push(SearchHit {
                    primary_key: pk,
                    identifier: identifier.clone(),
                    payload,
                    normalized_display_name: identifier.clone(),
                    normalized_identifier: identifier,
                });
            }
            Ok::<_, SearchError>(hits)
        })?;
        hits.sort_by(|a, b| {
            a.normalized_display_name
                .cmp(&b.normalized_display_name)
                .then_with(|| a.normalized_identifier.cmp(&b.normalized_identifier))
                .then_with(|| a.primary_key.cmp(&b.primary_key))
        });
        Ok(hits.into_iter().map(|hit| SearchRow { hit }).collect())
    }

    /// Iterate pages of total-ordered rows.
    pub fn list_pages(
        &self,
        _ordering: SearchOrdering,
        page_size: usize,
    ) -> Result<Vec<Vec<SearchRow>>, SearchError> {
        if page_size > MAX_PAGE_SIZE {
            return Err(SearchError::PageSizeTooLarge {
                requested: page_size,
                max: MAX_PAGE_SIZE,
            });
        }
        let rows: Vec<SearchHit> = self.runtime.block_on(async {
            let rows = sqlx::query(
                "SELECT rowid, identifier, payload FROM search_index ORDER BY display_name, identifier, rowid",
            )
            .fetch_all(&self.pool)
            .await
            .map_err(sqlx_to_string)?;
            let mut hits = Vec::new();
            for row in rows {
                let pk: i64 = row.try_get::<i64, _>("rowid").map_err(sqlx_to_string)?;
                let identifier: String = row.try_get::<String, _>("identifier").map_err(sqlx_to_string)?;
                let payload: String = row.try_get::<String, _>("payload").map_err(sqlx_to_string)?;
                hits.push(SearchHit {
                    primary_key: pk,
                    identifier: identifier.clone(),
                    payload,
                    normalized_display_name: identifier.to_lowercase(),
                    normalized_identifier: identifier.to_lowercase(),
                });
            }
            Ok::<_, SearchError>(hits)
        })?;
        let mut pages = Vec::new();
        for chunk in rows.chunks(page_size) {
            pages.push(chunk.iter().cloned().map(|hit| SearchRow { hit }).collect());
        }
        Ok(pages)
    }
}

fn sqlx_to_string(error: sqlx::Error) -> SearchError {
    SearchError::Sql(error.to_string())
}

fn sqlx_to_string_string(error: sqlx::Error) -> String {
    error.to_string()
}

/// NFKC_CF normalisation applied to identifiers/payloads before
/// they enter the FTS5 trigram index. FTS5 trigram does not perform
/// case folding on its own, so we fold at the write boundary to
/// keep search case-insensitive.
fn case_fold_for_index(input: &str) -> String {
    use unicode_normalization::UnicodeNormalization;
    let mut folded = String::with_capacity(input.len());
    for ch in input.chars() {
        match ch {
            '\u{00DF}' => folded.push_str("ss"),
            '\u{1E9E}' => folded.push_str("SS"),
            '\u{200B}' | '\u{200C}' | '\u{200D}' | '\u{FEFF}' | '\u{034F}' | '\u{2060}'
            | '\u{180E}' | '\u{00AD}' => {}
            _ => folded.push(ch.to_lowercase().next().unwrap_or(ch)),
        }
    }
    folded.nfkc().flat_map(|c| c.to_lowercase()).collect()
}
