//! User chat API — user chat_sessions_user + chat_messages_user
//!
//! Mounted on user-protected router at /api/user-chat/*

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
use serde::Deserialize;
use std::collections::HashMap;
use std::convert::Infallible;
use std::sync::Arc;
use std::time::Duration;
use tokio::sync::Mutex;

use providers::{ChatRequest, RetryMode};
use serde_json::json;

use crate::api::AppState;
use crate::services::chat as svc;
use crate::utils::error::ApiResponse;
use crate::utils::jwt::Claims;

static SSE_COUNTER: once_cell::sync::Lazy<Arc<Mutex<HashMap<i64, usize>>>> =
    once_cell::sync::Lazy::new(|| Arc::new(Mutex::new(HashMap::new())));

struct SseConcurrencyGuard {
    actor_id: i64,
}

impl Drop for SseConcurrencyGuard {
    fn drop(&mut self) {
        let actor_id = self.actor_id;
        let counter = SSE_COUNTER.clone();
        tokio::spawn(async move {
            let mut map = counter.lock().await;
            if let Some(c) = map.get_mut(&actor_id) {
                *c = c.saturating_sub(1);
                if *c == 0 {
                    map.remove(&actor_id);
                }
            }
        });
    }
}

async fn try_acquire_slot(actor_id: i64) -> bool {
    let mut map = SSE_COUNTER.lock().await;
    let cap: usize = std::env::var("CHAT_SSE_MAX_CONCURRENT_PER_ADMIN")
        .ok()
        .and_then(|v| v.parse().ok())
        .unwrap_or(2);
    let entry = map.entry(actor_id).or_insert(0);
    if *entry >= cap {
        return false;
    }
    *entry += 1;
    true
}

pub fn router() -> Router<AppState> {
    Router::new()
        .route(
            "/user-chat/sessions",
            get(list_sessions).post(create_session),
        )
        .route(
            "/user-chat/sessions/:id",
            axum::routing::delete(delete_session),
        )
        .route(
            "/user-chat/sessions/:id/messages",
            get(get_messages).post(post_message_sse),
        )
}

#[derive(Debug, Deserialize)]
pub struct ListSessionsQuery {
    pub offset: Option<i64>,
    pub limit: Option<i64>,
    pub search: Option<String>,
}

async fn create_session(
    State(state): State<AppState>,
    Extension(claims): Extension<Claims>,
    Json(body): Json<svc::CreateSession>,
) -> Result<ApiResponse<crate::models::ChatSessionUser>, ApiResponse<()>> {
    let user_id = claims.user_id.unwrap_or_default();
    svc::create_user_session(&state.pool, user_id, body.title)
        .await
        .map(ApiResponse::success)
        .map_err(|e| e.into_response())
}

async fn list_sessions(
    State(state): State<AppState>,
    Extension(claims): Extension<Claims>,
    Query(query): Query<ListSessionsQuery>,
) -> Result<ApiResponse<svc::SessionList>, ApiResponse<()>> {
    let user_id = claims.user_id.unwrap_or_default();
    let offset = query.offset.unwrap_or(0);
    let limit = query.limit.unwrap_or(50);
    let search = query.search.as_deref();
    svc::list_sessions_user(&state.pool, user_id, offset, limit, search)
        .await
        .map(ApiResponse::success)
        .map_err(|e| e.into_response())
}

async fn delete_session(
    State(state): State<AppState>,
    Extension(claims): Extension<Claims>,
    Path(id): Path<i64>,
) -> Result<ApiResponse<()>, ApiResponse<()>> {
    let session = svc::fetch_session_user(&state.pool, id)
        .await
        .map_err(|e| e.into_response())?;
    svc::check_ownership_user(&session, claims.user_id).map_err(|e| e.into_response())?;
    svc::delete_session_user(&state.pool, id)
        .await
        .map(ApiResponse::success)
        .map_err(|e| e.into_response())
}

async fn get_messages(
    State(state): State<AppState>,
    Extension(claims): Extension<Claims>,
    Path(id): Path<i64>,
) -> Result<ApiResponse<Vec<crate::models::ChatMessageUser>>, ApiResponse<()>> {
    let session = svc::fetch_session_user(&state.pool, id)
        .await
        .map_err(|e| e.into_response())?;
    svc::check_ownership_user(&session, claims.user_id).map_err(|e| e.into_response())?;
    svc::list_messages_user(&state.pool, id)
        .await
        .map(ApiResponse::success)
        .map_err(|e| e.into_response())
}

