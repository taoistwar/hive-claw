use axum::{
    extract::{Path, Query, State},
    Json, Router,
};
use serde::Deserialize;

use crate::api::AppState;
use crate::models::{Admin, Role};
use crate::services::admin;
use crate::utils::error::{ApiResponse, AppError};
use crate::utils::jwt::Claims;
use crate::utils::password::hash_password;

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
            role: admin.role,
            status: admin.status,
        }
    }
}

#[derive(serde::Serialize)]
pub struct PaginatedResponse<T: serde::Serialize> {
    items: Vec<T>,
    total: u64,
    offset: u32,
    limit: u32,
}

#[derive(Deserialize)]
pub struct ListAdminsQuery {
    #[serde(default = "default_offset")]
    offset: u32,
    #[serde(default = "default_limit")]
    limit: u32,
}

fn default_offset() -> u32 {
    0
}

fn default_limit() -> u32 {
    20
}

#[derive(Deserialize)]
pub struct CreateAdminRequest {
    phone: String,
    nickname: String,
    password: String,
    role: i8,
}

#[derive(Deserialize)]
pub struct UpdateAdminRequest {
    nickname: String,
    role: i8,
}

#[derive(Deserialize)]
pub struct ToggleStatusRequest {
    status: i8,
}

pub async fn list_admins(
    State(state): State<AppState>,
    Query(query): Query<ListAdminsQuery>,
    axum::Extension(claims): axum::Extension<Claims>,
) -> ApiResponse<PaginatedResponse<AdminPublic>> {
    let caller_role = match Role::try_from(claims.role) {
        Ok(role) => role,
        Err(_) => return AppError::Forbidden("Invalid role".to_string()).into_response(),
    };

    if !caller_role.can_manage_admins() {
        return AppError::Forbidden("Insufficient permissions".to_string()).into_response();
    }

    let (admins, total) = match admin::list_admins(&state.pool, query.offset, query.limit).await {
        Ok(result) => result,
        Err(e) => {
            tracing::error!("Database error: {}", e);
            return AppError::Internal("Service unavailable".to_string()).into_response();
        }
    };

    let items: Vec<AdminPublic> = admins.into_iter().map(|a| a.into()).collect();

    ApiResponse::success(PaginatedResponse {
        items,
        total,
        offset: query.offset,
        limit: query.limit,
    })
}

pub async fn get_admin(
    Path(id): Path<i64>,
    State(state): State<AppState>,
) -> ApiResponse<AdminPublic> {
    let admin = match admin::get_admin_by_id(&state.pool, id).await {
        Ok(Some(admin)) => admin,
        Ok(None) => return AppError::NotFound("Admin not found".to_string()).into_response(),
        Err(e) => {
            tracing::error!("Database error: {}", e);
            return AppError::Internal("Service unavailable".to_string()).into_response();
        }
    };

    ApiResponse::success(admin.into())
}

pub async fn create_admin(
    State(state): State<AppState>,
    axum::Extension(claims): axum::Extension<Claims>,
    Json(req): Json<CreateAdminRequest>,
) -> ApiResponse<AdminPublic> {
    let caller_role = match Role::try_from(claims.role) {
        Ok(role) => role,
        Err(_) => return AppError::Forbidden("Invalid role".to_string()).into_response(),
    };

    if !caller_role.can_manage_admins() {
        return AppError::Forbidden("Insufficient permissions".to_string()).into_response();
    }

    let target_role = match Role::try_from(req.role) {
        Ok(role) => role,
        Err(_) => return AppError::BadRequest("Invalid role value".to_string()).into_response(),
    };

    if !caller_role.can_modify_role(&target_role) {
        return AppError::Forbidden("Cannot assign this role".to_string()).into_response();
    }

    let password_hash = match hash_password(&req.password) {
        Ok(hash) => hash,
        Err(e) => return e.into_response(),
    };

    let new_admin = match admin::create_admin(
        &state.pool,
        &req.phone,
        &req.nickname,
        &password_hash,
        req.role,
    )
    .await
    {
        Ok(admin) => admin,
        Err(e) => {
            if e.to_string().contains("already exists") {
                return AppError::Conflict("Phone number already exists".to_string()).into_response();
            }
            tracing::error!("Database error: {}", e);
            return AppError::Internal("Service unavailable".to_string()).into_response();
        }
    };

    ApiResponse::success(new_admin.into())
}

