//! User auth API (phone-based auth removed — users synced from cloud_user)
use crate::api::AppState;
use axum::Router;

pub fn router() -> Router<AppState> {
    Router::new()
}
