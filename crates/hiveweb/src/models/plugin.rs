use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use sqlx::FromRow;

#[derive(Debug, Clone, Serialize, Deserialize, FromRow)]
pub struct Plugin {
    pub id: i64,
    pub identifier: String,
    pub name: String,
    pub description: Option<String>,
    pub manifest: Option<serde_json::Value>,
    pub runtime: String,
    pub version: String,
    pub author: Option<String>,
    pub repository_url: Option<String>,
    pub s3_key: String,
    pub sha256: String,
    pub size_bytes: i64,
    pub category_id: Option<i64>,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
    pub deleted_at: Option<DateTime<Utc>>,
}
