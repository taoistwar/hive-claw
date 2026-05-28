//! Workflow DAG 执行器（research §5 / FR-013 / US3 / T110）
//!
//! 拓扑序 BFS 分层 + `tokio::join_all` 并行同层节点。环检测在 service 层
//! `services::workflow::put_graph` 保存时已禁止（spec FR-014），运行时不再校验。
//!
//! mapping 解析：每个 edge 的 `mapping = {"dst.input.<field>": "<src_node_key>.output.<path>"}`
//! 路径 `<path>` 支持简单 dot-path (e.g. `temp_c` 或 `nested.field`)。

use aws_sdk_s3::Client as S3Client;
use serde_json::{Map, Value};
use sqlx::MySqlPool;
use std::collections::{HashMap, HashSet};
use std::sync::Arc;
use std::time::{Duration, Instant};
use tokio::time::timeout;

use crate::runtime::builtins;
use crate::runtime::capability::{CapabilityRegistry, DispatchCtx};
use crate::runtime::invoker::Invoker;
use crate::runtime::llm::LlmRegistry;

#[derive(Debug, thiserror::Error)]
pub enum WorkflowError {
    #[error("workflow {0} not found")]
    NotFound(i64),
    #[error("workflow load failed: {0}")]
    LoadFailed(String),
    #[error("missing function for node {0}")]
    MissingFunction(String),
    #[error("workflow timeout after {0}ms")]
    Timeout(u64),
    #[error("node {node_key} failed: {message}")]
    NodeFailure { node_key: String, message: String },
    #[error("mapping resolve failed at {node_key}.{field}: {message}")]
    MappingResolve { node_key: String, field: String, message: String },
}

/// Dependencies needed at execution time
pub struct ExecutorDeps {
    pub pool: MySqlPool,
    pub s3: S3Client,
    pub registry: Arc<CapabilityRegistry>,
    pub llm: Arc<LlmRegistry>,
    pub invoker: Arc<Invoker>,
}

#[derive(Debug, Default)]
pub struct WorkflowExecutor;

impl WorkflowExecutor {
    pub fn new() -> Self {
        Self
    }

    /// Execute a Workflow by id with external input. Returns map of
    /// `node_key → output Value` (containing terminal nodes' outputs).
    pub async fn execute(
        &self,
        deps: &ExecutorDeps,
        workflow_id: i64,
        external_input: Value,
        invoking_agent_id: i64,
    ) -> Result<HashMap<String, Value>, WorkflowError> {
        // 1. Load workflow + nodes + edges
        let wf_row: Option<(i32,)> = sqlx::query_as(
            "SELECT timeout_ms FROM workflows WHERE id = ?",
        )
        .bind(workflow_id)
        .fetch_optional(&deps.pool)
        .await
        .map_err(|e| WorkflowError::LoadFailed(format!("{e}")))?;
        let timeout_ms = wf_row
            .ok_or(WorkflowError::NotFound(workflow_id))?
            .0 as u64;

        let nodes: Vec<(i64, String, i64)> = sqlx::query_as(
            "SELECT id, node_key, function_id FROM workflow_nodes WHERE workflow_id = ?",
        )
        .bind(workflow_id)
        .fetch_all(&deps.pool)
        .await
        .map_err(|e| WorkflowError::LoadFailed(format!("{e}")))?;

        let edges: Vec<(i64, i64, Value)> = sqlx::query_as(
            "SELECT src_node_id, dst_node_id, mapping FROM workflow_edges WHERE workflow_id = ?",
        )
        .bind(workflow_id)
        .fetch_all(&deps.pool)
        .await
        .map_err(|e| WorkflowError::LoadFailed(format!("{e}")))?;

        // 2. Build lookup tables
        let id_to_key: HashMap<i64, String> =
            nodes.iter().map(|(id, k, _)| (*id, k.clone())).collect();
        let key_to_function: HashMap<String, i64> =
            nodes.iter().map(|(_, k, fid)| (k.clone(), *fid)).collect();

        // adjacency for topology: indegree per node_key
        let mut indegree: HashMap<String, usize> =
            nodes.iter().map(|(_, k, _)| (k.clone(), 0)).collect();
        // dst_node_key → Vec<(src_node_key, mapping object)>
        let mut inbound: HashMap<String, Vec<(String, Map<String, Value>)>> = HashMap::new();
        // src_node_key → Vec<dst_node_key>
        let mut succ: HashMap<String, Vec<String>> = HashMap::new();

        for (sid, did, mapping) in &edges {
            let src_key = match id_to_key.get(sid) {
                Some(k) => k.clone(),
                None => continue,
            };
            let dst_key = match id_to_key.get(did) {
                Some(k) => k.clone(),
                None => continue,
            };
            *indegree.entry(dst_key.clone()).or_insert(0) += 1;
            succ.entry(src_key.clone()).or_default().push(dst_key.clone());
            let m = mapping.as_object().cloned().unwrap_or_default();
            inbound.entry(dst_key).or_default().push((src_key, m));
        }

        // 3. Topological layer execution with overall timeout
        let result = timeout(
            Duration::from_millis(timeout_ms),
            run_layers(
                deps,
                &nodes,
                &key_to_function,
                &inbound,
                &succ,
                &mut indegree,
                external_input,
                invoking_agent_id,
                workflow_id,
            ),
        )
        .await;
        match result {
            Ok(r) => r,
            Err(_) => Err(WorkflowError::Timeout(timeout_ms)),
        }
    }
}

