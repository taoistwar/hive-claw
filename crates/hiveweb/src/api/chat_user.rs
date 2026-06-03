//! User chat API — user chat_sessions_user + chat_messages_user.
//!
//! Mounted on user-protected router at /api/user-chat/*

use axum::response::sse::Event;
use axum::{
    Extension, Json, Router,
    extract::{Path, Query, State},
    response::{IntoResponse, Response},
    routing::get,
};
use futures::stream::{self, Stream};
use std::convert::Infallible;
use std::sync::Arc;

use crate::api::AppState;
use crate::api::chat_common::{
    ListSessionsQuery, SseConcurrencyGuard, SseSlotConfig, sse_response, try_acquire_slot,
};
use crate::services::chat::{CreateSession, PostMessage, SessionList};
use crate::services::chat_user as svc;
use crate::utils::error::ApiResponse;
use crate::utils::jwt::Claims;

pub fn user_router() -> Router<AppState> {
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

async fn create_session(
    State(state): State<AppState>,
    Extension(claims): Extension<Claims>,
    Json(body): Json<CreateSession>,
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
) -> Result<ApiResponse<SessionList>, ApiResponse<()>> {
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
    Path(session_id): Path<i64>,
    Json(body): Json<PostMessage>,
) -> Response {
    let user_id = claims.user_id.unwrap_or_default();

    let session = match svc::fetch_session_user(&state.pool, session_id).await {
        Ok(s) => s,
        Err(e) => return IntoResponse::into_response(e.into_response::<()>()),
    };
    if let Err(e) = svc::check_ownership_user(&session, claims.user_id) {
        return IntoResponse::into_response(e.into_response::<()>());
    }

    if !try_acquire_slot(user_id, SseSlotConfig::USER).await {
        return crate::utils::error::AppError::SseConcurrencyExceeded(
            "并发会话过多，请关闭其它对话窗口后重试".into(),
        )
        .into_response::<()>()
        .into_response();
    }
    let _guard = SseConcurrencyGuard {
        actor_id: user_id,
        is_admin: false,
    };

    match svc::append_user_message_user(&state.pool, session_id, user_id, &body.content).await {
        Ok(m) => m,
        Err(e) => return IntoResponse::into_response(e.into_response::<()>()),
    };

    // Auto-generate title from first message (first 30 chars)
    if session.title.is_none() || session.title.as_ref().map_or(true, |t| t.is_empty()) {
        let title = body.content.chars().take(30).collect::<String>();
        let _ = sqlx::query("UPDATE chat_sessions_user SET title = ? WHERE id = ?")
            .bind(&title)
            .bind(session_id)
            .execute(&state.pool)
            .await;
    }

    let pool = state.pool.clone();
    let session_id = session_id;
    let user_content = body.content.clone();

    let history: Vec<crate::models::ChatMessageUser> =
        svc::list_messages_user(&state.pool, session_id)
            .await
            .unwrap_or_default();

    let (tx, rx) = tokio::sync::mpsc::unbounded_channel::<Result<Event, Infallible>>();

    let deps = crate::runtime::orchestrator::OrchestratorDeps {
        pool: pool.clone(),
        s3: state.s3.clone(),
        llm: Arc::clone(&state.runtime_state.llm),
        registry: Arc::clone(&state.runtime_state.capabilities),
        invoker: Arc::clone(&state.runtime_state.invoker),
        ext_pool: state.ext_pool.clone(),
        message: user_content.clone(),
        channel: String::new(),
        platform: String::new(),
        app_version: String::new(),
    };
    // Use the same Agent orchestrator as admin, but writes to user chat tables
    tokio::spawn(async move {
        crate::runtime::orchestrator::run_session_user(
            deps,
            session_id,
            1,
            user_id,
            history,
            user_content,
            tx,
        )
        .await;
    });

    let final_stream: std::pin::Pin<Box<dyn Stream<Item = Result<Event, Infallible>> + Send>> =
        Box::pin(stream::unfold(rx, |mut rx| async move {
            rx.recv().await.map(|item| (item, rx))
        }));

    sse_response(final_stream)
}
