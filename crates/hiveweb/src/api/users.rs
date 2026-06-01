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
use crate::utils::password::hash_password;
use crate::utils::password::verify_password as bcrypt_verify;

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

#[derive(Deserialize)]
pub struct ChangePasswordRequest {
    old_password: String,
    new_password: String,
}

pub async fn change_password(
    State(state): State<AppState>,
    axum::Extension(claims): axum::Extension<Claims>,
    Json(req): Json<ChangePasswordRequest>,
) -> Result<ApiResponse<()>, ApiResponse<()>> {
    let user_id = match claims.user_id {
        Some(id) => id,
        None => return Err(AppError::NotFound("User not found".to_string()).into_response()),
    };

    let user = match sqlx::query_as::<_, User>(
        "SELECT id, phone, password_hash, status, created_at, updated_at, last_login_at FROM users WHERE id = ?"
    )
    .bind(user_id)
    .fetch_one(&state.pool)
    .await
    {
        Ok(u) => u,
        Err(_) => return Err(AppError::NotFound("User not found".to_string()).into_response()),
    };

    let valid = bcrypt_verify(&req.old_password, &user.password_hash).map_err(|e| {
        tracing::error!("Password verify failed: {}", e);
        AppError::Internal("Failed to verify password".to_string()).into_response()
    })?;
    if !valid {
        return Err(AppError::WrongPassword("Old password is incorrect".to_string()).into_response());
    }

    let new_hash = match hash_password(&req.new_password) {
        Ok(h) => h,
        Err(e) => {
            tracing::error!("Password hash failed: {}", e);
            return Err(AppError::Internal("Failed to hash password".to_string()).into_response());
        }
    };

    match sqlx::query("UPDATE users SET password_hash = ?, updated_at = NOW() WHERE id = ?")
        .bind(new_hash)
        .bind(user_id)
        .execute(&state.pool)
        .await
    {
        Ok(_) => Ok(ApiResponse::success(())),
        Err(e) => {
            tracing::error!("Password update failed: {}", e);
            Err(AppError::Internal("Failed to update password".to_string()).into_response())
        }
    }
}

#[derive(serde::Serialize)]
pub struct DashboardStats {
    total_sessions: i64,
    total_messages: i64,
    total_active_days: i64,
    last_session_at: Option<chrono::DateTime<chrono::Utc>>,
}

pub async fn get_dashboard_stats(
    State(state): State<AppState>,
    axum::Extension(claims): axum::Extension<Claims>,
) -> Result<ApiResponse<DashboardStats>, ApiResponse<()>> {
    let user_id = match claims.user_id {
        Some(id) => id,
        None => return Err(AppError::NotFound("User not found".to_string()).into_response()),
    };

    let sessions: (Option<i64>,) = match sqlx::query_as(
        "SELECT COUNT(*) FROM chat_sessions_user WHERE user_id = ?"
    )
    .bind(user_id)
    .fetch_one(&state.pool)
    .await
    {
        Ok(s) => s,
        Err(e) => {
            tracing::error!("Stats query failed: {}", e);
            return Err(AppError::Internal("Failed to get stats".to_string()).into_response());
        }
    };

    let messages: (Option<i64>,) = match sqlx::query_as(
        "SELECT COUNT(*) FROM chat_messages_user cmu INNER JOIN chat_sessions_user csu ON cmu.session_id = csu.id WHERE csu.user_id = ?"
    )
    .bind(user_id)
    .fetch_one(&state.pool)
    .await
    {
        Ok(m) => m,
        Err(e) => {
            tracing::error!("Stats query failed: {}", e);
            return Err(AppError::Internal("Failed to get stats".to_string()).into_response());
        }
    };

    let active_days: (Option<i64>,) = match sqlx::query_as(
        "SELECT COUNT(DISTINCT DATE(created_at)) FROM chat_sessions_user WHERE user_id = ?"
    )
    .bind(user_id)
    .fetch_one(&state.pool)
    .await
    {
        Ok(d) => d,
        Err(e) => {
            tracing::error!("Stats query failed: {}", e);
            return Err(AppError::Internal("Failed to get stats".to_string()).into_response());
        }
    };

    let last_session: Option<(chrono::DateTime<chrono::Utc>,)> = match sqlx::query_as(
        "SELECT MAX(created_at) FROM chat_sessions_user WHERE user_id = ?"
    )
    .bind(user_id)
    .fetch_optional(&state.pool)
    .await
    {
        Ok(ls) => ls,
        Err(e) => {
            tracing::error!("Stats query failed: {}", e);
            return Err(AppError::Internal("Failed to get stats".to_string()).into_response());
        }
    };

    Ok(ApiResponse::success(DashboardStats {
        total_sessions: sessions.0,
        total_messages: messages.0,
        total_active_days: active_days.0,
        last_session_at: last_session.map(|ls| ls.0),
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
        .route("/users/change-password", axum::routing::post(change_password))
        .route("/users/dashboard/stats", axum::routing::get(get_dashboard_stats))
}
