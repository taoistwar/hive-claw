use axum::{
    Extension, Router,
    extract::{Path, Query, State},
    routing::get,
};
use chrono::{DateTime, NaiveDateTime};
use serde::{Deserialize, Serialize};

use crate::{
    api::AppState,
    models::{Role, RuntimeAuditLog},
    services::runtime_audit::{self, RuntimeAuditFilter},
    utils::{
        error::{ApiResponse, AppError},
        jwt::Claims,
    },
};

const MAX_PAGE_SIZE: u32 = 100;

#[derive(Debug, Serialize)]
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

impl From<RuntimeAuditLog> for RuntimeAuditLogPublic {
    fn from(log: RuntimeAuditLog) -> Self {
        Self {
            id: log.id,
            request_id: log.request_id,
            session_id: log.session_id,
            agent_id: log.agent_id,
            plugin_id: log.plugin_id,
            function_id: log.function_id,
            capability: log.capability,
            event_type: log.event_type,
            outcome: log.outcome,
            elapsed_ms: log.elapsed_ms,
            error_message: log.error_message,
            payload_summary: log.payload_summary,
            occurred_at: log.occurred_at.and_utc().to_rfc3339(),
        }
    }
}

#[derive(Debug, Serialize)]
pub struct PaginatedResponse<T: Serialize> {
    items: Vec<T>,
    total: u64,
    offset: u32,
    limit: u32,
}

#[derive(Debug, Deserialize)]
pub struct ListRuntimeAuditLogsQuery {
    #[serde(default)]
    offset: u32,
    #[serde(default = "default_limit")]
    limit: u32,
    event_type: Option<String>,
    outcome: Option<String>,
    capability: Option<String>,
    request_id: Option<String>,
    session_id: Option<i64>,
    agent_id: Option<i64>,
    occurred_at_start: Option<String>,
    occurred_at_end: Option<String>,
}

fn default_limit() -> u32 {
    20
}

fn require_runtime_audit_access(claims: &Claims) -> Result<(), AppError> {
    let role = Role::try_from(claims.role)
        .map_err(|_| AppError::InsufficientPermission("Insufficient permission".into()))?;
    if role.can_view_runtime_audit_logs() {
        Ok(())
    } else {
        Err(AppError::InsufficientPermission(
            "Insufficient permission".into(),
        ))
    }
}

fn parse_datetime(value: Option<&str>, field: &str) -> Result<Option<NaiveDateTime>, AppError> {
    let Some(value) = value else {
        return Ok(None);
    };

    DateTime::parse_from_rfc3339(value)
        .map(|value| Some(value.naive_utc()))
        .or_else(|_| NaiveDateTime::parse_from_str(value, "%Y-%m-%d %H:%M:%S").map(Some))
        .map_err(|_| {
            AppError::BadRequest(format!("{field} must be RFC3339 or YYYY-MM-DD HH:MM:SS"))
        })
}

fn to_filter(query: &ListRuntimeAuditLogsQuery) -> Result<RuntimeAuditFilter, AppError> {
    Ok(RuntimeAuditFilter {
        event_type: query.event_type.clone(),
        outcome: query.outcome.clone(),
        capability: query.capability.clone(),
        request_id: query.request_id.clone(),
        session_id: query.session_id,
        agent_id: query.agent_id,
        occurred_at_start: parse_datetime(query.occurred_at_start.as_deref(), "occurred_at_start")?,
        occurred_at_end: parse_datetime(query.occurred_at_end.as_deref(), "occurred_at_end")?,
    })
}

pub async fn list_runtime_audit_logs(
    State(state): State<AppState>,
    Extension(claims): Extension<Claims>,
    Query(query): Query<ListRuntimeAuditLogsQuery>,
) -> ApiResponse<PaginatedResponse<RuntimeAuditLogPublic>> {
    if let Err(error) = require_runtime_audit_access(&claims) {
        return error.into_response();
    }
    let filter = match to_filter(&query) {
        Ok(filter) => filter,
        Err(error) => return error.into_response(),
    };
    let limit = query.limit.clamp(1, MAX_PAGE_SIZE);
    match runtime_audit::list_runtime_audit_logs(&state.pool, query.offset, limit, &filter).await {
        Ok((items, total)) => ApiResponse::success(PaginatedResponse {
            items: items.into_iter().map(Into::into).collect(),
            total,
            offset: query.offset,
            limit,
        }),
        Err(_) => {
            tracing::error!(
                error_kind = "runtime_audit_query_failed",
                "runtime audit list query failed"
            );
            AppError::Internal("Service unavailable".into()).into_response()
        }
    }
}

pub async fn get_runtime_audit_log(
    State(state): State<AppState>,
    Extension(claims): Extension<Claims>,
    Path(id): Path<i64>,
) -> ApiResponse<RuntimeAuditLogPublic> {
    if let Err(error) = require_runtime_audit_access(&claims) {
        return error.into_response();
    }
    match runtime_audit::get_runtime_audit_log_by_id(&state.pool, id).await {
        Ok(Some(log)) => ApiResponse::success(log.into()),
        Ok(None) => AppError::NotFound("Runtime audit log not found".into()).into_response(),
        Err(_) => {
            tracing::error!(
                error_kind = "runtime_audit_query_failed",
                "runtime audit detail query failed"
            );
            AppError::Internal("Service unavailable".into()).into_response()
        }
    }
}

/// Super-only, read-only runtime audit API. There is intentionally no public
/// route for inserting, updating or deleting audit payloads.
pub fn router() -> Router<AppState> {
    Router::new()
        .route("/runtime-audit-logs", get(list_runtime_audit_logs))
        .route("/runtime-audit-logs/:id", get(get_runtime_audit_log))
}

#[cfg(test)]
mod tests {
    use super::{ListRuntimeAuditLogsQuery, require_runtime_audit_access, to_filter};
    use crate::utils::jwt::Claims;

    fn claims(role: i8) -> Claims {
        Claims {
            admin_id: Some(1),
            user_id: None,
            role,
            token_type: "admin".into(),
            exp: usize::MAX,
            iat: 0,
        }
    }

    #[test]
    fn runtime_audit_is_super_only() {
        assert!(require_runtime_audit_access(&claims(3)).is_ok());
        assert!(require_runtime_audit_access(&claims(1)).is_err());
        assert!(require_runtime_audit_access(&claims(2)).is_err());
        assert!(require_runtime_audit_access(&claims(99)).is_err());
    }

    #[test]
    fn parses_supported_dates_and_rejects_invalid_values() {
        let query = ListRuntimeAuditLogsQuery {
            offset: 0,
            limit: 20,
            event_type: None,
            outcome: None,
            capability: None,
            request_id: None,
            session_id: None,
            agent_id: None,
            occurred_at_start: Some("2026-07-23T08:30:00+08:00".into()),
            occurred_at_end: Some("2026-07-24 08:30:00".into()),
        };
        let filter = to_filter(&query).expect("valid date filters");
        assert_eq!(
            filter.occurred_at_start.unwrap().to_string(),
            "2026-07-23 00:30:00"
        );
        assert_eq!(
            filter.occurred_at_end.unwrap().to_string(),
            "2026-07-24 08:30:00"
        );

        let invalid = ListRuntimeAuditLogsQuery {
            occurred_at_start: Some("not-a-date".into()),
            ..query
        };
        assert!(to_filter(&invalid).is_err());
    }
}
