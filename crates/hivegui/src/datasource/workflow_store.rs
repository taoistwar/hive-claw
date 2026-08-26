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

use std::collections::{BTreeMap, HashSet};

use sqlx::FromRow;
use thiserror::Error;

use super::query_count::QueryCountObserver;
use super::query_plan::{
    WORKFLOW_BATCH_EDGES_SQL, WORKFLOW_BATCH_NODES_SQL, WORKFLOW_FTS_COUNT_SQL,
    WORKFLOW_FTS_LIST_SQL, WORKFLOW_SHORT_GRAM_COUNT_SQL, WORKFLOW_SHORT_GRAM_LIST_SQL,
    WORKFLOW_TOOL_REFERENCES_SQL, WORKFLOW_UNFILTERED_COUNT_SQL, WORKFLOW_UNFILTERED_LIST_SQL,
};
use super::search_index::{
    IndexBackend, IndexSelection, canonical_normalizer, delete_entity_search_documents,
    fts_literal_phrase, normalize_search_query, replace_entity_search_documents,
};
use super::store::Store;

const PAGE_SIZE: i64 = 20;
const BATCH_CAPACITY: usize = 25;

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

    /// Parse one of the four stable wire strings.
    ///
    /// Unknown or legacy short values fail closed instead of being silently
    /// reinterpreted as a Function node.
    pub fn from_wire(value: &str) -> Result<Self, WorkflowStoreError> {
        match value {
            "start_node" => Ok(Self::Start),
            "end_node" => Ok(Self::End),
            "function_node" => Ok(Self::Function),
            "generate_answer_node" => Ok(Self::GenerateAnswer),
            _ => Err(WorkflowStoreError::Internal(
                "invalid persisted workflow node type".to_string(),
            )),
        }
    }
}

/// A single node in a [`WorkflowGraph`].
#[derive(Debug, Clone)]
pub struct WorkflowNode {
    key: String,
    kind: NodeType,
    function_id: Option<i64>,
    node_config: Option<serde_json::Value>,
}

impl WorkflowNode {
    /// Construct a new node with the given `node_key` and kind.
    pub fn new(key: impl Into<String>, kind: NodeType) -> Self {
        Self {
            key: key.into(),
            kind,
            function_id: None,
            node_config: None,
        }
    }

    /// Attach the referenced `functions.id` for a `function_node`. The
    /// executor uses this to resolve the concrete [`crate::datasource::entity_store::Function`]
    /// record (plugin/builtin) to run; it is only meaningful when the node
    /// kind is [`NodeType::Function`].
    pub fn with_function_id(mut self, function_id: i64) -> Self {
        self.function_id = Some(function_id);
        self
    }

    /// Attach the node's `node_config` JSON. For a
    /// [`NodeType::GenerateAnswer`] node this may carry a `model` key that
    /// the executor uses to select the LLM model.
    pub fn with_node_config(mut self, node_config: serde_json::Value) -> Self {
        self.node_config = Some(node_config);
        self
    }

    /// Node key.
    pub fn key(&self) -> &str {
        &self.key
    }

    /// Node kind.
    pub fn kind(&self) -> NodeType {
        self.kind
    }

    /// Referenced `functions.id`, when the node is a `function_node`.
    pub fn function_id(&self) -> Option<i64> {
        self.function_id
    }

