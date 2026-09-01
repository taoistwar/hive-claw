//! SQL-backed persisted Tool boundary for HiveGUI.
//!
//! The v4 migration owns schema creation. This module owns complete FR-020
//! validation, target resolution, CRUD transactions, derived search rows, and
//! stable public errors. It never creates schema or calls HiveWeb.

#![warn(missing_docs)]

use std::collections::BTreeSet;

use chrono::Utc;
use hive_json_schema::CompiledJsonSchema;
use serde_json::Value;
use sqlx::{FromRow, Pool, Sqlite, Transaction};

use super::query_plan::{
    TOOL_CAPABILITY_GET_SQL, TOOL_FTS_COUNT_SQL, TOOL_FTS_LIST_SQL, TOOL_FUNCTION_TARGET_SQL,
    TOOL_GET_SQL, TOOL_SHORT_GRAM_COUNT_SQL, TOOL_SHORT_GRAM_LIST_SQL, TOOL_UNFILTERED_COUNT_SQL,
    TOOL_UNFILTERED_LIST_SQL, TOOL_WORKFLOW_TARGET_SQL,
};
use super::search_index::{
    IndexBackend, IndexSelection, canonical_normalizer, delete_entity_search_documents,
    fts_literal_phrase, normalize_search_query, replace_entity_search_documents,
};
use super::validation::{PublicBoundaryError, PublicErrorEnvelope};

const PAGE_SIZE: i64 = 20;
const MAX_TEXT_BYTES: usize = 255;
const MAX_DESCRIPTION_BYTES: usize = 2_000;
const MAX_SCHEMA_BYTES: usize = 1024 * 1024;

const TOOL_INSERT_SQL: &str = "INSERT INTO tools (\
    identifier, name, description, kind, source, is_always, function_id, workflow_id, \
    input_schema, output_schema, category_id, required_capabilities, created_at, updated_at\
) VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?) RETURNING id";

const TOOL_UPDATE_SQL: &str = "UPDATE tools SET identifier = ?, name = ?, description = ?, \
    kind = ?, source = ?, is_always = ?, function_id = ?, workflow_id = ?, input_schema = ?, \
    output_schema = ?, category_id = ?, required_capabilities = ?, updated_at = ? WHERE id = ?";

/// Stable persisted Tool target kind.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ToolKind {
    /// Wrap exactly one persisted Function.
    FunctionWrap,
    /// Wrap exactly one persisted Workflow.
    WorkflowWrap,
}

impl ToolKind {
    /// Stable wire/database value.
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::FunctionWrap => "function-wrap",
            Self::WorkflowWrap => "workflow-wrap",
        }
    }
}

impl TryFrom<&str> for ToolKind {
    type Error = PublicBoundaryError;

    fn try_from(value: &str) -> Result<Self, Self::Error> {
        match value {
            "function-wrap" => Ok(Self::FunctionWrap),
            "workflow-wrap" => Ok(Self::WorkflowWrap),
            _ => Err(invalid_input("kind", "invalid_enum")),
        }
    }
}

impl TryFrom<String> for ToolKind {
    type Error = PublicBoundaryError;

    fn try_from(value: String) -> Result<Self, Self::Error> {
        Self::try_from(value.as_str())
    }
}

/// Stable persisted Tool source.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ToolSource {
    /// User-managed local Tool.
    Workspace,
    /// Application-provided local Tool.
    Builtin,
}

impl ToolSource {
    /// Stable wire/database value.
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Workspace => "workspace",
            Self::Builtin => "builtin",
        }
    }
}

impl TryFrom<&str> for ToolSource {
    type Error = PublicBoundaryError;

    fn try_from(value: &str) -> Result<Self, Self::Error> {
        match value {
            "workspace" => Ok(Self::Workspace),
            "builtin" => Ok(Self::Builtin),
            _ => Err(invalid_input("source", "invalid_enum")),
        }
    }
}

