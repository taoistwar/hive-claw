//! User management API handlers

use axum::{
    Json, Router,
    extract::{Path, Query, State},
    routing::get,
};
use serde::Deserialize;

use crate::api::AppState;
use crate::models::User;
use crate::services::admin as admin_svc;
use crate::services::audit::{self as audit_svc, Operation};
use crate::utils::error::{ApiResponse, AppError};
use crate::utils::jwt::Claims;
use crate::utils::password::hash_password;
use crate::utils::validation::validate_phone;

#[derive(Deserialize)]
pub struct ListUsersQuery {
    #[serde(default = "default_page")]
    pub page: u32,
    #[serde(default = "default_page_size")]
    pub page_size: u32,
    pub search: Option<String>,
}

fn default_page() -> u32 {
    1
}
fn default_page_size() -> u32 {
    10
}

#[derive(serde::Serialize)]
pub struct UserListResponse {
    users: Vec<UserPublic>,
    total: u64,
    page: u32,
    page_size: u32,
}

#[derive(serde::Serialize)]
pub struct UserPublic {
    id: i64,
    phone: String,
    status: i8,
    created_at: chrono::DateTime<chrono::Utc>,
    updated_at: chrono::DateTime<chrono::Utc>,
}

impl From<User> for UserPublic {
    fn from(user: User) -> Self {
        UserPublic {
            id: user.id,
            phone: user.phone,
            status: user.status,
            created_at: user.created_at,
            updated_at: user.updated_at,
        }
    }
}

pub async fn list_users(
    State(state): State<AppState>,
    axum::Extension(claims): axum::Extension<Claims>,
    Query(query): Query<ListUsersQuery>,
) -> ApiResponse<UserListResponse> {
    let admin_id = match claims.admin_id {
        Some(id) => id,
        None => return AppError::AdminNotFound("Admin not found".to_string()).into_response(),
    };

    let admin = match admin_svc::get_admin_by_id(&state.pool, admin_id).await {
        Ok(Some(a)) => a,
        Ok(None) => return AppError::AdminNotFound("Admin not found".to_string()).into_response(),
        Err(e) => {
            tracing::error!("Database error: {}", e);
            return AppError::Internal("Service unavailable".to_string()).into_response();
        }
    };

    let pool = &state.pool;
    let search = query.search.clone().unwrap_or_default();
    let where_clause = if search.is_empty() {
        String::from("WHERE 1=1")
    } else {
        String::from("WHERE phone LIKE ?")
    };

    let search_pattern = format!("%{}%", search);
    let total: (i64,) =
        match sqlx::query_as(&format!("SELECT COUNT(*) FROM users {}", where_clause))
            .bind(if search.is_empty() {
                "%"
            } else {
                &search_pattern
            })
            .fetch_one(pool)
            .await
        {
            Ok(t) => t,
            Err(e) => {
                tracing::error!("Count query failed: {}", e);
                return AppError::Internal("Service unavailable".to_string()).into_response();
            }
        };

    let offset = ((query.page - 1) * query.page_size) as i64;
    let limit = query.page_size as i64;

    let users: Vec<User> = match sqlx::query_as(&format!(
        "SELECT id, phone, password_hash, status, created_at, updated_at, last_login_at FROM users {} ORDER BY id DESC LIMIT ? OFFSET ?",
        where_clause
    ))
    .bind(if search.is_empty() { "%" } else { &search_pattern })
    .bind(limit)
    .bind(offset)
    .fetch_all(pool)
    .await
    {
        Ok(u) => u,
        Err(e) => {
            tracing::error!("List query failed: {}", e);
            return AppError::Internal("Service unavailable".to_string()).into_response();
        }
    };

    let _ = audit_svc::record(
        pool,
        admin_id,
        &admin.phone,
        None,
        "",
        Operation::Update,
        Some(serde_json::json!({
            "resource": "user",
            "action": "list",
            "page": query.page,
            "page_size": query.page_size,
            "search": search,
        })),
    )
    .await;

    ApiResponse::success(UserListResponse {
        users: users.into_iter().map(|u| u.into()).collect(),
        total: total.0 as u64,
        page: query.page,
        page_size: query.page_size,
    })
}

