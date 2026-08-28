//! SQL-backed Function data boundary for HiveGUI.
//!
//! The v4 migration owns schema creation. This module owns user Function
//! validation, CRUD transactions, canonical indexed paging/search, immutable
//! Builtin synchronization, and safe public conflict reporting.

#![warn(missing_docs)]

use std::collections::BTreeSet;

use chrono::Utc;
use hive_builtins::{BUILTIN_IDENTIFIERS, BuiltinDefinition, BuiltinRegistry};
use hive_json_schema::CompiledJsonSchema;
use serde_json::Value;
use sqlx::{FromRow, Pool, Row, Sqlite, Transaction};

use super::plugin_manifest::{manifest_exports, validate_manifest};
use super::query_plan::{
    FUNCTION_FTS_COUNT_SQL, FUNCTION_FTS_LIST_SQL, FUNCTION_GET_SQL, FUNCTION_SHORT_GRAM_COUNT_SQL,
    FUNCTION_SHORT_GRAM_LIST_SQL, FUNCTION_TOOL_REFERENCES_SQL, FUNCTION_UNFILTERED_COUNT_SQL,
    FUNCTION_UNFILTERED_LIST_SQL, FUNCTION_WORKFLOW_NODE_REFERENCES_SQL,
};
use super::search_index::{
    IndexBackend, IndexSelection, canonical_normalizer, delete_entity_search_documents,
    fts_literal_phrase, normalize_search_query, replace_entity_search_documents,
};
use super::validation::{PublicBoundaryError, PublicErrorEnvelope};

/// Exact reserved Builtin identifiers, re-exported from the sole registry
/// owner rather than copied into HiveGUI.
pub use hive_builtins::BUILTIN_IDENTIFIERS as RESERVED_UNDERSCORE_IDENTIFIERS;

const PAGE_SIZE: i64 = 20;
const MAX_TEXT_BYTES: usize = 255;
const MAX_DESCRIPTION_BYTES: usize = 2_000;
const MAX_SCHEMA_BYTES: usize = 1024 * 1024;

const FUNCTION_INSERT_SQL: &str = "INSERT INTO functions (\
    identifier, name, description, kind, input_schema, output_schema, plugin_id, \
    plugin_export, category_id, required_capabilities, created_at, updated_at\
) VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?) RETURNING id";

const FUNCTION_UPDATE_SQL: &str = "UPDATE functions SET identifier = ?, name = ?, \
    description = ?, kind = ?, input_schema = ?, output_schema = ?, plugin_id = ?, \
    plugin_export = ?, category_id = ?, required_capabilities = ?, updated_at = ? \
    WHERE id = ?";

/// Stable persisted Function kind.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FunctionKind {
    /// Immutable pure Function registered by code.
    Builtin,
    /// Function backed by one live ABI-v1 Plugin export.
    Custom,
    /// Schema-only Function that can never execute.
    Placeholder,
}

impl FunctionKind {
    /// Stable wire string.
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Builtin => "builtin",
            Self::Custom => "custom",
            Self::Placeholder => "placeholder",
        }
    }
}

impl TryFrom<&str> for FunctionKind {
    type Error = PublicBoundaryError;

    fn try_from(value: &str) -> Result<Self, Self::Error> {
        match value {
            "builtin" => Ok(Self::Builtin),
            "custom" => Ok(Self::Custom),
            "placeholder" => Ok(Self::Placeholder),
            _ => Err(invalid_input("kind", "invalid_enum")),
        }
    }
}

impl TryFrom<String> for FunctionKind {
    type Error = PublicBoundaryError;

    fn try_from(value: String) -> Result<Self, Self::Error> {
        Self::try_from(value.as_str())
    }
}

/// Complete shared write DTO for Function create and update.
#[derive(Debug, Clone)]
pub struct FunctionInput {
    identifier: String,
    name: String,
    description: Option<String>,
    kind: FunctionKind,
    input_schema: String,
    output_schema: String,
    plugin_id: Option<i64>,
    plugin_export: Option<String>,
    category_id: Option<i64>,
    required_capabilities: Option<String>,
}

impl FunctionInput {
    /// Validate and construct the complete public write DTO.
    #[allow(clippy::too_many_arguments)]
    pub fn for_write(
        identifier: String,
        name: String,
        description: Option<String>,
        kind: FunctionKind,
        input_schema: String,
        output_schema: String,
        plugin_id: Option<i64>,
        plugin_export: Option<String>,
        category_id: Option<i64>,
        required_capabilities: Option<String>,
    ) -> Result<Self, PublicBoundaryError> {
        validate_identifier(&identifier)?;
        validate_name(&name)?;
        if let Some(description) = description.as_deref() {
            validate_description(description)?;
        }
        validate_schema("input_schema", &input_schema)?;
        validate_schema("output_schema", &output_schema)?;
        if plugin_id.is_some_and(|id| id <= 0) {
            return Err(invalid_input("plugin_id", "out_of_range"));
        }
        if category_id.is_some_and(|id| id <= 0) {
            return Err(invalid_input("category_id", "out_of_range"));
        }
        let plugin_export = plugin_export.map(|value| value.trim().to_string());
        let required_capabilities = required_capabilities
            .as_deref()
            .map(canonicalize_capability_json)
            .transpose()?;
        Ok(Self {
            identifier,
            name,
            description,
            kind,
            input_schema,
            output_schema,
            plugin_id,
            plugin_export,
            category_id,
            required_capabilities,
        })
    }

