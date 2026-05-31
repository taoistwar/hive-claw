//! Chat API + SSE (T127 / US6)
//!
//! `POST /api/chat/sessions/:id/messages` 返回 SSE 流。
//! 事件：token / tool_call / tool_result / routed / fallback_used / done | error
//!
//! 当前 MVP：AgentOrchestrator 真实路由 + LLM 流式接通待 US5 commit 2 完成；
//! 本 commit 提供完整 wire format + 并发限制 + headers + keep-alive；
//! 内容部分用一个 mock generator 拆分 user 输入为 token 流，便于前端联调。

use axum::{
    extract::{Path, Query, State},
    http::{header, HeaderMap, HeaderValue, StatusCode},
    response::{
        sse::{Event, KeepAlive, Sse},
        IntoResponse, Response,
    },
    routing::get,
    Extension, Json, Router,
};
use futures::stream::{self, Stream};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::convert::Infallible;
use std::sync::Arc;
use std::time::Duration;
use tokio::sync::Mutex;

use crate::api::AppState;
use crate::services::chat::{self as svc, PostMessage};
use crate::utils::error::{ApiResponse, AppError};
use crate::utils::jwt::Claims;

/// 单 admin 并发 SSE 流上限（FR-027 v7 / CHK232 / env CHAT_SSE_MAX_CONCURRENT_PER_ADMIN）
fn max_concurrent_per_admin() -> usize {
    std::env::var("CHAT_SSE_MAX_CONCURRENT_PER_ADMIN")
        .ok()
        .and_then(|v| v.parse().ok())
        .unwrap_or(2)
}

/// 进程内 per-admin SSE 计数器
static SSE_COUNTER: once_cell::sync::Lazy<Arc<Mutex<HashMap<i64, usize>>>> =
    once_cell::sync::Lazy::new(|| Arc::new(Mutex::new(HashMap::new())));

/// RAII 计数器 guard：drop 时自动 -1
struct SseConcurrencyGuard {
    admin_id: i64,
}

impl Drop for SseConcurrencyGuard {
    fn drop(&mut self) {
        let admin_id = self.admin_id;
        let counter = SSE_COUNTER.clone();
        tokio::spawn(async move {
            let mut map = counter.lock().await;
            if let Some(c) = map.get_mut(&admin_id) {
                *c = c.saturating_sub(1);
                if *c == 0 {
                    map.remove(&admin_id);
                }
            }
        });
    }
}

async fn try_acquire_slot(admin_id: i64) -> bool {
    let mut map = SSE_COUNTER.lock().await;
    let cap = max_concurrent_per_admin();
    let entry = map.entry(admin_id).or_insert(0);
    if *entry >= cap {
        return false;
    }
    *entry += 1;
    true
}

pub fn router() -> Router<AppState> {
    Router::new()
        .route(
            "/chat/sessions",
            get(list_sessions).post(create_session),
        )
        .route(
            "/chat/sessions/:id",
            axum::routing::delete(delete_session),
        )
        .route(
            "/chat/sessions/:id/messages",
            get(get_messages).post(post_message_sse),
        )
}

async fn create_session(
    State(state): State<AppState>,
    Extension(claims): Extension<Claims>,
    Json(body): Json<svc::CreateSession>,
) -> Result<ApiResponse<crate::models::ChatSession>, ApiResponse<()>> {
    svc::create_session(&state.pool, claims.admin_id, body.title)
        .await
        .map(ApiResponse::success)
        .map_err(|e| e.into_response())
}

#[derive(Debug, Deserialize)]
pub struct ListSessionsQuery {
    pub offset: Option<i64>,
    pub limit: Option<i64>,
    pub search: Option<String>,
}

async fn list_sessions(
    State(state): State<AppState>,
    Extension(claims): Extension<Claims>,
    Query(query): Query<ListSessionsQuery>,
) -> Result<ApiResponse<svc::SessionList>, ApiResponse<()>> {
    let offset = query.offset.unwrap_or(0);
    let limit = query.limit.unwrap_or(50);
    let search = query.search.as_deref();
    svc::list_sessions(&state.pool, claims.admin_id, claims.role, offset, limit, search)
        .await
        .map(ApiResponse::success)
        .map_err(|e| e.into_response())
}

async fn delete_session(
    State(state): State<AppState>,
    Extension(claims): Extension<Claims>,
    Path(id): Path<i64>,
) -> Result<ApiResponse<()>, ApiResponse<()>> {
    let session = svc::fetch_session(&state.pool, id)
        .await
        .map_err(|e| e.into_response())?;
    svc::check_ownership(&session, claims.admin_id, claims.role).map_err(|e| e.into_response())?;
    svc::delete_session(&state.pool, id)
        .await
        .map(ApiResponse::success)
        .map_err(|e| e.into_response())
}

