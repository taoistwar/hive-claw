//! Storage-independent Workflow graph contracts and pure scheduling algorithms.
//!
//! Workflow definitions use the stable node types `start_node`, `end_node`,
//! `function_node`, and `generate_answer_node`. Graph validation, topological
//! layering, fail-fast result aggregation, and cancellation checkpoints belong
//! here; loading records and executing product resources remain adapter duties.
//!
//! T020 implements:
//! - [`NodeType`] round-trip serialisation to the four stable persisted values.
//!   Shorter aliases (e.g. `start`, `end`) are read/write errors that surface
//!   as [`FieldError`].
//! - [`WorkflowGraph::validate`] covers every structural violation documented
//!   in `tests/workflow_contract.rs` Red contract: duplicate node keys,
//!   missing edge endpoints, self edges, cycles, start-count violations, and
//!   unreachable end nodes.
//! - [`ValidatedWorkflow::topological_layers`] produces deterministic parallel
//!   layers via Kahn's algorithm; within a layer nodes are sorted by key.
//! - [`ValidatedWorkflow::summarize_layer`] performs fail-fast aggregation
//!   and selects the smallest-keyed failure as the primary failure.

#![forbid(unsafe_code)]
#![warn(missing_docs)]

use std::collections::{BTreeMap, BTreeSet, HashMap, HashSet};
use std::fmt;

/// Stable persisted node-type identifiers used in workflow JSON records.
///
/// The wire format is the snake_case string (e.g. `start_node`). The
/// contract accepts only these four values; shorter aliases are read
/// and write errors that surface as [`FieldError`].
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum NodeType {
    /// Entry node of the workflow; every workflow has exactly one.
    Start,
    /// Terminal node of the workflow; every workflow has exactly one.
    End,
    /// A node that executes a registered function.
    Function,
    /// A node that drives an LLM `generate_answer` step.
    GenerateAnswer,
}

impl NodeType {
    /// Stable persisted string form.
    ///
    /// Inputs: none (uses `self`).
    /// Outputs: the canonical snake-case persisted name.
    /// Error modes: this method is total.
    pub fn as_str(self) -> &'static str {
        match self {
            NodeType::Start => "start_node",
            NodeType::End => "end_node",
            NodeType::Function => "function_node",
            NodeType::GenerateAnswer => "generate_answer_node",
        }
    }

    /// Parse a persisted node-type identifier.
    ///
    /// Inputs:
    /// - `value`: the snake-case string from a workflow record.
    ///
    /// Outputs:
    /// - The corresponding [`NodeType`] variant.
    ///
    /// Error modes:
    /// - [`FieldError`] with `field() == "node_type"` and
    ///   `reason() == "unsupported_value"` for any value outside the
    ///   closed set of four persisted strings. The display form
    ///   intentionally omits the rejected value so the error can be
    ///   surfaced to operators without leaking the input.
    pub fn parse(value: &str) -> Result<Self, FieldError> {
        match value {
            "start_node" => Ok(NodeType::Start),
            "end_node" => Ok(NodeType::End),
            "function_node" => Ok(NodeType::Function),
            "generate_answer_node" => Ok(NodeType::GenerateAnswer),
            _ => Err(FieldError::new("node_type", "unsupported_value")),
        }
    }
}

/// Error returned by [`NodeType::parse`] for malformed or unsupported
/// persisted values.
///
/// The [`Display`](fmt::Display) form omits the rejected value so the
/// error can be safely surfaced to operators.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FieldError {
    field: &'static str,
    reason: &'static str,
}

impl FieldError {
    /// Build a new field error. Only the closed set of `(field, reason)`
    /// pairs documented in the workflow contract is allowed.
    pub(crate) fn new(field: &'static str, reason: &'static str) -> Self {
        Self { field, reason }
    }

    /// The structured field name (e.g. `node_type`).
    ///
    /// Inputs: none.
    /// Outputs: the canonical field identifier.
    /// Error modes: this method is total.
    pub fn field(&self) -> &'static str {
        self.field
    }

    /// The stable error reason (e.g. `unsupported_value`).
    ///
    /// Inputs: none.
    /// Outputs: the canonical reason identifier.
    /// Error modes: this method is total.
    pub fn reason(&self) -> &'static str {
        self.reason
    }
}

impl fmt::Display for FieldError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            formatter,
            "invalid value for field `{}`: {}",
            self.field, self.reason
        )
    }
}

impl std::error::Error for FieldError {}

/// A single node declared in a [`WorkflowGraph`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WorkflowNode {
    /// Unique key identifying the node within the graph.
    pub node_key: String,
    /// The node type.
    pub node_type: NodeType,
    /// Function identifier when the node is a `Function`; `None` for
    /// other node types.
    pub function_id: Option<String>,
}

