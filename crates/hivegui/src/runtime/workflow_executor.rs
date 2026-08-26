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

use std::collections::{BTreeSet, HashMap, HashSet, VecDeque};
use std::sync::Arc;
use std::time::{Duration, Instant};

use hive_runtime_core::execution::{EventSink, ExecutionContext, PermissionSnapshot, RuntimeEvent};
use providers::ChatRequest;
use serde_json::Value;
use thiserror::Error;

use crate::datasource::Crypto;
use crate::datasource::function_store::FunctionStore;
use crate::datasource::llm_provider_store::LlmProviderStore;
use crate::datasource::llm_store::LlmStore;
use crate::datasource::workflow_store::{NodeType, WorkflowGraph, WorkflowNode};
use crate::runtime::execution::CancelHandle;
use crate::runtime::function_test_executor::FunctionTestExecutor;
use crate::runtime::provider_resolver::{
    ProviderCallOutcome, ProviderCallRequest, ProviderResolver, ProviderTransport,
};

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

/// Terminal state of one Workflow node.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum WorkflowNodeStatus {
    /// The node produced an output.
    Completed,
    /// The node returned a failure.
    Failed,
    /// The node exceeded its configured time budget.
    TimedOut,
    /// The node was interrupted by cooperative cancellation.
    Cancelled,
    /// Fail-fast stopped scheduling before this node started.
    NotStarted,
}

/// Terminal state of the complete Workflow run.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum WorkflowRunStatus {
    /// Every node completed.
    Completed,
    /// A node failed or timed out.
    Failed,
    /// Cooperative cancellation interrupted the run.
    Cancelled,
}

/// Complete diagnostic record for one node, including nodes never started.
#[derive(Debug, Clone)]
pub struct WorkflowNodeReport {
    /// Stable node key.
    pub node_key: String,
    /// Terminal node status.
    pub status: WorkflowNodeStatus,
    /// Output for a completed node.
    pub output: Option<Value>,
    /// Stable, sanitized failure text for a failed/interrupted node.
    pub error: Option<String>,
    /// Time spent inside the node executor.
    pub elapsed_ms: u64,
}

