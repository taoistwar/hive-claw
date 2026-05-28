use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use sqlx::FromRow;

#[derive(Debug, Clone, Serialize, Deserialize, FromRow)]
pub struct Workflow {
    pub id: i64,
    pub identifier: String,
    pub name: String,
    pub description: Option<String>,
    pub timeout_ms: i32,
    pub category_id: Option<i64>,
    pub required_capabilities: Option<serde_json::Value>,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
}

#[derive(Debug, Clone, Serialize, Deserialize, FromRow)]
pub struct WorkflowNode {
    pub id: i64,
    pub workflow_id: i64,
    pub node_key: String,
    pub function_id: i64,
    pub position: Option<serde_json::Value>,
}

#[derive(Debug, Clone, Serialize, Deserialize, FromRow)]
pub struct WorkflowEdge {
    pub id: i64,
    pub workflow_id: i64,
    pub src_node_id: i64,
    pub dst_node_id: i64,
    pub mapping: serde_json::Value,
}