async fn get_messages(
    State(state): State<AppState>,
    Extension(claims): Extension<Claims>,
    Path(id): Path<i64>,
) -> Result<ApiResponse<Vec<crate::models::ChatMessage>>, ApiResponse<()>> {
    let session = svc::fetch_session(&state.pool, id)
        .await
        .map_err(|e| e.into_response())?;
    svc::check_ownership(&session, claims.admin_id, claims.role).map_err(|e| e.into_response())?;
    svc::list_messages(&state.pool, id)
        .await
        .map(ApiResponse::success)
        .map_err(|e| e.into_response())
}

/// 6 种 SSE 事件 (contracts/api.md §10 / FR-028 v7)
#[derive(Debug, Serialize)]
#[serde(tag = "_event", rename_all = "snake_case")]
#[allow(dead_code)]
enum ChatEvent {
    Token { text: String },
    ToolCall { tool_call_id: String, name: String, args: serde_json::Value },
    ToolResult { tool_call_id: String, result: serde_json::Value },
    Routed { agent_id: i64, agent_identifier: String },
    FallbackUsed { from: String, to: String, reason: String },
    Done { elapsed_ms: i64, final_agent_id: Option<i64> },
}

async fn post_message_sse(
    State(state): State<AppState>,
    Extension(claims): Extension<Claims>,
    Path(id): Path<i64>,
    Json(body): Json<PostMessage>,
) -> Response {
    // 1. session 校验 + 所有权
    let session = match svc::fetch_session(&state.pool, id).await {
        Ok(s) => s,
        Err(e) => return IntoResponse::into_response(AppError::into_response::<()>(e)),
    };
    if let Err(e) = svc::check_ownership(&session, claims.admin_id, claims.role) {
        return IntoResponse::into_response(AppError::into_response::<()>(e));
    }

    // 2. 并发限制（CHK232 / 4291）
    if !try_acquire_slot(claims.admin_id).await {
        return AppError::SseConcurrencyExceeded(
            "并发会话过多，请关闭其它对话窗口后重试".into(),
        )
        .into_response::<()>()
        .into_response();
    }
    let _guard = SseConcurrencyGuard { admin_id: claims.admin_id };

    // 3. persist user message before streaming（中断时 user 已入库，assistant 不写）
    let user_msg = match svc::append_user_message(&state.pool, id, &body.content).await {
        Ok(m) => m,
        Err(e) => return IntoResponse::into_response(AppError::into_response::<()>(e)),
    };

    // 4. 准备 SSE 流：T119 接入 orchestrator 多跳 tool-calling 循环
    //
    // orchestrator::run_session 内部完成：
    //   - 装配 main agent 资源（system_prompt + skill markdown + tools schema）
    //   - LLM tool-calling 循环（每跳调 provider.chat_stream → 解析 tool_calls →
    //     执行宿主工具或路由到子 agent → tool message 喂回）
    //   - emit 6 类 SSE 事件 + 终态 persist + audit (llm_invoke / agent_route)
    //   - 5-hop guard + 循环路由检测 (depth/visited)
    let pool = state.pool.clone();
    let session_id = id;
    let user_content = body.content.clone();
    let _ = user_msg.id;

    // 拉历史对话作为 LLM 上下文
    let history: Vec<crate::models::ChatMessage> =
        svc::list_messages(&state.pool, session_id).await.unwrap_or_default();

    // 创建 mpsc channel：orchestrator → SSE 流读
    let (tx, rx) = tokio::sync::mpsc::unbounded_channel::<Result<Event, Infallible>>();

    let deps = crate::runtime::orchestrator::OrchestratorDeps {
        pool: pool.clone(),
        s3: state.s3.clone(),
        llm: Arc::clone(&state.runtime_state.llm),
        registry: Arc::clone(&state.runtime_state.capabilities),
        invoker: Arc::clone(&state.runtime_state.invoker),
    };
    let history_clone = history.clone();
    tokio::spawn(async move {
        crate::runtime::orchestrator::run_session(
            deps,
            session_id,
            1, // main agent
            history_clone,
            user_content,
            tx,
        )
        .await;
    });

    // mpsc::UnboundedReceiver → Stream via futures::stream::unfold
    let final_stream: std::pin::Pin<
        Box<dyn Stream<Item = Result<Event, Infallible>> + Send>,
    > = Box::pin(stream::unfold(rx, |mut rx| async move {
        rx.recv().await.map(|item| (item, rx))
    }));

    let sse = Sse::new(final_stream).keep_alive(
        KeepAlive::new()
            .interval(Duration::from_secs(15))
            .text(": ping"),
    );

    // 5. 显式 headers (CHK196 / production reverse-proxy bypass)
    let mut headers = HeaderMap::new();
    headers.insert(
        header::CACHE_CONTROL,
        HeaderValue::from_static("no-cache, no-transform"),
    );
    headers.insert(header::CONNECTION, HeaderValue::from_static("keep-alive"));
    headers.insert(
        "x-accel-buffering",
        HeaderValue::from_static("no"),
    );

    (StatusCode::OK, headers, sse).into_response()
}

