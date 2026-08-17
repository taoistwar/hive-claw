//! US10 workflow DAG execution engine (T096 boundary).
//!
//! The executor consumes the shared [`WorkflowGraph`] produced by
//! [`crate::datasource::workflow_store`] and schedules its nodes in
//! topological level order. Scheduling is fail-fast: the first node
//! failure aborts the whole run without retrying any node, and the
//! cooperative [`CancelHandle`] is propagated into every in-flight
//! node so cancellation stops future scheduling.
//!
//! The four stable `*_node` values are the only node kinds the engine
//! understands:
//!   - `start_node`            entry node
//!   - `end_node`              terminal node
//!   - `function_node`         Function/Plugin call
//!   - `generate_answer_node`  LLM answer generation
//!
//! A [`WorkflowNodeExecutor`] is injected by the caller so the real
//! Function/Plugin/LLM runtimes can be swapped for a recording stub in
//! tests. Remote execution is intentionally absent.

#![warn(missing_docs)]

use std::collections::{HashMap, HashSet, VecDeque};
use std::sync::Arc;

use serde_json::Value;
use thiserror::Error;

use crate::datasource::workflow_store::{NodeType, WorkflowGraph, WorkflowNode};
use crate::runtime::execution::CancelHandle;

/// Errors detected while validating a [`WorkflowGraph`] before execution.
#[derive(Debug, Clone, Error, PartialEq, Eq)]
pub enum WorkflowValidationError {
    /// The graph has no `start_node`.
    #[error("workflow has no start_node")]
    MissingStart,
    /// The graph has no `end_node`.
    #[error("workflow has no end_node")]
    MissingEnd,
    /// An edge references a node key that does not exist.
    #[error("edge references unknown node: {0}")]
    DanglingEdge(String),
    /// The graph contains a directed cycle.
    #[error("workflow contains a cycle through: {0:?}")]
    Cycle(Vec<String>),
    /// A node is not reachable from the `start_node`.
    #[error("node is not reachable from start_node: {0}")]
    UnreachableNode(String),
}

/// Errors surfaced while executing a [`WorkflowGraph`].
#[derive(Debug, Clone, Error, PartialEq, Eq)]
pub enum WorkflowExecutionError {
    /// The graph failed validation before execution.
    #[error("workflow validation failed: {0}")]
    Validation(#[from] WorkflowValidationError),
    /// A node failed; execution is fail-fast so no further node runs.
    #[error("node {0} failed: {1}")]
    NodeFailed(String, String),
    /// Execution was cancelled before producing a terminal outcome.
    #[error("workflow execution was cancelled")]
    Cancelled,
}

/// Result of a single node execution.
#[derive(Debug, Clone)]
pub struct WorkflowNodeResult {
    /// Node key.
    pub node_key: String,
    /// Node output value.
    pub output: Value,
}

/// Overall workflow execution outcome.
#[derive(Debug, Clone)]
pub struct WorkflowExecutionOutcome {
    /// Whether the run reached `end_node` successfully.
    pub completed: bool,
    /// Whether the run was cancelled.
    pub cancelled: bool,
    /// Per-node results in topological order.
    pub node_results: Vec<WorkflowNodeResult>,
    /// The node that failed, when `completed == false` and not cancelled.
    pub failed_node: Option<String>,
}

/// Abstraction over a single node's Function/Plugin/LLM execution. The
/// injected implementation must resolve exactly once to either a JSON
/// output or a failure reason. It MUST observe the [`CancelHandle`] and
/// return promptly once cancellation is signalled so fail-fast and
/// cancel propagation stay bounded.
pub trait WorkflowNodeExecutor: Send + Sync {
    /// Execute one node against the given input. The node is owned so
    /// the returned future is `'static` and can be spawned freely.
    fn execute(
        &self,
        node: WorkflowNode,
        input: Value,
        cancel: CancelHandle,
    ) -> std::pin::Pin<Box<dyn std::future::Future<Output = Result<Value, String>> + Send>>;
}

/// Workflow DAG executor.
pub struct WorkflowExecutor<E> {
    executor: Arc<E>,
}

impl<E: WorkflowNodeExecutor + 'static> WorkflowExecutor<E> {
    /// Build a new executor around the supplied node executor.
    pub fn new(executor: E) -> Self {
        Self {
            executor: Arc::new(executor),
        }
    }

