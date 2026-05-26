//! Workflow DAG 执行器（research §5 / FR-013 / US3）
//!
//! 拓扑序 BFS + `tokio::join_all` 并行同层节点；环检测在 service 层 PUT
//! /workflows/:id/graph 时已禁止环（spec FR-014）。当前为骨架。

use std::collections::HashMap;

#[derive(Debug, thiserror::Error)]
pub enum WorkflowError {
    #[error("cycle detected at nodes: {0:?}")]
    Cycle(Vec<String>),
    #[error("missing function for node {0}")]
    MissingFunction(String),
    #[error("workflow timeout after {0}ms")]
    Timeout(u64),
    #[error("node {node_key} failed: {message}")]
    NodeFailure { node_key: String, message: String },
}

#[derive(Debug, Default)]
pub struct WorkflowExecutor;

impl WorkflowExecutor {
    pub fn new() -> Self {
        Self
    }

    /// Stub: 拓扑排序 + 并行执行。具体实现在 US3。
    #[allow(unused_variables)]
    pub async fn execute(
        &self,
        _workflow_id: i64,
        _input: serde_json::Value,
    ) -> Result<HashMap<String, serde_json::Value>, WorkflowError> {
        Err(WorkflowError::NodeFailure {
            node_key: "<stub>".into(),
            message: "workflow executor not yet implemented (Phase 6 US3)".into(),
        })
    }
}
