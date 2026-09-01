//! T115 [P] [US13] Agent management contract.
//!
//! Source of truth: `specs/011-hivegui-standalone-mode/tasks.md` §T115
//! ("在 `crates/hivegui/tests/agent_management.rs` 使用固定 100+
//! Agent fixture 编写唯一默认根、首个自动默认、替代默认删除、自动
//! depth、循环/depth>10、非空 model_preset 必须存在及不存在时
//! 字段级 invalid_input/零修改、三类关联批量加载/查询计数、
//! 每页20条搜索分页、全部生产过滤/关联查询 EXPLAIN、固定100次
//! CRUD p95≤1s、搜索/翻页 p95≤500ms 及 T005 基线比较测试").
//!
//! Public boundaries the T124 implementation must satisfy:
//!   - `hivegui::datasource::entity_store::AgentStore`
//!   - `hivegui::datasource::entity_store::AgentInput`
//!   - `hivegui::datasource::entity_store::AgentRecord`
//!   - `hivegui::datasource::entity_store::AgentFilter`
//!   - `hivegui::datasource::entity_store::AgentPage`
//!
//! T123 reviewer signs §T115.11; T124 implementation then makes
//! these tests Green; T136 reruns to record the Green evidence.

#![allow(missing_docs)]

mod support;

use std::{collections::BTreeSet, path::Path};

use chrono::Utc;
use hivegui::datasource::{
    entity_store::{AgentFilter, AgentInput, AgentPage, AgentRecord, AgentStore},
    query_count::{QueryCountObserver, evaluate_query_count, production_query_count_catalog},
    query_plan::{QueryDialect, SqlitePlanRow, evaluate_sqlite_query, production_query_catalog},
    store::{Store, StoreOpenOptions},
};
use sqlx::{
    AssertSqlSafe, Connection, Row,
    sqlite::{SqliteConnectOptions, SqlitePoolOptions},
};
use support::{
    TestWorkspace,
    performance::{
        AGENT_CRUD_ID, AGENT_FIXTURE_ROWS, AGENT_SEARCH_PAGE_ID, AGENT_SEARCH_PAGE_SCHEDULE,
        ComparisonOutcome, EnvironmentFingerprint, MEASURED_SAMPLES, baseline_path,
        evaluate_benchmark_gate, regression_exception_path, run_target, source_revision,
        target_specs,
    },
};

const AGENT_FIXTURE_SIZE: usize = 100;
const PAGE_SIZE: usize = 20;

async fn fresh_store() -> (TestWorkspace, AgentStore) {
    let workspace = TestWorkspace::new().expect("test workspace");
    let database = Store::open_local(StoreOpenOptions::new(
        workspace.database_path(),
        workspace.plugin_root(),
    ))
    .await
    .expect("open canonical v4 Store");
    let store = AgentStore::from_store(&database).expect("Agent Store");
    (workspace, store)
}

async fn open_observed_store(
    workspace: &TestWorkspace,
    observer: QueryCountObserver,
) -> (Store, AgentStore) {
    let database = Store::open_local(
        StoreOpenOptions::new(workspace.database_path(), workspace.plugin_root())
            .with_query_count_observer(observer),
    )
    .await
    .expect("open canonical v4 Store");
    let agents = AgentStore::from_store(&database).expect("observed Agent Store");
    (database, agents)
}

