//! T091 [P] [US10] Workflow DAG execution contract.
//!
//! Source of truth: `specs/011-hivegui-standalone-mode/tasks.md` §T091
//! ("编写 `start_node/end_node/function_node/generate_answer_node`、
//! 可达性、悬空边、循环、并行层、失败/超时 fail-fast、零重试和取消测试").
//!
//! The executor under test is `hivegui::runtime::WorkflowExecutor`. The
//! real Function/Plugin/LLM runtimes are swapped for a recording stub so
//! the scheduling contract (topological order, fail-fast, zero-retry and
//! cancellation) can be asserted without invoking any backend.

use std::collections::HashMap;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use serde_json::{Value, json};

use hivegui::datasource::workflow_store::{NodeType, WorkflowGraph, WorkflowNode};
use hivegui::runtime::{
    CancelHandle, WorkflowExecutionError, WorkflowExecutor, WorkflowNodeExecutor,
    WorkflowValidationError,
};

/// Recording node executor. Records the order nodes execute (only after
/// the cooperative cancel check passes), and can inject a failure reason
/// or a wall-clock delay per node key.
#[derive(Clone, Default)]
struct RecordingExecutor {
    executed: Arc<Mutex<Vec<String>>>,
    failures: Arc<Mutex<HashMap<String, String>>>,
    delays: Arc<Mutex<HashMap<String, Duration>>>,
}

impl RecordingExecutor {
    fn new() -> Self {
        Self::default()
    }

    fn fail_on(&self, key: &str, reason: impl Into<String>) {
        self.failures
            .lock()
            .unwrap()
            .insert(key.to_string(), reason.into());
    }

    fn delay_on(&self, key: &str, d: Duration) {
        self.delays.lock().unwrap().insert(key.to_string(), d);
    }

    fn executed(&self) -> Vec<String> {
        self.executed.lock().unwrap().clone()
    }
}

impl WorkflowNodeExecutor for RecordingExecutor {
    fn execute(
        &self,
        node: WorkflowNode,
        _input: Value,
        cancel: CancelHandle,
    ) -> std::pin::Pin<Box<dyn std::future::Future<Output = Result<Value, String>> + Send>> {
        let executed = Arc::clone(&self.executed);
        let failures = Arc::clone(&self.failures);
        let delays = Arc::clone(&self.delays);
        Box::pin(async move {
            let key = node.key().to_string();
            // Copy the delay out first so the MutexGuard is dropped before
            // the await point (MutexGuard is not `Send`).
            let delay = delays.lock().unwrap().get(&key).copied();
            if let Some(d) = delay {
                tokio::time::sleep(d).await;
            }
            if cancel.is_cancelled() {
                return Err("cancelled".to_string());
            }
            executed.lock().unwrap().push(key.clone());
            let reason = failures.lock().unwrap().get(&key).cloned();
            if let Some(reason) = reason {
                return Err(reason);
            }
            Ok(Value::String(key))
        })
    }
}

fn linear_graph() -> WorkflowGraph {
    WorkflowGraph::builder()
        .name("linear")
        .node(WorkflowNode::new("start", NodeType::Start))
        .node(WorkflowNode::new("f1", NodeType::Function))
        .node(WorkflowNode::new("end", NodeType::End))
        .edge("start", "f1")
        .edge("f1", "end")
        .build()
}

#[test]
fn all_four_node_types_are_supported() {
    let graph = WorkflowGraph::builder()
        .name("all-kinds")
        .node(WorkflowNode::new("start", NodeType::Start))
        .node(WorkflowNode::new("f1", NodeType::Function))
        .node(WorkflowNode::new("gen", NodeType::GenerateAnswer))
        .node(WorkflowNode::new("end", NodeType::End))
        .edge("start", "f1")
        .edge("f1", "gen")
        .edge("gen", "end")
        .build();

    let executor = WorkflowExecutor::new(RecordingExecutor::new());
    executor.validate(&graph).expect("graph must be valid");
}

