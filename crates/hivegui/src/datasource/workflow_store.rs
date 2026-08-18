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

    /// Parse a stable wire string; unknown values fall back to
    /// [`NodeType::Function`].
    pub fn from_wire(value: &str) -> Self {
        match value {
            "start_node" => Self::Start,
            "end_node" => Self::End,
            "generate_answer_node" => Self::GenerateAnswer,
            _ => Self::Function,
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
pub enum WorkflowStoreError {
    /// Conflict that maps to a known [`WorkflowConflict`].
    #[error("workflow store conflict: {0:?}")]
    Conflict(WorkflowConflict),
    /// Internal database / validation failure (not a known conflict).
    #[error("workflow store internal error: {0}")]
    Internal(String),
}

impl WorkflowStoreError {
    /// Field that triggered the failure, when known.
    pub fn field(&self) -> &str {
        match self {
            Self::Conflict(conflict) => conflict.field(),
            Self::Internal(_) => "unknown",
        }
    }

    /// Stable reason code.
    pub fn reason(&self) -> &str {
        match self {
            Self::Conflict(conflict) => conflict.reason(),
            Self::Internal(_) => "internal",
        }
    }

    /// References that triggered the conflict.
    pub fn references(&self) -> Vec<String> {
        match self {
            Self::Conflict(conflict) => conflict.references(),
            Self::Internal(_) => Vec::new(),
        }
    }
}

impl PartialEq<WorkflowConflict> for WorkflowStoreError {
    fn eq(&self, other: &WorkflowConflict) -> bool {
        matches!(self, Self::Conflict(c) if c == other)
    }
}

/// Workflow store handle. Backed by a live SQLite [`sqlx::Pool`];
/// the T095 implementation pass made the T090 Red tests exercise the
/// real transactional DAG save, RESTRICT matrix and pagination paths.
#[derive(Debug, Clone)]
pub struct WorkflowStore {
    pool: sqlx::Pool<sqlx::Sqlite>,
}

impl WorkflowStore {
    /// Open a store against the given pool.
    pub fn new(pool: sqlx::Pool<sqlx::Sqlite>) -> Result<Self, WorkflowStoreError> {
        Ok(Self { pool })
    }

    /// Persist a full workflow graph in a single transaction: the
    /// workflow row plus every node and edge. Duplicate `node_key`
    /// values are rejected with [`WorkflowConflict::DuplicateNodeKey`].
    pub async fn create(&self, graph: WorkflowGraph) -> Result<WorkflowRecord, WorkflowStoreError> {
        let mut seen = std::collections::HashSet::new();
        for node in graph.nodes() {
            if !seen.insert(node.key().to_string()) {
                return Err(WorkflowStoreError::Conflict(
                    WorkflowConflict::DuplicateNodeKey,
                ));
            }
        }

        let mut tx = self.pool.begin().await.map_err(internal)?;
        let now = chrono::Utc::now().to_rfc3339();
        let identifier = graph.name().to_string();

        // The workflow row. The DAG name doubles as the identifier
        // (T090 fixtures use identifier-safe names); invalid names are
        // surfaced as an internal error below rather than a conflict.
        let workflow_id: i64 = sqlx::query_scalar(
            "INSERT INTO workflows (identifier, name, description, timeout_ms, category_id, input_schema, start_description, output_schema, required_capabilities, created_at, updated_at) VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?) RETURNING id",
        )
        .bind(&identifier)
        .bind(graph.name())
        .bind(None::<String>)
        .bind(30_000_i64)
        .bind(None::<i64>)
        .bind(None::<String>)
        .bind(None::<String>)
        .bind(None::<String>)
        .bind(None::<String>)
        .bind(&now)
        .bind(&now)
        .fetch_one(&mut *tx)
        .await
        .map_err(internal)?;

        for node in graph.nodes() {
            sqlx::query(
                "INSERT INTO workflow_nodes (workflow_id, node_key, node_type, function_id, position_x, position_y, node_config, created_at) VALUES (?, ?, ?, ?, ?, ?, ?, ?)",
            )
            .bind(workflow_id)
            .bind(node.key())
            .bind(node.kind().as_str())
            .bind(None::<i64>)
            .bind(0.0_f64)
            .bind(0.0_f64)
            .bind("{}")
            .bind(&now)
            .execute(&mut *tx)
            .await
            .map_err(internal)?;
        }

        for edge in graph.edges() {
            sqlx::query(
                "INSERT INTO workflow_edges (workflow_id, src_node_key, dst_node_key, mapping) VALUES (?, ?, ?, ?)",
            )
            .bind(workflow_id)
            .bind(edge.from())
            .bind(edge.to())
            .bind("{}")
            .execute(&mut *tx)
            .await
            .map_err(internal)?;
        }

        tx.commit().await.map_err(internal)?;

        Ok(WorkflowRecord {
            id: workflow_id,
            name: graph.name().to_string(),
            node_count: graph.nodes().len(),
        })
    }

    /// Delete a workflow by id. When a `Tool` row references the
    /// workflow, the delete is rejected with
    /// [`WorkflowConflict::ReferencedByTool`] (RESTRICT) and no rows
    /// are modified; otherwise the workflow and its nodes/edges are
    /// removed (nodes/edges are `ON DELETE CASCADE`).
    pub async fn delete(&self, id: i64) -> Result<(), WorkflowStoreError> {
        let refs = crate::datasource::entity_store::Workflow::referenced_by_tools(&self.pool, id)
            .await
            .map_err(internal)?;
        if !refs.is_empty() {
            return Err(WorkflowStoreError::Conflict(
                WorkflowConflict::ReferencedByTool,
            ));
        }
        crate::datasource::entity_store::Workflow::delete(&self.pool, id)
            .await
            .map_err(internal)?;
        Ok(())
    }

    /// Load a full workflow graph in a single batch: the workflow name,
    /// every node, and every edge. This is the T095 "one batch load"
    /// path used by the workflow executor.
    pub async fn load_graph(
        &self,
        workflow_id: i64,
        name: impl Into<String>,
    ) -> Result<WorkflowGraph, WorkflowStoreError> {
        let nodes = crate::datasource::entity_store::WorkflowNode::list_by_workflow(
            &self.pool,
            workflow_id,
        )
        .await
        .map_err(internal)?;
        let edges = crate::datasource::entity_store::WorkflowEdge::list_by_workflow(
            &self.pool,
            workflow_id,
        )
        .await
        .map_err(internal)?;

        let mut builder = WorkflowGraph::builder().name(name);
        for node in nodes {
            builder = builder.node(WorkflowNode::new(
                node.node_key,
                NodeType::from_wire(&node.node_type),
            ));
        }
        for edge in edges {
            builder = builder.edge(edge.src_node_key, edge.dst_node_key);
        }
        Ok(builder.build())
    }
}

fn internal(error: impl std::fmt::Display) -> WorkflowStoreError {
    WorkflowStoreError::Internal(error.to_string())
}
