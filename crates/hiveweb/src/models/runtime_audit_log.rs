use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use sqlx::FromRow;

#[derive(Debug, Clone, Serialize, Deserialize, FromRow)]
pub struct RuntimeAuditLog {
    pub id: i64,
    pub request_id: Option<String>,
    pub session_id: Option<i64>,
    pub agent_id: Option<i64>,
    pub plugin_id: Option<i64>,
    pub function_id: Option<i64>,
    pub capability: Option<String>,
    /// e.g. capability_call, capability_denied, plugin_invoke, workflow_node, agent_route, llm_invoke, llm_fallback
    pub event_type: String,
    /// e.g. success, error, denied, timeout
    pub outcome: String,
    pub elapsed_ms: Option<i32>,
    pub error_message: Option<String>,
    pub payload_summary: Option<serde_json::Value>,
    pub occurred_at: DateTime<Utc>,
}
