//! WorkflowExecutor — 工作流入口，加载 DAG、执行拓扑分层、构建 end 输出。
//!
//! Uses `run_layers`, `build_node_input`, etc. from the parent module.

use aws_sdk_s3::Client as S3Client;
use serde_json::{Map, Value};
use sqlx::MySqlPool;
use std::collections::{HashMap, HashSet};
use std::sync::Arc;
use std::time::{Duration, Instant};
use tokio::time::timeout;

use crate::runtime::capability::CapabilityRegistry;
use crate::runtime::hook::apply_agent_context_updates;
use crate::runtime::invoker::Invoker;
use crate::runtime::llm::LlmRegistry;
use crate::runtime::workflow::node_executor::execute_node;
use agent::context::AgentContext;

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
    MappingResolve {
        node_key: String,
        field: String,
        message: String,
    },
}

/// Dependencies needed at execution time
pub struct ExecutorDeps {
    pub pool: MySqlPool,
    /// 仅在 `PLUGIN_SYSTEM_ENABLED=true` 时为 `Some`。
    pub s3: Option<S3Client>,
    pub registry: Arc<CapabilityRegistry>,
    pub llm: Arc<LlmRegistry>,
    pub invoker: Arc<Invoker>,
    /// 外部数据库连接池（用于依赖外部 DB 的内置函数，如 query_balance）
    pub ext_pool: Option<MySqlPool>,
    /// Redis client for cache-aside operations.
    pub redis: Option<redis::Client>,
    /// Agent 的 capability 权限（生产路径传入 agent 实际权限，测试端点传入全部权限）
    pub permissions: Vec<String>,
}

/// Result of executing a workflow.
///
/// - `end_value`: synthesised end-node output (used as the workflow's "return value"
///   to upstream callers, with internal `_agent_context_updates` stripped).
/// - `node_results`: raw per-node outputs keyed by `node_key` (function_node,
///   generate_answer_node, start, end, etc.). Consumed by the API to power
///   the editor's per-node "已运行" indicators and result inspection.
#[derive(Debug)]
pub struct ExecuteOutcome {
    pub end_value: Value,
    pub node_results: HashMap<String, Value>,
    /// Per-node input values as resolved before execution (keyed by node_key)
    pub node_inputs: HashMap<String, Value>,
    /// Per-node AgentContext snapshot at execution time (keyed by node_key)
    pub node_agent_contexts: HashMap<String, Value>,
}

#[derive(Debug, Default)]
pub struct WorkflowExecutor;

impl WorkflowExecutor {
    pub fn new() -> Self {
        Self
    }

