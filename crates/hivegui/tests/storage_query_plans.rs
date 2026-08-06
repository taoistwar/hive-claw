//! Red EXPLAIN contract for every production filtering and joining query.

mod support;

use std::{
    collections::{BTreeMap, BTreeSet},
    fs,
    path::{Path, PathBuf},
};

use chrono::Utc;
use hivegui::datasource::{
    query_plan::{
        AccessExpectation, FtsPlanExpectation, FtsPlanVerdict, PlanFailureKind, ProductionQuery,
        QueryDialect, QueryPlanException, QueryPlanRequirement, SqlitePlanRow, evaluate_fts_plan,
        evaluate_mysql_plan, evaluate_sqlite_plan, explain_catalog_query, production_query_catalog,
    },
    store::{Store, StoreOpenOptions},
};
use serde_json::json;
use support::TestWorkspace;

const OWNER_PHASE: &str = "Foundation";
const APPROVAL_TASK: &str = "T012";
const SYNTHETIC_REVIEW_DATE: &str = "2026-07-22";

fn requirement(
    expected_access: AccessExpectation,
    expected_index: Option<&'static str>,
    filter_columns: &'static [&'static str],
    join_columns: &'static [&'static str],
    exception: Option<QueryPlanException>,
) -> QueryPlanRequirement {
    QueryPlanRequirement {
        query_id: "t012.synthetic",
        owner_phase: OWNER_PHASE,
        activation_task: APPROVAL_TASK,
        table: "agents",
        expected_access,
        expected_index,
        filter_columns,
        join_columns,
        scan_exception: exception,
    }
}

fn sqlite_row(detail: &'static str) -> SqlitePlanRow {
    SqlitePlanRow {
        id: 3,
        parent: 0,
        not_used: 0,
        detail,
    }
}

#[test]
fn production_catalog_is_exhaustive_owned_and_phase_activated() {
    let catalog = production_query_catalog();
    let current_date = current_utc_date();
    assert!(
        !catalog.is_empty(),
        "Foundation must publish its production query catalog"
    );

    let mut ids = BTreeSet::new();
    for query in catalog {
        assert!(!query.id.trim().is_empty());
        assert!(
            ids.insert(query.id.to_owned()),
            "duplicate query-plan id {}",
            query.id
        );
        assert!(
            !query.owner_phase.trim().is_empty(),
            "{} lacks owner_phase",
            query.id
        );
        assert!(
            !query.activation_task.trim().is_empty(),
            "{} lacks activation/approval task",
            query.id
        );

        if query.owner_phase == OWNER_PHASE {
            assert!(
                query.active,
                "Foundation query {} cannot be deferred",
                query.id
            );
            assert!(
                matches!(query.activation_task, "T012" | "T028"),
                "Foundation query {} has an unrelated activation task",
                query.id
            );
        } else {
            assert!(
                !query.active,
                "future story query {} must be activated by its own Tests/Reviewer phase",
                query.id
            );
            assert_ne!(query.activation_task, APPROVAL_TASK);
        }

        if query.has_filter_or_join() {
            assert!(
                !query.requirements.is_empty(),
                "{} has filters/joins but no expected access path",
                query.id
            );
            let covered = query
                .requirements
                .iter()
                .flat_map(|requirement| {
                    requirement
                        .filter_columns
                        .iter()
                        .chain(requirement.join_columns.iter())
                })
                .copied()
                .collect::<BTreeSet<_>>();
            for column in query.filter_columns.iter().chain(query.join_columns.iter()) {
                assert!(
                    covered.contains(column),
                    "{} leaves filter/join column {} outside its plan requirements",
                    query.id,
                    column
                );
            }
        }

        if let Some(exception) = &query.scan_exception {
            assert_complete_exception(query, exception, &current_date);
        }
    }

    let source_registrations = source_query_plan_registrations(foundation_query_source_files(
        &Path::new(env!("CARGO_MANIFEST_DIR")).join("src/datasource"),
    ));
    for registration in &source_registrations {
        let query = catalog
            .iter()
            .find(|query| query.id == registration.id)
            .unwrap_or_else(|| {
                panic!(
                    "source query-plan id {} at {}:{} is absent from the catalog",
                    registration.id,
                    registration.path.display(),
                    registration.line
                )
            });
        assert_eq!(query.owner_phase, registration.owner_phase);
        assert_eq!(query.activation_task, registration.activation_task);
    }

    let active_foundation_catalog_ids = catalog
        .iter()
        .filter(|query| query.active && query.owner_phase == OWNER_PHASE)
        .map(|query| query.id.to_owned())
        .collect::<BTreeSet<_>>();
    let active_foundation_source_ids = source_registrations
        .iter()
        .filter(|registration| registration.owner_phase == OWNER_PHASE)
        .map(|registration| registration.id.clone())
        .collect::<BTreeSet<_>>();
    assert!(!active_foundation_catalog_ids.is_empty());
    assert_eq!(
        active_foundation_source_ids, active_foundation_catalog_ids,
        "Foundation filter/join source IDs and active production catalog IDs must match bidirectionally"
    );
}

