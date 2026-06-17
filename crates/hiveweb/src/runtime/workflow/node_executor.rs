use serde_json::Value;
use std::sync::Arc;

use super::{ExecutorDeps, WorkflowError};
use crate::runtime::workflow::function_node::execute_function_node;
use crate::runtime::workflow::generate_answer_node::execute_answer_node;
use agent::context::AgentContext;

/// Execute a single function_node.
pub async fn execute_node(
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

    // Handle answer node: delegate to generate_answer_node
    if node_type == "generate_answer_node" {
        return execute_answer_node(
            deps,
            &node_key,
            input,
            node_config,
            invoking_agent_id,
            agent_ctx,
        )
        .await;
    }

    if node_type == "function_node" {
        // Continue to function node execution
        let function_id = function_id.ok_or_else(|| WorkflowError::NodeFailure {
            node_key: node_key.clone(),
            message: "function node has no function_id".into(),
        })?;

        execute_function_node(
            deps,
            &node_key,
            function_id,
            input,
            invoking_agent_id,
            agent_perms,
            &agent_ctx,
        )
        .await
        .map_err(|e| {
            tracing::error!(
                node_key = %node_key,
                function_id,
                error = %e,
                "function node execution failed"
            );
            e
        })
    } else {
        Err(WorkflowError::NodeFailure {
            node_key: node_key.clone(),
            message: format!("unsupported node type: {node_type}"),
        })
    }
}