    /// Execute a Workflow by id with external input. Returns both the synthesised
    /// end-node value and the per-node output map.
    pub async fn execute(
        &self,
        deps: &ExecutorDeps,
        workflow_id: i64,
        external_input: Value,
        invoking_agent_id: i64,
        agent_ctx: Arc<AgentContext>,
    ) -> Result<ExecuteOutcome, WorkflowError> {
        // 1. Load workflow + nodes + edges
        let wf_row: Option<(i32, Option<Value>)> =
            sqlx::query_as("SELECT timeout_ms, output_schema FROM workflows WHERE id = ?")
                .bind(workflow_id)
                .fetch_optional(&deps.pool)
                .await
                .map_err(|e| WorkflowError::LoadFailed(format!("{e}")))?;
        let (timeout_ms, output_schema) = wf_row
            .ok_or(WorkflowError::NotFound(workflow_id))
            .map(|(t, os)| (t as u64, os))?;

        let nodes: Vec<(i64, String, Option<i64>, String, Option<Value>)> = sqlx::query_as(
            "SELECT id, node_key, function_id, COALESCE(node_type, 'function_node'), node_config FROM workflow_nodes WHERE workflow_id = ?",
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
        let id_to_key: HashMap<i64, String> = nodes
            .iter()
            .map(|(id, k, _, _, _)| (*id, k.clone()))
            .collect();
        let key_to_function: HashMap<String, Option<i64>> = nodes
            .iter()
            .map(|(_, k, fid, _, _)| (k.clone(), *fid))
            .collect();
        let key_to_node_type: HashMap<String, String> = nodes
            .iter()
            .map(|(_, k, _, nt, _)| (k.clone(), nt.clone()))
            .collect();
        let key_to_node_config: HashMap<String, Option<Value>> = nodes
            .iter()
            .map(|(_, k, _, _, cfg)| (k.clone(), cfg.clone()))
            .collect();

        // adjacency for topology: indegree per node_key
        let mut indegree: HashMap<String, usize> =
            nodes.iter().map(|(_, k, _, _, _)| (k.clone(), 0)).collect();
        // src_node_key → Vec<dst_node_key>
        let mut succ: HashMap<String, Vec<String>> = HashMap::new();

        for (sid, did, _mapping) in &edges {
            let src_key = match id_to_key.get(sid) {
                Some(k) => k.clone(),
                None => continue,
            };
            let dst_key = match id_to_key.get(did) {
                Some(k) => k.clone(),
                None => continue,
            };
            *indegree.entry(dst_key.clone()).or_insert(0) += 1;
            succ.entry(src_key.clone())
                .or_default()
                .push(dst_key.clone());
        }

        // 3. 使用调用方传入的 agent 权限（生产路径=Agent 实际权限，测试端点=全部权限）
        let agent_perms: Vec<String> = deps.permissions.clone();

        // 4. Topological layer execution with overall timeout
        let result = timeout(
            Duration::from_millis(timeout_ms),
            run_layers(
                deps,
                &nodes,
                &key_to_function,
                &key_to_node_type,
                &key_to_node_config,
                &succ,
                &mut indegree,
                external_input,
                invoking_agent_id,
                workflow_id,
                &agent_perms,
                agent_ctx,
            ),
        )
        .await;
        let (node_inputs, outputs, node_agent_contexts) = match result {
            Ok(inner) => match inner {
                Ok(r) => r,
                Err(e) => return Err(e),
            },
            Err(_) => return Err(WorkflowError::Timeout(timeout_ms)),
        };

        // 5. Build "end" node output
        let end_value = build_end_output(&nodes, &succ, &outputs, output_schema.as_ref());

        Ok(ExecuteOutcome {
            end_value,
            node_results: outputs,
            node_inputs,
            node_agent_contexts,
        })
    }
}

/// Build the end virtual node output from terminal nodes' results.
///
/// Path A (output_schema present): extract fields declared in schema from nodes
/// that have no downstream edges (they naturally connect to end).
///
/// Path B (no output_schema): return the "end" key from outputs with
/// `_agent_context_updates` stripped.
fn build_end_output(
    nodes: &[(i64, String, Option<i64>, String, Option<Value>)],
    succ: &HashMap<String, Vec<String>>,
    outputs: &HashMap<String, Value>,
    output_schema: Option<&Value>,
) -> Value {
    if let Some(schema) = output_schema {
        let schema_fields: Vec<String> = schema
            .get("properties")
            .and_then(|p| p.as_object())
            .map(|props| props.keys().cloned().collect())
            .unwrap_or_default();

        if !schema_fields.is_empty() {
            let final_node_keys: Vec<&str> = nodes
                .iter()
                .filter(|(_, k, _, _, _)| !succ.contains_key(k.as_str()))
                .map(|(_, k, _, _, _)| k.as_str())
                .collect();

            let mut end_output = serde_json::Map::new();
            for nk in &final_node_keys {
                if let Some(result) = outputs.get(*nk) {
                    if let Value::Object(obj) = result {
                        for field in &schema_fields {
                            if let Some(val) = obj.get(field) {
                                end_output.insert(field.clone(), val.clone());
                            }
                        }
                    }
                }
            }
            if !end_output.is_empty() {
                return Value::Object(end_output);
            }
        }
    } else if let Some(end_out) = outputs.get("end") {
        let mut cleaned = end_out.clone();
        if let Value::Object(ref mut map) = cleaned {
            map.remove("_agent_context_updates");
        }
        return cleaned;
    }

    Value::Object(serde_json::Map::new())
}

#[allow(clippy::too_many_arguments)]
pub(super) async fn run_layers(
    deps: &ExecutorDeps,
    nodes: &[(i64, String, Option<i64>, String, Option<Value>)],
    key_to_function: &HashMap<String, Option<i64>>,
    key_to_node_type: &HashMap<String, String>,
    key_to_node_config: &HashMap<String, Option<Value>>,
    _succ: &HashMap<String, Vec<String>>,
    indegree: &mut HashMap<String, usize>,
    external_input: Value,
    invoking_agent_id: i64,
    workflow_id: i64,
    agent_perms: &[String],
    agent_ctx: Arc<AgentContext>,
) -> Result<
    (
        HashMap<String, Value>,
        HashMap<String, Value>,
        HashMap<String, Value>,
    ),
    WorkflowError,
> {
    let mut outputs: HashMap<String, Value> = HashMap::new();
    let mut node_inputs: HashMap<String, Value> = HashMap::new();
    let mut node_agent_contexts: HashMap<String, Value> = HashMap::new();
    let mut remaining: HashSet<String> = nodes.iter().map(|(_, k, _, _, _)| k.clone()).collect();
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
            let node_type = key_to_node_type
                .get(node_key.as_str())
                .map(|s| s.as_str())
                .unwrap_or("function_node");
            let node_config = key_to_node_config.get(node_key.as_str()).cloned().flatten();
            let node_input = build_node_input(
                node_type,
                node_key,
                &outputs,
                &external_input,
                node_config.as_ref(),
                &agent_ctx,
            )
            .map_err(|e| WorkflowError::MappingResolve {
                node_key: node_key.clone(),
                field: e.0,
                message: e.1,
            })?;
            let function_id_opt = *key_to_function
                .get(node_key)
                .ok_or_else(|| WorkflowError::MissingFunction(node_key.clone()))?;
            let nk = node_key.clone();
            // Record the resolved input for inspection (strip _agent_context)
            let mut clean_input = node_input.clone();
            if let Some(obj) = clean_input.as_object_mut() {
                obj.remove("_agent_context");
            }
            node_inputs.insert(nk.clone(), clean_input);
            // Capture AgentContext snapshot for debugging
            let ac_snapshot = agent_ctx
                .snapshot()
                .ok()
                .and_then(|s| serde_json::to_value(s).ok())
                .unwrap_or(Value::Null);
            node_agent_contexts.insert(nk.clone(), ac_snapshot);
            futures.push(execute_node(
                deps,
                function_id_opt,
                node_type.to_string(),
                node_config,
                nk,
                node_input,
                invoking_agent_id,
                workflow_id,
                agent_perms,
                Arc::clone(&agent_ctx),
            ));
        }

