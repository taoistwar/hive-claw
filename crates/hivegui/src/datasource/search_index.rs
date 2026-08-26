//! Canonical, migration-owned search-index runtime.
//!
//! Searchable text is stored in `search_documents`, mirrored into the
//! external-content `search_documents_fts` table, and expanded into distinct
//! one- and two-scalar windows in `search_short_grams`. This module never
//! creates or repairs those tables: schema creation and compatibility checks
//! belong to the v4 migration gate.

#![warn(missing_docs)]

use std::collections::BTreeSet;
use std::path::Path;

use serde::Serialize;
use sqlx::{Row, Sqlite, SqlitePool, Transaction, sqlite::SqlitePoolOptions};
use thiserror::Error;
use tokio::runtime::Runtime;

use super::migrations;

pub use super::search_normalization::{
    NORMALIZATION_ID, NormalizationFailure, NormalizationId, NormalizationIdError,
    NormalizerProvenance, ProvenanceFile, SearchNormalizer,
};

/// Maximum page size accepted by [`SearchIndex::list_pages`].
pub const MAX_PAGE_SIZE: usize = 100;

const FIXTURE_ENTITY_TYPE: &str = "function";
const FIXTURE_INPUT_SCHEMA: &str = r#"{"type":"object"}"#;
const FIXTURE_OUTPUT_SCHEMA: &str = r#"{"type":"object"}"#;

/// Errors emitted by the search surface.
#[derive(Debug, Clone, Error)]
pub enum SearchError {
    /// FTS5 trigram tokenizer or the authoritative v4 schema is unavailable.
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

    /// Search input failed its public validation contract.
    #[error("invalid_input {{ field: \"search\", reason: \"{reason}\" }}")]
    InvalidInput {
        /// Stable public reason.
        reason: &'static str,
    },

    /// Underlying SQL error.
    #[error("search sql error: {0}")]
    Sql(String),

    /// Runtime or open error.
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
        /// Length of the short gram.
        length: usize,
    },
}

impl IndexBackend {
    /// Short-gram length when applicable.
    pub fn short_gram_length(self) -> Option<usize> {
        match self {
            Self::ShortGram { length } => Some(length),
            Self::Fts5Trigram => None,
        }
    }
}

/// Index selection for a normalized input.
#[derive(Debug, Clone)]
pub struct IndexSelection {
    backend: IndexBackend,
}

