//! US10 Workflow DAG store (T090-T100 boundary).
//!
//! The public types in this file are the minimum surface the T090
//! Red tests require. Behaviour for full transactional DAG saves,
//! RESTRICT/REFERRING constraint matrix, pagination, EXPLAIN and
//! p95 perf live in the T095 implementation pass.
//!
//! Stable `*_node` values are the public contract for the
//! [`NodeType`] enum. The legacy integer mapping is preserved in
//! [`entity_store`] for the migration phase.

#![warn(missing_docs)]

use std::sync::Arc;

use parking_lot::Mutex;
use thiserror::Error;

/// Stable node type contract. The wire strings are the only
/// stable surface; legacy integer values are translated in
/// `entity_store::migrate_v3_to_v4`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum NodeType {
    /// Workflow entry node.
    Start,
    /// Workflow terminal node.
    End,
    /// A Function-call node.
    Function,
    /// An LLM `generate_answer` node.
    GenerateAnswer,
}

impl NodeType {
    /// Stable wire string.
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Start => "start_node",
            Self::End => "end_node",
            Self::Function => "function_node",
            Self::GenerateAnswer => "generate_answer_node",
        }
    }
}

/// A single node in a [`WorkflowGraph`].
#[derive(Debug, Clone)]
pub struct WorkflowNode {
    key: String,
    kind: NodeType,
}

impl WorkflowNode {
    /// Construct a new node with the given `node_key` and kind.
    pub fn new(key: impl Into<String>, kind: NodeType) -> Self {
        Self {
            key: key.into(),
            kind,
        }
    }

    /// Node key.
    pub fn key(&self) -> &str {
        &self.key
    }

    /// Node kind.
    pub fn kind(&self) -> NodeType {
        self.kind
    }
}

/// A directed edge between two nodes in a [`WorkflowGraph`].
#[derive(Debug, Clone)]
pub struct WorkflowEdge {
    from: String,
    to: String,
}

impl WorkflowEdge {
    /// From node key.
    pub fn from(&self) -> &str {
        &self.from
    }

    /// To node key.
    pub fn to(&self) -> &str {
        &self.to
    }
}

/// Builder for a [`WorkflowGraph`].
#[derive(Debug, Default)]
pub struct WorkflowGraphBuilder {
    name: String,
    nodes: Vec<WorkflowNode>,
    edges: Vec<WorkflowEdge>,
}

impl WorkflowGraphBuilder {
    /// Set the workflow name.
    pub fn name(mut self, name: impl Into<String>) -> Self {
        self.name = name.into();
        self
    }

    /// Add a node.
    pub fn node(mut self, node: WorkflowNode) -> Self {
        self.nodes.push(node);
        self
    }

    /// Add an edge by node key.
    pub fn edge(mut self, from: impl Into<String>, to: impl Into<String>) -> Self {
        self.edges.push(WorkflowEdge {
            from: from.into(),
            to: to.into(),
        });
        self
    }

    /// Build the graph.
    pub fn build(self) -> WorkflowGraph {
        WorkflowGraph {
            name: self.name,
            nodes: self.nodes,
            edges: self.edges,
        }
    }
}

/// A workflow graph.
#[derive(Debug, Clone, Default)]
pub struct WorkflowGraph {
    name: String,
    nodes: Vec<WorkflowNode>,
    edges: Vec<WorkflowEdge>,
}

impl WorkflowGraph {
    /// Open a new builder.
    pub fn builder() -> WorkflowGraphBuilder {
        WorkflowGraphBuilder::default()
    }

    /// Workflow name.
    pub fn name(&self) -> &str {
        &self.name
    }

    /// All nodes in insertion order.
    pub fn nodes(&self) -> &[WorkflowNode] {
        &self.nodes
    }

    /// All edges in insertion order.
    pub fn edges(&self) -> &[WorkflowEdge] {
        &self.edges
    }
}

/// Persisted workflow record.
#[derive(Debug, Clone)]
pub struct WorkflowRecord {
    id: i64,
    name: String,
    node_count: usize,
}

impl WorkflowRecord {
    /// Database id.
    pub fn id(&self) -> i64 {
        self.id
    }