    /// Compatibility constructor for a minimal schema-only input. New callers
    /// should use [`Self::for_write`] so the DTO field set remains explicit.
    pub fn new(
        identifier: impl Into<String>,
        kind: FunctionKind,
    ) -> Result<Self, PublicBoundaryError> {
        let identifier = identifier.into();
        Self::for_write(
            identifier.clone(),
            identifier,
            None,
            kind,
            r#"{"type":"object"}"#.to_string(),
            r#"{"type":"object"}"#.to_string(),
            None,
            None,
            None,
            None,
        )
    }

    /// Identifier.
    pub fn identifier(&self) -> &str {
        &self.identifier
    }

    /// Display name.
    pub fn name(&self) -> &str {
        &self.name
    }

    /// Optional description.
    pub fn description(&self) -> Option<&str> {
        self.description.as_deref()
    }

    /// Stable kind.
    pub fn kind(&self) -> FunctionKind {
        self.kind
    }

    /// Input JSON Schema text.
    pub fn input_schema(&self) -> &str {
        &self.input_schema
    }

    /// Output JSON Schema text.
    pub fn output_schema(&self) -> &str {
        &self.output_schema
    }

    /// Bound Plugin id.
    pub fn plugin_id(&self) -> Option<i64> {
        self.plugin_id
    }

    /// Bound Plugin export.
    pub fn plugin_export(&self) -> Option<&str> {
        self.plugin_export.as_deref()
    }

    /// Optional Category id.
    pub fn category_id(&self) -> Option<i64> {
        self.category_id
    }

    /// Canonical required-Capability JSON.
    pub fn required_capabilities(&self) -> Option<&str> {
        self.required_capabilities.as_deref()
    }
}

/// Persisted Function returned by every read/write method.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FunctionRecord {
    id: i64,
    identifier: String,
    name: String,
    description: Option<String>,
    kind: FunctionKind,
    input_schema: String,
    output_schema: String,
    plugin_id: Option<i64>,
    plugin_export: Option<String>,
    category_id: Option<i64>,
    required_capabilities: Option<String>,
    created_at: String,
    updated_at: String,
}

impl FunctionRecord {
    /// Database id.
    pub fn id(&self) -> i64 {
        self.id
    }

    /// Identifier.
    pub fn identifier(&self) -> &str {
        &self.identifier
    }

    /// Display name.
    pub fn name(&self) -> &str {
        &self.name
    }

    /// Optional description.
    pub fn description(&self) -> Option<&str> {
        self.description.as_deref()
    }

    /// Stable kind.
    pub fn kind(&self) -> FunctionKind {
        self.kind
    }

    /// Input JSON Schema text.
    pub fn input_schema(&self) -> &str {
        &self.input_schema
    }

    /// Output JSON Schema text.
    pub fn output_schema(&self) -> &str {
        &self.output_schema
    }

    /// Bound Plugin id.
    pub fn plugin_id(&self) -> Option<i64> {
        self.plugin_id
    }

    /// Bound Plugin export.
    pub fn plugin_export(&self) -> Option<&str> {
        self.plugin_export.as_deref()
    }

    /// Optional Category id.
    pub fn category_id(&self) -> Option<i64> {
        self.category_id
    }

    /// Canonical required-Capability JSON.
    pub fn required_capabilities(&self) -> Option<&str> {
        self.required_capabilities.as_deref()
    }

    /// Creation timestamp.
    pub fn created_at(&self) -> &str {
        &self.created_at
    }

    /// Last-update timestamp.
    pub fn updated_at(&self) -> &str {
        &self.updated_at
    }

    pub(crate) fn into_legacy_entity(self) -> super::entity_store::Function {
        super::entity_store::Function {
            id: self.id,
            identifier: self.identifier,
            name: self.name,
            description: self.description,
            kind: self.kind.as_str().to_string(),
            input_schema: self.input_schema,
            output_schema: self.output_schema,
            plugin_id: self.plugin_id,
            plugin_export: self.plugin_export,
            category_id: self.category_id,
            required_capabilities: self.required_capabilities,
            created_at: self.created_at,
            updated_at: self.updated_at,
        }
    }
}

/// Fixed-size, 1-based Function page.
#[derive(Debug, Clone)]
pub struct FunctionPage {
    items: Vec<FunctionRecord>,
    total: i64,
    page: i64,
}

impl FunctionPage {
    /// Items in this page.
    pub fn items(&self) -> &[FunctionRecord] {
        &self.items
    }

    /// Total matching Functions.
    pub fn total(&self) -> i64 {
        self.total
    }

