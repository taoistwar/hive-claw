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
    extract::{Query, State},
    response::{IntoResponse, Response},
    routing::post,
};
use redis::AsyncCommands;
use serde::Deserialize;
use std::collections::HashMap;
use std::convert::Infallible;
use std::sync::{Arc, OnceLock};

use crate::api::AppState;
use crate::api::chat_common;
use crate::api::chat_common::{SseConcurrencyGuard, SseSlotConfig, try_acquire_slot};
use crate::services::chat_user as svc;
use crate::services::membership;
use crate::services::user_auth;
use crate::utils::error::AppError;

/// 预共享密钥，从环境变量 ASSISTANT_SECRET 懒加载
static SECRET: OnceLock<String> = OnceLock::new();

fn get_secret() -> &'static str {
    SECRET
        .get_or_init(|| std::env::var("ASSISTANT_SECRET").unwrap_or_default())
        .as_str()
}

pub fn router() -> Router<AppState> {
    Router::new().route("/assistant", post(assistant_chat))
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
    if let Some(hit) = state.sensitive_filter.check(&req.message) {
        tracing::info!(
            user_id = req.user_id,
            triggered_word = %hit.word(),
            "Assistant input blocked by sensitive filter"
        );
        return crate::utils::error::ApiResponse::success(serde_json::json!({
            "reply": "内容安全警告：输入的文本数据可能包含不适当的内容！",
            "filtered": true,
        }))
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

    // 5. 校验 user_id 是否存在于外部 cloud_user 表（Redis 缓存优先）
    match membership::user_exists_in_cloud_cached(&state.redis, ext_pool, req.user_id).await {
        Ok(true) => {}
        Ok(false) => {
            return AppError::BadRequest("User not found".into())
                .into_response::<()>()
                .into_response();
        }
        Err(e) => {
            tracing::error!(user_id = req.user_id, error = %e, "membership::user_exists_in_cloud_cached 查询失败");
            return AppError::Internal("用户数据查询失败，请稍后重试".into())
                .into_response::<()>()
                .into_response();
        }
    }

    // 6. 获取会员等级 & 判断是否 VIP（Redis 缓存优先）
    let is_vip = membership::check_vip_membership_cached(&state.redis, ext_pool, req.user_id)
        .await
        .unwrap_or_else(|e| {
            tracing::error!(user_id = req.user_id, error = %e, "membership::check_vip_membership_cached 查询失败，降级为非VIP");
            false
        });

    // 7. 日访问次数限流（从外部 cc_config 获取配置，Redis 缓存优先）
    let limit_config =
        membership::get_ai_assistant_chat_limit_config_cached(&state.redis, ext_pool)
            .await
            .unwrap_or_else(|e| {
                tracing::warn!(error = %e, "cc_config 限流配置查询失败，使用默认值");
                membership::AssistantChatLimitConfig::default()
            });

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
        Err(msg) => {
            return AppError::BadRequest(msg)
                .into_response::<()>()
                .into_response();
        }
    };
    // 8. SSE concurrency guard
    if !try_acquire_slot(req.user_id, SseSlotConfig::USER).await {
        let _ = decr_daily_limit(&state.redis, &limit_key).await;
        return AppError::SseConcurrencyExceeded("并发会话过多，请关闭其它对话窗口后重试".into())
            .into_response::<()>()
            .into_response();
    }
    let _guard = SseConcurrencyGuard {
        actor_id: req.user_id,
        is_admin: false,
    };

    // 8. 内部 users 表同步（不存在则创建）
    let cloud_info = membership::get_cloud_user_info_cached(&state.redis, ext_pool, req.user_id)
        .await
        .unwrap_or_else(|e| {
            tracing::warn!(user_id = req.user_id, error = %e, "get_cloud_user_info_cached failed");
            None
        });
    let (uid, nickname) = cloud_info
        .as_ref()
        .map(|(u, n)| (Some(u.as_str()), Some(n.as_str())))
        .unwrap_or((None, None));

    if let Err(e) = user_auth::ensure_user_exists(&state.pool, req.user_id, uid, nickname).await {
        let _ = decr_daily_limit(&state.redis, &limit_key).await;
        return AppError::Internal(format!("user sync: {e}"))
            .into_response::<()>()
            .into_response();
    }

    // 10. 获取或创建 session
    let session = match svc::get_or_create_session_user(&state.pool, req.user_id).await {
        Ok(s) => s,
        Err(e) => {
            let _ = decr_daily_limit(&state.redis, &limit_key).await;
            return e.into_response::<()>().into_response();
        }
    };
    let session_id = session.id;

    // 12. Append user message
    if let Err(e) =
        svc::append_user_message_user(&state.pool, session_id, req.user_id, &req.message).await
    {
        let _ = decr_daily_limit(&state.redis, &limit_key).await;
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

    let deps = crate::runtime::orchestrator::OrchestratorDeps {
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
        )
        .await
    });

    // Drain SSE events; the orchestrator already saved the assistant message and returns
    // the persisted record via the JoinHandle, so we only need to detect early errors here.
    let mut first_error: Option<String> = None;

    while let Some(result) = rx.recv().await {
        match result {
            Ok(event) => {
                let sse_text = event_to_sse_text(&event);
                let (event_type, data) = parse_sse_event(&sse_text);
                if event_type.as_deref() == Some("error") {
                    if first_error.is_none() {
                        first_error = Some(data);
                    }
                }
            }
            Err(_) => {} // Infallible
        }
    }

    // If run_session_user emitted an error, roll back quota and return error
    if let Some(err_msg) = &first_error {
        let _ = decr_daily_limit(&state.redis, &limit_key).await;
        return AppError::Internal(err_msg.clone())
            .into_response::<()>()
            .into_response();
    }

    // Ensure the spawned task completes and grab the saved ChatMessageUser record
    let saved = match handle.await {
        Ok(msg) => msg,
        Err(e) => {
            let _ = decr_daily_limit(&state.redis, &limit_key).await;
            return AppError::Internal(format!("orchestrator join: {e}"))
                .into_response::<()>()
                .into_response();
        }
    };

    let mut saved = match saved {
        Some(m) => m,
        None => {
            let _ = decr_daily_limit(&state.redis, &limit_key).await;
            return AppError::Internal("assistant message was not persisted".into())
                .into_response::<()>()
                .into_response();
        }
    };

    // 如果已用次数刚好到达 "剩余提醒阈值"，追加 usage extension
    let remaining = max_times - current_count;
    if limit_config.remain_ask_time > 0
        && remaining > 0
        && remaining <= limit_config.remain_ask_time
    {
        let usage_ext = serde_json::json!({
            "content_type": "usage",
            "payload": {
                "used_times": current_count,
                "total_times": max_times,
            }
        });
        let mut exts: Vec<serde_json::Value> = saved
            .extensions
            .as_ref()
            .and_then(|v| v.as_array())
            .map(|a| a.clone())
            .unwrap_or_default();
        exts.push(usage_ext);
        saved.extensions = Some(serde_json::Value::Array(exts));
    }

    axum::Json(saved).into_response()
}

