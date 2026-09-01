//! T090 [P] [US10] Workflow DAG store contract.
//!
//! Source of truth: `specs/011-hivegui-standalone-mode/tasks.md` §T090
//! ("编写四个稳定 `*_node` 值、v4 普通写入拒绝短值、整图事务保存、
//! 唯一 node_key、外键、Function RESTRICT、Tool 引用 Workflow 时删除
//! 返回 `conflict { field: \"id\", reason: \"referenced_by_tool\", references }`
//! 且零修改、批量加载/查询计数、删除级联、每页20条搜索分页、全部生产
//! 过滤/关联查询 EXPLAIN 预期索引、CRUD p95≤1s 及搜索/翻页 p95≤500ms
//! 测试").
//!
//! Red public boundaries (T095 will add these):
//!   - `hivegui::datasource::workflow_store::WorkflowStore`
//!   - `hivegui::datasource::workflow_store::WorkflowGraph`
//!   - `hivegui::datasource::workflow_store::NodeType` enum
//!     (start_node | end_node | function_node | generate_answer_node)
//!   - `hivegui::datasource::workflow_store::WorkflowConflict`
//!
//! T094 reviewer signs §T090.11 + §T091-T093.11; T095-T099
//! implementation then make these tests Green; T100 reruns to record
//! the Green evidence.

mod support;

use hivegui::datasource::function_store::{FunctionInput, FunctionKind, FunctionStore};
use hivegui::datasource::query_count::{QueryCountObserver, evaluate_query_count};
use hivegui::datasource::store::{Store, StoreOpenOptions};
use hivegui::datasource::validation::PublicErrorEnvelope;
use hivegui::datasource::workflow_store::{
    NodeType, WorkflowConflict, WorkflowGraph, WorkflowNode, WorkflowStore,
};
use support::TestWorkspace;

/// Open a real v4 Store (which runs migrations / creates tables) and
/// return its pool plus a `WorkflowStore` bound to it.
async fn open_workflow_store(workspace: &TestWorkspace) -> (Store, WorkflowStore) {
    let store = Store::open_local(StoreOpenOptions::new(
        workspace.database_path(),
        workspace.plugin_root(),
    ))
    .await
    .expect("open real v4 Store");
    let workflow_store = WorkflowStore::new(store.pool().clone()).expect("workflow store");
    (store, workflow_store)
}

#[test]
fn node_type_enumerates_exactly_four_stable_values() {
    let kinds = [
        NodeType::Start,
        NodeType::End,
        NodeType::Function,
        NodeType::GenerateAnswer,
    ];
    let names: Vec<&'static str> = kinds.iter().map(|k| k.as_str()).collect();
    assert_eq!(
        names,
        vec![
            "start_node",
            "end_node",
            "function_node",
            "generate_answer_node"
        ],
        "NodeType strings are the stable v4 contract"
    );
}

#[tokio::test(flavor = "current_thread")]
async fn create_workflow_persists_full_graph_in_single_transaction() {
    let workspace = TestWorkspace::new().expect("test workspace");
    let (_store, store) = open_workflow_store(&workspace).await;

    let graph = WorkflowGraph::builder()
        .name("demo")
        .node(WorkflowNode::new("start", NodeType::Start))
        .node(WorkflowNode::new("end", NodeType::End))
        .edge("start", "end")
        .build();
    let created = store.create(graph).await.expect("create");
    assert_eq!(created.node_count(), 2);
}

#[tokio::test(flavor = "current_thread")]
async fn node_keys_must_be_unique_within_a_workflow() {
    let workspace = TestWorkspace::new().expect("test workspace");
    let (_store, store) = open_workflow_store(&workspace).await;

    let graph = WorkflowGraph::builder()
        .name("dup-keys")
        .node(WorkflowNode::new("only", NodeType::Start))
        .node(WorkflowNode::new("only", NodeType::End))
        .build();
    let err = store
        .create(graph)
        .await
        .expect_err("duplicate node_key must fail");
    assert_eq!(err.reason(), "duplicate_node_key");
    assert_eq!(err.field(), "node_key");
}