    /// One-based page number.
    pub fn page(&self) -> i64 {
        self.page
    }

    /// Fixed page size (`20`).
    pub fn page_size(&self) -> i64 {
        PAGE_SIZE
    }
}

#[derive(Debug, FromRow)]
struct FunctionRow {
    id: i64,
    identifier: String,
    name: String,
    description: Option<String>,
    kind: String,
    input_schema: String,
    output_schema: String,
    plugin_id: Option<i64>,
    plugin_export: Option<String>,
    category_id: Option<i64>,
    required_capabilities: Option<String>,
    created_at: String,
    updated_at: String,
}

#[derive(Debug, FromRow)]
struct FunctionFtsPageRow {
    #[sqlx(flatten)]
    function: FunctionRow,
    total_count: i64,
}

impl TryFrom<FunctionRow> for FunctionRecord {
    type Error = PublicBoundaryError;

    fn try_from(row: FunctionRow) -> Result<Self, Self::Error> {
        let kind = FunctionKind::try_from(row.kind.as_str())
            .map_err(|_| internal_error("persisted_kind"))?;
        Ok(Self {
            id: row.id,
            identifier: row.identifier,
            name: row.name,
            description: row.description,
            kind,
            input_schema: row.input_schema,
            output_schema: row.output_schema,
            plugin_id: row.plugin_id,
            plugin_export: row.plugin_export,
            category_id: row.category_id,
            required_capabilities: row.required_capabilities,
            created_at: row.created_at,
            updated_at: row.updated_at,
        })
    }
}

/// Unique SQL-backed Function store over a supplied canonical v4 pool.
#[derive(Debug, Clone)]
pub struct FunctionStore {
    pool: Pool<Sqlite>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum WriteAuthority {
    User,
    LegacyFixture,
}

#[derive(Debug, Clone)]
enum FunctionSearchRoute {
    Unfiltered,
    FtsPhrase(String),
    ShortGram { length: i64, gram: String },
}

impl FunctionSearchRoute {
    fn for_input(search: Option<String>) -> Result<Self, PublicBoundaryError> {
        let Some(search) = search.filter(|value| !value.is_empty()) else {
            return Ok(Self::Unfiltered);
        };
        let normalizer = canonical_normalizer().map_err(search_error)?;
        let normalized = normalize_search_query(&normalizer, &search).map_err(search_error)?;
        let selection = IndexSelection::for_input(&search, &normalizer).map_err(search_error)?;
        match selection.backend() {
            IndexBackend::Fts5Trigram => Ok(Self::FtsPhrase(fts_literal_phrase(&normalized))),
            IndexBackend::ShortGram { length } => Ok(Self::ShortGram {
                length: length as i64,
                gram: normalized,
            }),
        }
    }
}

impl FunctionStore {
    /// Construct a Function store. No schema is created or repaired here.
    pub fn new(pool: Pool<Sqlite>) -> Result<Self, PublicBoundaryError> {
        validate_builtin_registry()?;
        Ok(Self { pool })
    }

    /// Return the registry-owned reserved identifiers.
    pub fn reserved_builtin_ids(&self) -> &'static [&'static str] {
        RESERVED_UNDERSCORE_IDENTIFIERS
    }

    /// Create a user-managed Custom or Placeholder Function atomically with
    /// its canonical derived search rows.
    pub async fn create(
        &self,
        input: FunctionInput,
    ) -> Result<FunctionRecord, PublicBoundaryError> {
        self.create_with_authority(input, WriteAuthority::User)
            .await
    }

    pub(crate) async fn create_legacy_fixture(
        &self,
        input: FunctionInput,
    ) -> Result<FunctionRecord, PublicBoundaryError> {
        self.create_with_authority(input, WriteAuthority::LegacyFixture)
            .await
    }

    async fn create_with_authority(
        &self,
        input: FunctionInput,
        authority: WriteAuthority,
    ) -> Result<FunctionRecord, PublicBoundaryError> {
        if authority == WriteAuthority::User {
            reject_user_builtin_or_reserved(&input)?;
        }
        let mut transaction = self.pool.begin().await.map_err(storage_error)?;
        let input = prepare_input(&mut transaction, input, authority).await?;
        let now = Utc::now().to_rfc3339();
        let id = sqlx::query_scalar::<_, i64>(FUNCTION_INSERT_SQL)
            .bind(&input.identifier)
            .bind(&input.name)
            .bind(&input.description)
            .bind(input.kind.as_str())
            .bind(&input.input_schema)
            .bind(&input.output_schema)
            .bind(input.plugin_id)
            .bind(&input.plugin_export)
            .bind(input.category_id)
            .bind(&input.required_capabilities)
            .bind(&now)
            .bind(&now)
            .fetch_one(&mut *transaction)
            .await
            .map_err(|error| write_error(error, &input.identifier))?;
        replace_entity_search_documents(
            &mut transaction,
            "function",
            &id.to_string(),
            &[("identifier", &input.identifier), ("name", &input.name)],
        )
        .await
        .map_err(search_error)?;
        let record = fetch_function(&mut transaction, id)
            .await?
            .ok_or_else(|| internal_error("write_visibility"))?;
        transaction.commit().await.map_err(storage_error)?;
        Ok(record)
    }

