//! Assistant API — 对外公开的 AI 助手接口（MD5 签名鉴权）
//!
//! POST /api/assistant?sign={md5}
//!
//! 请求头：
//!   Content-Type: application/json; charset=UTF-8
//!
//! 请求体：
//!   { "user_id": 123, "message": "你好", "channel": "app", "client_type": "android", "client_version": "1.0.0" }
//!
//! 处理流程：
//!   1. Content-Type 校验
//!   2. MD5 签名校验（MD5(ASSISTANT_SECRET + "/api/assistant" + "?body=" + body)）
//!   3. user_id > 0 校验
//!   4. message 非空校验
//!   5. 外部 DB cloud_user 校验
//!   6. 外部 DB cc_user_membership VIP 判定（effective_end_time >= now）
//!   7. Redis 日访问次数限流（VIP: vip_ask_times, 普通: normal_ask_times）
//!   8. 内部 DB 同步用户（users 表，不存在则创建）
//!   9. 获取或创建 session → append_user_message_user → 更新 title →
//!      构建 OrchestratorDeps → run_session_user → 收集完整回复 → 返回 JSON

use axum::response::sse::Event;
use axum::{
    Router,
    extract::{Extension, Query, State},
    response::{IntoResponse, Response},
    routing::{get, post},
};
use redis::AsyncCommands;
use serde::Deserialize;
use std::collections::HashMap;
use std::convert::Infallible;
use std::sync::atomic::AtomicBool;
use std::sync::{Arc, OnceLock};

use crate::api::AppState;
use crate::api::chat_common;
use crate::api::chat_common::{SseConcurrencyGuard, SseSlotConfig, try_acquire_slot};
use crate::cache::redis::RedisClient;
use crate::middleware::request_id::RequestId;
use crate::runtime::execution_context::RuntimeExecutionContext;
use crate::services::chat_user as svc;
use crate::services::membership;
use crate::services::user_auth;
use crate::utils::error::{AppError, codes};

/// 预共享密钥，从环境变量 ASSISTANT_SECRET 懒加载
static SECRET: OnceLock<String> = OnceLock::new();

fn get_secret() -> &'static str {
    SECRET
        .get_or_init(|| std::env::var("ASSISTANT_SECRET").unwrap_or_default())
        .as_str()
}

pub fn router() -> Router<AppState> {
    Router::new()
        .route("/assistant", post(assistant_chat))
        .route("/quota", get(assistant_quota))
}

// ── 请求 / 响应 ──

#[derive(Debug, Deserialize)]
pub struct AssistantRequest {
    pub user_id: i64,
    pub message: String,
    /// 渠道（如 "app", "web", "api" 等）
    pub channel: String,
    /// 端：android、iphone、ipad、web 等
    pub client_type: String,
    /// 客户端版本号
    pub client_version: String,
    /// 是否创建新会话（保留字段，由 /api/newsession 接口处理新会话创建逻辑）
    #[serde(default)]
    #[allow(dead_code)]
    pub new_session: bool,
}

// ── 日访问次数限流（Redis） ──

