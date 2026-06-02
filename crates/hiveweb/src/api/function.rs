//! Function API handlers (T084 / US2)

use axum::{
    Json, Router,
    extract::{Extension, Path, Query, State},
    routing::get,
};
use serde::Deserialize;

use crate::api::AppState;
use crate::services::audit::{self as audit_svc, Operation};
use crate::services::function::{self as svc, CreateMeta, ListFilter, UpdateMeta};
use crate::utils::error::ApiResponse;
use crate::utils::jwt::Claims;

pub fn router() -> Router<AppState> {
    Router::new()
        .route("/functions", get(list_fns).post(create_fn))
        .route(
            "/functions/:id",
            get(get_fn).put(update_fn).delete(delete_fn),
        )
}

#[derive(Debug, Deserialize)]
pub struct ListQuery {
    #[serde(default)]
    pub offset: Option<i64>,
    #[serde(default)]
    pub limit: Option<i64>,
    #[serde(default)]
    pub search: Option<String>,
    #[serde(default)]
    pub category_id: Option<i64>,
    /// "builtin" | "custom"
    #[serde(default)]
    pub kind: Option<String>,
    #[serde(default)]
    pub identifier: Option<String>,
    #[serde(default)]
    pub name: Option<String>,
    #[serde(default)]
    pub plugin_id: Option<i64>,
    #[serde(default)]
    pub plugin_identifier: Option<String>,
    #[serde(default)]
    pub required_capabilities: Option<String>,
    #[serde(default)]
    pub tag_id: Option<i64>,
    #[serde(default)]
    pub created_at_start: Option<String>,
    #[serde(default)]
    pub created_at_end: Option<String>,
    #[serde(default)]
    pub updated_at_start: Option<String>,
    #[serde(default)]
    pub updated_at_end: Option<String>,
}

async fn list_fns(
    State(state): State<AppState>,
    Query(q): Query<ListQuery>,
) -> Result<ApiResponse<svc::FunctionList>, ApiResponse<()>> {
    let kind = q.kind.as_deref().and_then(|s| match s {
        "builtin" => Some(1),
        "custom" => Some(2),
        _ => None,
    });
    let filter = ListFilter {
        offset: q.offset.unwrap_or(0).max(0),
        limit: q.limit.unwrap_or(20).clamp(1, 100),
        search: q.search.filter(|s| !s.is_empty()),
        category_id: q.category_id,
        kind,
        identifier: q.identifier.filter(|s| !s.is_empty()),
        name: q.name.filter(|s| !s.is_empty()),
        plugin_id: q.plugin_id,
        plugin_identifier: q.plugin_identifier.filter(|s| !s.is_empty()),
        required_capabilities: q.required_capabilities.filter(|s| !s.is_empty()),
        tag_id: q.tag_id,
        created_at_start: q.created_at_start.filter(|s| !s.is_empty()),
        created_at_end: q.created_at_end.filter(|s| !s.is_empty()),
        updated_at_start: q.updated_at_start.filter(|s| !s.is_empty()),
        updated_at_end: q.updated_at_end.filter(|s| !s.is_empty()),
    };
    svc::list(&state.pool, filter)
        .await
        .map(ApiResponse::success)
        .map_err(|e| e.into_response())
}

async fn get_fn(
    State(state): State<AppState>,
    Path(id): Path<i64>,
) -> Result<ApiResponse<crate::models::Function>, ApiResponse<()>> {
    svc::fetch_by_id(&state.pool, id)
        .await
        .map(ApiResponse::success)
        .map_err(|e| e.into_response())
}

async fn create_fn(
    State(state): State<AppState>,
    Extension(claims): Extension<Claims>,
    Json(meta): Json<CreateMeta>,
) -> Result<ApiResponse<crate::models::Function>, ApiResponse<()>> {
    match svc::create_custom(&state.pool, meta).await {
        Ok(fn_item) => {
            if let Err(e) = audit_event(
                &state.pool,
                &claims,
                Operation::Create,
                Some(fn_item.id),
                &fn_item.name,
                serde_json::json!({ "kind": fn_item.kind }),
            )
            .await
            {
                tracing::error!("Failed to write audit log for function create: {}", e);
            }
            Ok(ApiResponse::success(fn_item))
        }
        Err(e) => Err(e.into_response()),
    }
}

async fn update_fn(
    State(state): State<AppState>,
    Extension(claims): Extension<Claims>,
    Path(id): Path<i64>,
    Json(meta): Json<UpdateMeta>,
) -> Result<ApiResponse<crate::models::Function>, ApiResponse<()>> {
    match svc::update(&state.pool, id, meta).await {
        Ok(fn_item) => {
            if let Err(e) = audit_event(
                &state.pool,
                &claims,
                Operation::Update,
                Some(fn_item.id),
                &fn_item.name,
                serde_json::json!({ "kind": fn_item.kind }),
            )
            .await
            {
                tracing::error!("Failed to write audit log for function update: {}", e);
            }
            Ok(ApiResponse::success(fn_item))
        }
        Err(e) => Err(e.into_response()),
    }
}

async fn delete_fn(
    State(state): State<AppState>,
    Extension(claims): Extension<Claims>,
    Path(id): Path<i64>,
) -> Result<ApiResponse<()>, ApiResponse<()>> {
    let prev = svc::fetch_by_id(&state.pool, id).await.ok();
    let target_id = prev.as_ref().map(|f| f.id);
    let target_name = prev.as_ref().map(|f| &f.name).cloned().unwrap_or_default();

    match svc::delete(&state.pool, id).await {
        Ok(()) => {
            if let Err(e) = audit_event(
                &state.pool,
                &claims,
                Operation::Delete,
                target_id,
                &target_name,
                serde_json::json!({}),
            )
            .await
            {
                tracing::error!("Failed to write audit log for function delete: {}", e);
            }
            Ok(ApiResponse::success(()))
        }
        Err(e) => Err(e.into_response()),
    }
}

/// 写入审计日志。失败时记录 tracing 日志但不影响主流程。
async fn audit_event(
    pool: &sqlx::MySqlPool,
    claims: &Claims,
    op: Operation,
    target_id: Option<i64>,
    target_name: &str,
    detail: serde_json::Value,
) -> anyhow::Result<()> {
    use crate::services::admin as admin_svc;
    let admin_id = claims
        .admin_id
        .ok_or_else(|| anyhow::anyhow!("No admin ID in claims"))?;
    let operator = admin_svc::get_admin_by_id(pool, admin_id).await?;
    let operator_phone = operator.map(|a| a.phone).unwrap_or_default();
    if let Err(e) = audit_svc::record(
        pool,
        admin_id,
        &operator_phone,
        target_id,
        target_name,
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