    /// Load one Function by id.
    pub async fn get(&self, id: i64) -> Result<Option<FunctionRecord>, PublicBoundaryError> {
        if id <= 0 {
            return Err(invalid_input("id", "out_of_range"));
        }
        let row = sqlx::query_as::<_, FunctionRow>(FUNCTION_GET_SQL)
            .bind(id)
            .fetch_optional(&self.pool)
            .await
            .map_err(storage_error)?;
        row.map(FunctionRecord::try_from).transpose()
    }

    /// Update a user-managed Function and its search documents atomically.
    pub async fn update(
        &self,
        id: i64,
        input: FunctionInput,
    ) -> Result<FunctionRecord, PublicBoundaryError> {
        self.update_with_authority(id, input, WriteAuthority::User)
            .await
    }

    pub(crate) async fn update_legacy_fixture(
        &self,
        id: i64,
        input: FunctionInput,
    ) -> Result<FunctionRecord, PublicBoundaryError> {
        self.update_with_authority(id, input, WriteAuthority::LegacyFixture)
            .await
    }

    async fn update_with_authority(
        &self,
        id: i64,
        input: FunctionInput,
        authority: WriteAuthority,
    ) -> Result<FunctionRecord, PublicBoundaryError> {
        if id <= 0 {
            return Err(invalid_input("id", "out_of_range"));
        }
        if authority == WriteAuthority::User {
            reject_user_builtin_or_reserved(&input)?;
        }
        let mut transaction = self.pool.begin().await.map_err(storage_error)?;
        let existing = fetch_function(&mut transaction, id)
            .await?
            .ok_or_else(not_found)?;
        if authority == WriteAuthority::User
            && (existing.kind == FunctionKind::Builtin
                || BUILTIN_IDENTIFIERS.contains(&existing.identifier.as_str()))
        {
            return Err(invalid_input("kind", "builtin_immutable"));
        }
        let input = prepare_input(&mut transaction, input, authority).await?;
        let now = Utc::now().to_rfc3339();
        let result = sqlx::query(FUNCTION_UPDATE_SQL)
            .bind(&input.identifier)
            .bind(&input.name)
            .bind(&input.description)
            .bind(input.kind.as_str())
            .bind(&input.input_schema)
            .bind(&input.output_schema)
            .bind(input.plugin_id)
            .bind(&input.plugin_export)
            .bind(input.category_id)
            .bind(&input.required_capabilities)
            .bind(&now)
            .bind(id)
            .execute(&mut *transaction)
            .await
            .map_err(|error| write_error(error, &input.identifier))?;
        if result.rows_affected() != 1 {
            return Err(not_found());
        }
        replace_entity_search_documents(
            &mut transaction,
            "function",
            &id.to_string(),
            &[("identifier", &input.identifier), ("name", &input.name)],
        )
        .await
        .map_err(search_error)?;
        let record = fetch_function(&mut transaction, id)
            .await?
            .ok_or_else(|| internal_error("write_visibility"))?;
        transaction.commit().await.map_err(storage_error)?;
        Ok(record)
    }

    /// Delete an unreferenced user-managed Function. Tool and WorkflowNode
    /// references are checked and reported within the same transaction.
    pub async fn delete(&self, id: i64) -> Result<(), PublicBoundaryError> {
        self.delete_with_authority(id, WriteAuthority::User).await
    }

    pub(crate) async fn delete_legacy_fixture(&self, id: i64) -> Result<(), PublicBoundaryError> {
        self.delete_with_authority(id, WriteAuthority::LegacyFixture)
            .await
    }

    async fn delete_with_authority(
        &self,
        id: i64,
        authority: WriteAuthority,
    ) -> Result<(), PublicBoundaryError> {
        if id <= 0 {
            return Err(invalid_input("id", "out_of_range"));
        }
        let mut transaction = self.pool.begin().await.map_err(storage_error)?;
        let existing = fetch_function(&mut transaction, id)
            .await?
            .ok_or_else(not_found)?;
        if authority == WriteAuthority::User
            && (existing.kind == FunctionKind::Builtin
                || BUILTIN_IDENTIFIERS.contains(&existing.identifier.as_str()))
        {
            return Err(invalid_input("kind", "builtin_immutable"));
        }

        let workflow_references = sqlx::query(FUNCTION_WORKFLOW_NODE_REFERENCES_SQL)
            .bind(id)
            .fetch_all(&mut *transaction)
            .await
            .map_err(storage_error)?;
        if !workflow_references.is_empty() {
            let references = workflow_references
                .into_iter()
                .map(|row| {
                    safe_reference(row.get::<String, _>("workflow_identifier"), row.get("id"))
                })
                .collect::<BTreeSet<_>>()
                .into_iter();
            return Err(reference_conflict(
                "referenced_by_workflow_node",
                references,
            ));
        }

        let tool_references = sqlx::query(FUNCTION_TOOL_REFERENCES_SQL)
            .bind(id)
            .fetch_all(&mut *transaction)
            .await
            .map_err(storage_error)?;
        if !tool_references.is_empty() {
            let references = tool_references
                .into_iter()
                .map(|row| safe_reference(row.get::<String, _>("identifier"), row.get("id")))
                .collect::<BTreeSet<_>>()
                .into_iter();
            return Err(reference_conflict("referenced_by_tool", references));
        }

        delete_entity_search_documents(&mut transaction, "function", &id.to_string())
            .await
            .map_err(search_error)?;
        let result = sqlx::query("DELETE FROM functions WHERE id = ?")
            .bind(id)
            .execute(&mut *transaction)
            .await
            .map_err(storage_error)?;
        if result.rows_affected() != 1 {
            return Err(not_found());
        }
        transaction.commit().await.map_err(storage_error)?;
        Ok(())
    }

