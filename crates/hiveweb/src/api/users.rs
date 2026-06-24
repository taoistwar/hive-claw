//! User auth API (phone-based auth removed — users synced from cloud_user)
use axum::Router;
use crate::api::AppState;

pub fn router() -> Router<AppState> { Router::new() }
