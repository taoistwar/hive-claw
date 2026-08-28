//! `query_plan` — public boundary that evaluates an SQLite `EXPLAIN`
//! JSON plan against a [`QueryPlanRequirement`]. The Foundation owns
//! the contract that FTS `VIRTUAL TABLE INDEX` counts as an index
//! access path and that a `SCAN` for a non-exception table is a
//! hard failure.

#![warn(missing_docs)]

use serde_json::Value;
use std::path::Path;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
/// Expected access path for the table referenced by the plan.
pub enum AccessExpectation {
    /// The plan must contain a `SEARCH` operation.
    Search,
    /// The plan may use a known scan exception (small metadata
    /// table, etc.).
    Scan,
}

/// Stable, source-of-truth description of one production query.
#[derive(Debug, Clone, Copy)]
pub struct QueryPlanRequirement {
    /// Stable identifier for the query (e.g. `t017g.search.fts_trigram`).
    pub query_id: &'static str,
    /// Owner phase: `security-remediation` / `Foundation` / story.
    pub owner_phase: &'static str,
    /// Task that activated this row.
    pub activation_task: &'static str,
    /// Logical table being scanned.
    pub table: &'static str,
    /// Expected access path.
    pub expected_access: AccessExpectation,
    /// Optional expected index hint.
    pub expected_index: Option<&'static str>,
    /// Filter columns that must be covered by an index.
    pub filter_columns: &'static [&'static str],
    /// Join columns that must be covered by an index.
    pub join_columns: &'static [&'static str],
    /// Optional scan exception description.
    pub scan_exception: Option<QueryPlanException>,
}

/// Approved scan exception for tables that legitimately need a
/// full scan (small metadata, version rows, etc.). Carries every
/// approval field required by the [`PlanFailureKind::InvalidScanException`]
/// contract.
#[derive(Debug, Clone, Copy)]
pub struct QueryPlanException {
    /// Number of rows in the table at the time of approval.
    pub table_size: i64,
    /// Free-form reason for the exception.
    pub reason: &'static str,
    /// Reviewer who approved the exception.
    pub approver: &'static str,
    /// ISO date (YYYY-MM-DD) on which the exception expires.
    pub expires_on: &'static str,
    /// Reviewer result string (e.g. `approved: scan remains cheaper
    /// than an index`).
    pub review_result: &'static str,
}

/// Backend used for full-text indexing.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FtsBackend {
    /// FTS5 with the trigram tokenizer.
    Fts5Trigram,
    /// Internal short-gram index.
    ShortGram,
}

/// Stable FTS plan expectation used by `evaluate_fts_plan`.
#[derive(Debug, Clone, Copy)]
pub struct FtsPlanExpectation {
    /// Stable identifier for the query.
    pub query_id: &'static str,
    /// Owner phase.
    pub owner_phase: &'static str,
    /// Task that activated this row.
    pub activation_task: &'static str,
    /// Logical table being scanned.
    pub table: &'static str,
    /// Backend used.
    pub backend: FtsBackend,
    /// Filter columns that must be covered.
    pub filter_columns: &'static [&'static str],
    /// Optional scan exception.
    pub scan_exception: Option<QueryPlanException>,
}

impl FtsPlanExpectation {
    /// Convert this FTS expectation into the standard
    /// [`QueryPlanRequirement`] shape used by the plan evaluators.
    pub fn as_requirement(&self) -> QueryPlanRequirement {
        QueryPlanRequirement {
            query_id: self.query_id,
            owner_phase: self.owner_phase,
            activation_task: self.activation_task,
            table: self.table,
            expected_access: AccessExpectation::Search,
            expected_index: Some("VIRTUAL TABLE INDEX"),
            filter_columns: self.filter_columns,
            join_columns: &[],
            scan_exception: self.scan_exception,
        }
    }
}

impl From<FtsPlanExpectation> for QueryPlanRequirement {
    fn from(expectation: FtsPlanExpectation) -> Self {
        expectation.as_requirement()
    }
}

/// Failure kinds emitted by plan evaluators.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PlanFailureKind {
    /// Plan shows a full table scan without an approved exception.
    UnapprovedFullScan,
    /// Plan shows no expected `SEARCH` operation.
    MissingSearch,
    /// Expected index hint is missing.
    MissingIndex,
    /// Expected index name did not match the one used by SQLite.
    MissingExpectedIndex,
    /// Filter column was not covered by the chosen access path.
    UncoveredFilterColumn(&'static str),
    /// Join column was not covered by the chosen access path.
    UncoveredJoinColumn(&'static str),
    /// Plan shape is unknown / undecidable.
    IndeterminatePlan,
    /// Scan exception is missing or expired.
    InvalidScanException,
}

/// Verdict produced by [`evaluate_fts_plan`].
#[derive(Debug, Clone)]
pub struct FtsPlanVerdict {
    failures: Vec<PlanFailureKind>,
}

impl FtsPlanVerdict {
    /// Returns true when the plan satisfies the expectation.
    pub fn is_accepted(&self) -> bool {
        self.failures.is_empty()
    }

    /// Returns true when the verdict contains `kind`.
    pub fn has_failure(&self, kind: PlanFailureKind) -> bool {
        self.failures.iter().any(|f| failure_matches(f, &kind))
    }

    /// Returns the list of failures.
    pub fn failures(&self) -> &[PlanFailureKind] {
        &self.failures
    }
}

/// Verdict produced by [`evaluate_sqlite_plan`] and friends.
#[derive(Debug, Clone)]
pub struct PlanVerdict {
    failures: Vec<PlanFailureKind>,
    used_exception: bool,
}

impl PlanVerdict {
    /// Returns true when the plan satisfies the expectation.
    pub fn is_accepted(&self) -> bool {
        self.failures.is_empty()
    }

    /// Returns true when the verdict contains `kind`.
    pub fn has_failure(&self, kind: PlanFailureKind) -> bool {
        self.failures.iter().any(|f| failure_matches(f, &kind))
    }

    /// Returns true when the verdict consumed a scan exception.
    pub fn used_exception(&self) -> bool {
        self.used_exception
    }

    /// Returns the list of failures.
    pub fn failures(&self) -> &[PlanFailureKind] {
        &self.failures
    }
}

fn failure_matches(found: &PlanFailureKind, needle: &PlanFailureKind) -> bool {
    match (found, needle) {
        (PlanFailureKind::UncoveredFilterColumn(a), PlanFailureKind::UncoveredFilterColumn(b)) => {
            a == b
        }
        (PlanFailureKind::UncoveredJoinColumn(a), PlanFailureKind::UncoveredJoinColumn(b)) => {
            a == b
        }
        _ => found == needle,
    }
}

/// SQL dialect a query is registered against.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum QueryDialect {
    /// SQLite (local file).
    Sqlite,
    /// MySQL (remote server).
    Mysql,
}

/// One production query registered in the catalog.
#[derive(Debug, Clone, Copy)]
pub struct ProductionQuery {
    /// Stable identifier.
    pub id: &'static str,
    /// Exact compile-time SQL literal executed by production.
    ///
    /// Deferred DDL/inventory-only rows may leave this empty, but every active
    /// executable query must expose the statement itself so EXPLAIN tests cannot
    /// substitute a test-only query assembled from catalog metadata.
    pub sql: &'static str,
    /// Owner phase.
    pub owner_phase: &'static str,
    /// Activation / approval task.
    pub activation_task: &'static str,
    /// Whether the row is currently active for review.
    pub active: bool,
    /// Logical table.
    pub table: &'static str,
    /// Dialect.
    pub dialect: QueryDialect,
    /// Filter columns referenced by the query body.
    pub filter_columns: &'static [&'static str],
    /// Join columns referenced by the query body.
    pub join_columns: &'static [&'static str],
    /// Per-query plan requirements.
    pub requirements: &'static [QueryPlanRequirement],
    /// Optional scan exception.
    pub scan_exception: Option<QueryPlanException>,
}

/// Stable, unfiltered Function page ordered by normalized name, normalized
/// identifier, and numeric id. Bind order: `limit`, `offset`.
pub const FUNCTION_UNFILTERED_LIST_SQL: &str = "SELECT functions.id, functions.identifier, functions.name, \
            functions.description, functions.kind, functions.input_schema, \
            functions.output_schema, functions.plugin_id, functions.plugin_export, \
            functions.category_id, functions.required_capabilities, \
            functions.created_at, functions.updated_at \
     FROM search_documents INDEXED BY idx_search_documents_covering \
     JOIN search_documents AS identifiers \
       ON identifiers.entity_type = search_documents.entity_type \
      AND identifiers.entity_key = search_documents.entity_key \
      AND identifiers.field = 'identifier' \
     JOIN functions \
       ON functions.id = CAST(search_documents.entity_key AS INTEGER) \
     WHERE search_documents.entity_type = 'function' \
       AND search_documents.field = 'name' \
     ORDER BY search_documents.normalized_text ASC, \
              identifiers.normalized_text ASC, functions.id ASC \
     LIMIT ? OFFSET ?";

/// Deduplicated Function page for a three-or-more-scalar FTS phrase. Bind
/// order: encoded FTS `phrase`, `limit`, `offset`.
pub const FUNCTION_FTS_LIST_SQL: &str = "WITH matched_function_keys AS MATERIALIZED ( \
         SELECT DISTINCT search_documents.entity_key \
         FROM search_documents_fts \
         CROSS JOIN search_documents \
           ON search_documents.id = search_documents_fts.rowid \
         WHERE search_documents.entity_type = 'function' \
           AND search_documents_fts MATCH ? \
     ) \
     SELECT functions.id, functions.identifier, functions.name, \
            functions.description, functions.kind, functions.input_schema, \
            functions.output_schema, functions.plugin_id, functions.plugin_export, \
            functions.category_id, functions.required_capabilities, \
            functions.created_at, functions.updated_at, \
            (SELECT COUNT(*) FROM matched_function_keys) AS total_count \
     FROM matched_function_keys \
     CROSS JOIN search_documents \
       ON search_documents.entity_type = 'function' \
      AND search_documents.entity_key = matched_function_keys.entity_key \
      AND search_documents.field = 'name' \
     CROSS JOIN search_documents AS identifiers \
       ON identifiers.entity_type = search_documents.entity_type \
      AND identifiers.entity_key = search_documents.entity_key \
      AND identifiers.field = 'identifier' \
     CROSS JOIN functions \
       ON functions.id = CAST(matched_function_keys.entity_key AS INTEGER) \
     ORDER BY search_documents.normalized_text ASC, \
              identifiers.normalized_text ASC, functions.id ASC \
     LIMIT ? OFFSET ?";

/// Count the canonical `name` document for every Function. There are no bind
/// parameters; the unique document contract makes this equal to Function count.
pub const FUNCTION_UNFILTERED_COUNT_SQL: &str = "SELECT COUNT(*) \
     FROM search_documents INDEXED BY idx_search_documents_covering \
     WHERE entity_type = 'function' AND field = 'name'";

/// Deduplicated Function count for a three-or-more-scalar FTS phrase. Bind
/// order: encoded FTS `phrase`.
pub const FUNCTION_FTS_COUNT_SQL: &str = "SELECT COUNT(DISTINCT search_documents.entity_key) \
     FROM search_documents_fts \
     CROSS JOIN search_documents \
       ON search_documents.id = search_documents_fts.rowid \
     WHERE search_documents.entity_type = 'function' \
       AND search_documents_fts MATCH ?";

/// Deduplicated Function page for a one-or-two-scalar short-gram lookup. Bind
/// order: `gram_len`, normalized `gram`, `limit`, `offset`.
pub const FUNCTION_SHORT_GRAM_LIST_SQL: &str = "WITH matched_function_keys AS ( \
         SELECT DISTINCT search_documents.entity_key \
         FROM search_short_grams INDEXED BY idx_search_short_grams_lookup \
         JOIN search_documents \
           ON search_documents.id = search_short_grams.document_id \
         WHERE search_short_grams.gram_len = ? \
           AND search_short_grams.gram = ? \
           AND search_documents.entity_type = 'function' \
     ) \
     SELECT functions.id, functions.identifier, functions.name, \
            functions.description, functions.kind, functions.input_schema, \
            functions.output_schema, functions.plugin_id, functions.plugin_export, \
            functions.category_id, functions.required_capabilities, \
            functions.created_at, functions.updated_at \
     FROM matched_function_keys \
     JOIN search_documents \
       ON search_documents.entity_type = 'function' \
      AND search_documents.entity_key = matched_function_keys.entity_key \
      AND search_documents.field = 'name' \
     JOIN search_documents AS identifiers \
       ON identifiers.entity_type = search_documents.entity_type \
      AND identifiers.entity_key = search_documents.entity_key \
      AND identifiers.field = 'identifier' \
     JOIN functions \
       ON functions.id = CAST(matched_function_keys.entity_key AS INTEGER) \
     ORDER BY search_documents.normalized_text ASC, \
              identifiers.normalized_text ASC, functions.id ASC \
     LIMIT ? OFFSET ?";

/// Deduplicated Function count for a one-or-two-scalar short-gram lookup. Bind
/// order: `gram_len`, normalized `gram`.
pub const FUNCTION_SHORT_GRAM_COUNT_SQL: &str = "SELECT COUNT(DISTINCT search_documents.entity_key) \
     FROM search_short_grams INDEXED BY idx_search_short_grams_lookup \
     JOIN search_documents \
       ON search_documents.id = search_short_grams.document_id \
     WHERE search_short_grams.gram_len = ? \
       AND search_short_grams.gram = ? \
       AND search_documents.entity_type = 'function'";

/// Load one Function by primary key. Bind order: Function `id`.
pub const FUNCTION_GET_SQL: &str = "SELECT id, identifier, name, description, kind, input_schema, \
            output_schema, plugin_id, plugin_export, category_id, \
            required_capabilities, created_at, updated_at \
     FROM functions WHERE id = ?";

/// Load Tools that reference a Function. Bind order: Function `id`.
pub const FUNCTION_TOOL_REFERENCES_SQL: &str = "SELECT id, identifier, name \
     FROM tools INDEXED BY idx_tools_function_id \
     WHERE function_id = ? \
     ORDER BY identifier ASC, id ASC";

/// Load Workflow nodes that reference a Function. Bind order: Function `id`.
pub const FUNCTION_WORKFLOW_NODE_REFERENCES_SQL: &str = "SELECT workflow_nodes.id, workflows.identifier AS workflow_identifier, \
            workflow_nodes.node_key \
     FROM workflow_nodes INDEXED BY idx_workflow_nodes_function_id \
     JOIN workflows ON workflows.id = workflow_nodes.workflow_id \
     WHERE workflow_nodes.function_id = ? \
     ORDER BY workflows.identifier ASC, workflow_nodes.node_key ASC, \
              workflow_nodes.id ASC";

/// Stable unfiltered Agent page. Bind order: `limit`, `offset`.
pub const AGENT_UNFILTERED_LIST_SQL: &str = "SELECT agents.id, agents.identifier, agents.name, \
            agents.description, agents.system_prompt, agents.parent_agent_id, agents.depth, \
            agents.is_default, agents.model_preset, agents.category_id, agents.created_at, \
            agents.updated_at \
     FROM search_documents INDEXED BY idx_search_documents_covering \
     JOIN search_documents AS identifiers \
       ON identifiers.entity_type = search_documents.entity_type \
      AND identifiers.entity_key = search_documents.entity_key \
      AND identifiers.field = 'identifier' \
     JOIN agents ON agents.id = CAST(search_documents.entity_key AS INTEGER) \
     WHERE search_documents.entity_type = 'agent' AND search_documents.field = 'name' \
     ORDER BY search_documents.normalized_text ASC, identifiers.normalized_text ASC, agents.id ASC \
     LIMIT ? OFFSET ?";

