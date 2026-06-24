use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use sqlx::FromRow;

/// `kind` 取值：1 = function-wrap；2 = workflow-wrap（互斥，由 DB CHECK 约束）
/// `source` 取值：workspace（用户创建）| builtin（系统预定义，不可修改/删除）
/// `is_always` 取值：0（普通）| 1（所有 Agent 自动加载）
#[derive(Debug, Clone, Serialize, Deserialize, FromRow)]
pub struct Tool {
    pub id: i64,
    pub identifier: String,
    pub name: String,
    pub description: String,
    pub kind: i8,
    pub source: String,
    pub is_always: bool,
    pub function_id: Option<i64>,
    pub workflow_id: Option<i64>,
    pub input_schema: serde_json::Value,
    pub output_schema: serde_json::Value,
    pub category_id: Option<i64>,
    pub required_capabilities: Option<serde_json::Value>,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
}
