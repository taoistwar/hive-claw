use serde_json::{Value, json};
use std::sync::Arc;
use std::time::Instant;

use super::{ExecutorDeps, WorkflowError};
use crate::runtime::execution_context::RuntimeExecutionContext;
use crate::runtime::workflow::function_node::execute_function_node;
use crate::runtime::workflow::generate_answer_node::execute_answer_node;
use crate::services::runtime_audit::{self, AuditRecord};
use agent::context::AgentContext;

struct WorkflowNodeTimeoutAuditGuard {
    execution_context: RuntimeExecutionContext,
    agent_id: i64,
    function_id: Option<i64>,
    workflow_id: i64,
    node_type: String,
    node_key: String,
    started: Instant,
    completed: bool,
}

impl WorkflowNodeTimeoutAuditGuard {
    fn new(
        execution_context: &RuntimeExecutionContext,
        agent_id: i64,
        function_id: Option<i64>,
        workflow_id: i64,
        node_type: &str,
        node_key: &str,
    ) -> Self {
        Self {
            execution_context: execution_context.clone(),
            agent_id,
            function_id,
            workflow_id,
            node_type: node_type.to_string(),
            node_key: node_key.to_string(),
            started: Instant::now(),
            completed: false,
        }
    }

    fn mark_completed(&mut self) {
        self.completed = true;
    }
}

impl Drop for WorkflowNodeTimeoutAuditGuard {
    fn drop(&mut self) {
        if self.completed {
            return;
        }

        runtime_audit::record(
            &self.execution_context,
            AuditRecord {
                agent_id: Some(self.agent_id),
                plugin_id: None,
                function_id: self.function_id,
                capability: None,
                event_type: "workflow_node",
                outcome: "timeout",
                elapsed_ms: Some(self.started.elapsed().as_millis() as i32),
                error_message: Some("workflow node execution timed out"),
                payload_summary: Some(json!({
                    "workflow_id": self.workflow_id,
                    "node_type": self.node_type,
                    "node_key": self.node_key,
                })),
            },
        );
    }
}

/// Execute a single function_node.
#[expect(
    clippy::too_many_arguments,
    reason = "the workflow scheduler passes explicit node execution context"
)]
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
    let started = Instant::now();
    let mut timeout_audit = WorkflowNodeTimeoutAuditGuard::new(
        &deps.execution_context,
        invoking_agent_id,
        function_id,
        workflow_id,
        &node_type,
        &node_key,
    );
    let result = if node_type == "generate_answer_node" {
        execute_answer_node(
            deps,
            &node_key,
            input,
            node_config,
            invoking_agent_id,
            agent_ctx,
        )
        .await
    } else if node_type == "function_node" {
        match function_id {
            Some(function_id) => execute_function_node(
                deps,
                &node_key,
                function_id,
                input,
                invoking_agent_id,
                agent_perms,
                &agent_ctx,
            )
            .await
            .inspect_err(|_| {
                tracing::error!(
                    node_key_fingerprint =
                        %runtime_audit::identifier_fingerprint(&node_key),
                    function_id,
                    error_kind = "function_node_execution_failed",
                    "function node execution failed"
                );
            }),
            None => Err(WorkflowError::NodeFailure {
                node_key: node_key.clone(),
                message: "function node has no function_id".into(),
            }),
        }
    } else {
        Err(WorkflowError::NodeFailure {
            node_key: node_key.clone(),
            message: format!("unsupported node type: {node_type}"),
        })
    };

    timeout_audit.mark_completed();
    let outcome = if result.is_ok() { "success" } else { "error" };
    runtime_audit::record(
        &deps.execution_context,
        AuditRecord {
            agent_id: Some(invoking_agent_id),
            plugin_id: None,
            function_id,
            capability: None,
            event_type: "workflow_node",
            outcome,
            elapsed_ms: Some(started.elapsed().as_millis() as i32),
            error_message: result
                .as_ref()
                .err()
                .map(|_| "workflow node execution failed"),
            payload_summary: Some(json!({
                "workflow_id": workflow_id,
                "node_type": node_type,
                "node_key": node_key,
            })),
        },
    );

    result
}

#[cfg(test)]
mod tests {
    use super::WorkflowNodeTimeoutAuditGuard;
    use crate::runtime::execution_context::RuntimeExecutionContext;
    use std::sync::{Arc, Mutex};

    #[derive(Clone, Default)]
    struct TraceWriter(Arc<Mutex<Vec<u8>>>);

    struct TraceWriterGuard(Arc<Mutex<Vec<u8>>>);

    impl std::io::Write for TraceWriterGuard {
        fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
            self.0.lock().expect("trace buffer poisoned").extend(buf);
            Ok(buf.len())
        }

        fn flush(&mut self) -> std::io::Result<()> {
            Ok(())
        }
    }

    impl<'a> tracing_subscriber::fmt::MakeWriter<'a> for TraceWriter {
        type Writer = TraceWriterGuard;

        fn make_writer(&'a self) -> Self::Writer {
            TraceWriterGuard(Arc::clone(&self.0))
        }
    }

    #[tokio::test]
    async fn overall_timeout_drops_guard_and_traces_without_raw_node_key() {
        let writer = TraceWriter::default();
        let subscriber = tracing_subscriber::fmt()
            .without_time()
            .with_ansi(false)
            .with_target(false)
            .with_writer(writer.clone())
            .finish();
        let _subscriber = tracing::subscriber::set_default(subscriber);

        let execution_context =
            RuntimeExecutionContext::best_effort(Some("workflow-request".into()), Some(91))
                .for_hook();
        let raw_node_key = "PRIVATE_NODE_KEY_SENTINEL";
        let timed_out = tokio::time::timeout(std::time::Duration::from_millis(1), async {
            let _timeout_audit = WorkflowNodeTimeoutAuditGuard::new(
                &execution_context,
                7,
                Some(8),
                9,
                "function_node",
                raw_node_key,
            );
            std::future::pending::<()>().await;
        })
        .await;
        assert!(timed_out.is_err());

        let output =
            String::from_utf8(writer.0.lock().expect("trace buffer poisoned").clone()).unwrap();
        assert!(output.contains("event_type=\"workflow_node\""));
        assert!(output.contains("outcome=\"timeout\""));
        assert!(output.contains("node_key_fingerprint"));
        assert!(!output.contains(raw_node_key));
    }
}
