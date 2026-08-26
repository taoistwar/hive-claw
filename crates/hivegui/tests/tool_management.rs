//! T101 [P] [US11] Tool management supplemental Red contract.
//!
//! The historical four-test in-memory facade did not exercise the canonical
//! v4 database or the FR-020 Tool shape. This batch makes the production
//! boundary explicit before T104 review: one SQL-backed async Store, complete
//! fields, stable enums, target XOR/schema/Capability validation, safe
//! conflicts, fixed paging/indexed search, and the two T005 management targets.

mod support;

use std::{collections::BTreeSet, path::Path};

use chrono::Utc;
use hivegui::datasource::{
    query_plan::{QueryDialect, production_query_catalog},
    store::{Store, StoreOpenOptions},
    tool_store::{ToolInput, ToolKind, ToolPage, ToolRecord, ToolSource, ToolStore},
    validation::PublicErrorEnvelope,
};
use support::{
    TestWorkspace,
    performance::{
        ComparisonOutcome, EnvironmentFingerprint, MEASURED_SAMPLES, TOOL_CRUD_ID,
        TOOL_FIXTURE_ROWS, TOOL_SEARCH_PAGE_ID, TOOL_SEARCH_PAGE_SCHEDULE, baseline_path,
        evaluate_benchmark_gate, regression_exception_path, run_target, source_revision,
        target_specs,
    },
};

const INPUT_SCHEMA: &str =
    r#"{"type":"object","properties":{"value":{"type":"string"}},"required":["value"]}"#;
const OUTPUT_SCHEMA: &str =
    r#"{"type":"object","properties":{"result":{"type":"string"}},"required":["result"]}"#;
const FIXTURE_TIME: &str = "2026-08-25T00:00:00Z";

async fn open_tool_store(workspace: &TestWorkspace) -> (Store, ToolStore) {
    let database = Store::open_local(StoreOpenOptions::new(
        workspace.database_path(),
        workspace.plugin_root(),
    ))
    .await
    .expect("open canonical v4 Store");
    let tools = ToolStore::new(database.pool().clone()).expect("Tool Store");
    (database, tools)
}

async fn seed_function(database: &Store, identifier: &str) -> i64 {
    sqlx::query_scalar::<_, i64>(
        "INSERT INTO functions (identifier, name, description, kind, input_schema, \
         output_schema, plugin_id, plugin_export, category_id, required_capabilities, \
         created_at, updated_at) \
         VALUES (?, ?, '', 'builtin', ?, ?, NULL, NULL, NULL, NULL, ?, ?) RETURNING id",
    )
    .bind(identifier)
    .bind(identifier)
    .bind(INPUT_SCHEMA)
    .bind(OUTPUT_SCHEMA)
    .bind(FIXTURE_TIME)
    .bind(FIXTURE_TIME)
    .fetch_one(database.pool())
    .await
    .expect("seed Function target")
}

async fn seed_workflow(database: &Store, identifier: &str) -> i64 {
    sqlx::query_scalar::<_, i64>(
        "INSERT INTO workflows (identifier, name, description, timeout_ms, input_schema, \
         start_description, output_schema, required_capabilities, created_at, updated_at) \
         VALUES (?, ?, '', 30000, ?, '', ?, '[\"log.emit\"]', ?, ?) RETURNING id",
    )
    .bind(identifier)
    .bind(identifier)
    .bind(INPUT_SCHEMA)
    .bind(OUTPUT_SCHEMA)
    .bind(FIXTURE_TIME)
    .bind(FIXTURE_TIME)
    .fetch_one(database.pool())
    .await
    .expect("seed Workflow target")
}