        let results = futures::future::join_all(futures).await;
        for r in results {
            match r {
                Ok((nk, out)) => {
                    // 立即同步 _agent_context_updates 到 AgentContext
                    apply_agent_context_updates(&agent_ctx, &out);
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
                Err(e) => {
                    tracing::error!(
                        workflow_id,
                        error = %e,
                        "workflow node execution failed"
                    );
                    return Err(e);
                }
            }
        }
    }

    tracing::info!(
        workflow_id,
        nodes = outputs.len(),
        elapsed_ms = t0.elapsed().as_millis(),
        "workflow executed"
    );
    Ok((node_inputs, outputs, node_agent_contexts))
}

/// Resolve a node's input by reading the structured `InputSpec` from its
/// `node_config.input_mapping` and merging values from all three source
/// kinds (upstream node output, literal, AgentContext).
///
/// Used for both `function_node` and `generate_answer_node` — they share
/// the same structured spec under `node_config.input_mapping`. Other node
/// types (start / end) pass through `external_input` as-is.
fn build_node_input(
    node_type: &str,
    node_key: &str,
    outputs: &HashMap<String, Value>,
    external_input: &Value,
    node_config: Option<&Value>,
    agent_ctx: &AgentContext,
) -> Result<Value, (String, String)> {
    let spec_key = match node_type {
        "function_node" | "generate_answer_node" => Some("input_mapping"),
        _ => None,
    };

    // start / end / unknown: pass through external_input
    let Some(spec_key) = spec_key else {
        return Ok(external_input.clone());
    };

    let spec_value = node_config
        .and_then(|c| c.get(spec_key))
        .cloned()
        .unwrap_or(Value::Null);
    let spec = crate::runtime::input_source::parse_input_spec(&spec_value).map_err(|e| {
        (
            node_key.to_string(),
            format!("parse node_config.{spec_key}: {e}"),
        )
    })?;

    let ctx = InputBuildCtx {
        external_input,
        outputs,
        agent_ctx,
    };
    build_input_from_spec(&spec, &ctx)
}

/// Context passed to `build_input_from_spec` so it can resolve any source kind.
struct InputBuildCtx<'a> {
    external_input: &'a Value,
    outputs: &'a HashMap<String, Value>,
    agent_ctx: &'a AgentContext,
}