#[test]
fn sqlite_search_with_expected_index_and_all_filter_join_columns_passes() {
    let expectation = requirement(
        AccessExpectation::Search,
        Some("idx_agents_identifier_category_id"),
        &["identifier", "category_id"],
        &["category_id"],
        None,
    );
    let plan = [sqlite_row(
        "SEARCH agents USING INDEX idx_agents_identifier_category_id (identifier=? AND category_id=?)",
    )];

    let verdict = evaluate_sqlite_plan(&expectation, &plan, SYNTHETIC_REVIEW_DATE);

    assert!(verdict.is_accepted(), "{:?}", verdict.failures());
    assert!(!verdict.used_exception());
}

#[test]
fn sqlite_missing_search_or_expected_index_is_rejected() {
    let expectation = requirement(
        AccessExpectation::Search,
        Some("idx_agents_identifier"),
        &["identifier"],
        &[],
        None,
    );

    let scan = evaluate_sqlite_plan(
        &expectation,
        &[sqlite_row("SCAN agents")],
        SYNTHETIC_REVIEW_DATE,
    );
    assert!(scan.has_failure(PlanFailureKind::MissingSearch));
    assert!(scan.has_failure(PlanFailureKind::UnapprovedFullScan));

    let wrong_index = evaluate_sqlite_plan(
        &expectation,
        &[sqlite_row(
            "SEARCH agents USING INDEX idx_agents_name (identifier=?)",
        )],
        SYNTHETIC_REVIEW_DATE,
    );
    assert!(wrong_index.has_failure(PlanFailureKind::MissingExpectedIndex));
}

#[test]
fn sqlite_rejects_any_filter_or_join_column_not_covered_by_the_access_path() {
    let expectation = requirement(
        AccessExpectation::Search,
        Some("idx_agents_identifier"),
        &["identifier", "status"],
        &["category_id"],
        None,
    );
    let verdict = evaluate_sqlite_plan(
        &expectation,
        &[sqlite_row(
            "SEARCH agents USING INDEX idx_agents_identifier (identifier=?)",
        )],
        SYNTHETIC_REVIEW_DATE,
    );

    assert!(verdict.has_failure(PlanFailureKind::UncoveredFilterColumn("status")));
    assert!(verdict.has_failure(PlanFailureKind::UncoveredJoinColumn("category_id")));
}

#[test]
fn sqlite_non_small_scan_and_indeterminate_plan_are_blocking_failures() {
    let expectation = requirement(
        AccessExpectation::Search,
        Some("idx_agents_identifier"),
        &["identifier"],
        &[],
        None,
    );

    let full_scan = evaluate_sqlite_plan(
        &expectation,
        &[sqlite_row("SCAN agents")],
        SYNTHETIC_REVIEW_DATE,
    );
    assert!(full_scan.has_failure(PlanFailureKind::UnapprovedFullScan));

    let indeterminate = evaluate_sqlite_plan(
        &expectation,
        &[sqlite_row("USE TEMP B-TREE FOR ORDER BY")],
        SYNTHETIC_REVIEW_DATE,
    );
    assert!(indeterminate.has_failure(PlanFailureKind::IndeterminatePlan));
}