impl TryFrom<String> for ToolSource {
    type Error = PublicBoundaryError;

    fn try_from(value: String) -> Result<Self, Self::Error> {
        Self::try_from(value.as_str())
    }
}

/// Complete shared Tool create/update DTO.
#[derive(Debug, Clone)]
pub struct ToolInput {
    identifier: String,
    name: String,
    description: String,
    kind: ToolKind,
    source: ToolSource,
    is_always: bool,
    function_id: Option<i64>,
    workflow_id: Option<i64>,
    input_schema: String,
    output_schema: String,
    category_id: Option<i64>,
    required_capabilities: Option<String>,
}

impl ToolInput {
    /// Validate and construct the complete FR-020 write DTO.
    #[allow(clippy::too_many_arguments)]
    pub fn for_write(
        identifier: String,
        name: String,
        description: String,
        kind: ToolKind,
        source: ToolSource,
        is_always: bool,
        function_id: Option<i64>,
        workflow_id: Option<i64>,
        input_schema: String,
        output_schema: String,
        category_id: Option<i64>,
        required_capabilities: Option<String>,
    ) -> Result<Self, PublicBoundaryError> {
        validate_identifier(&identifier)?;
        validate_name(&name)?;
        validate_description(&description)?;
        validate_schema("input_schema", &input_schema)?;
        validate_schema("output_schema", &output_schema)?;
        if function_id.is_some_and(|id| id <= 0) {
            return Err(invalid_input("function_id", "out_of_range"));
        }
        if workflow_id.is_some_and(|id| id <= 0) {
            return Err(invalid_input("workflow_id", "out_of_range"));
        }
        if category_id.is_some_and(|id| id <= 0) {
            return Err(invalid_input("category_id", "out_of_range"));
        }
        validate_target_xor(kind, function_id, workflow_id)?;
        let required_capabilities = required_capabilities
            .as_deref()
            .map(canonicalize_capability_json)
            .transpose()?;
        Ok(Self {
            identifier,
            name,
            description,
            kind,
            source,
            is_always,
            function_id,
            workflow_id,
            input_schema,
            output_schema,
            category_id,
            required_capabilities,
        })
    }

    /// Identifier.
    pub fn identifier(&self) -> &str {
        &self.identifier
    }
    /// Display name.
    pub fn name(&self) -> &str {
        &self.name
    }
    /// Description.
    pub fn description(&self) -> &str {
        &self.description
    }
    /// Target kind.
    pub fn kind(&self) -> ToolKind {
        self.kind
    }
    /// Source.
    pub fn source(&self) -> ToolSource {
        self.source
    }
    /// Whether this Tool is always exposed to its Agent.
    pub fn is_always(&self) -> bool {
        self.is_always
    }
    /// Function target id.
    pub fn function_id(&self) -> Option<i64> {
        self.function_id
    }
    /// Workflow target id.
    pub fn workflow_id(&self) -> Option<i64> {
        self.workflow_id
    }
    /// Input JSON Schema.
    pub fn input_schema(&self) -> &str {
        &self.input_schema
    }
    /// Output JSON Schema.
    pub fn output_schema(&self) -> &str {
        &self.output_schema
    }
    /// Optional category id.
    pub fn category_id(&self) -> Option<i64> {
        self.category_id
    }
    /// Ordered canonical required-Capability JSON.
    pub fn required_capabilities(&self) -> Option<&str> {
        self.required_capabilities.as_deref()
    }
}

/// Complete persisted Tool record.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ToolRecord {
    id: i64,
    identifier: String,
    name: String,
    description: String,
    kind: ToolKind,
    source: ToolSource,
    is_always: bool,
    function_id: Option<i64>,
    workflow_id: Option<i64>,
    input_schema: String,
    output_schema: String,
    category_id: Option<i64>,
    required_capabilities: Option<String>,
    created_at: String,
    updated_at: String,
}