pub async fn create_user(
    State(state): State<AppState>,
    axum::Extension(claims): axum::Extension<Claims>,
    Json(req): Json<CreateUserRequest>,
) -> ApiResponse<UserPublic> {
    let admin_id = match claims.admin_id {
        Some(id) => id,
        None => return AppError::AdminNotFound("Admin not found".to_string()).into_response(),
    };

    let admin = match admin_svc::get_admin_by_id(&state.pool, admin_id).await {
        Ok(Some(a)) => a,
        Ok(None) => return AppError::AdminNotFound("Admin not found".to_string()).into_response(),
        Err(e) => {
            tracing::error!("Database error: {}", e);
            return AppError::Internal("Service unavailable".to_string()).into_response();
        }
    };

    if let Err(e) = validate_phone(&req.phone) {
        return e.into_response();
    }

    let hash = match hash_password(&req.password) {
        Ok(h) => h,
        Err(e) => {
            tracing::error!("Password hash failed: {}", e);
            return AppError::Internal("Failed to hash password".to_string()).into_response();
        }
    };

    let result =
        match sqlx::query("INSERT INTO users (phone, password_hash, status) VALUES (?, ?, 1)")
            .bind(&req.phone)
            .bind(hash)
            .execute(&state.pool)
            .await
        {
            Ok(r) => r,
            Err(e) if e.to_string().contains("UNIQUE") => {
                return AppError::PhoneAlreadyExists(format!("手机号 {} 已存在", req.phone))
                    .into_response();
            }
            Err(e) => {
                tracing::error!("Create user failed: {}", e);
                return AppError::Internal("Failed to create user".to_string()).into_response();
            }
        };

    let user_id = result.last_insert_id();
    let user = match sqlx::query_as::<_, User>(
        "SELECT id, phone, password_hash, status, created_at, updated_at, last_login_at FROM users WHERE id = ?",
    )
    .bind(user_id)
    .fetch_one(&state.pool)
    .await
    {
        Ok(u) => u,
        Err(e) => {
            tracing::error!("Fetch user after create failed: {}", e);
            return AppError::Internal("Failed to fetch created user".to_string()).into_response();
        }
    };

    let _ = audit_svc::record(
        &state.pool,
        admin_id,
        &admin.phone,
        Some(user.id),
        &user.phone,
        Operation::Create,
        Some(serde_json::json!({ "resource": "user" })),
    )
    .await;

    ApiResponse::success(user.into())
}

#[derive(Deserialize)]
pub struct CreateUserRequest {
    phone: String,
    password: String,
}

pub async fn delete_user(
    State(state): State<AppState>,
    axum::Extension(claims): axum::Extension<Claims>,
    Path(id): Path<i64>,
) -> ApiResponse<()> {
    let admin_id = match claims.admin_id {
        Some(id) => id,
        None => return AppError::AdminNotFound("Admin not found".to_string()).into_response(),
    };

    let admin = match admin_svc::get_admin_by_id(&state.pool, admin_id).await {
        Ok(Some(a)) => a,
        Ok(None) => return AppError::AdminNotFound("Admin not found".to_string()).into_response(),
        Err(e) => {
            tracing::error!("Database error: {}", e);
            return AppError::Internal("Service unavailable".to_string()).into_response();
        }
    };

    let user_opt = match sqlx::query_as::<_, User>(
        "SELECT id, phone, password_hash, status, created_at, updated_at, last_login_at FROM users WHERE id = ?",
    )
    .bind(id)
    .fetch_optional(&state.pool)
    .await
    {
        Ok(u) => u,
        Err(e) => {
            tracing::error!("Fetch user before delete failed: {}", e);
            return AppError::Internal("Service unavailable".to_string()).into_response();
        }
    };

    let user_phone = user_opt
        .as_ref()
        .map(|u| u.phone.as_str())
        .unwrap_or("")
        .to_string();

    let result = match sqlx::query("DELETE FROM users WHERE id = ?")
        .bind(id)
        .execute(&state.pool)
        .await
    {
        Ok(r) => r,
        Err(e) => {
            tracing::error!("Delete user failed: {}", e);
            return AppError::Internal("Failed to delete user".to_string()).into_response();
        }
    };

    if result.rows_affected() == 0 {
        return AppError::NotFound(format!("User {} not found", id)).into_response();
    }

    let _ = audit_svc::record(
        &state.pool,
        admin_id,
        &admin.phone,
        Some(id),
        &user_phone,
        Operation::Delete,
        Some(serde_json::json!({ "resource": "user" })),
    )
    .await;

    ApiResponse::success(())
}