#[test]
fn scan_exception_requires_every_approval_field_and_must_be_current() {
    let approved = QueryPlanException {
        table_size: 8,
        reason: "fixed schema-version metadata table",
        approver: "sqlite-reviewer",
        expires_on: "2026-12-31",
        review_result: "approved: scan remains cheaper than an index",
    };
    let expectation = requirement(
        AccessExpectation::Scan,
        None,
        &["version"],
        &[],
        Some(approved),
    );
    let verdict = evaluate_sqlite_plan(
        &expectation,
        &[sqlite_row("SCAN schema_versions")],
        SYNTHETIC_REVIEW_DATE,
    );
    assert!(verdict.is_accepted(), "{:?}", verdict.failures());
    assert!(verdict.used_exception());

    for invalid in [
        QueryPlanException {
            table_size: 0,
            reason: "fixed table",
            approver: "sqlite-reviewer",
            expires_on: "2026-12-31",
            review_result: "approved",
        },
        QueryPlanException {
            table_size: 8,
            reason: "",
            approver: "sqlite-reviewer",
            expires_on: "2026-12-31",
            review_result: "approved",
        },
        QueryPlanException {
            table_size: 8,
            reason: "fixed table",
            approver: "",
            expires_on: "2026-12-31",
            review_result: "approved",
        },
        QueryPlanException {
            table_size: 8,
            reason: "fixed table",
            approver: "sqlite-reviewer",
            expires_on: "2026-07-21",
            review_result: "approved",
        },
        QueryPlanException {
            table_size: 8,
            reason: "fixed table",
            approver: "sqlite-reviewer",
            expires_on: "2026-12-31",
            review_result: "",
        },
    ] {
        let expectation = requirement(
            AccessExpectation::Scan,
            None,
            &["version"],
            &[],
            Some(invalid),
        );
        let verdict = evaluate_sqlite_plan(
            &expectation,
            &[sqlite_row("SCAN schema_versions")],
            SYNTHETIC_REVIEW_DATE,
        );
        assert!(
            verdict.has_failure(PlanFailureKind::InvalidScanException),
            "{:?}",
            verdict.failures()
        );
    }
}

#[test]
fn mysql_ref_access_with_expected_key_and_columns_passes() {
    let expectation = requirement(
        AccessExpectation::Search,
        Some("idx_agents_identifier_category_id"),
        &["identifier", "category_id"],
        &["category_id"],
        None,
    );
    let explain = json!({
        "query_block": {
            "table": {
                "table_name": "agents",
                "access_type": "ref",
                "possible_keys": ["idx_agents_identifier_category_id"],
                "key": "idx_agents_identifier_category_id",
                "used_key_parts": ["identifier", "category_id"],
                "rows_examined_per_scan": 1
            }
        }
    });

    let verdict = evaluate_mysql_plan(&expectation, &explain, SYNTHETIC_REVIEW_DATE);

    assert!(verdict.is_accepted(), "{:?}", verdict.failures());
}

#[test]
fn mysql_full_scan_wrong_key_uncovered_column_and_unknown_shape_are_rejected() {
    let expectation = requirement(
        AccessExpectation::Search,
        Some("idx_agents_identifier_category_id"),
        &["identifier", "status"],
        &["category_id"],
        None,
    );

    let full_scan = json!({
        "query_block": {"table": {"table_name": "agents", "access_type": "ALL", "key": null}}
    });
    let verdict = evaluate_mysql_plan(&expectation, &full_scan, SYNTHETIC_REVIEW_DATE);
    assert!(verdict.has_failure(PlanFailureKind::UnapprovedFullScan));
    assert!(verdict.has_failure(PlanFailureKind::MissingExpectedIndex));

    let incomplete = json!({
        "query_block": {
            "table": {
                "table_name": "agents",
                "access_type": "ref",
                "key": "idx_agents_identifier_category_id",
                "used_key_parts": ["identifier"]
            }
        }
    });
    let verdict = evaluate_mysql_plan(&expectation, &incomplete, SYNTHETIC_REVIEW_DATE);
    assert!(verdict.has_failure(PlanFailureKind::UncoveredFilterColumn("status")));
    assert!(verdict.has_failure(PlanFailureKind::UncoveredJoinColumn("category_id")));

    let unknown = json!({"unexpected_future_mysql_shape": true});
    let verdict = evaluate_mysql_plan(&expectation, &unknown, SYNTHETIC_REVIEW_DATE);
    assert!(verdict.has_failure(PlanFailureKind::IndeterminatePlan));
}

#[tokio::test]
async fn every_active_foundation_sqlite_query_is_explained_against_real_v4_store() {
    let current_date = current_utc_date();
    let workspace = TestWorkspace::new().expect("isolated workspace");
    let store = Store::open_local(StoreOpenOptions::new(
        workspace.database_path(),
        workspace.plugin_root(),
    ))
    .await
    .expect("open real v4 Store");

    let active = production_query_catalog()
        .iter()
        .filter(|query| {
            query.active
                && query.owner_phase == OWNER_PHASE
                && query.dialect == QueryDialect::Sqlite
                && query.has_filter_or_join()
        })
        .collect::<Vec<_>>();
    assert!(!active.is_empty());

    for query in active {
        let plan = explain_catalog_query(&store, query)
            .await
            .unwrap_or_else(|error| panic!("EXPLAIN {}: {error}", query.id));
        let verdict = evaluate_sqlite_plan_set(query, &plan, &current_date);
        assert!(
            verdict.is_accepted(),
            "query {} failed its plan contract: {:?}",
            query.id,
            verdict.failures()
        );
    }
}

