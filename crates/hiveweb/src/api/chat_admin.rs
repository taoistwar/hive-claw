//! Admin chat API — chat_sessions_admin + chat_messages_admin.
//!
//! Mounted on admin-protected router at /api/admin-chat/*

use axum::response::sse::Event;
use axum::{
    Extension, Json, Router,
    extract::{Path, Query, State},
    response::{IntoResponse, Response},
    routing::get,
};
use futures::stream::{self, Stream};
use serde::Serialize;
use std::convert::Infallible;
use std::sync::Arc;

use crate::api::AppState;
use crate::api::chat_common::{
    ListSessionsQuery, SseConcurrencyGuard, SseSlotConfig, sse_response, try_acquire_slot,
};
use crate::models::chat_admin::ChatSessionAdmin;
use crate::services::chat::{CreateSession, PostMessage, SessionList};
use crate::services::chat_admin::{
    append_user_message_admin, check_ownership_admin, create_session_admin, delete_session_admin, fetch_session_admin, list_messages_admin, list_sessions_admin
};
use crate::utils::error::{ApiResponse, AppError};
use crate::utils::jwt::Claims;

/// Admin chat routes — mounted on admin-protected router
pub fn admin_router() -> Router<AppState> {
    Router::new()
        .route(
            "/admin-chat/sessions",
            get(admin_list_sessions).post(admin_create_session),
        )
        .route(
            "/admin-chat/sessions/:id",
            axum::routing::delete(admin_delete_session),
        )
        .route(
            "/admin-chat/sessions/:id/messages",
            get(admin_get_messages).post(admin_post_message_sse),
        )
}

// ---- Admin handlers ----

async fn admin_create_session(
    State(state): State<AppState>,
    Extension(claims): Extension<Claims>,
    Json(body): Json<CreateSession>,
) -> Result<ApiResponse<ChatSessionAdmin>, ApiResponse<()>> {
    let admin_id = match claims.admin_id {
        Some(id) => id,
        None => return Err(AppError::Internal("No admin context".to_string()).into_response()),
    };
    create_session_admin(&state.pool, admin_id, body.title)
        .await
        .map(ApiResponse::success)
        .map_err(|e| e.into_response())
}

async fn admin_list_sessions(
    State(state): State<AppState>,
    Extension(claims): Extension<Claims>,
    Query(query): Query<ListSessionsQuery>,
) -> Result<ApiResponse<SessionList>, ApiResponse<()>> {
    let offset = query.offset.unwrap_or(0);
    let limit = query.limit.unwrap_or(50);
    let search = query.search.as_deref();
    list_sessions_admin(
        &state.pool,
        claims.admin_id,
        claims.role,
        offset,
        limit,
        search,
    )
    .await
    .map(ApiResponse::success)
    .map_err(|e| e.into_response())
}

async fn admin_delete_session(
    State(state): State<AppState>,
    Extension(claims): Extension<Claims>,
    Path(id): Path<i64>,
) -> Result<ApiResponse<()>, ApiResponse<()>> {
    let session = fetch_session_admin(&state.pool, id)
        .await
        .map_err(|e| e.into_response())?;
    check_ownership_admin(&session, claims.admin_id, claims.role).map_err(|e| e.into_response())?;
    delete_session_admin(&state.pool, id)
        .await
        .map(ApiResponse::success)
        .map_err(|e| e.into_response())
}

async fn admin_get_messages(
    State(state): State<AppState>,
    Extension(claims): Extension<Claims>,
    Path(id): Path<i64>,
) -> Result<ApiResponse<Vec<crate::models::ChatMessageAdmin>>, ApiResponse<()>> {
    let session = fetch_session_admin(&state.pool, id)
        .await
        .map_err(|e| e.into_response())?;
    check_ownership_admin(&session, claims.admin_id, claims.role).map_err(|e| e.into_response())?;
    list_messages_admin(&state.pool, id)
        .await
        .map(ApiResponse::success)
        .map_err(|e| e.into_response())
}

/// 6 种 SSE 事件 (contracts/api.md §10 / FR-028 v7)
#[derive(Debug, Serialize)]
#[serde(tag = "_event", rename_all = "snake_case")]
#[allow(dead_code)]
enum ChatEvent {
    Token {
        text: String,
    },
    ToolCall {
        tool_call_id: String,
        name: String,
        args: serde_json::Value,
    },
    ToolResult {
        tool_call_id: String,
        result: serde_json::Value,
    },
    Routed {
        agent_id: i64,
        agent_identifier: String,
    },
    FallbackUsed {
        from: String,
        to: String,
        reason: String,
    },
    Done {
        elapsed_ms: i64,
        final_agent_id: Option<i64>,
    },
}

async fn admin_post_message_sse(
    State(state): State<AppState>,
    Extension(claims): Extension<Claims>,
    Path(id): Path<i64>,
    Json(body): Json<PostMessage>,
) -> Response {
    let admin_id = claims
        .admin_id
        .ok_or_else(|| AppError::Internal("No admin context".to_string()));
    let admin_id = match admin_id {
        Ok(id) => id,
        Err(e) => return IntoResponse::into_response(AppError::into_response::<()>(e)),
    };

    let session = match fetch_session_admin(&state.pool, id).await {
        Ok(s) => s,
        Err(e) => return IntoResponse::into_response(AppError::into_response::<()>(e)),
    };
    if let Err(e) = check_ownership_admin(&session, claims.admin_id, claims.role) {
        return IntoResponse::into_response(AppError::into_response::<()>(e));
    }

    if !try_acquire_slot(admin_id, SseSlotConfig::ADMIN).await {
        return AppError::SseConcurrencyExceeded("并发会话过多，请关闭其它对话窗口后重试".into())
            .into_response::<()>()
            .into_response();
    }
    let _guard = SseConcurrencyGuard {
        actor_id: admin_id,
        is_admin: true,
    };

    let user_msg = match append_user_message_admin(&state.pool, id, admin_id, &body.content).await {
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
        list_messages_admin(&state.pool, session_id)
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
    };
    let history_clone = history.clone();
    tokio::spawn(async move {
        crate::runtime::orchestrator::run_session_admin(
            deps,
            session_id,
            1,
            admin_id,
            history_clone,
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