impl ToolRecord {
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
    /// Description.
    pub fn description(&self) -> &str {
        &self.description
    }
    /// Target kind.
    pub fn kind(&self) -> ToolKind {
        self.kind
    }
    /// Source.
    pub fn source(&self) -> ToolSource {
        self.source
    }
    /// Always-exposed flag.
    pub fn is_always(&self) -> bool {
        self.is_always
    }
    /// Function target id.
    pub fn function_id(&self) -> Option<i64> {
        self.function_id
    }
    /// Workflow target id.
    pub fn workflow_id(&self) -> Option<i64> {
        self.workflow_id
    }
    /// Input JSON Schema.
    pub fn input_schema(&self) -> &str {
        &self.input_schema
    }
    /// Output JSON Schema.
    pub fn output_schema(&self) -> &str {
        &self.output_schema
    }
    /// Optional category id.
    pub fn category_id(&self) -> Option<i64> {
        self.category_id
    }
    /// Ordered required-Capability JSON.
    pub fn required_capabilities(&self) -> Option<&str> {
        self.required_capabilities.as_deref()
    }
    /// Creation timestamp.
    pub fn created_at(&self) -> &str {
        &self.created_at
    }
    /// Last update timestamp.
    pub fn updated_at(&self) -> &str {
        &self.updated_at
    }

    pub(crate) fn into_legacy_entity(self) -> super::entity_store::Tool {
        super::entity_store::Tool {
            id: self.id,
            identifier: self.identifier,
            name: self.name,
            description: self.description,
            kind: self.kind.as_str().to_string(),
            source: self.source.as_str().to_string(),
            is_always: self.is_always,
            function_id: self.function_id,
            workflow_id: self.workflow_id,
            input_schema: self.input_schema,
            output_schema: self.output_schema,
            category_id: self.category_id,
            required_capabilities: self.required_capabilities,
            created_at: self.created_at,
            updated_at: self.updated_at,
        }
    }
}

/// Fixed 20-row, one-based Tool page.
#[derive(Debug, Clone)]
pub struct ToolPage {
    items: Vec<ToolRecord>,
    total: i64,
    page: i64,
}

impl ToolPage {
    /// Page records.
    pub fn items(&self) -> &[ToolRecord] {
        &self.items
    }
    /// Total matching records.
    pub fn total(&self) -> i64 {
        self.total
    }
    /// One-based page number.
    pub fn page(&self) -> i64 {
        self.page
    }
    /// Fixed page size.
    pub fn page_size(&self) -> i64 {
        PAGE_SIZE
    }
}

/// Compatibility enum retained for downstream source stability. New methods
/// return [`PublicBoundaryError`] directly.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ToolStoreErrorKind {
    /// Invalid input.
    InvalidInput,
    /// Builtin row is immutable.
    BuiltinImmutable,
    /// Referenced row cannot be deleted.
    ReferencedByWorkflow,
    /// Storage failure.
    Io,
}

/// Compatibility alias for the stable public boundary error.
pub type ToolStoreError = PublicBoundaryError;

#[derive(Debug, FromRow)]
struct ToolRow {
    id: i64,
    identifier: String,
    name: String,
    description: String,
    kind: String,
    source: String,
    is_always: bool,
    function_id: Option<i64>,
    workflow_id: Option<i64>,
    input_schema: String,
    output_schema: String,
    category_id: Option<i64>,
    required_capabilities: Option<String>,
    created_at: String,
    updated_at: String,
}

#[derive(Debug, FromRow)]
struct ToolFtsPageRow {
    #[sqlx(flatten)]
    tool: ToolRow,
    total_count: i64,
}

impl TryFrom<ToolRow> for ToolRecord {
    type Error = PublicBoundaryError;