pub async fn update_admin(
    Path(id): Path<i64>,
    State(state): State<AppState>,
    axum::Extension(claims): axum::Extension<Claims>,
    Json(req): Json<UpdateAdminRequest>,
) -> ApiResponse<AdminPublic> {
    let caller_role = match Role::try_from(claims.role) {
        Ok(role) => role,
        Err(_) => return AppError::Forbidden("Invalid role".to_string()).into_response(),
    };

    if !caller_role.can_manage_admins() {
        return AppError::Forbidden("Insufficient permissions".to_string()).into_response();
    }

    let target_role = match Role::try_from(req.role) {
        Ok(role) => role,
        Err(_) => return AppError::BadRequest("Invalid role value".to_string()).into_response(),
    };

    if !caller_role.can_modify_role(&target_role) {
        return AppError::Forbidden("Cannot assign this role".to_string()).into_response();
    }

    let updated_admin = match admin::update_admin(&state.pool, id, &req.nickname, req.role).await {
        Ok(admin) => admin,
        Err(_) => return AppError::NotFound("Admin not found".to_string()).into_response(),
    };

    ApiResponse::success(updated_admin.into())
}

pub async fn delete_admin(
    Path(id): Path<i64>,
    State(state): State<AppState>,
    axum::Extension(claims): axum::Extension<Claims>,
) -> ApiResponse<()> {
    let caller_role = match Role::try_from(claims.role) {
        Ok(role) => role,
        Err(_) => return AppError::Forbidden("Invalid role".to_string()).into_response(),
    };

    if !caller_role.can_manage_admins() {
        return AppError::Forbidden("Insufficient permissions".to_string()).into_response();
    }

    match admin::delete_admin(&state.pool, id).await {
        Ok(_) => ApiResponse::success(()),
        Err(e) => {
            let msg = e.to_string();
            if msg.contains("not found") {
                AppError::NotFound(msg).into_response()
            } else if msg.contains("super admin") {
                AppError::Forbidden(msg).into_response()
            } else {
                tracing::error!("Database error: {}", e);
                AppError::Internal("Service unavailable".to_string()).into_response()
            }
        }
    }
}

pub async fn toggle_admin_status(
    Path(id): Path<i64>,
    State(state): State<AppState>,
    axum::Extension(claims): axum::Extension<Claims>,
    Json(req): Json<ToggleStatusRequest>,
) -> ApiResponse<AdminPublic> {
    let caller_role = match Role::try_from(claims.role) {
        Ok(role) => role,
        Err(_) => return AppError::Forbidden("Invalid role".to_string()).into_response(),
    };

    if !caller_role.can_manage_admins() {
        return AppError::Forbidden("Insufficient permissions".to_string()).into_response();
    }

    if req.status != 0 && req.status != 1 {
        return AppError::BadRequest("Status must be 0 or 1".to_string()).into_response();
    }

    match admin::toggle_admin_status(&state.pool, id, req.status).await {
        Ok(admin) => ApiResponse::success(admin.into()),
        Err(e) => {
            let msg = e.to_string();
            if msg.contains("not found") {
                AppError::NotFound(msg).into_response()
            } else if msg.contains("last active super admin") {
                AppError::Forbidden(msg).into_response()
            } else {
                tracing::error!("Database error: {}", e);
                AppError::Internal("Service unavailable".to_string()).into_response()
            }
        }
    }
}

pub fn router() -> Router<AppState> {
    Router::new()
        .route("/admins", axum::routing::get(list_admins).post(create_admin))
        .route("/admins/:id", axum::routing::get(get_admin).put(update_admin).delete(delete_admin))
        .route("/admins/:id/status", axum::routing::put(toggle_admin_status))
}
