//! GlobalConfig API handlers — 全局配置 CRUD
//!
//! 权限模型：
//! - Normal (role=1)  → 只能查看
//! - System (role=2)  → 查看 + 编辑值
//! - Super  (role=3)  → 全部（增 / 删 / 改 / 查）

use axum::{
    Json, Router,
    extract::{Path, Query, State},
    routing::get,
};
use serde::Deserialize;
use sqlx::MySqlPool;

use crate::api::AppState;
use crate::models::Role;
use crate::services::global_config::{self as svc, CreateMeta, UpdateMeta};
use crate::utils::error::{ApiResponse, AppError};
use crate::utils::jwt::Claims;

pub fn router() -> Router<AppState> {
    Router::new()
        .route(
            "/global-configs",
            get(list_global_configs).post(create_global_config),
        )
        .route(
            "/global-configs/:id",
            get(get_global_config)
                .put(update_global_config)
                .delete(delete_global_config),
        )
}

/// 解析 Claims → Role，失败时返回权限拒绝响应。
fn resolve_role(claims: &Claims) -> Result<Role, ApiResponse<()>> {
    Role::try_from(claims.role).map_err(|_| {
        AppError::InsufficientPermission("Invalid role in token".to_string()).into_response()
    })
}

#[derive(Debug, Deserialize)]
pub struct ListQuery {
    #[serde(default)]
    pub q: Option<String>,
    #[serde(default)]
    pub offset: Option<i64>,
    #[serde(default)]
    pub limit: Option<i64>,
}

async fn list_global_configs(
    State(state): State<AppState>,
    Query(q): Query<ListQuery>,
) -> Result<ApiResponse<serde_json::Value>, ApiResponse<()>> {
    let offset = q.offset.unwrap_or(0).max(0);
    let limit = q.limit.unwrap_or(20).clamp(1, 100);

    let (items, total) = svc::list(
        &state.pool,
        q.q.as_deref().filter(|s| !s.is_empty()),
        offset,
        limit,
    )
    .await
    .map_err(|e| e.into_response())?;

    let resp = serde_json::json!({
        "items": items,
        "total": total,
        "offset": offset,
        "limit": limit,
    });

    Ok(ApiResponse::success(resp))
}

async fn get_global_config(
    State(state): State<AppState>,
    Path(id): Path<i64>,
) -> Result<ApiResponse<crate::models::GlobalConfig>, ApiResponse<()>> {
    svc::fetch_by_id(&state.pool, id)
        .await
        .map(ApiResponse::success)
        .map_err(|e| e.into_response())
}

async fn create_global_config(
    State(state): State<AppState>,
    axum::Extension(claims): axum::Extension<Claims>,
    Json(meta): Json<CreateMeta>,
) -> Result<ApiResponse<crate::models::GlobalConfig>, ApiResponse<()>> {
    let role = resolve_role(&claims)?;

    // 仅 Super 管理员可创建
    if !matches!(role, Role::Super) {
        return Err(
            AppError::InsufficientPermission("仅超级管理员可创建全局配置".to_string())
                .into_response(),
        );
    }

    svc::create(&state.pool, meta)
        .await
        .map(ApiResponse::success)
        .map_err(|e| e.into_response())
}

async fn update_global_config(
    State(state): State<AppState>,
    Path(id): Path<i64>,
    axum::Extension(claims): axum::Extension<Claims>,
    Json(mut meta): Json<UpdateMeta>,
) -> Result<ApiResponse<crate::models::GlobalConfig>, ApiResponse<()>> {
    let role = resolve_role(&claims)?;

    // System 或 Super 可编辑
    if !matches!(role, Role::System | Role::Super) {
        return Err(AppError::InsufficientPermission(
            "仅系统管理员或超级管理员可修改全局配置".to_string(),
        )
        .into_response());
    }

    // System 管理员只能修改 data，不能修改 name/type
    if !matches!(role, Role::Super) {
        meta.name = None;
        meta.config_type = None;
    }

    svc::update(&state.pool, id, meta)
        .await
        .map(|cfg| cfg)
        .map(ApiResponse::success)
        .map_err(|e| e.into_response())
}

async fn delete_global_config(
    State(state): State<AppState>,
    Path(id): Path<i64>,
    axum::Extension(claims): axum::Extension<Claims>,
) -> Result<ApiResponse<()>, ApiResponse<()>> {
    let role = resolve_role(&claims)?;

    // 仅 Super 管理员可删除
    if !matches!(role, Role::Super) {
        return Err(
            AppError::InsufficientPermission("仅超级管理员可删除全局配置".to_string())
                .into_response(),
        );
    }

    match svc::delete(&state.pool, id).await {
        Ok(()) => Ok(ApiResponse::success(())),
        Err(e) => Err(e.into_response()),
    }
}
