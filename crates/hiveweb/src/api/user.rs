//! User management API handlers

use axum::{
    Json, Router,
    extract::{Path, Query, State},
    routing::get,
};
use chrono::{DateTime, Utc};
use serde::Deserialize;

use crate::api::AppState;
use crate::models::User;
use crate::utils::error::{ApiResponse, AppError};
use crate::utils::jwt::Claims;

#[derive(Deserialize)]
pub struct ListUsersQuery {
    #[serde(default = "default_page")]
    pub page: u32,
    #[serde(default = "default_page_size")]
    pub page_size: u32,
    #[serde(default)]
    pub id: Option<i64>,
    #[serde(default)]
    pub search: Option<String>,
    #[serde(default)]
    pub created_at_from: Option<DateTime<Utc>>,
    #[serde(default)]
    pub created_at_to: Option<DateTime<Utc>>,
    #[serde(default)]
    pub updated_at_from: Option<DateTime<Utc>>,
    #[serde(default)]
    pub updated_at_to: Option<DateTime<Utc>>,
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
    pub id: i64,
    pub uid: String,
    pub nickname: String,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
}
impl From<User> for UserPublic {
    fn from(u: User) -> Self {
        Self {
            id: u.id,
            uid: u.uid.unwrap_or_default(),
            nickname: u.nickname.unwrap_or_default(),
            created_at: u.created_at,
            updated_at: u.updated_at,
        }
    }
}

pub fn router() -> Router<AppState> {
    Router::new()
        .route("/users", get(list_users).post(create_user))
        .route("/users/:id", get(get_user).delete(delete_user))
}

async fn list_users(
    State(state): State<AppState>,
    Query(q): Query<ListUsersQuery>,
) -> ApiResponse<UserListResponse> {
    let search = q.search.clone().unwrap_or_default();
    let has_search = !search.is_empty();
    // search_pattern 必须活到所有 bind() 完成之后，所以提到外层 scope
    let search_pattern: Option<String> = if has_search {
        Some(format!("%{}%", search))
    } else {
        None
    };

    // 动态拼 WHERE 条件：id 走精确 =，search 仅模糊匹配 uid/nickname 两个文本字段，
    // 日期范围分别落在 created_at 和 updated_at 上。占位符顺序与下方 bind() 一一对应。
    let mut where_clauses: Vec<&str> = Vec::new();
    if q.id.is_some() {
        where_clauses.push("id = ?");
    }
    if search_pattern.is_some() {
        where_clauses.push("(uid LIKE ? OR nickname LIKE ?)");
    }
    if q.created_at_from.is_some() {
        where_clauses.push("created_at >= ?");
    }
    if q.created_at_to.is_some() {
        where_clauses.push("created_at <= ?");
    }
    if q.updated_at_from.is_some() {
        where_clauses.push("updated_at >= ?");
    }
    if q.updated_at_to.is_some() {
        where_clauses.push("updated_at <= ?");
    }

    let where_sql = if where_clauses.is_empty() {
        String::new()
    } else {
        format!(" WHERE {}", where_clauses.join(" AND "))
    };

    let count_sql = format!("SELECT COUNT(*) FROM users{where_sql}");
    let list_sql = format!(
        "SELECT id, uid, nickname, created_at, updated_at, last_login_at FROM users{where_sql} ORDER BY id DESC LIMIT ? OFFSET ?"
    );

    let total: (i64,) = {
        let mut qc = sqlx::query_as(&count_sql);
        if let Some(id) = q.id {
            qc = qc.bind(id);
        }
        if let Some(ref pattern) = search_pattern {
            qc = qc.bind(pattern.as_str()).bind(pattern.as_str());
        }
        if let Some(t) = q.created_at_from {
            qc = qc.bind(t);
        }
        if let Some(t) = q.created_at_to {
            qc = qc.bind(t);
        }
        if let Some(t) = q.updated_at_from {
            qc = qc.bind(t);
        }
        if let Some(t) = q.updated_at_to {
            qc = qc.bind(t);
        }
        match qc.fetch_one(&state.pool).await {
            Ok(t) => t,
            Err(_) => {
                tracing::error!(error_kind = "user_count_query_failed", "count users failed");
                return AppError::Internal("Service unavailable".into()).into_response();
            }
        }
    };

    let users: Vec<User> = {
        let mut qr = sqlx::query_as(&list_sql);
        if let Some(id) = q.id {
            qr = qr.bind(id);
        }
        if let Some(ref pattern) = search_pattern {
            qr = qr.bind(pattern.as_str()).bind(pattern.as_str());
        }
        if let Some(t) = q.created_at_from {
            qr = qr.bind(t);
        }
        if let Some(t) = q.created_at_to {
            qr = qr.bind(t);
        }
        if let Some(t) = q.updated_at_from {
            qr = qr.bind(t);
        }
        if let Some(t) = q.updated_at_to {
            qr = qr.bind(t);
        }
        qr = qr
            .bind(q.page_size as i64)
            .bind(((q.page - 1) * q.page_size) as i64);
        match qr.fetch_all(&state.pool).await {
            Ok(u) => u,
            Err(_) => {
                tracing::error!(error_kind = "user_list_query_failed", "list users failed");
                return AppError::Internal("Service unavailable".into()).into_response();
            }
        }
    };

    ApiResponse::success(UserListResponse {
        users: users.into_iter().map(|u| u.into()).collect(),
        total: total.0 as u64,
        page: q.page,
        page_size: q.page_size,
    })
}

