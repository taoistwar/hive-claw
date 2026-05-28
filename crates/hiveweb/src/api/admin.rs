use axum::{
    extract::{Path, Query, State},
    Json, Router,
};
use serde::Deserialize;

use crate::api::AppState;
use crate::models::{Admin, Role};
use crate::services::admin;
use crate::services::audit::{self, Operation};
use crate::utils::error::{ApiResponse, AppError};
use crate::utils::jwt::Claims;
use crate::utils::password::hash_password;
use crate::utils::validation::{validate_nickname, validate_phone};

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
    #[serde(default)]
    search: Option<String>,
    #[serde(default)]
    status: Option<i8>,
    #[serde(default)]
    role: Option<i8>,
    #[serde(default)]
    created_at_start: Option<String>,
    #[serde(default)]
    created_at_end: Option<String>,
    #[serde(default)]
    last_login_start: Option<String>,
    #[serde(default)]
    last_login_end: Option<String>,
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
        Err(_) => {
            return AppError::InsufficientPermission("Invalid role in token".to_string())
                .into_response()
        }
    };

    // spec §US3 AS-1：所有已认证角色都可以查看列表；操作类按钮在前端按 capability 隐藏。
    if !caller_role.can_view_admins() {
        return AppError::InsufficientPermission("Insufficient permissions".to_string())
            .into_response();
    }

    let filter = admin::AdminFilter {
        search: query.search.clone(),
        status: query.status,
        role: query.role,
        created_at_start: query.created_at_start.clone(),
        created_at_end: query.created_at_end.clone(),
        last_login_start: query.last_login_start.clone(),
        last_login_end: query.last_login_end.clone(),
    };

    let (admins, total) = match admin::list_admins(&state.pool, query.offset, query.limit, &filter).await {
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
        Ok(None) => return AppError::AdminNotFound("Admin not found".to_string()).into_response(),
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
        Err(_) => {
            return AppError::InsufficientPermission("Invalid role in token".to_string())
                .into_response()
        }
    };

    // spec §US3 AS-2/AS-3：仅 System 与 Super 可以创建。
    if !caller_role.can_modify_admins() {
        return AppError::InsufficientPermission("Insufficient permissions".to_string())
            .into_response();
    }

    // Boundary validation — spec.md §Assumptions + data-model.md §Admin.
    if let Err(e) = validate_phone(&req.phone) {
        return e.into_response();
    }
    if let Err(e) = validate_nickname(&req.nickname) {
        return e.into_response();
    }

    let target_role = match Role::try_from(req.role) {
        Ok(role) => role,
        Err(_) => return AppError::BadRequest("Invalid role value".to_string()).into_response(),
    };

    if !caller_role.can_modify_role(&target_role) {
        return AppError::InsufficientPermission("Cannot assign this role".to_string())
            .into_response();
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
                return AppError::PhoneAlreadyExists("Phone number already exists".to_string())
                    .into_response();
            }
            tracing::error!("Database error: {}", e);
            return AppError::Internal("Service unavailable".to_string()).into_response();
        }
    };

    if let Err(e) = audit_event(
        &state.pool,
        &claims,
        Operation::Create,
        Some(new_admin.id),
        &new_admin.phone,
        serde_json::json!({ "role": new_admin.role, "nickname": new_admin.nickname }),
    )
    .await
    {
        tracing::error!("Failed to write audit log for admin create: {}", e);
    }

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
        Err(_) => {
            return AppError::InsufficientPermission("Invalid role in token".to_string())
                .into_response()
        }
    };

    // spec §US3 AS-2/AS-3：仅 System 与 Super 可以修改。
    if !caller_role.can_modify_admins() {
        return AppError::InsufficientPermission("Insufficient permissions".to_string())
            .into_response();
    }

    if let Err(e) = validate_nickname(&req.nickname) {
        return e.into_response();
    }

    let target_role = match Role::try_from(req.role) {
        Ok(role) => role,
        Err(_) => return AppError::BadRequest("Invalid role value".to_string()).into_response(),
    };

    if !caller_role.can_modify_role(&target_role) {
        return AppError::InsufficientPermission("Cannot assign this role".to_string())
            .into_response();
    }

    let updated_admin = match admin::update_admin(&state.pool, id, &req.nickname, req.role).await {
        Ok(admin) => admin,
        Err(_) => return AppError::AdminNotFound("Admin not found".to_string()).into_response(),
    };

    if let Err(e) = audit_event(
        &state.pool,
        &claims,
        Operation::Update,
        Some(updated_admin.id),
        &updated_admin.phone,
        serde_json::json!({ "nickname": updated_admin.nickname, "role": updated_admin.role }),
    )
    .await
    {
        tracing::error!("Failed to write audit log for admin update: {}", e);
    }

    ApiResponse::success(updated_admin.into())
}