    /// Validate the graph without executing it.
    pub fn validate(&self, graph: &WorkflowGraph) -> Result<(), WorkflowValidationError> {
        let nodes = graph.nodes();
        let edges = graph.edges();

        let keys: HashSet<&str> = nodes.iter().map(|n| n.key()).collect();

        let has_start = nodes.iter().any(|n| n.kind() == NodeType::Start);
        if !has_start {
            return Err(WorkflowValidationError::MissingStart);
        }
        let has_end = nodes.iter().any(|n| n.kind() == NodeType::End);
        if !has_end {
            return Err(WorkflowValidationError::MissingEnd);
        }

        // Dangling edges.
        for edge in edges {
            if !keys.contains(edge.from()) {
                return Err(WorkflowValidationError::DanglingEdge(
                    edge.from().to_string(),
                ));
            }
            if !keys.contains(edge.to()) {
                return Err(WorkflowValidationError::DanglingEdge(edge.to().to_string()));
            }
        }

        // Build the adjacency + in-degree maps.
        let mut adjacency: HashMap<&str, Vec<&str>> = HashMap::new();
        let mut in_degree: HashMap<&str, usize> = nodes.iter().map(|n| (n.key(), 0)).collect();
        for edge in edges {
            adjacency.entry(edge.from()).or_default().push(edge.to());
            *in_degree.entry(edge.to()).or_insert(0) += 1;
        }

        // Kahn topological sort: detects cycles when the processed count
        // is less than the node count.
        let mut queue: VecDeque<&str> = in_degree
            .iter()
            .filter(|(_, deg)| **deg == 0)
            .map(|(key, _)| *key)
            .collect();
        let mut visited = 0usize;
        while let Some(key) = queue.pop_front() {
            visited += 1;
            if let Some(next) = adjacency.get(key) {
                for to in next {
                    let deg = in_degree.get_mut(to).expect("edge target in degree map");
                    *deg = deg.saturating_sub(1);
                    if *deg == 0 {
                        queue.push_back(to);
                    }
                }
            }
        }
        if visited != nodes.len() {
            let cyclic: Vec<String> = in_degree
                .iter()
                .filter(|(_, deg)| **deg > 0)
                .map(|(key, _)| (*key).to_string())
                .collect();
            return Err(WorkflowValidationError::Cycle(cyclic));
        }

        // Reachability from the start node.
        let start_keys: Vec<&str> = nodes
            .iter()
            .filter(|n| n.kind() == NodeType::Start)
            .map(|n| n.key())
            .collect();
        let mut reachable: HashSet<&str> = HashSet::new();
        let mut stack: Vec<&str> = start_keys;
        while let Some(key) = stack.pop() {
            if !reachable.insert(key) {
                continue;
            }
            if let Some(next) = adjacency.get(key) {
                stack.extend(next.iter().copied());
            }
        }
        for node in nodes {
            if !reachable.contains(node.key()) {
                return Err(WorkflowValidationError::UnreachableNode(
                    node.key().to_string(),
                ));
            }
        }

        Ok(())
    }

