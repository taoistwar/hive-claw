use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use sqlx::FromRow;

/// Skill 沿用 markdown + frontmatter 模式（参考 `crates/skills`）
#[derive(Debug, Clone, Serialize, Deserialize, FromRow)]
pub struct Skill {
    pub id: i64,
    pub identifier: String,
    pub name: String,
    pub description: String,
    pub frontmatter: Option<serde_json::Value>,
    pub content: String,
    pub source: String,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
}
