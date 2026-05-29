use axum::{
    extract::{Path, Query, State},
    Router,
};
use serde::Deserialize;
use sqlx::Row;

use crate::api::AppState;
use crate::utils::error::{ApiResponse, AppError};

#[derive(serde::Serialize)]
pub struct AdminAuditLogPublic {
    id: i64,
    operator_id: Option<i64>,
    operator_phone_snapshot: String,
    target_admin_id: Option<i64>,
    target_phone_snapshot: String,
    operation: String,
    detail: Option<serde_json::Value>,
    occurred_at: String,
}

fn row_to_admin_audit_log(row: &sqlx::mysql::MySqlRow) -> AdminAuditLogPublic {
    let detail: Option<serde_json::Value> = row.get("detail");
    let occurred_at: chrono::NaiveDateTime = row.get("occurred_at");

    AdminAuditLogPublic {
        id: row.get("id"),
        operator_id: row.get("operator_id"),
        operator_phone_snapshot: row.get("operator_phone_snapshot"),
        target_admin_id: row.get("target_admin_id"),
        target_phone_snapshot: row.get("target_phone_snapshot"),
        operation: row.get("operation"),
        detail,
        occurred_at: occurred_at.and_utc().to_rfc3339(),
    }
}

#[derive(serde::Serialize)]
pub struct PaginatedResponse<T: serde::Serialize> {
    items: Vec<T>,
    total: u64,
    offset: u32,
    limit: u32,
}

#[derive(Deserialize)]
pub struct ListAdminAuditLogsQuery {
    #[serde(default = "default_offset")]
    offset: u32,
    #[serde(default = "default_limit")]
    limit: u32,
    #[serde(default)]
    operation: Option<String>,
    #[serde(default)]
    operator_id: Option<i64>,
    #[serde(default)]
    target_admin_id: Option<i64>,
    #[serde(default)]
    occurred_at_start: Option<String>,
    #[serde(default)]
    occurred_at_end: Option<String>,
}

fn default_offset() -> u32 {
    0
}

fn default_limit() -> u32 {
    20
}

pub async fn list_admin_audit_logs(
    State(state): State<AppState>,
    Query(query): Query<ListAdminAuditLogsQuery>,
) -> ApiResponse<PaginatedResponse<AdminAuditLogPublic>> {
    let mut conditions = Vec::new();
    let mut string_params: Vec<String> = Vec::new();
    let mut i64_params: Vec<i64> = Vec::new();

    if let Some(ref operation) = query.operation {
        conditions.push("operation = ?");
        string_params.push(operation.clone());
    }
    if let Some(operator_id) = query.operator_id {
        conditions.push("operator_id = ?");
        i64_params.push(operator_id);
    }
    if let Some(target_admin_id) = query.target_admin_id {
        conditions.push("target_admin_id = ?");
        i64_params.push(target_admin_id);
    }
    if let Some(ref occurred_at_start) = query.occurred_at_start {
        conditions.push("occurred_at >= ?");
        string_params.push(occurred_at_start.clone());
    }
    if let Some(ref occurred_at_end) = query.occurred_at_end {
        conditions.push("occurred_at <= ?");
        string_params.push(occurred_at_end.clone());
    }

    let where_clause = if conditions.is_empty() {
        String::new()
    } else {
        format!("WHERE {}", conditions.join(" AND "))
    };

    let count_sql = format!("SELECT COUNT(*) FROM admin_audit_logs {}", where_clause);
    let total = match execute_count_query(&count_sql, &string_params, &i64_params, &state.pool).await {
        Ok(result) => result,
        Err(e) => {
            tracing::error!("Database error: {}", e);
            return AppError::Internal("Service unavailable".to_string()).into_response();
        }
    };

    let data_sql = format!(
        "SELECT * FROM admin_audit_logs {} ORDER BY occurred_at DESC LIMIT ? OFFSET ?",
        where_clause
    );

    let rows = match execute_list_query(&data_sql, &string_params, &i64_params, query.limit, query.offset, &state.pool).await {
        Ok(rows) => rows,
        Err(e) => {
            tracing::error!("Database error: {}", e);
            return AppError::Internal("Service unavailable".to_string()).into_response();
        }
    };

    let items: Vec<AdminAuditLogPublic> = rows.iter().map(|row| row_to_admin_audit_log(row)).collect();

    ApiResponse::success(PaginatedResponse {
        items,
        total,
        offset: query.offset,
        limit: query.limit,
    })
}

async fn execute_count_query(
    sql: &str,
    string_params: &[String],
    i64_params: &[i64],
    pool: &sqlx::MySqlPool,
) -> Result<u64, sqlx::Error> {
    let mut query = sqlx::query(sql);
    for p in string_params {
        query = query.bind(p);
    }
    for p in i64_params {
        query = query.bind(p);
    }
    let row = query.fetch_one(pool).await?;
    let count: i64 = row.get(0);
    Ok(count as u64)
}

async fn execute_list_query(
    sql: &str,
    string_params: &[String],
    i64_params: &[i64],
    limit: u32,
    offset: u32,
    pool: &sqlx::MySqlPool,
) -> Result<Vec<sqlx::mysql::MySqlRow>, sqlx::Error> {
    let mut query = sqlx::query(sql);
    for p in string_params {
        query = query.bind(p);
    }
    for p in i64_params {
        query = query.bind(p);
    }
    let query = query.bind(limit as i64).bind(offset as i64);
    query.fetch_all(pool).await
}

pub async fn get_admin_audit_log(
    Path(id): Path<i64>,
    State(state): State<AppState>,
) -> ApiResponse<AdminAuditLogPublic> {
    let row = match sqlx::query("SELECT * FROM admin_audit_logs WHERE id = ?")
        .bind(id)
        .fetch_one(&state.pool)
        .await
    {
        Ok(row) => row,
        Err(e) => {
            if e.to_string().contains("not found") || e.to_string().contains("returned no rows") {
                return AppError::NotFound("Admin audit log not found".to_string()).into_response();
            }
            tracing::error!("Database error: {}", e);
            return AppError::Internal("Service unavailable".to_string()).into_response();
        }
    };

    ApiResponse::success(row_to_admin_audit_log(&row))
}

pub fn router() -> Router<AppState> {
    Router::new()
        .route("/admin-audit-logs", axum::routing::get(list_admin_audit_logs))
        .route("/admin-audit-logs/:id", axum::routing::get(get_admin_audit_log))
}
