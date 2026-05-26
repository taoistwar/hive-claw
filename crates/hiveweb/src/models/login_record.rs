use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use sqlx::FromRow;

#[derive(Debug, Clone, Serialize, Deserialize, FromRow)]
pub struct LoginRecord {
    pub id: i64,
    pub admin_id: i64,
    pub admin_nickname: String,
    pub login_at: DateTime<Utc>,
    pub ip_address: String,
    pub success: bool,
    pub failure_reason: Option<String>,
}