#[allow(clippy::too_many_arguments)]
fn input(
    identifier: &str,
    name: &str,
    kind: ToolKind,
    source: ToolSource,
    is_always: bool,
    function_id: Option<i64>,
    workflow_id: Option<i64>,
    input_schema: &str,
    output_schema: &str,
    required_capabilities: Option<&str>,
) -> Result<ToolInput, hivegui::datasource::validation::PublicBoundaryError> {
    ToolInput::for_write(
        identifier.to_string(),
        name.to_string(),
        "Tool fixture description".to_string(),
        kind,
        source,
        is_always,
        function_id,
        workflow_id,
        input_schema.to_string(),
        output_schema.to_string(),
        None,
        required_capabilities.map(str::to_string),
    )
}

fn assert_full_record(
    record: &ToolRecord,
    identifier: &str,
    kind: ToolKind,
    source: ToolSource,
    is_always: bool,
    targets: (Option<i64>, Option<i64>),
    capabilities: Option<&str>,
) {
    let (function_id, workflow_id) = targets;
    assert_eq!(record.identifier(), identifier);
    assert_eq!(record.name(), format!("{identifier} name"));
    assert_eq!(record.description(), "Tool fixture description");
    assert_eq!(record.kind(), kind);
    assert_eq!(record.source(), source);
    assert_eq!(record.is_always(), is_always);
    assert_eq!(record.function_id(), function_id);
    assert_eq!(record.workflow_id(), workflow_id);
    assert_eq!(record.input_schema(), INPUT_SCHEMA);
    assert_eq!(record.output_schema(), OUTPUT_SCHEMA);
    assert_eq!(record.category_id(), None);
    assert_eq!(record.required_capabilities(), capabilities);
}

#[test]
fn tool_kind_and_source_enumerate_only_stable_wire_values() {
    assert_eq!(ToolKind::FunctionWrap.as_str(), "function-wrap");
    assert_eq!(ToolKind::WorkflowWrap.as_str(), "workflow-wrap");
    assert_eq!(ToolSource::Workspace.as_str(), "workspace");
    assert_eq!(ToolSource::Builtin.as_str(), "builtin");
    assert_eq!(
        ToolKind::try_from("function-wrap").unwrap(),
        ToolKind::FunctionWrap
    );
    assert_eq!(
        ToolKind::try_from("workflow-wrap").unwrap(),
        ToolKind::WorkflowWrap
    );
    assert!(ToolKind::try_from("1").is_err());
    assert!(ToolKind::try_from("custom").is_err());
    assert_eq!(
        ToolSource::try_from("workspace").unwrap(),
        ToolSource::Workspace
    );
    assert_eq!(
        ToolSource::try_from("builtin").unwrap(),
        ToolSource::Builtin
    );
    assert!(ToolSource::try_from("remote").is_err());
}

#[tokio::test(flavor = "current_thread")]
async fn tool_store_writes_the_supplied_canonical_v4_pool() {
    let workspace = TestWorkspace::new().expect("test workspace");
    let (database, tools) = open_tool_store(&workspace).await;
    let function_id = seed_function(&database, "pool_owned_function").await;

    tools
        .create(
            input(
                "pool_owned_tool",
                "pool_owned_tool name",
                ToolKind::FunctionWrap,
                ToolSource::Workspace,
                false,
                Some(function_id),
                None,
                INPUT_SCHEMA,
                OUTPUT_SCHEMA,
                None,
            )
            .expect("valid Tool input"),
        )
        .await
        .expect("create Tool through public Store boundary");

    let persisted = sqlx::query_scalar::<_, i64>(
        "SELECT COUNT(*) FROM tools WHERE identifier = 'pool_owned_tool'",
    )
    .fetch_one(database.pool())
    .await
    .expect("read canonical tools table");
    assert_eq!(
        persisted, 1,
        "ToolStore must own the supplied v4 SQL pool instead of an in-memory Vec"
    );
}