    fn try_from(row: ToolRow) -> Result<Self, Self::Error> {
        Ok(Self {
            id: row.id,
            identifier: row.identifier,
            name: row.name,
            description: row.description,
            kind: ToolKind::try_from(row.kind.as_str())
                .map_err(|_| internal_error("persisted_kind"))?,
            source: ToolSource::try_from(row.source.as_str())
                .map_err(|_| internal_error("persisted_source"))?,
            is_always: row.is_always,
            function_id: row.function_id,
            workflow_id: row.workflow_id,
            input_schema: row.input_schema,
            output_schema: row.output_schema,
            category_id: row.category_id,
            required_capabilities: row.required_capabilities,
            created_at: row.created_at,
            updated_at: row.updated_at,
        })
    }
}

#[derive(Debug, Clone)]
enum ToolSearchRoute {
    Unfiltered,
    FtsPhrase(String),
    ShortGram { length: i64, gram: String },
}

impl ToolSearchRoute {
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

/// Unique SQL-backed Tool store over a supplied canonical v4 pool.
#[derive(Debug, Clone)]
pub struct ToolStore {
    pool: Pool<Sqlite>,
}

impl ToolStore {
    /// Construct a Tool store. No schema is created or repaired here.
    pub fn new(pool: Pool<Sqlite>) -> Result<Self, PublicBoundaryError> {
        Ok(Self { pool })
    }

    /// Fail with the stable identifier conflict when another Tool owns the
    /// exact value. UI callers use this before reporting other incomplete-form
    /// diagnostics so a duplicate never loses the user's safe value.
    pub async fn check_identifier_available(
        &self,
        identifier: &str,
        except_id: Option<i64>,
    ) -> Result<(), PublicBoundaryError> {
        validate_identifier(identifier)?;
        let existing = sqlx::query_scalar::<_, i64>(
            "SELECT id FROM tools WHERE identifier = ? COLLATE BINARY",
        )
        .bind(identifier)
        .fetch_optional(&self.pool)
        .await
        .map_err(storage_error)?;
        if existing.is_some_and(|id| Some(id) != except_id) {
            return Err(identifier_conflict(identifier));
        }
        Ok(())
    }

    /// Create a Tool and its search documents in one transaction.
    pub async fn create(&self, input: ToolInput) -> Result<ToolRecord, PublicBoundaryError> {
        self.check_identifier_available(&input.identifier, None)
            .await?;
        let mut transaction = self.pool.begin().await.map_err(storage_error)?;
        validate_relations(&mut transaction, &input).await?;
        let now = Utc::now().to_rfc3339();
        let id = sqlx::query_scalar::<_, i64>(TOOL_INSERT_SQL)
            .bind(&input.identifier)
            .bind(&input.name)
            .bind(&input.description)
            .bind(input.kind.as_str())
            .bind(input.source.as_str())
            .bind(input.is_always)
            .bind(input.function_id)
            .bind(input.workflow_id)
            .bind(&input.input_schema)
            .bind(&input.output_schema)
            .bind(input.category_id)
            .bind(&input.required_capabilities)
            .bind(&now)
            .bind(&now)
            .fetch_one(&mut *transaction)
            .await
            .map_err(|error| write_error(error, &input.identifier))?;
        replace_entity_search_documents(
            &mut transaction,
            "tool",
            &id.to_string(),
            &[("identifier", &input.identifier), ("name", &input.name)],
        )
        .await
        .map_err(search_error)?;
        let record = fetch_tool(&mut transaction, id)
            .await?
            .ok_or_else(|| internal_error("write_visibility"))?;
        transaction.commit().await.map_err(storage_error)?;
        Ok(record)
    }

    /// Load one Tool by id.
    pub async fn get(&self, id: i64) -> Result<Option<ToolRecord>, PublicBoundaryError> {
        if id <= 0 {
            return Err(invalid_input("id", "out_of_range"));
        }
        sqlx::query_as::<_, ToolRow>(TOOL_GET_SQL)
            .bind(id)
            .fetch_optional(&self.pool)
            .await
            .map_err(storage_error)?
            .map(ToolRecord::try_from)
            .transpose()
    }