async fn assistant_chat(
    State(state): State<AppState>,
    Extension(RequestId(request_id)): Extension<RequestId>,
    headers: axum::http::HeaderMap,
    Query(params): Query<HashMap<String, String>>,
    body: String,
) -> Response {
    // 0. Content-Type 校验
    let content_type = headers
        .get(axum::http::header::CONTENT_TYPE)
        .and_then(|v| v.to_str().ok())
        .unwrap_or("");
    if content_type != "application/json; charset=UTF-8" {
        return AppError::BadRequest(
            "Invalid Content-Type, must be: application/json; charset=UTF-8".into(),
        )
        .into_response::<()>()
        .into_response();
    }

    // 1. MD5 签名校验
    let secret = get_secret();
    if !secret.is_empty() {
        let sign = params.get("sign").map(|s| s.as_str()).unwrap_or("");
        if !chat_common::verify_sign(secret, "/api/assistant", &body, sign) {
            return AppError::BadRequest("Invalid signature".into())
                .into_response::<()>()
                .into_response();
        }
    }

    // 2. JSON 解析
    let req: AssistantRequest = match serde_json::from_str(&body) {
        Ok(r) => r,
        Err(_) => {
            return AppError::BadRequest("Invalid request body".into())
                .into_response::<()>()
                .into_response();
        }
    };

    // 3. user_id > 0 校验
    if req.user_id <= 0 {
        return AppError::BadRequest("user_id must be positive".into())
            .into_response::<()>()
            .into_response();
    }

    // 4. message 非空 + 长度校验
    if req.message.trim().is_empty() {
        return AppError::BadRequest("message must not be empty".into())
            .into_response::<()>()
            .into_response();
    }
    if req.message.chars().count() > 500 {
        return AppError::BadRequest("message must not exceed 500 characters".into())
            .into_response::<()>()
            .into_response();
    }

    // 4b. Sensitive word filter — input check (010-sensitive-word-filter)
    if state.sensitive_filter.check(&req.message).is_some() {
        tracing::info!(
            user_id = req.user_id,
            "Assistant input blocked by sensitive filter"
        );
        return AppError::SensitiveWordBlocked(
            "内容安全警告：输入的文本数据可能包含不适当的内容！".into(),
        )
        .into_response::<()>()
        .into_response();
    }

    let ext_pool = match &state.ext_pool {
        Some(p) => p,
        None => {
            return AppError::Internal("Assistant service unavailable".into())
                .into_response::<()>()
                .into_response();
        }
    };

    // 5. 获取 cloud_user 信息并校验用户是否存在（缓存优先）
    let cloud_info =
        match membership::get_cloud_user_info_cached(&state.redis, ext_pool, req.user_id).await {
            Ok(Some(info)) => info,
            Ok(None) => {
                return AppError::BadRequest("User not found".into())
                    .into_response::<()>()
                    .into_response();
            }
            Err(_) => {
                tracing::error!(
                    user_id = req.user_id,
                    error_kind = "cloud_user_query_failed",
                    "get_cloud_user_info_cached 查询失败"
                );
                return AppError::Internal("用户数据查询失败，请稍后重试".into())
                    .into_response::<()>()
                    .into_response();
            }
        };

    // 6. 获取会员等级 & 判断是否 VIP
    let is_vip = membership::check_vip_membership(ext_pool, req.user_id)
        .await
        .unwrap_or_else(|_| {
            tracing::error!(
                user_id = req.user_id,
                error_kind = "membership_query_failed",
                "membership::check_vip_membership 查询失败，降级为非VIP"
            );
            false
        });
    tracing::debug!(user_id = req.user_id, is_vip, "用户 VIP 状态");
    // 7. 日访问次数限流（从外部 cc_config 获取配置，Redis 缓存优先）
    let limit_config = membership::get_ai_assistant_chat_limit_config(ext_pool)
        .await
        .unwrap_or_else(|_| {
            tracing::warn!(
                error_kind = "rate_limit_config_query_failed",
                "cc_config 限流配置查询失败，使用默认值"
            );
            None
        })
        .unwrap_or_default();

    let max_times = if is_vip {
        limit_config.vip_ask_times
    } else {
        limit_config.normal_ask_times
    };

    let limit_key = format!("assistant:daily:{}", req.user_id);
    let current_count = match check_and_incr_daily_limit(
        &state.redis,
        &limit_key,
        max_times,
        limit_config.limit_reset_hour,
    )
    .await
    {
        Ok(count) => count,
        Err(Some(msg)) => {
            return AppError::DailyLimitReached(msg)
                .into_response::<()>()
                .into_response();
        }
        Err(None) => {
            return AppError::Internal("Redis unavailable".into())
                .into_response::<()>()
                .into_response();
        }
    };
    // 8. Per-user Assistant request concurrency guard
    if !try_acquire_slot(req.user_id, SseSlotConfig::USER).await {
        rollback_daily_limit(
            &state.redis,
            &limit_key,
            "assistant concurrency slot unavailable",
        )
        .await;
        return AppError::SseConcurrencyExceeded("并发会话过多，请关闭其它对话窗口后重试".into())
            .into_response::<()>()
            .into_response();
    }
    let _guard = SseConcurrencyGuard {
        actor_id: req.user_id,
    };

    // 8. 内部 users 表同步（不存在则创建）
    let (uid, nickname) = (Some(cloud_info.0.as_str()), Some(cloud_info.1.as_str()));

    if let Err(e) = user_auth::ensure_user_exists(&state.pool, req.user_id, uid, nickname).await {
        rollback_daily_limit(&state.redis, &limit_key, "assistant user sync failed").await;
        return AppError::Internal(format!("user sync: {e}"))
            .into_response::<()>()
            .into_response();
    }

    // 10. 获取或创建 session
    let session = match svc::get_or_create_session_user(&state.pool, req.user_id).await {
        Ok(s) => s,
        Err(e) => {
            rollback_daily_limit(&state.redis, &limit_key, "assistant session lookup failed").await;
            return e.into_response::<()>().into_response();
        }
    };
    let session_id = session.id;

    // 12. Append user message
    if let Err(e) =
        svc::append_user_message_user(&state.pool, session_id, req.user_id, &req.message).await
    {
        rollback_daily_limit(
            &state.redis,
            &limit_key,
            "assistant user message persistence failed",
        )
        .await;
        return e.into_response::<()>().into_response();
    }

    // 13. Build OrchestratorDeps + run session (collect full response, return JSON)
    let pool = state.pool.clone();
    let user_content = req.message.clone();

    let history: Vec<crate::models::ChatMessageUser> =
        svc::list_messages_user(&state.pool, session_id)
            .await
            .unwrap_or_default();

    let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel::<Result<Event, Infallible>>();
    let (error_tx, mut error_rx) = tokio::sync::mpsc::unbounded_channel();
    let cancel = Arc::new(std::sync::atomic::AtomicBool::new(false));

    let deps = crate::runtime::orchestrator::OrchestratorDeps {
        execution_context: RuntimeExecutionContext::best_effort(Some(request_id), Some(session_id)),
        pool: pool.clone(),
        redis: state.redis.clone(),
        s3: state.s3.clone(),
        llm: Arc::clone(&state.runtime_state.llm),
        registry: Arc::clone(&state.runtime_state.capabilities),
        invoker: Arc::clone(&state.runtime_state.invoker),
        ext_pool: state.ext_pool.clone(),
        message: req.message.clone(),
        channel: req.channel.clone(),
        client_type: req.client_type.clone(),
        client_version: req.client_version.clone(),
        sensitive_filter: state.sensitive_filter.clone(),
        cancel: Arc::clone(&cancel),
    };
    let handle = tokio::spawn(async move {
        crate::runtime::orchestrator::run_session_user(
            deps,
            session_id,
            1,
            req.user_id,
            history,
            user_content,
            tx,
            error_tx,
        )
        .await
    });
    let _request_child_task = crate::shutdown::AbortTaskOnDrop::new(handle.abort_handle());

    // 客户端断开时取消 orchestrator
    struct CancelGuard(Arc<AtomicBool>);
    impl Drop for CancelGuard {
        fn drop(&mut self) {
            self.0.store(true, std::sync::atomic::Ordering::Relaxed);
        }
    }
    let _guard = CancelGuard(cancel);

    // Drain internal streaming events. Terminal errors travel over a separate typed channel so
    // this synchronous endpoint never has to parse an `Event` debug representation.
    while rx.recv().await.is_some() {}

    // If run_session_user emitted an error, roll back quota and return error
    if let Some(error) = error_rx.recv().await {
        return rollback_quota_and_build_error_response(error, || {
            decr_daily_limit(&state.redis, &limit_key)
        })
        .await;
    }

    // Ensure the spawned task completes and grab the saved ChatMessageUser record
    let saved = match handle.await {
        Ok(msg) => msg,
        Err(e) => {
            rollback_daily_limit(
                &state.redis,
                &limit_key,
                "assistant orchestrator join failed",
            )
            .await;
            return AppError::Internal(format!("orchestrator join: {e}"))
                .into_response::<()>()
                .into_response();
        }
    };

    let mut saved = match saved {
        Some(m) => m,
        None => {
            rollback_daily_limit(
                &state.redis,
                &limit_key,
                "assistant response was not persisted",
            )
            .await;
            return AppError::Internal("assistant message was not persisted".into())
                .into_response::<()>()
                .into_response();
        }
    };

    // 如果已用次数刚好到达 "剩余提醒阈值"，追加 usage extension
    let remaining = max_times - current_count;
    if limit_config.remain_ask_time > 0 && remaining <= limit_config.remain_ask_time {
        let usage_ext = serde_json::json!({
            "content_type": "usage",
            "payload": {
                "used_times": current_count,
                "total_times": max_times,
                "membership_max_times": limit_config.vip_ask_times,
                "remain_ask_time": limit_config.remain_ask_time,
            }
        });
        let mut exts: Vec<serde_json::Value> = saved
            .extensions
            .as_ref()
            .and_then(|v| v.as_array())
            .cloned()
            .unwrap_or_default();
        exts.push(usage_ext);
        saved.extensions = Some(serde_json::Value::Array(exts));
    }

    axum::Json(saved).into_response()
}

