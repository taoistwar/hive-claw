use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use sqlx::FromRow;

// --- Admin tables ---

#[derive(Debug, Clone, Serialize, Deserialize, FromRow)]
pub struct ChatSessionAdmin {
    pub id: i64,
    pub admin_id: i64,
    pub admin_phone_snapshot: String,
    pub admin_nickname_snapshot: String,
    pub title: Option<String>,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
}

#[derive(Debug, Clone, Serialize, Deserialize, FromRow)]
pub struct ChatMessageAdmin {
    pub id: i64,
    pub session_id: i64,
    pub admin_id: i64,
    pub role: String,
    pub content: Option<String>,
    pub tool_calls: Option<serde_json::Value>,
    pub routed_to_agent_id: Option<i64>,
    pub elapsed_ms: Option<i32>,
    pub created_at: DateTime<Utc>,
}

// --- User tables ---

#[derive(Debug, Clone, Serialize, Deserialize, FromRow)]
pub struct ChatSessionUser {
    pub id: i64,
    pub user_id: i64,
    pub user_phone_snapshot: String,
    pub user_nickname_snapshot: String,
    pub title: Option<String>,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
}

#[derive(Debug, Clone, Serialize, Deserialize, FromRow)]
pub struct ChatMessageUser {
    pub id: i64,
    pub session_id: i64,
    pub user_id: i64,
    pub role: String,
    pub content: Option<String>,
    pub elapsed_ms: Option<i32>,
    pub created_at: DateTime<Utc>,
}