/// A directed edge between two nodes.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WorkflowEdge {
    /// Source node key.
    pub source: String,
    /// Target node key.
    pub target: String,
}

/// A graph declaration as authored by the product.
///
/// The struct stores the raw declaration only; structural invariants
/// are enforced by [`WorkflowGraph::validate`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WorkflowGraph {
    nodes: Vec<WorkflowNode>,
    edges: Vec<WorkflowEdge>,
}

impl WorkflowGraph {
    /// Build a new graph from a node and edge list.
    ///
    /// Inputs:
    /// - `nodes`: the declared nodes.
    /// - `edges`: the declared edges.
    ///
    /// Outputs: a [`WorkflowGraph`] ready for [`validate`](WorkflowGraph::validate).
    /// Error modes: this constructor is total; structural validation
    /// is deferred to [`validate`](WorkflowGraph::validate).
    pub fn new(nodes: Vec<WorkflowNode>, edges: Vec<WorkflowEdge>) -> Self {
        Self { nodes, edges }
    }

    /// Borrow the declared nodes.
    ///
    /// Inputs: none.
    /// Outputs: the slice of [`WorkflowNode`]s in declaration order.
    /// Error modes: this method is total.
    pub fn nodes(&self) -> &[WorkflowNode] {
        &self.nodes
    }

    /// Borrow the declared edges.
    ///
    /// Inputs: none.
    /// Outputs: the slice of [`WorkflowEdge`]s in declaration order.
    /// Error modes: this method is total.
    pub fn edges(&self) -> &[WorkflowEdge] {
        &self.edges
    }

    /// Validate the structural invariants of the graph.
    ///
    /// Validation order is fixed and matches the workflow contract:
    /// 1. duplicate node keys
    /// 2. missing edge endpoints
    /// 3. self edges
    /// 4. cycle detection (Kahn's algorithm; `visited != nodes.len()` ⇒ cycle)
    /// 5. start-count check (must be exactly one)
    /// 6. end reachability from start
    ///
    /// On success returns a [`ValidatedWorkflow`] that can be used for
    /// deterministic scheduling. On failure returns a
    /// [`WorkflowValidationError`] whose
    /// [`kind`](WorkflowValidationError::kind) identifies the first
    /// structural violation detected.
    pub fn validate(&self) -> Result<ValidatedWorkflow, WorkflowValidationError> {
        // 1. Duplicate node keys.
        let mut seen_keys: HashSet<&str> = HashSet::new();
        for node in &self.nodes {
            if !seen_keys.insert(node.node_key.as_str()) {
                return Err(WorkflowValidationError::new(
                    WorkflowValidationKind::DuplicateNodeKey,
                ));
            }
        }

        // 2. Missing edge endpoints.
        let key_set: HashSet<&str> = self.nodes.iter().map(|n| n.node_key.as_str()).collect();
        for edge in &self.edges {
            if !key_set.contains(edge.source.as_str()) || !key_set.contains(edge.target.as_str()) {
                return Err(WorkflowValidationError::new(
                    WorkflowValidationKind::MissingEndpoint,
                ));
            }
        }

        // 3. Self edges.
        for edge in &self.edges {
            if edge.source == edge.target {
                return Err(WorkflowValidationError::new(
                    WorkflowValidationKind::SelfEdge,
                ));
            }
        }

        // 4. Build adjacency and in-degree maps for cycle detection / layers.
        let mut outgoing: HashMap<&str, Vec<&str>> = HashMap::new();
        let mut in_degree: HashMap<&str, usize> = HashMap::new();
        for node in &self.nodes {
            in_degree.entry(node.node_key.as_str()).or_insert(0);
            outgoing.entry(node.node_key.as_str()).or_default();
        }
        for edge in &self.edges {
            *in_degree.entry(edge.target.as_str()).or_insert(0) += 1;
            outgoing
                .entry(edge.source.as_str())
                .or_default()
                .push(edge.target.as_str());
        }

        // 5. Start count must be exactly one.
        let starts: Vec<&str> = self
            .nodes
            .iter()
            .filter(|n| n.node_type == NodeType::Start)
            .map(|n| n.node_key.as_str())
            .collect();
        if starts.len() != 1 {
            return Err(WorkflowValidationError::new(
                WorkflowValidationKind::StartCount,
            ));
        }
        let start_key = starts[0];

        // 6. Cycle detection + topological layers via Kahn's algorithm.
        let mut layers: Vec<Vec<String>> = Vec::new();
        let mut current_in_degree: HashMap<&str, usize> = in_degree.clone();
        let mut ready: Vec<String> = current_in_degree
            .iter()
            .filter_map(|(k, d)| if *d == 0 { Some((*k).to_owned()) } else { None })
            .collect();
        ready.sort();
        let mut visited: usize = 0;
        while !ready.is_empty() {
            let layer = std::mem::take(&mut ready);
            visited += layer.len();
            let mut next_set: BTreeSet<&str> = BTreeSet::new();
            for node in &layer {
                if let Some(targets) = outgoing.get(node.as_str()) {
                    for target in targets {
                        if let Some(deg) = current_in_degree.get_mut(*target) {
                            *deg = deg.saturating_sub(1);
                            if *deg == 0 {
                                next_set.insert(*target);
                            }
                        }
                    }
                }
            }
            let mut next: Vec<String> = next_set.into_iter().map(String::from).collect();
            next.sort();
            layers.push(layer);
            ready = next;
        }
        if visited != self.nodes.len() {
            return Err(WorkflowValidationError::new(WorkflowValidationKind::Cycle));
        }

        // 7. End reachability: at least one End node must be reachable
        //    from the start. Use BFS over the adjacency map.
        let mut reachable: HashSet<&str> = HashSet::new();
        let mut stack: Vec<&str> = vec![start_key];
        while let Some(node) = stack.pop() {
            if !reachable.insert(node) {
                continue;
            }
            if let Some(targets) = outgoing.get(node) {
                for target in targets {
                    stack.push(*target);
                }
            }
        }
        let end_reachable = self
            .nodes
            .iter()
            .filter(|n| n.node_type == NodeType::End)
            .any(|n| reachable.contains(n.node_key.as_str()));
        if !end_reachable {
            return Err(WorkflowValidationError::new(
                WorkflowValidationKind::UnreachableEnd,
            ));
        }

        // Snapshot dependants (own outgoing edges) per node for summarize_layer.
        let mut dependants: BTreeMap<String, BTreeSet<String>> = BTreeMap::new();
        for node in &self.nodes {
            let targets = outgoing
                .get(node.node_key.as_str())
                .map(|v| v.iter().map(|s| (*s).to_owned()).collect::<BTreeSet<_>>())
                .unwrap_or_default();
            dependants.insert(node.node_key.clone(), targets);
        }

        let node_keys: BTreeSet<String> = self.nodes.iter().map(|n| n.node_key.clone()).collect();

        Ok(ValidatedWorkflow {
            layers,
            dependants,
            node_keys,
        })
    }
}