fn app_error_from_orchestrator(error: crate::runtime::orchestrator::OrchestratorError) -> AppError {
    if error.message.trim().is_empty() {
        return AppError::Internal("Malformed orchestrator error".into());
    }

    match error.code {
        codes::HOOK_EXECUTION_TIMEOUT => AppError::HookExecutionTimeout(error.message),
        codes::HOOK_BLOCKING_FAILED => AppError::HookBlockingFailed(error.message),
        _ => AppError::Internal(error.message),
    }
}

async fn rollback_quota_and_build_error_response<F, Fut, E>(
    error: crate::runtime::orchestrator::OrchestratorError,
    rollback: F,
) -> Response
where
    F: FnOnce() -> Fut,
    Fut: std::future::Future<Output = Result<(), E>>,
    E: std::fmt::Display,
{
    let orchestrator_code = error.code;
    if rollback().await.is_err() {
        tracing::error!(
            orchestrator_code,
            error_kind = "quota_rollback_failed",
            "assistant quota rollback failed"
        );
    }

    app_error_from_orchestrator(error)
        .into_response::<()>()
        .into_response()
}

// ── 日访问次数限流（Redis） ──

/// Check and increment the daily access counter.
///
/// Returns the current count (after INCR) on success.
/// Returns `Err(Some(msg))` when the daily limit is reached,
/// or `Err(None)` for Redis/infra errors.
async fn check_and_incr_daily_limit(
    redis: &RedisClient,
    key: &str,
    max_times: i64,
    reset_hour: u32,
) -> Result<i64, Option<String>> {
    let mut conn = match redis.get_multiplexed_async_connection().await {
        Ok(c) => c,
        Err(_) => {
            tracing::error!(
                error_kind = "redis_connection_failed",
                "Redis connection failed"
            );
            return Err(None);
        }
    };

    let current: i64 = match conn.incr(key, 1).await {
        Ok(v) => v,
        Err(_) => {
            tracing::error!(error_kind = "redis_increment_failed", "Redis INCR failed");
            return Err(None);
        }
    };

    if current == 1 {
        let secs = seconds_until_reset_hour(reset_hour);
        let _: Result<(), _> = conn.expire(key, secs as i64).await;
    }

    if current > max_times {
        // rollback: 超限不计入
        if conn.decr::<_, _, i64>(key, 1).await.is_err() {
            tracing::error!(
                error_kind = "quota_rollback_failed",
                "assistant over-limit quota rollback failed"
            );
        }
        let max_times = max_times - 1;
        return Err(Some(format!(
            "Daily limit reached ({}/{})",
            max_times, max_times
        )));
    }

    Ok(current)
}