#[tokio::test(flavor = "current_thread")]
async fn deletion_referenced_by_tool_returns_conflict() {
    let workspace = TestWorkspace::new().expect("test workspace");
    let (store, workflow_store) = open_workflow_store(&workspace).await;

    let graph = WorkflowGraph::builder()
        .name("referenced")
        .node(WorkflowNode::new("start", NodeType::Start))
        .node(WorkflowNode::new("end", NodeType::End))
        .edge("start", "end")
        .build();
    let workflow = workflow_store.create(graph).await.expect("create");

    // Create a workflow-wrap Tool referencing the workflow so the
    // RESTRICT boundary is actually exercised (the workflow is now
    // referenced, so delete must fail with zero modification).
    hivegui::datasource::entity_store::Tool::create(
        store.pool(),
        "ref-tool".to_string(),
        "Ref Tool".to_string(),
        "References the workflow".to_string(),
        "workflow-wrap".to_string(),
        "workspace".to_string(),
        false,
        None,
        Some(workflow.id()),
        "{}".to_string(),
        "{}".to_string(),
        None,
        None,
    )
    .await
    .expect("create referencing tool");

    let conflict = workflow_store
        .delete(workflow.id())
        .await
        .expect_err("delete must fail when referenced by a tool");
    let expected = WorkflowConflict::ReferencedByTool;
    assert_eq!(conflict, expected);
    assert_eq!(conflict.field(), "id");
    assert_eq!(conflict.reason(), "referenced_by_tool");
    assert!(!conflict.references().is_empty());
}

#[allow(dead_code, clippy::diverging_sub_expression)]
fn _pin_types() {
    let _: WorkflowNode = panic!("placeholder so the type is referenced");
}

#[tokio::test(flavor = "current_thread")]
async fn function_id_roundtrips_through_create_and_load_graph() {
    // T098 突破：function_node 的 function_id 必须在 create → load_graph
    // 之间往返，这样 LocalWorkflowNodeExecutor 才能把节点关联到 Function。
    let workspace = TestWorkspace::new().expect("test workspace");
    let (store, workflow_store) = open_workflow_store(&workspace).await;

    // function_id 有 FK 引用，先创建真实 Function 拿 id。
    let function_input = FunctionInput::for_write(
        "fn-1".to_string(),
        "Fn 1".to_string(),
        None,
        FunctionKind::Placeholder,
        "{}".to_string(),
        "{}".to_string(),
        None,
        None,
        None,
        None,
    )
    .expect("validate function fixture");
    let function = FunctionStore::new(store.pool().clone())
        .expect("Function Store")
        .create(function_input)
        .await
        .expect("create function");

    let graph = WorkflowGraph::builder()
        .name("fn-linked")
        .node(WorkflowNode::new("start", NodeType::Start))
        .node(WorkflowNode::new("run", NodeType::Function).with_function_id(function.id()))
        .node(WorkflowNode::new("end", NodeType::End))
        .edge("start", "run")
        .edge("run", "end")
        .build();
    let created = workflow_store.create(graph).await.expect("create");

    let loaded = workflow_store
        .load_graph(created.id(), "fn-linked")
        .await
        .expect("load graph");
    let run_node = loaded
        .nodes()
        .iter()
        .find(|n| n.key() == "run")
        .expect("run node");
    assert_eq!(
        run_node.function_id(),
        Some(function.id()),
        "function_id must round-trip through create + load_graph"
    );

    let start_node = loaded
        .nodes()
        .iter()
        .find(|n| n.key() == "start")
        .expect("start node");
    assert_eq!(start_node.function_id(), None, "start has no function_id");
}