async fn seed_resource_fixture(database: &Store) -> (i64, i64, String) {
    const FIXTURE_TIME: &str = "2026-08-25T00:00:00Z";
    sqlx::query(
        "INSERT OR IGNORE INTO capabilities (name, description, is_dangerous, category_id, created_at) \
         VALUES ('log.emit', 'Agent fixture capability', 0, NULL, ?)",
    )
    .bind(FIXTURE_TIME)
    .execute(database.pool())
    .await
    .expect("seed Capability");
    let function_id = sqlx::query_scalar::<_, i64>(
        "INSERT INTO functions (identifier, name, description, kind, input_schema, \
         output_schema, plugin_id, plugin_export, category_id, required_capabilities, \
         created_at, updated_at) \
         VALUES ('agent_fixture_function', 'Agent fixture Function', '', 'builtin', \
         '{}', '{}', NULL, NULL, NULL, NULL, ?, ?) RETURNING id",
    )
    .bind(FIXTURE_TIME)
    .bind(FIXTURE_TIME)
    .fetch_one(database.pool())
    .await
    .expect("seed Function");
    let tool_id = sqlx::query_scalar::<_, i64>(
        "INSERT INTO tools (identifier, name, description, kind, source, is_always, \
         function_id, workflow_id, input_schema, output_schema, category_id, \
         required_capabilities, created_at, updated_at) \
         VALUES ('agent_fixture_tool', 'Agent fixture Tool', '', 'function-wrap', \
         'workspace', 0, ?, NULL, '{}', '{}', NULL, '[\"log.emit\"]', ?, ?) \
         RETURNING id",
    )
    .bind(function_id)
    .bind(FIXTURE_TIME)
    .bind(FIXTURE_TIME)
    .fetch_one(database.pool())
    .await
    .expect("seed Tool");
    let skill_id = sqlx::query_scalar::<_, i64>(
        "INSERT INTO skills (identifier, name, description, frontmatter, content, source, \
         is_always, category_id, required_capabilities, created_at, updated_at) \
         VALUES ('agent_fixture_skill', 'Agent fixture Skill', '', NULL, \
         'fixture skill', 'workspace', 0, NULL, '[\"log.emit\"]', ?, ?) RETURNING id",
    )
    .bind(FIXTURE_TIME)
    .bind(FIXTURE_TIME)
    .fetch_one(database.pool())
    .await
    .expect("seed Skill");
    (tool_id, skill_id, "log.emit".to_string())
}

fn root_input(identifier: &str, name: &str) -> AgentInput {
    AgentInput::new_root(identifier, name, "system prompt").expect("validated root")
}

#[tokio::test(flavor = "current_thread")]
async fn first_agent_is_automatically_default() {
    let (_workspace, store) = fresh_store().await;

    let first = store
        .create(root_input("first", "First Agent"))
        .await
        .expect("create first");
    assert!(
        first.is_default(),
        "first Agent must be auto-default per T115"
    );

    let second = store
        .create(root_input("second", "Second Agent"))
        .await
        .expect("create second");
    assert!(!second.is_default(), "second Agent must not be default");

    let default = store
        .default_agent()
        .await
        .expect("default fetch")
        .expect("default exists");
    assert_eq!(default.identifier(), "first");
}

#[tokio::test(flavor = "current_thread")]
async fn default_replacement_is_atomic_and_keeps_invariant() {
    let (_workspace, store) = fresh_store().await;

    let a = store
        .create(root_input("a", "Agent A"))
        .await
        .expect("create a");
    let b = store
        .create(root_input("b", "Agent B"))
        .await
        .expect("create b");

    let switched = store
        .set_default(b.id())
        .await
        .expect("set_default must succeed for root Agent");
    assert!(switched.is_default());
    assert_eq!(switched.identifier(), "b");

    // Original default must have been atomically cleared.
    let a_reloaded = store
        .fetch_one(a.id())
        .await
        .expect("fetch a")
        .expect("a exists");
    assert!(!a_reloaded.is_default());
}

#[tokio::test(flavor = "current_thread")]
async fn parent_agent_id_computes_depth_automatically() {
    let (_workspace, store) = fresh_store().await;

    let parent = store
        .create(root_input("parent", "Parent"))
        .await
        .expect("create parent");
    let child = store
        .create(root_input("child", "Child").with_parent(Some(parent.id())))
        .await
        .expect("create child");
    assert_eq!(child.depth(), 1, "child must inherit parent depth + 1");
    assert_eq!(child.parent_agent_id(), Some(parent.id()));
}

#[tokio::test(flavor = "current_thread")]
async fn non_default_root_cannot_become_default() {
    let (_workspace, store) = fresh_store().await;

    let parent = store
        .create(root_input("parent", "Parent"))
        .await
        .expect("create parent");
    let child = store
        .create(root_input("child", "Child").with_parent(Some(parent.id())))
        .await
        .expect("create child");

    let err = store
        .set_default(child.id())
        .await
        .expect_err("child must not be allowed as default");
    assert_eq!(err.field(), "id");
    assert_eq!(err.reason(), "must_be_root");
}