/// 配额回滚：LLM 调用失败或内部错误时 DECR 计数器
async fn decr_daily_limit(redis: &RedisClient, key: &str) -> Result<(), String> {
    let mut conn = redis
        .get_multiplexed_async_connection()
        .await
        .map_err(|error| format!("Redis DECR connection failed: {error}"))?;
    conn.decr::<_, _, i64>(key, 1)
        .await
        .map(|_| ())
        .map_err(|error| format!("Redis DECR failed: {error}"))
}

async fn rollback_daily_limit(redis: &RedisClient, key: &str, reason: &'static str) {
    if decr_daily_limit(redis, key).await.is_err() {
        tracing::error!(
            reason,
            error_kind = "quota_rollback_failed",
            "assistant quota rollback failed"
        );
    }
}

/// 计算到指定重置时间点剩余的秒数（UTC）。
///
/// `reset_hour` 取值 0-23，表示每天在该小时（UTC）重置计数器。
/// 如果当前时间已经过了今天的重置点，则计算到明天的重置点。
fn seconds_until_reset_hour(reset_hour: u32) -> u64 {
    let now = chrono::Utc::now();
    let naive_now = now.naive_utc();
    let today_reset = naive_now
        .date()
        .and_hms_opt(reset_hour, 0, 0)
        .unwrap_or_else(|| {
            // Fallback: midnight if hour is out of range
            naive_now.date().and_hms_opt(0, 0, 0).unwrap()
        });
    let target = if naive_now >= today_reset {
        // Already past today's reset → next reset is tomorrow
        today_reset + chrono::Duration::days(1)
    } else {
        today_reset
    };
    let dur = target - naive_now;
    dur.num_seconds().max(60) as u64
}