pub async fn delete_admin(
    Path(id): Path<i64>,
    State(state): State<AppState>,
    axum::Extension(claims): axum::Extension<Claims>,
) -> ApiResponse<()> {
    let caller_role = match Role::try_from(claims.role) {
        Ok(role) => role,
        Err(_) => {
            return AppError::InsufficientPermission("Invalid role in token".to_string())
                .into_response()
        }
    };

    // spec §US3 AS-3：仅 Super 可以删除（System 看不到该按钮，后端也必须拒绝）。
    if !caller_role.can_delete_admins() {
        return AppError::InsufficientPermission("Insufficient permissions".to_string())
            .into_response();
    }

    // Snapshot target identity *before* the delete so audit has phone to log.
    let target = admin::get_admin_by_id(&state.pool, id).await.ok().flatten();
    let target_phone = target.as_ref().map(|t| t.phone.clone()).unwrap_or_default();

    match admin::delete_admin(&state.pool, id).await {
        Ok(_) => {
            if let Err(e) = audit_event(
                &state.pool,
                &claims,
                Operation::Delete,
                Some(id),
                &target_phone,
                serde_json::json!({}),
            )
            .await
            {
                tracing::error!("Failed to write audit log for admin delete: {}", e);
            }
            ApiResponse::success(())
        }
        Err(e) => {
            let msg = e.to_string();
            if msg.contains("not found") {
                AppError::AdminNotFound(msg).into_response()
            } else if msg.contains("super admin") {
                AppError::CannotDeleteSuperAdmin(msg).into_response()
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
        Err(_) => {
            return AppError::InsufficientPermission("Invalid role in token".to_string())
                .into_response()
        }
    };

    // spec §US3 AS-2/AS-3：System 与 Super 都可以禁用/启用。
    if !caller_role.can_modify_admins() {
        return AppError::InsufficientPermission("Insufficient permissions".to_string())
            .into_response();
    }

    if req.status != 0 && req.status != 1 {
        return AppError::BadRequest("Status must be 0 or 1".to_string()).into_response();
    }

    match admin::toggle_admin_status(&state.pool, id, req.status).await {
        Ok(admin) => {
            let op = if req.status == 1 { Operation::Enable } else { Operation::Disable };
            if let Err(e) = audit_event(
                &state.pool,
                &claims,
                op,
                Some(admin.id),
                &admin.phone,
                serde_json::json!({ "new_status": req.status }),
            )
            .await
            {
                tracing::error!("Failed to write audit log for admin status toggle: {}", e);
            }
            ApiResponse::success(admin.into())
        }
        Err(e) => {
            let msg = e.to_string();
            if msg.contains("not found") {
                AppError::AdminNotFound(msg).into_response()
            } else if msg.contains("last active super admin") {
                AppError::CannotDisableLastSuperAdmin(msg).into_response()
            } else {
                tracing::error!("Database error: {}", e);
                AppError::Internal("Service unavailable".to_string()).into_response()
            }
        }
    }
}

/// Write one audit row. Operator's phone is resolved via the claims;
/// failures are logged but do not abort the response (audit-best-effort
/// for now — promoting to mandatory requires the change of contract that
/// FR-022 hints at).
async fn audit_event(
    pool: &sqlx::MySqlPool,
    claims: &Claims,
    op: Operation,
    target_id: Option<i64>,
    target_phone: &str,
    detail: serde_json::Value,
) -> anyhow::Result<()> {
    let operator = admin::get_admin_by_id(pool, claims.admin_id).await?;
    let operator_phone = operator.map(|a| a.phone).unwrap_or_default();
    if let Err(e) = audit::record(
        pool,
        claims.admin_id,
        &operator_phone,
        target_id,
        target_phone,
        op,
        Some(detail),
    )
    .await
    {
        tracing::error!("Failed to write audit log: {}", e);
        return Err(e);
    }
    Ok(())
}

pub fn router() -> Router<AppState> {
    Router::new()
        .route("/admins", axum::routing::get(list_admins).post(create_admin))
        .route("/admins/:id", axum::routing::get(get_admin).put(update_admin).delete(delete_admin))
        .route("/admins/:id/status", axum::routing::patch(toggle_admin_status))
}
