use axum::{
    extract::{Path, Query, State},
    Json, Router,
};
use serde::Deserialize;
use sqlx::Row;

use crate::api::AppState;
use crate::utils::error::{ApiResponse, AppError};

#[derive(serde::Serialize)]
pub struct RuntimeAuditLogPublic {
    id: i64,
    request_id: Option<String>,
    session_id: Option<i64>,
    agent_id: Option<i64>,
    plugin_id: Option<i64>,
    function_id: Option<i64>,
    capability: Option<String>,
    event_type: String,
    outcome: String,
    elapsed_ms: Option<i32>,
    error_message: Option<String>,
    payload_summary: Option<serde_json::Value>,
    occurred_at: String,
}

fn row_to_runtime_audit_log(row: &sqlx::mysql::MySqlRow) -> RuntimeAuditLogPublic {
    let payload_summary: Option<serde_json::Value> = row.get("payload_summary");
    let occurred_at: chrono::NaiveDateTime = row.get("occurred_at");

    RuntimeAuditLogPublic {
        id: row.get("id"),
        request_id: row.get("request_id"),
        session_id: row.get("session_id"),
        agent_id: row.get("agent_id"),
        plugin_id: row.get("plugin_id"),
        function_id: row.get("function_id"),
        capability: row.get("capability"),
        event_type: row.get("event_type"),
        outcome: row.get("outcome"),
        elapsed_ms: row.get("elapsed_ms"),
        error_message: row.get("error_message"),
        payload_summary,
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
pub struct ListRuntimeAuditLogsQuery {
    #[serde(default = "default_offset")]
    offset: u32,
    #[serde(default = "default_limit")]
    limit: u32,
    #[serde(default)]
    event_type: Option<String>,
    #[serde(default)]
    outcome: Option<String>,
    #[serde(default)]
    capability: Option<String>,
    #[serde(default)]
    request_id: Option<String>,
    #[serde(default)]
    session_id: Option<i64>,
    #[serde(default)]
    agent_id: Option<i64>,
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

pub async fn list_runtime_audit_logs(
    State(state): State<AppState>,
    Query(query): Query<ListRuntimeAuditLogsQuery>,
) -> ApiResponse<PaginatedResponse<RuntimeAuditLogPublic>> {
    let mut conditions = Vec::new();
    let mut string_params: Vec<String> = Vec::new();
    let mut i64_params: Vec<i64> = Vec::new();

    if let Some(ref event_type) = query.event_type {
        conditions.push("event_type = ?");
        string_params.push(event_type.clone());
    }
    if let Some(ref outcome) = query.outcome {
        conditions.push("outcome = ?");
        string_params.push(outcome.clone());
    }
    if let Some(ref capability) = query.capability {
        conditions.push("capability = ?");
        string_params.push(capability.clone());
    }
    if let Some(ref request_id) = query.request_id {
        conditions.push("request_id = ?");
        string_params.push(request_id.clone());
    }
    if let Some(session_id) = query.session_id {
        conditions.push("session_id = ?");
        i64_params.push(session_id);
    }
    if let Some(agent_id) = query.agent_id {
        conditions.push("agent_id = ?");
        i64_params.push(agent_id);
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

    let count_sql = format!("SELECT COUNT(*) FROM runtime_audit_logs {}", where_clause);
    let total = match execute_count_query(&count_sql, &string_params, &i64_params, &state.pool).await {
        Ok(result) => result,
        Err(e) => {
            tracing::error!("Database error: {}", e);
            return AppError::Internal("Service unavailable".to_string()).into_response();
        }
    };

    let data_sql = format!(
        "SELECT * FROM runtime_audit_logs {} ORDER BY occurred_at DESC LIMIT ? OFFSET ?",
        where_clause
    );

    let rows = match execute_list_query(&data_sql, &string_params, &i64_params, query.limit, query.offset, &state.pool).await {
        Ok(rows) => rows,
        Err(e) => {
            tracing::error!("Database error: {}", e);
            return AppError::Internal("Service unavailable".to_string()).into_response();
        }
    };

    let items: Vec<RuntimeAuditLogPublic> = rows.iter().map(|row| row_to_runtime_audit_log(row)).collect();

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

pub async fn get_runtime_audit_log(
    Path(id): Path<i64>,
    State(state): State<AppState>,
) -> ApiResponse<RuntimeAuditLogPublic> {
    let row = match sqlx::query("SELECT * FROM runtime_audit_logs WHERE id = ?")
        .bind(id)
        .fetch_one(&state.pool)
        .await
    {
        Ok(row) => row,
        Err(e) => {
            if e.to_string().contains("not found") || e.to_string().contains("returned no rows") {
                return AppError::NotFound("Runtime audit log not found".to_string()).into_response();
            }
            tracing::error!("Database error: {}", e);
            return AppError::Internal("Service unavailable".to_string()).into_response();
        }
    };

    ApiResponse::success(row_to_runtime_audit_log(&row))
}

pub fn router() -> Router<AppState> {
    Router::new()
        .route("/runtime-audit-logs", axum::routing::get(list_runtime_audit_logs))
        .route("/runtime-audit-logs/:id", axum::routing::get(get_runtime_audit_log))
}