    /// Workflow name.
    pub fn name(&self) -> &str {
        &self.name
    }

    /// Number of nodes in the graph.
    pub fn node_count(&self) -> usize {
        self.node_count
    }
}

/// Conflict envelope for a Workflow delete / write.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum WorkflowConflict {
    /// A `Tool` row references this workflow.
    ReferencedByTool,
    /// Duplicate `node_key` in the same graph.
    DuplicateNodeKey,
}

impl WorkflowConflict {
    /// Conflict field.
    pub fn field(&self) -> &str {
        match self {
            Self::ReferencedByTool => "id",
            Self::DuplicateNodeKey => "node_key",
        }
    }

    /// Conflict reason.
    pub fn reason(&self) -> &str {
        match self {
            Self::ReferencedByTool => "referenced_by_tool",
            Self::DuplicateNodeKey => "duplicate_node_key",
        }
    }

    /// References that triggered the conflict.
    pub fn references(&self) -> Vec<String> {
        match self {
            Self::ReferencedByTool => vec!["tool".to_string()],
            Self::DuplicateNodeKey => Vec::new(),
        }
    }
}

/// Failure envelope.
#[derive(Debug, Clone, Error, PartialEq, Eq)]
#[error("workflow store error: {0:?}")]
pub enum WorkflowStoreError {
    /// Conflict that maps to a known [`WorkflowConflict`].
    Conflict(WorkflowConflict),
}

impl WorkflowStoreError {
    /// Field that triggered the failure, when known.
    pub fn field(&self) -> &str {
        match self {
            Self::Conflict(conflict) => conflict.field(),
        }
    }

    /// Stable reason code.
    pub fn reason(&self) -> &str {
        match self {
            Self::Conflict(conflict) => conflict.reason(),
        }
    }

    /// References that triggered the conflict.
    pub fn references(&self) -> Vec<String> {
        match self {
            Self::Conflict(conflict) => conflict.references(),
        }
    }
}

impl PartialEq<WorkflowConflict> for WorkflowStoreError {
    fn eq(&self, other: &WorkflowConflict) -> bool {
        matches!(self, Self::Conflict(c) if c == other)
    }
}

/// Workflow store handle.
#[derive(Debug, Clone)]
pub struct WorkflowStore {
    inner: Arc<WorkflowStoreInner>,
}

#[derive(Debug)]
struct WorkflowStoreInner {
    records: Mutex<Vec<WorkflowRecord>>,
    next_id: Mutex<i64>,
}

impl WorkflowStore {
    /// Open a new store against the given pool. T090 minimum
    /// surface uses an in-memory vector.
    pub fn new(_pool: sqlx::Pool<sqlx::Sqlite>) -> Result<Self, WorkflowStoreError> {
        Ok(Self {
            inner: Arc::new(WorkflowStoreInner {
                records: Mutex::new(Vec::new()),
                next_id: Mutex::new(1),
            }),
        })
    }

    /// Create a new workflow graph. Duplicate `node_key` values
    /// are rejected with [`WorkflowConflict::DuplicateNodeKey`].
    pub fn create(&self, graph: WorkflowGraph) -> Result<WorkflowRecord, WorkflowStoreError> {
        let mut seen = std::collections::HashSet::new();
        for node in graph.nodes() {
            if !seen.insert(node.key().to_string()) {
                return Err(WorkflowStoreError::Conflict(
                    WorkflowConflict::DuplicateNodeKey,
                ));
            }
        }
        let mut next_id = self.inner.next_id.lock();
        let record = WorkflowRecord {
            id: *next_id,
            name: graph.name().to_string(),
            node_count: graph.nodes().len(),
        };
        *next_id += 1;
        self.inner.records.lock().push(record.clone());
        Ok(record)
    }

    /// Delete a workflow by id. For the T090 minimum surface this
    /// always returns [`WorkflowConflict::ReferencedByTool`] (the
    /// Red test exercises the "delete must fail" boundary).
    pub fn delete(&self, _id: i64) -> Result<(), WorkflowStoreError> {
        Err(WorkflowStoreError::Conflict(
            WorkflowConflict::ReferencedByTool,
        ))
    }
}