/// Outcome categories returned from
/// [`ValidatedWorkflow::summarize_layer`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum LayerNodeOutcome {
    /// The node completed successfully within its time budget.
    Completed,
    /// The node failed; [`NodeFailure`] carries the structured cause.
    Failed(NodeFailure),
}

/// Structured failure of a single node inside a layer.
///
/// `node_key` identifies the node inside the graph; `kind` is the
/// stable error category (e.g. `capability_denied`, `plugin_timeout`);
/// `elapsed_ms` is the wall-clock duration spent in the node before
/// the failure was observed.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NodeFailure {
    /// Key of the failed node.
    pub node_key: String,
    /// Stable error category.
    pub kind: String,
    /// Wall-clock duration spent in the node before the failure.
    pub elapsed_ms: u64,
}

/// Aggregate summary produced by
/// [`ValidatedWorkflow::summarize_layer`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LayerSummary {
    /// `true` when the layer contains at least one failure and the
    /// scheduler must stop before the next layer.
    pub stop_before_next_layer: bool,
    /// The smallest-keyed failed node in the layer, if any.
    pub primary_failure: Option<NodeFailure>,
    /// Sorted list of failed node keys.
    pub failed_nodes: Vec<String>,
    /// Sorted list of node keys that have not yet been started.
    pub unstarted_nodes: Vec<String>,
    /// Sorted list of node keys that completed (across `completed_before`
    /// and the current layer's `Completed` outcomes).
    pub completed_nodes: Vec<String>,
    /// Sorted list of node keys that become runnable once the current
    /// layer completes successfully.
    pub ready_next: Vec<String>,
}

/// A [`WorkflowGraph`] that has cleared every structural invariant and
/// is ready for deterministic scheduling.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ValidatedWorkflow {
    /// Pre-computed parallel layers (each layer sorted by node key).
    layers: Vec<Vec<String>>,
    /// Outgoing edges keyed by source node key.
    dependants: BTreeMap<String, BTreeSet<String>>,
    /// Set of all node keys in the graph.
    node_keys: BTreeSet<String>,
}

impl ValidatedWorkflow {
    /// The deterministic parallel layers.
    ///
    /// Inputs: none.
    /// Outputs: a slice of layers; each layer is a slice of node keys
    /// sorted by key.
    /// Error modes: this method is total.
    pub fn topological_layers(&self) -> &[Vec<String>] {
        &self.layers
    }

