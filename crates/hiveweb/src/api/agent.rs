//! Agent API (T117 / US5)

use axum::{
    extract::{Extension, Path, State},
    routing::get,
    Json, Router,
};

use crate::api::AppState;
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
    svc::create(
        &state.pool,
        &state.runtime_state.capabilities,
        &state.runtime_state.llm,
        claims.role,
        meta,
    )
    .await
    .map(ApiResponse::success)
    .map_err(|e| e.into_response())
}

async fn update_agent(
    State(state): State<AppState>,
    Extension(claims): Extension<Claims>,
    Path(id): Path<i64>,
    Json(meta): Json<UpdateMeta>,
) -> Result<ApiResponse<svc::AgentDetail>, ApiResponse<()>> {
    svc::update(
        &state.pool,
        &state.runtime_state.capabilities,
        &state.runtime_state.llm,
        claims.role,
        id,
        meta,
    )
    .await
    .map(ApiResponse::success)
    .map_err(|e| e.into_response())
}

async fn delete_agent(
    State(state): State<AppState>,
    Path(id): Path<i64>,
) -> Result<ApiResponse<()>, ApiResponse<()>> {
    match svc::delete(&state.pool, id).await {
        Ok(()) => Ok(ApiResponse::success(())),
        Err(e) => Err(e.into_response()),
    }
}

async fn list_presets(
    State(state): State<AppState>,
) -> ApiResponse<Vec<crate::runtime::llm::PresetEntry>> {
    ApiResponse::success(state.runtime_state.llm.snapshot())
}
