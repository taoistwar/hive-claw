use axum::{
    Json, Router,
    extract::{Extension, Path, Query, State},
    routing::{delete, get, post, put},
};
use serde::Serialize;

use crate::api::AppState;
use crate::models::agent_hook::{
    CreateHookRequest, HookExecutionQuery, UpdateHookRequest,
};
use crate::services::agent_hook as svc;
use crate::utils::error::ApiResponse;
use crate::utils::jwt::Claims;

pub fn router() -> Router<AppState> {
    Router::new()
        .route("/agents/:id/hooks", get(list_hooks).post(create_hook))
        .route(
            "/agents/:id/hooks/:hook_id",
            put(update_hook).delete(delete_hook),
        )
        .route(
            "/agents/:id/hooks/executions",
            get(list_executions),
        )
}

// ── Handlers ──

async fn create_hook(
    State(state): State<AppState>,
    Extension(claims): Extension<Claims>,
    Path(agent_id): Path<i64>,
    Json(meta): Json<CreateHookRequest>,
) -> Result<ApiResponse<impl Serialize>, ApiResponse<()>> {
    svc::create_hook(&state.pool, agent_id, claims.role, meta)
        .await
        .map(ApiResponse::success)
        .map_err(|e| e.into_response())
}

async fn list_hooks(
    State(state): State<AppState>,
    Path(agent_id): Path<i64>,
) -> Result<ApiResponse<impl Serialize>, ApiResponse<()>> {
    svc::list_hooks_enriched(&state.pool, agent_id)
        .await
        .map(ApiResponse::success)
        .map_err(|e| e.into_response())
}

async fn update_hook(
    State(state): State<AppState>,
    Extension(claims): Extension<Claims>,
    Path((agent_id, hook_id)): Path<(i64, i64)>,
    Json(meta): Json<UpdateHookRequest>,
) -> Result<ApiResponse<impl Serialize>, ApiResponse<()>> {
    svc::update_hook(&state.pool, agent_id, hook_id, claims.role, meta)
        .await
        .map(ApiResponse::success)
        .map_err(|e| e.into_response())
}

async fn delete_hook(
    State(state): State<AppState>,
    Extension(claims): Extension<Claims>,
    Path((agent_id, hook_id)): Path<(i64, i64)>,
) -> Result<ApiResponse<impl Serialize>, ApiResponse<()>> {
    svc::delete_hook(&state.pool, agent_id, hook_id, claims.role)
        .await
        .map(|_| ApiResponse::success(()))
        .map_err(|e| e.into_response())
}

async fn list_executions(
    State(state): State<AppState>,
    Extension(_claims): Extension<Claims>,
    Path(_agent_id): Path<i64>,
    Query(query): Query<HookExecutionQuery>,
) -> Result<ApiResponse<impl Serialize>, ApiResponse<()>> {
    // FR-019: Non-Super users limited to own sessions
    let page = query.page;
    let page_size = query.page_size;

    svc::list_executions(&state.pool, query)
        .await
        .map(|(items, total)| {
            #[derive(Serialize)]
            struct PaginatedExecutions {
                items: Vec<crate::models::agent_hook::HookExecution>,
                total: u64,
                page: u32,
                page_size: u32,
            }
            ApiResponse::success(PaginatedExecutions {
                items,
                total,
                page,
                page_size,
            })
        })
        .map_err(|e| e.into_response())
}