pub async fn toggle_user_status(
    State(state): State<AppState>,
    axum::Extension(claims): axum::Extension<Claims>,
    Path(id): Path<i64>,
    Json(req): Json<ToggleUserStatusRequest>,
) -> ApiResponse<UserPublic> {
    let admin_id = match claims.admin_id {
        Some(id) => id,
        None => return AppError::AdminNotFound("Admin not found".to_string()).into_response(),
    };

    let admin = match admin_svc::get_admin_by_id(&state.pool, admin_id).await {
        Ok(Some(a)) => a,
        Ok(None) => return AppError::AdminNotFound("Admin not found".to_string()).into_response(),
        Err(e) => {
            tracing::error!("Database error: {}", e);
            return AppError::Internal("Service unavailable".to_string()).into_response();
        }
    };

    let user_opt = match sqlx::query_as::<_, User>(
        "SELECT id, phone, password_hash, status, created_at, updated_at, last_login_at FROM users WHERE id = ?",
    )
    .bind(id)
    .fetch_optional(&state.pool)
    .await
    {
        Ok(u) => u,
        Err(e) => {
            tracing::error!("Fetch user before toggle failed: {}", e);
            return AppError::Internal("Service unavailable".to_string()).into_response();
        }
    };

    let user_phone = user_opt
        .as_ref()
        .map(|u| u.phone.as_str())
        .unwrap_or("")
        .to_string();

    let op = if req.status == 1 {
        Operation::Enable
    } else {
        Operation::Disable
    };

    let result = match sqlx::query("UPDATE users SET status = ?, updated_at = NOW() WHERE id = ?")
        .bind(req.status)
        .bind(id)
        .execute(&state.pool)
        .await
    {
        Ok(r) => r,
        Err(e) => {
            tracing::error!("Toggle user status failed: {}", e);
            return AppError::Internal("Failed to update user status".to_string()).into_response();
        }
    };

    if result.rows_affected() == 0 {
        return AppError::NotFound(format!("User {} not found", id)).into_response();
    }

    let user = match sqlx::query_as::<_, User>(
        "SELECT id, phone, password_hash, status, created_at, updated_at, last_login_at FROM users WHERE id = ?",
    )
    .bind(id)
    .fetch_one(&state.pool)
    .await
    {
        Ok(u) => u,
        Err(e) => {
            tracing::error!("Fetch user after toggle failed: {}", e);
            return AppError::Internal("Service unavailable".to_string()).into_response();
        }
    };

    let _ = audit_svc::record(
        &state.pool,
        admin_id,
        &admin.phone,
        Some(id),
        &user_phone,
        op,
        Some(serde_json::json!({ "resource": "user", "status": req.status })),
    )
    .await;

    ApiResponse::success(user.into())
}

#[derive(Deserialize)]
pub struct ToggleUserStatusRequest {
    status: i8,
}

pub fn router() -> Router<AppState> {
    Router::new()
        .route("/users", get(list_users).post(create_user))
        .route("/users/:id", axum::routing::delete(delete_user))
        .route(
            "/users/:id/status",
            axum::routing::patch(toggle_user_status),
        )
}
