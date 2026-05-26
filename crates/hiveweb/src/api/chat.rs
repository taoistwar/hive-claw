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

    // 4. 准备 SSE 流：T128 实接 LLM stream
    //
    // 资源装配（main agent 视角，T128 MVP — 多 Agent routing 留后续）：
    //   1. resolve main agent (id=1) → 取 system_prompt + skill markdown 拼装 + permissions
    //   2. resolve model_preset → providers::build_provider 拿 Arc<dyn LLMProvider>
    //   3. provider.chat_stream(req, on_delta=回调推 mpsc) — 把 token 塞 mpsc::channel
    //   4. SSE 流 = ReceiverStream<Event>
    //
    // 无 API key / preset 缺失 → emit error event 而非 5xx，让前端能看到具体错误
    let pool = state.pool.clone();
    let session_id = id;
    let llm = Arc::clone(&state.runtime_state.llm);
    let user_content = body.content.clone();
    let elapsed_start = std::time::Instant::now();
    let _ = user_msg.id;

    // 拉 main agent 配置（id=1） — 简化：T128 MVP 暂用 main，后续接 route_to_subagent
    let (system_prompt, model_preset_name) = match sqlx::query_as::<_, (String, Option<String>)>(
        "SELECT system_prompt, model_preset FROM agents WHERE id = 1",
    )
    .fetch_optional(&state.pool)
    .await
    {
        Ok(Some((sp, mp))) => (sp, mp),
        _ => (
            "You are a helpful assistant.".to_string(),
            None,
        ),
    };

    // 拉 main agent 的 skill markdown 拼到 system prompt
    let skills: Vec<(String,)> = sqlx::query_as(
        r#"SELECT s.content FROM skills s
           JOIN agent_skills ax ON ax.skill_id = s.id
           WHERE ax.agent_id = 1"#,
    )
    .fetch_all(&state.pool)
    .await
    .unwrap_or_default();
    let mut sys = system_prompt;
    for (md,) in &skills {
        sys.push_str("\n\n");
        sys.push_str(md);
    }

    // 拉历史对话作为 LLM 上下文
    let history: Vec<crate::models::ChatMessage> =
        svc::list_messages(&state.pool, session_id).await.unwrap_or_default();

    // 构造 provider
    let (provider, model) = match llm.build_primary(model_preset_name.as_deref()) {
        Ok(p) => p,
        Err(e) => {
            // 立即 emit error 事件然后 done — 整个流退化为单事件
            let error_evt = Event::default()
                .event("error")
                .data(json!({"code": 5007, "message": format!("preset error: {e}")}).to_string());
            let final_stream: std::pin::Pin<
                Box<dyn Stream<Item = Result<Event, Infallible>> + Send>,
            > = Box::pin(stream::iter(vec![Ok::<_, Infallible>(error_evt)]));
            let sse = Sse::new(final_stream).keep_alive(KeepAlive::new());
            let mut headers = HeaderMap::new();
            headers.insert(header::CACHE_CONTROL, HeaderValue::from_static("no-cache, no-transform"));
            headers.insert(header::CONNECTION, HeaderValue::from_static("keep-alive"));
            headers.insert("x-accel-buffering", HeaderValue::from_static("no"));
            return (StatusCode::OK, headers, sse).into_response();
        }
    };

    // 把 LLM 历史拼成 ChatRequest.messages（OpenAI-style）
    use providers::ChatRequest;
    let mut messages: Vec<serde_json::Value> = Vec::new();
    messages.push(json!({"role": "system", "content": sys}));
    for m in &history {
        if let Some(c) = &m.content {
            messages.push(json!({"role": m.role, "content": c}));
        }
    }
    messages.push(json!({"role": "user", "content": user_content}));

    let req = ChatRequest {
        model: Some(model.clone()),
        messages,
        max_tokens: 2048,
        temperature: 0.7,
        tools: None,
        tool_choice: None,
        reasoning_effort: None,
    };

    // 创建 mpsc channel：LLM streaming task → SSE 流读
    let (tx, rx) = tokio::sync::mpsc::unbounded_channel::<Result<Event, Infallible>>();

    let pool_clone = pool.clone();
    tokio::spawn(async move {
        let tx_inner = tx.clone();
        let on_delta: providers::StreamDeltaCallback = Arc::new(move |delta: String| {
            let payload = json!({ "text": delta });
            let ev = Event::default().event("token").data(payload.to_string());
            let _ = tx_inner.send(Ok::<_, Infallible>(ev));
        });
        let resp = provider.chat_stream(req, Some(on_delta), None).await;

        let elapsed = elapsed_start.elapsed().as_millis() as i32;

        if resp.is_error() {
            let err_msg = resp
                .content
                .clone()
                .or(resp.error_kind.clone())
                .unwrap_or_else(|| "LLM error".into());
            let ev = Event::default()
                .event("error")
                .data(
                    json!({
                        "code": resp.error_status_code.unwrap_or(5000),
                        "message": err_msg,
                    })
                    .to_string(),
                );
            let _ = tx.send(Ok(ev));
        } else if let Some(text) = resp.content.clone() {
            // persist assistant message
            let _ = svc::append_assistant_message(
                &pool_clone,
                session_id,
                &text,
                None,
                Some(elapsed),
            )
            .await;
        }

        let done_ev = Event::default()
            .event("done")
            .data(json!({"elapsed_ms": elapsed, "final_agent_id": null}).to_string());
        let _ = tx.send(Ok(done_ev));
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