#[tokio::test(flavor = "current_thread")]
async fn workflow_crud_roundtrips_every_public_metadata_field() {
    use hivegui::datasource::entity_store::{Category, Workflow};

    let workspace = TestWorkspace::new().expect("test workspace");
    let (store, _workflow_store) = open_workflow_store(&workspace).await;
    let first_category = Category::create(
        store.pool(),
        None,
        "Workflow category one".to_string(),
        "workflow-category-one".to_string(),
        Some("first category".to_string()),
    )
    .await
    .expect("create first category");
    let second_category = Category::create(
        store.pool(),
        None,
        "Workflow category two".to_string(),
        "workflow-category-two".to_string(),
        Some("second category".to_string()),
    )
    .await
    .expect("create second category");

    let created = Workflow::create(
        store.pool(),
        "metadata-roundtrip".to_string(),
        "Metadata round-trip".to_string(),
        Some("created description".to_string()),
        45_678,
        Some(first_category.id),
        Some(r#"{"type":"object","required":["query"]}"#.to_string()),
        Some("Start with the local query.".to_string()),
        Some(r#"{"type":"object","required":["answer"]}"#.to_string()),
        Some(r#"["filesystem.read"]"#.to_string()),
    )
    .await
    .expect("create workflow with every public metadata field");

    assert_eq!(created.identifier, "metadata-roundtrip");
    assert_eq!(created.name, "Metadata round-trip");
    assert_eq!(created.description.as_deref(), Some("created description"));
    assert_eq!(created.timeout_ms, 45_678);
    assert_eq!(created.category_id, Some(first_category.id));
    assert_eq!(
        created.input_schema.as_deref(),
        Some(r#"{"type":"object","required":["query"]}"#)
    );
    assert_eq!(
        created.start_description.as_deref(),
        Some("Start with the local query.")
    );
    assert_eq!(
        created.output_schema.as_deref(),
        Some(r#"{"type":"object","required":["answer"]}"#)
    );
    assert_eq!(
        created.required_capabilities.as_deref(),
        Some(r#"["filesystem.read"]"#)
    );

    let updated = Workflow::update(
        store.pool(),
        created.id,
        "metadata-roundtrip-v2".to_string(),
        "Metadata round-trip v2".to_string(),
        Some("updated description".to_string()),
        98_765,
        Some(second_category.id),
        Some(r#"{"type":"array"}"#.to_string()),
        Some("Updated start description.".to_string()),
        Some(r#"{"type":"string"}"#.to_string()),
        Some(r#"["network.http"]"#.to_string()),
    )
    .await
    .expect("update workflow with every public metadata field");
    let reloaded = Workflow::get(store.pool(), created.id)
        .await
        .expect("reload updated workflow")
        .expect("updated workflow exists");

    assert_eq!(updated.identifier, "metadata-roundtrip-v2");
    assert_eq!(reloaded.identifier, updated.identifier);
    assert_eq!(reloaded.name, "Metadata round-trip v2");
    assert_eq!(reloaded.description.as_deref(), Some("updated description"));
    assert_eq!(reloaded.timeout_ms, 98_765);
    assert_eq!(reloaded.category_id, Some(second_category.id));
    assert_eq!(
        reloaded.input_schema.as_deref(),
        Some(r#"{"type":"array"}"#)
    );
    assert_eq!(
        reloaded.start_description.as_deref(),
        Some("Updated start description.")
    );
    assert_eq!(
        reloaded.output_schema.as_deref(),
        Some(r#"{"type":"string"}"#)
    );
    assert_eq!(
        reloaded.required_capabilities.as_deref(),
        Some(r#"["network.http"]"#)
    );
}

#[tokio::test(flavor = "current_thread")]
async fn referenced_delete_reports_the_actual_safe_tool_and_preserves_every_row() {
    let workspace = TestWorkspace::new().expect("test workspace");
    let (store, workflow_store) = open_workflow_store(&workspace).await;

    let graph = WorkflowGraph::builder()
        .name("snapshot-preserved")
        .node(WorkflowNode::new("start", NodeType::Start))
        .node(WorkflowNode::new("end", NodeType::End))
        .edge("start", "end")
        .build();
    let workflow = workflow_store.create(graph).await.expect("create workflow");
    let tool = hivegui::datasource::entity_store::Tool::create(
        store.pool(),
        "safe-tool-reference".to_string(),
        "Safe tool reference".to_string(),
        "References the workflow".to_string(),
        "workflow-wrap".to_string(),
        "workspace".to_string(),
        false,
        None,
        Some(workflow.id()),
        "{}".to_string(),
        "{}".to_string(),
        None,
        None,
    )
    .await
    .expect("create referencing tool");

    async fn snapshot(
        pool: &sqlx::Pool<sqlx::Sqlite>,
        workflow_id: i64,
        tool_id: i64,
    ) -> (
        Option<(String, String, Option<String>, i64)>,
        Vec<(String, String, Option<i64>, String)>,
        Vec<(String, String, String)>,
        Option<(String, String, Option<i64>)>,
    ) {
        let workflow = sqlx::query_as(
            "SELECT identifier, name, description, timeout_ms FROM workflows WHERE id = ?",
        )
        .bind(workflow_id)
        .fetch_optional(pool)
        .await
        .expect("snapshot workflow");
        let nodes = sqlx::query_as(
            "SELECT node_key, node_type, function_id, node_config FROM workflow_nodes WHERE workflow_id = ? ORDER BY node_key",
        )
        .bind(workflow_id)
        .fetch_all(pool)
        .await
        .expect("snapshot nodes");
        let edges = sqlx::query_as(
            "SELECT src_node_key, dst_node_key, mapping FROM workflow_edges WHERE workflow_id = ? ORDER BY src_node_key, dst_node_key",
        )
        .bind(workflow_id)
        .fetch_all(pool)
        .await
        .expect("snapshot edges");
        let tool = sqlx::query_as("SELECT identifier, name, workflow_id FROM tools WHERE id = ?")
            .bind(tool_id)
            .fetch_optional(pool)
            .await
            .expect("snapshot tool");
        (workflow, nodes, edges, tool)
    }

    let before = snapshot(store.pool(), workflow.id(), tool.id).await;
    let conflict = workflow_store
        .delete(workflow.id())
        .await
        .expect_err("a referenced workflow must not be deleted");
    let after = snapshot(store.pool(), workflow.id(), tool.id).await;

    assert_eq!(
        after, before,
        "referenced_by_tool must preserve the Workflow, every node/edge, and the Tool byte-for-byte at the public field boundary"
    );
    assert_eq!(conflict.field(), "id");
    assert_eq!(conflict.reason(), "referenced_by_tool");
    assert_eq!(
        conflict.references(),
        vec!["safe-tool-reference".to_string()],
        "references must contain the actual Tool identifier"
    );
}

#[tokio::test(flavor = "current_thread")]
async fn v4_rejects_every_legacy_short_node_type_without_modification() {
    let workspace = TestWorkspace::new().expect("test workspace");
    let (store, workflow_store) = open_workflow_store(&workspace).await;
    let workflow = workflow_store
        .create(
            WorkflowGraph::builder()
                .name("reject-short-node-types")
                .node(WorkflowNode::new("start", NodeType::Start))
                .node(WorkflowNode::new("end", NodeType::End))
                .edge("start", "end")
                .build(),
        )
        .await
        .expect("create canonical workflow");

    let before: i64 =
        sqlx::query_scalar("SELECT COUNT(*) FROM workflow_nodes WHERE workflow_id = ?")
            .bind(workflow.id())
            .fetch_one(store.pool())
            .await
            .expect("count nodes before invalid writes");

    for (index, short) in ["start", "end", "function", "generate_answer"]
        .into_iter()
        .enumerate()
    {
        let result = sqlx::query(
            "INSERT INTO workflow_nodes \
             (workflow_id, node_key, node_type, function_id, position_x, position_y, node_config, created_at) \
             VALUES (?, ?, ?, NULL, 0, 0, '{}', 'fixture')",
        )
        .bind(workflow.id())
        .bind(format!("legacy-{index}"))
        .bind(short)
        .execute(store.pool())
        .await;
        assert!(
            result.is_err(),
            "v4 must reject legacy short node_type {short:?}"
        );
    }

    let after: i64 =
        sqlx::query_scalar("SELECT COUNT(*) FROM workflow_nodes WHERE workflow_id = ?")
            .bind(workflow.id())
            .fetch_one(store.pool())
            .await
            .expect("count nodes after invalid writes");
    assert_eq!(
        after, before,
        "invalid node_type writes must be zero modification"
    );
}

#[tokio::test(flavor = "current_thread")]
async fn invalid_function_reference_rolls_back_the_entire_graph_transaction() {
    let workspace = TestWorkspace::new().expect("test workspace");
    let (store, workflow_store) = open_workflow_store(&workspace).await;
    let graph = WorkflowGraph::builder()
        .name("atomic-invalid-function")
        .node(WorkflowNode::new("start", NodeType::Start))
        .node(WorkflowNode::new("missing", NodeType::Function).with_function_id(i64::MAX))
        .node(WorkflowNode::new("end", NodeType::End))
        .edge("start", "missing")
        .edge("missing", "end")
        .build();

    workflow_store
        .create(graph)
        .await
        .expect_err("missing Function FK must reject the graph");
    let footprint: (i64, i64, i64) = sqlx::query_as(
        "SELECT \
           (SELECT COUNT(*) FROM workflows WHERE identifier = 'atomic-invalid-function'), \
           (SELECT COUNT(*) FROM workflow_nodes), \
           (SELECT COUNT(*) FROM workflow_edges)",
    )
    .fetch_one(store.pool())
    .await
    .expect("read graph footprint");
    assert_eq!(
        footprint,
        (0, 0, 0),
        "failed graph save must roll back every row"
    );
}

#[tokio::test(flavor = "current_thread")]
async fn workflow_node_reference_restricts_function_delete_with_a_safe_reference() {
    let workspace = TestWorkspace::new().expect("test workspace");
    let (store, workflow_store) = open_workflow_store(&workspace).await;
    let function_store = FunctionStore::new(store.pool().clone()).expect("Function Store");
    let function = function_store
        .create(
            FunctionInput::for_write(
                "workflow-restricted-function".to_string(),
                "Workflow restricted function".to_string(),
                None,
                FunctionKind::Placeholder,
                "{}".to_string(),
                "{}".to_string(),
                None,
                None,
                None,
                None,
            )
            .expect("valid Function fixture"),
        )
        .await
        .expect("create Function");
    let workflow = workflow_store
        .create(
            WorkflowGraph::builder()
                .name("workflow-function-restrict")
                .node(WorkflowNode::new("start", NodeType::Start))
                .node(WorkflowNode::new("call", NodeType::Function).with_function_id(function.id()))
                .node(WorkflowNode::new("end", NodeType::End))
                .edge("start", "call")
                .edge("call", "end")
                .build(),
        )
        .await
        .expect("create referencing Workflow");

    let error = function_store
        .delete(function.id())
        .await
        .expect_err("WorkflowNode reference must RESTRICT Function deletion");
    assert_eq!(
        error.envelope(),
        &PublicErrorEnvelope::Conflict {
            shape: "references".to_string(),
            field: "id".to_string(),
            reason: "referenced_by_workflow_node".to_string(),
        }
    );
    assert_eq!(
        error.references(),
        &["workflow-function-restrict".to_string()]
    );
    assert!(
        FunctionStore::new(store.pool().clone())
            .expect("Function Store")
            .get(function.id())
            .await
            .expect("reload Function")
            .is_some(),
        "referenced Function must remain"
    );
    assert_eq!(
        workflow_store
            .load_graph(workflow.id(), "workflow-function-restrict")
            .await
            .expect("reload Workflow")
            .nodes()
            .len(),
        3,
        "referencing graph must remain"
    );
}

#[tokio::test(flavor = "current_thread")]
async fn deleting_an_unreferenced_workflow_cascades_every_node_and_edge() {
    let workspace = TestWorkspace::new().expect("test workspace");
    let (store, workflow_store) = open_workflow_store(&workspace).await;
    let workflow = workflow_store
        .create(
            WorkflowGraph::builder()
                .name("cascade-workflow")
                .node(WorkflowNode::new("start", NodeType::Start))
                .node(WorkflowNode::new("middle", NodeType::GenerateAnswer))
                .node(WorkflowNode::new("end", NodeType::End))
                .edge("start", "middle")
                .edge("middle", "end")
                .build(),
        )
        .await
        .expect("create Workflow");

    workflow_store
        .delete(workflow.id())
        .await
        .expect("delete Workflow");
    let footprint: (i64, i64, i64) = sqlx::query_as(
        "SELECT \
           (SELECT COUNT(*) FROM workflows WHERE id = ?), \
           (SELECT COUNT(*) FROM workflow_nodes WHERE workflow_id = ?), \
           (SELECT COUNT(*) FROM workflow_edges WHERE workflow_id = ?)",
    )
    .bind(workflow.id())
    .bind(workflow.id())
    .bind(workflow.id())
    .fetch_one(store.pool())
    .await
    .expect("read cascade footprint");
    assert_eq!(footprint, (0, 0, 0));
}

#[test]
fn t090_production_query_catalog_owns_every_workflow_filter_and_association() {
    let required = [
        "t090.workflows.page",
        "t090.workflows.count",
        "t090.workflow_graph.nodes",
        "t090.workflow_graph.edges",
        "t090.workflow.references.tools",
    ];
    let catalog = hivegui::datasource::query_plan::production_query_catalog();
    for id in required {
        let query = catalog
            .iter()
            .find(|query| query.id == id)
            .unwrap_or_else(|| panic!("missing T090 production query catalog row {id}"));
        assert_eq!(query.owner_phase, "US10");
        assert_eq!(query.activation_task, "T095");
        assert!(
            query.active,
            "T090 query {id} must be active before EXPLAIN review"
        );
        assert!(
            !query.sql.trim().is_empty(),
            "T090 query {id} must expose exact SQL"
        );
        assert!(
            !query.requirements.is_empty(),
            "T090 query {id} needs plan requirements"
        );
    }
}

#[tokio::test(flavor = "current_thread")]
async fn workflow_list_is_fixed_twenty_stably_ordered_and_searchable() {
    let workspace = TestWorkspace::new().expect("test workspace");
    let (store, workflow_store) = open_workflow_store(&workspace).await;
    for index in (0..41).rev() {
        workflow_store
            .create(
                WorkflowGraph::builder()
                    .name(format!("workflow-page-{index:03}"))
                    .node(WorkflowNode::new("start", NodeType::Start))
                    .node(WorkflowNode::new("end", NodeType::End))
                    .edge("start", "end")
                    .build(),
            )
            .await
            .expect("seed paged Workflow");
    }

    let first = WorkflowStore::from_store(&store)
        .expect("Workflow Store")
        .list(None, 1)
        .await
        .expect("first page");
    let second = WorkflowStore::from_store(&store)
        .expect("Workflow Store")
        .list(None, 2)
        .await
        .expect("second page");
    let filtered = WorkflowStore::from_store(&store)
        .expect("Workflow Store")
        .list(Some("page-040".to_string()), 1)
        .await
        .expect("filtered page");

    assert_eq!(first.page(), 1);
    assert_eq!(first.page_size(), 20);
    assert_eq!(first.total(), 41);
    assert_eq!(first.items().len(), 20);
    assert_eq!(second.page(), 2);
    assert_eq!(second.items().len(), 20);
    let first_names = first
        .items()
        .iter()
        .map(|workflow| workflow.name().to_string())
        .collect::<Vec<_>>();
    let second_names = second
        .items()
        .iter()
        .map(|workflow| workflow.name().to_string())
        .collect::<Vec<_>>();
    assert!(first_names.windows(2).all(|pair| pair[0] < pair[1]));
    assert!(second_names.windows(2).all(|pair| pair[0] < pair[1]));
    assert!(
        first_names.iter().all(|name| !second_names.contains(name)),
        "adjacent pages must not overlap"
    );
    assert_eq!(filtered.total(), 1);
    assert_eq!(filtered.items()[0].name(), "workflow-page-040");
}

#[tokio::test(flavor = "current_thread")]
async fn batch_graph_load_has_constant_query_count_as_cardinality_grows() {
    let workspace = TestWorkspace::new().expect("test workspace");
    let observer = QueryCountObserver::new();
    let store = Store::open_local(
        StoreOpenOptions::new(workspace.database_path(), workspace.plugin_root())
            .with_query_count_observer(observer.clone()),
    )
    .await
    .expect("open observed Store");
    let workflow_store = WorkflowStore::from_store(&store).expect("Workflow Store");
    let mut identities = Vec::new();
    for index in 0..25 {
        let name = format!("batch-load-{index:03}");
        let record = workflow_store
            .create(
                WorkflowGraph::builder()
                    .name(&name)
                    .node(WorkflowNode::new("start", NodeType::Start))
                    .node(WorkflowNode::new("end", NodeType::End))
                    .edge("start", "end")
                    .build(),
            )
            .await
            .expect("seed batch Workflow");
        identities.push((record.id(), name));
    }

    let small_scope = observer.start_scope("us10.workflow.batch_load", 1);
    let small_graphs = workflow_store
        .load_graphs(&identities[..1])
        .await
        .expect("load one graph");
    let small = small_scope.finish();
    let large_scope = observer.start_scope("us10.workflow.batch_load", identities.len());
    let large_graphs = workflow_store
        .load_graphs(&identities)
        .await
        .expect("load many graphs");
    let large = large_scope.finish();

    assert_eq!(small_graphs.len(), 1);
    assert_eq!(large_graphs.len(), identities.len());
    let verdict = evaluate_query_count(3, &small, &large);
    assert!(
        verdict.is_accepted(),
        "batch load must stay within three production queries without N+1: {:?}",
        verdict.failures()
    );
}

fn p95_ms(mut samples: Vec<u128>) -> u128 {
    assert_eq!(
        samples.len(),
        100,
        "T090 performance gates use exactly 100 samples"
    );
    samples.sort_unstable();
    samples[94]
}

#[allow(dead_code)]
fn _pin_t090_public_batch_and_page_boundaries() {
    let _ = WorkflowStore::from_store;
    let _ = WorkflowStore::list;
    let _ = WorkflowStore::load_graphs;
}

#[tokio::test(flavor = "current_thread")]
async fn workflow_crud_and_search_page_meet_fixed_sample_p95_budgets() {
    let workspace = TestWorkspace::new().expect("test workspace");
    let (store, _workflow_store) = open_workflow_store(&workspace).await;
    let workflow_store = WorkflowStore::from_store(&store).expect("Workflow Store");
    for index in 0..100 {
        workflow_store
            .create(
                WorkflowGraph::builder()
                    .name(format!("workflow-perf-fixture-{index:03}"))
                    .node(WorkflowNode::new("start", NodeType::Start))
                    .node(WorkflowNode::new("end", NodeType::End))
                    .edge("start", "end")
                    .build(),
            )
            .await
            .expect("seed performance Workflow");
    }

    let mut crud_samples = Vec::with_capacity(100);
    for index in 0..100 {
        let started = std::time::Instant::now();
        let name = format!("workflow-crud-sample-{index:03}");
        let record = workflow_store
            .create(
                WorkflowGraph::builder()
                    .name(&name)
                    .node(WorkflowNode::new("start", NodeType::Start))
                    .node(WorkflowNode::new("end", NodeType::End))
                    .edge("start", "end")
                    .build(),
            )
            .await
            .expect("create CRUD sample");
        let graph = workflow_store
            .load_graph(record.id(), &name)
            .await
            .expect("read CRUD sample");
        assert_eq!(graph.nodes().len(), 2);
        workflow_store
            .delete(record.id())
            .await
            .expect("delete CRUD sample");
        crud_samples.push(started.elapsed().as_millis());
    }

    let mut search_samples = Vec::with_capacity(100);
    for index in 0..100 {
        let started = std::time::Instant::now();
        let page = workflow_store
            .list(
                if index % 2 == 0 {
                    Some("perf-fixture".to_string())
                } else {
                    None
                },
                (index % 5 + 1) as i64,
            )
            .await
            .expect("search/page sample");
        assert!(page.items().len() <= 20);
        assert_eq!(page.page_size(), 20);
        search_samples.push(started.elapsed().as_millis());
    }

    assert!(
        p95_ms(crud_samples) <= 1_000,
        "Workflow CRUD p95 must be <=1s"
    );
    assert!(
        p95_ms(search_samples) <= 500,
        "Workflow search/page p95 must be <=500ms"
    );
}
