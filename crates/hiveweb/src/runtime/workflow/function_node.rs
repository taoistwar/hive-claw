//! Function node execution for Workflow DAG.
//!
//! Dispatches to builtin handler (kind=1, plugin_id IS NULL) or Plugin invoker
//! (kind=2). Answer nodes are delegated to `generate_answer_node`.

use serde_json::Value;
use std::sync::Arc;

use super::{ExecutorDeps, WorkflowError};
use crate::runtime::builtins;
use crate::runtime::capability::DispatchCtx;
use crate::runtime::hook::inject_agent_context_snapshot;
use agent::context::AgentContext;

/// Load function metadata and dispatch to builtin or plugin handler.
pub async fn execute_function_node(
    deps: &ExecutorDeps,
    node_key: &str,
    function_id: i64,
    input: Value,
    invoking_agent_id: i64,
    agent_perms: &[String],
    agent_ctx: &Arc<AgentContext>,
) -> Result<(String, Value), WorkflowError> {
    // Load function metadata from services layer
    let meta = crate::services::function::fetch_runtime_meta(&deps.pool, function_id)
        .await
        .map_err(|e| WorkflowError::NodeFailure {
            node_key: node_key.to_string(),
            message: format!("function fetch: {e}"),
        })?
        .ok_or_else(|| WorkflowError::NodeFailure {
            node_key: node_key.to_string(),
            message: format!("function id={function_id} not found"),
        })?;

    // Builtin (kind=1) → direct handler
    if meta.kind == 1  {
        return execute_builtin_function(deps, node_key, &meta.identifier, input, agent_ctx).await;
    }

    // Custom (kind=2) → Plugin invoker
    execute_plugin_function(
        deps,
        node_key,
        function_id,
        meta.plugin_id,
        meta.plugin_export.as_deref(),
        input,
        invoking_agent_id,
        agent_perms,
        agent_ctx,
    )
    .await
}

/// Execute a builtin function node (kind=1, no plugin).
async fn execute_builtin_function(
    deps: &ExecutorDeps,
    node_key: &str,
    identifier: &str,
    input: Value,
    agent_ctx: &Arc<AgentContext>,
) -> Result<(String, Value), WorkflowError> {
    let result = builtins::lookup(identifier).ok_or_else(|| WorkflowError::NodeFailure {
        node_key: node_key.to_string(),
        message: format!("unknown builtin: {identifier}"),
    })?;

    // builtin function parameter:
    let mut node_input = input;
    inject_agent_context_snapshot(&mut node_input, agent_ctx);

    let ctx = crate::runtime::builtins::BuiltinContext {
        pool: &deps.pool,
        ext_pool: deps.ext_pool.as_ref(),
        agent_ctx: Some(Arc::clone(agent_ctx)),
    };
    let out = (result.handler)(node_input, &ctx).map_err(|e| WorkflowError::NodeFailure {
        node_key: node_key.to_string(),
        message: format!("{e}"),
    })?;
    Ok((node_key.to_string(), out))
}

/// Execute a plugin function node (kind=2, WASM plugin via invoker).
#[allow(clippy::too_many_arguments)]
async fn execute_plugin_function(
    deps: &ExecutorDeps,
    node_key: &str,
    function_id: i64,
    plugin_id: Option<i64>,
    export: Option<&str>,
    input: Value,
    invoking_agent_id: i64,
    agent_perms: &[String],
    agent_ctx: &Arc<AgentContext>,
) -> Result<(String, Value), WorkflowError> {
    // Validate: plugin and export parameters
    let plugin_id = plugin_id.ok_or_else(|| WorkflowError::NodeFailure {
        node_key: node_key.to_string(),
        message: "custom function missing plugin_id".into(),
    })?;
    let export = export.ok_or_else(|| WorkflowError::NodeFailure {
        node_key: node_key.to_string(),
        message: "custom function missing plugin_export".into(),
    })?;

    // plugin parameter: input inject agent context snapshot
    let mut node_input = input;
    inject_agent_context_snapshot(&mut node_input, agent_ctx);
    let input_json =
        serde_json::to_string(&node_input).map_err(|e| WorkflowError::NodeFailure {
            node_key: node_key.to_string(),
            message: format!("input serialize: {e}"),
        })?;

    // Dispatch to invoker
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
            export,
            input_json,
            dispatch_ctx,
        )
        .await
        .map_err(|e| WorkflowError::NodeFailure {
            node_key: node_key.to_string(),
            message: format!("plugin invoke: {e}"),
        })?;
    let out: Value = serde_json::from_str(&out_str).unwrap_or_else(|_| Value::String(out_str));
    Ok((node_key.to_string(), out))
}
