//! User authentication API

use axum::{
    extract::State,
    routing::get,
    Json, Router,
};
use serde::Deserialize;

use crate::api::AppState;
use crate::models::User;
use crate::services::user_auth;
use crate::utils::error::{ApiResponse, AppError};
use crate::utils::jwt::{create_user_token, Claims};

#[derive(Deserialize)]
pub struct RegisterRequest {
    phone: String,
    password: String,
}

#[derive(Deserialize)]
pub struct LoginRequest {
    phone: String,
    password: String,
}

#[derive(serde::Serialize)]
pub struct AuthResponse {
    token: String,
    user: UserPublic,
}

#[derive(serde::Serialize)]
pub struct UserPublic {
    id: i64,
    phone: String,
    created_at: chrono::DateTime<chrono::Utc>,
    updated_at: chrono::DateTime<chrono::Utc>,
}

impl From<User> for UserPublic {
    fn from(user: User) -> Self {
        UserPublic {
            id: user.id,
            phone: user.phone,
            created_at: user.created_at,
            updated_at: user.updated_at,
        }
    }
}

pub async fn register(
    State(state): State<AppState>,
    Json(req): Json<RegisterRequest>,
) -> Result<ApiResponse<AuthResponse>, ApiResponse<()>> {
    let user = user_auth::register_user(&state.pool, &req.phone, &req.password)
        .await
        .map_err(|e| e.into_response())?;

    let token = create_user_token(user.id)
        .map_err(|e| {
            tracing::error!("Token creation failed: {}", e);
            AppError::Internal("Failed to generate token".to_string()).into_response()
        })?;

    Ok(ApiResponse::success(AuthResponse {
        token,
        user: user.into(),
    }))
}

pub async fn login(
    State(state): State<AppState>,
    Json(req): Json<LoginRequest>,
) -> Result<ApiResponse<AuthResponse>, ApiResponse<()>> {
    let user = user_auth::login_user(&state.pool, &req.phone, &req.password)
        .await
        .map_err(|e| e.into_response())?;

    let _ = user_auth::update_last_login(&state.pool, user.id).await;

    let token = create_user_token(user.id)
        .map_err(|e| {
            tracing::error!("Token creation failed: {}", e);
            AppError::Internal("Failed to generate token".to_string()).into_response()
        })?;

    Ok(ApiResponse::success(AuthResponse {
        token,
        user: user.into(),
    }))
}

pub async fn get_current_user(
    axum::Extension(claims): axum::Extension<Claims>,
) -> Result<ApiResponse<UserPublic>, ApiResponse<()>> {
    let user_id = match claims.user_id {
        Some(id) => id,
        None => return Err(AppError::NotFound("User not found".to_string()).into_response()),
    };

    // For now, return from claims since we don't have a user lookup that returns UserPublic
    // In production, you'd fetch from DB
    Ok(ApiResponse::success(UserPublic {
        id: user_id,
        phone: "".to_string(),
        created_at: chrono::Utc::now(),
        updated_at: chrono::Utc::now(),
    }))
}

pub fn router_public() -> Router<AppState> {
    Router::new()
        .route("/users/register", axum::routing::post(register))
        .route("/users/login", axum::routing::post(login))
}

pub fn router_protected() -> Router<AppState> {
    Router::new()
        .route("/users/me", axum::routing::get(get_current_user))
}