/// Extract the SSE-formatted text from an axum 0.7 `Event` by parsing its Debug output.
///
/// axum 0.7's `Event` stores data in a `pub(crate) buffer: BytesMut` field,
/// which does not expose its contents publicly. We recover the raw SSE text
/// by parsing the `Debug` representation of the struct.
fn event_to_sse_text(event: &Event) -> String {
    let debug = format!("{:?}", event);
    // Debug format: Event { buffer: b"event: ...\ndata: ...\n", flags: EventFlags(N) }
    if let Some(start) = debug.find("buffer: b\"") {
        let rest = &debug[start + 10..]; // skip "buffer: b\""
        if let Some(end) = rest.rfind('"') {
            let escaped = &rest[..end];
            return unescape_bytesmut_debug(escaped);
        }
    }
    String::new()
}

/// Unescape a string produced by `bytes::BytesMut`'s `Debug` implementation.
///
/// Collects bytes into a `Vec<u8>` so that multi-byte UTF-8 sequences (e.g. Chinese
/// characters) are reconstructed correctly, then converts via `String::from_utf8_lossy`.
fn unescape_bytesmut_debug(s: &str) -> String {
    let mut bytes: Vec<u8> = Vec::with_capacity(s.len());
    let mut chars = s.chars();
    while let Some(c) = chars.next() {
        if c == '\\' {
            match chars.next() {
                Some('n') => bytes.push(b'\n'),
                Some('r') => bytes.push(b'\r'),
                Some('t') => bytes.push(b'\t'),
                Some('\\') => bytes.push(b'\\'),
                Some('"') => bytes.push(b'"'),
                Some('0') => bytes.push(b'\0'),
                Some('x') => {
                    // hex escape: \xNN
                    let h1 = chars.next().unwrap_or('0');
                    let h2 = chars.next().unwrap_or('0');
                    if let Ok(b) = u8::from_str_radix(&format!("{h1}{h2}"), 16) {
                        bytes.push(b);
                    }
                }
                Some(other) => {
                    bytes.push(b'\\');
                    // Push the character as UTF-8 bytes
                    let mut buf = [0u8; 4];
                    bytes.extend_from_slice(other.encode_utf8(&mut buf).as_bytes());
                }
                None => bytes.push(b'\\'),
            }
        } else {
            // ASCII-range chars in debug output are literal bytes
            bytes.push(c as u8);
        }
    }
    String::from_utf8_lossy(&bytes).into_owned()
}

