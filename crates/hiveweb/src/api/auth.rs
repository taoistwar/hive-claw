use axum::{
    extract::State,
    Json, Router,
};
use serde::Deserialize;

use crate::api::AppState;
use crate::models::{Admin, Role};
use crate::services::{admin, auth as auth_service};
use crate::utils::error::{ApiResponse, AppError};
use crate::utils::jwt::{create_admin_token, Claims};
use crate::utils::logging::mask_phone;
use crate::utils::password::{verify_password, hash_password};

#[derive(Deserialize)]
pub struct LoginRequest {
    phone: String,
    password: String,
}

#[derive(Deserialize)]
pub struct ChangePasswordRequest {
    old_password: String,
    new_password: String,
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

    let role = match Role::try_from(admin.role) {
        Ok(role) => role,
        Err(e) => {
            tracing::error!("Invalid role: {}", e);
            return AppError::Internal("Invalid role configuration".to_string()).into_response();
        }
    };

    if !matches!(role, Role::System | Role::Super) {
        return AppError::NotAdministrator(
            "Only System and Super administrators can log in to the admin center".to_string(),
        )
        .into_response();
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

    let token = match create_admin_token(admin.id, admin.role) {
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
    let admin_id = match claims.admin_id {
        Some(id) => id,
        None => return AppError::AdminNotFound("Admin not found".to_string()).into_response(),
    };
    let admin = match admin::get_admin_by_id(&state.pool, admin_id).await {
        Ok(Some(admin)) => admin,
        Ok(None) => return AppError::AdminNotFound("Admin not found".to_string()).into_response(),
        Err(e) => {
            tracing::error!("Database error: {}", e);
            return AppError::Internal("Service unavailable".to_string()).into_response();
        }
    };

    ApiResponse::success(admin.into())
}

pub async fn change_password(
    State(state): State<AppState>,
    axum::Extension(claims): axum::Extension<Claims>,
    Json(req): Json<ChangePasswordRequest>,
) -> ApiResponse<()> {
    let admin_id = match claims.admin_id {
        Some(id) => id,
        None => return AppError::AdminNotFound("Admin not found".to_string()).into_response(),
    };

    let is_locked = match admin::get_admin_by_id(&state.pool, admin_id).await {
        Ok(Some(admin)) => {
            let locked = match auth_service::is_account_locked(&state.redis, &admin.phone).await {
                Ok(l) => l,
                Err(e) => {
                    tracing::error!("Redis check failed: {}", e);
                    return AppError::Internal("Service unavailable".to_string()).into_response();
                }
            };
            locked
        }
        Ok(None) => return AppError::AdminNotFound("Admin not found".to_string()).into_response(),
        Err(e) => {
            tracing::error!("Database error: {}", e);
            return AppError::Internal("Service unavailable".to_string()).into_response();
        }
    };

    if is_locked {
        return AppError::AccountLocked(
            "Account locked due to too many failed attempts. Try again in 15 minutes".to_string(),
        )
        .into_response();
    }

    let admin = match admin::get_admin_by_id(&state.pool, admin_id).await {
        Ok(Some(a)) => a,
        Ok(None) => return AppError::AdminNotFound("Admin not found".to_string()).into_response(),
        Err(e) => {
            tracing::error!("Database error: {}", e);
            return AppError::Internal("Service unavailable".to_string()).into_response();
        }
    };

    let old_valid = match verify_password(&req.old_password, &admin.password_hash) {
        Ok(v) => v,
        Err(e) => {
            tracing::error!("Password verification error: {}", e);
            return AppError::Internal("Service unavailable".to_string()).into_response();
        }
    };

    if !old_valid {
        let _ = auth_service::increment_failed_attempts(&state.redis, &admin.phone).await;
        tracing::info!(
            admin_id = admin.id,
            phone = %mask_phone(&admin.phone),
            outcome = "wrong_old_password",
            operation = "change_password",
            "admin provided wrong old password"
        );
        return AppError::WrongPassword("Old password is incorrect".to_string()).into_response();
    }

    let old_matches_new = match verify_password(&req.new_password, &admin.password_hash) {
        Ok(v) => v,
        Err(e) => {
            tracing::error!("Password comparison error: {}", e);
            return AppError::Internal("Service unavailable".to_string()).into_response();
        }
    };

    if old_matches_new {
        return AppError::NewPasswordSameAsOld("New password cannot be the same as old password".to_string()).into_response();
    }

    if let Err(e) = crate::utils::password::validate_password(&req.new_password) {
        return e.into_response();
    }

    let new_hash = match hash_password(&req.new_password) {
        Ok(h) => h,
        Err(e) => {
            tracing::error!("Password hashing failed: {}", e);
            return AppError::Internal("Failed to hash password".to_string()).into_response();
        }
    };

    if let Err(e) = admin::update_admin_password(&state.pool, admin_id, &new_hash).await {
        tracing::error!("Password update failed: {}", e);
        return AppError::Internal("Failed to update password".to_string()).into_response();
    }

    let _ = auth_service::reset_failed_attempts(&state.redis, &admin.phone).await;

    tracing::info!(
        admin_id = admin.id,
        phone = %mask_phone(&admin.phone),
        outcome = "success",
        operation = "change_password",
        "admin password changed successfully"
    );

    ApiResponse::success(())
}

pub fn router_public() -> Router<AppState> {
    Router::new()
        .route("/auth/login", axum::routing::post(login))
}

pub fn router_protected() -> Router<AppState> {
    Router::new()
        .route("/auth/logout", axum::routing::post(logout))
        .route("/auth/me", axum::routing::get(get_current_user))
        .route("/auth/change-password", axum::routing::post(change_password))
}