#[tokio::test(flavor = "current_thread")]
async fn list_children_returns_only_direct_children() {
    let (_workspace, store) = fresh_store().await;

    let parent = store
        .create(root_input("parent", "Parent"))
        .await
        .expect("create parent");
    let child = store
        .create(root_input("child", "Child").with_parent(Some(parent.id())))
        .await
        .expect("create child");
    let grandchild = store
        .create(root_input("grand", "Grandchild").with_parent(Some(child.id())))
        .await
        .expect("create grandchild");

    let direct = store
        .list_children(parent.id())
        .await
        .expect("list children");
    assert_eq!(direct.len(), 1);
    assert_eq!(direct[0].identifier(), "child");

    let grand = store
        .list_children(child.id())
        .await
        .expect("list grandchildren");
    assert_eq!(grand.len(), 1);
    assert_eq!(grand[0].identifier(), "grand");
    let _ = grandchild;
}

#[tokio::test(flavor = "current_thread")]
async fn empty_identifier_is_rejected_with_field_level_error() {
    // AgentInput::new_root enforces the field-level contract at
    // the public input boundary; the store layer never sees an
    // invalid value.
    let err = AgentInput::new_root("", "Empty", "system prompt")
        .expect_err("empty identifier must fail at the input layer");
    assert_eq!(err.field(), "identifier");
    assert_eq!(err.reason(), "empty");

    let err = AgentInput::new_root("ok", "Ok", "")
        .expect_err("empty system prompt must fail at the input layer");
    assert_eq!(err.field(), "system_prompt");
    assert_eq!(err.reason(), "empty");
}

#[tokio::test(flavor = "current_thread")]
async fn invalid_charset_identifier_is_rejected() {
    let (_workspace, store) = fresh_store().await;

    let err = AgentInput::new_root("invalid name with spaces", "Bad", "prompt")
        .expect_err("invalid identifier must fail at input layer");
    assert_eq!(err.field(), "identifier");
    assert_eq!(err.reason(), "invalid_charset");
    let _ = store;
}

#[tokio::test(flavor = "current_thread")]
async fn duplicate_identifier_returns_conflict() {
    let (_workspace, store) = fresh_store().await;

    store
        .create(root_input("dup", "First"))
        .await
        .expect("first create");
    let err = store
        .create(root_input("dup", "Second"))
        .await
        .expect_err("second create must fail");
    assert_eq!(err.field(), "identifier");
    assert_eq!(err.reason(), "duplicate");
}

#[tokio::test(flavor = "current_thread")]
async fn search_filter_pages_records_in_groups_of_twenty() {
    let (_workspace, store) = fresh_store().await;

    for i in 0..AGENT_FIXTURE_SIZE {
        store
            .create(root_input(&format!("agent-{i:03}"), &format!("Agent {i}")))
            .await
            .expect("create");
    }

    let first = store
        .search(&AgentFilter::first().with_page(1))
        .await
        .expect("page 1");
    let second = store
        .search(&AgentFilter::first().with_page(2))
        .await
        .expect("page 2");
    let sixth = store
        .search(&AgentFilter::first().with_page(6))
        .await
        .expect("page 6");

    assert_eq!(first.records().len(), PAGE_SIZE);
    assert_eq!(second.records().len(), PAGE_SIZE);
    assert_eq!(
        sixth.records().len(),
        0,
        "100 records / 20 = 5 pages; page 6 must be empty"
    );
    assert_eq!(first.total(), AGENT_FIXTURE_SIZE as i64);
}

#[tokio::test(flavor = "current_thread")]
async fn search_by_normalized_term_returns_matching_agents() {
    let (_workspace, store) = fresh_store().await;

    store
        .create(root_input("alpha", "Alpha Searcher"))
        .await
        .expect("create alpha");
    store
        .create(root_input("beta", "Beta Helper"))
        .await
        .expect("create beta");

    let page = store
        .search(&AgentFilter::first().with_search("ALPHA"))
        .await
        .expect("search");
    assert_eq!(page.total(), 1, "search must use the normalized name index");
    assert_eq!(page.records()[0].identifier(), "alpha");
}