    /// Update a workspace Tool and its search documents atomically.
    pub async fn update(
        &self,
        id: i64,
        input: ToolInput,
    ) -> Result<ToolRecord, PublicBoundaryError> {
        if id <= 0 {
            return Err(invalid_input("id", "out_of_range"));
        }
        let mut transaction = self.pool.begin().await.map_err(storage_error)?;
        let existing = fetch_tool(&mut transaction, id)
            .await?
            .ok_or_else(not_found)?;
        if existing.source == ToolSource::Builtin {
            return Err(invalid_input("source", "builtin_immutable"));
        }
        validate_relations(&mut transaction, &input).await?;
        let now = Utc::now().to_rfc3339();
        let result = sqlx::query(TOOL_UPDATE_SQL)
            .bind(&input.identifier)
            .bind(&input.name)
            .bind(&input.description)
            .bind(input.kind.as_str())
            .bind(input.source.as_str())
            .bind(input.is_always)
            .bind(input.function_id)
            .bind(input.workflow_id)
            .bind(&input.input_schema)
            .bind(&input.output_schema)
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
            "tool",
            &id.to_string(),
            &[("identifier", &input.identifier), ("name", &input.name)],
        )
        .await
        .map_err(search_error)?;
        let record = fetch_tool(&mut transaction, id)
            .await?
            .ok_or_else(|| internal_error("write_visibility"))?;
        transaction.commit().await.map_err(storage_error)?;
        Ok(record)
    }