    /// List one fixed 20-row page, optionally using the canonical literal
    /// Function search route.
    pub async fn list(
        &self,
        search: Option<String>,
        page: i64,
    ) -> Result<FunctionPage, PublicBoundaryError> {
        if page < 1 {
            return Err(invalid_input("page", "out_of_range"));
        }
        let offset = page
            .checked_sub(1)
            .and_then(|value| value.checked_mul(PAGE_SIZE))
            .ok_or_else(|| invalid_input("page", "out_of_range"))?;
        let route = FunctionSearchRoute::for_input(search)?;
        let (items, total) = match &route {
            FunctionSearchRoute::FtsPhrase(phrase) => {
                self.fetch_fts_page(phrase, PAGE_SIZE, offset).await?
            }
            _ => (
                self.fetch_window(&route, PAGE_SIZE, offset).await?,
                self.fetch_count(&route).await?,
            ),
        };
        Ok(FunctionPage { items, total, page })
    }

    async fn fetch_fts_page(
        &self,
        phrase: &str,
        limit: i64,
        offset: i64,
    ) -> Result<(Vec<FunctionRecord>, i64), PublicBoundaryError> {
        let rows = sqlx::query_as::<_, FunctionFtsPageRow>(FUNCTION_FTS_LIST_SQL)
            .bind(phrase)
            .bind(limit)
            .bind(offset)
            .fetch_all(&self.pool)
            .await
            .map_err(storage_error)?;
        let total = match rows.first() {
            Some(row) => row.total_count,
            None => sqlx::query_scalar::<_, i64>(FUNCTION_FTS_COUNT_SQL)
                .bind(phrase)
                .fetch_one(&self.pool)
                .await
                .map_err(storage_error)?,
        };
        let items = rows
            .into_iter()
            .map(|row| FunctionRecord::try_from(row.function))
            .collect::<Result<Vec<_>, _>>()?;
        Ok((items, total))
    }

    pub(crate) async fn list_window(
        &self,
        search: Option<String>,
        limit: i64,
        offset: i64,
    ) -> Result<Vec<FunctionRecord>, PublicBoundaryError> {
        let route = FunctionSearchRoute::for_input(search)?;
        self.fetch_window(&route, limit, offset).await
    }

    pub(crate) async fn count_matching(
        &self,
        search: Option<String>,
    ) -> Result<i64, PublicBoundaryError> {
        let route = FunctionSearchRoute::for_input(search)?;
        self.fetch_count(&route).await
    }

    async fn fetch_window(
        &self,
        route: &FunctionSearchRoute,
        limit: i64,
        offset: i64,
    ) -> Result<Vec<FunctionRecord>, PublicBoundaryError> {
        let rows = match route {
            FunctionSearchRoute::Unfiltered => {
                sqlx::query_as::<_, FunctionRow>(FUNCTION_UNFILTERED_LIST_SQL)
                    .bind(limit)
                    .bind(offset)
                    .fetch_all(&self.pool)
                    .await
                    .map_err(storage_error)?
            }
            FunctionSearchRoute::FtsPhrase(phrase) => {
                sqlx::query_as::<_, FunctionRow>(FUNCTION_FTS_LIST_SQL)
                    .bind(phrase)
                    .bind(limit)
                    .bind(offset)
                    .fetch_all(&self.pool)
                    .await
                    .map_err(storage_error)?
            }
            FunctionSearchRoute::ShortGram { length, gram } => {
                sqlx::query_as::<_, FunctionRow>(FUNCTION_SHORT_GRAM_LIST_SQL)
                    .bind(length)
                    .bind(gram)
                    .bind(limit)
                    .bind(offset)
                    .fetch_all(&self.pool)
                    .await
                    .map_err(storage_error)?
            }
        };
        rows.into_iter()
            .map(FunctionRecord::try_from)
            .collect::<Result<Vec<_>, _>>()
    }

