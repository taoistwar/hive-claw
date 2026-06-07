use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use sqlx::FromRow;

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
    /// AgentContext 扩展数据（cards, images, suggestions 等）
    pub extensions: Option<serde_json::Value>,
    pub created_at: DateTime<Utc>,
}