    /// Delete an unreferenced workspace Tool and its search documents.
    pub async fn delete(&self, id: i64) -> Result<(), PublicBoundaryError> {
        if id <= 0 {
            return Err(invalid_input("id", "out_of_range"));
        }
        let mut transaction = self.pool.begin().await.map_err(storage_error)?;
        let existing = fetch_tool(&mut transaction, id)
            .await?
            .ok_or_else(not_found)?;
        if existing.source == ToolSource::Builtin {
            return Err(invalid_input("source", "builtin_immutable"));
        }
        let references = sqlx::query_scalar::<_, String>(
            "SELECT agents.identifier FROM agent_tools \
             JOIN agents ON agents.id = agent_tools.agent_id \
             WHERE agent_tools.tool_id = ? ORDER BY agents.identifier",
        )
        .bind(id)
        .fetch_all(&mut *transaction)
        .await
        .map_err(storage_error)?;
        if !references.is_empty() {
            return Err(PublicBoundaryError::new(PublicErrorEnvelope::Conflict {
                shape: "references".to_string(),
                field: "id".to_string(),
                reason: "referenced_by_agent".to_string(),
            })
            .with_references(references));
        }
        delete_entity_search_documents(&mut transaction, "tool", &id.to_string())
            .await
            .map_err(search_error)?;
        let result = sqlx::query("DELETE FROM tools WHERE id = ?")
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

    /// List one fixed 20-row page with canonical literal search.
    pub async fn list(
        &self,
        search: Option<String>,
        page: i64,
    ) -> Result<ToolPage, PublicBoundaryError> {
        if page < 1 {
            return Err(invalid_input("page", "out_of_range"));
        }
        let offset = page
            .checked_sub(1)
            .and_then(|value| value.checked_mul(PAGE_SIZE))
            .ok_or_else(|| invalid_input("page", "out_of_range"))?;
        let route = ToolSearchRoute::for_input(search)?;
        let (items, total) = match &route {
            ToolSearchRoute::FtsPhrase(phrase) => {
                self.fetch_fts_page(phrase, PAGE_SIZE, offset).await?
            }
            _ => (
                self.fetch_window(&route, PAGE_SIZE, offset).await?,
                self.fetch_count(&route).await?,
            ),
        };
        Ok(ToolPage { items, total, page })
    }

    async fn fetch_fts_page(
        &self,
        phrase: &str,
        limit: i64,
        offset: i64,
    ) -> Result<(Vec<ToolRecord>, i64), PublicBoundaryError> {
        let rows = sqlx::query_as::<_, ToolFtsPageRow>(TOOL_FTS_LIST_SQL)
            .bind(phrase)
            .bind(limit)
            .bind(offset)
            .fetch_all(&self.pool)
            .await
            .map_err(storage_error)?;
        let total = match rows.first() {
            Some(row) => row.total_count,
            None => sqlx::query_scalar::<_, i64>(TOOL_FTS_COUNT_SQL)
                .bind(phrase)
                .fetch_one(&self.pool)
                .await
                .map_err(storage_error)?,
        };
        let items = rows
            .into_iter()
            .map(|row| ToolRecord::try_from(row.tool))
            .collect::<Result<Vec<_>, _>>()?;
        Ok((items, total))
    }

    pub(crate) async fn list_window(
        &self,
        search: Option<String>,
        limit: i64,
        offset: i64,
    ) -> Result<Vec<ToolRecord>, PublicBoundaryError> {
        let route = ToolSearchRoute::for_input(search)?;
        self.fetch_window(&route, limit, offset).await
    }

    async fn fetch_window(
        &self,
        route: &ToolSearchRoute,
        limit: i64,
        offset: i64,
    ) -> Result<Vec<ToolRecord>, PublicBoundaryError> {
        let rows = match route {
            ToolSearchRoute::Unfiltered => sqlx::query_as::<_, ToolRow>(TOOL_UNFILTERED_LIST_SQL)
                .bind(limit)
                .bind(offset)
                .fetch_all(&self.pool)
                .await
                .map_err(storage_error)?,
            ToolSearchRoute::FtsPhrase(phrase) => sqlx::query_as::<_, ToolRow>(TOOL_FTS_LIST_SQL)
                .bind(phrase)
                .bind(limit)
                .bind(offset)
                .fetch_all(&self.pool)
                .await
                .map_err(storage_error)?,
            ToolSearchRoute::ShortGram { length, gram } => {
                sqlx::query_as::<_, ToolRow>(TOOL_SHORT_GRAM_LIST_SQL)
                    .bind(length)
                    .bind(gram)
                    .bind(limit)
                    .bind(offset)
                    .fetch_all(&self.pool)
                    .await
                    .map_err(storage_error)?
            }
        };
        rows.into_iter().map(ToolRecord::try_from).collect()
    }

    async fn fetch_count(&self, route: &ToolSearchRoute) -> Result<i64, PublicBoundaryError> {
        match route {
            ToolSearchRoute::Unfiltered => sqlx::query_scalar(TOOL_UNFILTERED_COUNT_SQL)
                .fetch_one(&self.pool)
                .await
                .map_err(storage_error),
            ToolSearchRoute::FtsPhrase(phrase) => sqlx::query_scalar(TOOL_FTS_COUNT_SQL)
                .bind(phrase)
                .fetch_one(&self.pool)
                .await
                .map_err(storage_error),
            ToolSearchRoute::ShortGram { length, gram } => {
                sqlx::query_scalar(TOOL_SHORT_GRAM_COUNT_SQL)
                    .bind(length)
                    .bind(gram)
                    .fetch_one(&self.pool)
                    .await
                    .map_err(storage_error)
            }
        }
    }
}

async fn validate_relations(
    transaction: &mut Transaction<'_, Sqlite>,
    input: &ToolInput,
) -> Result<(), PublicBoundaryError> {
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
    if let Some(capabilities) = input.required_capabilities.as_deref() {
        for capability in serde_json::from_str::<Vec<String>>(capabilities)
            .map_err(|_| invalid_input("required_capabilities", "invalid_json"))?
        {
            let known = sqlx::query_scalar::<_, String>(TOOL_CAPABILITY_GET_SQL)
                .bind(&capability)
                .fetch_optional(&mut **transaction)
                .await
                .map_err(storage_error)?;
            if known.is_none() {
                return Err(invalid_input("required_capabilities", "unknown"));
            }
        }
    }
    let (target_input, target_output) = match input.kind {
        ToolKind::FunctionWrap => {
            let id = input
                .function_id
                .expect("constructor enforces function target");
            sqlx::query_as::<_, (String, String)>(TOOL_FUNCTION_TARGET_SQL)
                .bind(id)
                .fetch_optional(&mut **transaction)
                .await
                .map_err(storage_error)?
                .ok_or_else(|| invalid_input("function_id", "not_found"))?
        }
        ToolKind::WorkflowWrap => {
            let id = input
                .workflow_id
                .expect("constructor enforces workflow target");
            let (input_schema, output_schema) =
                sqlx::query_as::<_, (Option<String>, Option<String>)>(TOOL_WORKFLOW_TARGET_SQL)
                    .bind(id)
                    .fetch_optional(&mut **transaction)
                    .await
                    .map_err(storage_error)?
                    .ok_or_else(|| invalid_input("workflow_id", "not_found"))?;
            (
                input_schema.unwrap_or_else(|| "{}".to_string()),
                output_schema.unwrap_or_else(|| "{}".to_string()),
            )
        }
    };
    if !schema_equivalent(&input.input_schema, &target_input) {
        return Err(invalid_input("input_schema", "target_mismatch"));
    }
    if !schema_equivalent(&input.output_schema, &target_output) {
        return Err(invalid_input("output_schema", "target_mismatch"));
    }
    Ok(())
}

async fn fetch_tool(
    transaction: &mut Transaction<'_, Sqlite>,
    id: i64,
) -> Result<Option<ToolRecord>, PublicBoundaryError> {
    sqlx::query_as::<_, ToolRow>(TOOL_GET_SQL)
        .bind(id)
        .fetch_optional(&mut **transaction)
        .await
        .map_err(storage_error)?
        .map(ToolRecord::try_from)
        .transpose()
}

fn validate_target_xor(
    kind: ToolKind,
    function_id: Option<i64>,
    workflow_id: Option<i64>,
) -> Result<(), PublicBoundaryError> {
    let valid = matches!(
        (kind, function_id, workflow_id),
        (ToolKind::FunctionWrap, Some(_), None) | (ToolKind::WorkflowWrap, None, Some(_))
    );
    if valid {
        Ok(())
    } else {
        Err(invalid_input("target", "kind_xor"))
    }
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

fn schema_equivalent(left: &str, right: &str) -> bool {
    match (
        serde_json::from_str::<Value>(left),
        serde_json::from_str::<Value>(right),
    ) {
        (Ok(left), Ok(right)) => left == right,
        _ => false,
    }
}

fn canonicalize_capability_json(value: &str) -> Result<String, PublicBoundaryError> {
    let values = serde_json::from_str::<Vec<Value>>(value)
        .map_err(|_| invalid_input("required_capabilities", "invalid_json"))?;
    let mut seen = BTreeSet::new();
    let mut canonical = Vec::with_capacity(values.len());
    for value in values {
        let value = value
            .as_str()
            .ok_or_else(|| invalid_input("required_capabilities", "array_of_strings_required"))?
            .trim();
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
                .contains("UNIQUE constraint failed: tools.identifier"))
    {
        return identifier_conflict(identifier);
    }
    storage_error(error)
}

fn identifier_conflict(identifier: &str) -> PublicBoundaryError {
    PublicBoundaryError::new(PublicErrorEnvelope::Conflict {
        shape: "value".to_string(),
        field: "identifier".to_string(),
        reason: "duplicate".to_string(),
    })
    .with_value(identifier.to_string())
}