/// 查询用户当日配额使用情况（只读，不消耗配额）。
///
/// GET /api/quota?sign={md5}&user_id={uid}
///
/// 返回 `used_times`、`total_times`、`membership_max_times`。
async fn assistant_quota(
    State(state): State<AppState>,
    Query(params): Query<HashMap<String, String>>,
) -> Response {
    // 1. MD5 签名校验
    let secret = get_secret();
    if !secret.is_empty() {
        let sign = params.get("sign").map(|s| s.as_str()).unwrap_or("");
        let raw_body = format!("user_id={}", params.get("user_id").unwrap_or(&"".into()));
        if !chat_common::verify_sign(secret, "/api/quota", &raw_body, sign) {
            return AppError::BadRequest("Invalid signature".into())
                .into_response::<()>()
                .into_response();
        }
    }

    // 2. 解析 user_id
    let user_id: i64 = match params.get("user_id").and_then(|v| v.parse().ok()) {
        Some(uid) if uid > 0 => uid,
        _ => {
            return AppError::BadRequest("user_id must be positive".into())
                .into_response::<()>()
                .into_response();
        }
    };

    // 3. 获取外部 DB 连接
    let ext_pool = match &state.ext_pool {
        Some(p) => p,
        None => {
            return AppError::Internal("Service unavailable".into())
                .into_response::<()>()
                .into_response();
        }
    };

    // 4. 校验 user_id 是否存在于外部 cloud_user 表
    match membership::get_cloud_user_info_cached(&state.redis, ext_pool, user_id).await {
        Ok(Some(_)) => {}
        Ok(None) => {
            return AppError::BadRequest("User not found".into())
                .into_response::<()>()
                .into_response();
        }
        Err(_) => {
            tracing::error!(
                user_id,
                error_kind = "cloud_user_query_failed",
                "quota: get_cloud_user_info_cached failed"
            );
            return AppError::Internal("Query failed".into())
                .into_response::<()>()
                .into_response();
        }
    }

    // 5. 获取会员等级 & 限流配置
    let is_vip = membership::check_vip_membership(ext_pool, user_id)
        .await
        .unwrap_or_else(|_| {
            tracing::error!(
                user_id,
                error_kind = "membership_query_failed",
                "quota: check_vip_membership failed"
            );
            false
        });
    tracing::debug!(user_id, is_vip, "quota: user VIP status");
    let limit_config = membership::get_ai_assistant_chat_limit_config(ext_pool)
        .await
        .unwrap_or_else(|_| {
            tracing::warn!(
                error_kind = "rate_limit_config_query_failed",
                "quota: limit config failed, using default"
            );
            None
        })
        .unwrap_or_default();

    let total_times = if is_vip {
        limit_config.vip_ask_times
    } else {
        limit_config.normal_ask_times
    };

    // 6. 读取当前 Redis 计数器（只读，不 INCR）
    let limit_key = format!("assistant:daily:{}", user_id);
    let used_times = read_daily_limit(&state.redis, &limit_key).await;

    crate::utils::error::ApiResponse::success(serde_json::json!({
        "used_times": used_times,
        "total_times": total_times,
        "membership_max_times": limit_config.vip_ask_times,
        "remain_ask_time": limit_config.remain_ask_time,
    }))
    .into_response()
}

