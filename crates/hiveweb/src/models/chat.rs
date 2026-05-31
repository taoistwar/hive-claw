use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use sqlx::FromRow;

#[derive(Debug, Clone, Serialize, Deserialize, FromRow)]
pub struct ChatSession {
    pub id: i64,
    /// 管理员操作者；admin 删除后 SET NULL，仅 Super 可继续访问该 session
    pub admin_id: Option<i64>,
    pub admin_phone_snapshot: String,
    pub admin_nickname_snapshot: String,
    /// 普通用户操作者；与 admin_id 二选一
    pub user_id: Option<i64>,
    pub user_phone_snapshot: String,
    pub user_nickname_snapshot: String,
    pub title: Option<String>,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
}

#[derive(Debug, Clone, Serialize, Deserialize, FromRow)]
pub struct ChatMessage {
    pub id: i64,
    pub session_id: i64,
    pub seq: i32,
    pub role: String,
    pub content: Option<String>,
    pub tool_calls: Option<serde_json::Value>,
    pub routed_to_agent_id: Option<i64>,
    pub elapsed_ms: Option<i32>,
    pub created_at: DateTime<Utc>,
}