/// Complete terminal Workflow report. Node failures are represented here
/// rather than discarding successful siblings from the same parallel layer.
#[derive(Debug, Clone)]
pub struct WorkflowExecutionReport {
    /// Overall run status.
    pub status: WorkflowRunStatus,
    /// Deterministically selected primary failed/interrupted node.
    pub failed_node: Option<String>,
    /// Stable category: `node_failed`, `timeout`, or `cancelled`.
    pub error_category: Option<String>,
    /// Whether any completed node may already have produced an external side
    /// effect that cannot be rolled back automatically.
    pub side_effects_may_have_occurred: bool,
    /// Total scheduler wall-clock duration.
    pub elapsed_ms: u64,
    /// Every graph node in deterministic topological order.
    pub node_results: Vec<WorkflowNodeReport>,
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
        let report = self.execute_report(graph, input, cancel).await?;
        let node_results = report
            .node_results
            .iter()
            .filter_map(|node| {
                node.output.clone().map(|output| WorkflowNodeResult {
                    node_key: node.node_key.clone(),
                    output,
                })
            })
            .collect::<Vec<_>>();
        match report.status {
            WorkflowRunStatus::Completed => Ok(WorkflowExecutionOutcome {
                completed: true,
                cancelled: false,
                node_results,
                failed_node: None,
            }),
            WorkflowRunStatus::Cancelled => {
                if let Some(node_key) = report.failed_node {
                    let reason = report
                        .node_results
                        .iter()
                        .find(|node| node.node_key == node_key)
                        .and_then(|node| node.error.clone())
                        .unwrap_or_else(|| "cancelled".to_string());
                    Err(WorkflowExecutionError::NodeFailed(node_key, reason))
                } else {
                    Ok(WorkflowExecutionOutcome {
                        completed: false,
                        cancelled: true,
                        node_results,
                        failed_node: None,
                    })
                }
            }
            WorkflowRunStatus::Failed => {
                let node_key = report
                    .failed_node
                    .unwrap_or_else(|| "<executor>".to_string());
                let reason = report
                    .node_results
                    .iter()
                    .find(|node| node.node_key == node_key)
                    .and_then(|node| node.error.clone())
                    .unwrap_or_else(|| "node_failed".to_string());
                Err(WorkflowExecutionError::NodeFailed(node_key, reason))
            }
        }
    }

    /// Execute a graph and always return a complete typed terminal report for
    /// node failures and cancellation. Structural validation errors remain
    /// ordinary errors because no node was eligible to run.
    pub async fn execute_report(
        &self,
        graph: &WorkflowGraph,
        input: Value,
        cancel: CancelHandle,
    ) -> Result<WorkflowExecutionReport, WorkflowExecutionError> {
        self.execute_report_inner(graph, input, cancel, None).await
    }

    /// Execute with one per-node timeout. A timed-out node is never retried;
    /// its current parallel layer is fully collected and no later layer starts.
    pub async fn execute_report_with_timeout(
        &self,
        graph: &WorkflowGraph,
        input: Value,
        cancel: CancelHandle,
        timeout: Duration,
    ) -> Result<WorkflowExecutionReport, WorkflowExecutionError> {
        self.execute_report_inner(graph, input, cancel, Some(timeout))
            .await
    }

    async fn execute_report_inner(
        &self,
        graph: &WorkflowGraph,
        input: Value,
        cancel: CancelHandle,
        timeout: Option<Duration>,
    ) -> Result<WorkflowExecutionReport, WorkflowExecutionError> {
        self.validate(graph)?;
        let started = Instant::now();
        let levels = topological_levels(graph);
        let ordered_keys = levels
            .iter()
            .flatten()
            .map(|node| node.key().to_string())
            .collect::<Vec<_>>();
        let mut reports = ordered_keys
            .iter()
            .map(|node_key| WorkflowNodeReport {
                node_key: node_key.clone(),
                status: WorkflowNodeStatus::NotStarted,
                output: None,
                error: None,
                elapsed_ms: 0,
            })
            .collect::<Vec<_>>();
        let report_index = reports
            .iter()
            .enumerate()
            .map(|(index, report)| (report.node_key.clone(), index))
            .collect::<HashMap<_, _>>();
        let mut outputs = HashMap::<String, Value>::new();

        if cancel.is_cancelled() {
            return Ok(WorkflowExecutionReport {
                status: WorkflowRunStatus::Cancelled,
                failed_node: None,
                error_category: Some("cancelled".to_string()),
                side_effects_may_have_occurred: false,
                elapsed_ms: elapsed_millis(started),
                node_results: reports,
            });
        }

        for level in levels {
            if cancel.is_cancelled() {
                return Ok(finish_report(
                    WorkflowRunStatus::Cancelled,
                    None,
                    "cancelled",
                    started,
                    reports,
                ));
            }

            let mut set = tokio::task::JoinSet::new();
            for node in level {
                let node_key = node.key().to_string();
                let node_input = mapped_node_input(&node, &input, &outputs);
                let executor = Arc::clone(&self.executor);
                let node_cancel = cancel.clone();
                set.spawn(async move {
                    let node_started = Instant::now();
                    let result = match node_input {
                        Ok(node_input) => match timeout {
                            Some(timeout) => match tokio::time::timeout(
                                timeout,
                                executor.execute(node, node_input, node_cancel),
                            )
                            .await
                            {
                                Ok(result) => NodeInvocation::Finished(result),
                                Err(_) => NodeInvocation::TimedOut,
                            },
                            None => NodeInvocation::Finished(
                                executor.execute(node, node_input, node_cancel).await,
                            ),
                        },
                        Err(reason) => NodeInvocation::Finished(Err(reason)),
                    };
                    (node_key, result, elapsed_millis(node_started))
                });
            }

            let mut terminal = Vec::<(String, WorkflowNodeStatus)>::new();
            while let Some(joined) = set.join_next().await {
                let (node_key, invocation, elapsed_ms) = joined.map_err(|join_error| {
                    WorkflowExecutionError::NodeFailed(
                        "<executor>".to_string(),
                        format!("node task panicked: {join_error}"),
                    )
                })?;
                let index = *report_index
                    .get(&node_key)
                    .expect("every scheduled node has a report slot");
                let node_report = &mut reports[index];
                node_report.elapsed_ms = elapsed_ms;
                match invocation {
                    NodeInvocation::Finished(Ok(output)) => {
                        node_report.status = WorkflowNodeStatus::Completed;
                        node_report.output = Some(output.clone());
                        outputs.insert(node_key, output);
                    }
                    NodeInvocation::Finished(Err(reason)) => {
                        let status = if reason == "cancelled" && cancel.is_cancelled() {
                            WorkflowNodeStatus::Cancelled
                        } else {
                            WorkflowNodeStatus::Failed
                        };
                        node_report.status = status;
                        node_report.error = Some(reason);
                        terminal.push((node_key, status));
                    }
                    NodeInvocation::TimedOut => {
                        node_report.status = WorkflowNodeStatus::TimedOut;
                        node_report.error = Some("timeout".to_string());
                        terminal.push((node_key, WorkflowNodeStatus::TimedOut));
                    }
                }
            }

            if !terminal.is_empty() {
                terminal.sort_by(|left, right| left.0.cmp(&right.0));
                let cancelled = cancel.is_cancelled()
                    && terminal
                        .iter()
                        .any(|(_, status)| *status == WorkflowNodeStatus::Cancelled);
                cancel.signal();
                let (status, category) = if cancelled {
                    (WorkflowRunStatus::Cancelled, "cancelled")
                } else if terminal
                    .iter()
                    .any(|(_, status)| *status == WorkflowNodeStatus::TimedOut)
                {
                    (WorkflowRunStatus::Failed, "timeout")
                } else {
                    (WorkflowRunStatus::Failed, "node_failed")
                };
                let failed_node = terminal
                    .iter()
                    .find(|(_, node_status)| match category {
                        "cancelled" => *node_status == WorkflowNodeStatus::Cancelled,
                        "timeout" => *node_status == WorkflowNodeStatus::TimedOut,
                        _ => *node_status == WorkflowNodeStatus::Failed,
                    })
                    .map(|(node_key, _)| node_key.clone());
                return Ok(finish_report(
                    status,
                    failed_node,
                    category,
                    started,
                    reports,
                ));
            }
        }

        Ok(WorkflowExecutionReport {
            status: WorkflowRunStatus::Completed,
            failed_node: None,
            error_category: None,
            side_effects_may_have_occurred: !reports.is_empty(),
            elapsed_ms: elapsed_millis(started),
            node_results: reports,
        })
    }
}

