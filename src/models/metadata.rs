//! 元数据模型

use serde::{Deserialize, Serialize};

/// 数据库元数据
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DatabaseMeta {
    pub name: String,
    pub description: Option<String>,
    pub location: Option<String>,
}

/// 表元数据
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TableMeta {
    pub db_name: String,
    pub name: String,
    pub table_type: String,
    pub comment: Option<String>,
}

/// 字段元数据
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ColumnMeta {
    pub name: String,
    pub data_type: String,
    pub comment: Option<String>,
    pub position: i32,
}
