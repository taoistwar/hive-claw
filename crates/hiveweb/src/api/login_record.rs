use axum::{
    Router,
    extract::{Path, Query, State},
};
use serde::Deserialize;

use crate::api::AppState;
use crate::services::login_record;
use crate::utils::error::{ApiResponse, AppError};

#[derive(serde::Serialize)]
pub struct PaginatedResponse<T: serde::Serialize> {
    items: Vec<T>,
    total: u64,
    offset: u32,
    limit: u32,
}

#[derive(serde::Serialize)]
pub struct LoginRecordPublic {
    pub id: i64,
    pub admin_id: Option<i64>,
    pub admin_phone_snapshot: String,
    pub admin_nickname_snapshot: String,
    pub login_at: String,
    pub ip_address: String,
    pub success: bool,
    pub failure_reason: Option<String>,
}

impl From<crate::models::LoginRecord> for LoginRecordPublic {
    fn from(record: crate::models::LoginRecord) -> Self {
        LoginRecordPublic {
            id: record.id,
            admin_id: record.admin_id,
            admin_phone_snapshot: record.admin_phone_snapshot,
            admin_nickname_snapshot: record.admin_nickname_snapshot,
            login_at: record.login_at.to_rfc3339(),
            ip_address: record.ip_address,
            success: record.success,
            failure_reason: record.failure_reason,
        }
    }
}

#[derive(Deserialize)]
pub struct ListLoginRecordsQuery {
    #[serde(default = "default_offset")]
    offset: u32,
    #[serde(default = "default_limit")]
    limit: u32,
    #[serde(default)]
    admin_id: Option<i64>,
    #[serde(default)]
    search: Option<String>,
    #[serde(default)]
    success: Option<bool>,
    #[serde(default)]
    failure_reason: Option<String>,
    #[serde(default)]
    ip_address: Option<String>,
    #[serde(default)]
    login_at_start: Option<String>,
    #[serde(default)]
    login_at_end: Option<String>,
}

fn default_offset() -> u32 {
    0
}

fn default_limit() -> u32 {
    20
}

pub async fn list_login_records(
    State(state): State<AppState>,
    Query(query): Query<ListLoginRecordsQuery>,
) -> ApiResponse<PaginatedResponse<LoginRecordPublic>> {
    let filter = login_record::LoginRecordFilter {
        admin_id: query.admin_id,
        search: query.search.clone(),
        success: query.success,
        failure_reason: query.failure_reason.clone(),
        ip_address: query.ip_address.clone(),
        login_at_start: query.login_at_start.clone(),
        login_at_end: query.login_at_end.clone(),
    };

    let (records, total) =
        match login_record::list_login_records(&state.pool, query.offset, query.limit, &filter)
            .await
        {
            Ok(result) => result,
            Err(e) => {
                tracing::error!("Database error: {}", e);
                return AppError::Internal("Service unavailable".to_string()).into_response();
            }
        };

    let items: Vec<LoginRecordPublic> = records.into_iter().map(|r| r.into()).collect();

    ApiResponse::success(PaginatedResponse {
        items,
        total,
        offset: query.offset,
        limit: query.limit,
    })
}

pub async fn get_login_record(
    Path(id): Path<i64>,
    State(state): State<AppState>,
) -> ApiResponse<LoginRecordPublic> {
    let record = match login_record::get_login_record_by_id(&state.pool, id).await {
        Ok(Some(record)) => record,
        Ok(None) => {
            return AppError::NotFound("Login record not found".to_string()).into_response();
        }
        Err(e) => {
            tracing::error!("Database error: {}", e);
            return AppError::Internal("Service unavailable".to_string()).into_response();
        }
    };

    ApiResponse::success(record.into())
}

pub fn router() -> Router<AppState> {
    Router::new()
        .route("/login-records", axum::routing::get(list_login_records))
        .route("/login-records/:id", axum::routing::get(get_login_record))
}
