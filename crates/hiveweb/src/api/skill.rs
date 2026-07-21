//! Skill API handlers (T086 / US2)

use axum::{
    Json, Router,
    extract::{Extension, Path, Query, State},
    routing::{get, post},
};
use serde::Deserialize;

use crate::api::AppState;
use crate::runtime::skill_test::{self as test_svc, TestSkillRequest};
use crate::services::audit::{self as audit_svc, Operation};
use crate::services::skill::{self as svc, CreateMeta, ListFilter, UpdateMeta};
use crate::utils::error::ApiResponse;
use crate::utils::jwt::Claims;

pub fn router() -> Router<AppState> {
    Router::new()
        .route("/skills", get(list_skills).post(create_skill))
        .route(
            "/skills/:id",
            get(get_skill).put(update_skill).delete(delete_skill),
        )
        .route("/skills/:id/test", post(test_skill))
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
    pub source: Option<String>,
    #[serde(default)]
    pub category_id: Option<i64>,
    #[serde(default)]
    pub identifier: Option<String>,
    #[serde(default)]
    pub name: Option<String>,
    #[serde(default)]
    pub description: Option<String>,
    #[serde(default)]
    pub required_capabilities: Option<String>,
    #[serde(default)]
    pub created_at_start: Option<String>,
    #[serde(default)]
    pub created_at_end: Option<String>,
    #[serde(default)]
    pub updated_at_start: Option<String>,
    #[serde(default)]
    pub updated_at_end: Option<String>,
}

async fn list_skills(
    State(state): State<AppState>,
    Query(q): Query<ListQuery>,
) -> Result<ApiResponse<svc::SkillList>, ApiResponse<()>> {
    let filter = ListFilter {
        offset: q.offset.unwrap_or(0).max(0),
        limit: q.limit.unwrap_or(20).clamp(1, 100),
        search: q.search.filter(|s| !s.is_empty()),
        source: q.source.filter(|s| !s.is_empty()),
        category_id: q.category_id,
        identifier: q.identifier.filter(|s| !s.is_empty()),
        name: q.name.filter(|s| !s.is_empty()),
        description: q.description.filter(|s| !s.is_empty()),
        required_capabilities: q.required_capabilities.filter(|s| !s.is_empty()),
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

async fn get_skill(
    State(state): State<AppState>,
    Path(id): Path<i64>,
) -> Result<ApiResponse<crate::models::Skill>, ApiResponse<()>> {
    svc::fetch_by_id(&state.pool, id)
        .await
        .map(ApiResponse::success)
        .map_err(|e| e.into_response())
}

async fn create_skill(
    State(state): State<AppState>,
    Extension(claims): Extension<Claims>,
    Json(meta): Json<CreateMeta>,
) -> Result<ApiResponse<crate::models::Skill>, ApiResponse<()>> {
    match svc::create(&state.pool, meta).await {
        Ok(skill) => {
            if let Err(e) = audit_event(
                &state.pool,
                &claims,
                Operation::Create,
                Some(skill.id),
                &skill.name,
                serde_json::json!({ "identifier": skill.identifier, "source": skill.source }),
            )
            .await
            {
                tracing::error!("Failed to write audit log for skill create: {}", e);
            }
            Ok(ApiResponse::success(skill))
        }
        Err(e) => Err(e.into_response()),
    }
}

async fn update_skill(
    State(state): State<AppState>,
    Extension(claims): Extension<Claims>,
    Path(id): Path<i64>,
    Json(meta): Json<UpdateMeta>,
) -> Result<ApiResponse<crate::models::Skill>, ApiResponse<()>> {
    match svc::update(&state.pool, id, meta).await {
        Ok(skill) => {
            if let Err(e) = audit_event(
                &state.pool,
                &claims,
                Operation::Update,
                Some(skill.id),
                &skill.name,
                serde_json::json!({ "identifier": skill.identifier, "source": skill.source }),
            )
            .await
            {
                tracing::error!("Failed to write audit log for skill update: {}", e);
            }
            Ok(ApiResponse::success(skill))
        }
        Err(e) => Err(e.into_response()),
    }
}

async fn delete_skill(
    State(state): State<AppState>,
    Extension(claims): Extension<Claims>,
    Path(id): Path<i64>,
) -> Result<ApiResponse<()>, ApiResponse<()>> {
    let prev = svc::fetch_by_id(&state.pool, id).await.ok();
    let target_id = prev.as_ref().map(|s| s.id);
    let target_name = prev.as_ref().map(|s| &s.name).cloned().unwrap_or_default();

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
                tracing::error!("Failed to write audit log for skill delete: {}", e);
            }
            Ok(ApiResponse::success(()))
        }
        Err(e) => Err(e.into_response()),
    }
}

async fn test_skill(
    State(state): State<AppState>,
    Path(id): Path<i64>,
    Json(req): Json<TestSkillRequest>,
) -> Result<ApiResponse<test_svc::TestSkillResult>, ApiResponse<()>> {
    let deps = crate::runtime::orchestrator::OrchestratorDeps {
        pool: state.pool.clone(),
        redis: state.redis.clone(),
        s3: state.s3.clone(),
        llm: state.runtime_state.llm.clone(),
        registry: state.runtime_state.capabilities.clone(),
        invoker: state.runtime_state.invoker.clone(),
        ext_pool: state.ext_pool.clone(),
        message: String::new(),
        channel: String::new(),
        client_type: String::new(),
        client_version: String::new(),
        sensitive_filter: state.sensitive_filter.clone(),
        cancel: std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false)),
    };
    match test_svc::run_skill_test(&state.pool, &deps, id, req).await {
        Ok(result) => Ok(ApiResponse::success(result)),
        Err(e) => Err(ApiResponse::err(5000, e)),
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