enum NodeInvocation {
    Finished(Result<Value, String>),
    TimedOut,
}

fn finish_report(
    status: WorkflowRunStatus,
    failed_node: Option<String>,
    category: &str,
    started: Instant,
    reports: Vec<WorkflowNodeReport>,
) -> WorkflowExecutionReport {
    let side_effects_may_have_occurred = reports
        .iter()
        .any(|node| node.status == WorkflowNodeStatus::Completed);
    WorkflowExecutionReport {
        status,
        failed_node,
        error_category: Some(category.to_string()),
        side_effects_may_have_occurred,
        elapsed_ms: elapsed_millis(started),
        node_results: reports,
    }
}

fn elapsed_millis(started: Instant) -> u64 {
    started.elapsed().as_millis().try_into().unwrap_or(u64::MAX)
}

fn mapped_node_input(
    node: &WorkflowNode,
    root_input: &Value,
    outputs: &HashMap<String, Value>,
) -> Result<Value, String> {
    let Some(mapping) = node
        .node_config()
        .and_then(|config| config.get("input_mapping"))
    else {
        return Ok(root_input.clone());
    };
    let entries = mapping
        .as_object()
        .ok_or_else(|| "input_mapping must be an object".to_string())?;
    let mut input = serde_json::Map::new();
    for (field, source) in entries {
        let kind = source
            .get("kind")
            .and_then(Value::as_str)
            .ok_or_else(|| format!("input_mapping.{field} missing kind"))?;
        let value = match kind {
            "upstream" => {
                let node_key = source
                    .get("node_key")
                    .and_then(Value::as_str)
                    .ok_or_else(|| format!("input_mapping.{field} missing node_key"))?;
                let output = outputs
                    .get(node_key)
                    .ok_or_else(|| format!("input_mapping.{field} upstream output unavailable"))?;
                let path = source
                    .get("path")
                    .or_else(|| source.get("field"))
                    .and_then(Value::as_str);
                match path {
                    Some(path) if !path.is_empty() => json_path(output, path).ok_or_else(|| {
                        format!("input_mapping.{field} upstream path unavailable")
                    })?,
                    _ => output.clone(),
                }
            }
            "custom" => source
                .get("value")
                .cloned()
                .ok_or_else(|| format!("input_mapping.{field} missing custom value"))?,
            _ => return Err(format!("input_mapping.{field} has unsupported kind")),
        };
        input.insert(field.clone(), value);
    }
    Ok(Value::Object(input))
}

