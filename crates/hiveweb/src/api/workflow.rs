//! Workflow API (T109 / US3)
//!
//! 端点：
//!   GET    /api/workflows                — list metadata
//!   POST   /api/workflows                — create metadata
//!   GET    /api/workflows/:id            — single metadata
//!   PUT    /api/workflows/:id            — update metadata
//!   DELETE /api/workflows/:id
//!   GET    /api/workflows/:id/graph      — full DAG (nodes + edges)
//!   PUT    /api/workflows/:id/graph      — replace nodes + edges (cycle / mapping check)
//!   POST   /api/workflows/:id/execute    — run（占位；T110 接通）

use axum::{
    Json, Router,
    extract::{Path, Query, State},
    routing::{get, post},
};
use chrono::{DateTime, Utc};
use serde::Deserialize;
use serde_json::Value;
use std::collections::HashMap;

use crate::api::AppState;
use crate::services::workflow::{self as svc, CreateMeta, GraphPut, UpdateMeta};
use crate::utils::error::{ApiResponse, AppError};

pub fn router() -> Router<AppState> {
    Router::new()
        .route("/workflows", get(list_workflows).post(create_workflow))
        .route(
            "/workflows/:id",
            get(get_workflow)
                .put(update_workflow)
                .delete(delete_workflow),
        )
        .route(
            "/workflows/:id/graph",
            get(get_graph).put(put_graph_handler),
        )
        .route("/workflows/:id/execute", post(execute_workflow))
}

#[derive(Debug, Deserialize)]
pub struct ListQuery {
    #[serde(default)]
    pub offset: Option<i64>,
    #[serde(default)]
    pub limit: Option<i64>,
    #[serde(default)]
    pub id: Option<i64>,
    #[serde(default)]
    pub identifier: Option<String>,
    #[serde(default)]
    pub name: Option<String>,
    #[serde(default)]
    pub search: Option<String>,
    #[serde(default)]
    pub category_id: Option<i64>,
    #[serde(default)]
    pub tag_id: Option<i64>,
    #[serde(default)]
    pub required_capabilities: Option<String>,
    #[serde(default)]
    pub timeout_ms_from: Option<i64>,
    #[serde(default)]
    pub timeout_ms_to: Option<i64>,
    #[serde(default)]
    pub created_at_from: Option<DateTime<Utc>>,
    #[serde(default)]
    pub created_at_to: Option<DateTime<Utc>>,
    #[serde(default)]
    pub updated_at_from: Option<DateTime<Utc>>,
    #[serde(default)]
    pub updated_at_to: Option<DateTime<Utc>>,
}

async fn list_workflows(
    State(state): State<AppState>,
    Query(q): Query<ListQuery>,
) -> Result<ApiResponse<svc::WorkflowList>, ApiResponse<()>> {
    let offset = q.offset.unwrap_or(0).max(0);
    let limit = q.limit.unwrap_or(20).clamp(1, 100);
    svc::list(
        &state.pool,
        offset,
        limit,
        svc::ListFilter {
            id: q.id,
            identifier: q.identifier,
            name: q.name,
            search: q.search,
            category_id: q.category_id,
            tag_id: q.tag_id,
            required_capabilities: q.required_capabilities,
            timeout_ms_from: q.timeout_ms_from,
            timeout_ms_to: q.timeout_ms_to,
            created_at_from: q.created_at_from,
            created_at_to: q.created_at_to,
            updated_at_from: q.updated_at_from,
            updated_at_to: q.updated_at_to,
        },
    )
    .await
    .map(ApiResponse::success)
    .map_err(|e| e.into_response())
}

async fn get_workflow(
    State(state): State<AppState>,
    Path(id): Path<i64>,
) -> Result<ApiResponse<crate::models::Workflow>, ApiResponse<()>> {
    svc::fetch_by_id(&state.pool, id)
        .await
        .map(ApiResponse::success)
        .map_err(|e| e.into_response())
}

async fn create_workflow(
    State(state): State<AppState>,
    Json(meta): Json<CreateMeta>,
) -> Result<ApiResponse<crate::models::Workflow>, ApiResponse<()>> {
    svc::create(&state.pool, meta)
        .await
        .map(ApiResponse::success)
        .map_err(|e| e.into_response())
}

async fn update_workflow(
    State(state): State<AppState>,
    Path(id): Path<i64>,
    Json(meta): Json<UpdateMeta>,
) -> Result<ApiResponse<crate::models::Workflow>, ApiResponse<()>> {
    svc::update(&state.pool, id, meta)
        .await
        .map(ApiResponse::success)
        .map_err(|e| e.into_response())
}

async fn delete_workflow(
    State(state): State<AppState>,
    Path(id): Path<i64>,
) -> Result<ApiResponse<()>, ApiResponse<()>> {
    match svc::delete(&state.pool, id).await {
        Ok(()) => Ok(ApiResponse::success(())),
        Err(e) => Err(e.into_response()),
    }
}

async fn get_graph(
    State(state): State<AppState>,
    Path(id): Path<i64>,
) -> Result<ApiResponse<svc::WorkflowGraph>, ApiResponse<()>> {
    svc::fetch_graph(&state.pool, id)
        .await
        .map(ApiResponse::success)
        .map_err(|e| e.into_response())
}