#[tokio::test(flavor = "current_thread")]
async fn search_page_hydration_uses_five_queries_independent_of_page_cardinality() {
    let workspace = TestWorkspace::new().expect("test workspace");
    let observer = QueryCountObserver::new();
    let (database, agents) = open_observed_store(&workspace, observer.clone()).await;
    let (tool_id, skill_id, capability) = seed_resource_fixture(&database).await;
    for index in 0..PAGE_SIZE {
        let name = if index == 0 {
            "Unique needle"
        } else {
            "Ordinary fixture"
        };
        agents
            .create(
                root_input(&format!("search-batch-{index:02}"), name)
                    .with_tools([tool_id])
                    .with_skills([skill_id])
                    .with_capabilities([capability.clone()]),
            )
            .await
            .expect("seed Agent search page");
    }

    let contract = production_query_count_catalog()
        .iter()
        .find(|contract| contract.id == "agent.search_page")
        .expect("Agent search-page query-count contract");
    assert_eq!(contract.owner_phase, "US13");
    assert_eq!(contract.activation_task, "T115");
    assert!(contract.active);
    assert_eq!(contract.maximum_queries, 5);

    let small_scope = observer.start_scope(contract.id, 1);
    let small_page = agents
        .search(&AgentFilter::first().with_search("needle"))
        .await
        .expect("search one Agent");
    let small = small_scope.finish();

    let large_scope = observer.start_scope(contract.id, PAGE_SIZE);
    let large_page = agents
        .search(&AgentFilter::first())
        .await
        .expect("search full Agent page");
    let large = large_scope.finish();

    assert_eq!(small_page.records().len(), 1);
    assert_eq!(large_page.records().len(), PAGE_SIZE);
    assert_eq!(
        large_page
            .records()
            .iter()
            .map(AgentRecord::identifier)
            .collect::<Vec<_>>(),
        (1..PAGE_SIZE)
            .chain(std::iter::once(0))
            .map(|index| format!("search-batch-{index:02}"))
            .collect::<Vec<_>>(),
        "batch hydration must preserve the search query's stable ordering"
    );
    for record in large_page.records() {
        assert_eq!(record.tool_ids(), [tool_id]);
        assert_eq!(record.skill_ids(), [skill_id]);
        assert_eq!(record.capability_names(), [capability.as_str()]);
    }
    assert_eq!(
        (small.total_queries, large.total_queries),
        (contract.maximum_queries, contract.maximum_queries),
        "Agent search hydration must use one window, one count, and three batched relation queries"
    );
    let verdict = evaluate_query_count(contract.maximum_queries, &small, &large);
    assert!(verdict.is_accepted(), "{:?}", verdict.failures());
}

#[tokio::test(flavor = "current_thread")]
async fn single_agent_fetch_reuses_four_query_hydration_and_missing_short_circuits() {
    let workspace = TestWorkspace::new().expect("test workspace");
    let database = Store::open_local(StoreOpenOptions::new(
        workspace.database_path(),
        workspace.plugin_root(),
    ))
    .await
    .expect("open canonical v4 Store");
    let agents = AgentStore::from_store(&database).expect("Agent Store");
    let (tool_id, skill_id, capability) = seed_resource_fixture(&database).await;
    let always_skill_id = sqlx::query_scalar::<_, i64>(
        "INSERT INTO skills (identifier, name, description, frontmatter, content, source, \
         is_always, category_id, required_capabilities, created_at, updated_at) \
         VALUES ('agent_fetch_always_skill', 'Agent fetch always Skill', '', NULL, \
         'always fixture skill', 'workspace', 1, NULL, NULL, ?, ?) RETURNING id",
    )
    .bind("2026-08-26T00:00:00Z")
    .bind("2026-08-26T00:00:00Z")
    .fetch_one(database.pool())
    .await
    .expect("seed always-on Skill outside the measured fetch");
    let expected = agents
        .create(
            root_input("fetch-query-count", "Fetch query count")
                .with_tools([tool_id])
                .with_skills([skill_id])
                .with_capabilities([capability]),
        )
        .await
        .expect("seed Agent with all three resource kinds");

    let measured_pool = SqlitePoolOptions::new()
        .max_connections(1)
        .connect_with(
            SqliteConnectOptions::new()
                .filename(workspace.database_path())
                .foreign_keys(true),
        )
        .await
        .expect("open one-connection query-count pool over the real v4 Store");
    let measured_agents = AgentStore::new(measured_pool.clone()).expect("measured Agent Store");

    let mut connection = measured_pool
        .acquire()
        .await
        .expect("acquire measured pool");
    connection
        .clear_cached_statements()
        .await
        .expect("clear statements before existing fetch");
    assert_eq!(connection.cached_statements_size(), 0);
    drop(connection);

    let fetched = measured_agents
        .fetch_one(expected.id())
        .await
        .expect("fetch existing Agent")
        .expect("existing Agent");
    assert_eq!(fetched.tool_ids(), [tool_id]);
    assert_eq!(fetched.skill_ids(), [skill_id]);
    assert_eq!(fetched.always_skill_ids(), [always_skill_id]);
    assert_eq!(fetched.capability_names(), ["log.emit"]);
    let mut connection = measured_pool
        .acquire()
        .await
        .expect("reacquire measured pool");
    let existing_queries = connection.cached_statements_size();
    connection
        .clear_cached_statements()
        .await
        .expect("clear statements before missing fetch");
    drop(connection);

    assert!(
        measured_agents
            .fetch_one(i64::MAX)
            .await
            .expect("fetch missing Agent")
            .is_none()
    );
    let mut connection = measured_pool
        .acquire()
        .await
        .expect("reacquire measured pool");
    let missing_queries = connection.cached_statements_size();
    connection
        .clear_cached_statements()
        .await
        .expect("clear statements after query-count sample");

    assert_eq!(
        (existing_queries, missing_queries),
        (4, 1),
        "existing Agent fetch must use one root plus three batched relation queries; missing fetch must stop after its root query"
    );
}