fn json_path(value: &Value, path: &str) -> Option<Value> {
    path.split('.')
        .try_fold(value, |current, segment| current.get(segment))
        .cloned()
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

    let mut queue: VecDeque<&str> = {
        let mut ready = in_degree
            .iter()
            .filter(|(_, deg)| **deg == 0)
            .map(|(key, _)| *key)
            .collect::<Vec<_>>();
        ready.sort_unstable();
        ready.into()
    };

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
        level_nodes.sort_by(|left, right| left.key().cmp(right.key()));
        let mut next = queue.drain(..).collect::<Vec<_>>();
        next.sort_unstable();
        queue.extend(next);
        levels.push(level_nodes);
    }
    levels
}

/// Production [`WorkflowNodeExecutor`] backed by the local HiveGUI runtime.
///
/// Node execution semantics (T098):
///   - `start_node` / `end_node` — pass the input through unchanged.
///   - `function_node` — resolve the referenced
///     [`FunctionRecord`](crate::datasource::function_store::FunctionRecord) by `function_id`
///     and run it through [`FunctionTestExecutor::execute_with_capabilities`],
///     the same controlled no-follow plugin path used by the Function test
///     dialog. Its `required_capabilities` are resolved into the capability
///     snapshot for the run.
///   - `generate_answer_node` — LLM answer generation through the production
///     [`ProviderResolver`] (T052). The node's `node_config.model_preset`
///     selects the local Preset; an empty value resolves the unique default
///     Preset. The node input JSON becomes the user message.
///
/// The executor observes the cooperative [`CancelHandle`] before dispatch; a
/// plugin call still runs to its 30s timeout (the Function executor has no
/// mid-flight cancel today) but the engine's fail-fast scheduling stops any
/// further level from starting once cancelled.
pub struct LocalWorkflowNodeExecutor {
    pool: sqlx::Pool<sqlx::Sqlite>,
    plugin_root: std::path::PathBuf,
    crypto: Crypto,
    transport: Option<Arc<dyn ProviderTransport>>,
    capabilities: Vec<String>,
}

impl LocalWorkflowNodeExecutor {
    /// Build a new executor. `plugin_root` is `Store::plugin_root()` (the
    /// managed plugin artifact root, `{data_root}/plugins`); `crypto` is the
    /// device-key handle used to resolve provider tokens for LLM nodes.
    pub fn new(
        pool: sqlx::Pool<sqlx::Sqlite>,
        plugin_root: impl Into<std::path::PathBuf>,
        crypto: Crypto,
    ) -> Self {
        Self {
            pool,
            plugin_root: plugin_root.into(),
            crypto,
            transport: None,
            capabilities: Vec::new(),
        }
    }

    /// Bind the immutable caller Capability snapshot to every Function and
    /// provider node in this Workflow execution. The default is deny-all;
    /// Function metadata never grants itself additional capabilities.
    pub fn with_capabilities(mut self, capabilities: impl IntoIterator<Item = String>) -> Self {
        self.capabilities = capabilities
            .into_iter()
            .collect::<BTreeSet<_>>()
            .into_iter()
            .collect();
        self
    }

    /// Override the provider transport used for `generate_answer_node`
    /// calls. This legacy deterministic seam is only for the already-approved
    /// T048/Workflow tests; production always uses the workspace `providers`
    /// chain selected by the local Preset.
    pub fn with_provider_transport(mut self, transport: Arc<dyn ProviderTransport>) -> Self {
        self.transport = Some(transport);
        self
    }
}