    async fn fetch_count(&self, route: &FunctionSearchRoute) -> Result<i64, PublicBoundaryError> {
        match route {
            FunctionSearchRoute::Unfiltered => {
                sqlx::query_scalar::<_, i64>(FUNCTION_UNFILTERED_COUNT_SQL)
                    .fetch_one(&self.pool)
                    .await
                    .map_err(storage_error)
            }
            FunctionSearchRoute::FtsPhrase(phrase) => {
                sqlx::query_scalar::<_, i64>(FUNCTION_FTS_COUNT_SQL)
                    .bind(phrase)
                    .fetch_one(&self.pool)
                    .await
                    .map_err(storage_error)
            }
            FunctionSearchRoute::ShortGram { length, gram } => {
                sqlx::query_scalar::<_, i64>(FUNCTION_SHORT_GRAM_COUNT_SQL)
                    .bind(length)
                    .bind(gram)
                    .fetch_one(&self.pool)
                    .await
                    .map_err(storage_error)
            }
        }
    }

    /// Idempotently synchronize the exact immutable Builtin registry into a
    /// canonical v4 pool. Store-open calls this after migration and Capability
    /// registration; no runtime DDL is performed.
    pub(crate) async fn synchronize_builtins(
        pool: &Pool<Sqlite>,
    ) -> Result<(), PublicBoundaryError> {
        validate_builtin_registry()?;
        let mut transaction = pool.begin().await.map_err(storage_error)?;
        let existing_builtin_ids = sqlx::query_scalar::<_, String>(
            "SELECT identifier FROM functions WHERE kind = 'builtin' ORDER BY identifier",
        )
        .fetch_all(&mut *transaction)
        .await
        .map_err(storage_error)?;
        if existing_builtin_ids
            .iter()
            .any(|identifier| !BUILTIN_IDENTIFIERS.contains(&identifier.as_str()))
        {
            return Err(internal_error("builtin_registry_drift"));
        }

        for definition in BuiltinRegistry::definitions() {
            synchronize_builtin(&mut transaction, definition).await?;
        }
        transaction.commit().await.map_err(storage_error)?;
        Ok(())
    }
}

#[derive(Debug)]
struct PreparedFunctionInput {
    identifier: String,
    name: String,
    description: Option<String>,
    kind: FunctionKind,
    input_schema: String,
    output_schema: String,
    plugin_id: Option<i64>,
    plugin_export: Option<String>,
    category_id: Option<i64>,
    required_capabilities: Option<String>,
}

impl From<FunctionInput> for PreparedFunctionInput {
    fn from(input: FunctionInput) -> Self {
        Self {
            identifier: input.identifier,
            name: input.name,
            description: input.description,
            kind: input.kind,
            input_schema: input.input_schema,
            output_schema: input.output_schema,
            plugin_id: input.plugin_id,
            plugin_export: input.plugin_export,
            category_id: input.category_id,
            required_capabilities: input.required_capabilities,
        }
    }
}

async fn prepare_input(
    transaction: &mut Transaction<'_, Sqlite>,
    input: FunctionInput,
    authority: WriteAuthority,
) -> Result<PreparedFunctionInput, PublicBoundaryError> {
    if authority == WriteAuthority::LegacyFixture {
        return Ok(input.into());
    }
    if let Some(category_id) = input.category_id {
        let exists = sqlx::query_scalar::<_, i64>("SELECT 1 FROM categories WHERE id = ?")
            .bind(category_id)
            .fetch_optional(&mut **transaction)
            .await
            .map_err(storage_error)?
            .is_some();
        if !exists {
            return Err(invalid_input("category_id", "not_found"));
        }
    }

    let known_capabilities =
        sqlx::query_scalar::<_, String>("SELECT name FROM capabilities ORDER BY name")
            .fetch_all(&mut **transaction)
            .await
            .map_err(storage_error)?
            .into_iter()
            .collect::<BTreeSet<_>>();
    if let Some(required_capabilities) = input.required_capabilities.as_deref() {
        let capabilities = serde_json::from_str::<Vec<String>>(required_capabilities)
            .map_err(|_| invalid_input("required_capabilities", "invalid_json"))?;
        if capabilities
            .iter()
            .any(|capability| !known_capabilities.contains(capability))
        {
            return Err(invalid_input("required_capabilities", "unknown"));
        }
    }

    let (plugin_id, plugin_export, required_capabilities) = match input.kind {
        FunctionKind::Builtin => return Err(invalid_input("kind", "builtin_immutable")),
        FunctionKind::Placeholder => (None, None, None),
        FunctionKind::Custom => {
            let plugin_id = input
                .plugin_id
                .ok_or_else(|| invalid_input("plugin_id", "required"))?;
            let plugin_export = input
                .plugin_export
                .as_deref()
                .ok_or_else(|| invalid_input("plugin_export", "required"))?;
            if plugin_export.is_empty() {
                return Err(invalid_input("plugin_export", "empty"));
            }
            if plugin_export.len() > MAX_TEXT_BYTES {
                return Err(invalid_input("plugin_export", "too_long"));
            }
            let manifest = sqlx::query_scalar::<_, Option<String>>(
                "SELECT manifest FROM plugins WHERE id = ? AND deleted_at IS NULL",
            )
            .bind(plugin_id)
            .fetch_optional(&mut **transaction)
            .await
            .map_err(storage_error)?
            .flatten()
            .ok_or_else(|| invalid_input("plugin_id", "not_found"))?;
            validate_manifest(&manifest, &known_capabilities)
                .map_err(|_| invalid_input("plugin_id", "invalid_manifest"))?;
            if !manifest_exports(Some(&manifest))
                .iter()
                .any(|declared| declared == plugin_export)
            {
                return Err(invalid_input("plugin_export", "not_declared"));
            }
            (
                Some(plugin_id),
                Some(plugin_export.to_string()),
                input.required_capabilities.clone(),
            )
        }
    };

    Ok(PreparedFunctionInput {
        identifier: input.identifier,
        name: input.name,
        description: input.description,
        kind: input.kind,
        input_schema: input.input_schema,
        output_schema: input.output_schema,
        plugin_id,
        plugin_export,
        category_id: input.category_id,
        required_capabilities,
    })
}

