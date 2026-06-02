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
            succ.entry(src_key.clone())
                .or_default()
                .push(dst_key.clone());
            let m = mapping.as_object().cloned().unwrap_or_default();
            inbound.entry(dst_key).or_default().push((src_key, m));
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
                &inbound,
                &succ,
                &mut indegree,
                external_input,
                invoking_agent_id,
                workflow_id,
                &agent_perms,
            ),
        )
        .await;
        let mut outputs: HashMap<String, Value> = match result {
            Ok(inner) => match inner {
                Ok(r) => r,
                Err(e) => return Err(e),
            },
            Err(_) => return Err(WorkflowError::Timeout(timeout_ms)),
        };

        // 5. Build "end" node output: 从上游节点输出中按 output_schema 提取字段
        if let Some(ref schema) = output_schema {
            let schema_fields: Vec<String> = schema
                .get("properties")
                .and_then(|p| p.as_object())
                .map(|props| props.keys().cloned().collect())
                .unwrap_or_default();

            if !schema_fields.is_empty() {
                // 找到没有下游 DB 边的节点（它们连接到 end）
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
                    outputs.insert("end".to_string(), Value::Object(end_output));
                }
            }
        }

        Ok(outputs)
    }
}

#[allow(clippy::too_many_arguments)]
async fn run_layers(
    deps: &ExecutorDeps,
    nodes: &[(i64, String, Option<i64>, String, Option<Value>)],
    key_to_function: &HashMap<String, Option<i64>>,
    key_to_node_type: &HashMap<String, String>,
    key_to_node_config: &HashMap<String, Option<Value>>,
    inbound: &HashMap<String, Vec<(String, Map<String, Value>)>>,
    _succ: &HashMap<String, Vec<String>>,
    indegree: &mut HashMap<String, usize>,
    external_input: Value,
    invoking_agent_id: i64,
    workflow_id: i64,
    agent_perms: &[String],
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
            let node_config = key_to_node_config.get(node_key.as_str()).cloned().flatten();
            let node_input = build_node_input(
                node_key,
                inbound,
                &outputs,
                &external_input,
                node_config.as_ref(),
            )
            .map_err(|e| WorkflowError::MappingResolve {
                node_key: node_key.clone(),
                field: e.0,
                message: e.1,
            })?;
            let node_type = key_to_node_type
                .get(node_key.as_str())
                .map(|s| s.as_str())
                .unwrap_or("function_node");
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
            ));
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
    node_config: Option<&Value>,
) -> Result<Value, (String, String)> {
    let edges = inbound.get(node_key);
    if edges.is_none() || edges.unwrap().is_empty() {
        // 入口节点：检查 node_config 中是否有 input_mapping 用于映射 start → field
        if let Some(cfg) = node_config {
            if let Some(mapping) = cfg.get("input_mapping") {
                if let Value::Object(map) = mapping {
                    let mut input = Map::new();
                    for (field, m) in map {
                        if let Value::Object(src_cfg) = m {
                            let source =
                                src_cfg.get("source").and_then(|v| v.as_str()).unwrap_or("");
                            if source == "upstream" {
                                if let Some(snk) =
                                    src_cfg.get("source_node_key").and_then(|v| v.as_str())
                                {
                                    if snk == "start" {
                                        // 从 external_input 中取对应字段
                                        let src_field =
                                            src_cfg.get("source_field").and_then(|v| v.as_str());
                                        if let Value::Object(ext) = external_input {
                                            if let Some(src_field) = src_field {
                                                if let Some(val) = ext.get(src_field) {
                                                    input.insert(field.clone(), val.clone());
                                                }
                                            } else {
                                                // 无 source_field：用字段名在 external_input 中查找
                                                if let Some(val) = ext.get(field.as_str()) {
                                                    input.insert(field.clone(), val.clone());
                                                }
                                            }
                                        }
                                    }
                                }
                            } else if source == "custom" {
                                if let Some(cv) = src_cfg.get("custom_value") {
                                    input.insert(field.clone(), cv.clone());
                                }
                            }
                        }
                    }
                    if !input.is_empty() {
                        return Ok(Value::Object(input));
                    }
                }
            }
        }
        // 默认：入口节点直接喂外部输入
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
            let field = dst_key
                .strip_prefix("dst.input.")
                .unwrap_or(dst_key.as_str());
            let path = match src_path.as_str() {
                Some(s) => s,
                None => return Err((field.to_string(), "mapping value is not a string".into())),
            };
            // path: "<src_node_key>.output.<...>" or shorthand "output.<...>"
            // start 是虚拟节点，其"输出"就是 external_input
            let resolved = if src_key == "start" {
                resolve_from_external(path, external_input)?
            } else {
                resolve_src_path(path, src_key, outputs)?
            };
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
    let upstream = outputs.get(expected_src).ok_or_else(|| {
        (
            path.to_string(),
            format!("upstream {expected_src} produced no output"),
        )
    })?;
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

/// 解析 start 节点的虚拟输出：从 external_input 中取值
/// path 格式: "start.output.<field>"  — 从中提取 <field> 并在 external_input 中查找
fn resolve_from_external(path: &str, external_input: &Value) -> Result<Value, (String, String)> {
    // path: "start.output.query" → 提取 "query"
    let field = if let Some(r) = path.strip_prefix("start.output.") {
        r
    } else if let Some(r) = path.strip_prefix("output.") {
        r
    } else {
        return Err((
            path.to_string(),
            "expected 'start.output.<field>' or 'output.<field>' for start mapping".into(),
        ));
    };
    if let Value::Object(ext) = external_input {
        ext.get(field).cloned().ok_or_else(|| {
            (
                path.to_string(),
                format!("external input missing field '{field}'"),
            )
        })
    } else {
        Err((path.to_string(), "external input is not an object".into()))
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
        let ctx = crate::runtime::builtins::BuiltinContext {
            pool: &deps.pool,
            ext_pool: None,
        };
        let out = (result.handler)(input, &ctx).map_err(|e| WorkflowError::NodeFailure {
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
    input: Value,
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
        .unwrap_or(3);

    // Resolve template variables in system_prompt (e.g., "{{query}}" → actual value)
    let resolved_prompt = resolve_template_vars(system_prompt, &input);

    // Build the user message from the resolved input
    let user_message = serde_json::to_string(&input).unwrap_or_else(|_| "{}".to_string());

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

/// Replace `{{var_name}}` template variables in a string with values from input JSON.
fn resolve_template_vars(template: &str, input: &Value) -> String {
    let mut result = template.to_string();
    if let Value::Object(map) = input {
        for (key, val) in map {
            let placeholder = format!("{{{{{key}}}}}");
            let replacement = match val {
                Value::String(s) => s.clone(),
                other => other.to_string(),
            };
            result = result.replace(&placeholder, &replacement);
        }
    }
    // Replace remaining unresolved placeholders with empty string
    let re = regex::Regex::new(r"\{\{[a-zA-Z0-9_]+\}\}")
        .unwrap_or_else(|_| regex::Regex::new(r"\{\{[^}]+\}\}").unwrap());
    re.replace_all(&result, "").to_string()
}