#[tokio::test(flavor = "current_thread")]
async fn hierarchy_cycle_and_depth_over_ten_are_rejected_without_modification() {
    let (_workspace, store) = fresh_store().await;
    let root = store
        .create(root_input("depth-root", "Depth root"))
        .await
        .expect("create root");
    let mut parent = root.clone();
    for depth in 1..=10 {
        parent = store
            .create(
                root_input(&format!("depth-{depth:02}"), &format!("Depth {depth}"))
                    .with_parent(Some(parent.id())),
            )
            .await
            .expect("create accepted depth");
        assert_eq!(parent.depth(), depth);
    }
    let before_count: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM agents")
        .fetch_one(store.pool())
        .await
        .expect("count before depth rejection");
    let depth_error = store
        .create(root_input("depth-11", "Depth 11").with_parent(Some(parent.id())))
        .await
        .expect_err("depth 11 must fail before mutation");
    assert_eq!(depth_error.field(), "parent_agent_id");
    assert_eq!(depth_error.reason(), "depth_exceeded");
    assert_eq!(
        sqlx::query_scalar::<_, i64>("SELECT COUNT(*) FROM agents")
            .fetch_one(store.pool())
            .await
            .expect("count after depth rejection"),
        before_count
    );

    let cycle_error = store
        .update(
            root.id(),
            root_input("depth-root", "Depth root").with_parent(Some(parent.id())),
        )
        .await
        .expect_err("an ancestor cannot be moved below its descendant");
    assert_eq!(cycle_error.field(), "parent_agent_id");
    assert_eq!(cycle_error.reason(), "cycle");
    let root_after = store
        .fetch_one(root.id())
        .await
        .expect("reload root")
        .expect("root remains");
    let deepest_after = store
        .fetch_one(parent.id())
        .await
        .expect("reload deepest")
        .expect("deepest remains");
    assert_eq!(root_after.parent_agent_id(), None);
    assert_eq!(root_after.depth(), 0);
    assert_eq!(deepest_after.depth(), 10);
}

#[tokio::test(flavor = "current_thread")]
async fn missing_model_preset_is_field_invalid_input_with_zero_modification() {
    let (_workspace, store) = fresh_store().await;
    let before: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM agents")
        .fetch_one(store.pool())
        .await
        .expect("count before invalid preset");
    let error = store
        .create(
            root_input("missing-preset", "Missing preset")
                .with_model_preset("preset-does-not-exist"),
        )
        .await
        .expect_err("non-empty model_preset must reference LlmPreset.name");
    assert_eq!(error.field(), "model_preset");
    assert_eq!(error.reason(), "not_found");
    let after: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM agents")
        .fetch_one(store.pool())
        .await
        .expect("count after invalid preset");
    assert_eq!(after, before, "invalid preset must not write an Agent");
}