    /// The node's `node_config` JSON, when present.
    pub fn node_config(&self) -> Option<&serde_json::Value> {
        self.node_config.as_ref()
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

/// One fixed-size page of Workflow records.
#[derive(Debug, Clone)]
pub struct WorkflowPage {
    items: Vec<WorkflowRecord>,
    total: i64,
    page: i64,
}

impl WorkflowPage {
    /// Records in canonical normalized-name order.
    pub fn items(&self) -> &[WorkflowRecord] {
        &self.items
    }

    /// Total matching Workflow count.
    pub fn total(&self) -> i64 {
        self.total
    }

    /// One-based page number.
    pub fn page(&self) -> i64 {
        self.page
    }

    /// Fixed page size.
    pub const fn page_size(&self) -> i64 {
        PAGE_SIZE
    }
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
    Conflict(WorkflowConflict, Vec<String>),
    /// Internal database / validation failure (not a known conflict).
    #[error("workflow store internal error: {0}")]
    Internal(String),
}

impl WorkflowStoreError {
    /// Field that triggered the failure, when known.
    pub fn field(&self) -> &str {
        match self {
            Self::Conflict(conflict, _) => conflict.field(),
            Self::Internal(_) => "unknown",
        }
    }

    /// Stable reason code.
    pub fn reason(&self) -> &str {
        match self {
            Self::Conflict(conflict, _) => conflict.reason(),
            Self::Internal(_) => "internal",
        }
    }

    /// References that triggered the conflict.
    pub fn references(&self) -> Vec<String> {
        match self {
            Self::Conflict(_, references) => references.clone(),
            Self::Internal(_) => Vec::new(),
        }
    }
}

impl PartialEq<WorkflowConflict> for WorkflowStoreError {
    fn eq(&self, other: &WorkflowConflict) -> bool {
        matches!(self, Self::Conflict(c, _) if c == other)
    }
}

/// Workflow store handle. Backed by a live SQLite [`sqlx::Pool`];
/// the T095 implementation pass made the T090 Red tests exercise the
/// real transactional DAG save, RESTRICT matrix and pagination paths.
#[derive(Debug, Clone)]
pub struct WorkflowStore {
    pool: sqlx::Pool<sqlx::Sqlite>,
    observer: Option<QueryCountObserver>,
}

impl WorkflowStore {
    /// Open a store against the given pool.
    pub fn new(pool: sqlx::Pool<sqlx::Sqlite>) -> Result<Self, WorkflowStoreError> {
        Ok(Self {
            pool,
            observer: None,
        })
    }

    /// Bind the Workflow boundary to an opened Store, including its optional
    /// query-count observer.
    pub fn from_store(store: &Store) -> Result<Self, WorkflowStoreError> {
        Ok(Self {
            pool: store.pool().clone(),
            observer: store.query_count_observer().cloned(),
        })
    }

    /// Persist a full workflow graph in a single transaction: the
    /// workflow row plus every node and edge. Duplicate `node_key`
    /// values are rejected with [`WorkflowConflict::DuplicateNodeKey`].
    pub async fn create(&self, graph: WorkflowGraph) -> Result<WorkflowRecord, WorkflowStoreError> {
        let mut seen = HashSet::new();
        for node in graph.nodes() {
            if !seen.insert(node.key().to_string()) {
                return Err(WorkflowStoreError::Conflict(
                    WorkflowConflict::DuplicateNodeKey,
                    Vec::new(),
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
            .bind(node.function_id())
            .bind(0.0_f64)
            .bind(0.0_f64)
            .bind(
                node.node_config()
                    .map(|c| c.to_string())
                    .unwrap_or_else(|| "{}".to_string()),
            )
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

        replace_entity_search_documents(
            &mut tx,
            "workflow",
            &workflow_id.to_string(),
            &[("identifier", &identifier), ("name", graph.name())],
        )
        .await
        .map_err(internal)?;

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
        let mut transaction = self.pool.begin().await.map_err(internal)?;
        let refs = sqlx::query_scalar::<_, String>(WORKFLOW_TOOL_REFERENCES_SQL)
            .bind(id)
            .fetch_all(&mut *transaction)
            .await
            .map_err(internal)?;
        if !refs.is_empty() {
            return Err(WorkflowStoreError::Conflict(
                WorkflowConflict::ReferencedByTool,
                refs,
            ));
        }
        delete_entity_search_documents(&mut transaction, "workflow", &id.to_string())
            .await
            .map_err(internal)?;
        sqlx::query("DELETE FROM workflows WHERE id = ?")
            .bind(id)
            .execute(&mut *transaction)
            .await
            .map_err(internal)?;
        transaction.commit().await.map_err(internal)?;
        Ok(())
    }

    /// List one fixed 20-row page using the canonical local search index.
    pub async fn list(
        &self,
        search: Option<String>,
        page: i64,
    ) -> Result<WorkflowPage, WorkflowStoreError> {
        if page < 1 {
            return Err(WorkflowStoreError::Internal(
                "invalid workflow page".to_string(),
            ));
        }
        let offset = page
            .checked_sub(1)
            .and_then(|value| value.checked_mul(PAGE_SIZE))
            .ok_or_else(|| WorkflowStoreError::Internal("invalid workflow page".to_string()))?;
        let route = WorkflowSearchRoute::for_input(search)?;
        let (rows, total) = match route {
            WorkflowSearchRoute::Unfiltered => {
                let rows = sqlx::query_as::<_, WorkflowListRow>(WORKFLOW_UNFILTERED_LIST_SQL)
                    .bind(PAGE_SIZE)
                    .bind(offset)
                    .fetch_all(&self.pool)
                    .await
                    .map_err(internal)?;
                let total = sqlx::query_scalar::<_, i64>(WORKFLOW_UNFILTERED_COUNT_SQL)
                    .fetch_one(&self.pool)
                    .await
                    .map_err(internal)?;
                (rows, total)
            }
            WorkflowSearchRoute::FtsPhrase(phrase) => {
                let rows = sqlx::query_as::<_, WorkflowListRow>(WORKFLOW_FTS_LIST_SQL)
                    .bind(&phrase)
                    .bind(PAGE_SIZE)
                    .bind(offset)
                    .fetch_all(&self.pool)
                    .await
                    .map_err(internal)?;
                let total = sqlx::query_scalar::<_, i64>(WORKFLOW_FTS_COUNT_SQL)
                    .bind(phrase)
                    .fetch_one(&self.pool)
                    .await
                    .map_err(internal)?;
                (rows, total)
            }
            WorkflowSearchRoute::ShortGram { length, gram } => {
                let rows = sqlx::query_as::<_, WorkflowListRow>(WORKFLOW_SHORT_GRAM_LIST_SQL)
                    .bind(length)
                    .bind(&gram)
                    .bind(PAGE_SIZE)
                    .bind(offset)
                    .fetch_all(&self.pool)
                    .await
                    .map_err(internal)?;
                let total = sqlx::query_scalar::<_, i64>(WORKFLOW_SHORT_GRAM_COUNT_SQL)
                    .bind(length)
                    .bind(gram)
                    .fetch_one(&self.pool)
                    .await
                    .map_err(internal)?;
                (rows, total)
            }
        };
        Ok(WorkflowPage {
            items: rows.into_iter().map(WorkflowRecord::from).collect(),
            total,
            page,
        })
    }

    /// Load a full workflow graph in a single batch: the workflow name,
    /// every node, and every edge. This is the T095 "one batch load"
    /// path used by the workflow executor.
    pub async fn load_graph(
        &self,
        workflow_id: i64,
        name: impl Into<String>,
    ) -> Result<WorkflowGraph, WorkflowStoreError> {
        let name = name.into();
        self.load_graphs(&[(workflow_id, name)])
            .await?
            .into_iter()
            .next()
            .ok_or_else(|| WorkflowStoreError::Internal("workflow graph not found".to_string()))
    }

    /// Load up to 25 complete graphs with exactly two indexed SQL queries,
    /// independent of batch cardinality.
    pub async fn load_graphs(
        &self,
        identities: &[(i64, String)],
    ) -> Result<Vec<WorkflowGraph>, WorkflowStoreError> {
        if identities.is_empty() {
            return Ok(Vec::new());
        }
        if identities.len() > BATCH_CAPACITY {
            return Err(WorkflowStoreError::Internal(
                "workflow graph batch exceeds 25 items".to_string(),
            ));
        }
        let ids = identities.iter().map(|(id, _)| *id).collect::<Vec<_>>();

        self.record_query("t090.workflow_graph.nodes");
        let mut node_query = sqlx::query_as::<_, WorkflowNodeRow>(WORKFLOW_BATCH_NODES_SQL);
        for slot in 0..BATCH_CAPACITY {
            node_query = node_query.bind(ids.get(slot).copied().unwrap_or(-1));
        }
        let nodes = node_query.fetch_all(&self.pool).await.map_err(internal)?;

        self.record_query("t090.workflow_graph.edges");
        let mut edge_query = sqlx::query_as::<_, WorkflowEdgeRow>(WORKFLOW_BATCH_EDGES_SQL);
        for slot in 0..BATCH_CAPACITY {
            edge_query = edge_query.bind(ids.get(slot).copied().unwrap_or(-1));
        }
        let edges = edge_query.fetch_all(&self.pool).await.map_err(internal)?;

        let mut nodes_by_workflow = BTreeMap::<i64, Vec<WorkflowNodeRow>>::new();
        for node in nodes {
            nodes_by_workflow
                .entry(node.workflow_id)
                .or_default()
                .push(node);
        }
        let mut edges_by_workflow = BTreeMap::<i64, Vec<WorkflowEdgeRow>>::new();
        for edge in edges {
            edges_by_workflow
                .entry(edge.workflow_id)
                .or_default()
                .push(edge);
        }

        identities
            .iter()
            .map(|(workflow_id, name)| {
                let mut builder = WorkflowGraph::builder().name(name);
                for node in nodes_by_workflow.remove(workflow_id).unwrap_or_default() {
                    let mut graph_node =
                        WorkflowNode::new(node.node_key, NodeType::from_wire(&node.node_type)?);
                    if let Some(function_id) = node.function_id {
                        graph_node = graph_node.with_function_id(function_id);
                    }
                    let config = serde_json::from_str::<serde_json::Value>(&node.node_config)
                        .map_err(|_| {
                            WorkflowStoreError::Internal(
                                "invalid persisted workflow node config".to_string(),
                            )
                        })?;
                    if !config.is_null() {
                        graph_node = graph_node.with_node_config(config);
                    }
                    builder = builder.node(graph_node);
                }
                for edge in edges_by_workflow.remove(workflow_id).unwrap_or_default() {
                    builder = builder.edge(edge.src_node_key, edge.dst_node_key);
                }
                Ok(builder.build())
            })
            .collect()
    }

    fn record_query(&self, query_id: &'static str) {
        if let Some(observer) = self.observer.as_ref() {
            observer.record_checked_query(query_id);
        }
    }
}

#[derive(Debug, FromRow)]
struct WorkflowListRow {
    id: i64,
    name: String,
}

impl From<WorkflowListRow> for WorkflowRecord {
    fn from(row: WorkflowListRow) -> Self {
        Self {
            id: row.id,
            name: row.name,
            node_count: 0,
        }
    }
}

#[derive(Debug, FromRow)]
struct WorkflowNodeRow {
    workflow_id: i64,
    node_key: String,
    node_type: String,
    function_id: Option<i64>,
    node_config: String,
}

#[derive(Debug, FromRow)]
struct WorkflowEdgeRow {
    workflow_id: i64,
    src_node_key: String,
    dst_node_key: String,
}

enum WorkflowSearchRoute {
    Unfiltered,
    FtsPhrase(String),
    ShortGram { length: i64, gram: String },
}

impl WorkflowSearchRoute {
    fn for_input(search: Option<String>) -> Result<Self, WorkflowStoreError> {
        let Some(search) = search else {
            return Ok(Self::Unfiltered);
        };
        let normalizer = canonical_normalizer().map_err(internal)?;
        let normalized = normalize_search_query(&normalizer, &search).map_err(internal)?;
        let selection = IndexSelection::for_input(&search, &normalizer).map_err(internal)?;
        Ok(match selection.backend() {
            IndexBackend::Fts5Trigram => Self::FtsPhrase(fts_literal_phrase(&normalized)),
            IndexBackend::ShortGram { length } => Self::ShortGram {
                length: length as i64,
                gram: normalized,
            },
        })
    }
}

fn internal(error: impl std::fmt::Display) -> WorkflowStoreError {
    WorkflowStoreError::Internal(error.to_string())
}