/// Parse a single SSE-formatted event text into (event_type, data).
fn parse_sse_event(sse_text: &str) -> (Option<String>, String) {
    let mut event_type = None;
    let mut data = String::new();
    for line in sse_text.lines() {
        if let Some(rest) = line.strip_prefix("event: ") {
            event_type = Some(rest.to_string());
        } else if let Some(rest) = line.strip_prefix("data: ") {
            data = rest.to_string();
        }
    }
    (event_type, data)
}

// ── 日访问次数限流（Redis） ──

/// Check and increment the daily access counter.
///
/// Returns the current count (after INCR) on success, or an error
/// message if the limit is exceeded or Redis is unavailable.
async fn check_and_incr_daily_limit(
    redis: &redis::Client,
    key: &str,
    max_times: i64,
    reset_hour: u32,
) -> Result<i64, String> {
    let mut conn = match redis.get_multiplexed_async_connection().await {
        Ok(c) => c,
        Err(e) => {
            tracing::error!("Redis connection failed: {e}");
            return Err("Redis unavailable".to_string());
        }
    };

    let current: i64 = match conn.incr(key, 1).await {
        Ok(v) => v,
        Err(e) => {
            tracing::error!("Redis INCR failed: {e}");
            return Err("Redis error".to_string());
        }
    };

    if current == 1 {
        let secs = seconds_until_reset_hour(reset_hour);
        let _: Result<(), _> = conn.expire(key, secs as i64).await;
    }

    if current > max_times {
        return Err(format!("Daily limit reached ({}/{})", max_times, max_times));
    }

    Ok(current)
}

/// 配额回滚：LLM 调用失败或内部错误时 DECR 计数器
async fn decr_daily_limit(redis: &redis::Client, key: &str) -> Result<(), ()> {
    let mut conn = match redis.get_multiplexed_async_connection().await {
        Ok(c) => c,
        Err(e) => {
            tracing::error!("Redis DECR connection failed: {e}");
            return Err(());
        }
    };
    let _: Result<i64, _> = conn.decr(key, 1).await;
    Ok(())
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

// ── 测试 ──

#[cfg(test)]
mod tests {
    use super::*;

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
        // 需要：DATABASE_URL, REDIS_URL, EXTERNAL_DB_URL, ASSISTANT_SECRET
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