impl IndexSelection {
    /// Decide the backend for the given input.
    pub fn for_input(input: &str, normalizer: &SearchNormalizer) -> Result<Self, SearchError> {
        let normalized = normalize_search_query(normalizer, input)?;
        let length = normalized.chars().count();
        let backend = if length >= 3 {
            IndexBackend::Fts5Trigram
        } else {
            IndexBackend::ShortGram { length }
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

    /// Normalized display name.
    pub fn normalized_display_name(&self) -> &str {
        &self.normalized_display_name
    }

    /// Normalized identifier.
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

    /// Restrict search to a specific field (`identifier`, `display_name`, or
    /// `payload`). Unknown names are ignored rather than interpolated into SQL.
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

/// Total ordering for list and search results.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SearchOrdering {
    /// Normalized display name, normalized identifier, then numeric id.
    TotalOrder,
}

/// Search index handle.
#[derive(Debug)]
pub struct SearchIndex {
    pool: SqlitePool,
    runtime: Runtime,
    pending_tx: Option<Transaction<'static, Sqlite>>,
}

impl SearchIndex {
    /// Open an already-migrated canonical index. Missing tables, a mismatched
    /// normalization id, or a non-functional trigram table fail closed.
    pub fn open(database_path: &Path) -> Result<Result<Self, SearchError>, String> {
        let runtime = Runtime::new().map_err(|error| error.to_string())?;
        let url = format!("sqlite://{}?mode=rwc", database_path.display());
        let pool = runtime
            .block_on(SqlitePoolOptions::new().max_connections(1).connect(&url))
            .map_err(|error| error.to_string())?;

        let canonical = runtime.block_on(async {
            let id = sqlx::query_scalar::<_, String>(
                "SELECT value FROM schema_metadata WHERE key = 'search_normalization_id'",
            )
            .fetch_optional(&pool)
            .await?;
            if id.as_deref() != Some(NORMALIZATION_ID) {
                return Ok::<bool, sqlx::Error>(false);
            }
            sqlx::query_scalar::<_, i64>(
                "SELECT COUNT(*) FROM search_documents_fts \
                 WHERE search_documents_fts MATCH '\"__hivegui_fts_probe__\"'",
            )
            .fetch_one(&pool)
            .await?;
            Ok(true)
        });
        if !matches!(canonical, Ok(true)) {
            return Ok(Err(SearchError::Fts5Unavailable));
        }

        Ok(Ok(Self {
            pool,
            runtime,
            pending_tx: None,
        }))
    }

    /// Migrate the database, then open the canonical index.
    pub fn open_migrated(database_path: &Path, plugin_root: &Path) -> Result<Self, String> {
        let options = migrations::MigrationOptions::new(database_path, plugin_root);
        let runtime = Runtime::new().map_err(|error| error.to_string())?;
        runtime
            .block_on(migrations::migrate_to_current(options))
            .map_err(|error| error.to_string())?;
        Self::open(database_path)?.map_err(|error| error.to_string())
    }

    /// Seed the compatibility fixture through the same canonical documents,
    /// FTS commands, short grams, and transaction boundary as production.
    pub fn seed_for_test(&self, rows: &[(&str, &str)]) -> Result<(), SearchError> {
        self.runtime.block_on(async {
            let normalizer = canonical_normalizer()?;
            let mut transaction = self.pool.begin().await.map_err(sqlx_to_search)?;
            for (identifier, payload) in rows {
                upsert_fixture(&mut transaction, &normalizer, identifier, payload).await?;
            }
            transaction.commit().await.map_err(sqlx_to_search)
        })
    }

    /// Begin a compatibility-fixture write transaction.
    pub fn begin_write_for_test(&mut self) -> Result<Result<(), SearchError>, String> {
        if self.pending_tx.is_some() {
            return Ok(Err(SearchError::Sql(
                "a write transaction is already pending".to_string(),
            )));
        }
        let pool = self.pool.clone();
        match self.runtime.block_on(async move { pool.begin().await }) {
            Ok(transaction) => {
                self.pending_tx = Some(transaction);
                Ok(Ok(()))
            }
            Err(error) => Ok(Err(SearchError::Sql(error.to_string()))),
        }
    }

    /// Roll back the current compatibility-fixture transaction.
    pub fn rollback_for_test(&mut self) -> Result<(), String> {
        let Some(transaction) = self.pending_tx.take() else {
            return Ok(());
        };
        self.runtime
            .block_on(async move { transaction.rollback().await })
            .map_err(|error| error.to_string())
    }

    /// Commit the current compatibility-fixture transaction.
    pub fn commit_for_test(&mut self) -> Result<(), String> {
        let Some(transaction) = self.pending_tx.take() else {
            return Ok(());
        };
        self.runtime
            .block_on(async move { transaction.commit().await })
            .map_err(|error| error.to_string())
    }

    /// Upsert one compatibility-fixture row through the canonical write path.
    pub fn upsert_for_test(
        &mut self,
        _primary_key: &str,
        identifier: &str,
        payload: &str,
    ) -> Result<(), String> {
        let normalizer = canonical_normalizer().map_err(|error| error.to_string())?;
        if let Some(transaction) = self.pending_tx.as_mut() {
            self.runtime
                .block_on(upsert_fixture(
                    transaction,
                    &normalizer,
                    identifier,
                    payload,
                ))
                .map_err(|error| error.to_string())
        } else {
            self.seed_for_test(&[(identifier, payload)])
                .map_err(|error| error.to_string())
        }
    }

    /// Perform a byte-case-sensitive identifier lookup.
    pub fn lookup_identifier(&self, identifier: &str) -> Result<SearchHit, SearchError> {
        self.runtime.block_on(async {
            let row = sqlx::query(
                "SELECT functions.id, functions.identifier, functions.description AS payload, \
                        names.normalized_text AS normalized_display_name, \
                        identifiers.normalized_text AS normalized_identifier \
                 FROM functions \
                 JOIN search_documents AS names \
                   ON names.entity_type = 'function' \
                  AND names.entity_key = CAST(functions.id AS TEXT) AND names.field = 'name' \
                 JOIN search_documents AS identifiers \
                   ON identifiers.entity_type = names.entity_type \
                  AND identifiers.entity_key = names.entity_key \
                  AND identifiers.field = 'identifier' \
                 WHERE functions.identifier = ? COLLATE BINARY \
                 ORDER BY functions.id LIMIT 1",
            )
            .bind(identifier)
            .fetch_optional(&self.pool)
            .await
            .map_err(sqlx_to_search)?;
            row.map(row_to_hit)
                .transpose()?
                .ok_or_else(|| SearchError::Sql("not found".to_string()))
        })
    }

    /// Search the canonical index using literal phrase or exact short-gram
    /// semantics, then return deduplicated total-ordered entities.
    pub fn search(&self, input: SearchInput) -> Result<Vec<SearchRow>, SearchError> {
        let normalizer = canonical_normalizer()?;
        let normalized = normalize_search_query(&normalizer, input.query())?;
        let selection = IndexSelection::for_input(input.query(), &normalizer)?;
        let (identifier, payload, name) = selected_fixture_fields(input.fields());
        let rows = self.runtime.block_on(async {
            let query = match selection.backend() {
                IndexBackend::Fts5Trigram => sqlx::query(FIXTURE_FTS_SEARCH_SQL)
                    .bind(fts_literal_phrase(&normalized))
                    .bind(identifier)
                    .bind(payload)
                    .bind(name),
                IndexBackend::ShortGram { length } => sqlx::query(FIXTURE_SHORT_SEARCH_SQL)
                    .bind(length as i64)
                    .bind(&normalized)
                    .bind(identifier)
                    .bind(payload)
                    .bind(name),
            };
            query
                .fetch_all(&self.pool)
                .await
                .map_err(sqlx_to_search)?
                .into_iter()
                .map(row_to_hit)
                .collect::<Result<Vec<_>, _>>()
        })?;
        Ok(rows.into_iter().map(|hit| SearchRow { hit }).collect())
    }

    /// Iterate pages of canonical total-ordered rows.
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
        if page_size == 0 {
            return Ok(Vec::new());
        }
        let rows = self.runtime.block_on(async {
            sqlx::query(FIXTURE_UNFILTERED_SQL)
                .fetch_all(&self.pool)
                .await
                .map_err(sqlx_to_search)?
                .into_iter()
                .map(row_to_hit)
                .collect::<Result<Vec<_>, _>>()
        })?;
        Ok(rows
            .chunks(page_size)
            .map(|page| page.iter().cloned().map(|hit| SearchRow { hit }).collect())
            .collect())
    }
}

/// Load the single approved normalizer without a fallback implementation.
pub(crate) fn canonical_normalizer() -> Result<SearchNormalizer, SearchError> {
    SearchNormalizer::load(NORMALIZATION_ID)
        .map_err(SearchError::Runtime)?
        .map_err(|error| SearchError::Runtime(error.to_string()))
}

/// Normalize and validate a public search input.
pub(crate) fn normalize_search_query(
    normalizer: &SearchNormalizer,
    input: &str,
) -> Result<String, SearchError> {
    let raw_length = input.chars().count();
    if !(1..=255).contains(&raw_length) {
        return Err(SearchError::InvalidInput {
            reason: "length_out_of_range",
        });
    }
    normalizer
        .normalize(input)
        .map_err(|failure| match failure {
            Ok(NormalizationFailure::EmptyAfterNormalization) => SearchError::InvalidInput {
                reason: "empty_after_normalization",
            },
            Err(message) => SearchError::Runtime(message),
        })
}

/// Encode a normalized value as one bound FTS5 literal phrase.
pub(crate) fn fts_literal_phrase(normalized: &str) -> String {
    format!("\"{}\"", normalized.replace('"', "\"\""))
}

/// Replace every derived search row for one entity inside the caller's
/// existing base-row transaction.
pub(crate) async fn replace_entity_search_documents(
    transaction: &mut Transaction<'_, Sqlite>,
    entity_type: &str,
    entity_key: &str,
    fields: &[(&str, &str)],
) -> Result<(), SearchError> {
    let normalizer = canonical_normalizer()?;
    delete_entity_search_documents(transaction, entity_type, entity_key).await?;
    for (field, value) in fields {
        let normalized = normalizer
            .normalize(value)
            .map_err(normalization_failure_to_search)?;
        let document_id = sqlx::query_scalar::<_, i64>(
            "INSERT INTO search_documents \
                 (entity_type, entity_key, field, normalized_text) \
             VALUES (?, ?, ?, ?) RETURNING id",
        )
        .bind(entity_type)
        .bind(entity_key)
        .bind(field)
        .bind(&normalized)
        .fetch_one(&mut **transaction)
        .await
        .map_err(sqlx_to_search)?;
        sqlx::query("INSERT INTO search_documents_fts(rowid, normalized_text) VALUES (?, ?)")
            .bind(document_id)
            .bind(&normalized)
            .execute(&mut **transaction)
            .await
            .map_err(sqlx_to_search)?;

        let scalars = normalized.chars().collect::<Vec<_>>();
        for gram_len in [1_usize, 2] {
            let grams = scalars
                .windows(gram_len)
                .map(|window| window.iter().collect::<String>())
                .collect::<BTreeSet<_>>();
            for gram in grams {
                sqlx::query(
                    "INSERT INTO search_short_grams (document_id, gram_len, gram) \
                     VALUES (?, ?, ?)",
                )
                .bind(document_id)
                .bind(gram_len as i64)
                .bind(gram)
                .execute(&mut **transaction)
                .await
                .map_err(sqlx_to_search)?;
            }
        }
    }
    Ok(())
}

/// Remove an entity's external-content FTS rows and canonical documents inside
/// the caller's base-row transaction.
pub(crate) async fn delete_entity_search_documents(
    transaction: &mut Transaction<'_, Sqlite>,
    entity_type: &str,
    entity_key: &str,
) -> Result<(), SearchError> {
    let documents = sqlx::query(
        "SELECT id, normalized_text FROM search_documents \
         WHERE entity_type = ? AND entity_key = ? ORDER BY id",
    )
    .bind(entity_type)
    .bind(entity_key)
    .fetch_all(&mut **transaction)
    .await
    .map_err(sqlx_to_search)?;
    for document in documents {
        let id = document.try_get::<i64, _>("id").map_err(sqlx_to_search)?;
        let normalized = document
            .try_get::<String, _>("normalized_text")
            .map_err(sqlx_to_search)?;
        sqlx::query(
            "INSERT INTO search_documents_fts(search_documents_fts, rowid, normalized_text) \
             VALUES ('delete', ?, ?)",
        )
        .bind(id)
        .bind(normalized)
        .execute(&mut **transaction)
        .await
        .map_err(sqlx_to_search)?;
    }
    sqlx::query("DELETE FROM search_documents WHERE entity_type = ? AND entity_key = ?")
        .bind(entity_type)
        .bind(entity_key)
        .execute(&mut **transaction)
        .await
        .map_err(sqlx_to_search)?;
    Ok(())
}

async fn upsert_fixture(
    transaction: &mut Transaction<'_, Sqlite>,
    normalizer: &SearchNormalizer,
    identifier: &str,
    payload: &str,
) -> Result<(), SearchError> {
    let existing = sqlx::query_scalar::<_, i64>(
        "SELECT id FROM functions WHERE identifier = ? COLLATE BINARY",
    )
    .bind(identifier)
    .fetch_optional(&mut **transaction)
    .await
    .map_err(sqlx_to_search)?;
    if let Some(id) = existing {
        delete_entity_search_documents(transaction, FIXTURE_ENTITY_TYPE, &id.to_string()).await?;
        sqlx::query("DELETE FROM functions WHERE id = ?")
            .bind(id)
            .execute(&mut **transaction)
            .await
            .map_err(sqlx_to_search)?;
    }

    normalizer
        .normalize(identifier)
        .map_err(normalization_failure_to_search)?;
    normalizer
        .normalize(payload)
        .map_err(normalization_failure_to_search)?;

    let now = chrono::Utc::now().to_rfc3339();
    let id = sqlx::query_scalar::<_, i64>(
        "INSERT INTO functions \
             (identifier, name, description, kind, input_schema, output_schema, \
              plugin_id, plugin_export, category_id, required_capabilities, created_at, updated_at) \
         VALUES (?, ?, ?, 'placeholder', ?, ?, NULL, NULL, NULL, NULL, ?, ?) RETURNING id",
    )
    .bind(identifier)
    .bind(identifier)
    .bind(payload)
    .bind(FIXTURE_INPUT_SCHEMA)
    .bind(FIXTURE_OUTPUT_SCHEMA)
    .bind(&now)
    .bind(&now)
    .fetch_one(&mut **transaction)
    .await
    .map_err(sqlx_to_search)?;
    replace_entity_search_documents(
        transaction,
        FIXTURE_ENTITY_TYPE,
        &id.to_string(),
        &[
            ("identifier", identifier),
            ("name", identifier),
            ("payload", payload),
        ],
    )
    .await
}

fn selected_fixture_fields(fields: &[&str]) -> (i64, i64, i64) {
    if fields.is_empty() {
        return (1, 1, 1);
    }
    (
        i64::from(fields.contains(&"identifier")),
        i64::from(fields.contains(&"payload")),
        i64::from(fields.contains(&"display_name") || fields.contains(&"name")),
    )
}

fn row_to_hit(row: sqlx::sqlite::SqliteRow) -> Result<SearchHit, SearchError> {
    Ok(SearchHit {
        primary_key: row.try_get("id").map_err(sqlx_to_search)?,
        identifier: row.try_get("identifier").map_err(sqlx_to_search)?,
        payload: row
            .try_get::<Option<String>, _>("payload")
            .map_err(sqlx_to_search)?
            .unwrap_or_default(),
        normalized_display_name: row
            .try_get("normalized_display_name")
            .map_err(sqlx_to_search)?,
        normalized_identifier: row
            .try_get("normalized_identifier")
            .map_err(sqlx_to_search)?,
    })
}

fn normalization_failure_to_search(failure: Result<NormalizationFailure, String>) -> SearchError {
    match failure {
        Ok(NormalizationFailure::EmptyAfterNormalization) => SearchError::InvalidInput {
            reason: "empty_after_normalization",
        },
        Err(message) => SearchError::Runtime(message),
    }
}

fn sqlx_to_search(error: sqlx::Error) -> SearchError {
    SearchError::Sql(error.to_string())
}

const FIXTURE_FTS_SEARCH_SQL: &str = "WITH matches AS (\
    SELECT DISTINCT documents.entity_key \
    FROM search_documents_fts \
    JOIN search_documents AS documents ON documents.id = search_documents_fts.rowid \
    WHERE search_documents_fts MATCH ? AND documents.entity_type = 'function' \
      AND ((? = 1 AND documents.field = 'identifier') \
        OR (? = 1 AND documents.field = 'payload') \
        OR (? = 1 AND documents.field = 'name'))\
) \
SELECT functions.id, functions.identifier, functions.description AS payload, \
       names.normalized_text AS normalized_display_name, \
       identifiers.normalized_text AS normalized_identifier \
FROM matches \
JOIN functions ON functions.id = CAST(matches.entity_key AS INTEGER) \
JOIN search_documents AS names ON names.entity_type = 'function' \
 AND names.entity_key = matches.entity_key AND names.field = 'name' \
JOIN search_documents AS identifiers ON identifiers.entity_type = 'function' \
 AND identifiers.entity_key = matches.entity_key AND identifiers.field = 'identifier' \
ORDER BY names.normalized_text, identifiers.normalized_text, functions.id";

const FIXTURE_SHORT_SEARCH_SQL: &str = "WITH matches AS (\
    SELECT DISTINCT documents.entity_key \
    FROM search_short_grams AS grams \
    JOIN search_documents AS documents ON documents.id = grams.document_id \
    WHERE grams.gram_len = ? AND grams.gram = ? AND documents.entity_type = 'function' \
      AND ((? = 1 AND documents.field = 'identifier') \
        OR (? = 1 AND documents.field = 'payload') \
        OR (? = 1 AND documents.field = 'name'))\
) \
SELECT functions.id, functions.identifier, functions.description AS payload, \
       names.normalized_text AS normalized_display_name, \
       identifiers.normalized_text AS normalized_identifier \
FROM matches \
JOIN functions ON functions.id = CAST(matches.entity_key AS INTEGER) \
JOIN search_documents AS names ON names.entity_type = 'function' \
 AND names.entity_key = matches.entity_key AND names.field = 'name' \
JOIN search_documents AS identifiers ON identifiers.entity_type = 'function' \
 AND identifiers.entity_key = matches.entity_key AND identifiers.field = 'identifier' \
ORDER BY names.normalized_text, identifiers.normalized_text, functions.id";

const FIXTURE_UNFILTERED_SQL: &str = "SELECT functions.id, functions.identifier, \
    functions.description AS payload, names.normalized_text AS normalized_display_name, \
    identifiers.normalized_text AS normalized_identifier \
FROM search_documents AS names \
JOIN search_documents AS identifiers ON identifiers.entity_type = names.entity_type \
 AND identifiers.entity_key = names.entity_key AND identifiers.field = 'identifier' \
JOIN functions ON functions.id = CAST(names.entity_key AS INTEGER) \
WHERE names.entity_type = 'function' AND names.field = 'name' \
ORDER BY names.normalized_text, identifiers.normalized_text, functions.id";