async fn get_user(State(state): State<AppState>, Path(id): Path<i64>) -> ApiResponse<UserPublic> {
    match sqlx::query_as::<_, User>(
        "SELECT id, uid, nickname, created_at, updated_at, last_login_at FROM users WHERE id = ?",
    )
    .bind(id)
    .fetch_optional(&state.pool)
    .await
    {
        Ok(Some(u)) => ApiResponse::success(u.into()),
        Ok(None) => AppError::NotFound("User not found".into()).into_response(),
        Err(_) => AppError::Internal("Service unavailable".into()).into_response(),
    }
}

#[derive(Deserialize)]
pub struct CreateUserRequest {
    pub uid: Option<String>,
    pub nickname: Option<String>,
}

async fn create_user(
    State(state): State<AppState>,
    _claims: axum::Extension<Claims>,
    Json(req): Json<CreateUserRequest>,
) -> ApiResponse<UserPublic> {
    let uid = req.uid.as_deref().unwrap_or("");
    let nickname = req.nickname.as_deref().unwrap_or("");
    let result = match sqlx::query("INSERT INTO users (uid, nickname) VALUES (?, ?)")
        .bind(uid)
        .bind(nickname)
        .execute(&state.pool)
        .await
    {
        Ok(r) => r,
        Err(_) => {
            tracing::error!(error_kind = "user_create_failed", "create user failed");
            return AppError::Internal("Failed to create user".into()).into_response();
        }
    };
    let user_id = result.last_insert_id();
    match sqlx::query_as::<_, User>(
        "SELECT id, uid, nickname, created_at, updated_at, last_login_at FROM users WHERE id = ?",
    )
    .bind(user_id)
    .fetch_one(&state.pool)
    .await
    {
        Ok(u) => ApiResponse::success(u.into()),
        Err(_) => {
            tracing::error!(
                error_kind = "created_user_query_failed",
                "fetch created user failed"
            );
            AppError::Internal("Failed to fetch created user".into()).into_response()
        }
    }
}

async fn delete_user(
    State(state): State<AppState>,
    _claims: axum::Extension<Claims>,
    Path(id): Path<i64>,
) -> ApiResponse<()> {
    let exists = match sqlx::query_as::<_, User>(
        "SELECT id, uid, nickname, created_at, updated_at, last_login_at FROM users WHERE id = ?",
    )
    .bind(id)
    .fetch_optional(&state.pool)
    .await
    {
        Ok(Some(_)) => true,
        Ok(None) => false,
        Err(_) => return AppError::Internal("Service unavailable".into()).into_response(),
    };
    if !exists {
        return AppError::NotFound("User not found".into()).into_response();
    }
    let _ = sqlx::query("DELETE FROM users WHERE id = ?")
        .bind(id)
        .execute(&state.pool)
        .await;
    ApiResponse::success(())
}
