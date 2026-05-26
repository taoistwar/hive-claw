use axum::{
    extract::State,
    Json, Router,
};
use serde::Deserialize;

use crate::api::AppState;
use crate::models::Admin;
use crate::services::{admin, auth as auth_service};
use crate::utils::error::{ApiResponse, AppError};
use crate::utils::jwt::{create_token, Claims};
use crate::utils::logging::mask_phone;
use crate::utils::password::verify_password;

#[derive(Deserialize)]
pub struct LoginRequest {
    phone: String,
    password: String,
}

#[derive(serde::Serialize)]
pub struct LoginResponse {
    token: String,
    admin: AdminPublic,
}

#[derive(serde::Serialize)]
pub struct AdminPublic {
    id: i64,
    phone: String,
    nickname: String,
    role: i8,
    status: i8,
}

impl From<Admin> for AdminPublic {
    fn from(admin: Admin) -> Self {
        AdminPublic {
            id: admin.id,
            phone: admin.phone,
            nickname: admin.nickname,
            role: admin.role as i8,
            status: admin.status,
        }
    }
}

pub async fn login(
    State(state): State<AppState>,
    Json(req): Json<LoginRequest>,
) -> ApiResponse<LoginResponse> {
    let ip_address = "unknown";

    let is_locked = match auth_service::is_account_locked(&state.redis, &req.phone).await {
        Ok(locked) => locked,
        Err(e) => {
            tracing::error!("Redis check failed: {}", e);
            return AppError::Internal("Service unavailable".to_string()).into_response();
        }
    };

    if is_locked {
        return AppError::AccountLocked(
            "Account locked due to too many failed login attempts. Try again in 15 minutes"
                .to_string(),
        )
        .into_response();
    }

    let admin = match admin::find_admin_by_phone(&state.pool, &req.phone).await {
        Ok(Some(admin)) => admin,
        Ok(None) => {
            let _ = auth_service::increment_failed_attempts(&state.redis, &req.phone).await;
            return AppError::WrongPassword("Invalid phone or password".to_string()).into_response();
        }
        Err(e) => {
            tracing::error!("Database error: {}", e);
            return AppError::Internal("Service unavailable".to_string()).into_response();
        }
    };

    if admin.status == 0 {
        return AppError::AccountDisabled("Account has been disabled".to_string()).into_response();
    }

    let password_valid = match verify_password(&req.password, &admin.password_hash) {
        Ok(valid) => valid,
        Err(e) => {
            tracing::error!("Password verification error: {}", e);
            return AppError::Internal("Service unavailable".to_string()).into_response();
        }
    };

    if !password_valid {
        let failed_count = match auth_service::increment_failed_attempts(&state.redis, &req.phone).await {
            Ok(count) => count,
            Err(e) => {
                tracing::error!("Redis increment failed: {}", e);
                return AppError::Internal("Service unavailable".to_string()).into_response();
            }
        };

        let remaining = 5 - failed_count;

        if let Err(e) = auth_service::create_login_record(
            &state.pool,
            admin.id,
            &admin.phone,
            &admin.nickname,
            ip_address,
            false,
            Some("WRONG_PASSWORD"),
        )
        .await
        {
            tracing::error!("Failed to persist failed-login record: {}", e);
        }

        tracing::info!(
            admin_id = admin.id,
            phone = %mask_phone(&admin.phone),
            outcome = "wrong_password",
            operation = "login",
            remaining = remaining,
            "admin login failed"
        );

        if remaining <= 0 {
            return AppError::AccountLocked(
                "Account locked due to too many failed login attempts. Try again in 15 minutes"
                    .to_string(),
            )
            .into_response();
        }
        return AppError::WrongPassword(format!(
            "Invalid phone or password. {} attempts remaining",
            remaining
        ))
        .into_response();
    }

    let _ = auth_service::reset_failed_attempts(&state.redis, &req.phone).await;

    let _ = admin::update_last_login(&state.pool, admin.id).await;

    if let Err(e) = auth_service::create_login_record(
        &state.pool,
        admin.id,
        &admin.phone,
        &admin.nickname,
        ip_address,
        true,
        None,
    )
    .await
    {
        tracing::error!("Failed to persist successful-login record: {}", e);
    }

    tracing::info!(
        admin_id = admin.id,
        phone = %mask_phone(&admin.phone),
        outcome = "success",
        operation = "login",
        "admin login success"
    );

    let token = match create_token(admin.id, admin.role) {
        Ok(token) => token,
        Err(e) => {
            tracing::error!("Token creation failed: {}", e);
            return AppError::Internal("Failed to generate token".to_string()).into_response();
        }
    };

    let admin_public: AdminPublic = admin.into();

    ApiResponse::success(LoginResponse {
        token,
        admin: admin_public,
    })
}

pub async fn logout() -> ApiResponse<()> {
    ApiResponse::success(())
}

pub async fn get_current_user(
    State(state): State<AppState>,
    axum::Extension(claims): axum::Extension<Claims>,
) -> ApiResponse<AdminPublic> {
    let admin = match admin::get_admin_by_id(&state.pool, claims.admin_id).await {
        Ok(Some(admin)) => admin,
        Ok(None) => return AppError::AdminNotFound("Admin not found".to_string()).into_response(),
        Err(e) => {
            tracing::error!("Database error: {}", e);
            return AppError::Internal("Service unavailable".to_string()).into_response();
        }
    };

    ApiResponse::success(admin.into())
}

pub fn router_public() -> Router<AppState> {
    Router::new()
        .route("/auth/login", axum::routing::post(login))
}

pub fn router_protected() -> Router<AppState> {
    Router::new()
        .route("/auth/logout", axum::routing::post(logout))
        .route("/auth/me", axum::routing::get(get_current_user))
}