async fn fetch_function(
    transaction: &mut Transaction<'_, Sqlite>,
    id: i64,
) -> Result<Option<FunctionRecord>, PublicBoundaryError> {
    sqlx::query_as::<_, FunctionRow>(FUNCTION_GET_SQL)
        .bind(id)
        .fetch_optional(&mut **transaction)
        .await
        .map_err(storage_error)?
        .map(FunctionRecord::try_from)
        .transpose()
}

async fn synchronize_builtin(
    transaction: &mut Transaction<'_, Sqlite>,
    definition: &BuiltinDefinition,
) -> Result<(), PublicBoundaryError> {
    let existing = sqlx::query_as::<_, FunctionRow>(
        "SELECT id, identifier, name, description, kind, input_schema, output_schema, \
                plugin_id, plugin_export, category_id, required_capabilities, created_at, updated_at \
         FROM functions WHERE identifier = ? COLLATE BINARY",
    )
    .bind(definition.identifier)
    .fetch_optional(&mut **transaction)
    .await
    .map_err(storage_error)?;
    let required_capabilities = if definition.required_capabilities.is_empty() {
        None
    } else {
        Some(
            serde_json::to_string(definition.required_capabilities)
                .map_err(|_| internal_error("builtin_registry"))?,
        )
    };
    let now = Utc::now().to_rfc3339();
    let id = match existing {
        Some(row) => {
            if FunctionKind::try_from(row.kind.as_str())? != FunctionKind::Builtin {
                return Err(internal_error("reserved_identifier_collision"));
            }
            let drifted = row.name != definition.name
                || row.description.as_deref() != Some(definition.description)
                || row.input_schema != definition.input_schema
                || row.output_schema != definition.output_schema
                || row.plugin_id.is_some()
                || row.plugin_export.is_some()
                || row.category_id.is_some()
                || row.required_capabilities != required_capabilities;
            if drifted {
                sqlx::query(
                    "UPDATE functions SET name = ?, description = ?, kind = 'builtin', \
                     input_schema = ?, output_schema = ?, plugin_id = NULL, plugin_export = NULL, \
                     category_id = NULL, required_capabilities = ?, updated_at = ? WHERE id = ?",
                )
                .bind(definition.name)
                .bind(definition.description)
                .bind(definition.input_schema)
                .bind(definition.output_schema)
                .bind(&required_capabilities)
                .bind(&now)
                .bind(row.id)
                .execute(&mut **transaction)
                .await
                .map_err(storage_error)?;
            }
            row.id
        }
        None => sqlx::query_scalar::<_, i64>(FUNCTION_INSERT_SQL)
            .bind(definition.identifier)
            .bind(definition.name)
            .bind(Some(definition.description))
            .bind(FunctionKind::Builtin.as_str())
            .bind(definition.input_schema)
            .bind(definition.output_schema)
            .bind(Option::<i64>::None)
            .bind(Option::<String>::None)
            .bind(Option::<i64>::None)
            .bind(&required_capabilities)
            .bind(&now)
            .bind(&now)
            .fetch_one(&mut **transaction)
            .await
            .map_err(storage_error)?,
    };
    replace_entity_search_documents(
        transaction,
        "function",
        &id.to_string(),
        &[
            ("identifier", definition.identifier),
            ("name", definition.name),
        ],
    )
    .await
    .map_err(search_error)
}

fn validate_builtin_registry() -> Result<(), PublicBoundaryError> {
    let definitions = BuiltinRegistry::definitions();
    let definition_ids = definitions
        .iter()
        .map(|definition| definition.identifier)
        .collect::<BTreeSet<_>>();
    let reserved_ids = BUILTIN_IDENTIFIERS.iter().copied().collect::<BTreeSet<_>>();
    if definitions.len() != 4 || definition_ids.len() != 4 || definition_ids != reserved_ids {
        return Err(internal_error("builtin_registry"));
    }
    Ok(())
}

