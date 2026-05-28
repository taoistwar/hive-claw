//! Agent API (T117 / US5)

use axum::{
    extract::{Extension, Path, State},
    routing::get,
    Json, Router,
};

use crate::api::AppState;
use crate::services::audit::{self as audit_svc, Operation};
use crate::services::agent::{self as svc, CreateMeta, UpdateMeta};
use crate::utils::error::ApiResponse;
use crate::utils::jwt::Claims;

pub fn router() -> Router<AppState> {
    Router::new()
        .route("/agents", get(list_agents).post(create_agent))
        .route("/agents/model-presets", get(list_presets))
        .route(
            "/agents/:id",
            get(get_agent).put(update_agent).delete(delete_agent),
        )
}

async fn list_agents(
    State(state): State<AppState>,
) -> Result<ApiResponse<Vec<svc::AgentTreeNode>>, ApiResponse<()>> {
    svc::list_tree(&state.pool)
        .await
        .map(ApiResponse::success)
        .map_err(|e| e.into_response())
}

async fn get_agent(
    State(state): State<AppState>,
    Path(id): Path<i64>,
) -> Result<ApiResponse<svc::AgentDetail>, ApiResponse<()>> {
    svc::fetch_detail(&state.pool, id)
        .await
        .map(ApiResponse::success)
        .map_err(|e| e.into_response())
}

async fn create_agent(
    State(state): State<AppState>,
    Extension(claims): Extension<Claims>,
    Json(meta): Json<CreateMeta>,
) -> Result<ApiResponse<svc::AgentDetail>, ApiResponse<()>> {
    match svc::create(
        &state.pool,
        &state.runtime_state.capabilities,
        &state.runtime_state.llm,
        claims.role,
        meta,
    )
    .await
    {
        Ok(detail) => {
            if let Err(e) = audit_event(
                &state.pool,
                &claims,
                Operation::Create,
                Some(detail.agent.id),
                &detail.agent.name,
                serde_json::json!({ "identifier": detail.agent.identifier }),
            )
            .await
            {
                tracing::error!("Failed to write audit log for agent create: {}", e);
            }
            Ok(ApiResponse::success(detail))
        }
        Err(e) => Err(e.into_response()),
    }
}

async fn update_agent(
    State(state): State<AppState>,
    Extension(claims): Extension<Claims>,
    Path(id): Path<i64>,
    Json(meta): Json<UpdateMeta>,
) -> Result<ApiResponse<svc::AgentDetail>, ApiResponse<()>> {
    match svc::update(
        &state.pool,
        &state.runtime_state.capabilities,
        &state.runtime_state.llm,
        claims.role,
        id,
        meta,
    )
    .await
    {
        Ok(detail) => {
            if let Err(e) = audit_event(
                &state.pool,
                &claims,
                Operation::Update,
                Some(detail.agent.id),
                &detail.agent.name,
                serde_json::json!({ "identifier": detail.agent.identifier }),
            )
            .await
            {
                tracing::error!("Failed to write audit log for agent update: {}", e);
            }
            Ok(ApiResponse::success(detail))
        }
        Err(e) => Err(e.into_response()),
    }
}

async fn delete_agent(
    State(state): State<AppState>,
    Extension(claims): Extension<Claims>,
    Path(id): Path<i64>,
) -> Result<ApiResponse<()>, ApiResponse<()>> {
    let prev = svc::fetch_detail(&state.pool, id).await.ok();
    let target_id = prev.as_ref().map(|d| d.agent.id);
    let target_name = prev.as_ref().map(|d| &d.agent.name).cloned().unwrap_or_default();

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
                tracing::error!("Failed to write audit log for agent delete: {}", e);
            }
            Ok(ApiResponse::success(()))
        }
        Err(e) => Err(e.into_response()),
    }
}

async fn list_presets(
    State(state): State<AppState>,
) -> ApiResponse<Vec<crate::runtime::llm::PresetEntry>> {
    ApiResponse::success(state.runtime_state.llm.snapshot())
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
    let operator = admin_svc::get_admin_by_id(pool, claims.admin_id).await?;
    let operator_phone = operator.map(|a| a.phone).unwrap_or_default();
    if let Err(e) = audit_svc::record(
        pool,
        claims.admin_id,
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