#[tokio::test(flavor = "current_thread")]
async fn function_and_workflow_tools_roundtrip_every_fr020_field() {
    let workspace = TestWorkspace::new().expect("test workspace");
    let (database, tools) = open_tool_store(&workspace).await;
    let function_id = seed_function(&database, "tool_roundtrip_function").await;
    let workflow_id = seed_workflow(&database, "tool_roundtrip_workflow").await;

    let function = tools
        .create(
            input(
                "function_roundtrip",
                "function_roundtrip name",
                ToolKind::FunctionWrap,
                ToolSource::Workspace,
                true,
                Some(function_id),
                None,
                INPUT_SCHEMA,
                OUTPUT_SCHEMA,
                Some(r#"["network.http","log.emit"]"#),
            )
            .expect("function Tool input"),
        )
        .await
        .expect("create function Tool");
    let workflow = tools
        .create(
            input(
                "workflow_roundtrip",
                "workflow_roundtrip name",
                ToolKind::WorkflowWrap,
                ToolSource::Builtin,
                false,
                None,
                Some(workflow_id),
                INPUT_SCHEMA,
                OUTPUT_SCHEMA,
                Some(r#"["log.emit","network.http"]"#),
            )
            .expect("workflow Tool input"),
        )
        .await
        .expect("create workflow Tool");

    let function = tools
        .get(function.id())
        .await
        .expect("get function Tool")
        .expect("function Tool exists");
    let workflow = tools
        .get(workflow.id())
        .await
        .expect("get workflow Tool")
        .expect("workflow Tool exists");
    assert_full_record(
        &function,
        "function_roundtrip",
        ToolKind::FunctionWrap,
        ToolSource::Workspace,
        true,
        (Some(function_id), None),
        Some(r#"["network.http","log.emit"]"#),
    );
    assert_full_record(
        &workflow,
        "workflow_roundtrip",
        ToolKind::WorkflowWrap,
        ToolSource::Builtin,
        false,
        (None, Some(workflow_id)),
        Some(r#"["log.emit","network.http"]"#),
    );
}

#[tokio::test(flavor = "current_thread")]
async fn target_schema_and_capability_validation_is_fail_closed_and_zero_modification() {
    let workspace = TestWorkspace::new().expect("test workspace");
    let (database, tools) = open_tool_store(&workspace).await;
    let function_id = seed_function(&database, "validation_function").await;
    let workflow_id = seed_workflow(&database, "validation_workflow").await;
    let before = sqlx::query_scalar::<_, i64>("SELECT COUNT(*) FROM tools")
        .fetch_one(database.pool())
        .await
        .expect("count before invalid writes");

    let invalid = [
        input(
            "both_targets",
            "both_targets name",
            ToolKind::FunctionWrap,
            ToolSource::Workspace,
            false,
            Some(function_id),
            Some(workflow_id),
            INPUT_SCHEMA,
            OUTPUT_SCHEMA,
            None,
        ),
        input(
            "wrong_schema",
            "wrong_schema name",
            ToolKind::FunctionWrap,
            ToolSource::Workspace,
            false,
            Some(function_id),
            None,
            r#"{"type":"array"}"#,
            OUTPUT_SCHEMA,
            None,
        ),
        input(
            "duplicate_capability",
            "duplicate_capability name",
            ToolKind::FunctionWrap,
            ToolSource::Workspace,
            false,
            Some(function_id),
            None,
            INPUT_SCHEMA,
            OUTPUT_SCHEMA,
            Some(r#"[" network.http ","network.http"]"#),
        ),
        input(
            "unknown_capability",
            "unknown_capability name",
            ToolKind::FunctionWrap,
            ToolSource::Workspace,
            false,
            Some(function_id),
            None,
            INPUT_SCHEMA,
            OUTPUT_SCHEMA,
            Some(r#"["unknown.capability"]"#),
        ),
    ];

    let mut accepted = Vec::new();
    for candidate in invalid {
        match candidate {
            Err(_) => {}
            Ok(candidate) => {
                if let Ok(record) = tools.create(candidate).await {
                    accepted.push(record.identifier().to_string());
                }
            }
        }
    }
    let after = sqlx::query_scalar::<_, i64>("SELECT COUNT(*) FROM tools")
        .fetch_one(database.pool())
        .await
        .expect("count after invalid writes");
    assert!(
        accepted.is_empty(),
        "invalid Tool writes were accepted: {accepted:?}"
    );
    assert_eq!(after, before, "invalid Tool writes must modify zero rows");
}

#[tokio::test(flavor = "current_thread")]
async fn duplicate_identifier_is_a_safe_typed_conflict_and_preserves_the_first_row() {
    let workspace = TestWorkspace::new().expect("test workspace");
    let (database, tools) = open_tool_store(&workspace).await;
    let function_id = seed_function(&database, "duplicate_function").await;
    let first = input(
        "safe_tool_duplicate",
        "safe_tool_duplicate name",
        ToolKind::FunctionWrap,
        ToolSource::Workspace,
        false,
        Some(function_id),
        None,
        INPUT_SCHEMA,
        OUTPUT_SCHEMA,
        None,
    )
    .expect("valid first Tool");
    tools
        .create(first.clone())
        .await
        .expect("create first Tool");

    let error = tools
        .create(first)
        .await
        .expect_err("duplicate identifier must fail");
    assert!(matches!(
        error.envelope(),
        PublicErrorEnvelope::Conflict { shape, field, reason }
            if shape == "value" && field == "identifier" && reason == "duplicate"
    ));
    assert_eq!(error.value(), Some("safe_tool_duplicate"));
    assert!(error.references().is_empty());
    assert_eq!(
        sqlx::query_scalar::<_, i64>(
            "SELECT COUNT(*) FROM tools WHERE identifier = 'safe_tool_duplicate'",
        )
        .fetch_one(database.pool())
        .await
        .expect("count duplicate Tool"),
        1
    );
}

#[tokio::test(flavor = "current_thread")]
async fn fixed_hundred_plus_fixture_pages_and_searches_literal_text() {
    let workspace = TestWorkspace::new().expect("test workspace");
    let (database, tools) = open_tool_store(&workspace).await;
    let function_id = seed_function(&database, "paging_function").await;
    const { assert!(TOOL_FIXTURE_ROWS >= 100) };

    for index in 0..TOOL_FIXTURE_ROWS {
        let identifier = if index == 42 {
            "literal_under_score".to_string()
        } else {
            format!("tool-{index:04}")
        };
        tools
            .create(
                input(
                    &identifier,
                    &format!("{identifier} name"),
                    ToolKind::FunctionWrap,
                    ToolSource::Workspace,
                    index % 7 == 0,
                    Some(function_id),
                    None,
                    INPUT_SCHEMA,
                    OUTPUT_SCHEMA,
                    None,
                )
                .expect("valid fixture Tool"),
            )
            .await
            .expect("create fixture Tool");
    }

    let first: ToolPage = tools.list(None, 1).await.expect("first page");
    let second = tools.list(None, 2).await.expect("second page");
    assert_eq!(first.page(), 1);
    assert_eq!(first.page_size(), 20);
    assert_eq!(first.items().len(), 20);
    assert_eq!(first.total(), TOOL_FIXTURE_ROWS as i64);
    assert_eq!(second.items().len(), 20);
    let first_ids = first
        .items()
        .iter()
        .map(ToolRecord::id)
        .collect::<BTreeSet<_>>();
    let second_ids = second
        .items()
        .iter()
        .map(ToolRecord::id)
        .collect::<BTreeSet<_>>();
    assert!(first_ids.is_disjoint(&second_ids));

    let literal = tools
        .list(Some("_".to_string()), 1)
        .await
        .expect("literal underscore search");
    assert_eq!(
        literal.total(),
        1,
        "underscore must not behave as SQL LIKE wildcard"
    );
    assert_eq!(literal.items()[0].identifier(), "literal_under_score");
}

#[test]
fn tool_query_catalog_owns_every_story_filter_and_association_route() {
    let activated = production_query_catalog()
        .iter()
        .filter(|query| {
            query.active
                && query.owner_phase == "US11"
                && query.activation_task == "T105"
                && query.dialect == QueryDialect::Sqlite
        })
        .collect::<Vec<_>>();
    let required = [
        ("search_documents_fts", &[][..]),
        ("search_short_grams", &["gram_len", "gram"][..]),
        ("search_documents", &["entity_type", "field"][..]),
        ("tools", &["id"][..]),
        ("functions", &["id"][..]),
        ("workflows", &["id"][..]),
        ("capabilities", &["name"][..]),
    ];

    for (table, columns) in required {
        assert!(
            activated.iter().any(|query| {
                query.requirements.iter().any(|requirement| {
                    requirement.table == table
                        && columns.iter().all(|column| {
                            requirement.filter_columns.contains(column)
                                || requirement.join_columns.contains(column)
                        })
                })
            }),
            "active US11/T105 catalog lacks exact production SQL ownership for {table} {columns:?}; activated={:?}",
            activated.iter().map(|query| query.id).collect::<Vec<_>>()
        );
    }
}

#[test]
fn tool_management_uses_exactly_two_canonical_t005_targets() {
    let owned = target_specs()
        .into_iter()
        .filter(|target| target.owner_task == "T101")
        .collect::<Vec<_>>();
    let ids = owned
        .iter()
        .map(|target| target.id.as_str())
        .collect::<BTreeSet<_>>();
    assert_eq!(ids, BTreeSet::from([TOOL_CRUD_ID, TOOL_SEARCH_PAGE_ID]));
    assert_eq!(owned.len(), 2);
    assert_eq!(TOOL_FIXTURE_ROWS, 10_000);
    assert_eq!(TOOL_SEARCH_PAGE_SCHEDULE.len(), 100);
    assert_eq!(owned[0].measured_samples, 100);
    assert_eq!(owned[1].measured_samples, 100);
    for target in owned {
        assert!(
            match target.id.as_str() {
                TOOL_CRUD_ID => target.p95_budget_ns == 1_000_000_000,
                TOOL_SEARCH_PAGE_ID => target.p95_budget_ns == 500_000_000,
                _ => false,
            },
            "unexpected Tool management target: {target:?}"
        );
    }
}

#[cfg_attr(
    debug_assertions,
    ignore = "release-only T005 benchmark gate; debug has a distinct environment fingerprint"
)]
#[tokio::test(flavor = "current_thread")]
async fn tool_performance_runner_meets_budgets_and_approved_baselines() {
    let current_source_revision = source_revision(Path::new(env!("CARGO_MANIFEST_DIR")))
        .expect("capture repository-anchored benchmark source revision");
    let as_of = Utc::now().date_naive();
    let environment = EnvironmentFingerprint::capture();

    for target in target_specs()
        .into_iter()
        .filter(|target| target.id == TOOL_CRUD_ID || target.id == TOOL_SEARCH_PAGE_ID)
    {
        let report = run_target(&target, &environment)
            .await
            .unwrap_or_else(|error| panic!("run canonical {} benchmark: {error}", target.id))
            .unwrap_or_else(|| {
                panic!(
                    "canonical T005 runner must own the {} operation and fixed typed-Store fixture",
                    target.id
                )
            });
        assert_eq!(report.target, target);
        assert_eq!(report.environment, environment);
        assert_eq!(report.sample_count, MEASURED_SAMPLES);

        let baseline_path = baseline_path(&target, &environment);
        let exception_path = regression_exception_path(&target, &environment);
        let evaluation = evaluate_benchmark_gate(
            &report,
            &current_source_revision,
            &baseline_path,
            &exception_path,
            as_of,
        )
        .unwrap_or_else(|error| {
            panic!(
                "{} performance gate evaluation failed: baseline_path={}, exception_path={}, evaluation=Err({error})",
                target.id,
                baseline_path.display(),
                exception_path.display(),
            )
        });
        assert!(
            matches!(
                evaluation.outcome,
                ComparisonOutcome::Passed | ComparisonOutcome::ApprovedException
            ),
            "{} performance gate requires Passed or ApprovedException: baseline_path={}, exception_path={}, evaluation={evaluation:?}",
            target.id,
            baseline_path.display(),
            exception_path.display(),
        );
    }
}
