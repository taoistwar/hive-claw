use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize, sqlx::FromRow)]
pub struct DataSource {
    pub id: i64,
    pub name: String,
    pub host: String,
    pub port: u16,
    pub username: String,
    #[serde(skip_serializing, skip_deserializing)]
    pub encrypted_password: Vec<u8>,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
}

#[derive(Debug, Clone)]
pub struct DatabaseInfo {
    pub name: String,
    pub charset: Option<String>,
    pub collation: Option<String>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct TableInfo {
    pub name: String,
    pub comment: Option<String>,
    pub engine: Option<String>,
    pub row_count: Option<i64>,
}

#[derive(Debug, Clone)]
pub struct ColumnInfo {
    pub name: String,
    pub data_type: String,
    pub is_nullable: bool,
    pub column_default: Option<String>,
    pub is_primary_key: bool,
    pub comment: Option<String>,
    pub character_maximum_length: Option<i64>,
    pub numeric_precision: Option<i64>,
}

#[derive(Debug, Clone)]
pub struct TableData {
    pub columns: Vec<String>,
    pub rows: Vec<Vec<Option<String>>>,
    pub total_count: i64,
    pub limit: i64,
    pub offset: i64,
}

impl DataSource {
    pub fn default_name(host: &str, port: u16) -> String {
        format!("mysql@{host}:{port}")
    }
}

#[derive(Debug, Clone)]
pub struct CreateDataSourceRequest {
    pub name: String,
    pub host: String,
    pub port: u16,
    pub username: String,
    pub password: String,
}

#[derive(Debug, Clone)]
pub struct TableDataRequest {
    pub limit: i64,
    pub offset: i64,
    pub where_clause: Option<String>,
    pub order_by: Option<String>,
}