#[tokio::test(flavor = "current_thread")]
async fn three_resource_relations_batch_load_in_four_constant_queries() {
    let workspace = TestWorkspace::new().expect("test workspace");
    let observer = QueryCountObserver::new();
    let (database, agents) = open_observed_store(&workspace, observer.clone()).await;
    let (tool_id, skill_id, capability) = seed_resource_fixture(&database).await;
    let mut ids = Vec::new();
    for index in 0..25 {
        let record = agents
            .create(
                root_input(
                    &format!("resource-{index:03}"),
                    &format!("Resource {index:03}"),
                )
                .with_tools([tool_id])
                .with_skills([skill_id])
                .with_capabilities([capability.clone()]),
            )
            .await
            .expect("seed resource Agent");
        ids.push(record.id());
    }

    let contract = production_query_count_catalog()
        .iter()
        .find(|contract| contract.id == "agent.resource_snapshot")
        .expect("Agent resource query-count contract");
    assert_eq!(contract.owner_phase, "US13");
    assert_eq!(contract.activation_task, "T115");
    assert!(
        contract.active,
        "T115 must activate the production observer"
    );
    assert_eq!(contract.maximum_queries, 4);

    let small_scope = observer.start_scope(contract.id, 1);
    let small = agents
        .load_resource_snapshots(&ids[..1])
        .await
        .expect("load one Agent snapshot");
    let small_sample = small_scope.finish();
    let large_scope = observer.start_scope(contract.id, ids.len());
    let large = agents
        .load_resource_snapshots(&ids)
        .await
        .expect("load many Agent snapshots");
    let large_sample = large_scope.finish();

    assert_eq!(small.len(), 1);
    assert_eq!(large.len(), ids.len());
    for record in &large {
        assert_eq!(record.tool_ids(), [tool_id]);
        assert_eq!(record.skill_ids(), [skill_id]);
        assert_eq!(record.capability_names(), [capability.as_str()]);
    }
    let verdict = evaluate_query_count(contract.maximum_queries, &small_sample, &large_sample);
    assert!(
        verdict.is_accepted(),
        "Agent resource hydration must remain four indexed production queries: {:?}",
        verdict.failures()
    );
}

#[test]
fn agent_query_catalog_owns_every_filter_and_association_route() {
    let activated = production_query_catalog()
        .iter()
        .filter(|query| {
            query.active
                && query.owner_phase == "US13"
                && query.activation_task == "T115"
                && query.dialect == QueryDialect::Sqlite
        })
        .collect::<Vec<_>>();
    let required_tables = BTreeSet::from([
        "agents",
        "search_documents",
        "search_documents_fts",
        "search_short_grams",
        "agent_tools",
        "agent_skills",
        "agent_capabilities",
        "tools",
        "skills",
        "capabilities",
        "llm_presets",
    ]);
    let observed_tables = activated
        .iter()
        .flat_map(|query| {
            query
                .requirements
                .iter()
                .map(|requirement| requirement.table)
        })
        .collect::<BTreeSet<_>>();
    assert!(
        !activated.is_empty()
            && activated
                .iter()
                .all(|query| { !query.sql.trim().is_empty() && !query.requirements.is_empty() })
            && required_tables.is_subset(&observed_tables),
        "active US13/T115 production query catalog is incomplete: required={required_tables:?}, observed={observed_tables:?}"
    );
}

#[tokio::test(flavor = "current_thread")]
async fn every_active_agent_query_explains_its_exact_registered_production_sql() {
    let workspace = TestWorkspace::new().expect("test workspace");
    let database = Store::open_local(StoreOpenOptions::new(
        workspace.database_path(),
        workspace.plugin_root(),
    ))
    .await
    .expect("open canonical v4 Store");
    let activated = production_query_catalog()
        .iter()
        .filter(|query| {
            query.active
                && query.owner_phase == "US13"
                && query.activation_task == "T115"
                && query.dialect == QueryDialect::Sqlite
        })
        .collect::<Vec<_>>();
    assert!(
        !activated.is_empty(),
        "T115 must activate exact production SQL"
    );

    for query in activated {
        let explain_sql = format!("EXPLAIN QUERY PLAN {}", query.sql);
        let mut statement = sqlx::query(AssertSqlSafe(explain_sql));
        for _ in 0..query.sql.matches('?').count() {
            statement = statement.bind(Option::<String>::None);
        }
        let rows = statement
            .fetch_all(database.pool())
            .await
            .unwrap_or_else(|error| panic!("EXPLAIN exact {}: {error}", query.id));
        let plan = rows
            .into_iter()
            .map(|row| {
                let detail = row.try_get::<String, _>(3).expect("SQLite EXPLAIN detail");
                SqlitePlanRow {
                    id: row.try_get(0).expect("SQLite EXPLAIN id"),
                    parent: row.try_get(1).expect("SQLite EXPLAIN parent"),
                    not_used: row.try_get(2).expect("SQLite EXPLAIN not-used"),
                    detail: Box::leak(detail.into_boxed_str()),
                }
            })
            .collect::<Vec<_>>();
        let verdict = evaluate_sqlite_query(query, &plan, "2026-08-25");
        assert!(
            verdict.is_accepted(),
            "Agent query {} failed exact production EXPLAIN: {:?}; plan={plan:?}",
            query.id,
            verdict.failures()
        );
    }
}