/// 读取 Redis 当日计数器（GET，不增加）。
async fn read_daily_limit(redis: &RedisClient, key: &str) -> i64 {
    let mut conn = match redis.get_multiplexed_async_connection().await {
        Ok(c) => c,
        Err(_) => {
            tracing::error!(
                error_kind = "redis_connection_failed",
                "Redis GET connection failed"
            );
            return 0;
        }
    };
    let ret: Result<i64, _> = conn.get(key).await;
    ret.unwrap_or(0)
}

// ── 测试 ──

#[cfg(test)]
mod tests {
    use super::*;
    use axum::http::StatusCode;
    use http_body_util::BodyExt;
    use std::sync::atomic::{AtomicUsize, Ordering};

    async fn response_json(response: Response) -> serde_json::Value {
        let bytes = response
            .into_body()
            .collect()
            .await
            .expect("response body should be readable")
            .to_bytes();
        serde_json::from_slice(&bytes).expect("response body should be JSON")
    }

    #[tokio::test]
    async fn hook_timeout_response_preserves_6004_and_rolls_back_once() {
        let rollback_count = Arc::new(AtomicUsize::new(0));
        let count = Arc::clone(&rollback_count);
        let response = rollback_quota_and_build_error_response(
            crate::runtime::orchestrator::OrchestratorError {
                code: codes::HOOK_EXECUTION_TIMEOUT,
                message: "Hook「slow-hook」阻塞模式执行超时".into(),
            },
            move || async move {
                count.fetch_add(1, Ordering::SeqCst);
                Ok::<_, &'static str>(())
            },
        )
        .await;

        assert_eq!(response.status(), StatusCode::REQUEST_TIMEOUT);
        let body = response_json(response).await;
        assert_eq!(body["code"], codes::HOOK_EXECUTION_TIMEOUT);
        assert_eq!(body["message"], "Hook「slow-hook」阻塞模式执行超时");
        assert_eq!(rollback_count.load(Ordering::SeqCst), 1);
    }