fn evaluate_sqlite_plan_set(
    query: &ProductionQuery,
    plan: &[SqlitePlanRow],
    review_date: &str,
) -> hivegui::datasource::query_plan::PlanVerdict {
    hivegui::datasource::query_plan::evaluate_sqlite_query(query, plan, review_date)
}

fn assert_complete_exception(
    query: &ProductionQuery,
    exception: &QueryPlanException,
    review_date: &str,
) {
    assert!(
        exception.table_size > 0,
        "{} exception lacks table size",
        query.id
    );
    assert!(
        !exception.reason.trim().is_empty(),
        "{} exception lacks reason",
        query.id
    );
    assert!(
        !exception.approver.trim().is_empty(),
        "{} exception lacks approver",
        query.id
    );
    assert!(
        !exception.expires_on.trim().is_empty(),
        "{} exception lacks expiry",
        query.id
    );
    assert!(
        exception.expires_on >= review_date,
        "{} scan exception expired on {}",
        query.id,
        exception.expires_on
    );
    assert!(
        !exception.review_result.trim().is_empty(),
        "{} exception lacks review result",
        query.id
    );
}

#[derive(Debug)]
struct SourceQueryPlanRegistration {
    id: String,
    owner_phase: String,
    activation_task: String,
    path: PathBuf,
    line: usize,
}

fn current_utc_date() -> String {
    Utc::now().format("%Y-%m-%d").to_string()
}

fn foundation_query_source_files(root: &Path) -> Vec<PathBuf> {
    // Later story query owners are deliberately absent here. Their Tests and
    // Reviewer tasks activate them without making T028 implement US2-US13.
    ["store.rs", "migrations.rs", "plugin_artifacts.rs"]
        .into_iter()
        .map(|name| root.join(name))
        .collect()
}

fn source_query_plan_registrations(paths: Vec<PathBuf>) -> Vec<SourceQueryPlanRegistration> {
    let mut registrations = Vec::new();
    let mut ids = BTreeSet::new();
    for path in paths {
        assert!(
            path.is_file(),
            "planned Foundation query owner is missing: {}",
            path.display()
        );
        let source = fs::read_to_string(&path).expect("read datasource source");
        let lines = source.lines().collect::<Vec<_>>();
        assert_filter_and_join_queries_are_registered(&path, &lines);

        for (index, line) in lines.iter().enumerate() {
            let Some(fields) = line.trim().strip_prefix("// query-plan:") else {
                continue;
            };
            let mut parsed = BTreeMap::new();
            for field in fields.split(';') {
                let (name, value) = field.trim().split_once('=').unwrap_or_else(|| {
                    panic!(
                        "invalid query-plan field at {}:{}",
                        path.display(),
                        index + 1
                    )
                });
                assert!(
                    matches!(name, "id" | "owner_phase" | "activation_task"),
                    "unknown query-plan field {name} at {}:{}",
                    path.display(),
                    index + 1
                );
                assert!(!value.trim().is_empty());
                assert!(
                    parsed.insert(name, value.trim()).is_none(),
                    "duplicate {name}"
                );
            }
            assert_eq!(
                parsed.keys().copied().collect::<BTreeSet<_>>(),
                BTreeSet::from(["activation_task", "id", "owner_phase"]),
                "query-plan registration at {}:{} must contain exactly id, owner_phase, activation_task",
                path.display(),
                index + 1
            );
            let id = parsed["id"].to_owned();
            assert!(
                ids.insert(id.clone()),
                "duplicate source query-plan id {id} at {}:{}",
                path.display(),
                index + 1
            );
            registrations.push(SourceQueryPlanRegistration {
                id,
                owner_phase: parsed["owner_phase"].to_owned(),
                activation_task: parsed["activation_task"].to_owned(),
                path: path.clone(),
                line: index + 1,
            });
        }
    }
    registrations
}

