use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use sqlx::FromRow;

#[derive(Debug, Clone, Serialize, Deserialize, FromRow)]
pub struct Capability {
    pub name: String,
    pub description: String,
    pub is_dangerous: i8,
    pub category_id: Option<i64>,
    pub created_at: DateTime<Utc>,
}