/// Resolve every entry in `spec` to a concrete value, returning a JSON object.
fn build_input_from_spec(
    spec: &crate::runtime::input_source::InputSpec,
    ctx: &InputBuildCtx<'_>,
) -> Result<Value, (String, String)> {
    use crate::runtime::input_source::{AgentContextCategory, InputSource};

    let snapshot = crate::runtime::hook::agent_context_snapshot_value(ctx.agent_ctx);
    let mut input = Map::new();
    for (field, src) in spec {
        let value = match src {
            InputSource::Upstream {
                node_key,
                field: sub,
            } => {
                if node_key == "start" {
                    // start is a virtual node whose "output" is external_input.
                    let key = sub.as_deref().unwrap_or(field.as_str());
                    resolve_from_external(key, field, ctx.external_input)?
                } else {
                    let upstream = ctx.outputs.get(node_key).ok_or_else(|| {
                        (
                            field.clone(),
                            format!("upstream node '{node_key}' produced no output"),
                        )
                    })?;
                    match sub {
                        Some(path) => resolve_dot_path(upstream, path, field)?,
                        None => upstream.clone(),
                    }
                }
            }
            InputSource::Custom { value } => value.clone(),
            InputSource::AgentContext {
                category,
                key,
                sub_key,
            } => resolve_from_agent_context(category, key, sub_key.as_deref(), &snapshot, field)?,
        };
        input.insert(field.clone(), value);
    }
    Ok(Value::Object(input))
}

fn resolve_dot_path(root: &Value, path: &str, field: &str) -> Result<Value, (String, String)> {
    let mut current = root;
    for seg in path.split('.') {
        match current {
            Value::Object(m) => {
                current = m.get(seg).ok_or_else(|| {
                    (
                        field.to_string(),
                        format!("path segment '{seg}' not found in upstream output"),
                    )
                })?;
            }
            _ => {
                return Err((
                    field.to_string(),
                    format!("cannot index into non-object at segment '{seg}'"),
                ));
            }
        }
    }
    Ok(current.clone())
}

fn resolve_from_external(
    key: &str,
    field: &str,
    external_input: &Value,
) -> Result<Value, (String, String)> {
    let ext = external_input.as_object().ok_or_else(|| {
        (
            field.to_string(),
            "external_input is not a JSON object".to_string(),
        )
    })?;
    Ok(ext
        .get(key)
        .cloned()
        .unwrap_or(Value::String(String::new())))
}

fn resolve_from_agent_context(
    category: &crate::runtime::input_source::AgentContextCategory,
    key: &str,
    sub_key: Option<&str>,
    snapshot: &Value,
    field: &str,
) -> Result<Value, (String, String)> {
    use crate::runtime::input_source::AgentContextCategory;
    match category {
        AgentContextCategory::UserInput => {
            let ui = snapshot
                .get("user_input")
                .and_then(|v| v.as_object())
                .ok_or_else(|| (field.to_string(), "snapshot.user_input missing".to_string()))?;
            match sub_key {
                Some(sk) => {
                    let md = ui
                        .get("metadata")
                        .and_then(|v| v.as_object())
                        .ok_or_else(|| {
                            (
                                field.to_string(),
                                "snapshot.user_input.metadata missing".to_string(),
                            )
                        })?;
                    md.get(sk).cloned().ok_or_else(|| {
                        (
                            field.to_string(),
                            format!("user_input.metadata.{sk} not found"),
                        )
                    })
                }
                None => ui
                    .get(key)
                    .cloned()
                    .ok_or_else(|| (field.to_string(), format!("user_input.{key} not found"))),
            }
        }
        AgentContextCategory::Entities
        | AgentContextCategory::ToolResults
        | AgentContextCategory::StateChanges => {
            let cat_name = match category {
                AgentContextCategory::Entities => "entities",
                AgentContextCategory::ToolResults => "tool_results",
                AgentContextCategory::StateChanges => "state_changes",
                _ => unreachable!(),
            };
            let arr = snapshot
                .get(cat_name)
                .and_then(|v| v.as_array())
                .ok_or_else(|| (field.to_string(), format!("snapshot.{cat_name} missing")))?;
            arr.iter()
                .find(|r| r.get("key").and_then(|k| k.as_str()) == Some(key))
                .and_then(|r| r.get("value").cloned())
                .ok_or_else(|| (field.to_string(), format!("{cat_name}.{key} not found")))
        }
        AgentContextCategory::Extensions => {
            let arr = snapshot
                .get("extensions")
                .and_then(|v| v.as_array())
                .ok_or_else(|| (field.to_string(), "snapshot.extensions missing".to_string()))?;
            // key = extension id
            let ext = arr
                .iter()
                .find(|e| e.get("id").and_then(|i| i.as_str()) == Some(key))
                .ok_or_else(|| (field.to_string(), format!("extension id '{key}' not found")))?;
            // sub_key = top-level field name on the ExtensionContent (id, content_type, reply, data, render_hints)
            let sk = sub_key.ok_or_else(|| {
                (
                    field.to_string(),
                    "extensions requires sub_key (field name)".to_string(),
                )
            })?;
            ext.get(sk).cloned().ok_or_else(|| {
                (
                    field.to_string(),
                    format!("extension '{key}' has no field '{sk}'"),
                )
            })
        }
    }
}
