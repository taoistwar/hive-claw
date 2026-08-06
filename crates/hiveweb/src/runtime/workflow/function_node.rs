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
use crate::services::runtime_audit;
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
    if meta.kind == 1 {
        return execute_builtin_function(
            deps,
            node_key,
            &meta.identifier,
            input,
            invoking_agent_id,
            agent_ctx,
        )
        .await;
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
    invoking_agent_id: i64,
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
        execution_context: Some(deps.execution_context.clone()),
        pool: &deps.pool,
        ext_pool: deps.ext_pool.as_ref(),
        redis: deps.redis.as_ref(),
        agent_ctx: Some(Arc::clone(agent_ctx)),
        llm: Some(&deps.llm),
        agent_id: Some(invoking_agent_id),
    };
    let out = (result.handler)(node_input, &ctx).map_err(|error| {
        tracing::error!(
            node_key_fingerprint = %runtime_audit::identifier_fingerprint(node_key),
            error_kind = "builtin_function_failed",
            "builtin function execution failed"
        );
        map_builtin_error(node_key, error)
    })?;
    Ok((node_key.to_string(), out))
}

fn map_builtin_error(
    node_key: &str,
    error: crate::runtime::builtins::BuiltinError,
) -> WorkflowError {
    match error {
        crate::runtime::builtins::BuiltinError::ModelPresetUnknown(name) => {
            WorkflowError::ModelPresetUnknown(name)
        }
        error => WorkflowError::NodeFailure {
            node_key: node_key.to_string(),
            message: error.to_string(),
        },
    }
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
        execution_context: deps.execution_context.clone(),
        agent_id: invoking_agent_id,
        plugin_id,
        function_id: Some(function_id),
        permissions: agent_perms.to_vec(),
    };
    let out_str = deps
        .invoker
        .invoke(
            &deps.pool,
            deps.s3.as_ref(),
            Arc::clone(&deps.registry),
            Arc::clone(&deps.llm),
            plugin_id,
            export,
            input_json,
            dispatch_ctx,
        )
        .await
        .map_err(|e| {
            let msg = format!("plugin invoke: {e}");
            tracing::error!(
                node_key_fingerprint = %runtime_audit::identifier_fingerprint(node_key),
                plugin_id = %plugin_id,
                error_kind = "plugin_invocation_failed",
                "plugin function execution failed"
            );
            WorkflowError::NodeFailure {
                node_key: node_key.to_string(),
                message: msg,
            }
        })?;
    let out: Value = serde_json::from_str(&out_str).unwrap_or_else(|_| Value::String(out_str));
    Ok((node_key.to_string(), out))
}

#[cfg(test)]
mod tests {
    use super::map_builtin_error;
    use crate::runtime::builtins::BuiltinError;
    use crate::runtime::workflow::WorkflowError;

    #[test]
    fn builtin_model_preset_unknown_remains_typed_for_api_code_5007_mapping() {
        let error = map_builtin_error(
            "game_info",
            BuiltinError::ModelPresetUnknown("removed-preset".to_string()),
        );

        assert!(matches!(
            error,
            WorkflowError::ModelPresetUnknown(name) if name == "removed-preset"
        ));
    }
}
