use chrono::NaiveDateTime;
use serde::{Deserialize, Serialize};
use sqlx::FromRow;

/// Sanitized persisted copy of one non-Hook runtime audit event.
#[derive(Debug, Clone, Serialize, Deserialize, FromRow)]
pub struct RuntimeAuditLog {
    pub id: i64,
    pub request_id: Option<String>,
    pub session_id: Option<i64>,
    pub agent_id: Option<i64>,
    pub plugin_id: Option<i64>,
    pub function_id: Option<i64>,
    pub capability: Option<String>,
    pub event_type: String,
    pub outcome: String,
    pub elapsed_ms: Option<i32>,
    pub error_message: Option<String>,
    pub payload_summary: Option<serde_json::Value>,
    pub occurred_at: NaiveDateTime,
}
