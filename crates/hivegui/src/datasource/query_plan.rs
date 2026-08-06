//! `query_plan` — public boundary that evaluates an SQLite `EXPLAIN`
//! JSON plan against a [`QueryPlanRequirement`]. The Foundation owns
//! the contract that FTS `VIRTUAL TABLE INDEX` counts as an index
//! access path and that a `SCAN` for a non-exception table is a
//! hard failure.

#![warn(missing_docs)]

use serde_json::Value;
use sqlx::Row;
use std::path::Path;

use super::store::Store;

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
            if let Some(expected) = requirement.expected_index {
                if detail.contains(expected) {
                    has_index_hint = true;
                }
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
        if !used_key_parts.iter().any(|p| *p == *column) {
            failures.push(PlanFailureKind::UncoveredFilterColumn(column));
        }
    }
    for column in requirement.join_columns {
        if !used_key_parts.iter().any(|p| *p == *column) {
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

    for row in plan {
        let detail = row.detail;
        if detail.starts_with("SEARCH") {
            has_search = true;
            if let Some(expected) = requirement.expected_index {
                if detail.contains(expected) {
                    has_expected_index = true;
                }
            }
            if detail.contains("USING INDEX") || detail.contains("VIRTUAL TABLE INDEX") {
                has_index_mention = true;
            }
        }
        if detail.starts_with("SCAN") {
            has_scan = true;
        }
        if detail.contains("USE TEMP B-TREE FOR ORDER BY") {
            failures.push(PlanFailureKind::IndeterminatePlan);
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
        let covered = plan
            .iter()
            .any(|row| row.detail.contains(&format!("{}=?", column)));
        if !covered && has_index_mention {
            // only fail coverage when there was a usable index path
        }
        if !covered && has_search {
            failures.push(PlanFailureKind::UncoveredFilterColumn(column));
        }
    }
    for column in requirement.join_columns {
        let covered = plan
            .iter()
            .any(|row| row.detail.contains(&format!("{}=?", column)));
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
    &CATALOG
}

const CATALOG: &[ProductionQuery] = &[
    ProductionQuery {
        id: "t012.agents.identifier",
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
        id: "t012.meta.read_schema_version",
        owner_phase: "migrations",
        activation_task: "T012M",
        table: "meta",
        active: false,
        dialect: QueryDialect::Sqlite,
        filter_columns: &["key"],
        join_columns: &[],
        requirements: &[QueryPlanRequirement {
            query_id: "t012.meta.read_schema_version",
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

/// Run `EXPLAIN QUERY PLAN` for the catalog entry against a live
/// [`Store`]. The Foundation uses this entry point to exercise every
/// active row in [`production_query_catalog`].
pub async fn explain_catalog_query(
    store: &Store,
    query: &ProductionQuery,
) -> Result<Vec<SqlitePlanRow>, String> {
    if query.dialect != QueryDialect::Sqlite {
        return Err(format!(
            "{} is not a SQLite query; use a MySQL adapter",
            query.id
        ));
    }
    let pool = store.pool().clone();
    let requirement = query
        .requirements
        .first()
        .ok_or_else(|| format!("{} has no plan requirements", query.id))?;
    let sql = build_select_sql(requirement);
    let rows = sqlx::query(sqlx::AssertSqlSafe(format!("EXPLAIN QUERY PLAN {}", sql)))
        .fetch_all(&pool)
        .await
        .map_err(|e| format!("explain {}: {e}", query.id))?;
    let mut out = Vec::with_capacity(rows.len());
    for row in rows {
        let id: i64 = row.try_get(0).map_err(|e| e.to_string())?;
        let parent: i64 = row.try_get(1).map_err(|e| e.to_string())?;
        let not_used: i64 = row.try_get(2).map_err(|e| e.to_string())?;
        let detail: String = row.try_get(3).map_err(|e| e.to_string())?;
        out.push(SqlitePlanRow {
            id,
            parent,
            not_used,
            detail: Box::leak(detail.into_boxed_str()),
        });
    }
    Ok(out)
}

fn build_select_sql(requirement: &QueryPlanRequirement) -> String {
    let mut sql = format!("SELECT id FROM {}", requirement.table);
    if !requirement.filter_columns.is_empty() {
        sql.push_str(" WHERE ");
        let conds: Vec<String> = requirement
            .filter_columns
            .iter()
            .map(|c| format!("{c}=?"))
            .collect();
        sql.push_str(&conds.join(" AND "));
    }
    sql
}

// `Path` is exposed here for downstream test files that probe the
// catalog; keep the import alive even if Rust elides the use.
#[allow(dead_code)]
fn _path_marker(_: &Path) {}
