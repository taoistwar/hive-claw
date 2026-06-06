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

use agent::context::AgentContext;
use crate::runtime::builtins;
use crate::runtime::capability::{CapabilityRegistry, DispatchCtx};
use crate::runtime::hook::{apply_agent_context_updates, inject_agent_context_snapshot};
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
    MappingResolve {
        node_key: String,
        field: String,
        message: String,
    },
}

/// Dependencies needed at execution time
pub struct ExecutorDeps {
    pub pool: MySqlPool,
    pub s3: S3Client,
    pub registry: Arc<CapabilityRegistry>,
    pub llm: Arc<LlmRegistry>,
    pub invoker: Arc<Invoker>,
    /// 外部数据库连接池（用于依赖外部 DB 的内置函数，如 query_balance）
    pub ext_pool: Option<MySqlPool>,
}

#[derive(Debug, Default)]
pub struct WorkflowExecutor;

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
}

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

        // 3. Workflow 执行时授予全部 capability 权限（与 Tool 测试行为一致）
        let agent_perms: Vec<String> = crate::runtime::capability::CAPABILITIES
            .iter()
            .map(|c| c.name.to_string())
            .collect();

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
        let outputs: HashMap<String, Value> = match result {
            Ok(inner) => match inner {
                Ok(r) => r,
                Err(e) => return Err(e),
            },
            Err(_) => return Err(WorkflowError::Timeout(timeout_ms)),
        };

        // 5. Build "end" node output: 从上游节点输出中按 output_schema 提取字段
        //    同时清理内部的 _agent_context_updates（已在上层同步到 AgentContext）
        let mut end_value: Value = Value::Object(serde_json::Map::new());
        if let Some(ref schema) = output_schema {
            let schema_fields: Vec<String> = schema
                .get("properties")
                .and_then(|p| p.as_object())
                .map(|props| props.keys().cloned().collect())
                .unwrap_or_default();

            if !schema_fields.is_empty() {
                // 找到没有下游边的节点（它们连接到 end）
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
                    end_value = Value::Object(end_output);
                }
            }
        } else {
            // 无 output_schema 时，取 end 节点（如果存在已构建的）
            if let Some(end_out) = outputs.get("end") {
                let mut cleaned = end_out.clone();
                if let Value::Object(ref mut map) = cleaned {
                    map.remove("_agent_context_updates");
                }
                end_value = cleaned;
            }
        }

        Ok(ExecuteOutcome {
            end_value,
            node_results: outputs,
        })
    }
}

#[allow(clippy::too_many_arguments)]
async fn run_layers(
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
) -> Result<HashMap<String, Value>, WorkflowError> {
    let mut outputs: HashMap<String, Value> = HashMap::new();
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
                    // 后续节点可通过 inject_agent_context_snapshot 获取最新状态
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
    let spec = crate::runtime::input_source::parse_input_spec(&spec_value)
        .map_err(|e| (node_key.to_string(), format!("parse node_config.{spec_key}: {e}")))?;

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
            InputSource::Upstream { node_key, field: sub } => {
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
    ext.get(key).cloned().ok_or_else(|| {
        (
            field.to_string(),
            format!("start (external_input) has no field '{key}'"),
        )
    })
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
                .ok_or_else(|| {
                    (field.to_string(), "snapshot.user_input missing".to_string())
                })?;
            match sub_key {
                Some(sk) => {
                    let md = ui
                        .get("metadata")
                        .and_then(|v| v.as_object())
                        .ok_or_else(|| {
                            (field.to_string(), "snapshot.user_input.metadata missing".to_string())
                        })?;
                    md.get(sk).cloned().ok_or_else(|| {
                        (field.to_string(), format!("user_input.metadata.{sk} not found"))
                    })
                }
                None => ui.get(key).cloned().ok_or_else(|| {
                    (field.to_string(), format!("user_input.{key} not found"))
                }),
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
            let arr = snapshot.get(cat_name).and_then(|v| v.as_array()).ok_or_else(|| {
                (field.to_string(), format!("snapshot.{cat_name} missing"))
            })?;
            arr.iter()
                .find(|r| r.get("key").and_then(|k| k.as_str()) == Some(key))
                .and_then(|r| r.get("value").cloned())
                .ok_or_else(|| {
                    (field.to_string(), format!("{cat_name}.{key} not found"))
                })
        }
        AgentContextCategory::Extensions => {
            let arr = snapshot.get("extensions").and_then(|v| v.as_array()).ok_or_else(|| {
                (field.to_string(), "snapshot.extensions missing".to_string())
            })?;
            // key = extension id
            let ext = arr
                .iter()
                .find(|e| e.get("id").and_then(|i| i.as_str()) == Some(key))
                .ok_or_else(|| {
                    (field.to_string(), format!("extension id '{key}' not found"))
                })?;
            // sub_key = top-level field name on the ExtensionContent (id, content_type, reply, data, render_hints)
            let sk = sub_key.ok_or_else(|| {
                (
                    field.to_string(),
                    "extensions requires sub_key (field name)".to_string(),
                )
            })?;
            ext.get(sk).cloned().ok_or_else(|| {
                (field.to_string(), format!("extension '{key}' has no field '{sk}'"))
            })
        }
    }
}