    /// Execute the graph. Nodes are grouped into topological levels and
    /// run level-by-level; within a level they run concurrently. The
    /// first node failure is fail-fast: it aborts the run with
    /// [`WorkflowExecutionError::NodeFailed`] and no node is retried.
    /// Cancellation is propagated via the shared [`CancelHandle`].
    pub async fn execute(
        &self,
        graph: &WorkflowGraph,
        input: Value,
        cancel: CancelHandle,
    ) -> Result<WorkflowExecutionOutcome, WorkflowExecutionError> {
        self.validate(graph)?;

        if cancel.is_cancelled() {
            return Ok(WorkflowExecutionOutcome {
                completed: false,
                cancelled: true,
                node_results: Vec::new(),
                failed_node: None,
            });
        }

        // Group nodes into topological levels (Kahn's algorithm layered).
        let levels = topological_levels(graph);

        let mut node_results: Vec<WorkflowNodeResult> = Vec::new();

        for level in levels {
            if cancel.is_cancelled() {
                return Ok(WorkflowExecutionOutcome {
                    completed: false,
                    cancelled: true,
                    node_results,
                    failed_node: None,
                });
            }

            let mut set = tokio::task::JoinSet::new();
            for node in level {
                let executor = Arc::clone(&self.executor);
                let input = input.clone();
                let cancel = cancel.clone();
                set.spawn(async move {
                    let node_key = node.key().to_string();
                    let result = executor.execute(node, input, cancel).await;
                    (node_key, result)
                });
            }

            // Collect results; a level is fail-fast on the first error.
            let mut level_failure: Option<(String, String)> = None;
            while let Some(joined) = set.join_next().await {
                // A panic in the executor is treated as an internal failure.
                let (node_key, result) = match joined {
                    Ok((node_key, result)) => (node_key, result),
                    Err(join_err) => {
                        cancel.signal();
                        return Err(WorkflowExecutionError::NodeFailed(
                            "<executor>".to_string(),
                            format!("node task panicked: {join_err}"),
                        ));
                    }
                };
                match result {
                    Ok(output) => {
                        node_results.push(WorkflowNodeResult { node_key, output });
                    }
                    Err(reason) => {
                        if level_failure.is_none() {
                            level_failure = Some((node_key, reason));
                        }
                    }
                }
            }

            if let Some((node_key, reason)) = level_failure {
                cancel.signal();
                return Err(WorkflowExecutionError::NodeFailed(node_key, reason));
            }
        }

        Ok(WorkflowExecutionOutcome {
            completed: true,
            cancelled: false,
            node_results,
            failed_node: None,
        })
    }
}

/// Compute topological levels for a [`WorkflowGraph`]. Assumes the graph
/// has already been validated (acyclic, no dangling edges).
fn topological_levels(graph: &WorkflowGraph) -> Vec<Vec<WorkflowNode>> {
    let nodes = graph.nodes();
    let edges = graph.edges();

    let mut in_degree: HashMap<&str, usize> = nodes.iter().map(|n| (n.key(), 0)).collect();
    let mut adjacency: HashMap<&str, Vec<&str>> = HashMap::new();
    for edge in edges {
        adjacency.entry(edge.from()).or_default().push(edge.to());
        *in_degree.entry(edge.to()).or_insert(0) += 1;
    }

    let mut queue: VecDeque<&str> = in_degree
        .iter()
        .filter(|(_, deg)| **deg == 0)
        .map(|(key, _)| *key)
        .collect();

    let mut levels: Vec<Vec<WorkflowNode>> = Vec::new();
    while !queue.is_empty() {
        let level_keys: Vec<&str> = queue.drain(..).collect();
        let mut level_nodes: Vec<WorkflowNode> = Vec::new();
        for key in &level_keys {
            if let Some(node) = nodes.iter().find(|n| n.key() == *key) {
                level_nodes.push(node.clone());
            }
            if let Some(next) = adjacency.get(key) {
                for to in next {
                    let deg = in_degree.get_mut(to).expect("edge target in degree map");
                    *deg = deg.saturating_sub(1);
                    if *deg == 0 {
                        queue.push_back(to);
                    }
                }
            }
        }
        levels.push(level_nodes);
    }
    levels
}
