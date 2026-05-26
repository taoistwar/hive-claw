use axum::{
    extract::State,
    Router,
};

use crate::api::AppState;
use crate::services::dashboard;
use crate::utils::error::{ApiResponse, AppError};

pub async fn get_stats(
    State(state): State<AppState>,
) -> ApiResponse<dashboard::DashboardStats> {
    let stats = match dashboard::get_stats(&state.pool).await {
        Ok(stats) => stats,
        Err(e) => {
            tracing::error!("Database error: {}", e);
            return AppError::Internal("Service unavailable".to_string()).into_response();
        }
    };

    ApiResponse::success(stats)
}

pub fn router() -> Router<AppState> {
    Router::new()
        .route("/dashboard/stats", axum::routing::get(get_stats))
}