#[allow(clippy::too_many_arguments)]
async fn run_layers(
    deps: &ExecutorDeps,
    nodes: &[(i64, String, i64)],
    key_to_function: &HashMap<String, i64>,
    inbound: &HashMap<String, Vec<(String, Map<String, Value>)>>,
    _succ: &HashMap<String, Vec<String>>,
    indegree: &mut HashMap<String, usize>,
    external_input: Value,
    invoking_agent_id: i64,
    workflow_id: i64,
) -> Result<HashMap<String, Value>, WorkflowError> {
    let mut outputs: HashMap<String, Value> = HashMap::new();
    let mut remaining: HashSet<String> = nodes.iter().map(|(_, k, _)| k.clone()).collect();
    let t0 = Instant::now();

    while !remaining.is_empty() {
        // Pick all nodes with indegree==0 currently in remaining
        let layer: Vec<String> = remaining
            .iter()
            .filter(|k| indegree.get(*k).copied().unwrap_or(0) == 0)
            .cloned()
            .collect();
        if layer.is_empty() {
            // 没有可调度的节点说明剩余节点都有上游未完成 — 通常意味着图有未覆盖的环
            // （保存时已校验过；这里作为防御）
            return Err(WorkflowError::NodeFailure {
                node_key: "<topology>".into(),
                message: "no schedulable node — possible cycle slipped past save-time check".into(),
            });
        }

        // 并行执行同层节点
        let mut futures = Vec::with_capacity(layer.len());
        for node_key in &layer {
            let node_input = build_node_input(node_key, inbound, &outputs, &external_input)
                .map_err(|e| WorkflowError::MappingResolve {
                    node_key: node_key.clone(),
                    field: e.0,
                    message: e.1,
                })?;
            let function_id = *key_to_function
                .get(node_key)
                .ok_or_else(|| WorkflowError::MissingFunction(node_key.clone()))?;
            let nk = node_key.clone();
            futures.push(execute_node(deps, function_id, nk, node_input, invoking_agent_id, workflow_id));
        }

        let results = futures::future::join_all(futures).await;
        for r in results {
            match r {
                Ok((nk, out)) => {
                    outputs.insert(nk.clone(), out);
                    remaining.remove(&nk);
                    // 下游 indegree--
                    if let Some(downstream) = _succ.get(&nk) {
                        for dst in downstream {
                            if let Some(d) = indegree.get_mut(dst) {
                                *d = d.saturating_sub(1);
                            }
                        }
                    }
                }
                Err(e) => return Err(e),
            }
        }
    }

    tracing::info!(
        workflow_id,
        nodes = outputs.len(),
        elapsed_ms = t0.elapsed().as_millis(),
        "workflow executed"
    );
    Ok(outputs)
}

/// Build node input by resolving inbound edge mappings against upstream outputs.
/// External input is used when a required field is not in any mapping (entry-node behavior).
fn build_node_input(
    node_key: &str,
    inbound: &HashMap<String, Vec<(String, Map<String, Value>)>>,
    outputs: &HashMap<String, Value>,
    external_input: &Value,
) -> Result<Value, (String, String)> {
    let edges = inbound.get(node_key);
    if edges.is_none() || edges.unwrap().is_empty() {
        // 入口节点：直接喂外部输入
        return Ok(external_input.clone());
    }
    let mut input = Map::new();
    // entry external input also merges in (as base) so entry nodes can mix external + mapped
    if let Value::Object(ext) = external_input {
        for (k, v) in ext {
            input.insert(k.clone(), v.clone());
        }
    }
    for (src_key, mapping) in edges.unwrap() {
        for (dst_key, src_path) in mapping {
            // dst_key like "dst.input.field" → strip prefix
            let field = dst_key.strip_prefix("dst.input.").unwrap_or(dst_key.as_str());
            let path = match src_path.as_str() {
                Some(s) => s,
                None => return Err((field.to_string(), "mapping value is not a string".into())),
            };
            // path: "<src_node_key>.output.<...>" or shorthand "output.<...>"
            let resolved = resolve_src_path(path, src_key, outputs)?;
            input.insert(field.to_string(), resolved);
        }
    }
    Ok(Value::Object(input))
}