impl WorkflowNodeExecutor for LocalWorkflowNodeExecutor {
    fn execute(
        &self,
        node: WorkflowNode,
        input: Value,
        cancel: CancelHandle,
    ) -> std::pin::Pin<Box<dyn std::future::Future<Output = Result<Value, String>> + Send>> {
        let pool = self.pool.clone();
        let plugin_root = self.plugin_root.clone();
        let crypto = self.crypto.clone();
        let transport = self.transport.clone();
        let capabilities = self.capabilities.clone();
        Box::pin(async move {
            if cancel.is_cancelled() {
                return Err("cancelled".to_string());
            }
            match node.kind() {
                NodeType::Start | NodeType::End => Ok(input),
                NodeType::Function => {
                    let function_id = node.function_id().ok_or_else(|| {
                        "function_node 未关联函数（function_id 缺失）".to_string()
                    })?;
                    let function = FunctionStore::new(pool.clone())
                        .map_err(|e| format!("打开函数存储失败: {e}"))?
                        .get(function_id)
                        .await
                        .map_err(|e| format!("读取函数记录失败: {e}"))?
                        .ok_or_else(|| "函数记录不存在".to_string())?
                        .into_legacy_entity();
                    let executor = FunctionTestExecutor::new(plugin_root, pool.clone());
                    let output = executor
                        .execute_with_capabilities(&function, input, capabilities.clone())
                        .await?;
                    // Normalise the display string back to a JSON value when
                    // possible so downstream nodes receive structured output.
                    match serde_json::from_str::<Value>(&output) {
                        Ok(value) => Ok(value),
                        Err(_) => Ok(Value::String(output)),
                    }
                }
                NodeType::GenerateAnswer => {
                    if let Some(transport) = transport {
                        // Historical deterministic test seam. Production does
                        // not enter this branch and therefore cannot bypass the
                        // workspace provider builders.
                        let model = match node
                            .node_config()
                            .and_then(|config| config.get("model"))
                            .and_then(Value::as_str)
                        {
                            Some(model) => model.to_string(),
                            None => {
                                LlmStore::new(pool.clone(), crypto.clone())
                                    .default_model_name()
                                    .await
                                    .map_err(|_| "解析默认模型失败".to_string())?
                                    .ok_or_else(|| {
                                        "generate_answer_node 未配置 model，且无默认模型（默认 preset 下无 model）"
                                            .to_string()
                                    })?
                            }
                        };
                        let store = LlmProviderStore::from_crypto(pool, crypto)
                            .await
                            .map_err(|_| "初始化 LLM Provider 存储失败".to_string())?;
                        let resolver = ProviderResolver::new(store, transport)
                            .map_err(|_| "初始化 ProviderResolver 失败".to_string())?;
                        let request = ProviderCallRequest::single_message(model, input.to_string());
                        return match resolver.call(&request).await {
                            Ok(ProviderCallOutcome::Success { content, .. }) => {
                                Ok(Value::String(content))
                            }
                            Err(_) => Err("generate_answer_node LLM 调用失败".to_string()),
                        };
                    }

                    let llm_store = LlmStore::new(pool.clone(), crypto.clone());
                    let preset = match node
                        .node_config()
                        .and_then(|config| config.get("model_preset"))
                        .and_then(Value::as_str)
                        .filter(|preset| !preset.trim().is_empty())
                    {
                        Some(preset) => preset.trim().to_string(),
                        None => llm_store
                            .default_preset_name()
                            .await
                            .map_err(|_| "解析默认 Preset 失败".to_string())?
                            .ok_or_else(|| "generate_answer_node 无默认 Preset".to_string())?,
                    };
                    let resolver = ProviderResolver::from_local_config(pool, crypto)
                        .map_err(|_| "初始化 ProviderResolver 失败".to_string())?;
                    let request = ProviderCallRequest::for_preset(
                        preset,
                        ChatRequest {
                            messages: vec![serde_json::json!({
                                "role": "user",
                                "content": input.to_string()
                            })],
                            ..Default::default()
                        },
                    );
                    let context = ExecutionContext::new(
                        uuid::Uuid::now_v7().to_string(),
                        uuid::Uuid::now_v7().to_string(),
                        "workflow-generate-answer",
                        PermissionSnapshot::new(capabilities.iter().cloned().collect()),
                        Arc::new(WorkflowEventSink),
                    )
                    .map_err(|_| "初始化 LLM execution context 失败".to_string())?;
                    let provider_call = resolver.call_streaming(&request, &context);
                    tokio::pin!(provider_call);
                    let outcome = tokio::select! {
                        result = &mut provider_call => result,
                        _ = wait_for_workflow_cancel(cancel) => {
                            context.cancel_with_reason("workflow_stop");
                            provider_call.await
                        }
                    };
                    match outcome {
                        Ok(ProviderCallOutcome::Success { content, .. }) => {
                            Ok(Value::String(content))
                        }
                        Err(error)
                            if matches!(
                                error.kind(),
                                crate::runtime::provider_resolver::ProviderErrorKind::Cancelled { .. }
                            ) =>
                        {
                            Err("cancelled".to_string())
                        }
                        Err(_) => Err("generate_answer_node LLM 调用失败".to_string()),
                    }
                }
            }
        })
    }
}

struct WorkflowEventSink;

impl EventSink for WorkflowEventSink {
    fn emit(&self, _event: RuntimeEvent) {}
}

async fn wait_for_workflow_cancel(cancel: CancelHandle) {
    while !cancel.is_cancelled() {
        tokio::time::sleep(Duration::from_millis(2)).await;
    }
}