    #[tokio::test]
    async fn hook_blocking_failure_response_preserves_6005_without_nested_json() {
        let rollback_count = Arc::new(AtomicUsize::new(0));
        let count = Arc::clone(&rollback_count);
        let response = rollback_quota_and_build_error_response(
            crate::runtime::orchestrator::OrchestratorError {
                code: codes::HOOK_BLOCKING_FAILED,
                message: "Hook「guard-hook」阻塞模式执行失败".into(),
            },
            move || async move {
                count.fetch_add(1, Ordering::SeqCst);
                Ok::<_, &'static str>(())
            },
        )
        .await;

        assert_eq!(response.status(), StatusCode::INTERNAL_SERVER_ERROR);
        let body = response_json(response).await;
        assert_eq!(body["code"], codes::HOOK_BLOCKING_FAILED);
        assert_eq!(body["message"], "Hook「guard-hook」阻塞模式执行失败");
        assert!(!body["message"].as_str().unwrap().starts_with('{'));
        assert_eq!(rollback_count.load(Ordering::SeqCst), 1);
    }

    #[tokio::test]
    async fn malformed_orchestrator_error_falls_back_to_5000_and_rolls_back_once() {
        let rollback_count = Arc::new(AtomicUsize::new(0));
        let count = Arc::clone(&rollback_count);
        let response = rollback_quota_and_build_error_response(
            crate::runtime::orchestrator::OrchestratorError {
                code: codes::HOOK_BLOCKING_FAILED,
                message: "  ".into(),
            },
            move || async move {
                count.fetch_add(1, Ordering::SeqCst);
                Err::<(), _>("simulated Redis failure")
            },
        )
        .await;

        assert_eq!(response.status(), StatusCode::INTERNAL_SERVER_ERROR);
        let body = response_json(response).await;
        assert_eq!(body["code"], codes::INTERNAL);
        assert_eq!(body["message"], "Internal server error");
        assert_ne!(body["message"], "simulated Redis failure");
        assert_eq!(rollback_count.load(Ordering::SeqCst), 1);
    }

    // ── T025: 签名校验（纯函数，无需外部依赖） ──

    #[test]
    fn verify_sign_matches_correct() {
        let secret = "abc123";
        let body = r#"{"user_id":123,"message":"hello"}"#;
        let sign_string = format!("{}/api/assistant?body={}", secret, body);
        let expected = format!("{:x}", md5::compute(sign_string.as_bytes()));
        assert!(chat_common::verify_sign(
            secret,
            "/api/assistant",
            body,
            &expected
        ));
    }

    #[test]
    fn verify_sign_rejects_wrong_signature() {
        let secret = "abc123";
        let body = r#"{"user_id":123,"message":"hello"}"#;
        assert!(!chat_common::verify_sign(
            secret,
            "/api/assistant",
            body,
            "wrong_sign"
        ));
    }

    #[test]
    fn verify_sign_rejects_tampered_body() {
        let secret = "abc123";
        let body_tampered = r#"{"user_id":999,"message":"hack"}"#;
        // sign for a different body
        let original_body = r#"{"user_id":123,"message":"hello"}"#;
        let sign_string = format!("{}/api/assistant?body={}", secret, original_body);
        let sign = format!("{:x}", md5::compute(sign_string.as_bytes()));
        assert!(!chat_common::verify_sign(
            secret,
            "/api/assistant",
            body_tampered,
            &sign
        ));
    }

    #[test]
    fn verify_sign_empty_sign_fails() {
        assert!(!chat_common::verify_sign(
            "secret",
            "/api/assistant",
            "body",
            ""
        ));
    }

    #[test]
    fn verify_sign_case_sensitive() {
        let secret = "abc123";
        let body = r#"{"user_id":123,"message":"hello"}"#;
        let sign_string = format!("{}/api/assistant?body={}", secret, body);
        let sign = format!("{:x}", md5::compute(sign_string.as_bytes()));
        // uppercase should not match
        assert!(!chat_common::verify_sign(
            secret,
            "/api/assistant",
            body,
            &sign.to_uppercase()
        ));
    }

    // ── T033: 日限流 TTL 计算 ──