async fn put_graph_handler(
    State(state): State<AppState>,
    Path(id): Path<i64>,
    Json(graph): Json<GraphPut>,
) -> Result<ApiResponse<svc::WorkflowGraph>, ApiResponse<()>> {
    svc::put_graph(&state.pool, id, graph)
        .await
        .map(ApiResponse::success)
        .map_err(|e| e.into_response())
}

#[derive(Debug, Deserialize)]
pub struct ExecuteBody {
    #[serde(default = "default_input")]
    pub input: Value,
    /// 可选的用户上下文，用于依赖 AgentContext 的内置函数（如 query_balance）
    #[serde(default)]
    pub user_input: Option<ExecuteUserInput>,
}
fn default_input() -> Value {
    Value::Object(serde_json::Map::new())
}

/// 测试/调试时由前端配置的 UserInput 字段
#[derive(Debug, Deserialize)]
pub struct ExecuteUserInput {
    #[serde(default)]
    pub raw_text: Option<String>,
    /// 用户/玩家 ID（对应 UserInput.metadata["actor_id"]）
    pub actor_id: Option<String>,
    /// 来源渠道（对应 UserInput.metadata["channel"]）
    pub channel: Option<String>,
    /// 平台（对应 UserInput.metadata["platform"]）
    pub platform: Option<String>,
    /// 应用版本（对应 UserInput.metadata["app_version"]）
    pub app_version: Option<String>,
}

/// 将 ExecuteUserInput 转换为 AgentContext 所需的 UserInput + metadata
fn build_user_input_metadata(ui: &ExecuteUserInput) -> agent::context::UserInput {
    use agent::context::UserInput;
    let mut metadata: HashMap<String, String> = HashMap::new();
    if let Some(ref v) = ui.actor_id {
        metadata.insert("actor_id".into(), v.clone());
    }
    if let Some(ref v) = ui.channel {
        metadata.insert("channel".into(), v.clone());
    }
    if let Some(ref v) = ui.platform {
        metadata.insert("platform".into(), v.clone());
    }
    if let Some(ref v) = ui.app_version {
        metadata.insert("app_version".into(), v.clone());
    }
    UserInput {
        raw_text: ui.raw_text.clone().unwrap_or_default(),
        session_id: None,
        message_id: None,
        timestamp: chrono::Utc::now(),
        metadata,
    }
}

async fn execute_workflow(
    State(state): State<AppState>,
    Path(id): Path<i64>,
    Json(body): Json<ExecuteBody>,
) -> Result<ApiResponse<Value>, ApiResponse<()>> {
    use crate::runtime::hook::apply_agent_context_updates;
    use crate::runtime::workflow::ExecutorDeps;
    use agent::context::{AgentContext, Category, ContextConfig};
    use std::sync::Arc;
    let t0 = std::time::Instant::now();

    // 构建 AgentContext（始终创建，注入到每个函数节点的输入中）
    let agent_ctx: Arc<AgentContext> = match body.user_input {
        Some(ref ui) => {
            tracing::info!(
                actor_id = ?ui.actor_id,
                raw_text = ?ui.raw_text,
                "execute_workflow: 使用前端提供的 UserInput 构建 AgentContext"
            );
            Arc::new(AgentContext::new(
                format!("test-ctx-{}", uuid::Uuid::new_v4()),
                build_user_input_metadata(ui),
                ContextConfig::default(),
            ))
        }
        None => {
            tracing::info!("execute_workflow: 未提供 user_input，创建空 AgentContext");
            Arc::new(AgentContext::new(
                format!("test-ctx-{}", uuid::Uuid::new_v4()),
                agent::context::UserInput {
                    raw_text: String::new(),
                    session_id: None,
                    message_id: None,
                    timestamp: chrono::Utc::now(),
                    metadata: HashMap::new(),
                },
                ContextConfig::default(),
            ))
        }
    };

    let deps = ExecutorDeps {
        pool: state.pool.clone(),
        s3: state.s3.clone(),
        registry: std::sync::Arc::clone(&state.runtime_state.capabilities),
        llm: std::sync::Arc::clone(&state.runtime_state.llm),
        invoker: std::sync::Arc::clone(&state.runtime_state.invoker),
        ext_pool: state.ext_pool.clone(),
    };
    let outputs = state
        .runtime_state
        .workflows
        .execute(&deps, id, body.input, 1 /* main agent */, agent_ctx.clone())
        .await
        .map_err(|e| AppError::Internal(format!("workflow execute: {e}")).into_response())?;

    // ★ Apply AgentContext updates from the end node output
    apply_agent_context_updates(&agent_ctx, &outputs);
    // Store end node result as WorkflowResults
    if let Err(e) = agent_ctx.set_record(
        Category::WorkflowResults,
        "end".to_string(),
        outputs.clone(),
        "workflow_node".to_string(),
        0,
    ) {
        tracing::warn!(
            error = %e,
            "execute_workflow: 写回 WorkflowResults 失败"
        );
    }

    let elapsed_ms = t0.elapsed().as_millis() as i32;
    Ok(ApiResponse::success(serde_json::json!({
        "workflow_id": id,
        "node_results": outputs,
        "elapsed_ms": elapsed_ms,
    })))
}