async fn post_message_sse(
    State(state): State<AppState>,
    Extension(claims): Extension<Claims>,
    Path(id): Path<i64>,
    Json(body): Json<svc::PostMessage>,
) -> Response {
    let user_id = claims.user_id.unwrap_or_default();

    let session = match svc::fetch_session_user(&state.pool, id).await {
        Ok(s) => s,
        Err(e) => return IntoResponse::into_response(e.into_response::<()>()),
    };
    if let Err(e) = svc::check_ownership_user(&session, claims.user_id) {
        return IntoResponse::into_response(e.into_response::<()>());
    }

    if !try_acquire_slot(user_id).await {
        return crate::utils::error::AppError::SseConcurrencyExceeded(
            "并发会话过多，请关闭其它对话窗口后重试".into(),
        )
        .into_response::<()>()
        .into_response();
    }
    let _guard = SseConcurrencyGuard { actor_id: user_id };

    let user_msg = match svc::append_user_message_user(&state.pool, id, &body.content).await {
        Ok(m) => m,
        Err(e) => return IntoResponse::into_response(e.into_response::<()>()),
    };

    // Auto-generate title from first message (first 30 chars)
    if session.title.is_none() || session.title.as_ref().map_or(true, |t| t.is_empty()) {
        let title = body.content.chars().take(30).collect::<String>();
        let _ = sqlx::query("UPDATE chat_sessions_user SET title = ? WHERE id = ?")
            .bind(&title)
            .bind(id)
            .execute(&state.pool)
            .await;
    }

    let pool = state.pool.clone();
    let session_id = id;
    let user_content = body.content.clone();

    let history: Vec<crate::models::ChatMessageUser> =
        svc::list_messages_user(&state.pool, session_id).await.unwrap_or_default();

    let (tx, rx) = tokio::sync::mpsc::unbounded_channel::<Result<Event, Infallible>>();

    let deps = crate::runtime::orchestrator::OrchestratorDeps {
        pool: pool.clone(),
        s3: state.s3.clone(),
        llm: Arc::clone(&state.runtime_state.llm),
        registry: Arc::clone(&state.runtime_state.capabilities),
        invoker: Arc::clone(&state.runtime_state.invoker),
    };
    // Use the same Agent orchestrator as admin, but writes to user chat tables
    tokio::spawn(async move {
        crate::runtime::orchestrator::run_session_user(deps, session_id, 1, history, user_content, tx).await;
    });

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

    let mut headers = HeaderMap::new();
    headers.insert(
        header::CACHE_CONTROL,
        HeaderValue::from_static("no-cache, no-transform"),
    );
    headers.insert(header::CONNECTION, HeaderValue::from_static("keep-alive"));
    headers.insert("x-accel-buffering", HeaderValue::from_static("no"));

    (StatusCode::OK, headers, sse).into_response()
}

async fn user_chat_loop(
    deps: crate::runtime::orchestrator::OrchestratorDeps,
    session_id: i64,
    history: Vec<crate::models::ChatMessageUser>,
    user_content: String,
    tx: tokio::sync::mpsc::UnboundedSender<Result<Event, Infallible>>,
) {
    use std::time::Instant;
    let elapsed_start = Instant::now();

    // Build messages for LLM
    let messages: Vec<serde_json::Value> = history
        .into_iter()
        .filter_map(|m| m.content.map(|c| json!({"role": m.role, "content": c})))
        .chain(std::iter::once(json!({"role": "user", "content": user_content.clone()})))
        .collect();

    let (provider, model) = match deps.llm.build_primary(None) {
        Ok(p) => p,
        Err(e) => {
            tracing::error!("LLM provider error: {}", e);
            let _ = tx.send(Ok(Event::default()
                .event("error")
                .data(json!({"code": 5000, "message": format!("LLM 配置错误: {}", e)}).to_string())));
            return;
        }
    };

    let req = ChatRequest {
        model: Some(model.clone()),
        messages,
        max_tokens: 2048,
        temperature: 0.7,
        tools: None,
        tool_choice: None,
        reasoning_effort: None,
    };

    let tx_inner = tx.clone();
    let on_delta: providers::StreamDeltaCallback = Arc::new(move |delta: String| {
        let payload = json!({ "text": delta });
        let ev = Event::default().event("token").data(payload.to_string());
        let _ = tx_inner.send(Ok::<_, Infallible>(ev));
    });

    let resp = provider
        .chat_stream_with_retry(req, Some(on_delta), None, RetryMode::Standard, None, None)
        .await;

    if resp.is_error() {
        let msg = resp
            .content
            .clone()
            .or(resp.error_kind.clone())
            .unwrap_or_else(|| "LLM error".into());
        let _ = tx.send(Ok(Event::default()
            .event("error")
            .data(json!({"code": resp.error_status_code.unwrap_or(5000) as u16, "message": msg}).to_string())));
        return;
    }

    let assistant_content = resp.content.unwrap_or_default();
    let elapsed_ms = elapsed_start.elapsed().as_millis() as i32;

    // Persist assistant response to user chat_messages_user
    let _ = svc::append_assistant_message_user(&deps.pool, session_id, &assistant_content, Some(elapsed_ms)).await;

    let _ = tx.send(Ok(Event::default()
        .event("done")
        .data(json!({"elapsed_ms": elapsed_ms}).to_string())));
}
