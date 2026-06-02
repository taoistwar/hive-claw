use axum::{
    Router,
    extract::{Query, State},
};
use serde::Deserialize;

use crate::api::AppState;
use crate::services::dashboard;
use crate::utils::error::{ApiResponse, AppError};

pub async fn get_stats(State(state): State<AppState>) -> ApiResponse<dashboard::DashboardStats> {
    match dashboard::get_stats(&state.pool).await {
        Ok(stats) => ApiResponse::success(stats),
        Err(e) => {
            tracing::error!("Dashboard stats failed: {}", e);
            AppError::Internal("Service unavailable".to_string()).into_response()
        }
    }
}

#[derive(Deserialize)]
pub struct RecentLoginsQuery {
    #[serde(default = "default_limit")]
    pub limit: u32,
}

fn default_limit() -> u32 {
    10
}

pub async fn get_recent_logins(
    State(state): State<AppState>,
    Query(query): Query<RecentLoginsQuery>,
) -> ApiResponse<Vec<dashboard::RecentLoginRow>> {
    match dashboard::get_recent_logins(&state.pool, query.limit).await {
        Ok(rows) => ApiResponse::success(rows),
        Err(e) => {
            tracing::error!("Dashboard recent-logins failed: {}", e);
            AppError::Internal("Service unavailable".to_string()).into_response()
        }
    }
}

pub fn router() -> Router<AppState> {
    Router::new()
        .route("/dashboard/stats", axum::routing::get(get_stats))
        .route(
            "/dashboard/recent-logins",
            axum::routing::get(get_recent_logins),
        )
}