fn reject_user_builtin_or_reserved(input: &FunctionInput) -> Result<(), PublicBoundaryError> {
    if input.kind == FunctionKind::Builtin {
        return Err(invalid_input("kind", "builtin_immutable"));
    }
    if BUILTIN_IDENTIFIERS.contains(&input.identifier.as_str()) {
        return Err(invalid_input("identifier", "reserved"));
    }
    Ok(())
}

fn validate_identifier(identifier: &str) -> Result<(), PublicBoundaryError> {
    if identifier.trim().is_empty() {
        return Err(invalid_input("identifier", "empty"));
    }
    if identifier.len() > MAX_TEXT_BYTES {
        return Err(invalid_input("identifier", "too_long"));
    }
    if !identifier
        .chars()
        .all(|character| character.is_ascii_alphanumeric() || matches!(character, '_' | '-'))
    {
        return Err(invalid_input("identifier", "invalid_format"));
    }
    Ok(())
}

fn validate_name(name: &str) -> Result<(), PublicBoundaryError> {
    if name.trim().is_empty() {
        return Err(invalid_input("name", "empty"));
    }
    if name.len() > MAX_TEXT_BYTES {
        return Err(invalid_input("name", "too_long"));
    }
    Ok(())
}

fn validate_description(description: &str) -> Result<(), PublicBoundaryError> {
    if description.len() > MAX_DESCRIPTION_BYTES {
        return Err(invalid_input("description", "too_long"));
    }
    Ok(())
}

fn validate_schema(field: &'static str, schema: &str) -> Result<(), PublicBoundaryError> {
    if schema.len() > MAX_SCHEMA_BYTES {
        return Err(invalid_input(field, "too_large"));
    }
    let value =
        serde_json::from_str::<Value>(schema).map_err(|_| invalid_input(field, "invalid_json"))?;
    CompiledJsonSchema::compile(&value).map_err(|error| invalid_input(field, error.category()))?;
    Ok(())
}

fn canonicalize_capability_json(value: &str) -> Result<String, PublicBoundaryError> {
    let values = serde_json::from_str::<Vec<Value>>(value)
        .map_err(|_| invalid_input("required_capabilities", "invalid_json"))?;
    let mut seen = BTreeSet::new();
    let mut canonical = Vec::with_capacity(values.len());
    for value in values {
        let Some(value) = value.as_str() else {
            return Err(invalid_input(
                "required_capabilities",
                "array_of_strings_required",
            ));
        };
        let value = value.trim();
        if value.is_empty() {
            return Err(invalid_input("required_capabilities", "empty"));
        }
        if !seen.insert(value.to_string()) {
            return Err(invalid_input("required_capabilities", "duplicate"));
        }
        canonical.push(value.to_string());
    }
    serde_json::to_string(&canonical).map_err(|_| internal_error("capability_serialization"))
}

fn invalid_input(field: impl Into<String>, reason: impl Into<String>) -> PublicBoundaryError {
    PublicBoundaryError::new(PublicErrorEnvelope::InvalidInput {
        field: field.into(),
        reason: reason.into(),
    })
}

fn not_found() -> PublicBoundaryError {
    PublicBoundaryError::new(PublicErrorEnvelope::NotFound)
}

fn internal_error(reason: impl Into<String>) -> PublicBoundaryError {
    PublicBoundaryError::new(PublicErrorEnvelope::Internal {
        reason: reason.into(),
    })
}

fn storage_error(error: sqlx::Error) -> PublicBoundaryError {
    internal_error("storage").with_cause(error)
}

fn search_error(error: super::search_index::SearchError) -> PublicBoundaryError {
    match error {
        super::search_index::SearchError::InvalidInput { reason } => {
            invalid_input("search", reason)
        }
        other => internal_error("search_index").with_cause(other),
    }
}

fn write_error(error: sqlx::Error, identifier: &str) -> PublicBoundaryError {
    if let sqlx::Error::Database(database_error) = &error
        && (database_error.is_unique_violation()
            || database_error
                .message()
                .contains("UNIQUE constraint failed: functions.identifier"))
    {
        return PublicBoundaryError::new(PublicErrorEnvelope::Conflict {
            shape: "value".to_string(),
            field: "identifier".to_string(),
            reason: "duplicate".to_string(),
        })
        .with_value(identifier.to_string());
    }
    storage_error(error)
}

fn reference_conflict(
    reason: &'static str,
    references: impl IntoIterator<Item = String>,
) -> PublicBoundaryError {
    PublicBoundaryError::new(PublicErrorEnvelope::Conflict {
        shape: "references".to_string(),
        field: "id".to_string(),
        reason: reason.to_string(),
    })
    .with_references(references)
}

fn safe_reference(identifier: String, id: i64) -> String {
    if !identifier.is_empty()
        && identifier
            .chars()
            .all(|character| character.is_ascii_alphanumeric() || matches!(character, '_' | '-'))
    {
        identifier
    } else {
        id.to_string()
    }
}
