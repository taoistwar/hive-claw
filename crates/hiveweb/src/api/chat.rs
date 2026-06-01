//! Chat API + SSE (T127 / US6) — Admin and user routes separated.
//!
//! Admin routes: /api/chat/sessions (admin chat)
//! User routes: handled in users chat sub-routes
//!
//! Admin session -> chat_sessions_admin + chat_messages_admin
//! User session -> chat_sessions_user + chat_messages_user

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

/// 单 actor 并发 SSE 流上限
fn max_concurrent_per_actor() -> usize {
    std::env::var("CHAT_SSE_MAX_CONCURRENT_PER_ADMIN")
        .ok()
        .and_then(|v| v.parse().ok())
        .unwrap_or(2)
}

/// 进程内 per-actor SSE 计数器
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
    let cap = max_concurrent_per_actor();
    let entry = map.entry(actor_id).or_insert(0);
    if *entry >= cap {
        return false;
    }
    *entry += 1;
    true
}

/// Admin chat routes — mounted on admin-protected router
pub fn admin_router() -> Router<AppState> {
    Router::new()
        .route(
            "/chat/sessions",
            get(admin_list_sessions).post(admin_create_session),
        )
        .route(
            "/chat/sessions/:id",
            axum::routing::delete(admin_delete_session),
        )
        .route(
            "/chat/sessions/:id/messages",
            get(admin_get_messages).post(admin_post_message_sse),
        )
}

// ---- Admin handlers ----

async fn admin_create_session(
    State(state): State<AppState>,
    Extension(claims): Extension<Claims>,
    Json(body): Json<svc::CreateSession>,
) -> Result<ApiResponse<crate::models::ChatSessionAdmin>, ApiResponse<()>> {
    let admin_id = match claims.admin_id {
        Some(id) => id,
        None => return Err(AppError::Internal("No admin context".to_string()).into_response()),
    };
    svc::create_session(&state.pool, admin_id, body.title)
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

async fn admin_list_sessions(
    State(state): State<AppState>,
    Extension(claims): Extension<Claims>,
    Query(query): Query<ListSessionsQuery>,
) -> Result<ApiResponse<svc::SessionList>, ApiResponse<()>> {
    let offset = query.offset.unwrap_or(0);
    let limit = query.limit.unwrap_or(50);
    let search = query.search.as_deref();
    svc::list_sessions_admin(&state.pool, claims.admin_id, claims.role, offset, limit, search)
        .await
        .map(ApiResponse::success)
        .map_err(|e| e.into_response())
}

async fn admin_delete_session(
    State(state): State<AppState>,
    Extension(claims): Extension<Claims>,
    Path(id): Path<i64>,
) -> Result<ApiResponse<()>, ApiResponse<()>> {
    let session = svc::fetch_session_admin(&state.pool, id)
        .await
        .map_err(|e| e.into_response())?;
    svc::check_ownership_admin(&session, claims.admin_id, claims.role).map_err(|e| e.into_response())?;
    svc::delete_session_admin(&state.pool, id)
        .await
        .map(ApiResponse::success)
        .map_err(|e| e.into_response())
}

async fn admin_get_messages(
    State(state): State<AppState>,
    Extension(claims): Extension<Claims>,
    Path(id): Path<i64>,
) -> Result<ApiResponse<Vec<crate::models::ChatMessageAdmin>>, ApiResponse<()>> {
    let session = svc::fetch_session_admin(&state.pool, id)
        .await
        .map_err(|e| e.into_response())?;
    svc::check_ownership_admin(&session, claims.admin_id, claims.role).map_err(|e| e.into_response())?;
    svc::list_messages_admin(&state.pool, id)
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

async fn admin_post_message_sse(
    State(state): State<AppState>,
    Extension(claims): Extension<Claims>,
    Path(id): Path<i64>,
    Json(body): Json<PostMessage>,
) -> Response {
    let admin_id = claims.admin_id.ok_or_else(|| {
        AppError::Internal("No admin context".to_string())
    });
    let admin_id = match admin_id {
        Ok(id) => id,
        Err(e) => return IntoResponse::into_response(AppError::into_response::<()>(e)),
    };

    let session = match svc::fetch_session_admin(&state.pool, id).await {
        Ok(s) => s,
        Err(e) => return IntoResponse::into_response(AppError::into_response::<()>(e)),
    };
    if let Err(e) = svc::check_ownership_admin(&session, claims.admin_id, claims.role) {
        return IntoResponse::into_response(AppError::into_response::<()>(e));
    }

    if !try_acquire_slot(admin_id).await {
        return AppError::SseConcurrencyExceeded(
            "并发会话过多，请关闭其它对话窗口后重试".into(),
        )
        .into_response::<()>()
        .into_response();
    }
    let _guard = SseConcurrencyGuard { actor_id: admin_id };

    let user_msg = match svc::append_user_message_admin(&state.pool, id, &body.content).await {
        Ok(m) => m,
        Err(e) => return IntoResponse::into_response(e.into_response::<()>()),
    };

    // Auto-generate title from first message (first 30 chars)
    if session.title.is_none() || session.title.as_ref().map_or(true, |t| t.is_empty()) {
        let title = body.content.chars().take(30).collect::<String>();
        let _ = sqlx::query("UPDATE chat_sessions_admin SET title = ? WHERE id = ?")
            .bind(&title)
            .bind(id)
            .execute(&state.pool)
            .await;
    }

    let pool = state.pool.clone();
    let session_id = id;
    let user_content = body.content.clone();
    let _ = user_msg.id;

    let history: Vec<crate::models::ChatMessageAdmin> =
        svc::list_messages_admin(&state.pool, session_id).await.unwrap_or_default();

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
        crate::runtime::orchestrator::run_session_admin(
            deps,
            session_id,
            1,
            history_clone,
            user_content,
            tx,
        )
        .await;
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
    headers.insert(
        "x-accel-buffering",
        HeaderValue::from_static("no"),
    );

    (StatusCode::OK, headers, sse).into_response()
}