fn resolve_src_path(
    path: &str,
    expected_src: &str,
    outputs: &HashMap<String, Value>,
) -> Result<Value, (String, String)> {
    // 接受两种形式：
    //   "<src_key>.output.foo.bar"  (entries in mapping)
    //   "output.foo.bar"             (shorthand, uses expected_src)
    let rest = if let Some(r) = path.strip_prefix(&format!("{expected_src}.output.")) {
        r
    } else if let Some(r) = path.strip_prefix("output.") {
        r
    } else {
        return Err((
            path.to_string(),
            format!("expected '{expected_src}.output.<path>' or 'output.<path>'"),
        ));
    };
    let upstream = outputs
        .get(expected_src)
        .ok_or_else(|| (path.to_string(), format!("upstream {expected_src} produced no output")))?;
    let mut current = upstream;
    for seg in rest.split('.') {
        match current {
            Value::Object(m) => {
                current = m
                    .get(seg)
                    .ok_or_else(|| (path.to_string(), format!("path segment '{seg}' not found")))?;
            }
            _ => {
                return Err((
                    path.to_string(),
                    format!("cannot index into non-object at segment '{seg}'"),
                ));
            }
        }
    }
    Ok(current.clone())
}

/// Execute a single node by function_id.
async fn execute_node(
    deps: &ExecutorDeps,
    function_id: i64,
    node_key: String,
    input: Value,
    invoking_agent_id: i64,
    workflow_id: i64,
) -> Result<(String, Value), WorkflowError> {
    // Load function metadata
    let row: Option<(i8, Option<i64>, Option<String>, String)> = sqlx::query_as(
        "SELECT kind, plugin_id, plugin_export, identifier FROM functions WHERE id = ?",
    )
    .bind(function_id)
    .fetch_optional(&deps.pool)
    .await
    .map_err(|e| WorkflowError::NodeFailure {
        node_key: node_key.clone(),
        message: format!("function fetch: {e}"),
    })?;
    let (kind, plugin_id, plugin_export, identifier) = row.ok_or_else(|| {
        WorkflowError::NodeFailure {
            node_key: node_key.clone(),
            message: format!("function id={function_id} not found"),
        }
    })?;

    // Builtin (kind=1, plugin_id IS NULL) → direct handler
    if kind == 1 && plugin_id.is_none() {
        let result = builtins::lookup(&identifier).ok_or_else(|| WorkflowError::NodeFailure {
            node_key: node_key.clone(),
            message: format!("unknown builtin: {identifier}"),
        })?;
        let out = (result.handler)(input).map_err(|e| WorkflowError::NodeFailure {
            node_key: node_key.clone(),
            message: format!("{e}"),
        })?;
        return Ok((node_key, out));
    }

    // Custom (kind=2, plugin_id set) → Plugin invoker
    let plugin_id = plugin_id.ok_or_else(|| WorkflowError::NodeFailure {
        node_key: node_key.clone(),
        message: "custom function missing plugin_id".into(),
    })?;
    let export = plugin_export.ok_or_else(|| WorkflowError::NodeFailure {
        node_key: node_key.clone(),
        message: "custom function missing plugin_export".into(),
    })?;
    let input_json = serde_json::to_string(&input).map_err(|e| WorkflowError::NodeFailure {
        node_key: node_key.clone(),
        message: format!("input serialize: {e}"),
    })?;
    let dispatch_ctx = DispatchCtx {
        request_id: None,
        session_id: None,
        agent_id: invoking_agent_id,
        plugin_id,
        function_id: Some(function_id),
        permissions: vec![],
    };
    let out_str = deps
        .invoker
        .invoke(
            &deps.pool,
            &deps.s3,
            Arc::clone(&deps.registry),
            Arc::clone(&deps.llm),
            plugin_id,
            &export,
            input_json,
            dispatch_ctx,
        )
        .await
        .map_err(|e| WorkflowError::NodeFailure {
            node_key: node_key.clone(),
            message: format!("plugin invoke: {e}"),
        })?;
    let _ = workflow_id;
    let out: Value =
        serde_json::from_str(&out_str).unwrap_or_else(|_| Value::String(out_str));
    Ok((node_key, out))
}