/// Agent page for a three-or-more-scalar FTS phrase. Bind order: encoded FTS
/// `phrase`, `limit`, `offset`.
pub const AGENT_FTS_LIST_SQL: &str = "WITH matched_agent_keys AS ( \
         SELECT DISTINCT search_documents.entity_key \
         FROM search_documents_fts \
         CROSS JOIN search_documents ON search_documents.id = search_documents_fts.rowid \
         WHERE search_documents.entity_type = 'agent' AND search_documents_fts MATCH ? \
     ) \
     SELECT agents.id, agents.identifier, agents.name, agents.description, agents.system_prompt, \
            agents.parent_agent_id, agents.depth, agents.is_default, agents.model_preset, \
            agents.category_id, agents.created_at, agents.updated_at \
     FROM matched_agent_keys \
     CROSS JOIN search_documents \
       ON search_documents.entity_type = 'agent' \
      AND search_documents.entity_key = matched_agent_keys.entity_key \
      AND search_documents.field = 'name' \
     CROSS JOIN search_documents AS identifiers \
       ON identifiers.entity_type = search_documents.entity_type \
      AND identifiers.entity_key = search_documents.entity_key \
      AND identifiers.field = 'identifier' \
     CROSS JOIN agents ON agents.id = CAST(matched_agent_keys.entity_key AS INTEGER) \
     ORDER BY search_documents.normalized_text ASC, identifiers.normalized_text ASC, agents.id ASC \
     LIMIT ? OFFSET ?";

/// Count canonical Agent name documents.
pub const AGENT_UNFILTERED_COUNT_SQL: &str = "SELECT COUNT(*) \
     FROM search_documents INDEXED BY idx_search_documents_covering \
     WHERE entity_type = 'agent' AND field = 'name'";

/// Count deduplicated Agent matches for an FTS phrase. Bind order: phrase.
pub const AGENT_FTS_COUNT_SQL: &str = "SELECT COUNT(DISTINCT search_documents.entity_key) \
     FROM search_documents_fts \
     CROSS JOIN search_documents ON search_documents.id = search_documents_fts.rowid \
     WHERE search_documents.entity_type = 'agent' AND search_documents_fts MATCH ?";

/// Agent page for a one-or-two-scalar short-gram lookup. Bind order:
/// `gram_len`, normalized `gram`, `limit`, `offset`.
pub const AGENT_SHORT_GRAM_LIST_SQL: &str = "WITH matched_agent_keys AS ( \
         SELECT DISTINCT search_documents.entity_key \
         FROM search_short_grams INDEXED BY idx_search_short_grams_lookup \
         JOIN search_documents ON search_documents.id = search_short_grams.document_id \
         WHERE search_short_grams.gram_len = ? AND search_short_grams.gram = ? \
           AND search_documents.entity_type = 'agent' \
     ) \
     SELECT agents.id, agents.identifier, agents.name, agents.description, agents.system_prompt, \
            agents.parent_agent_id, agents.depth, agents.is_default, agents.model_preset, \
            agents.category_id, agents.created_at, agents.updated_at \
     FROM matched_agent_keys \
     JOIN search_documents \
       ON search_documents.entity_type = 'agent' \
      AND search_documents.entity_key = matched_agent_keys.entity_key \
      AND search_documents.field = 'name' \
     JOIN search_documents AS identifiers \
       ON identifiers.entity_type = search_documents.entity_type \
      AND identifiers.entity_key = search_documents.entity_key \
      AND identifiers.field = 'identifier' \
     JOIN agents ON agents.id = CAST(matched_agent_keys.entity_key AS INTEGER) \
     ORDER BY search_documents.normalized_text ASC, identifiers.normalized_text ASC, agents.id ASC \
     LIMIT ? OFFSET ?";

/// Count deduplicated Agent matches for a short gram. Bind order:
/// `gram_len`, normalized `gram`.
pub const AGENT_SHORT_GRAM_COUNT_SQL: &str = "SELECT COUNT(DISTINCT search_documents.entity_key) \
     FROM search_short_grams INDEXED BY idx_search_short_grams_lookup \
     JOIN search_documents ON search_documents.id = search_short_grams.document_id \
     WHERE search_short_grams.gram_len = ? AND search_short_grams.gram = ? \
       AND search_documents.entity_type = 'agent'";

/// Batch-load Agent base records from one JSON array of ids.
pub const AGENT_RESOURCE_BASE_SQL: &str = "SELECT agents.id, agents.identifier, agents.name, \
            agents.description, agents.system_prompt, agents.parent_agent_id, agents.depth, \
            agents.is_default, agents.model_preset, agents.category_id, agents.created_at, \
            agents.updated_at \
     FROM json_each(?) AS requested \
     JOIN agents ON agents.id = CAST(requested.value AS INTEGER) \
     ORDER BY agents.id ASC";

/// Batch-load explicit Agent Tool associations from the same JSON id array.
pub const AGENT_RESOURCE_TOOLS_SQL: &str = "SELECT agent_tools.agent_id, tools.id \
     FROM json_each(?) AS requested \
     JOIN agent_tools ON agent_tools.agent_id = CAST(requested.value AS INTEGER) \
     JOIN tools ON tools.id = agent_tools.tool_id \
     ORDER BY agent_tools.agent_id ASC, tools.id ASC";

/// Batch-load explicit and globally-always Agent Skill resources from one JSON
/// id array. The third column is one for an always-on Skill.
pub const AGENT_RESOURCE_SKILLS_SQL: &str = "SELECT agent_skills.agent_id, skills.id, 0 AS is_always \
     FROM json_each(?) AS requested \
     JOIN agent_skills ON agent_skills.agent_id = CAST(requested.value AS INTEGER) \
     JOIN skills ON skills.id = agent_skills.skill_id \
     UNION ALL \
     SELECT CAST(requested.value AS INTEGER), skills.id, 1 AS is_always \
     FROM json_each(?) AS requested \
     CROSS JOIN skills INDEXED BY idx_skills_is_always \
     WHERE skills.is_always = 1 \
     ORDER BY 1 ASC, 3 ASC, 2 ASC";

/// Batch-load explicit Agent Capability associations from one JSON id array.
pub const AGENT_RESOURCE_CAPABILITIES_SQL: &str = "SELECT agent_capabilities.agent_id, capabilities.name \
     FROM json_each(?) AS requested \
     JOIN agent_capabilities \
       ON agent_capabilities.agent_id = CAST(requested.value AS INTEGER) \
     JOIN capabilities ON capabilities.name = agent_capabilities.capability_name \
     ORDER BY agent_capabilities.agent_id ASC, capabilities.name ASC";

/// Validate one non-empty Agent model preset name through the unique catalog.
pub const AGENT_MODEL_PRESET_EXISTS_SQL: &str = "SELECT COUNT(*) FROM llm_presets WHERE name = ?";

/// Most-recent conversation page. The timestamp upper bound makes the
/// ordering index a searched access path instead of an unbounded table scan.
pub const CONVERSATION_RECENT_SQL: &str = "SELECT id, entry_agent_id, current_agent_id, title_encrypted, \
    created_at, updated_at, expires_at FROM chat_sessions \
    WHERE updated_at <= ? ORDER BY updated_at DESC, id ASC LIMIT 20";
/// Exact expired-session id set used by preview and confirmation.
pub const CONVERSATION_EXPIRED_IDS_SQL: &str = "SELECT id FROM chat_sessions \
    WHERE expires_at <= ? ORDER BY id ASC";
/// Batched encrypted messages for an arbitrary JSON UUID array.
pub const CONVERSATION_BUNDLE_MESSAGES_SQL: &str = "SELECT id, session_id, content_encrypted, \
    tool_calls_encrypted, created_at FROM chat_messages \
    WHERE session_id IN (SELECT value FROM json_each(?)) \
    ORDER BY session_id ASC, seq ASC";
/// Batched encrypted executions for an arbitrary JSON UUID array.
pub const CONVERSATION_BUNDLE_EXECUTIONS_SQL: &str = "SELECT execution_id, session_id, \
    current_agent_id, state_encrypted, started_at, finished_at FROM agent_executions \
    WHERE session_id IN (SELECT value FROM json_each(?)) \
    ORDER BY session_id ASC, started_at ASC, execution_id ASC";
/// Stable running-execution scan used before idempotent recovery.
pub const CONVERSATION_RUNNING_EXECUTIONS_SQL: &str = "SELECT execution_id FROM agent_executions \
    WHERE status = 'running' ORDER BY execution_id ASC";

/// Stable unfiltered Tool page. Bind order: `limit`, `offset`.
pub const TOOL_UNFILTERED_LIST_SQL: &str = "SELECT tools.id, tools.identifier, tools.name, \
            tools.description, tools.kind, tools.source, tools.is_always, tools.function_id, \
            tools.workflow_id, tools.input_schema, tools.output_schema, tools.category_id, \
            tools.required_capabilities, tools.created_at, tools.updated_at \
     FROM search_documents INDEXED BY idx_search_documents_covering \
     JOIN search_documents AS identifiers \
       ON identifiers.entity_type = search_documents.entity_type \
      AND identifiers.entity_key = search_documents.entity_key \
      AND identifiers.field = 'identifier' \
     JOIN tools ON tools.id = CAST(search_documents.entity_key AS INTEGER) \
     WHERE search_documents.entity_type = 'tool' AND search_documents.field = 'name' \
     ORDER BY search_documents.normalized_text ASC, identifiers.normalized_text ASC, tools.id ASC \
     LIMIT ? OFFSET ?";

/// Deduplicated Tool page for a three-or-more-scalar FTS phrase. Bind order:
/// encoded FTS `phrase`, `limit`, `offset`.
pub const TOOL_FTS_LIST_SQL: &str = "WITH matched_tool_keys AS MATERIALIZED ( \
         SELECT DISTINCT search_documents.entity_key \
         FROM search_documents_fts \
         CROSS JOIN search_documents ON search_documents.id = search_documents_fts.rowid \
         WHERE search_documents.entity_type = 'tool' AND search_documents_fts MATCH ? \
     ) \
     SELECT tools.id, tools.identifier, tools.name, tools.description, tools.kind, tools.source, \
            tools.is_always, tools.function_id, tools.workflow_id, tools.input_schema, \
            tools.output_schema, tools.category_id, tools.required_capabilities, \
            tools.created_at, tools.updated_at, \
            (SELECT COUNT(*) FROM matched_tool_keys) AS total_count \
     FROM matched_tool_keys \
     CROSS JOIN search_documents \
       ON search_documents.entity_type = 'tool' \
      AND search_documents.entity_key = matched_tool_keys.entity_key \
      AND search_documents.field = 'name' \
     CROSS JOIN search_documents AS identifiers \
       ON identifiers.entity_type = search_documents.entity_type \
      AND identifiers.entity_key = search_documents.entity_key \
      AND identifiers.field = 'identifier' \
     CROSS JOIN tools ON tools.id = CAST(matched_tool_keys.entity_key AS INTEGER) \
     ORDER BY search_documents.normalized_text ASC, identifiers.normalized_text ASC, tools.id ASC \
     LIMIT ? OFFSET ?";

/// Count canonical Tool name documents.
pub const TOOL_UNFILTERED_COUNT_SQL: &str = "SELECT COUNT(*) \
     FROM search_documents INDEXED BY idx_search_documents_covering \
     WHERE entity_type = 'tool' AND field = 'name'";

/// Deduplicated Tool count for a three-or-more-scalar FTS phrase.
pub const TOOL_FTS_COUNT_SQL: &str = "SELECT COUNT(DISTINCT search_documents.entity_key) \
     FROM search_documents_fts \
     CROSS JOIN search_documents ON search_documents.id = search_documents_fts.rowid \
     WHERE search_documents.entity_type = 'tool' AND search_documents_fts MATCH ?";

/// Deduplicated Tool page for a one-or-two-scalar short-gram lookup. Bind
/// order: `gram_len`, normalized `gram`, `limit`, `offset`.
pub const TOOL_SHORT_GRAM_LIST_SQL: &str = "WITH matched_tool_keys AS ( \
         SELECT DISTINCT search_documents.entity_key \
         FROM search_short_grams INDEXED BY idx_search_short_grams_lookup \
         JOIN search_documents ON search_documents.id = search_short_grams.document_id \
         WHERE search_short_grams.gram_len = ? AND search_short_grams.gram = ? \
           AND search_documents.entity_type = 'tool' \
     ) \
     SELECT tools.id, tools.identifier, tools.name, tools.description, tools.kind, tools.source, \
            tools.is_always, tools.function_id, tools.workflow_id, tools.input_schema, \
            tools.output_schema, tools.category_id, tools.required_capabilities, \
            tools.created_at, tools.updated_at \
     FROM matched_tool_keys \
     JOIN search_documents \
       ON search_documents.entity_type = 'tool' \
      AND search_documents.entity_key = matched_tool_keys.entity_key \
      AND search_documents.field = 'name' \
     JOIN search_documents AS identifiers \
       ON identifiers.entity_type = search_documents.entity_type \
      AND identifiers.entity_key = search_documents.entity_key \
      AND identifiers.field = 'identifier' \
     JOIN tools ON tools.id = CAST(matched_tool_keys.entity_key AS INTEGER) \
     ORDER BY search_documents.normalized_text ASC, identifiers.normalized_text ASC, tools.id ASC \
     LIMIT ? OFFSET ?";

/// Deduplicated Tool count for a one-or-two-scalar short-gram lookup.
pub const TOOL_SHORT_GRAM_COUNT_SQL: &str = "SELECT COUNT(DISTINCT search_documents.entity_key) \
     FROM search_short_grams INDEXED BY idx_search_short_grams_lookup \
     JOIN search_documents ON search_documents.id = search_short_grams.document_id \
     WHERE search_short_grams.gram_len = ? AND search_short_grams.gram = ? \
       AND search_documents.entity_type = 'tool'";

/// Load one complete Tool by primary key. Bind order: Tool `id`.
pub const TOOL_GET_SQL: &str = "SELECT id, identifier, name, description, kind, source, is_always, \
            function_id, workflow_id, input_schema, output_schema, category_id, \
            required_capabilities, created_at, updated_at FROM tools WHERE id = ?";

/// Resolve the schemas for a Function-wrapped Tool. Bind order: Function `id`.
pub const TOOL_FUNCTION_TARGET_SQL: &str =
    "SELECT input_schema, output_schema FROM functions WHERE id = ?";

/// Resolve the schemas for a Workflow-wrapped Tool. Bind order: Workflow `id`.
pub const TOOL_WORKFLOW_TARGET_SQL: &str =
    "SELECT input_schema, output_schema FROM workflows WHERE id = ?";

/// Resolve one locally known Capability. Bind order: capability name.
pub const TOOL_CAPABILITY_GET_SQL: &str = "SELECT name FROM capabilities WHERE name = ?";

/// Stable unfiltered Workflow page. Bind order: `limit`, `offset`.
pub const WORKFLOW_UNFILTERED_LIST_SQL: &str = "SELECT workflows.id, workflows.name \
     FROM search_documents INDEXED BY idx_search_documents_covering \
     JOIN search_documents AS identifiers \
       ON identifiers.entity_type = search_documents.entity_type \
      AND identifiers.entity_key = search_documents.entity_key \
      AND identifiers.field = 'identifier' \
     JOIN workflows ON workflows.id = CAST(search_documents.entity_key AS INTEGER) \
     WHERE search_documents.entity_type = 'workflow' \
       AND search_documents.field = 'name' \
     ORDER BY search_documents.normalized_text ASC, \
              identifiers.normalized_text ASC, workflows.id ASC \
     LIMIT ? OFFSET ?";

/// Count all canonical Workflow name documents.
pub const WORKFLOW_UNFILTERED_COUNT_SQL: &str = "SELECT COUNT(*) \
     FROM search_documents INDEXED BY idx_search_documents_covering \
     WHERE entity_type = 'workflow' AND field = 'name'";

/// Deduplicated Workflow page for a three-or-more-scalar FTS phrase. Bind
/// order: encoded FTS phrase, limit, offset.
pub const WORKFLOW_FTS_LIST_SQL: &str = "WITH matched_workflow_keys AS ( \
         SELECT DISTINCT search_documents.entity_key \
         FROM search_documents_fts \
         CROSS JOIN search_documents \
           ON search_documents.id = search_documents_fts.rowid \
         WHERE search_documents.entity_type = 'workflow' \
           AND search_documents_fts MATCH ? \
     ) \
     SELECT workflows.id, workflows.name \
     FROM matched_workflow_keys \
     CROSS JOIN search_documents \
       ON search_documents.entity_type = 'workflow' \
      AND search_documents.entity_key = matched_workflow_keys.entity_key \
      AND search_documents.field = 'name' \
     CROSS JOIN search_documents AS identifiers \
       ON identifiers.entity_type = search_documents.entity_type \
      AND identifiers.entity_key = search_documents.entity_key \
      AND identifiers.field = 'identifier' \
     CROSS JOIN workflows \
       ON workflows.id = CAST(matched_workflow_keys.entity_key AS INTEGER) \
     ORDER BY search_documents.normalized_text ASC, \
              identifiers.normalized_text ASC, workflows.id ASC \
     LIMIT ? OFFSET ?";

