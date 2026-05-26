use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use sqlx::FromRow;

/// `kind` 取值：1 = builtin（由代码注册）；2 = custom（来自 Plugin 导出）
#[derive(Debug, Clone, Serialize, Deserialize, FromRow)]
pub struct Function {
    pub id: i64,
    pub identifier: String,
    pub name: String,
    pub description: Option<String>,
    pub kind: i8,
    pub input_schema: serde_json::Value,
    pub output_schema: serde_json::Value,
    pub plugin_id: Option<i64>,
    pub plugin_export: Option<String>,
    pub category_id: Option<i64>,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
}