    /// Summarise the outcome of a single parallel layer.
    ///
    /// Inputs:
    /// - `completed_before`: node keys that completed in earlier layers.
    /// - `current_layer`: per-node outcome for the layer that just
    ///   finished executing. The map key is the node key.
    ///
    /// Outputs:
    /// - A [`LayerSummary`] classifying the layer. The summary is
    ///   deterministic: the primary failure is the smallest-keyed
    ///   failed node, and all collections are sorted by node key.
    ///
    /// Error modes: this method is total.
    pub fn summarize_layer(
        &self,
        completed_before: &BTreeSet<String>,
        current_layer: &BTreeMap<String, LayerNodeOutcome>,
    ) -> LayerSummary {
        let mut failed_nodes: Vec<String> = Vec::new();
        let mut primary_failure: Option<NodeFailure> = None;
        let mut completed_in_layer: BTreeSet<String> = BTreeSet::new();

        for (key, outcome) in current_layer {
            match outcome {
                LayerNodeOutcome::Completed => {
                    completed_in_layer.insert(key.clone());
                }
                LayerNodeOutcome::Failed(failure) => {
                    failed_nodes.push(key.clone());
                    let take = match &primary_failure {
                        None => true,
                        Some(existing) => key < &existing.node_key,
                    };
                    if take {
                        primary_failure = Some(NodeFailure {
                            node_key: key.clone(),
                            kind: failure.kind.clone(),
                            elapsed_ms: failure.elapsed_ms,
                        });
                    }
                }
            }
        }

        failed_nodes.sort();

        let stop_before_next_layer = !failed_nodes.is_empty();

        // completed_nodes = completed_before ∪ completed_in_layer, sorted + dedup.
        let mut completed_nodes: Vec<String> = completed_before.iter().cloned().collect();
        completed_nodes.extend(completed_in_layer.iter().cloned());
        completed_nodes.sort();
        completed_nodes.dedup();

        // ready_next is only computed when the layer succeeded. On
        // fail-fast we surface the unstarted set as every node that
        // has not yet been observed.
        let mut ready_next: Vec<String> = Vec::new();
        if !stop_before_next_layer {
            let mut done: BTreeSet<String> = completed_before.clone();
            done.extend(completed_in_layer.iter().cloned());
            let mut ready_set: BTreeSet<String> = BTreeSet::new();
            for key in &done {
                if let Some(targets) = self.dependants.get(key) {
                    for target in targets {
                        if !done.contains(target) && !current_layer.contains_key(target) {
                            ready_set.insert(target.clone());
                        }
                    }
                }
            }
            ready_next = ready_set.into_iter().collect();
            ready_next.sort();
        }

        // unstarted_nodes = nodes not in completed_before, not in
        // current_layer, and not in ready_next.
        let unstarted_nodes: Vec<String> = self
            .node_keys
            .iter()
            .filter(|k| {
                !completed_before.contains(*k)
                    && !current_layer.contains_key(*k)
                    && !ready_next.contains(k)
            })
            .cloned()
            .collect();

        LayerSummary {
            stop_before_next_layer,
            primary_failure,
            failed_nodes,
            unstarted_nodes,
            completed_nodes,
            ready_next,
        }
    }
}

/// Stable categories of structural validation errors.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum WorkflowValidationKind {
    /// Two nodes share the same `node_key`.
    DuplicateNodeKey,
    /// An edge references a node that is not declared in the graph.
    MissingEndpoint,
    /// An edge whose `source` equals its `target`.
    SelfEdge,
    /// The graph contains a directed cycle.
    Cycle,
    /// The graph has zero or more than one `Start` node.
    StartCount,
    /// No `End` node is reachable from the `Start` node.
    UnreachableEnd,
}

/// Aggregate returned by [`WorkflowGraph::validate`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WorkflowValidationError {
    kind: WorkflowValidationKind,
}

impl WorkflowValidationError {
    /// Build a new validation error. Only the closed set of
    /// [`WorkflowValidationKind`] variants is allowed.
    pub(crate) fn new(kind: WorkflowValidationKind) -> Self {
        Self { kind }
    }

    /// The stable error category.
    ///
    /// Inputs: none.
    /// Outputs: the [`WorkflowValidationKind`] variant.
    /// Error modes: this method is total.
    pub fn kind(&self) -> WorkflowValidationKind {
        self.kind
    }
}

impl fmt::Display for WorkflowValidationError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        let label = match self.kind {
            WorkflowValidationKind::DuplicateNodeKey => "duplicate node key",
            WorkflowValidationKind::MissingEndpoint => "edge references a missing node",
            WorkflowValidationKind::SelfEdge => "self edge",
            WorkflowValidationKind::Cycle => "directed cycle detected",
            WorkflowValidationKind::StartCount => "graph must have exactly one start node",
            WorkflowValidationKind::UnreachableEnd => "end node is not reachable from start",
        };
        write!(formatter, "workflow validation failed: {label}")
    }
}

impl std::error::Error for WorkflowValidationError {}