#[tokio::test(flavor = "current_thread")]
async fn linear_dag_executes_in_topological_order() {
    let recording = RecordingExecutor::new();
    let executor = WorkflowExecutor::new(recording.clone());

    let outcome = executor
        .execute(&linear_graph(), json!({}), CancelHandle::new())
        .await
        .expect("linear graph must execute");
    assert!(outcome.completed, "linear graph must complete");

    let keys = recording.executed();
    assert_eq!(keys, vec!["start", "f1", "end"], "topological order");
}

#[tokio::test(flavor = "current_thread")]
async fn parallel_nodes_in_same_level_all_execute() {
    let graph = WorkflowGraph::builder()
        .name("parallel")
        .node(WorkflowNode::new("start", NodeType::Start))
        .node(WorkflowNode::new("a", NodeType::Function))
        .node(WorkflowNode::new("b", NodeType::Function))
        .node(WorkflowNode::new("end", NodeType::End))
        .edge("start", "a")
        .edge("start", "b")
        .edge("a", "end")
        .edge("b", "end")
        .build();

    let recording = RecordingExecutor::new();
    let executor = WorkflowExecutor::new(recording.clone());
    let outcome = executor
        .execute(&graph, json!({}), CancelHandle::new())
        .await
        .expect("parallel graph must execute");
    assert!(outcome.completed);

    let keys = recording.executed();
    // start runs first, then a and b (any order), then end.
    assert_eq!(keys.first().map(String::as_str), Some("start"));
    assert_eq!(keys.last().map(String::as_str), Some("end"));
    assert!(keys.contains(&"a".to_string()), "node a must run");
    assert!(keys.contains(&"b".to_string()), "node b must run");
    assert_eq!(keys.len(), 4);
}

#[tokio::test(flavor = "current_thread")]
async fn dangling_edge_is_rejected() {
    let graph = WorkflowGraph::builder()
        .name("dangling")
        .node(WorkflowNode::new("start", NodeType::Start))
        .node(WorkflowNode::new("end", NodeType::End))
        .edge("start", "missing")
        .build();

    let executor = WorkflowExecutor::new(RecordingExecutor::new());
    let err = executor
        .execute(&graph, json!({}), CancelHandle::new())
        .await
        .expect_err("dangling edge must fail");
    match err {
        WorkflowExecutionError::Validation(WorkflowValidationError::DanglingEdge(key)) => {
            assert_eq!(key, "missing");
        }
        other => panic!("expected DanglingEdge, got {other:?}"),
    }
}

#[tokio::test(flavor = "current_thread")]
async fn cycle_is_rejected() {
    let graph = WorkflowGraph::builder()
        .name("cycle")
        .node(WorkflowNode::new("start", NodeType::Start))
        .node(WorkflowNode::new("a", NodeType::Function))
        .node(WorkflowNode::new("b", NodeType::Function))
        .node(WorkflowNode::new("end", NodeType::End))
        .edge("start", "a")
        .edge("a", "b")
        .edge("b", "a")
        .edge("b", "end")
        .build();

    let executor = WorkflowExecutor::new(RecordingExecutor::new());
    let err = executor
        .execute(&graph, json!({}), CancelHandle::new())
        .await
        .expect_err("cycle must fail");
    match err {
        WorkflowExecutionError::Validation(WorkflowValidationError::Cycle(keys)) => {
            assert!(!keys.is_empty(), "cycle must name at least one node");
        }
        other => panic!("expected Cycle, got {other:?}"),
    }
}