fn assert_filter_and_join_queries_are_registered(path: &Path, lines: &[&str]) {
    for (index, line) in lines.iter().enumerate() {
        let query_api_or_sql_literal = [
            "query!(",
            "query_as!(",
            "query_scalar!(",
            "sqlx::query(",
            "sqlx::query_as(",
            "sqlx::query_scalar(",
            "sqlx::query::<",
            "sqlx::query_as::<",
            "sqlx::query_scalar::<",
            "query::<",
            "query_as::<",
            "query_scalar::<",
            "QueryBuilder::new(",
            "QueryBuilder::<",
            "QueryBuilder::with_arguments(",
        ]
        .iter()
        .any(|marker| line.contains(marker))
            || {
                let uppercase = line.to_ascii_uppercase();
                ["SELECT ", "INSERT ", "UPDATE ", "DELETE "]
                    .iter()
                    .any(|keyword| uppercase.contains(keyword))
            };
        if !query_api_or_sql_literal {
            continue;
        }

        let search_end = (index + 80).min(lines.len());
        let query_end = lines[index..search_end]
            .iter()
            .position(|candidate| candidate.trim_end().ends_with(';'))
            .map(|offset| index + offset + 1)
            .unwrap_or(search_end);
        let query_window = lines[index..query_end].join(" ").to_ascii_uppercase();
        if !(query_window.contains(" WHERE ") || query_window.contains(" JOIN ")) {
            continue;
        }

        let annotation_start = index.saturating_sub(5);
        let registered = lines[annotation_start..index]
            .iter()
            .any(|candidate| candidate.trim().starts_with("// query-plan:"));
        assert!(
            registered,
            "filtered/joined production query at {}:{} has no stable query-plan registration",
            path.display(),
            index + 1
        );
    }
}

// ---------------------------------------------------------------------------
// §T017G — FTS5 VIRTUAL TABLE INDEX recognition and short-gram coverage.
// ---------------------------------------------------------------------------

fn fts_expectation() -> FtsPlanExpectation {
    FtsPlanExpectation {
        query_id: "t017g.search.fts_trigram",
        owner_phase: "security-remediation",
        activation_task: "T017G",
        table: "search_index",
        backend: hivegui::datasource::query_plan::FtsBackend::Fts5Trigram,
        filter_columns: &["term"],
        scan_exception: None,
    }
}

fn fts_short_gram_expectation() -> FtsPlanExpectation {
    FtsPlanExpectation {
        query_id: "t017g.search.short_gram",
        owner_phase: "security-remediation",
        activation_task: "T017G",
        table: "short_gram_index",
        backend: hivegui::datasource::query_plan::FtsBackend::ShortGram,
        filter_columns: &["term"],
        scan_exception: None,
    }
}

#[test]
fn fts5_virtual_table_index_is_recognised_as_index_access_not_scan() {
    let plan = [
        json!({
            "id": 0,
            "parent": 0,
            "notused": 0,
            "detail": "SCAN CONSTANT ROW"
        }),
        json!({
            "id": 1,
            "parent": 0,
            "notused": 0,
            "detail": "SEARCH search_index USING VIRTUAL TABLE INDEX 1 (term=?)"
        }),
    ];
    let verdict: FtsPlanVerdict =
        evaluate_fts_plan(&fts_expectation(), &plan, "2026-07-30").expect("verdict");
    assert!(
        verdict.is_accepted(),
        "FTS5 VIRTUAL TABLE INDEX must be accepted as index access; got {:?}",
        verdict.failures()
    );
}

#[test]
fn fts5_table_scan_is_rejected_as_unapproved_full_scan() {
    let plan = [json!({
        "id": 1,
        "parent": 0,
        "notused": 0,
        "detail": "SCAN search_index"
    })];
    let verdict: FtsPlanVerdict =
        evaluate_fts_plan(&fts_expectation(), &plan, "2026-07-30").expect("verdict");
    assert!(
        verdict.has_failure(PlanFailureKind::UnapprovedFullScan),
        "FTS5 SCAN must fail; got {:?}",
        verdict.failures()
    );
}

#[test]
fn short_gram_index_scan_is_rejected_as_unapproved_full_scan() {
    let plan = [json!({
        "id": 1,
        "parent": 0,
        "notused": 0,
        "detail": "SCAN short_gram_index"
    })];
    let verdict: FtsPlanVerdict =
        evaluate_fts_plan(&fts_short_gram_expectation(), &plan, "2026-07-30").expect("verdict");
    assert!(
        verdict.has_failure(PlanFailureKind::UnapprovedFullScan),
        "short-gram SCAN must fail; got {:?}",
        verdict.failures()
    );
}

#[test]
fn fts_query_with_uncovered_filter_column_is_rejected() {
    let plan = [json!({
        "id": 1,
        "parent": 0,
        "notused": 0,
        "detail": "SEARCH search_index USING VIRTUAL TABLE INDEX 1 (other=?)"
    })];
    let verdict: FtsPlanVerdict =
        evaluate_fts_plan(&fts_expectation(), &plan, "2026-07-30").expect("verdict");
    assert!(
        verdict.has_failure(PlanFailureKind::UncoveredFilterColumn("term")),
        "an FTS plan that does not use the term column must fail; got {:?}",
        verdict.failures()
    );
}