/// Execute a single node. For answer nodes (function_id=None), generate a response using LLM.
async fn execute_node(
    deps: &ExecutorDeps,
    function_id: Option<i64>,
    node_type: String,
    node_config: Option<Value>,
    node_key: String,
    input: Value,
    invoking_agent_id: i64,
    workflow_id: i64,
    agent_perms: &[String],
    agent_ctx: Arc<AgentContext>,
) -> Result<(String, Value), WorkflowError> {
    let _ = workflow_id;

    // Handle answer node: generate response using LLM
    if node_type == "generate_answer_node" {
        return execute_answer_node(deps, &node_key, input, node_config, invoking_agent_id).await;
    }

    let function_id = function_id.ok_or_else(|| WorkflowError::NodeFailure {
        node_key: node_key.clone(),
        message: "node has no function_id and is not an answer node".into(),
    })?;

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
    let (kind, plugin_id, plugin_export, identifier) =
        row.ok_or_else(|| WorkflowError::NodeFailure {
            node_key: node_key.clone(),
            message: format!("function id={function_id} not found"),
        })?;

    // Builtin (kind=1, plugin_id IS NULL) → direct handler
    if kind == 1 && plugin_id.is_none() {
        let result = builtins::lookup(&identifier).ok_or_else(|| WorkflowError::NodeFailure {
            node_key: node_key.clone(),
            message: format!("unknown builtin: {identifier}"),
        })?;
        let mut node_input = input;
        inject_agent_context_snapshot(&mut node_input, &agent_ctx);
        let ctx = crate::runtime::builtins::BuiltinContext {
            pool: &deps.pool,
            ext_pool: deps.ext_pool.as_ref(),
            agent_ctx: Some(Arc::clone(&agent_ctx)),
        };
        let out = (result.handler)(node_input, &ctx).map_err(|e| WorkflowError::NodeFailure {
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
    let mut node_input = input;
    inject_agent_context_snapshot(&mut node_input, &agent_ctx);
    let input_json = serde_json::to_string(&node_input).map_err(|e| WorkflowError::NodeFailure {
        node_key: node_key.clone(),
        message: format!("input serialize: {e}"),
    })?;
    let dispatch_ctx = DispatchCtx {
        request_id: None,
        session_id: None,
        agent_id: invoking_agent_id,
        plugin_id,
        function_id: Some(function_id),
        permissions: agent_perms.to_vec(),
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
    let out: Value = serde_json::from_str(&out_str).unwrap_or_else(|_| Value::String(out_str));
    Ok((node_key, out))
}

/// Execute a generate-answer node: resolves template variables from upstream outputs
/// and calls LLM to generate a response.
async fn execute_answer_node(
    deps: &ExecutorDeps,
    node_key: &str,
    mut input: Value,
    node_config: Option<Value>,
    invoking_agent_id: i64,
) -> Result<(String, Value), WorkflowError> {
    let config = node_config.unwrap_or_default();
    let system_prompt = config
        .get("system_prompt")
        .and_then(|v| v.as_str())
        .unwrap_or("You are a helpful assistant.");
    let model_preset = config
        .get("model_preset")
        .and_then(|v| v.as_str())
        .filter(|s| !s.is_empty());
    let history_window = config
        .get("history_window")
        .and_then(|v| v.as_i64())
        .unwrap_or(0);

    // Strip _agent_context from input — it's runtime machinery, not user data
    if let Value::Object(ref mut map) = input {
        map.remove("_agent_context");
    }

    // Resolve template variables in system_prompt (e.g., "{query}" → actual value)
    let resolved_prompt = resolve_template_vars(system_prompt, &input);

    // Build the user message from the input. We prefer the "user's question"
    // field (e.g. `query`) so the LLM sees the actual question, not a
    // `"key: value"` dump. Fields injected into the system_prompt via `{var}`
    // are excluded from the fallback dump to avoid duplicating context data.
    let user_message = build_user_message(&input, system_prompt);

    tracing::info!(
        node_key,
        system_prompt_len = system_prompt.len(),
        resolved_prompt_len = resolved_prompt.len(),
        user_message_len = user_message.len(),
        input_keys = ?input.as_object().map(|m| m.keys().collect::<Vec<_>>()),
        query_value = ?input.get("query").and_then(|v| v.as_str()),
        "execute_answer_node: invoking LLM"
    );

    // Try LLM invocation; fall back to direct response if no LLM available
    let answer = match deps.llm.build_primary(model_preset) {
        Ok((provider, model)) => {
            use providers::ChatRequest;
            let req = ChatRequest {
                messages: vec![
                    serde_json::json!({"role": "system", "content": resolved_prompt}),
                    serde_json::json!({"role": "user", "content": user_message}),
                ],
                model: Some(model.clone()),
                max_tokens: 2048,
                temperature: 0.7,
                tools: None,
                tool_choice: None,
                reasoning_effort: None,
            };
            let resp = provider.chat(req).await;
            if resp.is_error() {
                let err_msg = resp
                    .content
                    .unwrap_or_else(|| "unknown LLM error".to_string());
                tracing::warn!(node_key, error = %err_msg, "answer node LLM failed, using fallback");
                format!("[LLM 调用失败: {err_msg}] 输入: {user_message}")
            } else {
                resp.content.unwrap_or_default()
            }
        }
        Err(e) => {
            tracing::warn!(node_key, error = %e, "answer node no LLM provider, using fallback");
            format!("[无可用模型] 系统提示: {resolved_prompt}\n输入: {user_message}")
        }
    };

    let _ = invoking_agent_id;
    let _ = history_window;

    Ok((
        node_key.to_string(),
        serde_json::json!({
            "answer": answer,
            "model_preset": model_preset,
        }),
    ))
}

/// Build a human-readable user message from the resolved node input.
///
/// The LLM chat pattern expects:
///   - `system`: instructions + context (with `{var}` placeholders resolved)
///   - `user`: the user's actual question / request
///
/// The user's question is conventionally a single string field (often named
/// `query`, `text`, `input`, etc.). We prefer that field as the user message
/// so the LLM sees the actual question rather than a `"key: value"` dump.
///
/// If no "user question" field is found we fall back to the first string
/// field; if there are no string fields we dump remaining fields as
/// `"key: value"` (skipping internal `_`-prefixed keys). Keys that were
/// injected into the system_prompt via `{var}` are skipped in the fallback
/// path to avoid duplicating context data.
fn build_user_message(input: &Value, system_prompt: &str) -> String {
    // Common names for the field that holds the user's actual question.
    const USER_QUESTION_KEYS: &[&str] = &[
        "query", "question", "text", "input", "message", "prompt",
        "user_input", "raw_text", "user_message", "ask",
    ];

    if let Value::Object(map) = input {
        // Priority 1: a well-known "user question" field
        for key in USER_QUESTION_KEYS {
            if !key.starts_with('_') {
                if let Some(Value::String(s)) = map.get(*key) {
                    return s.clone();
                }
            }
        }
        // Priority 2: the first string field (alphabetical order from BTreeMap)
        for (k, v) in map {
            if !k.starts_with('_') {
                if let Value::String(s) = v {
                    return s.clone();
                }
            }
        }
        // Priority 3: dump unreferenced non-internal fields as "key: value"
        let referenced = referenced_template_keys(system_prompt);
        let lines: Vec<String> = map
            .iter()
            .filter(|(k, _)| !referenced.contains(*k) && !k.starts_with('_'))
            .map(|(k, v)| {
                let val = serde_json::to_string(v).unwrap_or_default();
                format!("{k}: {val}")
            })
            .collect();
        if !lines.is_empty() {
            return lines.join("\n");
        }
    }
    // Last resort: original "all fields" behavior
    match input {
        Value::Object(map) if !map.is_empty() => {
            let lines: Vec<String> = map
                .iter()
                .filter(|(k, _)| !k.starts_with('_'))
                .map(|(k, v)| {
                    let val = match v {
                        Value::String(s) => s.clone(),
                        other => serde_json::to_string(other).unwrap_or_default(),
                    };
                    format!("{k}: {val}")
                })
                .collect();
            lines.join("\n")
        }
        Value::String(s) => s.clone(),
        _ => serde_json::to_string(input).unwrap_or_default(),
    }
}

/// Extract the set of keys referenced by `{var}` placeholders in a template.
/// Supports optional whitespace: `{ var }` and `{var}` both match.
fn referenced_template_keys(template: &str) -> std::collections::HashSet<String> {
    let re = regex::Regex::new(r"\{\s*([a-zA-Z0-9_]+)\s*\}")
        .unwrap_or_else(|_| regex::Regex::new(r"\{[^}]+\}").unwrap());
    re.captures_iter(template)
        .filter_map(|c| c.get(1).map(|m| m.as_str().to_string()))
        .collect()
}

/// Replace `{var_name}` template variables in a string with values from input JSON.
fn resolve_template_vars(template: &str, input: &Value) -> String {
    let mut result = template.to_string();
    if let Value::Object(map) = input {
        for (key, val) in map {
            let placeholder = format!("{{{key}}}");
            let replacement = match val {
                Value::String(s) => s.clone(),
                other => other.to_string(),
            };
            result = result.replace(&placeholder, &replacement);
        }
    }
    // Replace remaining unresolved placeholders with empty string.
    // Supports optional whitespace: { var } or {var}
    let re = regex::Regex::new(r"\{\s*[a-zA-Z0-9_]+\s*\}")
        .unwrap_or_else(|_| regex::Regex::new(r"\{[^}]+\}").unwrap());
    re.replace_all(&result, "").to_string()
}