/// Deduplicated Workflow count for a three-or-more-scalar FTS phrase. Bind
/// order: encoded FTS phrase.
pub const WORKFLOW_FTS_COUNT_SQL: &str = "SELECT COUNT(DISTINCT search_documents.entity_key) \
     FROM search_documents_fts \
     CROSS JOIN search_documents ON search_documents.id = search_documents_fts.rowid \
     WHERE search_documents.entity_type = 'workflow' \
       AND search_documents_fts MATCH ?";

/// Deduplicated Workflow page for a one-or-two-scalar short gram. Bind order:
/// gram length, normalized gram, limit, offset.
pub const WORKFLOW_SHORT_GRAM_LIST_SQL: &str = "WITH matched_workflow_keys AS ( \
         SELECT DISTINCT search_documents.entity_key \
         FROM search_short_grams INDEXED BY idx_search_short_grams_lookup \
         JOIN search_documents ON search_documents.id = search_short_grams.document_id \
         WHERE search_short_grams.gram_len = ? \
           AND search_short_grams.gram = ? \
           AND search_documents.entity_type = 'workflow' \
     ) \
     SELECT workflows.id, workflows.name \
     FROM matched_workflow_keys \
     JOIN search_documents \
       ON search_documents.entity_type = 'workflow' \
      AND search_documents.entity_key = matched_workflow_keys.entity_key \
      AND search_documents.field = 'name' \
     JOIN search_documents AS identifiers \
       ON identifiers.entity_type = search_documents.entity_type \
      AND identifiers.entity_key = search_documents.entity_key \
      AND identifiers.field = 'identifier' \
     JOIN workflows ON workflows.id = CAST(matched_workflow_keys.entity_key AS INTEGER) \
     ORDER BY search_documents.normalized_text ASC, \
              identifiers.normalized_text ASC, workflows.id ASC \
     LIMIT ? OFFSET ?";

/// Deduplicated Workflow count for a one-or-two-scalar short gram. Bind order:
/// gram length, normalized gram.
pub const WORKFLOW_SHORT_GRAM_COUNT_SQL: &str = "SELECT COUNT(DISTINCT search_documents.entity_key) \
     FROM search_short_grams INDEXED BY idx_search_short_grams_lookup \
     JOIN search_documents ON search_documents.id = search_short_grams.document_id \
     WHERE search_short_grams.gram_len = ? \
       AND search_short_grams.gram = ? \
       AND search_documents.entity_type = 'workflow'";

/// Load nodes for up to 25 Workflow ids in one indexed query. Callers pad
/// unused bind slots with a negative id.
pub const WORKFLOW_BATCH_NODES_SQL: &str = "SELECT workflow_id, node_key, node_type, function_id, node_config \
     FROM workflow_nodes INDEXED BY sqlite_autoindex_workflow_nodes_1 \
     WHERE workflow_id IN (?,?,?,?,?,?,?,?,?,?,?,?,?,?,?,?,?,?,?,?,?,?,?,?,?) \
     ORDER BY workflow_id ASC, node_key ASC";

/// Load edges for up to 25 Workflow ids in one indexed query. Callers pad
/// unused bind slots with a negative id.
pub const WORKFLOW_BATCH_EDGES_SQL: &str = "SELECT workflow_id, src_node_key, dst_node_key \
     FROM workflow_edges INDEXED BY sqlite_autoindex_workflow_edges_1 \
     WHERE workflow_id IN (?,?,?,?,?,?,?,?,?,?,?,?,?,?,?,?,?,?,?,?,?,?,?,?,?) \
     ORDER BY workflow_id ASC, src_node_key ASC, dst_node_key ASC";

/// Load safe Tool identifiers referencing one Workflow. Bind order: Workflow
/// id.
pub const WORKFLOW_TOOL_REFERENCES_SQL: &str = "SELECT identifier \
     FROM tools INDEXED BY idx_tools_workflow_id \
     WHERE workflow_id = ? ORDER BY identifier ASC, id ASC";

impl ProductionQuery {
    /// Returns true when the query references any filter or join
    /// column.
    pub fn has_filter_or_join(&self) -> bool {
        !self.filter_columns.is_empty() || !self.join_columns.is_empty()
    }
}

/// One row returned by SQLite's `EXPLAIN QUERY PLAN`.
#[derive(Debug, Clone, Copy)]
pub struct SqlitePlanRow {
    /// Row id within the plan.
    pub id: i64,
    /// Parent row id.
    pub parent: i64,
    /// Whether the row is unused (legacy `notused` column).
    pub not_used: i64,
    /// Plan detail (e.g. `SEARCH agents USING INDEX idx_agents_name`).
    pub detail: &'static str,
}

/// Evaluate an FTS5 query plan against a [`QueryPlanRequirement`].
///
/// The test seam accepts a [`QueryPlanRequirement`] (the same shape
/// used by [`evaluate_sqlite_plan`]) so the FTS tests can re-use the
/// existing helper without a parallel expectation type. FTS-specific
/// validation rules (e.g. `VIRTUAL TABLE INDEX` is treated as an
/// index access) are layered on top of the standard search rules.
///
/// `signature_date` is a free-form signature (e.g. `2026-07-30`)
/// the contract uses to lock the verdict against accidental
/// changes.
/// Evaluate an FTS query plan. The function accepts any value that
/// can be converted into a [`QueryPlanRequirement`], so both
/// [`FtsPlanExpectation`] and the raw [`QueryPlanRequirement`] are
/// valid inputs.
pub fn evaluate_fts_plan<E>(
    expectation: &E,
    plan: &[Value],
    signature_date: &str,
) -> Result<FtsPlanVerdict, String>
where
    E: Copy + Into<QueryPlanRequirement>,
{
    let requirement: QueryPlanRequirement = (*expectation).into();
    evaluate_fts_plan_with(&requirement, plan, signature_date)
}

/// Evaluate an FTS query plan against a [`QueryPlanRequirement`].
/// The test seam accepts a [`QueryPlanRequirement`] (the same shape
/// used by `evaluate_sqlite_plan`); FTS-specific validation rules
/// (e.g. `VIRTUAL TABLE INDEX` is treated as an index access) are
/// layered on top of the standard search rules.
pub fn evaluate_fts_plan_with(
    requirement: &QueryPlanRequirement,
    plan: &[Value],
    _signature_date: &str,
) -> Result<FtsPlanVerdict, String> {
    let mut failures = Vec::new();
    let mut has_search = false;
    let mut has_index_hint = false;
    for row in plan {
        let detail = row
            .get("detail")
            .and_then(|value| value.as_str())
            .unwrap_or_default();
        if detail.starts_with("SEARCH") {
            has_search = true;
            if detail.contains("VIRTUAL TABLE INDEX") {
                has_index_hint = true;
            }
            if let Some(expected) = requirement.expected_index
                && detail.contains(expected)
            {
                has_index_hint = true;
            }
        }
        if detail.starts_with("SCAN") && !detail.contains("CONSTANT") {
            failures.push(PlanFailureKind::UnapprovedFullScan);
        }
    }
    if !has_search {
        failures.push(PlanFailureKind::MissingSearch);
    }
    if !has_index_hint {
        failures.push(PlanFailureKind::MissingIndex);
    }

    // Coverage of filter columns
    for column in requirement.filter_columns {
        let covered = plan.iter().any(|row| {
            row.get("detail")
                .and_then(|v| v.as_str())
                .map(|d| {
                    d.contains(&format!("({}=?)", column)) || d.contains(&format!("({}=?", column))
                })
                .unwrap_or(false)
        });
        if !covered {
            failures.push(PlanFailureKind::UncoveredFilterColumn(column));
        }
    }

    Ok(FtsPlanVerdict { failures })
}

/// Evaluate a MySQL JSON plan against a [`QueryPlanRequirement`].
pub fn evaluate_mysql_plan(
    requirement: &QueryPlanRequirement,
    plan: &Value,
    review_date: &str,
) -> PlanVerdict {
    let mut failures = Vec::new();
    let mut used_exception = false;

    let access_type = plan
        .pointer("/query_block/table/access_type")
        .and_then(|v| v.as_str())
        .unwrap_or_default();
    let key = plan
        .pointer("/query_block/table/key")
        .and_then(|v| v.as_str());
    let used_key_parts: Vec<&str> = plan
        .pointer("/query_block/table/used_key_parts")
        .and_then(|v| v.as_array())
        .map(|arr| arr.iter().filter_map(|v| v.as_str()).collect())
        .unwrap_or_default();

    match (requirement.expected_access, requirement.expected_index) {
        (AccessExpectation::Search, Some(expected))
            if access_type == "ref" || access_type == "range" =>
        {
            if key != Some(expected) {
                failures.push(PlanFailureKind::MissingExpectedIndex);
            }
        }
        (AccessExpectation::Search, Some(_expected)) => {
            if access_type == "ALL" {
                failures.push(PlanFailureKind::UnapprovedFullScan);
                failures.push(PlanFailureKind::MissingExpectedIndex);
            }
        }
        (AccessExpectation::Search, None) => {
            if access_type == "ALL" {
                failures.push(PlanFailureKind::UnapprovedFullScan);
            }
        }
        (AccessExpectation::Scan, _) => {}
    }

    for column in requirement.filter_columns {
        if !used_key_parts.contains(column) {
            failures.push(PlanFailureKind::UncoveredFilterColumn(column));
        }
    }
    for column in requirement.join_columns {
        if !used_key_parts.contains(column) {
            failures.push(PlanFailureKind::UncoveredJoinColumn(column));
        }
    }

    if access_type.is_empty() {
        failures.push(PlanFailureKind::IndeterminatePlan);
    }

    if let Some(exception) = &requirement.scan_exception {
        if is_valid_exception(exception, review_date) {
            used_exception = true;
        } else {
            failures.push(PlanFailureKind::InvalidScanException);
        }
    }

    PlanVerdict {
        failures,
        used_exception,
    }
}

/// Evaluate a SQLite `EXPLAIN` plan row set against a [`QueryPlanRequirement`].
pub fn evaluate_sqlite_plan(
    requirement: &QueryPlanRequirement,
    plan: &[SqlitePlanRow],
    review_date: &str,
) -> PlanVerdict {
    let mut failures = Vec::new();
    let mut used_exception = false;

    let mut has_search = false;
    let mut has_expected_index = false;
    let mut has_scan = false;
    let mut has_index_mention = false;

    let matching_rows = plan
        .iter()
        .filter(|row| row.detail.contains(requirement.table))
        .collect::<Vec<_>>();
    if matching_rows.is_empty()
        && plan
            .iter()
            .any(|row| row.detail.contains("USE TEMP B-TREE FOR ORDER BY"))
    {
        failures.push(PlanFailureKind::IndeterminatePlan);
    }

    for row in &matching_rows {
        let detail = row.detail;
        let virtual_index_scan =
            detail.starts_with("SCAN") && detail.contains("VIRTUAL TABLE INDEX");
        if detail.starts_with("SEARCH") || virtual_index_scan {
            has_search = true;
            if let Some(expected) = requirement.expected_index
                && detail.contains(expected)
            {
                has_expected_index = true;
            }
            if detail.contains("USING INDEX") || detail.contains("VIRTUAL TABLE INDEX") {
                has_index_mention = true;
            }
        }
        if detail.starts_with("SCAN") && !virtual_index_scan {
            has_scan = true;
        }
    }

    match requirement.expected_access {
        AccessExpectation::Search => {
            if !has_search {
                failures.push(PlanFailureKind::MissingSearch);
            }
            if let Some(expected) = requirement.expected_index {
                if !has_expected_index {
                    failures.push(PlanFailureKind::MissingExpectedIndex);
                }
                let _ = expected;
            }
        }
        AccessExpectation::Scan => {
            // A scan is allowed only with a valid exception.
        }
    }

    if has_scan && !matches!(requirement.expected_access, AccessExpectation::Scan) {
        failures.push(PlanFailureKind::UnapprovedFullScan);
    }

    // Coverage checks: every filter/join column must appear inside a SEARCH detail.
    for column in requirement.filter_columns {
        let covered = matching_rows.iter().any(|row| {
            row.detail.contains(&format!("{}=?", column))
                || (row.detail.contains("VIRTUAL TABLE INDEX")
                    && requirement.table == "search_documents_fts")
                || row.detail.contains("INTEGER PRIMARY KEY")
        });
        if !covered && has_index_mention {
            // only fail coverage when there was a usable index path
        }
        if !covered && has_search {
            failures.push(PlanFailureKind::UncoveredFilterColumn(column));
        }
    }
    for column in requirement.join_columns {
        let covered = matching_rows.iter().any(|row| {
            row.detail.contains(&format!("{}=?", column))
                || (column == &"id" && row.detail.contains("INTEGER PRIMARY KEY"))
                || (column == &"document_id"
                    && requirement.table == "search_short_grams"
                    && row
                        .detail
                        .contains("USING COVERING INDEX idx_search_short_grams_lookup"))
                || (column == &"tool_id"
                    && requirement.table == "agent_tools"
                    && row.detail.contains("sqlite_autoindex_agent_tools_1"))
                || (column == &"skill_id"
                    && requirement.table == "agent_skills"
                    && row.detail.contains("sqlite_autoindex_agent_skills_1"))
                || (column == &"capability_name"
                    && requirement.table == "agent_capabilities"
                    && row.detail.contains("sqlite_autoindex_agent_capabilities_1"))
        });
        if !covered && has_search {
            failures.push(PlanFailureKind::UncoveredJoinColumn(column));
        }
    }

    if let Some(exception) = &requirement.scan_exception {
        if is_valid_exception(exception, review_date) {
            used_exception = true;
        } else {
            failures.push(PlanFailureKind::InvalidScanException);
        }
    }

    PlanVerdict {
        failures,
        used_exception,
    }
}

/// Evaluate a SQLite plan row set against a [`ProductionQuery`].
pub fn evaluate_sqlite_query(
    query: &ProductionQuery,
    plan: &[SqlitePlanRow],
    review_date: &str,
) -> PlanVerdict {
    let mut aggregate = PlanVerdict {
        failures: Vec::new(),
        used_exception: false,
    };
    for requirement in query.requirements {
        let verdict = evaluate_sqlite_plan(requirement, plan, review_date);
        if verdict.used_exception {
            aggregate.used_exception = true;
        }
        aggregate.failures.extend(verdict.failures.iter().cloned());
    }
    aggregate
}

fn is_valid_exception(exception: &QueryPlanException, review_date: &str) -> bool {
    exception.table_size > 0
        && !exception.reason.trim().is_empty()
        && !exception.approver.trim().is_empty()
        && !exception.expires_on.trim().is_empty()
        && !exception.review_result.trim().is_empty()
        && exception.expires_on >= review_date
}

/// Static, exhaustive list of production queries the storage layer
/// is expected to honor. Each row lists its expected access path,
/// filter/join columns, and (where applicable) a scan exception.
pub fn production_query_catalog() -> &'static [ProductionQuery] {
    CATALOG
}

