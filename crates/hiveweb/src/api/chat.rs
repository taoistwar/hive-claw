//! Chat API + SSE (T127 / US6)
//!
//! `POST /api/chat/sessions/:id/messages` 返回 SSE 流。
//! 事件：token / tool_call / tool_result / routed / fallback_used / done | error
//!
//! 当前 MVP：AgentOrchestrator 真实路由 + LLM 流式接通待 US5 commit 2 完成；
//! 本 commit 提供完整 wire format + 并发限制 + headers + keep-alive；
//! 内容部分用一个 mock generator 拆分 user 输入为 token 流，便于前端联调。

use axum::{
    extract::{Path, State},
    http::{header, HeaderMap, HeaderValue, StatusCode},
    response::{
        sse::{Event, KeepAlive, Sse},
        IntoResponse, Response,
    },
    routing::{get, post},
    Extension, Json, Router,
};
use futures::stream::{self, Stream, StreamExt};
use serde::Serialize;
use serde_json::json;
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

async fn list_sessions(
    State(state): State<AppState>,
    Extension(claims): Extension<Claims>,
) -> Result<ApiResponse<svc::SessionList>, ApiResponse<()>> {
    svc::list_sessions(&state.pool, claims.admin_id, claims.role, 0, 50)
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

    // 4. 准备 SSE 流：MVP 用 mock generator 拆分 echo 回内容；
    //    AgentOrchestrator 真实接入由 T128 完成（与 crates/agent 集成）
    let pool = state.pool.clone();
    let session_id = id;
    let echo = body.content.clone();
    let elapsed_start = std::time::Instant::now();

    let stream = stream::unfold(
        (0_usize, echo, false),
        move |(i, echo, done)| async move {
            if done {
                return None;
            }
            let chunks: Vec<String> = echo
                .chars()
                .collect::<Vec<_>>()
                .chunks(8)
                .map(|c| c.iter().collect())
                .collect();
            if i < chunks.len() {
                let payload = json!({ "text": chunks[i] });
                let ev = Event::default()
                    .event("token")
                    .data(payload.to_string());
                tokio::time::sleep(Duration::from_millis(30)).await;
                Some((Ok::<_, Infallible>(ev), (i + 1, echo, false)))
            } else {
                Some((Ok::<_, Infallible>(Event::default()), (i + 1, echo, true)))
            }
        },
    );

    // assistant 完成事件 + persist
    let user_msg_id = user_msg.id;
    let done_stream = futures::stream::once(async move {
        let elapsed = elapsed_start.elapsed().as_millis() as i32;
        // persist assistant message (echo 回内容)
        let _ = svc::append_assistant_message(
            &pool,
            session_id,
            &format!("(echo) {}", user_msg_id),
            None,
            Some(elapsed),
        )
        .await;
        let payload = json!({
            "elapsed_ms": elapsed,
            "final_agent_id": null,
        });
        Ok::<_, Infallible>(Event::default().event("done").data(payload.to_string()))
    });

    let final_stream: std::pin::Pin<
        Box<dyn Stream<Item = Result<Event, Infallible>> + Send>,
    > = Box::pin(stream.chain(done_stream));

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

