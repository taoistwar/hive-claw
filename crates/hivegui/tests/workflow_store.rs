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

use hivegui::datasource::store::{Store, StoreOpenOptions};
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