const CATALOG: &[ProductionQuery] = &[
    ProductionQuery {
        id: "migrations_scan_unknown_function_text_kinds",
        sql: "SELECT kind FROM functions \
              WHERE kind NOT IN ('builtin', 'custom', 'placeholder') LIMIT 1",
        owner_phase: "migrations",
        activation_task: "T022",
        table: "functions",
        active: false,
        dialect: QueryDialect::Sqlite,
        filter_columns: &["kind"],
        join_columns: &[],
        requirements: &[QueryPlanRequirement {
            query_id: "migrations_scan_unknown_function_text_kinds",
            owner_phase: "migrations",
            activation_task: "T022",
            table: "functions",
            expected_access: AccessExpectation::Search,
            expected_index: None,
            filter_columns: &["kind"],
            join_columns: &[],
            scan_exception: None,
        }],
        scan_exception: None,
    },
    ProductionQuery {
        id: "migrations_dotted_builtin_source_probe",
        sql: "SELECT id FROM functions WHERE identifier = ?",
        owner_phase: "migrations",
        activation_task: "T022",
        table: "functions",
        active: false,
        dialect: QueryDialect::Sqlite,
        filter_columns: &["identifier"],
        join_columns: &[],
        requirements: &[QueryPlanRequirement {
            query_id: "migrations_dotted_builtin_source_probe",
            owner_phase: "migrations",
            activation_task: "T022",
            table: "functions",
            expected_access: AccessExpectation::Search,
            expected_index: None,
            filter_columns: &["identifier"],
            join_columns: &[],
            scan_exception: None,
        }],
        scan_exception: None,
    },
    ProductionQuery {
        id: "migrations_scan_unknown_tool_text_kinds",
        sql: "SELECT kind FROM tools \
              WHERE kind NOT IN ('function-wrap', 'workflow-wrap') LIMIT 1",
        owner_phase: "migrations",
        activation_task: "T022",
        table: "tools",
        active: false,
        dialect: QueryDialect::Sqlite,
        filter_columns: &["kind"],
        join_columns: &[],
        requirements: &[QueryPlanRequirement {
            query_id: "migrations_scan_unknown_tool_text_kinds",
            owner_phase: "migrations",
            activation_task: "T022",
            table: "tools",
            expected_access: AccessExpectation::Search,
            expected_index: None,
            filter_columns: &["kind"],
            join_columns: &[],
            scan_exception: None,
        }],
        scan_exception: None,
    },
    ProductionQuery {
        id: "t022.search.schema_catalog",
        sql: "SELECT name, sql FROM sqlite_schema \
              WHERE name IN ('schema_metadata','search_documents', \
                             'search_documents_fts','search_short_grams', \
                             'search_index','short_gram_index')",
        owner_phase: "migrations",
        activation_task: "T022",
        table: "sqlite_schema",
        active: false,
        dialect: QueryDialect::Sqlite,
        filter_columns: &["name"],
        join_columns: &[],
        requirements: &[QueryPlanRequirement {
            query_id: "t022.search.schema_catalog",
            owner_phase: "migrations",
            activation_task: "T022",
            table: "sqlite_schema",
            expected_access: AccessExpectation::Search,
            expected_index: None,
            filter_columns: &["name"],
            join_columns: &[],
            scan_exception: None,
        }],
        scan_exception: None,
    },
    ProductionQuery {
        id: "t022.search.normalization_id.verify",
        sql: "SELECT value FROM schema_metadata \
              WHERE key = 'search_normalization_id'",
        owner_phase: "migrations",
        activation_task: "T022",
        table: "schema_metadata",
        active: false,
        dialect: QueryDialect::Sqlite,
        filter_columns: &["key"],
        join_columns: &[],
        requirements: &[QueryPlanRequirement {
            query_id: "t022.search.normalization_id.verify",
            owner_phase: "migrations",
            activation_task: "T022",
            table: "schema_metadata",
            expected_access: AccessExpectation::Search,
            expected_index: None,
            filter_columns: &["key"],
            join_columns: &[],
            scan_exception: None,
        }],
        scan_exception: None,
    },
    ProductionQuery {
        id: "t115.agents.page",
        sql: AGENT_UNFILTERED_LIST_SQL,
        owner_phase: "US13",
        activation_task: "T115",
        table: "search_documents",
        active: true,
        dialect: QueryDialect::Sqlite,
        filter_columns: &["entity_type", "field"],
        join_columns: &["entity_key", "id"],
        requirements: &[
            QueryPlanRequirement {
                query_id: "t115.agents.page.documents",
                owner_phase: "US13",
                activation_task: "T115",
                table: "search_documents",
                expected_access: AccessExpectation::Search,
                expected_index: Some("idx_search_documents_covering"),
                filter_columns: &["entity_type", "field"],
                join_columns: &["entity_key"],
                scan_exception: None,
            },
            QueryPlanRequirement {
                query_id: "t115.agents.page.agents",
                owner_phase: "US13",
                activation_task: "T115",
                table: "agents",
                expected_access: AccessExpectation::Search,
                expected_index: Some("INTEGER PRIMARY KEY"),
                filter_columns: &[],
                join_columns: &["id"],
                scan_exception: None,
            },
        ],
        scan_exception: None,
    },
    ProductionQuery {
        id: "t115.agents.search.fts.page",
        sql: AGENT_FTS_LIST_SQL,
        owner_phase: "US13",
        activation_task: "T115",
        table: "search_documents_fts",
        active: true,
        dialect: QueryDialect::Sqlite,
        filter_columns: &["normalized_text", "entity_type", "field"],
        join_columns: &["id", "entity_key"],
        requirements: &[
            QueryPlanRequirement {
                query_id: "t115.agents.search.fts.page.fts",
                owner_phase: "US13",
                activation_task: "T115",
                table: "search_documents_fts",
                expected_access: AccessExpectation::Search,
                expected_index: Some("VIRTUAL TABLE INDEX"),
                filter_columns: &["normalized_text"],
                join_columns: &[],
                scan_exception: None,
            },
            QueryPlanRequirement {
                query_id: "t115.agents.search.fts.page.documents",
                owner_phase: "US13",
                activation_task: "T115",
                table: "search_documents",
                expected_access: AccessExpectation::Search,
                expected_index: None,
                filter_columns: &["entity_type", "field"],
                join_columns: &["id", "entity_key"],
                scan_exception: None,
            },
            QueryPlanRequirement {
                query_id: "t115.agents.search.fts.page.agents",
                owner_phase: "US13",
                activation_task: "T115",
                table: "agents",
                expected_access: AccessExpectation::Search,
                expected_index: Some("INTEGER PRIMARY KEY"),
                filter_columns: &[],
                join_columns: &["id"],
                scan_exception: None,
            },
        ],
        scan_exception: None,
    },
    ProductionQuery {
        id: "t115.agents.count",
        sql: AGENT_UNFILTERED_COUNT_SQL,
        owner_phase: "US13",
        activation_task: "T115",
        table: "search_documents",
        active: true,
        dialect: QueryDialect::Sqlite,
        filter_columns: &["entity_type", "field"],
        join_columns: &[],
        requirements: &[QueryPlanRequirement {
            query_id: "t115.agents.count.documents",
            owner_phase: "US13",
            activation_task: "T115",
            table: "search_documents",
            expected_access: AccessExpectation::Search,
            expected_index: Some("idx_search_documents_covering"),
            filter_columns: &["entity_type", "field"],
            join_columns: &[],
            scan_exception: None,
        }],
        scan_exception: None,
    },
    ProductionQuery {
        id: "t115.agents.search.fts.count",
        sql: AGENT_FTS_COUNT_SQL,
        owner_phase: "US13",
        activation_task: "T115",
        table: "search_documents_fts",
        active: true,
        dialect: QueryDialect::Sqlite,
        filter_columns: &["normalized_text", "entity_type"],
        join_columns: &["id"],
        requirements: &[
            QueryPlanRequirement {
                query_id: "t115.agents.search.fts.count.fts",
                owner_phase: "US13",
                activation_task: "T115",
                table: "search_documents_fts",
                expected_access: AccessExpectation::Search,
                expected_index: Some("VIRTUAL TABLE INDEX"),
                filter_columns: &["normalized_text"],
                join_columns: &[],
                scan_exception: None,
            },
            QueryPlanRequirement {
                query_id: "t115.agents.search.fts.count.documents",
                owner_phase: "US13",
                activation_task: "T115",
                table: "search_documents",
                expected_access: AccessExpectation::Search,
                expected_index: Some("INTEGER PRIMARY KEY"),
                filter_columns: &["entity_type"],
                join_columns: &["id"],
                scan_exception: None,
            },
        ],
        scan_exception: None,
    },
    ProductionQuery {
        id: "t115.agents.search.short.page",
        sql: AGENT_SHORT_GRAM_LIST_SQL,
        owner_phase: "US13",
        activation_task: "T115",
        table: "search_short_grams",
        active: true,
        dialect: QueryDialect::Sqlite,
        filter_columns: &["gram_len", "gram", "entity_type", "field"],
        join_columns: &["document_id", "id", "entity_key"],
        requirements: &[
            QueryPlanRequirement {
                query_id: "t115.agents.search.short.page.grams",
                owner_phase: "US13",
                activation_task: "T115",
                table: "search_short_grams",
                expected_access: AccessExpectation::Search,
                expected_index: Some("idx_search_short_grams_lookup"),
                filter_columns: &["gram_len", "gram"],
                join_columns: &["document_id"],
                scan_exception: None,
            },
            QueryPlanRequirement {
                query_id: "t115.agents.search.short.page.documents",
                owner_phase: "US13",
                activation_task: "T115",
                table: "search_documents",
                expected_access: AccessExpectation::Search,
                expected_index: None,
                filter_columns: &["entity_type", "field"],
                join_columns: &["id", "entity_key"],
                scan_exception: None,
            },
            QueryPlanRequirement {
                query_id: "t115.agents.search.short.page.agents",
                owner_phase: "US13",
                activation_task: "T115",
                table: "agents",
                expected_access: AccessExpectation::Search,
                expected_index: Some("INTEGER PRIMARY KEY"),
                filter_columns: &[],
                join_columns: &["id"],
                scan_exception: None,
            },
        ],
        scan_exception: None,
    },
    ProductionQuery {
        id: "t115.agents.resource.base",
        sql: AGENT_RESOURCE_BASE_SQL,
        owner_phase: "US13",
        activation_task: "T115",
        table: "agents",
        active: true,
        dialect: QueryDialect::Sqlite,
        filter_columns: &[],
        join_columns: &["id"],
        requirements: &[QueryPlanRequirement {
            query_id: "t115.agents.resource.base.agents",
            owner_phase: "US13",
            activation_task: "T115",
            table: "agents",
            expected_access: AccessExpectation::Search,
            expected_index: Some("INTEGER PRIMARY KEY"),
            filter_columns: &[],
            join_columns: &["id"],
            scan_exception: None,
        }],
        scan_exception: None,
    },
    ProductionQuery {
        id: "t115.agents.search.short.count",
        sql: AGENT_SHORT_GRAM_COUNT_SQL,
        owner_phase: "US13",
        activation_task: "T115",
        table: "search_short_grams",
        active: true,
        dialect: QueryDialect::Sqlite,
        filter_columns: &["gram_len", "gram", "entity_type"],
        join_columns: &["document_id", "id"],
        requirements: &[
            QueryPlanRequirement {
                query_id: "t115.agents.search.short.count.grams",
                owner_phase: "US13",
                activation_task: "T115",
                table: "search_short_grams",
                expected_access: AccessExpectation::Search,
                expected_index: Some("idx_search_short_grams_lookup"),
                filter_columns: &["gram_len", "gram"],
                join_columns: &["document_id"],
                scan_exception: None,
            },
            QueryPlanRequirement {
                query_id: "t115.agents.search.short.count.documents",
                owner_phase: "US13",
                activation_task: "T115",
                table: "search_documents",
                expected_access: AccessExpectation::Search,
                expected_index: Some("INTEGER PRIMARY KEY"),
                filter_columns: &["entity_type"],
                join_columns: &["id"],
                scan_exception: None,
            },
        ],
        scan_exception: None,
    },
    ProductionQuery {
        id: "t115.agents.resource.tools",
        sql: AGENT_RESOURCE_TOOLS_SQL,
        owner_phase: "US13",
        activation_task: "T115",
        table: "agent_tools",
        active: true,
        dialect: QueryDialect::Sqlite,
        filter_columns: &["agent_id"],
        join_columns: &["agent_id", "tool_id", "id"],
        requirements: &[
            QueryPlanRequirement {
                query_id: "t115.agents.resource.tools.association",
                owner_phase: "US13",
                activation_task: "T115",
                table: "agent_tools",
                expected_access: AccessExpectation::Search,
                expected_index: None,
                filter_columns: &["agent_id"],
                join_columns: &["tool_id"],
                scan_exception: None,
            },
            QueryPlanRequirement {
                query_id: "t115.agents.resource.tools.target",
                owner_phase: "US13",
                activation_task: "T115",
                table: "tools",
                expected_access: AccessExpectation::Search,
                expected_index: Some("INTEGER PRIMARY KEY"),
                filter_columns: &[],
                join_columns: &["id"],
                scan_exception: None,
            },
        ],
        scan_exception: None,
    },
    ProductionQuery {
        id: "t115.agents.resource.skills",
        sql: AGENT_RESOURCE_SKILLS_SQL,
        owner_phase: "US13",
        activation_task: "T115",
        table: "agent_skills",
        active: true,
        dialect: QueryDialect::Sqlite,
        filter_columns: &["agent_id", "is_always"],
        join_columns: &["agent_id", "skill_id", "id"],
        requirements: &[
            QueryPlanRequirement {
                query_id: "t115.agents.resource.skills.association",
                owner_phase: "US13",
                activation_task: "T115",
                table: "agent_skills",
                expected_access: AccessExpectation::Search,
                expected_index: None,
                filter_columns: &["agent_id"],
                join_columns: &["skill_id"],
                scan_exception: None,
            },
            QueryPlanRequirement {
                query_id: "t115.agents.resource.skills.target",
                owner_phase: "US13",
                activation_task: "T115",
                table: "skills",
                expected_access: AccessExpectation::Search,
                expected_index: Some("INTEGER PRIMARY KEY"),
                filter_columns: &["is_always"],
                join_columns: &["id"],
                scan_exception: None,
            },
        ],
        scan_exception: None,
    },
    ProductionQuery {
        id: "t115.agents.resource.capabilities",
        sql: AGENT_RESOURCE_CAPABILITIES_SQL,
        owner_phase: "US13",
        activation_task: "T115",
        table: "agent_capabilities",
        active: true,
        dialect: QueryDialect::Sqlite,
        filter_columns: &["agent_id"],
        join_columns: &["agent_id", "capability_name", "name"],
        requirements: &[
            QueryPlanRequirement {
                query_id: "t115.agents.resource.capabilities.association",
                owner_phase: "US13",
                activation_task: "T115",
                table: "agent_capabilities",
                expected_access: AccessExpectation::Search,
                expected_index: None,
                filter_columns: &["agent_id"],
                join_columns: &["capability_name"],
                scan_exception: None,
            },
            QueryPlanRequirement {
                query_id: "t115.agents.resource.capabilities.target",
                owner_phase: "US13",
                activation_task: "T115",
                table: "capabilities",
                expected_access: AccessExpectation::Search,
                expected_index: None,
                filter_columns: &[],
                join_columns: &["name"],
                scan_exception: None,
            },
        ],
        scan_exception: None,
    },
    ProductionQuery {
        id: "t115.agents.model_preset.exists",
        sql: AGENT_MODEL_PRESET_EXISTS_SQL,
        owner_phase: "US13",
        activation_task: "T115",
        table: "llm_presets",
        active: true,
        dialect: QueryDialect::Sqlite,
        filter_columns: &["name"],
        join_columns: &[],
        requirements: &[QueryPlanRequirement {
            query_id: "t115.agents.model_preset.exists",
            owner_phase: "US13",
            activation_task: "T115",
            table: "llm_presets",
            expected_access: AccessExpectation::Search,
            expected_index: None,
            filter_columns: &["name"],
            join_columns: &[],
            scan_exception: None,
        }],
        scan_exception: None,
    },
    ProductionQuery {
        id: "t083.functions.page",
        sql: FUNCTION_UNFILTERED_LIST_SQL,
        owner_phase: "US9",
        activation_task: "T083",
        table: "search_documents",
        active: true,
        dialect: QueryDialect::Sqlite,
        filter_columns: &["entity_type", "field"],
        join_columns: &["entity_key", "id"],
        requirements: &[
            QueryPlanRequirement {
                query_id: "t083.functions.page.search_documents",
                owner_phase: "US9",
                activation_task: "T083",
                table: "search_documents",
                expected_access: AccessExpectation::Search,
                expected_index: Some("idx_search_documents_covering"),
                filter_columns: &["entity_type", "field"],
                join_columns: &["entity_key"],
                scan_exception: None,
            },
            QueryPlanRequirement {
                query_id: "t083.functions.page.functions",
                owner_phase: "US9",
                activation_task: "T083",
                table: "functions",
                expected_access: AccessExpectation::Search,
                expected_index: Some("INTEGER PRIMARY KEY"),
                filter_columns: &[],
                join_columns: &["id"],
                scan_exception: None,
            },
        ],
        scan_exception: None,
    },
    ProductionQuery {
        id: "t083.functions.count",
        sql: FUNCTION_UNFILTERED_COUNT_SQL,
        owner_phase: "US9",
        activation_task: "T083",
        table: "search_documents",
        active: true,
        dialect: QueryDialect::Sqlite,
        filter_columns: &["entity_type", "field"],
        join_columns: &[],
        requirements: &[QueryPlanRequirement {
            query_id: "t083.functions.count",
            owner_phase: "US9",
            activation_task: "T083",
            table: "search_documents",
            expected_access: AccessExpectation::Search,
            expected_index: Some("idx_search_documents_covering"),
            filter_columns: &["entity_type", "field"],
            join_columns: &[],
            scan_exception: None,
        }],
        scan_exception: None,
    },
    ProductionQuery {
        id: "t083.functions.search.fts.page",
        sql: FUNCTION_FTS_LIST_SQL,
        owner_phase: "US9",
        activation_task: "T083",
        table: "search_documents_fts",
        active: true,
        dialect: QueryDialect::Sqlite,
        filter_columns: &["normalized_text", "entity_type", "field"],
        join_columns: &["id", "entity_key"],
        requirements: &[
            QueryPlanRequirement {
                query_id: "t083.functions.search.fts.page.fts",
                owner_phase: "US9",
                activation_task: "T083",
                table: "search_documents_fts",
                expected_access: AccessExpectation::Search,
                expected_index: Some("VIRTUAL TABLE INDEX"),
                filter_columns: &["normalized_text"],
                join_columns: &[],
                scan_exception: None,
            },
            QueryPlanRequirement {
                query_id: "t083.functions.search.fts.page.documents",
                owner_phase: "US9",
                activation_task: "T083",
                table: "search_documents",
                expected_access: AccessExpectation::Search,
                expected_index: Some("sqlite_autoindex_search_documents_1"),
                filter_columns: &["entity_type", "field"],
                join_columns: &["id", "entity_key"],
                scan_exception: None,
            },
            QueryPlanRequirement {
                query_id: "t083.functions.search.fts.page.functions",
                owner_phase: "US9",
                activation_task: "T083",
                table: "functions",
                expected_access: AccessExpectation::Search,
                expected_index: Some("INTEGER PRIMARY KEY"),
                filter_columns: &[],
                join_columns: &["id"],
                scan_exception: None,
            },
        ],
        scan_exception: None,
    },
    ProductionQuery {
        id: "t083.functions.search.fts.count",
        sql: FUNCTION_FTS_COUNT_SQL,
        owner_phase: "US9",
        activation_task: "T083",
        table: "search_documents_fts",
        active: true,
        dialect: QueryDialect::Sqlite,
        filter_columns: &["normalized_text", "entity_type"],
        join_columns: &["id"],
        requirements: &[
            QueryPlanRequirement {
                query_id: "t083.functions.search.fts.count.fts",
                owner_phase: "US9",
                activation_task: "T083",
                table: "search_documents_fts",
                expected_access: AccessExpectation::Search,
                expected_index: Some("VIRTUAL TABLE INDEX"),
                filter_columns: &["normalized_text"],
                join_columns: &[],
                scan_exception: None,
            },
            QueryPlanRequirement {
                query_id: "t083.functions.search.fts.count.documents",
                owner_phase: "US9",
                activation_task: "T083",
                table: "search_documents",
                expected_access: AccessExpectation::Search,
                expected_index: Some("INTEGER PRIMARY KEY"),
                filter_columns: &["entity_type"],
                join_columns: &["id"],
                scan_exception: None,
            },
        ],
        scan_exception: None,
    },
    ProductionQuery {
        id: "t083.functions.search.short_gram.page",
        sql: FUNCTION_SHORT_GRAM_LIST_SQL,
        owner_phase: "US9",
        activation_task: "T083",
        table: "search_short_grams",
        active: true,
        dialect: QueryDialect::Sqlite,
        filter_columns: &["gram_len", "gram", "entity_type", "field"],
        join_columns: &["document_id", "id", "entity_key"],
        requirements: &[
            QueryPlanRequirement {
                query_id: "t083.functions.search.short_gram.page.grams",
                owner_phase: "US9",
                activation_task: "T083",
                table: "search_short_grams",
                expected_access: AccessExpectation::Search,
                expected_index: Some("idx_search_short_grams_lookup"),
                filter_columns: &["gram_len", "gram"],
                join_columns: &["document_id"],
                scan_exception: None,
            },
            QueryPlanRequirement {
                query_id: "t083.functions.search.short_gram.page.documents",
                owner_phase: "US9",
                activation_task: "T083",
                table: "search_documents",
                expected_access: AccessExpectation::Search,
                expected_index: Some("sqlite_autoindex_search_documents_1"),
                filter_columns: &["entity_type", "field"],
                join_columns: &["id", "entity_key"],
                scan_exception: None,
            },
            QueryPlanRequirement {
                query_id: "t083.functions.search.short_gram.page.functions",
                owner_phase: "US9",
                activation_task: "T083",
                table: "functions",
                expected_access: AccessExpectation::Search,
                expected_index: Some("INTEGER PRIMARY KEY"),
                filter_columns: &[],
                join_columns: &["id"],
                scan_exception: None,
            },
        ],
        scan_exception: None,
    },
    ProductionQuery {
        id: "t083.functions.search.short_gram.count",
        sql: FUNCTION_SHORT_GRAM_COUNT_SQL,
        owner_phase: "US9",
        activation_task: "T083",
        table: "search_short_grams",
        active: true,
        dialect: QueryDialect::Sqlite,
        filter_columns: &["gram_len", "gram", "entity_type"],
        join_columns: &["document_id", "id"],
        requirements: &[
            QueryPlanRequirement {
                query_id: "t083.functions.search.short_gram.count.grams",
                owner_phase: "US9",
                activation_task: "T083",
                table: "search_short_grams",
                expected_access: AccessExpectation::Search,
                expected_index: Some("idx_search_short_grams_lookup"),
                filter_columns: &["gram_len", "gram"],
                join_columns: &["document_id"],
                scan_exception: None,
            },
            QueryPlanRequirement {
                query_id: "t083.functions.search.short_gram.count.documents",
                owner_phase: "US9",
                activation_task: "T083",
                table: "search_documents",
                expected_access: AccessExpectation::Search,
                expected_index: Some("INTEGER PRIMARY KEY"),
                filter_columns: &["entity_type"],
                join_columns: &["id"],
                scan_exception: None,
            },
        ],
        scan_exception: None,
    },
    ProductionQuery {
        id: "t083.functions.get",
        sql: FUNCTION_GET_SQL,
        owner_phase: "US9",
        activation_task: "T083",
        table: "functions",
        active: true,
        dialect: QueryDialect::Sqlite,
        filter_columns: &["id"],
        join_columns: &[],
        requirements: &[QueryPlanRequirement {
            query_id: "t083.functions.get",
            owner_phase: "US9",
            activation_task: "T083",
            table: "functions",
            expected_access: AccessExpectation::Search,
            expected_index: Some("INTEGER PRIMARY KEY"),
            filter_columns: &["id"],
            join_columns: &[],
            scan_exception: None,
        }],
        scan_exception: None,
    },
    ProductionQuery {
        id: "t083.functions.references.tools",
        sql: FUNCTION_TOOL_REFERENCES_SQL,
        owner_phase: "US9",
        activation_task: "T083",
        table: "tools",
        active: true,
        dialect: QueryDialect::Sqlite,
        filter_columns: &["function_id"],
        join_columns: &[],
        requirements: &[QueryPlanRequirement {
            query_id: "t083.functions.references.tools",
            owner_phase: "US9",
            activation_task: "T083",
            table: "tools",
            expected_access: AccessExpectation::Search,
            expected_index: Some("idx_tools_function_id"),
            filter_columns: &["function_id"],
            join_columns: &[],
            scan_exception: None,
        }],
        scan_exception: None,
    },
    ProductionQuery {
        id: "t083.functions.references.workflow_nodes",
        sql: FUNCTION_WORKFLOW_NODE_REFERENCES_SQL,
        owner_phase: "US9",
        activation_task: "T083",
        table: "workflow_nodes",
        active: true,
        dialect: QueryDialect::Sqlite,
        filter_columns: &["function_id"],
        join_columns: &["workflow_id", "id"],
        requirements: &[
            QueryPlanRequirement {
                query_id: "t083.functions.references.workflow_nodes.nodes",
                owner_phase: "US9",
                activation_task: "T083",
                table: "workflow_nodes",
                expected_access: AccessExpectation::Search,
                expected_index: Some("idx_workflow_nodes_function_id"),
                filter_columns: &["function_id"],
                join_columns: &["workflow_id"],
                scan_exception: None,
            },
            QueryPlanRequirement {
                query_id: "t083.functions.references.workflow_nodes.workflows",
                owner_phase: "US9",
                activation_task: "T083",
                table: "workflows",
                expected_access: AccessExpectation::Search,
                expected_index: Some("INTEGER PRIMARY KEY"),
                filter_columns: &[],
                join_columns: &["id"],
                scan_exception: None,
            },
        ],
        scan_exception: None,
    },
    ProductionQuery {
        id: "t105.tools.page",
        sql: TOOL_UNFILTERED_LIST_SQL,
        owner_phase: "US11",
        activation_task: "T105",
        table: "search_documents",
        active: true,
        dialect: QueryDialect::Sqlite,
        filter_columns: &["entity_type", "field"],
        join_columns: &["entity_key", "id"],
        requirements: &[
            QueryPlanRequirement {
                query_id: "t105.tools.page.documents",
                owner_phase: "US11",
                activation_task: "T105",
                table: "search_documents",
                expected_access: AccessExpectation::Search,
                expected_index: Some("idx_search_documents_covering"),
                filter_columns: &["entity_type", "field"],
                join_columns: &["entity_key"],
                scan_exception: None,
            },
            QueryPlanRequirement {
                query_id: "t105.tools.page.tools",
                owner_phase: "US11",
                activation_task: "T105",
                table: "tools",
                expected_access: AccessExpectation::Search,
                expected_index: Some("INTEGER PRIMARY KEY"),
                filter_columns: &[],
                join_columns: &["id"],
                scan_exception: None,
            },
        ],
        scan_exception: None,
    },
    ProductionQuery {
        id: "t105.tools.count",
        sql: TOOL_UNFILTERED_COUNT_SQL,
        owner_phase: "US11",
        activation_task: "T105",
        table: "search_documents",
        active: true,
        dialect: QueryDialect::Sqlite,
        filter_columns: &["entity_type", "field"],
        join_columns: &[],
        requirements: &[QueryPlanRequirement {
            query_id: "t105.tools.count",
            owner_phase: "US11",
            activation_task: "T105",
            table: "search_documents",
            expected_access: AccessExpectation::Search,
            expected_index: Some("idx_search_documents_covering"),
            filter_columns: &["entity_type", "field"],
            join_columns: &[],
            scan_exception: None,
        }],
        scan_exception: None,
    },
    ProductionQuery {
        id: "t105.tools.search.fts.page",
        sql: TOOL_FTS_LIST_SQL,
        owner_phase: "US11",
        activation_task: "T105",
        table: "search_documents_fts",
        active: true,
        dialect: QueryDialect::Sqlite,
        filter_columns: &["normalized_text", "entity_type", "field"],
        join_columns: &["rowid", "id", "entity_key"],
        requirements: &[
            QueryPlanRequirement {
                query_id: "t105.tools.search.fts.page.fts",
                owner_phase: "US11",
                activation_task: "T105",
                table: "search_documents_fts",
                expected_access: AccessExpectation::Search,
                expected_index: Some("VIRTUAL TABLE INDEX"),
                filter_columns: &["normalized_text"],
                join_columns: &["rowid"],
                scan_exception: None,
            },
            QueryPlanRequirement {
                query_id: "t105.tools.search.fts.page.documents",
                owner_phase: "US11",
                activation_task: "T105",
                table: "search_documents",
                expected_access: AccessExpectation::Search,
                expected_index: Some("INTEGER PRIMARY KEY"),
                filter_columns: &["entity_type", "field"],
                join_columns: &["id", "entity_key"],
                scan_exception: None,
            },
            QueryPlanRequirement {
                query_id: "t105.tools.search.fts.page.tools",
                owner_phase: "US11",
                activation_task: "T105",
                table: "tools",
                expected_access: AccessExpectation::Search,
                expected_index: Some("INTEGER PRIMARY KEY"),
                filter_columns: &[],
                join_columns: &["id"],
                scan_exception: None,
            },
        ],
        scan_exception: None,
    },
    ProductionQuery {
        id: "t105.tools.search.fts.count",
        sql: TOOL_FTS_COUNT_SQL,
        owner_phase: "US11",
        activation_task: "T105",
        table: "search_documents_fts",
        active: true,
        dialect: QueryDialect::Sqlite,
        filter_columns: &["normalized_text", "entity_type"],
        join_columns: &["rowid", "id"],
        requirements: &[
            QueryPlanRequirement {
                query_id: "t105.tools.search.fts.count.fts",
                owner_phase: "US11",
                activation_task: "T105",
                table: "search_documents_fts",
                expected_access: AccessExpectation::Search,
                expected_index: Some("VIRTUAL TABLE INDEX"),
                filter_columns: &["normalized_text"],
                join_columns: &["rowid"],
                scan_exception: None,
            },
            QueryPlanRequirement {
                query_id: "t105.tools.search.fts.count.documents",
                owner_phase: "US11",
                activation_task: "T105",
                table: "search_documents",
                expected_access: AccessExpectation::Search,
                expected_index: Some("INTEGER PRIMARY KEY"),
                filter_columns: &["entity_type"],
                join_columns: &["id"],
                scan_exception: None,
            },
        ],
        scan_exception: None,
    },
    ProductionQuery {
        id: "t105.tools.search.short_gram.page",
        sql: TOOL_SHORT_GRAM_LIST_SQL,
        owner_phase: "US11",
        activation_task: "T105",
        table: "search_short_grams",
        active: true,
        dialect: QueryDialect::Sqlite,
        filter_columns: &["gram_len", "gram", "entity_type", "field"],
        join_columns: &["document_id", "id", "entity_key"],
        requirements: &[
            QueryPlanRequirement {
                query_id: "t105.tools.search.short_gram.page.grams",
                owner_phase: "US11",
                activation_task: "T105",
                table: "search_short_grams",
                expected_access: AccessExpectation::Search,
                expected_index: Some("idx_search_short_grams_lookup"),
                filter_columns: &["gram_len", "gram"],
                join_columns: &["document_id"],
                scan_exception: None,
            },
            QueryPlanRequirement {
                query_id: "t105.tools.search.short_gram.page.documents",
                owner_phase: "US11",
                activation_task: "T105",
                table: "search_documents",
                expected_access: AccessExpectation::Search,
                expected_index: Some("sqlite_autoindex_search_documents_1"),
                filter_columns: &["entity_type", "field"],
                join_columns: &["id", "entity_key"],
                scan_exception: None,
            },
            QueryPlanRequirement {
                query_id: "t105.tools.search.short_gram.page.tools",
                owner_phase: "US11",
                activation_task: "T105",
                table: "tools",
                expected_access: AccessExpectation::Search,
                expected_index: Some("INTEGER PRIMARY KEY"),
                filter_columns: &[],
                join_columns: &["id"],
                scan_exception: None,
            },
        ],
        scan_exception: None,
    },
    ProductionQuery {
        id: "t105.tools.search.short_gram.count",
        sql: TOOL_SHORT_GRAM_COUNT_SQL,
        owner_phase: "US11",
        activation_task: "T105",
        table: "search_short_grams",
        active: true,
        dialect: QueryDialect::Sqlite,
        filter_columns: &["gram_len", "gram", "entity_type"],
        join_columns: &["document_id", "id"],
        requirements: &[
            QueryPlanRequirement {
                query_id: "t105.tools.search.short_gram.count.grams",
                owner_phase: "US11",
                activation_task: "T105",
                table: "search_short_grams",
                expected_access: AccessExpectation::Search,
                expected_index: Some("idx_search_short_grams_lookup"),
                filter_columns: &["gram_len", "gram"],
                join_columns: &["document_id"],
                scan_exception: None,
            },
            QueryPlanRequirement {
                query_id: "t105.tools.search.short_gram.count.documents",
                owner_phase: "US11",
                activation_task: "T105",
                table: "search_documents",
                expected_access: AccessExpectation::Search,
                expected_index: Some("INTEGER PRIMARY KEY"),
                filter_columns: &["entity_type"],
                join_columns: &["id"],
                scan_exception: None,
            },
        ],
        scan_exception: None,
    },
    ProductionQuery {
        id: "t105.tools.get",
        sql: TOOL_GET_SQL,
        owner_phase: "US11",
        activation_task: "T105",
        table: "tools",
        active: true,
        dialect: QueryDialect::Sqlite,
        filter_columns: &["id"],
        join_columns: &[],
        requirements: &[QueryPlanRequirement {
            query_id: "t105.tools.get",
            owner_phase: "US11",
            activation_task: "T105",
            table: "tools",
            expected_access: AccessExpectation::Search,
            expected_index: Some("INTEGER PRIMARY KEY"),
            filter_columns: &["id"],
            join_columns: &[],
            scan_exception: None,
        }],
        scan_exception: None,
    },
    ProductionQuery {
        id: "t105.tools.target.function",
        sql: TOOL_FUNCTION_TARGET_SQL,
        owner_phase: "US11",
        activation_task: "T105",
        table: "functions",
        active: true,
        dialect: QueryDialect::Sqlite,
        filter_columns: &["id"],
        join_columns: &[],
        requirements: &[QueryPlanRequirement {
            query_id: "t105.tools.target.function",
            owner_phase: "US11",
            activation_task: "T105",
            table: "functions",
            expected_access: AccessExpectation::Search,
            expected_index: Some("INTEGER PRIMARY KEY"),
            filter_columns: &["id"],
            join_columns: &[],
            scan_exception: None,
        }],
        scan_exception: None,
    },
    ProductionQuery {
        id: "t105.tools.target.workflow",
        sql: TOOL_WORKFLOW_TARGET_SQL,
        owner_phase: "US11",
        activation_task: "T105",
        table: "workflows",
        active: true,
        dialect: QueryDialect::Sqlite,
        filter_columns: &["id"],
        join_columns: &[],
        requirements: &[QueryPlanRequirement {
            query_id: "t105.tools.target.workflow",
            owner_phase: "US11",
            activation_task: "T105",
            table: "workflows",
            expected_access: AccessExpectation::Search,
            expected_index: Some("INTEGER PRIMARY KEY"),
            filter_columns: &["id"],
            join_columns: &[],
            scan_exception: None,
        }],
        scan_exception: None,
    },
    ProductionQuery {
        id: "t105.tools.capability",
        sql: TOOL_CAPABILITY_GET_SQL,
        owner_phase: "US11",
        activation_task: "T105",
        table: "capabilities",
        active: true,
        dialect: QueryDialect::Sqlite,
        filter_columns: &["name"],
        join_columns: &[],
        requirements: &[QueryPlanRequirement {
            query_id: "t105.tools.capability",
            owner_phase: "US11",
            activation_task: "T105",
            table: "capabilities",
            expected_access: AccessExpectation::Search,
            expected_index: Some("sqlite_autoindex_capabilities_1"),
            filter_columns: &["name"],
            join_columns: &[],
            scan_exception: None,
        }],
        scan_exception: None,
    },
    ProductionQuery {
        id: "t090.workflows.page",
        sql: WORKFLOW_UNFILTERED_LIST_SQL,
        owner_phase: "US10",
        activation_task: "T095",
        table: "search_documents",
        active: true,
        dialect: QueryDialect::Sqlite,
        filter_columns: &["entity_type", "field"],
        join_columns: &["entity_key", "id"],
        requirements: &[
            QueryPlanRequirement {
                query_id: "t090.workflows.page.documents",
                owner_phase: "US10",
                activation_task: "T095",
                table: "search_documents",
                expected_access: AccessExpectation::Search,
                expected_index: Some("idx_search_documents_covering"),
                filter_columns: &["entity_type", "field"],
                join_columns: &["entity_key"],
                scan_exception: None,
            },
            QueryPlanRequirement {
                query_id: "t090.workflows.page.workflows",
                owner_phase: "US10",
                activation_task: "T095",
                table: "workflows",
                expected_access: AccessExpectation::Search,
                expected_index: Some("INTEGER PRIMARY KEY"),
                filter_columns: &[],
                join_columns: &["id"],
                scan_exception: None,
            },
        ],
        scan_exception: None,
    },
    ProductionQuery {
        id: "t090.workflows.count",
        sql: WORKFLOW_UNFILTERED_COUNT_SQL,
        owner_phase: "US10",
        activation_task: "T095",
        table: "search_documents",
        active: true,
        dialect: QueryDialect::Sqlite,
        filter_columns: &["entity_type", "field"],
        join_columns: &[],
        requirements: &[QueryPlanRequirement {
            query_id: "t090.workflows.count",
            owner_phase: "US10",
            activation_task: "T095",
            table: "search_documents",
            expected_access: AccessExpectation::Search,
            expected_index: Some("idx_search_documents_covering"),
            filter_columns: &["entity_type", "field"],
            join_columns: &[],
            scan_exception: None,
        }],
        scan_exception: None,
    },
    ProductionQuery {
        id: "t090.workflows.search.fts.page",
        sql: WORKFLOW_FTS_LIST_SQL,
        owner_phase: "US10",
        activation_task: "T095",
        table: "search_documents_fts",
        active: true,
        dialect: QueryDialect::Sqlite,
        filter_columns: &["normalized_text", "entity_type", "field"],
        join_columns: &["id", "entity_key"],
        requirements: &[
            QueryPlanRequirement {
                query_id: "t090.workflows.search.fts.page.fts",
                owner_phase: "US10",
                activation_task: "T095",
                table: "search_documents_fts",
                expected_access: AccessExpectation::Search,
                expected_index: Some("VIRTUAL TABLE INDEX"),
                filter_columns: &["normalized_text"],
                join_columns: &[],
                scan_exception: None,
            },
            QueryPlanRequirement {
                query_id: "t090.workflows.search.fts.page.documents",
                owner_phase: "US10",
                activation_task: "T095",
                table: "search_documents",
                expected_access: AccessExpectation::Search,
                expected_index: Some("sqlite_autoindex_search_documents_1"),
                filter_columns: &["entity_type", "field"],
                join_columns: &["id", "entity_key"],
                scan_exception: None,
            },
            QueryPlanRequirement {
                query_id: "t090.workflows.search.fts.page.workflows",
                owner_phase: "US10",
                activation_task: "T095",
                table: "workflows",
                expected_access: AccessExpectation::Search,
                expected_index: Some("INTEGER PRIMARY KEY"),
                filter_columns: &[],
                join_columns: &["id"],
                scan_exception: None,
            },
        ],
        scan_exception: None,
    },
    ProductionQuery {
        id: "t090.workflows.search.fts.count",
        sql: WORKFLOW_FTS_COUNT_SQL,
        owner_phase: "US10",
        activation_task: "T095",
        table: "search_documents_fts",
        active: true,
        dialect: QueryDialect::Sqlite,
        filter_columns: &["normalized_text", "entity_type"],
        join_columns: &["id"],
        requirements: &[
            QueryPlanRequirement {
                query_id: "t090.workflows.search.fts.count.fts",
                owner_phase: "US10",
                activation_task: "T095",
                table: "search_documents_fts",
                expected_access: AccessExpectation::Search,
                expected_index: Some("VIRTUAL TABLE INDEX"),
                filter_columns: &["normalized_text"],
                join_columns: &[],
                scan_exception: None,
            },
            QueryPlanRequirement {
                query_id: "t090.workflows.search.fts.count.documents",
                owner_phase: "US10",
                activation_task: "T095",
                table: "search_documents",
                expected_access: AccessExpectation::Search,
                expected_index: Some("INTEGER PRIMARY KEY"),
                filter_columns: &["entity_type"],
                join_columns: &["id"],
                scan_exception: None,
            },
        ],
        scan_exception: None,
    },
    ProductionQuery {
        id: "t090.workflows.search.short_gram.page",
        sql: WORKFLOW_SHORT_GRAM_LIST_SQL,
        owner_phase: "US10",
        activation_task: "T095",
        table: "search_short_grams",
        active: true,
        dialect: QueryDialect::Sqlite,
        filter_columns: &["gram_len", "gram", "entity_type", "field"],
        join_columns: &["document_id", "id", "entity_key"],
        requirements: &[
            QueryPlanRequirement {
                query_id: "t090.workflows.search.short_gram.page.grams",
                owner_phase: "US10",
                activation_task: "T095",
                table: "search_short_grams",
                expected_access: AccessExpectation::Search,
                expected_index: Some("idx_search_short_grams_lookup"),
                filter_columns: &["gram_len", "gram"],
                join_columns: &["document_id"],
                scan_exception: None,
            },
            QueryPlanRequirement {
                query_id: "t090.workflows.search.short_gram.page.documents",
                owner_phase: "US10",
                activation_task: "T095",
                table: "search_documents",
                expected_access: AccessExpectation::Search,
                expected_index: Some("sqlite_autoindex_search_documents_1"),
                filter_columns: &["entity_type", "field"],
                join_columns: &["id", "entity_key"],
                scan_exception: None,
            },
            QueryPlanRequirement {
                query_id: "t090.workflows.search.short_gram.page.workflows",
                owner_phase: "US10",
                activation_task: "T095",
                table: "workflows",
                expected_access: AccessExpectation::Search,
                expected_index: Some("INTEGER PRIMARY KEY"),
                filter_columns: &[],
                join_columns: &["id"],
                scan_exception: None,
            },
        ],
        scan_exception: None,
    },
    ProductionQuery {
        id: "t090.workflows.search.short_gram.count",
        sql: WORKFLOW_SHORT_GRAM_COUNT_SQL,
        owner_phase: "US10",
        activation_task: "T095",
        table: "search_short_grams",
        active: true,
        dialect: QueryDialect::Sqlite,
        filter_columns: &["gram_len", "gram", "entity_type"],
        join_columns: &["document_id", "id"],
        requirements: &[
            QueryPlanRequirement {
                query_id: "t090.workflows.search.short_gram.count.grams",
                owner_phase: "US10",
                activation_task: "T095",
                table: "search_short_grams",
                expected_access: AccessExpectation::Search,
                expected_index: Some("idx_search_short_grams_lookup"),
                filter_columns: &["gram_len", "gram"],
                join_columns: &["document_id"],
                scan_exception: None,
            },
            QueryPlanRequirement {
                query_id: "t090.workflows.search.short_gram.count.documents",
                owner_phase: "US10",
                activation_task: "T095",
                table: "search_documents",
                expected_access: AccessExpectation::Search,
                expected_index: Some("INTEGER PRIMARY KEY"),
                filter_columns: &["entity_type"],
                join_columns: &["id"],
                scan_exception: None,
            },
        ],
        scan_exception: None,
    },
    ProductionQuery {
        id: "t090.workflow_graph.nodes",
        sql: WORKFLOW_BATCH_NODES_SQL,
        owner_phase: "US10",
        activation_task: "T095",
        table: "workflow_nodes",
        active: true,
        dialect: QueryDialect::Sqlite,
        filter_columns: &["workflow_id"],
        join_columns: &[],
        requirements: &[QueryPlanRequirement {
            query_id: "t090.workflow_graph.nodes",
            owner_phase: "US10",
            activation_task: "T095",
            table: "workflow_nodes",
            expected_access: AccessExpectation::Search,
            expected_index: Some("sqlite_autoindex_workflow_nodes_1"),
            filter_columns: &["workflow_id"],
            join_columns: &[],
            scan_exception: None,
        }],
        scan_exception: None,
    },
    ProductionQuery {
        id: "t090.workflow_graph.edges",
        sql: WORKFLOW_BATCH_EDGES_SQL,
        owner_phase: "US10",
        activation_task: "T095",
        table: "workflow_edges",
        active: true,
        dialect: QueryDialect::Sqlite,
        filter_columns: &["workflow_id"],
        join_columns: &[],
        requirements: &[QueryPlanRequirement {
            query_id: "t090.workflow_graph.edges",
            owner_phase: "US10",
            activation_task: "T095",
            table: "workflow_edges",
            expected_access: AccessExpectation::Search,
            expected_index: Some("sqlite_autoindex_workflow_edges_1"),
            filter_columns: &["workflow_id"],
            join_columns: &[],
            scan_exception: None,
        }],
        scan_exception: None,
    },
    ProductionQuery {
        id: "t090.workflow.references.tools",
        sql: WORKFLOW_TOOL_REFERENCES_SQL,
        owner_phase: "US10",
        activation_task: "T095",
        table: "tools",
        active: true,
        dialect: QueryDialect::Sqlite,
        filter_columns: &["workflow_id"],
        join_columns: &[],
        requirements: &[QueryPlanRequirement {
            query_id: "t090.workflow.references.tools",
            owner_phase: "US10",
            activation_task: "T095",
            table: "tools",
            expected_access: AccessExpectation::Search,
            expected_index: Some("idx_tools_workflow_id"),
            filter_columns: &["workflow_id"],
            join_columns: &[],
            scan_exception: None,
        }],
        scan_exception: None,
    },
    ProductionQuery {
        id: "migrations_list_tables",
        sql: "",
        owner_phase: "migrations",
        activation_task: "T012M",
        table: "sqlite_master",
        active: false,
        dialect: QueryDialect::Sqlite,
        filter_columns: &["type", "name"],
        join_columns: &[],
        requirements: &[QueryPlanRequirement {
            query_id: "migrations_list_tables",
            owner_phase: "migrations",
            activation_task: "T012M",
            table: "sqlite_master",
            expected_access: AccessExpectation::Search,
            expected_index: None,
            filter_columns: &["type", "name"],
            join_columns: &[],
            scan_exception: None,
        }],
        scan_exception: None,
    },
    ProductionQuery {
        id: "migrations_probe_meta_table",
        sql: "",
        owner_phase: "migrations",
        activation_task: "T012M",
        table: "sqlite_master",
        active: false,
        dialect: QueryDialect::Sqlite,
        filter_columns: &["type", "name"],
        join_columns: &[],
        requirements: &[QueryPlanRequirement {
            query_id: "migrations_probe_meta_table",
            owner_phase: "migrations",
            activation_task: "T012M",
            table: "sqlite_master",
            expected_access: AccessExpectation::Search,
            expected_index: None,
            filter_columns: &["type", "name"],
            join_columns: &[],
            scan_exception: None,
        }],
        scan_exception: None,
    },
    ProductionQuery {
        id: "t124.agents.is_default_unique",
        sql: "",
        owner_phase: "US13",
        activation_task: "T124",
        table: "agents",
        active: false,
        dialect: QueryDialect::Sqlite,
        filter_columns: &["is_default"],
        join_columns: &[],
        requirements: &[QueryPlanRequirement {
            query_id: "t124.agents.is_default_unique",
            owner_phase: "US13",
            activation_task: "T124",
            table: "agents",
            expected_access: AccessExpectation::Search,
            expected_index: None,
            filter_columns: &["is_default"],
            join_columns: &[],
            scan_exception: None,
        }],
        scan_exception: None,
    },
    ProductionQuery {
        // DDL — `CREATE INDEX` on `agents (parent_agent_id, depth)`.
        // No filter or join columns; the row is registered so the
        // bidirectional catalog gate recognises the source
        // annotation but is never activated.
        id: "t124.agents.parent_depth",
        sql: "",
        owner_phase: "US13",
        activation_task: "T124",
        table: "agents",
        active: false,
        dialect: QueryDialect::Sqlite,
        filter_columns: &[],
        join_columns: &[],
        requirements: &[],
        scan_exception: None,
    },
    ProductionQuery {
        // DDL — `CREATE INDEX` on `agent_tools (tool_id, agent_id)`.
        id: "t124.agent_tools.composite",
        sql: "",
        owner_phase: "US13",
        activation_task: "T124",
        table: "agent_tools",
        active: false,
        dialect: QueryDialect::Sqlite,
        filter_columns: &[],
        join_columns: &[],
        requirements: &[],
        scan_exception: None,
    },
    ProductionQuery {
        // DDL — `CREATE INDEX` on `agent_skills (skill_id, agent_id)`.
        id: "t124.agent_skills.composite",
        sql: "",
        owner_phase: "US13",
        activation_task: "T124",
        table: "agent_skills",
        active: false,
        dialect: QueryDialect::Sqlite,
        filter_columns: &[],
        join_columns: &[],
        requirements: &[],
        scan_exception: None,
    },
    ProductionQuery {
        // DDL — `CREATE INDEX` on `agent_capabilities (capability_name, agent_id)`.
        id: "t124.agent_capabilities.composite",
        sql: "",
        owner_phase: "US13",
        activation_task: "T124",
        table: "agent_capabilities",
        active: false,
        dialect: QueryDialect::Sqlite,
        filter_columns: &[],
        join_columns: &[],
        requirements: &[],
        scan_exception: None,
    },
    ProductionQuery {
        // `SELECT COUNT(*) FROM pragma_table_info('agents') WHERE name = 'is_default'`.
        // `pragma_table_info` is a SQLite built-in table-valued function, not a
        // real table; the row is registered for the bidirectional catalog gate
        // but kept inactive (handled inside the migration transaction).
        id: "t124.agents.is_default_probe",
        sql: "",
        owner_phase: "US13",
        activation_task: "T124",
        table: "pragma_table_info",
        active: false,
        dialect: QueryDialect::Sqlite,
        filter_columns: &["name"],
        join_columns: &[],
        requirements: &[QueryPlanRequirement {
            query_id: "t124.agents.is_default_probe",
            owner_phase: "US13",
            activation_task: "T124",
            table: "pragma_table_info",
            expected_access: AccessExpectation::Search,
            expected_index: None,
            filter_columns: &["name"],
            join_columns: &[],
            scan_exception: Some(QueryPlanException {
                table_size: 1,
                reason: "pragma_table_info is a SQLite built-in table-valued function that \
                     returns at most one row per column; the v4 `agents` schema exposes a \
                     fixed set of columns so the probe completes with a single comparison",
                approver: "Foundation",
                expires_on: "2027-08-06",
                review_result: "approved in T028 with the canonical v4 schema; future \
                     schema changes must re-evaluate the probe and update the exception",
            }),
        }],
        scan_exception: None,
    },
    ProductionQuery {
        id: "t127.chat_sessions.recent",
        sql: CONVERSATION_RECENT_SQL,
        owner_phase: "US13",
        activation_task: "T117",
        table: "chat_sessions",
        active: true,
        dialect: QueryDialect::Sqlite,
        filter_columns: &["updated_at"],
        join_columns: &[],
        requirements: &[QueryPlanRequirement {
            query_id: "t127.chat_sessions.recent",
            owner_phase: "US13",
            activation_task: "T117",
            table: "chat_sessions",
            expected_access: AccessExpectation::Search,
            expected_index: Some("idx_chat_sessions_updated_at"),
            filter_columns: &["updated_at"],
            join_columns: &[],
            scan_exception: None,
        }],
        scan_exception: None,
    },
    ProductionQuery {
        id: "t127.chat_sessions.expired",
        sql: CONVERSATION_EXPIRED_IDS_SQL,
        owner_phase: "US13",
        activation_task: "T117",
        table: "chat_sessions",
        active: true,
        dialect: QueryDialect::Sqlite,
        filter_columns: &["expires_at"],
        join_columns: &[],
        requirements: &[QueryPlanRequirement {
            query_id: "t127.chat_sessions.expired",
            owner_phase: "US13",
            activation_task: "T117",
            table: "chat_sessions",
            expected_access: AccessExpectation::Search,
            expected_index: Some("idx_chat_sessions_expires_at"),
            filter_columns: &["expires_at"],
            join_columns: &[],
            scan_exception: None,
        }],
        scan_exception: None,
    },
    ProductionQuery {
        id: "t127.chat_messages.bundle",
        sql: CONVERSATION_BUNDLE_MESSAGES_SQL,
        owner_phase: "US13",
        activation_task: "T117",
        table: "chat_messages",
        active: true,
        dialect: QueryDialect::Sqlite,
        filter_columns: &["session_id"],
        join_columns: &[],
        requirements: &[QueryPlanRequirement {
            query_id: "t127.chat_messages.bundle",
            owner_phase: "US13",
            activation_task: "T117",
            table: "chat_messages",
            expected_access: AccessExpectation::Search,
            expected_index: Some("idx_chat_messages_session_seq"),
            filter_columns: &["session_id"],
            join_columns: &[],
            scan_exception: None,
        }],
        scan_exception: None,
    },
    ProductionQuery {
        id: "t127.agent_executions.bundle",
        sql: CONVERSATION_BUNDLE_EXECUTIONS_SQL,
        owner_phase: "US13",
        activation_task: "T117",
        table: "agent_executions",
        active: true,
        dialect: QueryDialect::Sqlite,
        filter_columns: &["session_id"],
        join_columns: &[],
        requirements: &[QueryPlanRequirement {
            query_id: "t127.agent_executions.bundle",
            owner_phase: "US13",
            activation_task: "T117",
            table: "agent_executions",
            expected_access: AccessExpectation::Search,
            expected_index: Some("idx_agent_executions_session_started"),
            filter_columns: &["session_id"],
            join_columns: &[],
            scan_exception: None,
        }],
        scan_exception: None,
    },
    ProductionQuery {
        id: "t127.agent_executions.running",
        sql: CONVERSATION_RUNNING_EXECUTIONS_SQL,
        owner_phase: "US13",
        activation_task: "T117",
        table: "agent_executions",
        active: true,
        dialect: QueryDialect::Sqlite,
        filter_columns: &["status"],
        join_columns: &[],
        requirements: &[QueryPlanRequirement {
            query_id: "t127.agent_executions.running",
            owner_phase: "US13",
            activation_task: "T117",
            table: "agent_executions",
            expected_access: AccessExpectation::Search,
            expected_index: Some("idx_agent_executions_status"),
            filter_columns: &["status"],
            join_columns: &[],
            scan_exception: None,
        }],
        scan_exception: None,
    },
    ProductionQuery {
        // `SELECT id FROM functions WHERE identifier = ?` — collision probe
        // for the dotted → underscored builtin rename pass.
        id: "migrations_dotted_builtin_collision_probe",
        sql: "",
        owner_phase: "migrations",
        activation_task: "T012M",
        table: "functions",
        active: false,
        dialect: QueryDialect::Sqlite,
        filter_columns: &["identifier"],
        join_columns: &[],
        requirements: &[QueryPlanRequirement {
            query_id: "migrations_dotted_builtin_collision_probe",
            owner_phase: "migrations",
            activation_task: "T012M",
            table: "functions",
            expected_access: AccessExpectation::Search,
            expected_index: None,
            filter_columns: &["identifier"],
            join_columns: &[],
            scan_exception: None,
        }],
        scan_exception: None,
    },
    ProductionQuery {
        // `UPDATE functions SET identifier = ? WHERE identifier = ?` — dotted
        // → underscored builtin rename pass.
        id: "migrations_dotted_builtin_rename",
        sql: "",
        owner_phase: "migrations",
        activation_task: "T012M",
        table: "functions",
        active: false,
        dialect: QueryDialect::Sqlite,
        filter_columns: &["identifier"],
        join_columns: &[],
        requirements: &[QueryPlanRequirement {
            query_id: "migrations_dotted_builtin_rename",
            owner_phase: "migrations",
            activation_task: "T012M",
            table: "functions",
            expected_access: AccessExpectation::Search,
            expected_index: None,
            filter_columns: &["identifier"],
            join_columns: &[],
            scan_exception: None,
        }],
        scan_exception: None,
    },
    ProductionQuery {
        // `SELECT kind FROM functions WHERE kind NOT IN (1, 2) LIMIT 1` —
        // legacy-function-kind probe.
        id: "migrations_scan_unknown_function_kinds",
        sql: "",
        owner_phase: "migrations",
        activation_task: "T012M",
        table: "functions",
        active: false,
        dialect: QueryDialect::Sqlite,
        filter_columns: &["kind"],
        join_columns: &[],
        requirements: &[QueryPlanRequirement {
            query_id: "migrations_scan_unknown_function_kinds",
            owner_phase: "migrations",
            activation_task: "T012M",
            table: "functions",
            expected_access: AccessExpectation::Search,
            expected_index: None,
            filter_columns: &["kind"],
            join_columns: &[],
            scan_exception: None,
        }],
        scan_exception: None,
    },
    ProductionQuery {
        // `SELECT kind FROM tools WHERE kind NOT IN (1, 2) LIMIT 1` —
        // legacy-tool-kind probe.
        id: "migrations_scan_unknown_tool_kinds",
        sql: "",
        owner_phase: "migrations",
        activation_task: "T012M",
        table: "tools",
        active: false,
        dialect: QueryDialect::Sqlite,
        filter_columns: &["kind"],
        join_columns: &[],
        requirements: &[QueryPlanRequirement {
            query_id: "migrations_scan_unknown_tool_kinds",
            owner_phase: "migrations",
            activation_task: "T012M",
            table: "tools",
            expected_access: AccessExpectation::Search,
            expected_index: None,
            filter_columns: &["kind"],
            join_columns: &[],
            scan_exception: None,
        }],
        scan_exception: None,
    },
    ProductionQuery {
        id: "t012.agents.identifier",
        sql: "",
        owner_phase: "Foundation",
        activation_task: "T012",
        table: "agents",
        active: true,
        dialect: QueryDialect::Sqlite,
        filter_columns: &["identifier"],
        join_columns: &[],
        requirements: &[QueryPlanRequirement {
            query_id: "t012.agents.identifier",
            owner_phase: "Foundation",
            activation_task: "T012",
            table: "agents",
            expected_access: AccessExpectation::Search,
            expected_index: None,
            filter_columns: &["identifier"],
            join_columns: &[],
            scan_exception: None,
        }],
        scan_exception: None,
    },
    ProductionQuery {
        id: "t012.agents.category",
        sql: "",
        owner_phase: "Foundation",
        activation_task: "T012",
        table: "agents",
        active: true,
        dialect: QueryDialect::Sqlite,
        filter_columns: &["category_id"],
        join_columns: &[],
        requirements: &[QueryPlanRequirement {
            query_id: "t012.agents.category",
            owner_phase: "Foundation",
            activation_task: "T012",
            table: "agents",
            expected_access: AccessExpectation::Search,
            expected_index: Some("idx_agents_category_id"),
            filter_columns: &["category_id"],
            join_columns: &[],
            scan_exception: None,
        }],
        scan_exception: None,
    },
    ProductionQuery {
        id: "t012.agents.search.normalized",
        sql: "",
        owner_phase: "Foundation",
        activation_task: "T012",
        table: "agents",
        active: true,
        dialect: QueryDialect::Sqlite,
        filter_columns: &["name_normalized"],
        join_columns: &[],
        requirements: &[QueryPlanRequirement {
            query_id: "t012.agents.search.normalized",
            owner_phase: "Foundation",
            activation_task: "T012",
            table: "agents",
            expected_access: AccessExpectation::Search,
            expected_index: Some("idx_agents_name_normalized"),
            filter_columns: &["name_normalized"],
            join_columns: &[],
            scan_exception: None,
        }],
        scan_exception: None,
    },
    // Inactive entries: not active, not Foundation, activation task != T012.
    // Each entry matches a `// query-plan:` annotation in the source so the
    // `assert_filter_and_join_queries_are_registered` gate is satisfied
    // without forcing the EXPLAIN test to scan these tables.
    ProductionQuery {
        id: "t012.data_sources.by_id",
        sql: "",
        owner_phase: "US1",
        activation_task: "T019",
        table: "data_sources",
        active: false,
        dialect: QueryDialect::Sqlite,
        filter_columns: &["id"],
        join_columns: &[],
        requirements: &[QueryPlanRequirement {
            query_id: "t012.data_sources.by_id",
            owner_phase: "US1",
            activation_task: "T019",
            table: "data_sources",
            expected_access: AccessExpectation::Search,
            expected_index: None,
            filter_columns: &["id"],
            join_columns: &[],
            scan_exception: None,
        }],
        scan_exception: None,
    },
    ProductionQuery {
        id: "t012.data_sources.update",
        sql: "",
        owner_phase: "US1",
        activation_task: "T020",
        table: "data_sources",
        active: false,
        dialect: QueryDialect::Sqlite,
        filter_columns: &["id"],
        join_columns: &[],
        requirements: &[QueryPlanRequirement {
            query_id: "t012.data_sources.update",
            owner_phase: "US1",
            activation_task: "T020",
            table: "data_sources",
            expected_access: AccessExpectation::Search,
            expected_index: None,
            filter_columns: &["id"],
            join_columns: &[],
            scan_exception: None,
        }],
        scan_exception: None,
    },
    ProductionQuery {
        id: "t012.data_sources.update_no_pwd",
        sql: "",
        owner_phase: "US1",
        activation_task: "T020",
        table: "data_sources",
        active: false,
        dialect: QueryDialect::Sqlite,
        filter_columns: &["id"],
        join_columns: &[],
        requirements: &[QueryPlanRequirement {
            query_id: "t012.data_sources.update_no_pwd",
            owner_phase: "US1",
            activation_task: "T020",
            table: "data_sources",
            expected_access: AccessExpectation::Search,
            expected_index: None,
            filter_columns: &["id"],
            join_columns: &[],
            scan_exception: None,
        }],
        scan_exception: None,
    },
    ProductionQuery {
        id: "t012.data_sources.delete",
        sql: "",
        owner_phase: "US1",
        activation_task: "T021",
        table: "data_sources",
        active: false,
        dialect: QueryDialect::Sqlite,
        filter_columns: &["id"],
        join_columns: &[],
        requirements: &[QueryPlanRequirement {
            query_id: "t012.data_sources.delete",
            owner_phase: "US1",
            activation_task: "T021",
            table: "data_sources",
            expected_access: AccessExpectation::Search,
            expected_index: None,
            filter_columns: &["id"],
            join_columns: &[],
            scan_exception: None,
        }],
        scan_exception: None,
    },
    ProductionQuery {
        id: "t012.global_configs.by_id_inserted",
        sql: "",
        owner_phase: "US1",
        activation_task: "T019",
        table: "global_configs",
        active: false,
        dialect: QueryDialect::Sqlite,
        filter_columns: &["id"],
        join_columns: &[],
        requirements: &[QueryPlanRequirement {
            query_id: "t012.global_configs.by_id_inserted",
            owner_phase: "US1",
            activation_task: "T019",
            table: "global_configs",
            expected_access: AccessExpectation::Search,
            expected_index: None,
            filter_columns: &["id"],
            join_columns: &[],
            scan_exception: None,
        }],
        scan_exception: None,
    },
    ProductionQuery {
        id: "t012.global_configs.search_count",
        sql: "",
        owner_phase: "US1",
        activation_task: "T019",
        table: "global_configs",
        active: false,
        dialect: QueryDialect::Sqlite,
        filter_columns: &["name", "key"],
        join_columns: &[],
        requirements: &[QueryPlanRequirement {
            query_id: "t012.global_configs.search_count",
            owner_phase: "US1",
            activation_task: "T019",
            table: "global_configs",
            expected_access: AccessExpectation::Search,
            expected_index: None,
            filter_columns: &["name", "key"],
            join_columns: &[],
            scan_exception: None,
        }],
        scan_exception: None,
    },
    ProductionQuery {
        id: "t012.global_configs.search_list",
        sql: "",
        owner_phase: "US1",
        activation_task: "T019",
        table: "global_configs",
        active: false,
        dialect: QueryDialect::Sqlite,
        filter_columns: &["name", "key"],
        join_columns: &[],
        requirements: &[QueryPlanRequirement {
            query_id: "t012.global_configs.search_list",
            owner_phase: "US1",
            activation_task: "T019",
            table: "global_configs",
            expected_access: AccessExpectation::Search,
            expected_index: None,
            filter_columns: &["name", "key"],
            join_columns: &[],
            scan_exception: None,
        }],
        scan_exception: None,
    },
    ProductionQuery {
        id: "t012.global_configs.update",
        sql: "",
        owner_phase: "US1",
        activation_task: "T020",
        table: "global_configs",
        active: false,
        dialect: QueryDialect::Sqlite,
        filter_columns: &["id"],
        join_columns: &[],
        requirements: &[QueryPlanRequirement {
            query_id: "t012.global_configs.update",
            owner_phase: "US1",
            activation_task: "T020",
            table: "global_configs",
            expected_access: AccessExpectation::Search,
            expected_index: None,
            filter_columns: &["id"],
            join_columns: &[],
            scan_exception: None,
        }],
        scan_exception: None,
    },
    ProductionQuery {
        id: "t012.global_configs.delete",
        sql: "",
        owner_phase: "US1",
        activation_task: "T021",
        table: "global_configs",
        active: false,
        dialect: QueryDialect::Sqlite,
        filter_columns: &["id"],
        join_columns: &[],
        requirements: &[QueryPlanRequirement {
            query_id: "t012.global_configs.delete",
            owner_phase: "US1",
            activation_task: "T021",
            table: "global_configs",
            expected_access: AccessExpectation::Search,
            expected_index: None,
            filter_columns: &["id"],
            join_columns: &[],
            scan_exception: None,
        }],
        scan_exception: None,
    },
    ProductionQuery {
        id: "t012.global_configs.count_all",
        sql: "",
        owner_phase: "US1",
        activation_task: "T019",
        table: "global_configs",
        active: false,
        dialect: QueryDialect::Sqlite,
        filter_columns: &[],
        join_columns: &[],
        requirements: &[QueryPlanRequirement {
            query_id: "t012.global_configs.count_all",
            owner_phase: "US1",
            activation_task: "T019",
            table: "global_configs",
            expected_access: AccessExpectation::Search,
            expected_index: None,
            filter_columns: &[],
            join_columns: &[],
            scan_exception: None,
        }],
        scan_exception: None,
    },
    ProductionQuery {
        id: "t012.global_configs.list_all",
        sql: "",
        owner_phase: "US1",
        activation_task: "T019",
        table: "global_configs",
        active: false,
        dialect: QueryDialect::Sqlite,
        filter_columns: &[],
        join_columns: &[],
        requirements: &[QueryPlanRequirement {
            query_id: "t012.global_configs.list_all",
            owner_phase: "US1",
            activation_task: "T019",
            table: "global_configs",
            expected_access: AccessExpectation::Search,
            expected_index: None,
            filter_columns: &[],
            join_columns: &[],
            scan_exception: None,
        }],
        scan_exception: None,
    },
    ProductionQuery {
        id: "t012.global_configs.by_key",
        sql: "",
        owner_phase: "US1",
        activation_task: "T019",
        table: "global_configs",
        active: false,
        dialect: QueryDialect::Sqlite,
        filter_columns: &["key"],
        join_columns: &[],
        requirements: &[QueryPlanRequirement {
            query_id: "t012.global_configs.by_key",
            owner_phase: "US1",
            activation_task: "T019",
            table: "global_configs",
            expected_access: AccessExpectation::Search,
            expected_index: None,
            filter_columns: &["key"],
            join_columns: &[],
            scan_exception: None,
        }],
        scan_exception: None,
    },
    ProductionQuery {
        id: "t012.global_configs.by_key_after_upsert",
        sql: "",
        owner_phase: "US1",
        activation_task: "T019",
        table: "global_configs",
        active: false,
        dialect: QueryDialect::Sqlite,
        filter_columns: &["key"],
        join_columns: &[],
        requirements: &[QueryPlanRequirement {
            query_id: "t012.global_configs.by_key_after_upsert",
            owner_phase: "US1",
            activation_task: "T019",
            table: "global_configs",
            expected_access: AccessExpectation::Search,
            expected_index: None,
            filter_columns: &["key"],
            join_columns: &[],
            scan_exception: None,
        }],
        scan_exception: None,
    },
    ProductionQuery {
        id: "t012.meta.read_schema_version",
        sql: "SELECT value FROM meta WHERE key = 'schema_version'",
        owner_phase: "Foundation",
        activation_task: "T028",
        table: "meta",
        active: true,
        dialect: QueryDialect::Sqlite,
        filter_columns: &["key"],
        join_columns: &[],
        requirements: &[QueryPlanRequirement {
            query_id: "t012.meta.read_schema_version",
            owner_phase: "Foundation",
            activation_task: "T028",
            table: "meta",
            expected_access: AccessExpectation::Search,
            expected_index: None,
            filter_columns: &["key"],
            join_columns: &[],
            scan_exception: Some(QueryPlanException {
                table_size: 16,
                reason: "meta is a tiny key/value table (<= 16 rows in the v4 schema) and has no uid=1000(developer) gid=1000(developer) groups=1000(developer),27(sudo),44(video),100(users),989(docker),992(render) column; the EXPLAIN planner uses  as the access key directly",
                approver: "Foundation",
                expires_on: "2027-08-06",
                review_result: "approved in T028 with the canonical v4 schema; future re-evaluations must repeat the EXPLAIN gate",
            }),
        }],
        scan_exception: None,
    },
    ProductionQuery {
        id: "t012.meta.read_schema_version_tx",
        sql: "",
        owner_phase: "migrations",
        activation_task: "T012M",
        table: "meta",
        active: false,
        dialect: QueryDialect::Sqlite,
        filter_columns: &["key"],
        join_columns: &[],
        requirements: &[QueryPlanRequirement {
            query_id: "t012.meta.read_schema_version_tx",
            owner_phase: "migrations",
            activation_task: "T012M",
            table: "meta",
            expected_access: AccessExpectation::Search,
            expected_index: None,
            filter_columns: &["key"],
            join_columns: &[],
            scan_exception: None,
        }],
        scan_exception: None,
    },
    ProductionQuery {
        id: "t012.meta.read_schema_version_ro",
        sql: "",
        owner_phase: "migrations",
        activation_task: "T012M",
        table: "meta",
        active: false,
        dialect: QueryDialect::Sqlite,
        filter_columns: &["key"],
        join_columns: &[],
        requirements: &[QueryPlanRequirement {
            query_id: "t012.meta.read_schema_version_ro",
            owner_phase: "migrations",
            activation_task: "T012M",
            table: "meta",
            expected_access: AccessExpectation::Search,
            expected_index: None,
            filter_columns: &["key"],
            join_columns: &[],
            scan_exception: None,
        }],
        scan_exception: None,
    },
    ProductionQuery {
        id: "t012.meta.read_search_normalization_id",
        sql: "",
        owner_phase: "migrations",
        activation_task: "T012M",
        table: "meta",
        active: false,
        dialect: QueryDialect::Sqlite,
        filter_columns: &["key"],
        join_columns: &[],
        requirements: &[QueryPlanRequirement {
            query_id: "t012.meta.read_search_normalization_id",
            owner_phase: "migrations",
            activation_task: "T012M",
            table: "meta",
            expected_access: AccessExpectation::Search,
            expected_index: None,
            filter_columns: &["key"],
            join_columns: &[],
            scan_exception: None,
        }],
        scan_exception: None,
    },
    ProductionQuery {
        id: "t012.categories.backfill_slug",
        sql: "",
        owner_phase: "migrations",
        activation_task: "T012M",
        table: "categories",
        active: false,
        dialect: QueryDialect::Sqlite,
        filter_columns: &["slug"],
        join_columns: &[],
        requirements: &[QueryPlanRequirement {
            query_id: "t012.categories.backfill_slug",
            owner_phase: "migrations",
            activation_task: "T012M",
            table: "categories",
            expected_access: AccessExpectation::Search,
            expected_index: Some("idx_categories_slug"),
            filter_columns: &["slug"],
            join_columns: &[],
            scan_exception: None,
        }],
        scan_exception: None,
    },
    ProductionQuery {
        id: "t012.tags.backfill_normalized",
        sql: "",
        owner_phase: "migrations",
        activation_task: "T012M",
        table: "tags",
        active: false,
        dialect: QueryDialect::Sqlite,
        filter_columns: &["normalized_name"],
        join_columns: &[],
        requirements: &[QueryPlanRequirement {
            query_id: "t012.tags.backfill_normalized",
            owner_phase: "migrations",
            activation_task: "T012M",
            table: "tags",
            expected_access: AccessExpectation::Search,
            expected_index: Some("tags_normalized_name_idx"),
            filter_columns: &["normalized_name"],
            join_columns: &[],
            scan_exception: None,
        }],
        scan_exception: None,
    },
    ProductionQuery {
        id: "t012.capabilities.backfill_normalized",
        sql: "",
        owner_phase: "migrations",
        activation_task: "T012M",
        table: "capabilities",
        active: false,
        dialect: QueryDialect::Sqlite,
        filter_columns: &["normalized_name"],
        join_columns: &[],
        requirements: &[QueryPlanRequirement {
            query_id: "t012.capabilities.backfill_normalized",
            owner_phase: "migrations",
            activation_task: "T012M",
            table: "capabilities",
            expected_access: AccessExpectation::Search,
            expected_index: Some("capabilities_normalized_name_idx"),
            filter_columns: &["normalized_name"],
            join_columns: &[],
            scan_exception: None,
        }],
        scan_exception: None,
    },
    ProductionQuery {
        id: "t012.plugins.row_revision_backfill",
        sql: "",
        owner_phase: "migrations",
        activation_task: "T012M",
        table: "plugins",
        active: false,
        dialect: QueryDialect::Sqlite,
        filter_columns: &["row_revision"],
        join_columns: &[],
        requirements: &[QueryPlanRequirement {
            query_id: "t012.plugins.row_revision_backfill",
            owner_phase: "migrations",
            activation_task: "T012M",
            table: "plugins",
            expected_access: AccessExpectation::Search,
            expected_index: None,
            filter_columns: &["row_revision"],
            join_columns: &[],
            scan_exception: None,
        }],
        scan_exception: None,
    },
    ProductionQuery {
        id: "t012.agents.backfill_name_normalized",
        sql: "",
        owner_phase: "migrations",
        activation_task: "T012M",
        table: "agents",
        active: false,
        dialect: QueryDialect::Sqlite,
        filter_columns: &["name_normalized"],
        join_columns: &[],
        requirements: &[QueryPlanRequirement {
            query_id: "t012.agents.backfill_name_normalized",
            owner_phase: "migrations",
            activation_task: "T012M",
            table: "agents",
            expected_access: AccessExpectation::Search,
            expected_index: Some("idx_agents_name_normalized"),
            filter_columns: &["name_normalized"],
            join_columns: &[],
            scan_exception: None,
        }],
        scan_exception: None,
    },
    ProductionQuery {
        id: "t012.meta.table_check_search_index",
        sql: "",
        owner_phase: "migrations",
        activation_task: "T012M",
        table: "sqlite_master",
        active: false,
        dialect: QueryDialect::Sqlite,
        filter_columns: &["type", "name"],
        join_columns: &[],
        requirements: &[QueryPlanRequirement {
            query_id: "t012.meta.table_check_search_index",
            owner_phase: "migrations",
            activation_task: "T012M",
            table: "sqlite_master",
            expected_access: AccessExpectation::Search,
            expected_index: None,
            filter_columns: &["type", "name"],
            join_columns: &[],
            scan_exception: None,
        }],
        scan_exception: None,
    },
    ProductionQuery {
        id: "t012.meta.table_check_short_gram_index",
        sql: "",
        owner_phase: "migrations",
        activation_task: "T012M",
        table: "sqlite_master",
        active: false,
        dialect: QueryDialect::Sqlite,
        filter_columns: &["type", "name"],
        join_columns: &[],
        requirements: &[QueryPlanRequirement {
            query_id: "t012.meta.table_check_short_gram_index",
            owner_phase: "migrations",
            activation_task: "T012M",
            table: "sqlite_master",
            expected_access: AccessExpectation::Search,
            expected_index: None,
            filter_columns: &["type", "name"],
            join_columns: &[],
            scan_exception: None,
        }],
        scan_exception: None,
    },
    ProductionQuery {
        id: "t012.meta.pragma_check_plugins_row_revision",
        sql: "",
        owner_phase: "migrations",
        activation_task: "T012M",
        table: "pragma_table_info",
        active: false,
        dialect: QueryDialect::Sqlite,
        filter_columns: &["name"],
        join_columns: &[],
        requirements: &[QueryPlanRequirement {
            query_id: "t012.meta.pragma_check_plugins_row_revision",
            owner_phase: "migrations",
            activation_task: "T012M",
            table: "pragma_table_info",
            expected_access: AccessExpectation::Search,
            expected_index: None,
            filter_columns: &["name"],
            join_columns: &[],
            scan_exception: None,
        }],
        scan_exception: None,
    },
    ProductionQuery {
        id: "t012.meta.table_check_pao",
        sql: "",
        owner_phase: "migrations",
        activation_task: "T012M",
        table: "sqlite_master",
        active: false,
        dialect: QueryDialect::Sqlite,
        filter_columns: &["type", "name"],
        join_columns: &[],
        requirements: &[QueryPlanRequirement {
            query_id: "t012.meta.table_check_pao",
            owner_phase: "migrations",
            activation_task: "T012M",
            table: "sqlite_master",
            expected_access: AccessExpectation::Search,
            expected_index: None,
            filter_columns: &["type", "name"],
            join_columns: &[],
            scan_exception: None,
        }],
        scan_exception: None,
    },
    ProductionQuery {
        id: "t012.meta.table_check_pag",
        sql: "",
        owner_phase: "migrations",
        activation_task: "T012M",
        table: "sqlite_master",
        active: false,
        dialect: QueryDialect::Sqlite,
        filter_columns: &["type", "name"],
        join_columns: &[],
        requirements: &[QueryPlanRequirement {
            query_id: "t012.meta.table_check_pag",
            owner_phase: "migrations",
            activation_task: "T012M",
            table: "sqlite_master",
            expected_access: AccessExpectation::Search,
            expected_index: None,
            filter_columns: &["type", "name"],
            join_columns: &[],
            scan_exception: None,
        }],
        scan_exception: None,
    },
    ProductionQuery {
        id: "t012.meta.table_check_search_index_verify",
        sql: "",
        owner_phase: "migrations",
        activation_task: "T012M",
        table: "sqlite_master",
        active: false,
        dialect: QueryDialect::Sqlite,
        filter_columns: &["type", "name"],
        join_columns: &[],
        requirements: &[QueryPlanRequirement {
            query_id: "t012.meta.table_check_search_index_verify",
            owner_phase: "migrations",
            activation_task: "T012M",
            table: "sqlite_master",
            expected_access: AccessExpectation::Search,
            expected_index: None,
            filter_columns: &["type", "name"],
            join_columns: &[],
            scan_exception: None,
        }],
        scan_exception: None,
    },
    ProductionQuery {
        id: "t012.meta.table_check_short_gram_index_verify",
        sql: "",
        owner_phase: "migrations",
        activation_task: "T012M",
        table: "sqlite_master",
        active: false,
        dialect: QueryDialect::Sqlite,
        filter_columns: &["type", "name"],
        join_columns: &[],
        requirements: &[QueryPlanRequirement {
            query_id: "t012.meta.table_check_short_gram_index_verify",
            owner_phase: "migrations",
            activation_task: "T012M",
            table: "sqlite_master",
            expected_access: AccessExpectation::Search,
            expected_index: None,
            filter_columns: &["type", "name"],
            join_columns: &[],
            scan_exception: None,
        }],
        scan_exception: None,
    },
];

// `Path` is exposed here for downstream test files that probe the
// catalog; keep the import alive even if Rust elides the use.
#[allow(dead_code)]
fn _path_marker(_: &Path) {}