    #[test]
    fn seconds_until_reset_hour_is_positive() {
        let secs = seconds_until_reset_hour(0);
        assert!(secs > 0, "TTL must be positive, got {}", secs);
    }

    #[test]
    fn seconds_until_reset_hour_does_not_exceed_24h() {
        let secs = seconds_until_reset_hour(23);
        assert!(
            secs <= 86400,
            "TTL must not exceed 24h (86400s), got {}",
            secs
        );
    }

    // ── T027: 参数校验（user_id） ──

    #[test]
    fn user_id_zero_should_fail_validation() {
        let user_id: i64 = 0;
        assert!(user_id <= 0, "user_id=0 must be rejected");
    }

    #[test]
    fn user_id_negative_should_fail_validation() {
        let user_id: i64 = -1;
        assert!(user_id <= 0, "user_id=-1 must be rejected");
    }

    #[test]
    fn user_id_positive_should_pass_validation() {
        let user_id: i64 = 1;
        assert!(user_id > 0, "user_id=1 must pass validation");
    }

    // ── T028: 参数校验（message） ──

    #[test]
    fn empty_message_should_fail() {
        let message = "";
        assert!(message.is_empty(), "empty message must be rejected");
    }

    #[test]
    fn non_empty_message_should_pass() {
        let message = "hello";
        assert!(
            !message.is_empty(),
            "non-empty message must pass validation"
        );
    }

    // ── JSON 序列化 ──

    #[test]
    fn assistant_request_deserializes_correctly() {
        let body = r#"{"user_id":42,"message":"hi","channel":"app","client_type":"android","client_version":"1.0.0"}"#;
        let req: AssistantRequest = serde_json::from_str(body).unwrap();
        assert_eq!(req.user_id, 42);
        assert_eq!(req.message, "hi");
        assert_eq!(req.channel, "app");
        assert_eq!(req.client_type, "android");
        assert_eq!(req.client_version, "1.0.0");
    }

    #[test]
    fn assistant_request_rejects_missing_field() {
        // missing 'message' field
        let body = r#"{"user_id":42}"#;
        assert!(serde_json::from_str::<AssistantRequest>(body).is_err());
    }

    // ── 集成测试（需要外部依赖，默认 skip） ──
    // 运行方式：cargo test --features integration -- --ignored
    // 或设置完整环境变量后：cargo test -- --include-ignored

    #[tokio::test]
    #[ignore = "requires MySQL + Redis + external DB + LLM"]
    async fn integration_successful_request() {
        // T024: 合法用户成功请求
        // 需要：DATABASE_URL、Redis 直连或 Sentinel 配置、EXTERNAL_DB_URL、ASSISTANT_SECRET
        // 以及 agents 表中 id=1 的 Main Agent
    }

    #[tokio::test]
    #[ignore = "requires MySQL + Redis"]
    async fn integration_user_not_found() {
        // T026: 用户不存在于 cloud_user
    }

    #[tokio::test]
    #[ignore = "requires MySQL + Redis + setup"]
    async fn integration_daily_limit_exceeded() {
        // T029: 限流超额
    }

    #[tokio::test]
    #[ignore = "requires external DB setup"]
    async fn integration_ext_db_unavailable() {
        // T030: 外部 DB 不可用
    }

    #[tokio::test]
    #[ignore = "requires MySQL"]
    async fn integration_user_sync() {
        // T031: 内部用户同步
    }

    #[tokio::test]
    #[ignore = "requires full infra"]
    async fn integration_llm_failure_quota_rollback() {
        // T032: LLM 失败配额回滚
    }

    #[tokio::test]
    #[ignore = "requires full infra"]
    async fn integration_cross_midnight_reset() {
        // T034: 跨天重置
    }

    #[tokio::test]
    #[ignore = "requires full infra"]
    async fn integration_concurrent_rate_limit() {
        // T035: 并发限流准确性
    }
}