#[test]
fn agent_management_uses_exactly_two_canonical_t005_targets() {
    let owned = target_specs()
        .into_iter()
        .filter(|target| target.owner_task == "T115")
        .collect::<Vec<_>>();
    let ids = owned
        .iter()
        .map(|target| target.id.as_str())
        .collect::<BTreeSet<_>>();
    assert_eq!(ids, BTreeSet::from([AGENT_CRUD_ID, AGENT_SEARCH_PAGE_ID]));
    assert_eq!(owned.len(), 2);
    const { assert!(AGENT_FIXTURE_ROWS >= 100) };
    assert_eq!(AGENT_SEARCH_PAGE_SCHEDULE.len(), MEASURED_SAMPLES);
    for target in owned {
        assert_eq!(target.warmup_iterations, 10);
        assert_eq!(target.measured_samples, MEASURED_SAMPLES);
        match target.id.as_str() {
            AGENT_CRUD_ID => assert_eq!(target.p95_budget_ns, 1_000_000_000),
            AGENT_SEARCH_PAGE_ID => assert_eq!(target.p95_budget_ns, 500_000_000),
            other => panic!("unexpected Agent management target {other}"),
        }
    }
}

#[cfg_attr(
    debug_assertions,
    ignore = "release-only T005 benchmark gate; debug has a distinct environment fingerprint"
)]
#[tokio::test(flavor = "current_thread")]
async fn agent_performance_runner_meets_budgets_and_approved_baselines() {
    let current_source_revision = source_revision(Path::new(env!("CARGO_MANIFEST_DIR")))
        .expect("capture repository-anchored benchmark source revision");
    let as_of = Utc::now().date_naive();
    let environment = EnvironmentFingerprint::capture();
    for target in target_specs()
        .into_iter()
        .filter(|target| target.id == AGENT_CRUD_ID || target.id == AGENT_SEARCH_PAGE_ID)
    {
        let report = run_target(&target, &environment)
            .await
            .unwrap_or_else(|error| panic!("run canonical {} benchmark: {error}", target.id))
            .unwrap_or_else(|| panic!("T005 runner must own {}", target.id));
        assert_eq!(report.target, target);
        assert_eq!(report.environment, environment);
        assert_eq!(report.sample_count, MEASURED_SAMPLES);
        let baseline = baseline_path(&target, &environment);
        let exception = regression_exception_path(&target, &environment);
        let evaluation = evaluate_benchmark_gate(
            &report,
            &current_source_revision,
            &baseline,
            &exception,
            as_of,
        )
        .unwrap_or_else(|error| {
            panic!(
                "{} gate error: baseline={}, exception={}, error={error}",
                target.id,
                baseline.display(),
                exception.display()
            )
        });
        assert!(
            matches!(
                evaluation.outcome,
                ComparisonOutcome::Passed | ComparisonOutcome::ApprovedException
            ),
            "{} requires an approved T005 baseline comparison: baseline={}, exception={}, evaluation={evaluation:?}",
            target.id,
            baseline.display(),
            exception.display()
        );
    }
}

#[tokio::test(flavor = "current_thread")]
async fn delete_agent_clears_default_invariant() {
    let (_workspace, store) = fresh_store().await;

    let first = store
        .create(root_input("first", "First"))
        .await
        .expect("create first");
    let second = store
        .create(root_input("second", "Second"))
        .await
        .expect("create second");

    // Deleting the default requires a replacement.
    let err = store
        .delete(first.id(), None)
        .await
        .expect_err("delete default without replacement must fail");
    assert_eq!(err.field(), "is_default");
    assert_eq!(err.reason(), "replacement_required");

    // Deleting with a replacement succeeds and swaps the default.
    store
        .delete(first.id(), Some(second.id()))
        .await
        .expect("delete with replacement");
    let default = store.default_agent().await.expect("default fetch");
    let current = default.expect("default must exist after replacement");
    assert_eq!(current.id(), second.id());
    assert!(current.is_default());
}

#[allow(dead_code)]
fn _pin_types() {
    let _ = std::any::TypeId::of::<AgentPage>();
    let _ = std::any::TypeId::of::<AgentRecord>();
}
