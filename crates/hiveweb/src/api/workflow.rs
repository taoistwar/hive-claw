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
    extract::{Extension, Path, Query, State},
    routing::{get, post},
};
use chrono::{DateTime, Utc};
use serde::Deserialize;
use serde_json::Value;
use std::collections::HashMap;

use crate::api::AppState;
use crate::middleware::request_id::RequestId;
use crate::runtime::execution_context::RuntimeExecutionContext;
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
    /// 平台（对应 UserInput.metadata["client_type"]）
    pub client_type: Option<String>,
    /// 应用版本（对应 UserInput.metadata["client_version"]）
    pub client_version: Option<String>,
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
    if let Some(ref v) = ui.client_type {
        metadata.insert("client_type".into(), v.clone());
    }
    if let Some(ref v) = ui.client_version {
        metadata.insert("client_version".into(), v.clone());
    }
    UserInput {
        raw_text: ui.raw_text.clone().unwrap_or_default(),
        session_id: None,
        message_id: None,
        timestamp: chrono::Utc::now(),
        metadata,
    }
}

fn build_execute_response_data(
    workflow_id: i64,
    outcome: crate::runtime::workflow::ExecuteOutcome,
    elapsed_ms: i32,
    agent_context: Option<Value>,
) -> Value {
    let node_results: HashMap<String, Value> = outcome
        .node_results
        .into_iter()
        .map(|(node_key, mut result)| {
            if let Value::Object(ref mut fields) = result {
                fields.remove(crate::runtime::input_source::AGENT_CONTEXT_UPDATES_KEY);
            }
            (node_key, result)
        })
        .collect();

    serde_json::json!({
        "workflow_id": workflow_id,
        "outputs": outcome.end_value,
        "node_results": node_results,
        "node_inputs": outcome.node_inputs,
        "node_agent_contexts": outcome.node_agent_contexts,
        "elapsed_ms": elapsed_ms,
        "agent_context": agent_context,
    })
}

fn workflow_execute_error_response(
    error: crate::runtime::workflow::WorkflowError,
) -> ApiResponse<()> {
    match error {
        crate::runtime::workflow::WorkflowError::ModelPresetUnknown(name) => {
            AppError::ModelPresetUnknown(format!("模型 preset「{name}」不存在，请重新选择"))
                .into_response()
        }
        error => AppError::Internal(format!("workflow execute: {error}")).into_response(),
    }
}

async fn execute_workflow(
    State(state): State<AppState>,
    Extension(RequestId(request_id)): Extension<RequestId>,
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
                actor_id_present = ui.actor_id.is_some(),
                raw_text_bytes = ui.raw_text.as_deref().map_or(0, str::len),
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
        execution_context: RuntimeExecutionContext::best_effort(Some(request_id), None),
        pool: state.pool.clone(),
        s3: state.s3.clone(),
        registry: std::sync::Arc::clone(&state.runtime_state.capabilities),
        llm: std::sync::Arc::clone(&state.runtime_state.llm),
        invoker: std::sync::Arc::clone(&state.runtime_state.invoker),
        ext_pool: state.ext_pool.clone(),
        redis: Some(state.redis.clone()),
        permissions: state
            .runtime_state
            .capabilities
            .all()
            .iter()
            .map(|c| c.name.to_string())
            .collect(),
    };
    let outcome = state
        .runtime_state
        .workflows
        .execute(
            &deps,
            id,
            body.input,
            1, /* main agent */
            agent_ctx.clone(),
        )
        .await
        .map_err(workflow_execute_error_response)?;

    // ★ Apply AgentContext updates from the end node output
    apply_agent_context_updates(&agent_ctx, &outcome.end_value);
    // Store end node result as WorkflowResults
    if agent_ctx
        .set_record(
            Category::WorkflowResults,
            "end".to_string(),
            outcome.end_value.clone(),
            "workflow_node".to_string(),
            0,
        )
        .is_err()
    {
        tracing::warn!(
            error_kind = "agent_context_set_record_failed",
            "execute_workflow: 写回 WorkflowResults 失败"
        );
    }

    let agent_context_snapshot = match agent_ctx.snapshot() {
        Ok(snapshot) => serde_json::to_value(&snapshot).ok(),
        Err(_) => {
            tracing::warn!(
                error_kind = "agent_context_snapshot_failed",
                "execute_workflow: AgentContext snapshot 失败"
            );
            None
        }
    };

    let elapsed_ms = t0.elapsed().as_millis() as i32;
    Ok(ApiResponse::success(build_execute_response_data(
        id,
        outcome,
        elapsed_ms,
        agent_context_snapshot,
    )))
}

#[cfg(test)]
mod tests {
    use super::{build_execute_response_data, workflow_execute_error_response};
    use crate::runtime::workflow::{ExecuteOutcome, WorkflowError};
    use crate::utils::error::{codes, http_status_for_code};
    use axum::http::StatusCode;
    use serde_json::json;
    use std::collections::HashMap;

    #[test]
    fn execute_response_preserves_every_public_workflow_output_shape() {
        for output in [
            json!({"answer": "ok"}),
            json!("primitive result"),
            json!({"alpha": {"answer": "a"}, "zeta": {"answer": "z"}}),
        ] {
            let data = build_execute_response_data(
                7,
                ExecuteOutcome {
                    end_value: output.clone(),
                    node_results: HashMap::from([(
                        "final".to_string(),
                        json!({
                            "raw": true,
                            "_agent_context_updates": {
                                "metadata": {"private": "value"}
                            }
                        }),
                    )]),
                    node_inputs: HashMap::new(),
                    node_agent_contexts: HashMap::new(),
                },
                42,
                None,
            );

            assert_eq!(data["workflow_id"], 7);
            assert_eq!(data["outputs"], output);
            assert_eq!(data["node_results"]["final"], json!({"raw": true}));
            assert_eq!(
                data["node_results"]["final"].get("_agent_context_updates"),
                None
            );
            assert_eq!(data["elapsed_ms"], 42);
        }
    }

    #[test]
    fn unknown_model_preset_maps_to_the_typed_workflow_api_error() {
        let response = workflow_execute_error_response(WorkflowError::ModelPresetUnknown(
            "missing".to_string(),
        ));

        assert_eq!(response.code, codes::MODEL_PRESET_UNKNOWN);
        assert_eq!(
            http_status_for_code(response.code),
            StatusCode::UNPROCESSABLE_ENTITY
        );
        assert!(response.message.contains("missing"));
        assert!(!response.message.contains("workflow execute:"));
    }
}