#[tokio::test(flavor = "current_thread")]
async fn unreachable_node_is_rejected() {
    let graph = WorkflowGraph::builder()
        .name("unreachable")
        .node(WorkflowNode::new("start", NodeType::Start))
        .node(WorkflowNode::new("end", NodeType::End))
        .node(WorkflowNode::new("orphan", NodeType::Function))
        .edge("start", "end")
        .build();

    let executor = WorkflowExecutor::new(RecordingExecutor::new());
    let err = executor
        .execute(&graph, json!({}), CancelHandle::new())
        .await
        .expect_err("unreachable node must fail");
    match err {
        WorkflowExecutionError::Validation(WorkflowValidationError::UnreachableNode(key)) => {
            assert_eq!(key, "orphan");
        }
        other => panic!("expected UnreachableNode, got {other:?}"),
    }
}

#[tokio::test(flavor = "current_thread")]
async fn node_failure_is_fail_fast_without_retry() {
    let graph = WorkflowGraph::builder()
        .name("fail-fast")
        .node(WorkflowNode::new("start", NodeType::Start))
        .node(WorkflowNode::new("f1", NodeType::Function))
        .node(WorkflowNode::new("f2", NodeType::Function))
        .node(WorkflowNode::new("end", NodeType::End))
        .edge("start", "f1")
        .edge("f1", "f2")
        .edge("f2", "end")
        .build();

    let recording = RecordingExecutor::new();
    recording.fail_on("f1", "boom");
    let executor = WorkflowExecutor::new(recording.clone());

    let err = executor
        .execute(&graph, json!({}), CancelHandle::new())
        .await
        .expect_err("node failure must propagate");
    match err {
        WorkflowExecutionError::NodeFailed(key, reason) => {
            assert_eq!(key, "f1");
            assert_eq!(reason, "boom");
        }
        other => panic!("expected NodeFailed, got {other:?}"),
    }

    // Zero retry: the failing node runs exactly once, and no node after
    // it runs.
    let keys = recording.executed();
    assert_eq!(
        keys,
        vec!["start", "f1"],
        "fail-fast stops after first failure"
    );
}

#[tokio::test(flavor = "current_thread")]
async fn pre_cancelled_execution_returns_cancelled() {
    let executor = WorkflowExecutor::new(RecordingExecutor::new());
    let cancel = CancelHandle::new();
    cancel.signal();

    let outcome = executor
        .execute(&linear_graph(), json!({}), cancel)
        .await
        .expect("pre-cancelled execution returns an outcome");
    assert!(!outcome.completed);
    assert!(outcome.cancelled);
    assert!(
        outcome.node_results.is_empty(),
        "no node runs after pre-cancel"
    );
}

#[tokio::test(flavor = "current_thread")]
async fn cancellation_stops_subsequent_levels() {
    let graph = WorkflowGraph::builder()
        .name("cancel")
        .node(WorkflowNode::new("start", NodeType::Start))
        .node(WorkflowNode::new("slow", NodeType::Function))
        .node(WorkflowNode::new("end", NodeType::End))
        .edge("start", "slow")
        .edge("slow", "end")
        .build();

    let recording = RecordingExecutor::new();
    recording.delay_on("slow", Duration::from_millis(300));
    let cancel = CancelHandle::new();

    let executor_for_task = WorkflowExecutor::new(recording.clone());
    let graph_for_task = graph.clone();
    let cancel_for_task = cancel.clone();
    let input = json!({});

    let handle = tokio::spawn(async move {
        executor_for_task
            .execute(&graph_for_task, input, cancel_for_task)
            .await
    });

    // Signal cancellation after the slow node has started.
    tokio::time::sleep(Duration::from_millis(50)).await;
    cancel.signal();

    let result = handle.await.expect("task must not panic");
    match result {
        Ok(outcome) => {
            assert!(!outcome.completed, "cancelled run must not complete");
        }
        Err(WorkflowExecutionError::NodeFailed(key, reason)) => {
            assert_eq!(reason, "cancelled", "slow node observes cancel");
            assert_eq!(key, "slow");
        }
        Err(other) => panic!("unexpected error: {other:?}"),
    }

    // `end` must never run because cancellation stops subsequent levels.
    let keys = recording.executed();
    assert!(
        !keys.contains(&"end".to_string()),
        "end must not run after cancellation"
    );
}
